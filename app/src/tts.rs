//! Spoken answers: local Piper text-to-speech, played with pw-play.
//!
//! One Piper process per language stays loaded (loading takes ~1.5 s, a sentence then
//! starts playing in ~0.1 s) and gets one sentence per line; its raw audio is forwarded
//! to pw-play. While audio plays, the transcript drops clips recorded at the same time,
//! so the app never transcribes (and answers) its own voice.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tauri::{AppHandle, Emitter};

/// Speak answers (toggled from the panel / `invisible-ai speak`).
static ENABLED: AtomicBool = AtomicBool::new(false);
static SPEAKER: Mutex<Option<Speaker>> = Mutex::new(None);
/// Recent playback intervals, for dropping transcript clips that overlap them.
static PLAYED: Mutex<VecDeque<(Instant, Instant)>> = Mutex::new(VecDeque::new());
/// Piper voices output 16-bit mono at this rate (the medium/high voices; read from the
/// voice's .onnx.json when present).
const DEFAULT_RATE: u32 = 22050;

#[derive(Deserialize)]
#[serde(default)]
pub struct SpeechConfig {
    /// Piper voice (.onnx, with its .onnx.json next to it) per language; `~` expanded.
    pub en: String,
    pub ru: String,
    /// Speed: < 1 faster, > 1 slower (Piper's length scale).
    pub length_scale: f32,
}

impl Default for SpeechConfig {
    fn default() -> Self {
        Self {
            en: "~/.local/share/invisible-ai/voices/en_US-lessac-medium.onnx".into(),
            ru: "~/.local/share/invisible-ai/voices/ru_RU-irina-medium.onnx".into(),
            length_scale: 1.0,
        }
    }
}

struct Speaker {
    lang: &'static str,
    piper: Child,
    input: ChildStdin,
    player: Child,
}

impl Drop for Speaker {
    fn drop(&mut self) {
        let _ = self.piper.kill();
        let _ = self.player.kill();
        let _ = self.piper.wait();
        let _ = self.player.wait();
    }
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}

/// Turn spoken answers on/off (`None` toggles); off also stops speaking.
pub fn set_enabled(app: &AppHandle, on: Option<bool>) -> bool {
    let on = on.unwrap_or(!enabled());
    ENABLED.store(on, Ordering::SeqCst);
    if !on {
        stop(app);
    }
    let _ = app.emit("speech", on);
    on
}

/// Stop speaking now (drops queued audio).
pub fn stop(app: &AppHandle) {
    if SPEAKER.lock().unwrap().take().is_some() {
        if let Ok(mut played) = PLAYED.lock() {
            if let Some(last) = played.back_mut() {
                last.1 = last.1.min(Instant::now());
            }
        }
        let _ = app.emit("speaking", false);
    }
}

/// Whether the app was speaking at any point in [`start`, `end`] (plus a margin for
/// echo and pw-play's buffer).
pub fn overlaps(start: Instant, end: Instant) -> bool {
    const MARGIN: Duration = Duration::from_millis(400);
    let played = PLAYED.lock().unwrap();
    played.iter().any(|&(from, to)| start <= to + MARGIN && from <= end)
}

/// Speak one sentence. `lang` is "en" or "ru".
pub fn say(app: &AppHandle, config: &SpeechConfig, lang: &'static str, sentence: &str) -> Result<(), String> {
    let sentence = sentence.replace('\n', " ");
    if sentence.trim().is_empty() {
        return Ok(());
    }
    let mut speaker = SPEAKER.lock().unwrap();
    if speaker.as_ref().is_some_and(|s| s.lang != lang) {
        *speaker = None; // other voice: start that one
    }
    if speaker.is_none() {
        *speaker = Some(spawn(app, config, lang)?);
    }
    let s = speaker.as_mut().unwrap();
    if writeln!(s.input, "{sentence}").and_then(|_| s.input.flush()).is_err() {
        *speaker = None; // Piper died; next sentence restarts it
        return Err("piper stopped".into());
    }
    Ok(())
}

fn expand(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", std::env::var("HOME").unwrap_or_default()),
        None => path.to_owned(),
    }
}

fn sample_rate(model: &str) -> u32 {
    std::fs::read_to_string(format!("{model}.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| v["audio"]["sample_rate"].as_u64())
        .map_or(DEFAULT_RATE, |r| r as u32)
}

fn spawn(app: &AppHandle, config: &SpeechConfig, lang: &'static str) -> Result<Speaker, String> {
    let model = expand(if lang == "ru" { &config.ru } else { &config.en });
    if !std::path::Path::new(&model).exists() {
        return Err(format!("Piper voice not found: {model} (see README, Spoken answers)"));
    }
    let rate = sample_rate(&model);
    let mut piper = Command::new("piper")
        .args(["-m", &model, "--output-raw", "--length-scale", &config.length_scale.to_string()])
        // Piper is self-contained; an inherited LD_LIBRARY_PATH (the app's GTK libs, or a
        // stray older libstdc++) can break its onnxruntime.
        .env_remove("LD_LIBRARY_PATH")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot run piper: {e}"))?;
    let mut player = Command::new("pw-play")
        .args(["--rate", &rate.to_string(), "--channels", "1", "--format", "s16", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot run pw-play: {e}"))?;
    let input = piper.stdin.take().ok_or("piper: no stdin")?;
    let mut audio = piper.stdout.take().ok_or("piper: no stdout")?;
    let mut out = player.stdin.take().ok_or("pw-play: no stdin")?;

    // Forward Piper's audio to pw-play, tracking how long it will play for.
    let app = app.clone();
    std::thread::spawn(move || {
        let bytes_per_sec = f64::from(rate) * 2.0;
        let mut buf = [0u8; 8192];
        let watcher_started = AtomicBool::new(false);
        loop {
            let n = match audio.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            if out.write_all(&buf[..n]).is_err() {
                break;
            }
            let now = Instant::now();
            let length = Duration::from_secs_f64(n as f64 / bytes_per_sec);
            let mut played = PLAYED.lock().unwrap();
            match played.back_mut() {
                Some(last) if last.1 + Duration::from_millis(50) >= now => last.1 += length,
                _ => {
                    played.push_back((now, now + length));
                    let _ = app.emit("speaking", true);
                    if !watcher_started.swap(true, Ordering::SeqCst) {
                        watch_end(app.clone());
                    }
                }
            }
            while played.len() > 32 {
                played.pop_front();
            }
        }
    });
    Ok(Speaker { lang, piper, input, player })
}

/// Tell the panel when playback has run out.
fn watch_end(app: AppHandle) {
    std::thread::spawn(move || {
        let mut speaking = true;
        loop {
            std::thread::sleep(Duration::from_millis(200));
            if SPEAKER.lock().unwrap().is_none() {
                return;
            }
            let until = PLAYED.lock().unwrap().back().map(|p| p.1);
            let now_speaking = until.is_some_and(|until| until > Instant::now());
            if now_speaking != speaking {
                speaking = now_speaking;
                let _ = app.emit("speaking", speaking);
            }
        }
    });
}

/// Turns a streamed Markdown answer into sentences to speak: code blocks are replaced
/// by a short note, Markdown syntax is stripped.
#[derive(Default)]
pub struct Sentences {
    buf: String,
    in_code: bool,
    /// Language of this answer, decided by its first letters (Cyrillic → "ru").
    lang: Option<&'static str>,
}

impl Sentences {
    /// Add streamed text; returns the sentences completed so far.
    pub fn push(&mut self, delta: &str) -> Vec<String> {
        self.buf.push_str(delta);
        let mut out = Vec::new();
        loop {
            if self.in_code {
                match self.buf.find("```") {
                    Some(i) => {
                        // Drop the code and the rest of the closing fence's line.
                        let rest = &self.buf[i + 3..];
                        let end = rest.find('\n').map_or(rest.len(), |n| n + 1);
                        self.buf = rest[end..].to_owned();
                        self.in_code = false;
                    }
                    None => {
                        // Keep a possible partial fence at the end.
                        let keep = self.buf.len().saturating_sub(2);
                        let keep = (0..=keep).rev().find(|&k| self.buf.is_char_boundary(k)).unwrap_or(0);
                        self.buf.drain(..keep);
                        return out;
                    }
                }
                continue;
            }
            let fence = self.buf.find("```");
            let end = sentence_end(&self.buf);
            match (fence, end) {
                (Some(f), e) if e.is_none_or(|e| f < e) => {
                    let before = self.buf[..f].to_owned();
                    self.add(&mut out, &before);
                    let note = if self.lang == Some("ru") { "Код — на экране." } else { "The code is on screen." };
                    out.push(note.to_owned());
                    self.buf.drain(..f + 3);
                    self.in_code = true;
                }
                (_, Some(e)) => {
                    let sentence: String = self.buf.drain(..e).collect();
                    self.add(&mut out, &sentence);
                }
                _ => return out,
            }
        }
    }

    /// The rest, once the answer is complete.
    pub fn finish(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.in_code {
            let rest = std::mem::take(&mut self.buf);
            self.add(&mut out, &rest);
        }
        out
    }

    pub fn lang(&self) -> &'static str {
        self.lang.unwrap_or("en")
    }

    fn add(&mut self, out: &mut Vec<String>, text: &str) {
        let text = plain(text);
        if text.chars().any(char::is_alphanumeric) {
            if self.lang.is_none() {
                if let Some(c) = text.chars().find(|c| c.is_alphabetic()) {
                    self.lang = Some(if ('\u{0400}'..='\u{04FF}').contains(&c) { "ru" } else { "en" });
                }
            }
            out.push(text);
        }
    }
}

/// Byte index just past the first sentence end: ".", "!", "?" (or "…") followed by
/// whitespace, or a newline.
fn sentence_end(text: &str) -> Option<usize> {
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '\n' {
            return Some(i + 1);
        }
        if matches!(c, '.' | '!' | '?' | '…') {
            if let Some(&(j, next)) = chars.peek() {
                if next.is_whitespace() {
                    return Some(j);
                }
            }
        }
    }
    None
}

/// Strip Markdown: emphasis, inline code ticks, heading/list/quote markers, links
/// (keep the text), tables' pipes.
fn plain(text: &str) -> String {
    let mut s = text.trim().to_owned();
    for prefix in ["#### ", "### ", "## ", "# ", "> ", "- ", "* ", "+ "] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_owned();
            break;
        }
    }
    // "1. item" → "item"
    if let Some(dot) = s.find(". ") {
        if dot > 0 && dot <= 3 && s[..dot].chars().all(|c| c.is_ascii_digit()) {
            s = s[dot + 2..].to_owned();
        }
    }
    // [text](url) → text
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find("](").map(|c| open + c) else { break };
        let Some(paren) = rest[close..].find(')').map(|p| close + p) else { break };
        out.push_str(&rest[..open]);
        out.push_str(&rest[open + 1..close]);
        rest = &rest[paren + 1..];
    }
    out.push_str(rest);
    out.chars().filter(|c| !matches!(c, '*' | '_' | '`' | '|' | '#')).collect::<String>().trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speak(chunks: &[&str]) -> Vec<String> {
        let mut s = Sentences::default();
        let mut out: Vec<String> = chunks.iter().flat_map(|c| s.push(c)).collect();
        out.extend(s.finish());
        out
    }

    #[test]
    fn splits_sentences_across_chunks() {
        assert_eq!(
            speak(&["A **closure** rem", "embers things. It keeps", " `count`!"]),
            ["A closure remembers things.", "It keeps count!"]
        );
    }

    #[test]
    fn skips_code_blocks_and_strips_markdown() {
        let out = speak(&["Here:\n```js\nconst a = 1;\n", "```\n- See [docs](https://x.y) for more.\n"]);
        assert_eq!(out, ["Here:", "The code is on screen.", "See docs for more."]);
    }

    #[test]
    fn picks_russian_voice_for_cyrillic() {
        let mut s = Sentences::default();
        s.push("Замыкание — это функция. ");
        assert_eq!(s.lang(), "ru");
    }
}
