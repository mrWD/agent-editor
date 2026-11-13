//! Starting the whole core and stopping it again.
//!
//! Two programs start the core: `ae-core`, which runs it on its own, and the
//! desktop app, which runs it inside its own process next to the window. Both
//! come through here, so the core starts and stops the same way in both.

use crate::agents::Registry;
use crate::server::{self, AppState};
use crate::store::Store;
use crate::tasks::Manager;
use anyhow::{Context, Result};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// The port `ae-core` listens on when it is not told another.
pub const DEFAULT_PORT: u16 = 4477;

/// Which port the core listens on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Port {
    /// This port and no other. Starting fails when it is taken.
    Exactly(u16),
    /// Any port that is free. `prefer` is tried first: the interface keeps what
    /// it remembers under its address, so the same port as last time keeps it.
    Free { prefer: Option<u16> },
}

/// Things only a desktop app can do, lent to the core by the app it runs in.
#[derive(Clone)]
pub struct Desktop {
    /// Known only to the app's own window. A page in a browser can reach the
    /// core too, and it must not be able to open dialogs on the user's screen.
    pub key: String,
    /// Shows the system's "choose a folder" dialog and waits for the answer.
    /// `None` means the user closed it without choosing. Called off the main thread.
    pub pick_folder: Arc<dyn Fn() -> Option<PathBuf> + Send + Sync>,
    /// Shows a notification of the system: something happened while no window of
    /// the editor was in front. The core decides when; this only shows it, and
    /// must come back at once. It is called with the app's window closed too.
    pub notify: Arc<dyn Fn(&crate::notify::Notice) + Send + Sync>,
}

/// How to start the core.
#[derive(Clone)]
pub struct Options {
    /// Address to listen on. Keep 127.0.0.1 unless another device must connect.
    pub host: IpAddr,
    pub port: Port,
    /// Folder with the built interface. Without it only the API is served.
    pub ui_dir: Option<PathBuf>,
    /// Where projects, tasks and working copies are kept. `None` means `default_data_dir`.
    pub data_dir: Option<PathBuf>,
    /// Set by the desktop app; `None` when the core runs on its own.
    pub desktop: Option<Desktop>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: Port::Exactly(DEFAULT_PORT),
            ui_dir: None,
            data_dir: None,
            desktop: None,
        }
    }
}

/// Where the core keeps its data when it is not told another folder:
/// `agent-editor` in the system's folder for application data.
pub fn default_data_dir() -> Result<PathBuf> {
    Ok(dirs::data_dir()
        .context("This system has no folder for application data; pass --data-dir")?
        .join("agent-editor"))
}

/// Prints the core's log lines to standard output, where `ae-core` has always printed them.
/// `RUST_LOG` picks what is logged; without it, "info".
pub fn log_to_stdout() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
}

/// A core that is running.
pub struct Core {
    addr: SocketAddr,
    state: AppState,
    stop: oneshot::Sender<()>,
    served: JoinHandle<Result<()>>,
}

impl Core {
    /// The address the core listens on. With `Port::Free` this is where the port is learned.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The address as the interface opens it.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The folder the core keeps its data in, as a full path.
    pub fn data_dir(&self) -> &Path {
        self.state.manager.data_dir()
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// Stops serving, then stops every terminal and every agent the core started.
    pub async fn stop(self) -> Result<()> {
        // The server may have ended by itself already; then nobody listens for this.
        let _ = self.stop.send(());
        self.served.await?
    }
}

/// Opens the data folder, puts the stored history right and starts serving.
/// Returns as soon as the core answers on its address.
pub async fn start(options: Options) -> Result<Core> {
    let data_dir = match &options.data_dir {
        Some(dir) => dir.clone(),
        None => default_data_dir()?,
    };
    std::fs::create_dir_all(&data_dir).with_context(|| format!("Cannot create {}", data_dir.display()))?;
    let data_dir = crate::paths::real_path(&data_dir)?;
    tracing::info!("Data is kept in {}", data_dir.display());

    let store = Store::open(&data_dir.join("agent-editor.db"))?;
    let registry = Registry::detect();
    for agent in registry.list() {
        tracing::info!("Agent {}: {}", agent.id, if agent.available { "available" } else { "not available" });
    }

    let manager = Manager::new(registry, store, data_dir);
    manager.recover()?;
    serve(manager, &options).await
}

/// Starts serving a manager that is ready. `start` ends here; tests come in
/// here with a manager that can reach only stand-ins.
pub async fn serve(manager: Arc<Manager>, options: &Options) -> Result<Core> {
    let listener = listen(options.host, options.port).await?;
    let addr = listener.local_addr()?;
    tracing::info!("Core is listening on http://{addr}");

    let mut state = AppState::listening_on(manager, Some(addr.port()));
    state.desktop = options.desktop.clone();
    if let Some(desktop) = &options.desktop {
        state.manager.notify_through(desktop.notify.clone());
    }
    let (stop, stopped) = oneshot::channel::<()>();
    let served = tokio::spawn(server::serve_until(listener, state.clone(), options.ui_dir.clone(), async {
        // A dropped sender counts as "stop" too: whoever held the core is gone.
        let _ = stopped.await;
    }));
    Ok(Core { addr, state, stop, served })
}

async fn listen(host: IpAddr, port: Port) -> Result<TcpListener> {
    let taken = |port: u16| format!("Cannot listen on {host}:{port}. Is another core running there?");
    match port {
        Port::Exactly(port) => TcpListener::bind(SocketAddr::new(host, port)).await.with_context(|| taken(port)),
        Port::Free { prefer } => {
            if let Some(port) = prefer
                && let Ok(listener) = TcpListener::bind(SocketAddr::new(host, port)).await
            {
                return Ok(listener);
            }
            // Port 0 asks the system for any port nobody uses.
            TcpListener::bind(SocketAddr::new(host, 0)).await.with_context(|| format!("Cannot listen on {host}"))
        }
    }
}

/// Runs the core until the process is asked to stop (Ctrl+C), then stops its agents and terminals.
pub async fn run(options: Options) -> Result<()> {
    let mut core = start(options).await?;
    tokio::select! {
        _ = tokio::signal::ctrl_c() => core.stop().await,
        // The server does not end by itself unless something is badly wrong; pass that on.
        ended = &mut core.served => ended?,
    }
}
