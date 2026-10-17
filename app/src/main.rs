mod ai;
mod capture;
mod ipc;
mod overlay;
mod tray;

use std::process::ExitCode;

use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, RunEvent};

const COMMANDS: &str = "toggle, interact, fullscreen, capture, ask [question], quit";
const DEFAULT_QUESTION: &str = "What's on my screen? Help me with what I'm working on.";

fn main() -> ExitCode {
    // `invisible-ai <command>` controls the running instance.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        return match ipc::send(&args.join(" ")) {
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
                eprintln!("invisible-ai is not running ({e})");
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

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![run])
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
                let _ = std::fs::remove_file(ipc::socket_path());
            }
        });
    ExitCode::SUCCESS
}

/// Lets the overlay UI run the same commands as the socket and tray.
#[tauri::command]
fn run(app: AppHandle, cmd: String) -> Result<String, String> {
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
        "capture" => {
            let path = capture::save(&capture::screen_without_overlay(&overlay)?)?;
            return Ok(path.display().to_string());
        }
        "ask" => {
            let question = match rest.trim() {
                "" => DEFAULT_QUESTION,
                q => q,
            }
            .to_owned();
            // Fail fast on config errors; the request itself streams in the background.
            let config = ai::load_config()?;
            config.active()?;
            let app = app.clone();
            std::thread::spawn(move || ask(&app, &config, &question));
        }
        "quit" => app.exit(0),
        _ => return Err(format!("unknown command '{cmd}' (expected {COMMANDS})")),
    }
    Ok(String::new())
}

/// Capture the screen and stream the provider's answer into the overlay as `answer` events.
fn ask(app: &AppHandle, config: &ai::Config, question: &str) {
    let emit = |kind: &str, text: &str| {
        let _ = app.emit("answer", json!({ "kind": kind, "text": text }));
    };
    let result = (|| {
        let overlay = app
            .get_webview_window("overlay")
            .ok_or("overlay window missing")?;
        let png = capture::png(&capture::screen_without_overlay(&overlay)?)?;
        overlay.show().map_err(|e| e.to_string())?;
        emit("start", question);
        config.active()?.ask(question, &png, |delta| emit("delta", delta))
    })();
    match result {
        Ok(()) => emit("done", ""),
        Err(e) => {
            eprintln!("ask failed: {e}");
            emit("error", &e);
        }
    }
}
