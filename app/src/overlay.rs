use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{Emitter, Manager, PhysicalPosition, WebviewWindow};

const TOP_MARGIN: i32 = 40;

/// Interactive: receives clicks and keyboard focus. Otherwise click-through.
static INTERACTIVE: AtomicBool = AtomicBool::new(false);

pub fn setup(window: &WebviewWindow) -> tauri::Result<()> {
    // Must happen before the first map so tiling WMs (i3) float the window.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        use gtk::prelude::GtkWindowExt;
        let gtk_window = window.gtk_window()?;
        gtk_window.set_type_hint(gtk::gdk::WindowTypeHint::Utility);
        // _NET_WM_USER_TIME=0: i3 won't steal focus each time the overlay is shown.
        gtk_window.set_focus_on_map(false);
    }

    // On X11, position and input shape must be set after the WM manages the
    // window: i3 ignores earlier positions and never copies an earlier input
    // shape to its frame (so clicks would hit the frame). This runs again on
    // every show, since hiding unmanages the window.
    #[cfg(target_os = "linux")]
    {
        use gtk::prelude::WidgetExtManual;
        let w = window.clone();
        window.gtk_window()?.connect_map_event(move |_, _| {
            let _ = apply_mode(&w, INTERACTIVE.load(Ordering::SeqCst));
            let _ = place(&w);
            gtk::glib::Propagation::Proceed
        });
    }

    window.set_visible_on_all_workspaces(true)?;
    window.show()?;

    #[cfg(not(target_os = "linux"))]
    {
        apply_mode(window, false)?;
        place(window)?;
    }
    Ok(())
}

pub fn toggle_visible(window: &WebviewWindow) -> tauri::Result<()> {
    if window.is_visible()? {
        window.hide()
    } else {
        window.show()
    }
}

pub fn toggle_interactive(window: &WebviewWindow) -> tauri::Result<()> {
    let interactive = !INTERACTIVE.fetch_xor(true, Ordering::SeqCst);
    apply_mode(window, interactive)
}

fn apply_mode(window: &WebviewWindow, interactive: bool) -> tauri::Result<()> {
    window.set_ignore_cursor_events(!interactive)?;
    if interactive {
        window.set_focus()?;
    }
    window.emit("interactive", interactive)
}

/// Top-center of the primary monitor.
fn place(window: &WebviewWindow) -> tauri::Result<()> {
    let Some(monitor) = window.primary_monitor()? else {
        return Ok(());
    };
    // outer_size() is still 0 right after mapping, so use the configured width.
    let width = window
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == window.label())
        .map_or(0.0, |w| w.width)
        * monitor.scale_factor();
    let origin = monitor.position();
    window.set_position(PhysicalPosition::new(
        origin.x + (monitor.size().width as i32 - width as i32) / 2,
        origin.y + TOP_MARGIN,
    ))
}
