//! Return the terminal to the caller on request.
//!
//! A second `mdedit` already exits after forwarding its files to the running
//! window, so this only changes a *first* launch, which would otherwise hold
//! the shell until the window closes. Without `--detach`, nothing here runs.

use std::ffi::OsStr;

/// True when this launch asked to give the terminal back.
fn is_detach_flag(arg: impl AsRef<OsStr>) -> bool {
    arg.as_ref() == "--detach"
}

pub fn maybe_detach() {
    if !std::env::args_os().skip(1).any(is_detach_flag) {
        return;
    }
    #[cfg(unix)]
    unix_detach();
    #[cfg(windows)]
    windows_detach();
}

/// Fork, let the parent die, and put the child in its own session so closing
/// the terminal cannot SIGHUP the editor. The working directory is left
/// alone: relative paths on argv are resolved later against it.
///
/// Must run before GTK/WebKit (or any other thread) exists — `fork` without
/// `exec` is only defined while we are still single-threaded.
#[cfg(unix)]
fn unix_detach() {
    // SAFETY: `run` calls this before the GUI or any worker thread exists, so
    // only this thread is duplicated. The parent `_exit`s instead of `exit` so
    // it cannot flush stdio buffers the child also holds.
    let pid = unsafe { libc::fork() };
    match pid {
        -1 => {
            eprintln!(
                "mdedit: --detach: fork failed: {}",
                std::io::Error::last_os_error()
            );
        }
        0 => {
            // SAFETY: still the only thread, and we are the child.
            unsafe { become_session_and_drop_tty() }
        }
        _ => {
            // SAFETY: `_exit` skips atexit handlers; `exit` would flush stdio
            // buffers the child also holds.
            unsafe { libc::_exit(0) }
        }
    }
}

/// New session + stdio on `/dev/null`. `setsid` failure is not fatal: we are
/// already the child, just still in the old session.
///
/// # Safety
///
/// Caller is the child of `fork`, still single-threaded.
#[cfg(unix)]
unsafe fn become_session_and_drop_tty() {
    let _ = libc::setsid();
    let path = std::ffi::CString::new("/dev/null").unwrap();
    let fd = libc::open(path.as_ptr(), libc::O_RDWR);
    if fd < 0 {
        return;
    }
    libc::dup2(fd, libc::STDIN_FILENO);
    libc::dup2(fd, libc::STDOUT_FILENO);
    libc::dup2(fd, libc::STDERR_FILENO);
    if fd > 2 {
        libc::close(fd);
    }
}

/// Spawn a copy of this process without `--detach`, detached from the console.
/// The child is the first instance (this process never starts the GUI).
#[cfg(windows)]
fn windows_detach() {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("mdedit: --detach: {e}");
            return;
        }
    };
    let args: Vec<_> = std::env::args_os()
        .skip(1)
        .filter(|a| !is_detach_flag(a))
        .collect();
    match Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
        .creation_flags(0x00000008 | 0x00000200)
        .spawn()
    {
        Ok(_) => std::process::exit(0),
        Err(e) => eprintln!("mdedit: --detach: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn detach_is_opt_in() {
        let args: [&OsStr; 1] = [OsStr::new("notes.md")];
        assert!(!args.into_iter().any(is_detach_flag));
    }

    #[test]
    fn detach_can_sit_anywhere_on_argv() {
        let args = ["notes.md", "--detach", "draft.md"].map(OsString::from);
        assert!(args.iter().any(is_detach_flag));
        let rest: Vec<_> = args.iter().filter(|a| !is_detach_flag(a)).collect();
        assert_eq!(rest.len(), 2);
        assert_eq!(rest[0], "notes.md");
        assert_eq!(rest[1], "draft.md");
    }

    #[test]
    fn a_short_or_similar_flag_is_not_detach() {
        for a in ["-d", "--daemon", "--detach-me", "-detach"] {
            assert!(!is_detach_flag(OsStr::new(a)), "{a}");
        }
        assert!(is_detach_flag(OsStr::new("--detach")));
    }
}
