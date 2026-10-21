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
            &item("listen", "Listen (transcribe)")?,
            &PredefinedMenuItem::separator(app)?,
            &item("toggle", "Show / hide")?,
            &item("interact", "Interactive")?,
            &item("fullscreen", "Fullscreen")?,
            &PredefinedMenuItem::separator(app)?,
            &item("quit", "Quit")?,
        ],
    )?;

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("ai-overlay")
        .menu(&menu)
        .on_menu_event(|app, event| {
            // Off the UI thread: some commands (listen) take seconds to start.
            let (app, cmd) = (app.clone(), event.id().as_ref().to_owned());
            std::thread::spawn(move || {
                if let Err(e) = crate::dispatch(&app, &cmd) {
                    eprintln!("tray: {e}");
                }
            });
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}
