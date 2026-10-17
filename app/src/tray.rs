use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::App;

/// Tray icon whose menu items run the same commands as the control socket.
pub fn setup(app: &App) -> tauri::Result<()> {
    let item = |id: &str, label: &str| MenuItem::with_id(app, id, label, true, None::<&str>);
    let menu = Menu::with_items(
        app,
        &[
            &item("ask", "Ask about screen")?,
            &PredefinedMenuItem::separator(app)?,
            &item("toggle", "Show / hide")?,
            &item("interact", "Interactive")?,
            &item("fullscreen", "Fullscreen")?,
            &PredefinedMenuItem::separator(app)?,
            &item("quit", "Quit")?,
        ],
    )?;

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("invisible-ai")
        .menu(&menu)
        .on_menu_event(|app, event| {
            if let Err(e) = crate::dispatch(app, event.id().as_ref()) {
                eprintln!("tray: {e}");
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}
