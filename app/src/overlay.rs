use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};

const TOP_MARGIN: i32 = 40;
/// Height of the panel header (logical px), which stays clickable in click-through mode.
const HEADER_HEIGHT: f64 = 40.0;

/// Interactive: receives clicks and keyboard focus. Otherwise click-through.
static INTERACTIVE: AtomicBool = AtomicBool::new(false);
/// Fullscreen: covers the monitor (minus a margin) instead of the compact top-center panel.
static FULLSCREEN: AtomicBool = AtomicBool::new(false);

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

pub fn toggle_fullscreen(window: &WebviewWindow) -> tauri::Result<()> {
    let fullscreen = !FULLSCREEN.fetch_xor(true, Ordering::SeqCst);
    place(window)?;
    window.emit("fullscreen", fullscreen)
}

fn apply_mode(window: &WebviewWindow, interactive: bool) -> tauri::Result<()> {
    set_input_region(window, interactive)?;
    if interactive {
        window.set_focus()?;
    }
    window.emit("interactive", interactive)
}

/// Compact: configured size, top-center of the primary monitor.
/// Fullscreen: the whole monitor minus `TOP_MARGIN` on every side.
fn place(window: &WebviewWindow) -> tauri::Result<()> {
    let Some(monitor) = window.primary_monitor()? else {
        return Ok(());
    };
    let (origin, screen) = (monitor.position(), monitor.size());
    let (width, height) = if FULLSCREEN.load(Ordering::SeqCst) {
        (screen.width - 2 * TOP_MARGIN as u32, screen.height - 2 * TOP_MARGIN as u32)
    } else {
        // outer_size() is still 0 right after mapping, so use the configured size.
        let config = window.config().app.windows.iter().find(|w| w.label == window.label());
        let scale = monitor.scale_factor();
        (
            (config.map_or(0.0, |w| w.width) * scale) as u32,
            (config.map_or(0.0, |w| w.height) * scale) as u32,
        )
    };
    window.set_size(PhysicalSize::new(width, height))?;
    window.set_position(PhysicalPosition::new(
        origin.x + (screen.width as i32 - width as i32) / 2,
        origin.y + TOP_MARGIN,
    ))
}


/// Interactive: the whole window takes input. Click-through: only the header
/// (with its buttons) does; clicks anywhere else go to the windows below.
#[cfg(target_os = "linux")]
fn set_input_region(window: &WebviewWindow, interactive: bool) -> tauri::Result<()> {
    let header = (HEADER_HEIGHT * window.scale_factor()?) as i32;
    let w = window.clone();
    window.run_on_main_thread(move || {
        use gtk::cairo::{RectangleInt, Region};
        use gtk::prelude::WidgetExt;
        let Ok(gtk_window) = w.gtk_window() else { return };
        if interactive {
            gtk_window.input_shape_combine_region(None);
        } else if let Some(gdk_window) = gtk_window.window() {
            let region = Region::create_rectangle(&RectangleInt::new(0, 0, i32::MAX / 2, header));
            gdk_window.input_shape_combine_region(&region, 0, 0);
        }
    })
}

#[cfg(not(target_os = "linux"))]
fn set_input_region(window: &WebviewWindow, interactive: bool) -> tauri::Result<()> {
    window.set_ignore_cursor_events(!interactive)
}
