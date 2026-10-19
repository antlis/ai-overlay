//! Live transcription: PipeWire capture (mic = "me", system output = "them"),
//! energy-based segmentation at pauses, and a local whisper.cpp server.

use std::collections::VecDeque;
use std::io::{BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

const RATE: usize = 16_000;
const FRAME: usize = RATE * 30 / 1000; // 30 ms
const PRE_ROLL_FRAMES: usize = 7; // keep ~200 ms before speech starts
const END_SILENCE_FRAMES: usize = 25; // ~750 ms of silence ends a segment
const MIN_SPEECH_FRAMES: usize = 10; // ignore blips under ~300 ms
const MAX_SEGMENT: usize = RATE * 15;
/// Pending system-audio clips beyond this are dropped (oldest first) so transcription
/// stays live. Mic clips are never dropped.
const MAX_BACKLOG: usize = 2;
/// How long system-audio lines are remembered for spotting speaker echo on the mic.
const ECHO_WINDOW: Duration = Duration::from_secs(30);
/// Mic clips wait this long so the matching system-audio clip (if it's echo) is
/// transcribed first and can be compared.
const MIC_DELAY: Duration = Duration::from_millis(1200);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[derive(Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    /// whisper.cpp ggml model; `~` is expanded.
    pub model: String,
    /// Languages to switch between in the panel ("en,ru"); the first is the default.
    /// Each clip is transcribed in the selected one.
    pub language: String,
    pub threads: u32,
    /// Local port for whisper-server.
    pub port: u16,
    /// Answer automatically when a transcribed line is a question or request.
    pub auto_answer: bool,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            model: "~/.local/share/invisible-ai/models/ggml-base.bin".into(),
            language: "en,ru".into(),
            threads: 4,
            port: 8178,
            auto_answer: true,
        }
    }
}

/// Speaker, audio, and when the clip ended.
type Segment = (&'static str, Vec<i16>, Instant);

/// Whether a transcribed line asks for something: a question (Whisper adds "?")
/// or a request that starts with a command word ("Explain …", "Tell me …"),
/// which Whisper doesn't punctuate as a question. Command words: English and Russian.
pub fn is_question(text: &str) -> bool {
    const REQUESTS: &[&str] = &[
        "explain", "tell", "describe", "write", "show", "give", "compare", "define", "help",
        "list", "summarize", "summarise",
    ];
    // Russian request verbs by stem: Whisper often garbles the ending or fuses the next
    // word ("Объясни мне" → "Объяснимли").
    const STEMS: &[&str] = &[
        "объясн", "расскаж", "подскаж", "скаж", "покаж", "напиш", "опиш", "сравн", "помог",
        "перечисл", "дай", "найд", "переведи", "исправ",
    ];
    let text = text.trim();
    // Leading letters of the first word, so "What's" counts as "what".
    let first: String = text
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .chars()
        .take_while(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    text.ends_with('?')
        || REQUESTS.contains(&first.as_str())
        || STEMS.iter().any(|stem| first.starts_with(stem))
}

/// How many transcript lines `recent()` keeps as context for questions.
const RECENT_LINES: usize = 40;
static RECENT: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// Forget the transcript (it's no longer sent as context).
pub fn clear_recent() {
    if let Ok(mut recent) = RECENT.lock() {
        recent.clear();
    }
}

/// Languages to switch between (from `voice.language`, e.g. "en,ru") and the current one,
/// which every clip is transcribed in. Starts with the first; set at runtime.
static LANGUAGES: Mutex<(Vec<String>, usize)> = Mutex::new((Vec::new(), 0));

/// Take the switchable languages from the config; keeps the current one if still listed.
pub fn set_options(config: &str) {
    let options: Vec<String> = config
        .split(',')
        .map(|l| l.trim().to_lowercase())
        .filter(|l| !l.is_empty())
        .collect();
    let options = if options.is_empty() { vec!["en".to_owned()] } else { options };
    let mut state = LANGUAGES.lock().unwrap();
    let current = state.0.get(state.1).cloned();
    state.1 = current.and_then(|c| options.iter().position(|o| *o == c)).unwrap_or(0);
    state.0 = options;
}

/// (current, options).
pub fn languages() -> (String, Vec<String>) {
    let state = LANGUAGES.lock().unwrap();
    (state.0.get(state.1).cloned().unwrap_or_else(|| "en".into()), state.0.clone())
}

fn language() -> String {
    languages().0
}

/// Switch to `code` (any Whisper language code), or with `None` to the next option.
pub fn switch_language(code: Option<&str>) -> String {
    let mut state = LANGUAGES.lock().unwrap();
    match code.map(str::to_lowercase) {
        Some(code) => {
            state.1 = match state.0.iter().position(|o| *o == code) {
                Some(i) => i,
                None => {
                    state.0.push(code);
                    state.0.len() - 1
                }
            }
        }
        None if !state.0.is_empty() => state.1 = (state.1 + 1) % state.0.len(),
        None => {}
    }
    state.0.get(state.1).cloned().unwrap_or_else(|| "en".into())
}

/// The latest transcript lines ("Me: ..." / "Them: ..."), oldest first.
pub fn recent() -> String {
    RECENT.lock().map(|r| r.iter().cloned().collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

/// Both pw-record processes while listening.
static SESSION: Mutex<Vec<Child>> = Mutex::new(Vec::new());
/// whisper-server while listening; restarted by the worker if it dies.
static SERVER: Mutex<Option<Child>> = Mutex::new(None);
/// Bumped per listening session, so a finishing worker only stops its own server.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// How to (re)start whisper-server.
#[derive(Clone)]
struct Server {
    model: String,
    language: String,
    threads: String,
    port: u16,
}

impl Server {
    /// Spawn whisper-server and wait until it accepts connections.
    fn spawn(&self, fresh_log: bool) -> Result<Child, String> {
        let log = server_log(fresh_log)?;
        let mut child = Command::new("whisper-server")
            .args(["-m", &self.model, "-l", &self.language, "-t", &self.threads])
            .args(["--host", "127.0.0.1", "--port", &self.port.to_string()])
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log)
            .spawn()
            .map_err(|e| format!("cannot run whisper-server: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(30);
        while TcpStream::connect(("127.0.0.1", self.port)).is_err() {
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!("whisper-server exited: {status}"));
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                return Err("whisper-server did not start within 30 s".into());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Ok(child)
    }

    /// If whisper-server has died, log why, keep the clip it died on, and restart it.
    /// Returns whether it was restarted.
    fn restart_if_dead(&self, clip: &[i16]) -> Result<bool, String> {
        let mut server = SERVER.lock().map_err(|e| e.to_string())?;
        let Some(child) = server.as_mut() else { return Ok(false) };
        let Ok(Some(status)) = child.try_wait() else { return Ok(false) };
        let saved = cache_dir().map(|dir| {
            let path = dir.join(format!("crash-{}.wav", now_ms()));
            let _ = std::fs::write(&path, wav(clip));
            path
        });
        eprintln!("whisper-server died ({status}); clip saved to {saved:?}; restarting");
        *server = Some(self.spawn(false)?);
        Ok(true)
    }
}

/// Start or stop listening. Returns whether it's listening now.
/// `on_line(speaker, text)` runs on the transcription thread for every line.
pub fn toggle(
    app: &AppHandle,
    config: &VoiceConfig,
    on_line: impl Fn(&str, &str) + Send + 'static,
) -> Result<bool, String> {
    let mut children = SESSION.lock().map_err(|e| e.to_string())?;
    if !children.is_empty() {
        // Only stop recording: speech in progress is flushed as a last clip, and the
        // worker transcribes what's queued before it shuts whisper-server down.
        kill_all(&mut children);
        return Ok(false);
    }
    if let Ok(mut recent) = RECENT.lock() {
        recent.clear();
    }
    if let Err(e) = start(app, config, &mut children, on_line) {
        kill_all(&mut children);
        stop_server();
        return Err(e);
    }
    Ok(true)
}

/// Kill all child processes right away; called on exit.
pub fn shutdown() {
    if let Ok(mut children) = SESSION.lock() {
        kill_all(&mut children);
    }
    stop_server();
}

fn kill_all(children: &mut Vec<Child>) {
    for child in children.iter_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
    children.clear();
}

fn stop_server() {
    if let Some(mut server) = SERVER.lock().ok().and_then(|mut s| s.take()) {
        let _ = server.kill();
        let _ = server.wait();
    }
}

fn start(
    app: &AppHandle,
    config: &VoiceConfig,
    children: &mut Vec<Child>,
    on_line: impl Fn(&str, &str) + Send + 'static,
) -> Result<(), String> {
    let model = match config.model.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", std::env::var("HOME").unwrap_or_default()),
        None => config.model.clone(),
    };
    if !std::path::Path::new(&model).exists() {
        return Err(format!("whisper model not found: {model} (see README)"));
    }
    let server = Server {
        model,
        language: if config.language.contains(',') { "auto".into() } else { config.language.clone() },
        threads: config.threads.to_string(),
        port: config.port,
    };
    // A previous session's worker may still be finishing; this session takes over.
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    stop_server();
    // Loading the model takes a few seconds; the panel shows that it's starting.
    let _ = app.emit("voice", "starting");
    *SERVER.lock().map_err(|e| e.to_string())? = Some(server.spawn(true)?);

    let (tx, rx) = mpsc::channel::<Segment>();
    let monitor: &[&str] = &["-P", "{ stream.capture.sink = true }", "--target", "@DEFAULT_AUDIO_SINK@"];
    for (speaker, extra) in [("me", &[][..]), ("them", monitor)] {
        let mut child = Command::new("pw-record")
            .args(["--rate", "16000", "--channels", "1", "--format", "s16", "--raw"])
            .args(extra)
            .arg("-")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot run pw-record: {e}"))?;
        let stdout = child.stdout.take().ok_or("pw-record has no stdout")?;
        children.push(child);
        let tx = tx.clone();
        std::thread::spawn(move || split_speech(stdout, speaker, tx));
    }
    drop(tx);

    // One worker so transcription never runs twice at once on the CPU.
    // It ends when both recorders are killed and their segmenters return.
    let app = app.clone();
    let url = format!("http://127.0.0.1:{}/inference", config.port);
    set_options(&config.language);
    std::thread::spawn(move || {
        let mut queue: VecDeque<Segment> = VecDeque::new();
        // Recent system-audio lines; on speakers the mic hears them too.
        let mut heard: VecDeque<(Instant, String)> = VecDeque::new();
        loop {
            if queue.is_empty() {
                match rx.recv() {
                    Ok(segment) => queue.push_back(segment),
                    Err(_) => break,
                }
            }
            queue.extend(rx.try_iter());
            while queue.iter().filter(|(speaker, ..)| *speaker == "them").count() > MAX_BACKLOG {
                if let Some(i) = queue.iter().position(|(speaker, ..)| *speaker == "them") {
                    queue.remove(i);
                    eprintln!("transcription is behind; dropped an old system-audio clip");
                }
            }
            // System audio first; a mic clip only once it's old enough that its echo
            // source (if any) has been queued.
            let next = match queue.iter().position(|(speaker, ..)| *speaker == "them") {
                Some(i) => i,
                None => {
                    let age = queue.front().map_or(MIC_DELAY, |(_, _, at)| at.elapsed());
                    if age < MIC_DELAY {
                        match rx.recv_timeout(MIC_DELAY - age) {
                            Ok(segment) => queue.push_back(segment),
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            // Recording stopped: just wait out the delay.
                            Err(mpsc::RecvTimeoutError::Disconnected) if !queue.is_empty() => {
                                std::thread::sleep(MIC_DELAY - age);
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                        continue;
                    }
                    0
                }
            };
            let Some((speaker, samples, _)) = queue.remove(next) else { continue };
            let _ = app.emit("voice", "transcribing");
            let mut result = transcribe(&url, &samples, &language());
            if result.is_err() {
                match server.restart_if_dead(&samples) {
                    Ok(true) => result = transcribe(&url, &samples, &language()),
                    Ok(false) => {}
                    Err(e) => result = Err(format!("restarting whisper-server: {e}")),
                }
            }
            let _ = app.emit("voice", "idle");
            heard.retain(|(at, _)| at.elapsed() < ECHO_WINDOW);
            match result {
                Ok(text) if speaker == "me" && is_echo(&text, heard.iter().map(|(_, t)| t.as_str())) => {
                    eprintln!("dropped mic line as speaker echo: {text}");
                }
                Ok(text) if !text.is_empty() => {
                    eprintln!("[{speaker}] {text}");
                    if speaker == "them" {
                        heard.push_back((Instant::now(), text.clone()));
                    }
                    let _ = app.emit("transcript", json!({ "speaker": speaker, "text": text }));
                    if let Ok(mut recent) = RECENT.lock() {
                        let label = if speaker == "me" { "Me" } else { "Them" };
                        recent.push_back(format!("{label}: {text}"));
                        if recent.len() > RECENT_LINES {
                            recent.pop_front();
                        }
                    }
                    on_line(speaker, &text);
                }
                Ok(_) => eprintln!("[{speaker}] (no speech recognised)"),
                Err(e) => {
                    eprintln!("transcribe: {e}");
                    let text = format!("Transcription failed: {e} (see ~/.cache/invisible-ai/whisper-server.log)");
                    let _ = app.emit("answer", json!({ "kind": "error", "text": text }));
                }
            }
        }
        // Recorders stopped and everything queued is transcribed.
        if GENERATION.load(Ordering::SeqCst) == generation {
            stop_server();
        }
    });
    Ok(())
}

fn cache_dir() -> Result<std::path::PathBuf, String> {
    let dir = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
        .ok_or("neither XDG_CACHE_HOME nor HOME is set")?
        .join("invisible-ai");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// whisper-server's output goes to `$XDG_CACHE_HOME/invisible-ai/whisper-server.log`,
/// started fresh for each listening session and appended to on restarts.
fn server_log(fresh: bool) -> Result<std::fs::File, String> {
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(!fresh)
        .truncate(fresh)
        .open(cache_dir()?.join("whisper-server.log"))
        .map_err(|e| e.to_string())
}

/// Read 16 kHz mono s16 audio and send each stretch of speech, cut at pauses.
fn split_speech(input: impl Read, speaker: &'static str, tx: mpsc::Sender<Segment>) {
    let mut reader = BufReader::new(input);
    let mut bytes = [0u8; FRAME * 2];
    // Noise floor: drops quickly, rises slowly, so steady background (music,
    // fans) eventually counts as silence.
    let mut noise = 300.0_f64;
    let mut pre_roll: VecDeque<Vec<i16>> = VecDeque::new();
    let mut segment: Vec<i16> = Vec::new();
    let (mut speech_frames, mut silent_run) = (0, 0);

    while reader.read_exact(&mut bytes).is_ok() {
        let frame: Vec<i16> = bytes
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        let rms = (frame.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / FRAME as f64).sqrt();
        let speech = rms > (noise * 3.0).max(400.0);
        noise = if rms < noise { noise * 0.9 + rms * 0.1 } else { noise * 0.999 + rms * 0.001 };

        if segment.is_empty() {
            if speech {
                segment.extend(pre_roll.drain(..).flatten());
                segment.extend_from_slice(&frame);
                (speech_frames, silent_run) = (1, 0);
            } else {
                pre_roll.push_back(frame);
                if pre_roll.len() > PRE_ROLL_FRAMES {
                    pre_roll.pop_front();
                }
            }
            continue;
        }

        segment.extend_from_slice(&frame);
        if speech {
            speech_frames += 1;
            silent_run = 0;
        } else {
            silent_run += 1;
        }
        if silent_run >= END_SILENCE_FRAMES || segment.len() >= MAX_SEGMENT {
            let done = std::mem::take(&mut segment);
            if speech_frames >= MIN_SPEECH_FRAMES && tx.send((speaker, done, Instant::now())).is_err() {
                return;
            }
            speech_frames = 0;
        }
    }
    // Recording stopped (e.g. mic toggled off right after a question): keep the last clip.
    if speech_frames >= MIN_SPEECH_FRAMES {
        let _ = tx.send((speaker, segment, Instant::now()));
    }
}

/// Whether a mic line is mostly words the system audio just said (speaker echo).
fn is_echo<'a>(mic: &str, heard: impl Iterator<Item = &'a str>) -> bool {
    let words = |t: &str| -> Vec<String> {
        t.split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.chars().count() >= 3)
            .map(str::to_lowercase)
            .collect()
    };
    let heard: std::collections::HashSet<String> = heard.flat_map(|t| words(t)).collect();
    let mic = words(mic);
    !mic.is_empty() && mic.iter().filter(|w| heard.contains(*w)).count() * 2 >= mic.len()
}

/// Transcribe a clip in `language` (forced: on short clips Whisper's own detection is
/// unreliable, especially for Russian with small models).
fn transcribe(url: &str, samples: &[i16], language: &str) -> Result<String, String> {
    request(url, &wav(samples), language).map(|(text, _)| text)
}

/// One whisper-server request: (text without non-speech markers, detected language name).
fn request(url: &str, audio: &[u8], language: &str) -> Result<(String, String), String> {
    const BOUNDARY: &str = "invisible-ai-audio";
    let mut body = Vec::new();
    let _ = write!(
        body,
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"response_format\"\r\n\r\nverbose_json\r\n\
         --{BOUNDARY}\r\nContent-Disposition: form-data; name=\"language\"\r\n\r\n{language}\r\n\
         --{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"speech.wav\"\r\n\
         Content-Type: audio/wav\r\n\r\n"
    );
    body.extend_from_slice(audio);
    let _ = write!(body, "\r\n--{BOUNDARY}--\r\n");

    // No connection reuse: whisper-server closes idle connections after ~5 s, and a
    // reused stale connection fails with "Unexpected EOF" after any pause in speech.
    let agent = ureq::AgentBuilder::new().max_idle_connections(0).build();
    let response: Value = agent
        .post(url)
        .set("Content-Type", &format!("multipart/form-data; boundary={BOUNDARY}"))
        .timeout(Duration::from_secs(60))
        .send_bytes(&body)
        .map_err(|e| e.to_string())?
        .into_json()
        .map_err(|e| e.to_string())?;
    let detected = response["language"].as_str().unwrap_or("").to_owned();
    let text = response["text"].as_str().unwrap_or("").trim();
    // Whisper marks non-speech as e.g. "[BLANK_AUDIO]", "(music)", "*applause*".
    let marker = |open, close| text.starts_with(open) && text.ends_with(close);
    if marker('[', ']') || marker('(', ')') || marker('*', '*') {
        return Ok((String::new(), detected));
    }
    Ok((text.to_owned(), detected))
}

/// 16 kHz mono 16-bit PCM WAV.
fn wav(samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&(RATE as u32).to_le_bytes());
    out.extend_from_slice(&(RATE as u32 * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{is_echo, is_question};

    #[test]
    fn detects_speaker_echo() {
        let heard = ["And so my fellow Americans, ask not what your country can do for you."];
        assert!(is_echo("so my fellow American has not what your country", heard.iter().copied()));
        assert!(!is_echo("Explain the event loop in Node.js", heard.iter().copied()));
        assert!(!is_echo("Explain the event loop in Node.js", std::iter::empty()));
    }

    #[test]
    fn detects_questions_and_requests() {
        assert!(is_question("Can you explain JavaScript Closure?"));
        assert!(is_question("Explain to me event clues in Node.js"));
        assert!(is_question("  tell me about Rust lifetimes."));
        assert!(is_question("What's the difference?"));
        assert!(!is_question("What your country can do for you, ask what you can do."));
        assert!(!is_question("I think we should ship on Friday."));
        assert!(!is_question("Thanks, that makes sense."));
        assert!(!is_question(""));
        assert!(is_question("Объясни мне асинхронные функции"));
        assert!(is_question("Как работает event loop?"));
        assert!(!is_question("Я думаю, это хорошая идея."));
        assert!(is_question("Объяснимли как работают замыкания в JavaScript."));
        assert!(is_question("Расскажите про event loop."));
        assert!(is_question("Подскажи, как отменить коммит."));
    }
}
