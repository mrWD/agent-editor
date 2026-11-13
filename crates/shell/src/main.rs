//! The desktop app of Agent Editor.
//!
//! One process holds everything. The core is started here as a library, on a
//! free port of this computer, and a window shows the interface the core
//! serves. It is the very interface a browser gets from `ae-core`: the window
//! adds what only an app can give, such as a folder dialog and a menu.
//!
//! Quitting the app stops the core, and with it every agent and terminal.

// A release build on Windows is a window app: without this a console window would open next to it.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod environment;
mod links;
mod menu;
mod settings;
mod window;
mod zoom;

use ae_core::{Desktop, Options, Port};
use anyhow::{Context, Result, bail};
use settings::Settings;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Manager, RunEvent, Url};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tauri_plugin_window_state::StateFlags;

/// What the app keeps for as long as it runs.
pub struct Shell {
    /// The running core. Taken out when the app quits, to stop it.
    core: Mutex<Option<ae_core::Core>>,
    /// Where the core serves the interface.
    pub address: Url,
    /// Handed to the app's window and to nobody else; see `ae_core::Desktop`.
    pub key: String,
    /// Where the app keeps what it remembers between starts.
    pub data_dir: PathBuf,
    /// How large the interface is drawn; 1.0 is the size as designed. See `zoom`.
    pub zoom: Mutex<f64>,
}

/// How long quitting waits for the core to stop its agents and terminals.
/// They are stopped by force, so this is plenty; the limit only keeps a quit from hanging.
const STOP_WITHIN: Duration = Duration::from_secs(10);

fn main() {
    ae_core::log_to_stdout();
    // Before anything else starts a thread: this changes the environment of the process.
    environment::adopt_shell_path();
    let settings = Settings::from_process();

    let app = tauri::Builder::default()
        // Registered first on purpose. A second launch ends inside this plugin, before
        // it could start a second core on the same data, and the first one shows its window.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| window::show(app)))
        .plugin(tauri_plugin_dialog::init())
        // Links are sent out by the window itself (see `window::open`), not by a script in the page.
        .plugin(tauri_plugin_opener::Builder::new().open_js_links_on_click(false).build())
        // Size, place and "maximized" are remembered. "Hidden" is not: the window must always come back visible.
        .plugin(
            tauri_plugin_window_state::Builder::new().with_state_flags(StateFlags::all() - StateFlags::VISIBLE).build(),
        )
        .menu(menu::build)
        .on_menu_event(menu::on_event)
        .setup(move |app| {
            let app = app.handle().clone();
            if let Err(error) = start(&app, &settings) {
                tracing::error!("The app could not start: {error:#}");
                // There is no window to say it in, so it is said in a dialog. Then the app quits.
                app.dialog()
                    .message(format!("{error:#}"))
                    .title("Agent Editor could not start")
                    .kind(MessageDialogKind::Error)
                    .show({
                        let app = app.clone();
                        move |_| app.exit(1)
                    });
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("the app could not be set up");

    app.run(|app, event| {
        // On a Mac, closing the window does not quit the app: the agents go on working,
        // and a click on the Dock icon brings the window back. `code` is `None` only when
        // the last window was closed; a real quit always gets through.
        #[cfg(target_os = "macos")]
        match &event {
            RunEvent::ExitRequested { code: None, api, .. } => api.prevent_exit(),
            RunEvent::Reopen { has_visible_windows: false, .. } => window::show(app),
            _ => {}
        }
        if matches!(event, RunEvent::Exit) {
            stop(app);
        }
    });
}

/// Starts the core and opens the window on it.
fn start(app: &AppHandle, settings: &Settings) -> Result<()> {
    let data_dir = match &settings.data_dir {
        Some(dir) => dir.clone(),
        None => ae_core::default_data_dir()?,
    };
    let ui_dir = match &settings.ui_dir {
        Some(dir) => dir.clone(),
        // The interface travels inside the app, among its resources.
        None => app.path().resource_dir().context("The app cannot find its own files")?.join("ui"),
    };
    if !ui_dir.join("index.html").is_file() {
        bail!("The interface is missing: there is no index.html in {}.", ui_dir.display());
    }

    let key = uuid::Uuid::new_v4().simple().to_string();
    let options = Options {
        port: Port::Free { prefer: settings::remembered_port(&data_dir) },
        ui_dir: Some(ui_dir),
        data_dir: Some(data_dir),
        desktop: Some(Desktop { key: key.clone(), pick_folder: folder_dialog(app) }),
        ..Options::default()
    };
    let core = tauri::async_runtime::block_on(ae_core::start(options))?;
    settings::remember_port(core.data_dir(), core.addr().port());
    let address = Url::parse(&core.url())?;
    let data_dir = core.data_dir().to_path_buf();
    let zoom = Mutex::new(zoom::remembered(&data_dir));

    // Kept before the window is opened: if opening fails, quitting still finds the core and stops it.
    app.manage(Shell { core: Mutex::new(Some(core)), address, key, data_dir, zoom });
    quit_on_signal(app);
    window::open(app)?;
    Ok(())
}

/// The system's "choose a folder" dialog, in the shape the core asks for it.
fn folder_dialog(app: &AppHandle) -> Arc<dyn Fn() -> Option<PathBuf> + Send + Sync> {
    let app = app.clone();
    Arc::new(move || {
        let mut dialog = app.dialog().file().set_title("Choose the folder of the project");
        // Tied to the window, so it cannot get lost behind it.
        if let Some(window) = app.get_webview_window(window::MAIN) {
            dialog = dialog.set_parent(&window);
        }
        dialog.blocking_pick_folder().and_then(|picked| picked.into_path().ok())
    })
}

/// Ctrl+C in the terminal the app was started from, or a polite "terminate" from
/// the system, quits the app the usual way, so the core is stopped first.
fn quit_on_signal(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let Ok(mut terminate) = signal(SignalKind::terminate()) else { return };
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
        }
        #[cfg(not(unix))]
        let _ = tokio::signal::ctrl_c().await;
        app.exit(0);
    });
}

/// Stops the core: every agent and every terminal it started ends with it.
fn stop(app: &AppHandle) {
    let Some(shell) = app.try_state::<Shell>() else { return };
    let Some(core) = shell.core.lock().unwrap().take() else { return };
    let stopped = tauri::async_runtime::block_on(async { tokio::time::timeout(STOP_WITHIN, core.stop()).await });
    match stopped {
        Ok(Ok(())) => tracing::info!("The core stopped, with its agents and terminals"),
        Ok(Err(error)) => tracing::error!("The core did not stop cleanly: {error:#}"),
        Err(_) => tracing::error!("The core did not stop within {} seconds", STOP_WITHIN.as_secs()),
    }
}
