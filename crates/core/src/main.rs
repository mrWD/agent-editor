use ae_core::agents::Registry;
use ae_core::server::{self, AppState};
use ae_core::store::Store;
use ae_core::tasks::Manager;
use anyhow::{Context, Result};
use clap::Parser;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

/// The core of Agent Editor. Start it, then open the interface.
#[derive(Parser)]
#[command(name = "ae-core", version)]
struct Args {
    /// Port to listen on.
    #[arg(long, default_value_t = 4477, env = "AE_PORT")]
    port: u16,

    /// Address to listen on. Keep 127.0.0.1 unless another device must connect.
    #[arg(long, default_value = "127.0.0.1", env = "AE_HOST")]
    host: IpAddr,

    /// Folder with the built interface. Without it only the API is served.
    #[arg(long, env = "AE_UI_DIR")]
    ui_dir: Option<PathBuf>,

    /// Where projects, tasks and working copies are kept.
    /// Defaults to the system's folder for application data.
    #[arg(long, env = "AE_DATA_DIR")]
    data_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let data_dir = match args.data_dir {
        Some(dir) => dir,
        None => dirs::data_dir()
            .context("This system has no folder for application data; pass --data-dir")?
            .join("agent-editor"),
    };
    std::fs::create_dir_all(&data_dir).with_context(|| format!("Cannot create {}", data_dir.display()))?;
    let data_dir = data_dir.canonicalize()?;
    tracing::info!("Data is kept in {}", data_dir.display());

    let store = Store::open(&data_dir.join("agent-editor.db"))?;
    let registry = Registry::detect();
    for agent in registry.list() {
        tracing::info!("Agent {}: {}", agent.id, if agent.available { "available" } else { "not available" });
    }

    let manager = Manager::new(registry, store, data_dir);
    manager.recover()?;
    let state = AppState::listening_on(manager, Some(args.port));
    server::serve(state, SocketAddr::new(args.host, args.port), args.ui_dir).await
}
