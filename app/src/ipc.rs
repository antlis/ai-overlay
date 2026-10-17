//! Control socket: the running app listens, `invisible-ai <command>` sends.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

pub fn socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("invisible-ai.sock")
}

/// Send one command to the running instance and return its reply.
pub fn send(cmd: &str) -> io::Result<String> {
    let mut stream = UnixStream::connect(socket_path())?;
    writeln!(stream, "{cmd}")?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    Ok(reply.trim_end().to_owned())
}

/// Bind the control socket, refusing to start if another instance is listening.
pub fn bind() -> io::Result<UnixListener> {
    let path = socket_path();
    if UnixStream::connect(&path).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "invisible-ai is already running",
        ));
    }
    // Left behind by a run that didn't exit cleanly.
    let _ = std::fs::remove_file(&path);
    UnixListener::bind(&path)
}

/// Handle one command per connection on a background thread.
pub fn serve<F>(listener: UnixListener, handle: F)
where
    F: Fn(&str) -> Result<(), String> + Send + 'static,
{
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(&stream).read_line(&mut line).is_err() {
                continue;
            }
            let reply = match handle(line.trim()) {
                Ok(()) => "ok".to_owned(),
                Err(e) => format!("error: {e}"),
            };
            let _ = writeln!(&stream, "{reply}");
        }
    });
}
