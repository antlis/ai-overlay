mod ai;
mod capture;
mod ipc;
mod overlay;
mod settings;
mod tray;
mod voice;

use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, RunEvent};

const COMMANDS: &str =
    "toggle, interact, fullscreen, move <dx> <dy>, resize <dw> <dh>, capture, ask [question], stop, copy, clear, listen, quit";
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
        .invoke_handler(tauri::generate_handler![run, get_settings, save_settings, copy_text])
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
            let image = capture::screen_without_overlay(&overlay)?
                .ok_or("the panel is fullscreen; exit fullscreen to capture")?;
            let path = capture::save(&image)?;
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
            app.emit("cleared", ()).map_err(|e| e.to_string())?;
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
        "quit" => app.exit(0),
        _ => return Err(format!("unknown command '{cmd}' (expected {COMMANDS})")),
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
        let png = match capture::screen_without_overlay(&overlay)? {
            Some(image) => Some(capture::png(&image)?),
            None => None,
        };
        // Re-showing a visible window re-maps it on i3, which looks like a restart.
        if !overlay::is_visible(&overlay).map_err(|e| e.to_string())? {
            overlay::set_visible(&overlay, true).map_err(|e| e.to_string())?;
        }
        emit("start", question);
        config.active()?.ask(&earlier, &prompt, png.as_deref(), |delta| {
            answer.push_str(delta);
            emit("delta", delta);
            current()
        })
    })();
    match result {
        Ok(()) if current() => {
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
