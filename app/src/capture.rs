use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::WebviewWindow;
use xcap::image::RgbaImage;
use xcap::Monitor;

/// Capture the monitor the overlay is on. The overlay itself is included.
pub fn screen(overlay: &WebviewWindow) -> Result<RgbaImage, String> {
    let pos = overlay.outer_position().map_err(|e| e.to_string())?;
    let monitor = Monitor::from_point(pos.x, pos.y).map_err(|e| e.to_string())?;
    monitor.capture_image().map_err(|e| e.to_string())
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
