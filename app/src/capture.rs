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
/// The compact panel stays on screen and is blanked out of the image. A fullscreen
/// panel would blank out everything, so on X11 it fades out for the capture instead.
pub fn screen_without_overlay(overlay: &WebviewWindow) -> Result<RgbaImage, String> {
    // Wayland: the compositor leaves the overlay out of captures (Niri's
    // block-out-from rule), so nothing to blank out, fullscreen or not.
    if crate::overlay::is_wayland() {
        return wayland_screen();
    }
    let visible = crate::overlay::is_visible(overlay).map_err(|e| e.to_string())?;
    if visible && crate::overlay::is_fullscreen() {
        set_transparent(overlay, true)?;
        // Give the compositor a frame or two to repaint without the overlay.
        std::thread::sleep(std::time::Duration::from_millis(150));
        let image = screen(overlay);
        set_transparent(overlay, false)?;
        return image;
    }
    let mut image = screen(overlay)?;
    if visible {
        blank_out(&mut image, overlay)?;
    }
    Ok(image)
}

/// Fade the overlay out via window opacity (needs a compositor, e.g. picom) rather
/// than hiding it, so i3 doesn't unmanage and re-place the window around every capture.
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

/// Capture via the wlr-screencopy protocol (`grim`), which Niri supports.
fn wayland_screen() -> Result<RgbaImage, String> {
    let out = std::process::Command::new("grim")
        .args(["-t", "png", "-l", "1", "-"])
        .output()
        .map_err(|e| format!("cannot run grim: {e}"))?;
    if !out.status.success() {
        return Err(format!("grim failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    xcap::image::load_from_memory(&out.stdout)
        .map(|image| image.to_rgba8())
        .map_err(|e| e.to_string())
}

/// Paint the overlay's area of a monitor capture flat grey.
fn blank_out(image: &mut RgbaImage, overlay: &WebviewWindow) -> Result<(), String> {
    let pos = overlay.outer_position().map_err(|e| e.to_string())?;
    let size = overlay.outer_size().map_err(|e| e.to_string())?;
    let monitor = Monitor::from_point(pos.x, pos.y).map_err(|e| e.to_string())?;
    let x0 = (pos.x - monitor.x().map_err(|e| e.to_string())?).max(0) as u32;
    let y0 = (pos.y - monitor.y().map_err(|e| e.to_string())?).max(0) as u32;
    for y in y0..(y0 + size.height).min(image.height()) {
        for x in x0..(x0 + size.width).min(image.width()) {
            image.put_pixel(x, y, xcap::image::Rgba([40, 40, 40, 255]));
        }
    }
    Ok(())
}


/// Encode as PNG in memory.
pub fn png(image: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut buf = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut buf, xcap::image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(buf.into_inner())
}

/// Save a capture under `$XDG_CACHE_HOME/ai-overlay/captures/` and return its path.
pub fn save(image: &RgbaImage) -> Result<PathBuf, String> {
    let dir = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .ok_or("neither XDG_CACHE_HOME nor HOME is set")?
        .join("ai-overlay/captures");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let path = dir.join(format!("{millis}.png"));
    image.save(&path).map_err(|e| e.to_string())?;
    Ok(path)
}
