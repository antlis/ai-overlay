# invisible-ai

A screen- and meeting-aware AI overlay for Linux, in the spirit of [Cluely](https://cluely.com/).
A translucent, click-through panel sits on top of your desktop; hotkeys capture the screen
and audio, and answers from Claude stream into the overlay.

Linux-first (X11 and Wayland), designed so macOS/Windows backends can be added later.

## Status

Early development. Works today on X11 (tested on i3 + picom): transparent, click-through,
always-on-top overlay controlled over a Unix socket; screenshot + question → streamed answer
from any OpenAI-compatible provider.

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
| `invisible-ai fullscreen` | Switch between the compact top-center panel and a full-screen panel |
| `invisible-ai capture` | Screenshot the overlay's monitor (without the overlay); prints the PNG path (`$XDG_CACHE_HOME/invisible-ai/captures/`) |
| `invisible-ai listen` | Start / stop the live transcript (mic = "Me", system audio = "Them") |
| `invisible-ai ask [question]` | Screenshot + question → streamed answer in the overlay (default question: help with what's on screen) |
| `invisible-ai models <provider>` | List the model ids a provider advertises (`refresh-models` refetches instead of reading the 24 h cache) |
| `invisible-ai prompt` | Show the panel, switch to interactive and focus the question input (an "Ask" hotkey) |
| `invisible-ai stop` | Stop the answer that is streaming (and speaking) |
| `invisible-ai copy` | Copy the last answer to the clipboard |
| `invisible-ai clear` | Clear everything: history, follow-up context, transcript |
| `invisible-ai scroll up\|down\|top\|bottom` | Scroll the answers a page at a time |
| `invisible-ai language [code]` | Voice language: next of `[voice] language`, or the given one |
| `invisible-ai speak [on\|off]` | Toggle spoken answers (local Piper voices) |
| `invisible-ai hush` | Stop speaking the current answer |
| `invisible-ai --help` | List the commands (doesn't start the app) |
| `invisible-ai quit` | Exit |

The same actions are in the **tray icon** menu and the panel header (Ask / fullscreen / hide),
which stays clickable in click-through mode. **Ask** opens a text box: type a question, Enter
sends it with a screenshot (empty = default question), Esc cancels. Only one instance runs at a time; any command starts the app if it isn't running (so a `toggle` hotkey also launches it; its log goes to `~/.cache/invisible-ai/app.log`). Bind the commands in your WM, e.g. i3:

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

## Panel settings

The ⚙ button opens the settings view (typing needs input, so it switches to interactive mode and
back on close):

- **Provider / model** — the model picker is a dropdown fed from the provider's own
  `GET {base_url}/models` (cached on disk for a day under `~/.cache/invisible-ai/models/`,
  ⟳ refetches). If the configured model isn't in the catalog it still shows as `… (configured)`,
  and *Other…* keeps a free-text field for ids the catalog doesn't list. Providers without a
  catalog (or when offline) fall back to typing.
- **Default question** — asked when you send an empty prompt.
- **Background opacity / text size** — live, persisted per machine in `localStorage`
  (same for the answer history, last 20 entries).
- **Hotkeys** — the bindings from your WM config, with a ⧉ button that copies the command to
  paste into a new binding.

## Live transcript

`listen` (or 🎙 in the header, or the tray) starts a local [whisper.cpp](https://github.com/ggml-org/whisper.cpp)
server and records the mic and the system output separately with `pw-record`. Speech is cut at
pauses (~750 ms) and transcribed locally, so no audio leaves the machine. Download the model once:

```sh
mkdir -p ~/.local/share/invisible-ai/models
curl -L -o ~/.local/share/invisible-ai/models/ggml-base.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin
```

Questions (lines Whisper ends with `?`) and requests ("Explain …", "Tell me …") are answered
automatically with the screen and recent transcript as context (`auto_answer` in `[voice]`).
Mic lines that repeat what the speakers just said are dropped as echo (compared by words, so background music doesn't mute you). Each transcribed line is logged to stderr. If
`whisper-server` crashes it's restarted; its log is `~/.cache/invisible-ai/whisper-server.log`
and the clip it died on is kept as `~/.cache/invisible-ai/crash-*.wav`.

Settings are under `[voice]` in the config. `language = "en,ru"` lists the languages the **EN/RU**
button (or `invisible-ai language [code]`) switches between; the first is the default. Each clip
is transcribed in the selected language: Whisper's own detection misreads short clips, especially
Russian with `base`. On an i7-8650U, `base` transcribes ~3x faster than real time; `small` is
slower than real time. Use headphones: on speakers the mic also hears the other side, so their
words show up as "Me" too.

### Spoken answers

The speaker button in the header (or `invisible-ai speak [on|off]`) reads answers aloud with
[Piper](https://github.com/OHF-Voice/piper1-gpl), locally: sentence by sentence while the answer
streams, Markdown stripped, code blocks replaced by "The code is on screen". A Cyrillic answer
uses the Russian voice. While it speaks, a click on the speaker (or `invisible-ai hush`, or
Stop) silences it, and the live transcript skips what was recorded meanwhile, so the app never
transcribes, or answers, its own voice. Download the voices once:

```sh
mkdir -p ~/.local/share/invisible-ai/voices && cd ~/.local/share/invisible-ai/voices
base=https://huggingface.co/rhasspy/piper-voices/resolve/main
for v in en/en_US/lessac/medium/en_US-lessac-medium ru/ru_RU/irina/medium/ru_RU-irina-medium; do
  curl -LO "$base/$v.onnx" -LO "$base/$v.onnx.json"
done
```

Other voices: [piper-voices](https://huggingface.co/rhasspy/piper-voices); set them under
`[speech]` (see `config.example.toml`). On speakers, people in a call hear the answer through
your mic; use headphones.

## AI provider

Any OpenAI-compatible chat-completions API with a vision model works (NanoGPT, OpenCode Zen,
OpenRouter, Ollama, …). Copy `config.example.toml` to `~/.config/invisible-ai/config.toml`
and set `provider` to one of the `[providers.*]` entries. Keys come from an env var
(`api_key_env`) or a shell command (`api_key_cmd`, e.g. reading opencode's `auth.json`),
so no key is stored in the config. The config is re-read on every `ask`.

OpenCode Zen's free models (`*-free`) refuse requests from anything but OpenCode itself.

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
  src/ai.rs        provider config + streaming chat-completions client
  src/tray.rs      tray icon + menu
  src/voice.rs     live transcript: pw-record capture, pause detection, whisper-server
config.example.toml
ui/            overlay frontend (static HTML, no build step; libs vendored in ui/vendor/)
```

Backend crates (`core`, `x11`, `wayland`, `audio`, `ai`) are added as their milestones land.

## Roadmap

- [x] **M0** Scaffold — flake dev shell, Tauri v2 workspace
- [x] **M1** Overlay (X11) — transparent, click-through, always on top, sticky, no taskbar entry, floats in i3
- [ ] **M1b** Overlay (Wayland/Niri) — `gtk-layer-shell` surface, namespace `invisible-ai`
- [x] **M2** Control — Unix socket + CLI (`toggle`, `interact`, `quit`), i3/Niri keybinding examples; `ask`/`listen` land with M4/M5
- [x] **M3** Screen capture (X11) — `capture` command via [xcap](https://crates.io/crates/xcap), ~200 ms per 1080p PNG
- [ ] **M3b** Screen capture (Wayland) — xcap tries wlroots screencopy / portal Screenshot; verify on Niri, fall back to portal ScreenCast if needed
- [x] **M4** Ask — screenshot + question → streamed answer in the overlay, any OpenAI-compatible provider
- [x] **UI** Tray icon menu, fullscreen toggle, header buttons in interactive mode
- [x] **M4b** Markdown answers with syntax-highlighted code (marked + highlight.js, sanitized with DOMPurify, vendored in `ui/vendor/`)
- [ ] Native Anthropic API (Claude) provider
- [x] **M5** Live transcript — `pw-record` mic + sink monitor, energy-based pause detection, local whisper.cpp (`base`)
- [ ] **M6** Meeting copilot — feed the transcript into `ask`, auto-suggestions, post-meeting summary
- [ ] **M7** Invisible in screen shares (Niri) — layer rule + Niri patch that skips drawing blocked-out layers
- [ ] Later: macOS (`NSWindow.sharingType = .none`) and Windows (`WDA_EXCLUDEFROMCAPTURE`) backends

## Hiding the overlay from screen sharing

Goal: the overlay is fully invisible in screen shares, with viewers seeing the desktop underneath
(as on macOS/Windows), not a black box.

- **X11:** not possible. Every capturer reads the same final screen image, and there's no
  per-window capture exclusion.
- **Wayland (Niri), planned:** the overlay becomes a layer-shell surface (namespace
  `invisible-ai`), blocked out of screencasts by a layer rule:

  ```kdl
  layer-rule {
      match namespace="^invisible-ai$"
      block-out-from "screen-capture"
  }
  ```

  Stock Niri draws a black rectangle in its place. A small Niri patch skips drawing blocked-out
  layer surfaces instead (`src/layer/mapped.rs`), so the share shows what's underneath.

## Notes

- **i3:** the overlay floats because it's a `UTILITY`-type window. Position and click-through are applied on GTK `map-event`: i3 ignores positions requested before the window is managed, and only copies the input shape to its frame if it's set afterwards.
- **Focus:** `focus_on_map` is off (`_NET_WM_USER_TIME=0`) so showing the overlay never steals focus; only `interact` focuses it.
- **Control socket** is Unix-only; Windows will need a named pipe.
- **Capture** never hides the panel: in compact mode its area is blanked out of the screenshot; in fullscreen mode questions go without a screenshot (text + transcript only).
- **Scrolling** in click-through mode: use the mouse wheel over the right edge of the panel (a 28 px strip that takes input). Old captures from `capture` are never deleted yet; `ask` keeps them in memory only.
- **Rendering:** the app sets `WEBKIT_FORCE_COMPOSITING_MODE=1`; WebKitGTK's on-demand switch into compositing mode (e.g. when an answer gets long enough to scroll) otherwise leaves the transparent overlay partly blank. Also don't set `WEBKIT_DISABLE_DMABUF_RENDERER=1`: partial repaints then blank the rest of the overlay.
- **Tray** uses libayatana-appindicator, which falls back to XEmbed for i3bar.
- **Dev builds** optimize only the image/PNG/xcap crates (see `Cargo.toml`); optimizing all deps makes builds much slower.
