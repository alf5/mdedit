//! Handing things to the operating system: links and folders.

use std::path::{Path, PathBuf};

/// May this address be handed to the OS? It comes out of a document, which
/// could be from anyone, so only web and mail links qualify: `file:` and custom
/// schemes can launch things.
pub fn is_openable_link(url: &str) -> bool {
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let Some((scheme, rest)) = url.split_once(':') else {
        return false;
    };
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" => rest.strip_prefix("//").is_some_and(|host| {
            host.split(['/', '?', '#'])
                .next()
                .is_some_and(|h| !h.is_empty())
        }),
        "mailto" => !rest.is_empty(),
        _ => false,
    }
}

/// Directory a file manager should open for this path: the path itself if it
/// is a directory, otherwise its parent. `None` when the path has no parent
/// (a bare filename with no directory component).
pub fn containing_folder(path: &Path) -> Option<PathBuf> {
    let dir = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()?.to_path_buf()
    };
    if dir.as_os_str().is_empty() {
        None
    } else {
        Some(dir)
    }
}

pub fn open_in_file_manager(dir: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let mut cmd = std::process::Command::new("explorer");
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(dir).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_yields_its_parent() {
        assert_eq!(
            containing_folder(Path::new("home/a/notes.md")),
            Some(PathBuf::from("home/a"))
        );
    }

    #[test]
    fn directory_yields_itself() {
        let dir = std::env::temp_dir();
        assert!(dir.is_dir());
        assert_eq!(containing_folder(&dir), Some(dir));
    }

    #[test]
    fn bare_filename_has_no_folder() {
        assert_eq!(containing_folder(Path::new("notes.md")), None);
    }

    #[test]
    fn only_web_and_mail_links_open() {
        for ok in [
            "http://x.y",
            "https://x.y/a?b=1#c",
            "HTTPS://x.y",
            "mailto:a@b.c",
        ] {
            assert!(is_openable_link(ok), "{ok}");
        }
        for bad in [
            "",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "relative/path.md",
            "#anchor",
            "https:",
            "https://",
            "https:///path",
            "mailto:",
            "ftp://x.y",
            "https://x.y/a b",
            "zennotes://open",
        ] {
            assert!(!is_openable_link(bad), "{bad}");
        }
    }
}
