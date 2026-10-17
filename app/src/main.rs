mod capture;
mod ipc;
mod overlay;

use std::process::ExitCode;

use tauri::{AppHandle, Manager, RunEvent};

const COMMANDS: &str = "toggle, interact, capture, quit";

fn main() -> ExitCode {
    // `invisible-ai <command>` controls the running instance.
    if let Some(cmd) = std::env::args().nth(1) {
        return match ipc::send(&cmd) {
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
        .setup(move |app| {
            let overlay = app
                .get_webview_window("overlay")
                .expect("overlay window missing from tauri.conf.json");
            overlay::setup(&overlay)?;

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

fn dispatch(app: &AppHandle, cmd: &str) -> Result<String, String> {
    let overlay = app
        .get_webview_window("overlay")
        .ok_or("overlay window missing")?;
    match cmd {
        "toggle" => overlay::toggle_visible(&overlay).map_err(|e| e.to_string())?,
        "interact" => overlay::toggle_interactive(&overlay).map_err(|e| e.to_string())?,
        "capture" => {
            let path = capture::save(&capture::screen(&overlay)?)?;
            return Ok(path.display().to_string());
        }
        "quit" => app.exit(0),
        _ => return Err(format!("unknown command '{cmd}' (expected {COMMANDS})")),
    }
    Ok(String::new())
}
