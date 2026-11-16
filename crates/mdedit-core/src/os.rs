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

/// Where an image in a document comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageRef {
    /// A file on this machine.
    Local(PathBuf),
    /// An `http(s)` URL: fetching it tells its server the document was
    /// opened, so it waits until asked.
    Remote(String),
    /// Not loaded at all: network shares (on Windows, touching one sends
    /// the user's credentials to its host) and other schemes.
    Refused,
}

/// Decide where image `url` in a document whose folder is `base` comes from.
pub fn resolve_image(url: &str, base: Option<&Path>) -> ImageRef {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return ImageRef::Remote(url.to_string());
    }
    if url.contains("://") || lower.starts_with("data:") || lower.starts_with("file:") {
        return ImageRef::Refused;
    }
    // `\\host\share`, `//host/share`, `\\?\UNC\…`: network paths.
    if url.starts_with("\\\\")
        || url.starts_with("//")
        || url.starts_with("\\/")
        || url.starts_with("/\\")
    {
        return ImageRef::Refused;
    }
    let decoded = percent_decode(url);
    let path = PathBuf::from(&decoded);
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        if let Some(Component::Prefix(p)) = path.components().next()
            && matches!(
                p.kind(),
                Prefix::UNC(..)
                    | Prefix::VerbatimUNC(..)
                    | Prefix::DeviceNS(..)
                    | Prefix::Verbatim(..)
            )
        {
            return ImageRef::Refused;
        }
    }
    if path.is_absolute() {
        return ImageRef::Local(path);
    }
    match base {
        Some(dir) => ImageRef::Local(dir.join(path)),
        None => ImageRef::Refused,
    }
}

/// `%20` and friends.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn images_resolve_locally_and_refuse_network_paths() {
        let base = Path::new("/docs/notes");
        assert_eq!(
            resolve_image("img/a b.png", Some(base)),
            ImageRef::Local(PathBuf::from("/docs/notes/img/a b.png"))
        );
        assert_eq!(
            resolve_image("img/a%20b.png", Some(base)),
            ImageRef::Local(PathBuf::from("/docs/notes/img/a b.png"))
        );
        assert_eq!(
            resolve_image("../shared/x.png", Some(base)),
            ImageRef::Local(PathBuf::from("/docs/notes/../shared/x.png"))
        );
        assert_eq!(
            resolve_image("https://x.y/i.png", Some(base)),
            ImageRef::Remote("https://x.y/i.png".into())
        );
        for bad in [
            r"\\host\share\x.png",
            "//host/share/x.png",
            "file:///etc/x.png",
            "data:image/png;base64,AAAA",
            "smb://host/x.png",
        ] {
            assert_eq!(resolve_image(bad, Some(base)), ImageRef::Refused, "{bad}");
        }
        assert_eq!(resolve_image("x.png", None), ImageRef::Refused);
    }

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
