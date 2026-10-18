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
/// Reply lines may be multi-line (lists); the server joins them with `\0`.
pub fn send(cmd: &str) -> io::Result<String> {
    let mut stream = UnixStream::connect(socket_path())?;
    writeln!(stream, "{cmd}")?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    Ok(reply.trim_end_matches('\0').trim_end_matches('\n').to_owned())
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
/// Replies are `ok`, `ok <output>` or `error: <message>`.
pub fn serve<F>(listener: UnixListener, handle: F)
where
    F: Fn(&str) -> Result<String, String> + Send + 'static,
{
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(&stream).read_line(&mut line).is_err() {
                continue;
            }
            let reply = match handle(line.trim()) {
                Ok(out) if out.is_empty() => "ok".to_owned(),
                Ok(out) => {
                    // Multi-line outputs (lists) are joined on NUL so the client can
                    // round-trip them through BufReader::read_line.
                    let out = out.replace('\0', " ").replace('\n', "\0");
                    format!("ok {out}")
                }
                Err(e) => format!("error: {e}"),
            };
            let _ = writeln!(&stream, "{reply}");
        }
    });
}
