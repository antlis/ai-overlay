use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};

use tauri::{Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};

const TOP_MARGIN: i32 = 40;
/// Height of the panel header (logical px), which stays clickable in click-through mode.
const HEADER_HEIGHT: f64 = 40.0;
/// Width of the strip along the right edge (logical px) that takes the mouse in
/// click-through mode: wheel scrolling, copy buttons and the resize corner live there.
const SCROLL_STRIP: f64 = 56.0;
/// Smallest compact panel (logical px).
const MIN_SIZE: (i32, i32) = (300, 160);
/// How long a resize/drag has to go quiet before the layer surface is remapped to its
/// new size (remapping is the only way GTK resizes it, but it drops the pointer grab).
const REMAP_DELAY: std::time::Duration = std::time::Duration::from_millis(140);

/// X11: where the panel was when last hidden, restored on the next show.
static HIDDEN_AT: std::sync::Mutex<Option<PhysicalPosition<i32>>> = std::sync::Mutex::new(None);
/// Interactive: receives clicks and keyboard focus. Otherwise click-through.
static INTERACTIVE: AtomicBool = AtomicBool::new(false);
/// Fullscreen: covers the monitor (minus a margin) instead of the compact top-center panel.
static FULLSCREEN: AtomicBool = AtomicBool::new(true); // opens fullscreen
/// Running on Wayland: the overlay is our own layer-shell window, not tao's window.
static WAYLAND: AtomicBool = AtomicBool::new(false);
/// Wayland only: whether the layer window is shown (readable off the main thread).
static SHOWN: AtomicBool = AtomicBool::new(false);
/// Wayland only: compact panel's top-left corner (logical px) once the user dragged
/// it; `i32::MIN` = not moved, top-center.
static POS_X: AtomicI32 = AtomicI32::new(i32::MIN);
static POS_Y: AtomicI32 = AtomicI32::new(i32::MIN);
/// Wayland only: compact panel's size (logical px) once resized; 0 = configured size.
static WIDTH: AtomicI32 = AtomicI32::new(0);
static HEIGHT: AtomicI32 = AtomicI32::new(0);
/// Wayland only: size the layer surface was last mapped at (packed width<<32|height),
/// whether the surface is currently mapped, and whether a remap is already queued. GTK
/// never resizes a mapped layer surface, so a size change has to hide + show the window;
/// these keep a drag to one remap and stop the first map from counting as one.
static MAPPED_SIZE: AtomicI64 = AtomicI64::new(0);
static MAPPED: AtomicBool = AtomicBool::new(false);
static REMAP_QUEUED: AtomicBool = AtomicBool::new(false);

/// Compact panel size: resized, else as configured in tauri.conf.json.
fn size(window: &WebviewWindow) -> (i32, i32) {
    let (w, h) = (WIDTH.load(Ordering::SeqCst), HEIGHT.load(Ordering::SeqCst));
    if w > 0 {
        return (w, h);
    }
    let config = window.config().app.windows.iter().find(|c| c.label == window.label());
    config.map_or((480, 320), |c| (c.width as i32, c.height as i32))
}

/// Position and size of the compact panel, kept across restarts (Wayland).
fn layout_path() -> std::path::PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state")
        })
        .join("invisible-ai/layout.json")
}

fn load_layout() {
    let Ok(text) = std::fs::read_to_string(layout_path()) else { return };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return };
    let get = |key: &str| v[key].as_i64().map(|n| n as i32);
    if let (Some(x), Some(y), Some(w), Some(h)) = (get("x"), get("y"), get("width"), get("height")) {
        POS_X.store(x, Ordering::SeqCst);
        POS_Y.store(y, Ordering::SeqCst);
        WIDTH.store(w.max(MIN_SIZE.0), Ordering::SeqCst);
        HEIGHT.store(h.max(MIN_SIZE.1), Ordering::SeqCst);
    }
}

fn save_layout(width: i32, height: i32) {
    let layout = serde_json::json!({
        "x": POS_X.load(Ordering::SeqCst),
        "y": POS_Y.load(Ordering::SeqCst),
        "width": width,
        "height": height,
    });
    let path = layout_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, layout.to_string());
}



/// Layer-shell namespace on Wayland; compositor rules (e.g. Niri's block-out) match it.
#[cfg(target_os = "linux")]
pub const NAMESPACE: &str = "invisible-ai";

#[cfg(target_os = "linux")]
thread_local! {
    /// Wayland: the layer-shell window holding the webview. GTK objects live on the
    /// main thread, so this is only touched there.
    static LAYER: std::cell::RefCell<Option<gtk::Window>> = const { std::cell::RefCell::new(None) };
}

/// The GTK window the overlay is in: our layer window on Wayland, tao's on X11.
/// Main thread only.
#[cfg(target_os = "linux")]
fn toplevel(window: &WebviewWindow) -> Option<gtk::Window> {
    use gtk::prelude::Cast;
    LAYER
        .with(|layer| layer.borrow().clone())
        .or_else(|| window.gtk_window().ok().map(|w| w.upcast()))
}

pub fn setup(window: &WebviewWindow) -> tauri::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use gtk::gdk::prelude::DisplayExtManual;
        use gtk::prelude::*;
        let tao_window = window.gtk_window()?;
        if tao_window.display().backend().is_wayland() {
            WAYLAND.store(true, Ordering::SeqCst);
            load_layout();
            // tao's window never draws once turned into a layer surface, so move its
            // content (the box holding the webview) into a fresh layer-shell window
            // before anything is realized. tao's window is never shown.
            use gtk_layer_shell::{KeyboardMode, Layer, LayerShell};
            let layer = gtk::Window::new(gtk::WindowType::Toplevel);
            if let Some(visual) = WidgetExt::screen(&layer).and_then(|s| s.rgba_visual()) {
                layer.set_visual(Some(&visual)); // transparent background
            }
            layer.set_app_paintable(true);
            layer.init_layer_shell();
            layer.set_layer(Layer::Overlay);
            layer.set_namespace(NAMESPACE);
            layer.set_keyboard_mode(KeyboardMode::None);
            // Position relative to the whole output, not the area bars leave free.
            layer.set_exclusive_zone(-1);
            if let Some(content) = tao_window.children().into_iter().next() {
                tao_window.remove(&content);
                layer.add(&content);
            }
            LAYER.with(|l| *l.borrow_mut() = Some(layer.clone()));
            connect_layout(window, &layer);
            place(window)?;
            // Map the layer window on the GTK main loop, once GTK has had a chance to
            // settle after setup. Calling show_all directly inside setup races the
            // window's own realize, so gtk-layer-shell's wait-for-configure inside its
            // map handler times out and the window never paints.
            let w = window.clone();
            window.run_on_main_thread(move || {
                use gtk::prelude::WidgetExt;
                if let Some(layer) = toplevel(&w) {
                    layer.show_all();
                    MAPPED.store(true, Ordering::SeqCst);
                }
            })?;
            SHOWN.store(true, Ordering::SeqCst);
            return Ok(());
        }

        // X11: i3 floats UTILITY windows; _NET_WM_USER_TIME=0 so showing the overlay
        // never steals focus. Both must be set before the first map.
        tao_window.set_type_hint(gtk::gdk::WindowTypeHint::Utility);
        tao_window.set_focus_on_map(false);
        connect_layout(window, tao_window.upcast_ref());
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

/// On X11, position and input shape must be set after the WM manages the window:
/// i3 ignores earlier positions and never copies an earlier input shape to its
/// frame (so clicks would hit the frame). The click-through region also depends on
/// the window size, so it's re-applied on every resize.
#[cfg(target_os = "linux")]
fn connect_layout(window: &WebviewWindow, gtk_window: &gtk::Window) {
    use gtk::prelude::{WidgetExt, WidgetExtManual};
    gtk_window.connect_size_allocate(|gtk_window, _| {
        input_region(gtk_window, INTERACTIVE.load(Ordering::SeqCst));
    });
    let w = window.clone();
    let placed = AtomicBool::new(false);
    gtk_window.connect_map_event(move |_, _| {
        let _ = apply_mode(&w, INTERACTIVE.load(Ordering::SeqCst));
        if !WAYLAND.load(Ordering::SeqCst) {
            // Later maps go back to where the panel was hidden. GTK would restore that
            // itself, but against i3's default frame, which the config then strips: the
            // panel crept down (and right) by the frame size on every show.
            let saved = *HIDDEN_AT.lock().unwrap();
            match saved {
                Some(pos) if placed.swap(true, Ordering::SeqCst) && !FULLSCREEN.load(Ordering::SeqCst) => {
                    let _ = w.set_position(pos);
                }
                _ => {
                    placed.store(true, Ordering::SeqCst);
                    let _ = place(&w);
                }
            }
        }
        gtk::glib::Propagation::Proceed
    });
}

pub fn is_wayland() -> bool {
    WAYLAND.load(Ordering::SeqCst)
}

pub fn is_visible(window: &WebviewWindow) -> tauri::Result<bool> {
    if WAYLAND.load(Ordering::SeqCst) {
        return Ok(SHOWN.load(Ordering::SeqCst));
    }
    window.is_visible()
}

pub fn set_visible(window: &WebviewWindow, visible: bool) -> tauri::Result<()> {
    #[cfg(target_os = "linux")]
    if WAYLAND.load(Ordering::SeqCst) {
        SHOWN.store(visible, Ordering::SeqCst);
        let w = window.clone();
        return window.run_on_main_thread(move || {
            use gtk::prelude::WidgetExt;
            if let Some(layer) = toplevel(&w) {
                if visible {
                    layer.show_all();
                    MAPPED.store(true, Ordering::SeqCst);
                } else {
                    layer.hide();
                    MAPPED.store(false, Ordering::SeqCst);
                    // The next show maps fresh, so any size then is a change again.
                    MAPPED_SIZE.store(0, Ordering::SeqCst);
                }
            }
        });
    }
    if !visible {
        *HIDDEN_AT.lock().unwrap() = window.outer_position().ok();
    }
    if visible { window.show() } else { window.hide() }
}

pub fn toggle_visible(window: &WebviewWindow) -> tauri::Result<()> {
    set_visible(window, !is_visible(window)?)
}

pub fn toggle_interactive(window: &WebviewWindow) -> tauri::Result<()> {
    let interactive = !INTERACTIVE.fetch_xor(true, Ordering::SeqCst);
    apply_mode(window, interactive)
}

/// Interactive mode with keyboard focus on the panel (the UI then focuses its question
/// input). On Wayland the panel takes the keyboard until interactive mode ends: an
/// overlay layer only gets focus on demand otherwise, i.e. after a click.
pub fn focus_prompt(window: &WebviewWindow) -> tauri::Result<()> {
    INTERACTIVE.store(true, Ordering::SeqCst);
    apply_mode(window, true)?;
    #[cfg(target_os = "linux")]
    if WAYLAND.load(Ordering::SeqCst) {
        let w = window.clone();
        window.run_on_main_thread(move || {
            use gtk_layer_shell::{KeyboardMode, LayerShell};
            if let Some(layer) = toplevel(&w) {
                layer.set_keyboard_mode(KeyboardMode::Exclusive);
            }
        })?;
    }
    window.emit("prompt", ())
}

pub fn is_fullscreen() -> bool {
    FULLSCREEN.load(Ordering::SeqCst)
}

pub fn toggle_fullscreen(window: &WebviewWindow) -> tauri::Result<()> {
    let fullscreen = !FULLSCREEN.fetch_xor(true, Ordering::SeqCst);
    place(window)?;
    window.emit("fullscreen", fullscreen)
}

fn apply_mode(window: &WebviewWindow, interactive: bool) -> tauri::Result<()> {
    set_input_region(window, interactive)?;
    if WAYLAND.load(Ordering::SeqCst) {
        // Layer surfaces only get keyboard focus if they ask for it.
        #[cfg(target_os = "linux")]
        {
            let w = window.clone();
            window.run_on_main_thread(move || {
                use gtk_layer_shell::{KeyboardMode, LayerShell};
                if let Some(layer) = toplevel(&w) {
                    let mode = if interactive { KeyboardMode::OnDemand } else { KeyboardMode::None };
                    layer.set_keyboard_mode(mode);
                }
            })?;
        }
    } else if interactive {
        window.set_focus()?;
    }
    window.emit("interactive", interactive)
}

/// Logical size of the monitor the layer window is on (the first one before it's mapped).
#[cfg(target_os = "linux")]
fn monitor_geometry(layer: &gtk::Window) -> Option<gtk::gdk::Rectangle> {
    use gtk::prelude::{MonitorExt, WidgetExt};
    let display = layer.display();
    layer
        .window()
        .and_then(|gdk_window| display.monitor_at_window(&gdk_window))
        .or_else(|| display.monitor(0))
        .map(|monitor| monitor.geometry())
}

/// Move the compact panel by (`dx`, `dy`) logical px (dragging its header).
pub fn move_by(window: &WebviewWindow, dx: i32, dy: i32) -> tauri::Result<()> {
    adjust(window, dx, dy, 0, 0)
}

/// Resize the compact panel by (`dw`, `dh`) logical px (dragging its corner).
pub fn resize_by(window: &WebviewWindow, dw: i32, dh: i32) -> tauri::Result<()> {
    adjust(window, 0, 0, dw, dh)
}

/// Move and/or resize the compact panel, keeping it on screen; on Wayland the result is
/// remembered across restarts.
fn adjust(window: &WebviewWindow, dx: i32, dy: i32, dw: i32, dh: i32) -> tauri::Result<()> {
    if FULLSCREEN.load(Ordering::SeqCst) {
        return Ok(());
    }
    let (width, height) = size(window);
    #[cfg(target_os = "linux")]
    if WAYLAND.load(Ordering::SeqCst) {
        let w = window.clone();
        return window.run_on_main_thread(move || {
            let Some(screen) = toplevel(&w).and_then(|layer| monitor_geometry(&layer)) else { return };
            let (mut x, mut y) = (POS_X.load(Ordering::SeqCst), POS_Y.load(Ordering::SeqCst));
            if x == i32::MIN {
                (x, y) = ((screen.width() - width) / 2, TOP_MARGIN);
            }
            let x = (x + dx).clamp(0, (screen.width() - MIN_SIZE.0).max(0));
            let y = (y + dy).clamp(0, (screen.height() - MIN_SIZE.1).max(0));
            let width = (width + dw).clamp(MIN_SIZE.0, (screen.width() - x).max(MIN_SIZE.0));
            let height = (height + dh).clamp(MIN_SIZE.1, (screen.height() - y).max(MIN_SIZE.1));
            // Moving keeps the whole panel on screen.
            let x = x.min((screen.width() - width).max(0));
            let y = y.min((screen.height() - height).max(0));
            POS_X.store(x, Ordering::SeqCst);
            POS_Y.store(y, Ordering::SeqCst);
            WIDTH.store(width, Ordering::SeqCst);
            HEIGHT.store(height, Ordering::SeqCst);
            save_layout(width, height);
            let _ = place(&w);
        });
    }
    let scale = window.scale_factor()?;
    if dw != 0 || dh != 0 {
        let outer = window.outer_size()?;
        let grow = |n: u32, d: i32, min: i32| ((n as f64 + f64::from(d) * scale) as i32).max(min) as u32;
        return window.set_size(PhysicalSize::new(
            grow(outer.width, dw, MIN_SIZE.0),
            grow(outer.height, dh, MIN_SIZE.1),
        ));
    }
    let pos = window.outer_position()?;
    window.set_position(PhysicalPosition::new(
        pos.x + (f64::from(dx) * scale) as i32,
        pos.y + (f64::from(dy) * scale) as i32,
    ))
}

/// Compact: `size()`, top-center of the primary monitor (on Wayland: where it was moved).
/// Fullscreen: the whole monitor minus `TOP_MARGIN` on every side.
fn place(window: &WebviewWindow) -> tauri::Result<()> {
    let fullscreen = FULLSCREEN.load(Ordering::SeqCst);
    let (width, height) = size(window);

    // Layer surfaces are positioned by anchors and margins, not coordinates.
    #[cfg(target_os = "linux")]
    if WAYLAND.load(Ordering::SeqCst) {
        let w = window.clone();
        return window.run_on_main_thread(move || {
            use gtk_layer_shell::{Edge, LayerShell};
            use gtk::prelude::{GtkWindowExt, WidgetExt};
            let Some(layer) = toplevel(&w) else { return };
            let Some(screen) = monitor_geometry(&layer) else { return };
            // Layer surfaces are positioned by margins, and their size comes from the
            // buffer GTK commits (anchoring both opposite edges would let the compositor
            // stretch it instead, which it never re-does for a mapped surface).
            let (mut x, mut y) = (POS_X.load(Ordering::SeqCst), POS_Y.load(Ordering::SeqCst));
            if x == i32::MIN {
                (x, y) = ((screen.width() - width) / 2, TOP_MARGIN);
            }
            for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
                let (anchor, margin) = match edge {
                    _ if fullscreen => (true, TOP_MARGIN),
                    Edge::Top => (true, y),
                    Edge::Left => (true, x),
                    Edge::Bottom => (false, 0),
                    _ => (false, 0),
                };
                layer.set_anchor(edge, anchor);
                layer.set_layer_shell_margin(edge, margin.max(0));
            }
            // GTK won't shrink a mapped window on its own. The documented recipe: a size
            // request on the layer window itself, then gtk_window_resize(1, 1) to force
            // GTK to honour it (ignored on axes anchored to two opposite edges).
            let (width, height) = if fullscreen {
                ((screen.width() - 2 * TOP_MARGIN).max(1), (screen.height() - 2 * TOP_MARGIN).max(1))
            } else {
                (width.max(1), height.max(1))
            };
            layer.set_size_request(width, height);
            layer.resize(1, 1);
            // GTK never resizes a mapped layer surface (its size is whatever the first
            // configure settled on), so the only way to change it is to remap: hide
            // destroys the layer surface, the next show creates it at the new size.
            // Remapping kills the pointer grab, so a drag would break after one step:
            // changes are debounced instead, and the pending remap applies whatever the
            // latest margins are when it finally fires.
            let key = (i64::from(width) << 32) | i64::from(height);
            if !MAPPED.load(Ordering::SeqCst) {
                MAPPED_SIZE.store(key, Ordering::SeqCst);
            } else if MAPPED_SIZE.swap(key, Ordering::SeqCst) != key && !REMAP_QUEUED.swap(true, Ordering::SeqCst) {
                let w2 = w.clone();
                gtk::glib::timeout_add_local(REMAP_DELAY, move || {
                    REMAP_QUEUED.store(false, Ordering::SeqCst);
                    if MAPPED.load(Ordering::SeqCst) {
                        if let Some(layer) = toplevel(&w2) {
                            layer.hide();
                            let w3 = w2.clone();
                            gtk::glib::idle_add_local(move || {
                                if let Some(layer) = toplevel(&w3) {
                                    layer.show_all();
                                }
                                gtk::glib::ControlFlow::Break
                            });
                        }
                    }
                    gtk::glib::ControlFlow::Break
                });
            }
        });
    }

    let Some(monitor) = window.primary_monitor()? else {
        return Ok(());
    };
    let (origin, screen) = (monitor.position(), monitor.size());
    let (width, height) = if fullscreen {
        (screen.width - 2 * TOP_MARGIN as u32, screen.height - 2 * TOP_MARGIN as u32)
    } else {
        // outer_size() is still 0 right after mapping, so use the configured size.
        let scale = monitor.scale_factor();
        ((f64::from(width) * scale) as u32, (f64::from(height) * scale) as u32)
    };
    window.set_size(PhysicalSize::new(width, height))?;
    window.set_position(PhysicalPosition::new(
        origin.x + (screen.width as i32 - width as i32) / 2,
        origin.y + TOP_MARGIN,
    ))
}

/// Interactive: the whole window takes input. Click-through: only the header (with
/// its buttons) and a scroll strip on the right do; clicks elsewhere go to the windows below.
#[cfg(target_os = "linux")]
fn set_input_region(window: &WebviewWindow, interactive: bool) -> tauri::Result<()> {
    let w = window.clone();
    window.run_on_main_thread(move || {
        if let Some(gtk_window) = toplevel(&w) {
            input_region(&gtk_window, interactive);
        }
    })
}

/// Must run on the main thread.
#[cfg(target_os = "linux")]
fn input_region(gtk_window: &gtk::Window, interactive: bool) {
    use gtk::cairo::{RectangleInt, Region};
    use gtk::prelude::WidgetExt;
    if interactive {
        gtk_window.input_shape_combine_region(None);
        return;
    }
    let Some(gdk_window) = gtk_window.window() else { return };
    let scale = f64::from(gtk_window.scale_factor());
    let (width, height) = (gtk_window.allocated_width(), gtk_window.allocated_height());
    let header = (HEADER_HEIGHT * scale) as i32;
    let strip = (SCROLL_STRIP * scale) as i32;
    let region = Region::create_rectangle(&RectangleInt::new(0, 0, width, header));
    let _ = region.union_rectangle(&RectangleInt::new(width - strip, 0, strip, height));
    gdk_window.input_shape_combine_region(&region, 0, 0);
}

#[cfg(not(target_os = "linux"))]
fn set_input_region(window: &WebviewWindow, interactive: bool) -> tauri::Result<()> {
    window.set_ignore_cursor_events(!interactive)
}
