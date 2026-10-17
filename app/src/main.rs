use tauri::{Manager, PhysicalPosition, WebviewWindow};

const TOP_MARGIN: i32 = 40;

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let overlay = app
                .get_webview_window("overlay")
                .expect("overlay window missing from tauri.conf.json");
            setup_overlay(&overlay)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running invisible-ai");
}

fn setup_overlay(window: &WebviewWindow) -> tauri::Result<()> {
    // Must happen before the first map so tiling WMs (i3) float the window.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        use gtk::prelude::GtkWindowExt;
        window
            .gtk_window()?
            .set_type_hint(gtk::gdk::WindowTypeHint::Utility);
    }

    // On X11 the input shape must be set after the WM manages the window,
    // otherwise i3 never copies it to its frame and clicks hit the frame.
    #[cfg(target_os = "linux")]
    {
        use gtk::prelude::WidgetExtManual;
        let w = window.clone();
        window.gtk_window()?.connect_map_event(move |_, _| {
            let _ = w.set_ignore_cursor_events(true);
            gtk::glib::Propagation::Proceed
        });
    }
    #[cfg(not(target_os = "linux"))]
    window.set_ignore_cursor_events(true)?;

    window.set_visible_on_all_workspaces(true)?;
    window.show()?;

    // After show(): i3 ignores positions requested before the window is mapped.
    if let Some(monitor) = window.primary_monitor()? {
        let screen = monitor.size();
        // outer_size() is still 0 right after show(), so use the configured width.
        let width = window
            .config()
            .app
            .windows
            .iter()
            .find(|w| w.label == window.label())
            .map_or(0.0, |w| w.width)
            * monitor.scale_factor();
        let width = width as i32;
        let origin = monitor.position();
        window.set_position(PhysicalPosition::new(
            origin.x + (screen.width as i32 - width) / 2,
            origin.y + TOP_MARGIN,
        ))?;
    }
    Ok(())
}
