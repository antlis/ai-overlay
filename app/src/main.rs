mod ipc;
mod overlay;

use std::process::ExitCode;

use tauri::{AppHandle, Manager, RunEvent};

const COMMANDS: &str = "toggle, interact, quit";

fn main() -> ExitCode {
    // `invisible-ai <command>` controls the running instance.
    if let Some(cmd) = std::env::args().nth(1) {
        return match ipc::send(&cmd) {
            Ok(reply) if reply == "ok" => ExitCode::SUCCESS,
            Ok(reply) => {
                eprintln!("{reply}");
                ExitCode::FAILURE
            }
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

fn dispatch(app: &AppHandle, cmd: &str) -> Result<(), String> {
    let overlay = app
        .get_webview_window("overlay")
        .ok_or("overlay window missing")?;
    match cmd {
        "toggle" => overlay::toggle_visible(&overlay),
        "interact" => overlay::toggle_interactive(&overlay),
        "quit" => {
            app.exit(0);
            Ok(())
        }
        _ => return Err(format!("unknown command '{cmd}' (expected {COMMANDS})")),
    }
    .map_err(|e| e.to_string())
}
