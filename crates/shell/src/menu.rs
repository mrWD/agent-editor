//! The app's menu: About, Edit with copy and paste, View with the size of the interface, Window, Quit.
//!
//! macOS has ready-made items for all of it, and they behave the way every Mac
//! app does. On Windows and Linux some of those ready-made items do nothing
//! (Quit, Minimize and Maximize on Linux), so there the app has items of its own.

use crate::{window, zoom};
use tauri::menu::{AboutMetadata, Menu, MenuEvent, MenuItemBuilder, PredefinedMenuItem, Submenu, SubmenuBuilder};
use tauri::{AppHandle, Manager, Wry};

/// What the app is called in its own menu and in About.
const NAME: &str = "Agent Editor";

// Ids of the items the app answers itself. Only the menu of Windows and Linux has them.
const QUIT: &str = "quit";
const MINIMIZE: &str = "minimize";
const MAXIMIZE: &str = "maximize";
// The size of the interface. Every system has these, with the keys a browser has for them.
const ZOOM_IN: &str = "zoom-in";
const ZOOM_OUT: &str = "zoom-out";
const ZOOM_ACTUAL: &str = "zoom-actual";

/// The View menu's items for the size of the interface: Cmd on a Mac, Ctrl elsewhere, with +, - and 0.
/// "+" is typed on the key that says "=", without Shift, as in every browser.
fn view(app: &AppHandle) -> tauri::Result<SubmenuBuilder<'_, Wry, AppHandle>> {
    let item = |id: &str, text: &str, keys: &str| MenuItemBuilder::with_id(id, text).accelerator(keys).build(app);
    Ok(SubmenuBuilder::new(app, "View")
        .item(&item(ZOOM_ACTUAL, "Actual Size", "CmdOrCtrl+0")?)
        .item(&item(ZOOM_IN, "Zoom In", "CmdOrCtrl+=")?)
        .item(&item(ZOOM_OUT, "Zoom Out", "CmdOrCtrl+-")?))
}

fn about(app: &AppHandle) -> tauri::Result<PredefinedMenuItem<Wry>> {
    let details = AboutMetadata {
        name: Some(NAME.into()),
        version: Some(app.package_info().version.to_string()),
        comments: Some("A code editor in which coding agents work on tasks.".into()),
        icon: app.default_window_icon().cloned(),
        ..Default::default()
    };
    PredefinedMenuItem::about(app, Some(&format!("About {NAME}")), Some(details))
}

/// Both menus are built by plain functions and one is picked here, rather than
/// leaving one out of the build: this way each is checked by the compiler on
/// every system, not only on its own.
pub fn build(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    if cfg!(target_os = "macos") { for_macos(app) } else { for_windows_and_linux(app) }
}

fn for_macos(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let app_menu = SubmenuBuilder::new(app, NAME)
        .item(&about(app)?)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;
    let file = SubmenuBuilder::new(app, "File").close_window().build()?;
    let edit = SubmenuBuilder::new(app, "Edit").undo().redo().separator().cut().copy().paste().select_all().build()?;
    let view: Submenu<Wry> = view(app)?.separator().fullscreen().build()?;
    let window = SubmenuBuilder::new(app, "Window").minimize().maximize().build()?;
    Menu::with_items(app, &[&app_menu, &file, &edit, &view, &window])
}

fn for_windows_and_linux(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let quit = MenuItemBuilder::with_id(QUIT, "Quit").accelerator("Ctrl+Q").build(app)?;
    let file = SubmenuBuilder::new(app, "File").item(&quit).build()?;
    // Undo and redo are left to the keyboard here: the ready-made menu items for them exist only on macOS.
    let edit = SubmenuBuilder::new(app, "Edit").cut().copy().paste().select_all().build()?;
    let view: Submenu<Wry> = view(app)?.build()?;
    let window =
        SubmenuBuilder::new(app, "Window").text(MINIMIZE, "Minimize").text(MAXIMIZE, "Maximize or restore").build()?;
    let help = SubmenuBuilder::new(app, "Help").item(&about(app)?).build()?;
    Menu::with_items(app, &[&file, &edit, &view, &window, &help])
}

/// Answers the items that are the app's own. The ready-made ones answer themselves.
pub fn on_event(app: &AppHandle, event: MenuEvent) {
    let main = || app.get_webview_window(window::MAIN);
    match event.id().as_ref() {
        // Goes the same way as every other quit, so the core is stopped first.
        QUIT => app.exit(0),
        MINIMIZE => {
            if let Some(window) = main() {
                let _ = window.minimize();
            }
        }
        ZOOM_IN => window::zoom(app, Some(zoom::Step::In)),
        ZOOM_OUT => window::zoom(app, Some(zoom::Step::Out)),
        ZOOM_ACTUAL => window::zoom(app, None),
        MAXIMIZE => {
            if let Some(window) = main() {
                let _ = if window.is_maximized().unwrap_or(false) { window.unmaximize() } else { window.maximize() };
            }
        }
        _ => {}
    }
}
