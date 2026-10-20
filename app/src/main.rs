mod ai;
mod capture;
mod ipc;
mod overlay;
mod settings;
mod tray;
mod tts;
mod voice;

use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, RunEvent};

const HELP: &str = "\
invisible-ai: AI overlay that answers questions about your screen.

Usage:
  invisible-ai             run the app (one instance at a time)
  invisible-ai <command>   control the running app; starts it first if needed

Commands:
  toggle             show / hide the panel
  interact           toggle interactive mode (click and type in the panel)
  prompt             show the panel and focus the question input
  ask [question]     ask about the screen now (no question: the default one)
  scroll <dir>       scroll the answers: up, down (a page), top, bottom
  stop               stop the answer that is streaming (and speaking)
  speak [on|off]     toggle spoken answers (local Piper voices)
  hush               stop speaking the current answer
  copy               copy the last answer to the clipboard
  clear              clear everything: history, follow-up context, transcript
  listen             start / stop live transcription (voice)
  language [code]    voice language: next of [voice] language (en, ru, ...) or <code>
  fullscreen         toggle fullscreen / compact panel
  move <dx> <dy>     move the compact panel (logical px)
  resize <dw> <dh>   resize the compact panel (logical px)
  capture            save a screenshot (without the panel), print its path
  quit               quit the app

Config: ~/.config/invisible-ai/config.toml";
pub const DEFAULT_QUESTION: &str = "What's on my screen? Help me with what I'm working on.";

/// Start the app in the background and wait until it listens on the control socket.
fn start() -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    let join = |built: &str, var: &str| match std::env::var(var) {
        Ok(current) if !current.is_empty() => format!("{built}:{current}"),
        _ => built.to_owned(),
    };
    let log_dir = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("invisible-ai");
    std::fs::create_dir_all(&log_dir)?;
    let log = std::fs::File::create(log_dir.join("app.log"))?;
    std::process::Command::new(std::env::current_exe()?)
        .env("LD_LIBRARY_PATH", join(env!("BUILD_LD_LIBRARY_PATH"), "LD_LIBRARY_PATH"))
        .env("PATH", join(env!("BUILD_PATH"), "PATH"))
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0) // outlives the hotkey's process
        .spawn()?;
    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if std::os::unix::net::UnixStream::connect(ipc::socket_path()).is_ok() {
            return Ok(());
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "no reply within 10 s; see ~/.cache/invisible-ai/app.log"))
}

fn main() -> ExitCode {
    // `invisible-ai <command>` controls the running instance.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.first().map(String::as_str), Some("-h" | "--help" | "help")) {
        println!("{HELP}");
        return ExitCode::SUCCESS;
    }
    if !args.is_empty() {
        let cmd = args.join(" ");
        let sent = ipc::send(&cmd).or_else(|_| {
            // Not running: start it. It starts visible, which is what `toggle` wants.
            start()?;
            if cmd == "toggle" { Ok("ok".to_owned()) } else { ipc::send(&cmd) }
        });
        return match sent {
            Ok(reply) => match reply.strip_prefix("ok") {
                Some(out) => {
                    let out = out.trim_start();
                    if !out.is_empty() {
                        println!("{out}");
                    }
                    ExitCode::SUCCESS
                }
                None => {
                    eprintln!("{reply}");
                    ExitCode::FAILURE
                }
            },
            Err(e) => {
                eprintln!("cannot start invisible-ai ({e})");
                ExitCode::FAILURE
            }
        };
    }

    let listener = match ipc::bind() {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    // WebKitGTK switches to compositing mode on demand (e.g. once content needs to
    // scroll); on a transparent window that switch leaves the overlay partly blank.
    // Starting in compositing mode avoids it. Set before any threads exist.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_FORCE_COMPOSITING_MODE").is_none() {
        std::env::set_var("WEBKIT_FORCE_COMPOSITING_MODE", "1");
    }

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![run, is_fullscreen, voice_language, speech_settings, list_voices, set_voice, set_speech_speed, preview_voice, get_settings, save_settings, copy_text, list_models, refresh_models])
        .setup(move |app| {
            let overlay = app
                .get_webview_window("overlay")
                .expect("overlay window missing from tauri.conf.json");
            overlay::setup(&overlay)?;
            tray::setup(app)?;

            let handle = app.handle().clone();
            ipc::serve(listener, move |cmd| dispatch(&handle, cmd));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building invisible-ai")
        .run(|_, event| {
            if let RunEvent::Exit = event {
                voice::shutdown();
                let _ = std::fs::remove_file(ipc::socket_path());
            }
        });
    ExitCode::SUCCESS
}

/// Earlier (question, answer) pairs sent with each ask, so follow-ups have context.
static CONTEXT: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
/// How many earlier exchanges to send.
const CONTEXT_LEN: usize = 3;
/// Id of the newest ask; older ones (and stopped ones) stop streaming.
static LATEST: AtomicU64 = AtomicU64::new(0);

#[tauri::command]
fn get_settings() -> Result<settings::Settings, String> {
    settings::get()
}

#[tauri::command]
fn save_settings(changes: settings::Changes) -> Result<(), String> {
    settings::save(changes)
}

#[tauri::command]
fn list_models(provider: String) -> Result<Vec<String>, String> {
    settings::models_for(&provider, false)
}

#[tauri::command]
fn refresh_models(provider: String) -> Result<Vec<String>, String> {
    settings::models_for(&provider, true)
}

/// Spoken-answer settings: on/off, voice per language, speed.
#[tauri::command]
fn speech_settings() -> Result<serde_json::Value, String> {
    let speech = ai::load_config()?.speech;
    Ok(json!({
        "enabled": tts::enabled(),
        "en": tts::key_of(&speech.en),
        "ru": tts::key_of(&speech.ru),
        "length_scale": speech.length_scale,
    }))
}

#[tauri::command]
async fn list_voices(lang: String) -> Result<Vec<tts::Voice>, String> {
    tts::voices(&lang)
}

/// Download the voice if needed and use it for `lang`.
#[tauri::command]
async fn set_voice(app: AppHandle, lang: String, key: String) -> Result<(), String> {
    if lang != "en" && lang != "ru" {
        return Err(format!("no voice setting for {lang}"));
    }
    let path = tts::install(&key)?;
    settings::set_speech(&lang, path)?;
    tts::stop(&app); // the next sentence starts the new voice
    Ok(())
}

#[tauri::command]
fn set_speech_speed(app: AppHandle, length_scale: f64) -> Result<(), String> {
    settings::set_speech("length_scale", (length_scale.clamp(0.5, 2.0) * 100.0).round() / 100.0)?;
    tts::stop(&app);
    Ok(())
}

/// Say a sample sentence with the configured voice for `lang`.
#[tauri::command]
async fn preview_voice(app: AppHandle, lang: String) -> Result<(), String> {
    let config = ai::load_config()?;
    tts::stop(&app);
    let (lang, sample) = if lang == "ru" {
        ("ru", "Привет! Так звучит мой голос, когда я отвечаю на ваши вопросы.")
    } else {
        ("en", "Hi! This is how I sound when I answer your questions.")
    };
    tts::say(&app, &config.speech, lang, sample)
}

/// Current voice language and the ones to switch between.
#[tauri::command]
fn voice_language() -> (String, Vec<String>) {
    if voice::languages().1.is_empty() {
        if let Ok(config) = ai::load_config() {
            voice::set_options(&config.voice.language);
        }
    }
    voice::languages()
}

/// Lets the page pick up the initial mode (it may load after the first `fullscreen` event).
#[tauri::command]
fn is_fullscreen() -> bool {
    overlay::is_fullscreen()
}

#[tauri::command]
fn copy_text(text: String) -> Result<(), String> {
    settings::copy(&text)
}

/// Lets the overlay UI run the same commands as the socket and tray.
/// Async so slow commands (starting whisper-server) don't block the UI thread.
#[tauri::command]
async fn run(app: AppHandle, cmd: String) -> Result<String, String> {
    dispatch(&app, &cmd)
}

pub fn dispatch(app: &AppHandle, cmd: &str) -> Result<String, String> {
    let overlay = app
        .get_webview_window("overlay")
        .ok_or("overlay window missing")?;
    let (verb, rest) = cmd.split_once(' ').unwrap_or((cmd, ""));
    match verb {
        "toggle" => overlay::toggle_visible(&overlay).map_err(|e| e.to_string())?,
        "interact" => overlay::toggle_interactive(&overlay).map_err(|e| e.to_string())?,
        "prompt" => {
            if !overlay::is_visible(&overlay).map_err(|e| e.to_string())? {
                overlay::set_visible(&overlay, true).map_err(|e| e.to_string())?;
            }
            overlay::focus_prompt(&overlay).map_err(|e| e.to_string())?;
        }
        "fullscreen" => overlay::toggle_fullscreen(&overlay).map_err(|e| e.to_string())?,
        "move" => {
            let mut delta = rest.split_whitespace().map(str::parse::<i32>);
            let (Some(Ok(dx)), Some(Ok(dy))) = (delta.next(), delta.next()) else {
                return Err("usage: move <dx> <dy>".into());
            };
            overlay::move_by(&overlay, dx, dy).map_err(|e| e.to_string())?
        }
        "resize" => {
            let mut delta = rest.split_whitespace().map(str::parse::<i32>);
            let (Some(Ok(dw)), Some(Ok(dh))) = (delta.next(), delta.next()) else {
                return Err("usage: resize <dw> <dh>".into());
            };
            overlay::resize_by(&overlay, dw, dh).map_err(|e| e.to_string())?
        }
        "capture" => {
            let path = capture::save(&capture::screen_without_overlay(&overlay)?)?;
            return Ok(path.display().to_string());
        }
        "ask" => {
            // Fail fast on config errors; the request itself streams in the background.
            let config = ai::load_config()?;
            config.active()?;
            let question = match rest.trim() {
                "" => config.ui.default_question.as_deref().unwrap_or(DEFAULT_QUESTION),
                q => q,
            }
            .to_owned();
            let app = app.clone();
            std::thread::spawn(move || ask(&app, &config, &question));
        }
        "stop" => {
            LATEST.fetch_add(1, Ordering::SeqCst);
            tts::stop(app);
            app.emit("answer", json!({ "kind": "stopped", "text": "" }))
                .map_err(|e| e.to_string())?;
        }
        "copy" => {
            let context = CONTEXT.lock().unwrap();
            let (_, answer) = context.last().ok_or("no answer to copy yet")?;
            settings::copy(answer)?;
        }
        "clear" => {
            CONTEXT.lock().unwrap().clear();
            voice::clear_recent();
            app.emit("cleared", ()).map_err(|e| e.to_string())?;
        }
        "scroll" => {
            let direction = rest.trim();
            if !matches!(direction, "up" | "down" | "top" | "bottom") {
                return Err("usage: scroll up|down|top|bottom".into());
            }
            app.emit("scroll", direction).map_err(|e| e.to_string())?;
        }
        "speak" => {
            let on = match rest.trim() {
                "" => None,
                "on" => Some(true),
                "off" => Some(false),
                _ => return Err("usage: speak [on|off]".into()),
            };
            return Ok(if tts::set_enabled(app, on) { "on" } else { "off" }.to_owned());
        }
        "hush" => tts::stop(app),
        "language" => {
            // Options come from the config, so switching works before listening starts.
            if let Ok(config) = ai::load_config() {
                voice::set_options(&config.voice.language);
            }
            let code = rest.split_whitespace().next();
            let current = voice::switch_language(code);
            app.emit("language", &current).map_err(|e| e.to_string())?;
            return Ok(current);
        }
        "listen" => {
            let config = ai::load_config()?;
            let auto_answer = config.voice.auto_answer;
            let handle = app.clone();
            let listening = voice::toggle(app, &config.voice, move |_speaker, text| {
                if auto_answer && voice::is_question(text) {
                    if let Err(e) = dispatch(&handle, &format!("ask {text}")) {
                        eprintln!("auto-answer: {e}");
                    }
                }
            })?;
            app.emit("listening", listening).map_err(|e| e.to_string())?;
        }
        "models" => {
            let provider = rest.trim();
            let models = settings::models_for(provider, false)?;
            return Ok(models.join("\n"));
        }
        "refresh-models" => {
            let provider = rest.trim();
            let models = settings::models_for(provider, true)?;
            return Ok(models.join("\n"));
        }
        "quit" => app.exit(0),
        _ => return Err(format!("unknown command '{cmd}' (see invisible-ai --help)")),
    }
    Ok(String::new())
}

/// Capture the screen and stream the provider's answer into the overlay as `answer` events.
/// The recent transcript, if any, is sent as context. A newer ask takes over the panel;
/// older ones still running stop emitting.
fn ask(app: &AppHandle, config: &ai::Config, question: &str) {
    let id = LATEST.fetch_add(1, Ordering::SeqCst) + 1;
    let current = || LATEST.load(Ordering::SeqCst) == id;
    let emit = |kind: &str, text: &str| {
        if current() {
            let _ = app.emit("answer", json!({ "kind": kind, "text": text }));
        }
    };
    let earlier = CONTEXT.lock().unwrap().clone();
    let mut answer = String::new();
    // Spoken answers: sentences go to Piper as they complete. A new answer interrupts.
    let speaking = tts::enabled();
    if speaking {
        tts::stop(app);
    }
    let mut sentences = tts::Sentences::default();
    let say = |sentences: Vec<String>, lang: &'static str| {
        for sentence in sentences {
            if !current() {
                return;
            }
            if let Err(e) = tts::say(app, &config.speech, lang, &sentence) {
                eprintln!("speech: {e}");
                emit("speech-error", &e);
                return;
            }
        }
    };
    let transcript = voice::recent();
    let prompt = if transcript.is_empty() {
        question.to_owned()
    } else {
        format!("Conversation transcript so far:\n{transcript}\n\nQuestion: {question}")
    };
    let result = (|| {
        let overlay = app
            .get_webview_window("overlay")
            .ok_or("overlay window missing")?;
        let png = Some(capture::png(&capture::screen_without_overlay(&overlay)?)?);
        // Re-showing a visible window re-maps it on i3, which looks like a restart.
        if !overlay::is_visible(&overlay).map_err(|e| e.to_string())? {
            overlay::set_visible(&overlay, true).map_err(|e| e.to_string())?;
        }
        emit("start", question);
        config.active()?.ask(&earlier, &prompt, png.as_deref(), |reasoning, delta| {
            if reasoning {
                emit("reasoning", delta);
            } else {
                answer.push_str(delta);
                emit("delta", delta);
                if speaking {
                    let done = sentences.push(delta);
                    say(done, sentences.lang());
                }
            }
            current()
        })
    })();
    match result {
        Ok(()) if current() => {
            if speaking {
                let rest = sentences.finish();
                say(rest, sentences.lang());
            }
            let mut context = CONTEXT.lock().unwrap();
            context.push((question.to_owned(), answer));
            let excess = context.len().saturating_sub(CONTEXT_LEN);
            context.drain(..excess);
            drop(context);
            emit("done", "");
        }
        Ok(()) => {} // stopped or superseded
        Err(e) => {
            eprintln!("ask failed: {e}");
            emit("error", &e);
        }
    }
}
