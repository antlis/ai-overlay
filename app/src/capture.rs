use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::WebviewWindow;
use xcap::image::RgbaImage;
use xcap::Monitor;

/// Capture the monitor the overlay is on, overlay included.
fn screen(overlay: &WebviewWindow) -> Result<RgbaImage, String> {
    let pos = overlay.outer_position().map_err(|e| e.to_string())?;
    let monitor = Monitor::from_point(pos.x, pos.y).map_err(|e| e.to_string())?;
    monitor.capture_image().map_err(|e| e.to_string())
}

/// Capture without the overlay in the shot, so the model never sees its own answers.
pub fn screen_without_overlay(overlay: &WebviewWindow) -> Result<RgbaImage, String> {
    set_transparent(overlay, true)?;
    // Give the compositor a frame or two to repaint without the overlay.
    std::thread::sleep(std::time::Duration::from_millis(150));
    let image = screen(overlay);
    set_transparent(overlay, false)?;
    image
}


/// Fade the overlay out via window opacity rather than hiding it, so i3 doesn't
/// unmanage and re-place the window around every capture.
#[cfg(target_os = "linux")]
fn set_transparent(overlay: &WebviewWindow, transparent: bool) -> Result<(), String> {
    let w = overlay.clone();
    let (done, wait) = std::sync::mpsc::channel();
    overlay
        .run_on_main_thread(move || {
            use gtk::prelude::WidgetExt;
            if let Ok(gtk_window) = w.gtk_window() {
                gtk_window.set_opacity(if transparent { 0.0 } else { 1.0 });
            }
            let _ = done.send(());
        })
        .map_err(|e| e.to_string())?;
    // Callers rely on the change having been applied (capture timing, repaint after restore).
    wait.recv().map_err(|e| e.to_string())
}

#[cfg(not(target_os = "linux"))]
fn set_transparent(overlay: &WebviewWindow, transparent: bool) -> Result<(), String> {
    if transparent { overlay.hide() } else { overlay.show() }.map_err(|e| e.to_string())
}

/// Encode as PNG in memory.
pub fn png(image: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut buf = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut buf, xcap::image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(buf.into_inner())
}

/// Save a capture under `$XDG_CACHE_HOME/invisible-ai/captures/` and return its path.
pub fn save(image: &RgbaImage) -> Result<PathBuf, String> {
    let dir = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .ok_or("neither XDG_CACHE_HOME nor HOME is set")?
        .join("invisible-ai/captures");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let path = dir.join(format!("{millis}.png"));
    image.save(&path).map_err(|e| e.to_string())?;
    Ok(path)
}
