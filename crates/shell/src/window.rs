//! The one window of the app.

use crate::{Shell, links};
use tauri::webview::{NewWindowResponse, PageLoadEvent};
use tauri::window::Color;
use tauri::{AppHandle, Manager, Theme, WebviewUrl, WebviewWindowBuilder};

/// The name Tauri knows the window by.
pub const MAIN: &str = "main";

/// Opens the window on the interface the core serves.
pub fn open(app: &AppHandle) -> tauri::Result<()> {
    let shell = app.state::<Shell>();
    let own = shell.address.clone();
    let (for_links, for_new_windows) = (app.clone(), app.clone());
    let previews = shell.core.lock().unwrap().as_ref().map(|core| core.state().previews.clone());

    WebviewWindowBuilder::new(app, MAIN, WebviewUrl::External(shell.address.clone()))
        .title("Agent Editor")
        // A first-time size; after that the window opens the way it was left.
        .inner_size(1280.0, 820.0)
        .min_inner_size(880.0, 560.0)
        .center()
        // The interface is dark. Without these the window would flash white before the page is drawn.
        .theme(Some(Theme::Dark))
        .background_color(Color(0x0f, 0x11, 0x15, 0xff))
        // How the interface knows it runs inside the app, and its key to the things only the app can do.
        .initialization_script(format!("window.__AE_SHELL__ = Object.freeze({{ key: \"{}\" }});", shell.key))
        // The window shows the interface and nothing else. A link that leaves it goes to the browser.
        .on_navigation(move |url| {
            if links::is_own(url, &own) || url.scheme() == "about" {
                return true;
            }
            // The Preview shows the running app in frames, and on some systems a frame asks here too.
            if previews.as_ref().is_some_and(|previews| links::is_preview(url, |port| previews.serves(port))) {
                return true;
            }
            links::open_outside(&for_links, url);
            false
        })
        // Links marked to open in a new window, as every link out of the interface is.
        .on_new_window(move |url, _features| {
            links::open_outside(&for_new_windows, &url);
            NewWindowResponse::Deny
        })
        .on_page_load(|_window, page| {
            if page.event() == PageLoadEvent::Finished {
                tracing::info!("The window shows {}", page.url());
            }
        })
        .build()?;
    Ok(())
}

/// Brings the window to the front, opening it again when it was closed while the app kept running.
pub fn show(app: &AppHandle) {
    match app.get_webview_window(MAIN) {
        Some(window) => {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        }
        // No `Shell` means the core never started; there is nothing to show then.
        None if app.try_state::<Shell>().is_some() => {
            if let Err(error) = open(app) {
                tracing::error!("Could not open the window: {error}");
            }
        }
        None => {}
    }
}
