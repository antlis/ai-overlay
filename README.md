# invisible-ai

A screen- and meeting-aware AI overlay for Linux, in the spirit of [Cluely](https://cluely.com/).
A translucent, click-through panel sits on top of your desktop; hotkeys capture the screen
and audio, and answers from Claude stream into the overlay.

Linux-first (X11 and Wayland), designed so macOS/Windows backends can be added later.

## Status

Early development. Works today on X11 (tested on i3 + picom): transparent, click-through,
always-on-top overlay controlled over a Unix socket; screenshots of the current monitor.

## Requirements

- Nix with flakes (the dev shell provides Rust, Tauri v2, WebKitGTK, PipeWire, etc.)
- X11: a compositor (e.g. picom) for transparency
- Wayland: a compositor with `wlr-layer-shell` (Niri, Sway, Hyprland, KDE) — not yet implemented

## Run

```sh
nix develop -c cargo run
```

The first build takes a few minutes.

## Control

The running app listens on `$XDG_RUNTIME_DIR/invisible-ai.sock`. The same binary, given a
command, sends it there:

| Command | Effect |
|---|---|
| `invisible-ai toggle` | Show / hide the overlay |
| `invisible-ai interact` | Switch between click-through and interactive (clickable, focused, purple border) |
| `invisible-ai capture` | Screenshot the overlay's monitor; prints the PNG path (`$XDG_CACHE_HOME/invisible-ai/captures/`) |
| `invisible-ai quit` | Exit |

Only one instance runs at a time. Bind the commands in your WM, e.g. i3:

```
bindsym $mod+backslash       exec --no-startup-id /path/to/invisible-ai toggle
bindsym $mod+Shift+backslash exec --no-startup-id /path/to/invisible-ai interact
```

Niri:

```kdl
binds {
    Mod+Backslash       { spawn "/path/to/invisible-ai" "toggle"; }
    Mod+Shift+Backslash { spawn "/path/to/invisible-ai" "interact"; }
}
```

Use `target/debug/invisible-ai` until there's a packaged build (or `cargo install --path app`).

> Nix flakes only see files tracked by git — `git add` new files before `nix develop`.

## Layout

```
flake.nix      dev shell
app/           Tauri v2 app (Rust)
  src/main.rs      entry point: run the app, or send a command to the running one
  src/overlay.rs   overlay window: placement, click-through/interactive modes
  src/ipc.rs       control socket
  src/capture.rs   screen capture (xcap)
ui/            overlay frontend (static HTML, no build step)
```

Backend crates (`core`, `x11`, `wayland`, `audio`, `ai`) are added as their milestones land.

## Roadmap

- [x] **M0** Scaffold — flake dev shell, Tauri v2 workspace
- [x] **M1** Overlay (X11) — transparent, click-through, always on top, sticky, no taskbar entry, floats in i3
- [ ] **M1b** Overlay (Wayland/Niri) — `gtk-layer-shell` surface, namespace `invisible-ai`
- [x] **M2** Control — Unix socket + CLI (`toggle`, `interact`, `quit`), i3/Niri keybinding examples; `ask`/`listen` land with M4/M5
- [x] **M3** Screen capture (X11) — `capture` command via [xcap](https://crates.io/crates/xcap), ~200 ms per 1080p PNG
- [ ] **M3b** Screen capture (Wayland) — xcap tries wlroots screencopy / portal Screenshot; verify on Niri, fall back to portal ScreenCast if needed
- [ ] **M4** Ask Claude — screenshot + question → streamed answer in the overlay
- [ ] **M5** Audio + transcription — PipeWire mic and sink monitor, VAD, local whisper.cpp
- [ ] **M6** Meeting copilot — rolling transcript, auto-suggestions, post-meeting summary
- [ ] **M7** Hide from capture (Niri) — ship the layer rule below
- [ ] Later: macOS (`NSWindow.sharingType = .none`) and Windows (`WDA_EXCLUDEFROMCAPTURE`) backends

## Hiding the overlay from screen sharing

There is no Linux-wide API for this.

- **X11:** not possible — every capturer reads the same composited screen. Share a single window instead of the whole screen.
- **Niri:** a layer rule blocks the overlay out of screencasts and screenshots. Viewers see a **black rectangle**, not the content behind it:

  ```kdl
  layer-rule {
      match namespace="^invisible-ai$"
      block-out-from "screen-capture"
  }
  ```

  Prefer `"screen-capture"` over `"screencast"`: the latter can leak via third-party screenshot tools.
- **GNOME, KDE, Hyprland:** no known equivalent.

## Notes

- **i3:** the overlay floats because it's a `UTILITY`-type window. Position and click-through are applied on GTK `map-event`: i3 ignores positions requested before the window is managed, and only copies the input shape to its frame if it's set afterwards.
- **Focus:** `focus_on_map` is off (`_NET_WM_USER_TIME=0`) so showing the overlay never steals focus; only `interact` focuses it.
- **Control socket** is Unix-only; Windows will need a named pipe.
- **Capture** includes the overlay itself. Old captures are never deleted yet.
- **Dev builds** optimize only the image/PNG/xcap crates (see `Cargo.toml`); optimizing all deps makes builds much slower.
