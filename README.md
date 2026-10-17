# invisible-ai

A screen- and meeting-aware AI overlay for Linux, in the spirit of [Cluely](https://cluely.com/).
A translucent, click-through panel sits on top of your desktop; hotkeys capture the screen
and audio, and answers from Claude stream into the overlay.

Linux-first (X11 and Wayland), designed so macOS/Windows backends can be added later.

## Status

Early development. Works today: transparent, click-through, always-on-top overlay on X11 (tested on i3 + picom).

## Requirements

- Nix with flakes (the dev shell provides Rust, Tauri v2, WebKitGTK, PipeWire, etc.)
- X11: a compositor (e.g. picom) for transparency
- Wayland: a compositor with `wlr-layer-shell` (Niri, Sway, Hyprland, KDE) — not yet implemented

## Run

```sh
nix develop -c cargo run
```

The first build takes a few minutes. The overlay ignores mouse input, so until hotkeys
exist, stop it with:

```sh
pkill -f 'target/debug/invisible-ai'
```

> Nix flakes only see files tracked by git — `git add` new files before `nix develop`.

## Layout

```
flake.nix      dev shell
app/           Tauri v2 app (Rust): window setup, platform glue
ui/            overlay frontend (static HTML, no build step)
```

Backend crates (`core`, `x11`, `wayland`, `audio`, `ai`) are added as their milestones land.

## Roadmap

- [x] **M0** Scaffold — flake dev shell, Tauri v2 workspace
- [x] **M1** Overlay (X11) — transparent, click-through, always on top, sticky, no taskbar entry, floats in i3
- [ ] **M1b** Overlay (Wayland/Niri) — `gtk-layer-shell` surface, namespace `invisible-ai`
- [ ] **M2** Control — Unix socket + `iai` CLI (`toggle`, `interact`, `ask`, `listen`), i3/Niri keybinding examples
- [ ] **M3** Screen capture — XShm on X11; xdg-desktop-portal ScreenCast (PipeWire, restore token) on Wayland
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

- **i3:** the overlay floats because it's a `UTILITY`-type window. Click-through is enabled on GTK `map-event`, because i3 only copies the input shape to its frame if it's set after the window is managed.
