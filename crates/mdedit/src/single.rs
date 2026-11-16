//! One instance per user: a second `mdedit file.md` hands its files to the
//! running window and exits, instead of opening another.
//!
//! The first instance listens on a per-user local socket — a socket file in
//! the user's runtime directory on Unix, a named pipe on Windows. A later
//! launch connects, writes its paths (already absolute, since the two
//! processes may not share a working directory) and leaves. A socket file
//! left behind by a crash is taken over: a dead listener refuses the
//! connection, and that is what decides who is first.

use std::io::{Read, Write};
use std::path::PathBuf;

use interprocess::local_socket::{ListenerOptions, Name, Stream, prelude::*};

/// What this process should do.
pub enum Role {
    /// The first instance. Paths from later launches arrive on the channel;
    /// the sender is for other sources of files to open (macOS Finder).
    Primary(
        async_channel::Sender<Vec<PathBuf>>,
        async_channel::Receiver<Vec<PathBuf>>,
    ),
    /// Another instance took the paths; exit.
    Forwarded,
}

fn socket_name() -> std::io::Result<Name<'static>> {
    #[cfg(unix)]
    {
        use interprocess::local_socket::GenericFilePath;
        // Per-user and private where the platform has a runtime directory
        // (Linux: $XDG_RUNTIME_DIR, mode 0700); the uid in the name keeps it
        // per-user in the shared temp directory otherwise.
        let uid = unsafe { libc::getuid() };
        let dir = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
        dir.join(format!("mdedit-{uid}.sock"))
            .to_fs_name::<GenericFilePath>()
    }
    #[cfg(windows)]
    {
        use interprocess::local_socket::GenericNamespaced;
        let user = std::env::var("USERNAME").unwrap_or_default();
        format!("mdedit-{user}").to_ns_name::<GenericNamespaced>()
    }
}

/// Encode paths for the wire: NUL-separated, since NUL is the one byte no
/// path can contain.
fn encode(paths: &[PathBuf]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in paths {
        out.extend_from_slice(p.as_os_str().as_encoded_bytes());
        out.push(0);
    }
    out
}

fn decode(bytes: &[u8]) -> Vec<PathBuf> {
    bytes
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        // SAFETY: the bytes were produced by `as_encoded_bytes` in `encode`,
        // by the same binary on the same platform.
        .map(|s| PathBuf::from(unsafe { std::ffi::OsStr::from_encoded_bytes_unchecked(s) }))
        .collect()
}

/// Become the primary instance, or hand `paths` to the one already running.
/// If the socket can't be set up at all, this process just runs on its own.
pub fn claim(paths: &[PathBuf]) -> Role {
    let (tx, rx) = async_channel::unbounded();
    let Ok(name) = socket_name() else {
        return Role::Primary(tx, rx);
    };
    if let Ok(mut stream) = Stream::connect(name.clone())
        && stream.write_all(&encode(paths)).is_ok()
    {
        return Role::Forwarded;
    }
    let listener = match ListenerOptions::new()
        .name(name)
        .try_overwrite(true)
        .create_sync()
    {
        Ok(l) => l,
        Err(_) => return Role::Primary(tx, rx),
    };
    let to_app = tx.clone();
    std::thread::Builder::new()
        .name("single-instance".into())
        .spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { continue };
                let mut bytes = Vec::new();
                if conn.read_to_end(&mut bytes).is_ok() {
                    // Even an empty list means something: a bare `mdedit`
                    // should bring the existing window forward.
                    if to_app.send_blocking(decode(&bytes)).is_err() {
                        break;
                    }
                }
            }
        })
        .ok();
    Role::Primary(tx, rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_survive_the_wire() {
        let paths = vec![
            PathBuf::from("/home/a/notes.md"),
            PathBuf::from("/tmp/with space/ünïcode.md"),
        ];
        assert_eq!(decode(&encode(&paths)), paths);
        assert!(decode(&encode(&[])).is_empty());
    }
}
