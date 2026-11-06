//! Single-instance hand-off.
//!
//! The first process listens on a per-user Unix domain socket. Later launches
//! forward their file arguments and exit, so the running window can open them
//! as tabs instead of creating another application instance.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

/// Returns true when the paths were handed to an already running instance.
#[cfg(unix)]
pub fn forward_to_running(paths: &[PathBuf]) -> bool {
    use std::io::Write;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::net::UnixStream;

    let Ok(mut stream) = UnixStream::connect(socket_path()) else {
        return false;
    };
    let mut payload = Vec::new();
    for path in paths {
        payload.extend_from_slice(path.as_os_str().as_bytes());
        payload.push(0);
    }
    stream.write_all(&payload).is_ok() && stream.flush().is_ok()
}

/// Starts the listener for a later launch. Returns the channel of path batches.
#[cfg(unix)]
pub fn listen() -> Option<Receiver<Vec<PathBuf>>> {
    use std::ffi::OsString;
    use std::io::Read;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;

    let path = socket_path();
    // A previous process may have exited without removing its socket. We only
    // reach this point after a failed connect, so it is safe to replace.
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).ok()?;
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("vofdreader-single-instance".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    continue;
                };
                let mut payload = Vec::new();
                if stream.read_to_end(&mut payload).is_err() {
                    continue;
                }
                let paths = payload
                    .split(|byte| *byte == 0)
                    .filter(|path| !path.is_empty())
                    .map(|path| PathBuf::from(OsString::from_vec(path.to_vec())))
                    .collect();
                if sender.send(paths).is_err() {
                    break;
                }
            }
        })
        .ok()?;
    Some(receiver)
}

#[cfg(unix)]
fn socket_path() -> PathBuf {
    let directory = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "default".to_string());
    let user = user
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>();
    directory.join(format!("vofdreader-{user}.sock"))
}

#[cfg(not(unix))]
pub fn forward_to_running(_paths: &[PathBuf]) -> bool {
    false
}

#[cfg(not(unix))]
pub fn listen() -> Option<Receiver<Vec<PathBuf>>> {
    None
}
