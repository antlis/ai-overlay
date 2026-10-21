# ai-overlay

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

The running app listens on `$XDG_RUNTIME_DIR/ai-overlay.sock`. The same binary, given a
command, sends it there:

| Command | Effect |
|---|---|
| `ai-overlay toggle` | Show / hide the overlay |
| `ai-overlay interact` | Switch between click-through and interactive (clickable, focused, purple border) |
| `ai-overlay fullscreen` | Switch between the compact top-center panel and a full-screen panel |
| `ai-overlay capture` | Screenshot the overlay's monitor (without the overlay); prints the PNG path (`$XDG_CACHE_HOME/ai-overlay/captures/`) |
| `ai-overlay listen` | Start / stop the live transcript (mic = "Me", system audio = "Them") |
| `ai-overlay ask [question]` | Screenshot + question → streamed answer in the overlay (default question: help with what's on screen) |
| `ai-overlay models <provider>` | List the model ids a provider advertises (`refresh-models` refetches instead of reading the 24 h cache) |
| `ai-overlay prompt` | Show the panel, switch to interactive and focus the question input (an "Ask" hotkey) |
| `ai-overlay stop` | Stop the answer that is streaming (and speaking) |
| `ai-overlay copy` | Copy the last answer to the clipboard |
| `ai-overlay clear` | Clear everything: history, follow-up context, transcript |
| `ai-overlay scroll up\|down\|top\|bottom` | Scroll the answers a page at a time |
| `ai-overlay language [code]` | Voice language: next of `[voice] language`, or the given one |
| `ai-overlay speak [on\|off]` | Toggle spoken answers (local Piper voices) |
| `ai-overlay hush` | Stop speaking the current answer |
| `ai-overlay --help` | List the commands (doesn't start the app) |
| `ai-overlay quit` | Exit |

The same actions are in the **tray icon** menu and the panel header (Ask / fullscreen / hide),
which stays clickable in click-through mode. **Ask** opens a text box: type a question, Enter
sends it with a screenshot (empty = default question), Esc cancels. Only one instance runs at a time; any command starts the app if it isn't running (so a `toggle` hotkey also launches it; its log goes to `~/.cache/ai-overlay/app.log`). Bind the commands in your WM, e.g. i3:

```
bindsym $mod+backslash       exec --no-startup-id /path/to/ai-overlay toggle
bindsym $mod+Shift+backslash exec --no-startup-id /path/to/ai-overlay interact
```

Niri:

```kdl
binds {
    Mod+Backslash       { spawn "/path/to/ai-overlay" "toggle"; }
    Mod+Shift+Backslash { spawn "/path/to/ai-overlay" "interact"; }
}
```

## Panel settings

The ⚙ button opens the settings view (typing needs input, so it switches to interactive mode and
back on close):

- **Provider / model** — the model picker is a dropdown fed from the provider's own
  `GET {base_url}/models` (cached on disk for a day under `~/.cache/ai-overlay/models/`,
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
mkdir -p ~/.local/share/ai-overlay/models
curl -L -o ~/.local/share/ai-overlay/models/ggml-base.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin
```

Questions (lines Whisper ends with `?`) and requests ("Explain …", "Tell me …") are answered
automatically with the screen and recent transcript as context (`auto_answer` in `[voice]`).
Mic lines that repeat what the speakers just said are dropped as echo (compared by words, so background music doesn't mute you). Each transcribed line is logged to stderr. If
`whisper-server` crashes it's restarted; its log is `~/.cache/ai-overlay/whisper-server.log`
and the clip it died on is kept as `~/.cache/ai-overlay/crash-*.wav`.

Settings are under `[voice]` in the config. `language = "en,ru"` lists the languages the **EN/RU**
button (or `ai-overlay language [code]`) switches between; the first is the default. Each clip
is transcribed in the selected language: Whisper's own detection misreads short clips, especially
Russian with `base`. On an i7-8650U, `base` transcribes ~3x faster than real time; `small` is
slower than real time. Use headphones: on speakers the mic also hears the other side, so their
words show up as "Me" too.

### Spoken answers

The speaker button in the header (or `ai-overlay speak [on|off]`) reads answers aloud with
[Piper](https://github.com/OHF-Voice/piper1-gpl), locally: sentence by sentence while the answer
streams, Markdown stripped, code blocks replaced by "The code is on screen". A Cyrillic answer
uses the Russian voice. While it speaks, a click on the speaker (or `ai-overlay hush`, or
Stop) silences it, and the live transcript skips what was recorded meanwhile, so the app never
transcribes, or answers, its own voice. Download the voices once:

```sh
mkdir -p ~/.local/share/ai-overlay/voices && cd ~/.local/share/ai-overlay/voices
base=https://huggingface.co/rhasspy/piper-voices/resolve/main
for v in en/en_US/lessac/medium/en_US-lessac-medium ru/ru_RU/irina/medium/ru_RU-irina-medium; do
  curl -LO "$base/$v.onnx" -LO "$base/$v.onnx.json"
done
```

Other voices: **Settings → Spoken answers** lists every single-speaker English and Russian
voice from [piper-voices](https://huggingface.co/rhasspy/piper-voices) (✓ = installed); picking
one downloads it (~60 MB) and saves it under `[speech]`. ▶ previews a voice, and the speed slider
sets `length_scale`. On speakers, people in a call hear the answer through
your mic; use headphones.

## AI provider

Any OpenAI-compatible chat-completions API with a vision model works (NanoGPT, OpenCode Zen,
OpenRouter, Ollama, …). Copy `config.example.toml` to `~/.config/ai-overlay/config.toml`
and set `provider` to one of the `[providers.*]` entries. Keys come from an env var
(`api_key_env`) or a shell command (`api_key_cmd`, e.g. reading opencode's `auth.json`),
so no key is stored in the config. The config is re-read on every `ask`.

OpenCode Zen's free models (`*-free`) refuse requests from anything but OpenCode itself.

Use `target/debug/ai-overlay` until there's a packaged build (or `cargo install --path app`).

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
- [ ] **M1b** Overlay (Wayland/Niri) — `gtk-layer-shell` surface, namespace `ai-overlay`
- [x] **M2** Control — Unix socket + CLI (`toggle`, `interact`, `quit`), i3/Niri keybinding examples; `ask`/`listen` land with M4/M5
- [x] **M3** Screen capture (X11) — `capture` command via [xcap](https://crates.io/crates/xcap), ~200 ms per 1080p PNG
- [ ] **M3b** Screen capture (Wayland) — xcap tries wlroots screencopy / portal Screenshot; verify on Niri, fall back to portal ScreenCast if needed
- [x] **M4** Ask — screenshot + question → streamed answer in the overlay, any OpenAI-compatible provider
- [x] **UI** Tray icon menu, fullscreen toggle, header buttons in interactive mode
- [x] **M4b** Markdown answers with syntax-highlighted code (marked + highlight.js, sanitized with DOMPurify, vendored in `ui/vendor/`)
- [ ] Native Anthropic API (Claude) provider
- [x] **M5** Live transcript — `pw-record` mic + sink monitor, energy-based pause detection, local whisper.cpp (`base`)
- [ ] **M6** Meeting copilot — feed the transcript into `ask`, auto-suggestions, post-meeting summary
- [x] **M7** Invisible in screen shares (Niri only) — layer rule + Niri patch that skips drawing blocked-out layers
- [ ] Later: macOS (`NSWindow.sharingType = .none`) and Windows (`WDA_EXCLUDEFROMCAPTURE`) backends

## Hiding the overlay from screen sharing

Goal: the overlay is fully invisible in screen shares, with viewers seeing the desktop underneath
(as on macOS/Windows), not a black box.

- **X11 (i3 etc.):** not possible to hide it from a full-screen share. Every capturer reads the
  same final screen image, and there's no per-window capture exclusion. Workarounds:
  - Share a single window instead of the screen (in Meet: "Window"). The browser captures just
    that window, so the overlay isn't in it.
  - Keep the overlay on a monitor you aren't sharing (a virtual one made with `xrandr` works too).
  - For other apps you don't need to see while sharing (mail, messengers), the i3 scratchpad
    (`move scratchpad` / `scratchpad show`) hides them, but it hides them from you too, and a shown
    scratchpad window is visible in the share. For the overlay, use `ai-overlay toggle` instead.
- **Wayland (Niri), works, but needs a patched Niri:** the overlay is a layer-shell surface
  (namespace `ai-overlay`), blocked out of screencasts by a layer rule:

  ```kdl
  layer-rule {
      match namespace="^ai-overlay$"
      block-out-from "screen-capture"
  }
  ```

  Stock Niri draws a black rectangle in the surface's place. This only worked for me on Niri
  with the patch below (`contrib/niri/niri-invisible-layers.patch`), which skips drawing
  blocked-out layer surfaces, so the share shows what's underneath. I haven't got it working
  anywhere else.

  ```diff
  --- a/src/layer/mapped.rs
  +++ b/src/layer/mapped.rs
  @@ -168,17 +168,9 @@ impl MappedLayer {
           let location = location + self.bob_offset();

           if target.should_block_out(self.rules.block_out_from) {
  -            // Round to physical pixels.
  -            let location = location.to_physical_precise_round(scale).to_logical(scale);
  -
  -            // FIXME: take geometry-corner-radius into account.
  -            let elem = SolidColorRenderElement::from_buffer(
  -                &self.block_out_buffer,
  -                location,
  -                alpha,
  -                Kind::Unspecified,
  -            );
  -            rv.normal.push(elem.into());
  +            // Draw nothing instead of a black rectangle, so screencasts and screenshots
  +            // show whatever is underneath, as if the surface weren't there.
  +            return rv;
           } else {
               // Layer surfaces don't have extra geometry like windows.
               let buf_pos = location;
  ```

  Upstream discussion: [niri-wm/niri#3685](https://github.com/niri-wm/niri/pull/3685)
  (a PR adding a hide-from rule for screen-capture targets). In
  [this comment](https://github.com/niri-wm/niri/pull/3685#issuecomment-6287890459) the
  maintainer suggests a `kind="transparent"` block-out option (or `skip` / `omit`, with the
  current fill as the default), which would make this patch unnecessary once it lands.

  The "Invisible in screen sharing" checkbox in Settings (`[ui] invisible`, on by default)
  switches the layer namespace to `ai-overlay-visible`, which the rule doesn't match.

## Notes

- **i3:** the overlay floats because it's a `UTILITY`-type window. Position and click-through are applied on GTK `map-event`: i3 ignores positions requested before the window is managed, and only copies the input shape to its frame if it's set afterwards.
- **Focus:** `focus_on_map` is off (`_NET_WM_USER_TIME=0`) so showing the overlay never steals focus; only `interact` focuses it.
- **Control socket** is Unix-only; Windows will need a named pipe.
- **Capture** never hides the panel: in compact mode its area is blanked out of the screenshot; in fullscreen mode questions go without a screenshot (text + transcript only).
- **Scrolling** in click-through mode: use the mouse wheel over the right edge of the panel (a 28 px strip that takes input). Old captures from `capture` are never deleted yet; `ask` keeps them in memory only.
- **Rendering:** the app sets `WEBKIT_FORCE_COMPOSITING_MODE=1`; WebKitGTK's on-demand switch into compositing mode (e.g. when an answer gets long enough to scroll) otherwise leaves the transparent overlay partly blank. Also don't set `WEBKIT_DISABLE_DMABUF_RENDERER=1`: partial repaints then blank the rest of the overlay.
- **Tray** uses libayatana-appindicator, which falls back to XEmbed for i3bar.
- **Dev builds** optimize only the image/PNG/xcap crates (see `Cargo.toml`); optimizing all deps makes builds much slower.
