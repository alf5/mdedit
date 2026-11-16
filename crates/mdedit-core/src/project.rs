//! The project tree, and opening whatever is in it.
//!
//! mdedit is a markdown editor, so everything else it will show is deliberately
//! limited: an image is a picture, a binary is bytes, and anything else is
//! text. What a file *is* comes from its extension for the kinds that have to
//! be recognised on sight (markdown, images) and from its contents otherwise —
//! extensions run out fast in a source tree, where `Makefile`, `.gitignore`
//! and `LICENSE` are all perfectly ordinary text.

use std::path::{Path, PathBuf};

/// Text and binaries are held in memory, edited, and undone; a megabyte of
/// either is already a strange thing to be editing here.
pub const TEXT_LIMIT: u64 = 1024 * 1024;
/// An image is only ever displayed, so the ceiling is about not wedging the
/// renderer rather than about editing.
pub const IMAGE_LIMIT: u64 = 16 * 1024 * 1024;

/// One row of the tree. Directories are not recursed into here — the tree
/// asks for a directory's children when it is expanded, so opening a folder
/// with a huge `node_modules` in it costs nothing until someone clicks on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub dir: bool,
}

pub fn list_dir(path: &Path) -> Result<Vec<Entry>, String> {
    let mut entries: Vec<Entry> = std::fs::read_dir(path)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| Entry {
            name: e.file_name().to_string_lossy().into_owned(),
            // `file_type` rather than `metadata`: a symlink to a directory
            // that has gone missing should still list, as a file.
            dir: e.file_type().map(|t| t.is_dir()).unwrap_or(false),
            path: e.path(),
        })
        .collect();
    // Directories first, then case-insensitively by name — the order every
    // file manager uses, and the only one that makes a long list scannable.
    entries.sort_by(|a, b| {
        b.dir
            .cmp(&a.dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

/// What a file holds, as far as the editor cares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Markdown(String),
    Text(String),
    Image {
        bytes: Vec<u8>,
        mime: &'static str,
    },
    Binary(Vec<u8>),
    /// Past the limit for its kind; the tab says so rather than pretending
    /// the file is empty.
    Oversized,
}

impl Content {
    pub fn kind(&self) -> &'static str {
        match self {
            Content::Markdown(_) => "markdown",
            Content::Text(_) => "text",
            Content::Image { .. } => "image",
            Content::Binary(_) => "binary",
            Content::Oversized => "oversized",
        }
    }
}

/// A file read for the editor, whatever it turned out to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnyFile {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub content: Content,
}

fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// `.txt` is deliberately not here: it opens as plain text everywhere, in the
/// tree, from Open and from a drop alike.
pub fn is_markdown_ext(path: &Path) -> bool {
    matches!(ext_of(path).as_str(), "md" | "markdown" | "mdown" | "mkd")
}

/// The image types the app displays. SVG is text, but nobody wants to read an
/// icon as source, so it is shown as the picture it is.
pub fn image_mime(path: &Path) -> Option<&'static str> {
    Some(match ext_of(path).as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        _ => return None,
    })
}

/// Git's heuristic, and it is the right one: a NUL byte early on means binary.
/// Invalid UTF-8 counts too, because the editor can only hold a `String` — a
/// Latin-1 file would otherwise arrive mangled, and mangled text that can be
/// saved is worse than honest bytes.
fn looks_binary(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(8000)];
    head.contains(&0) || std::str::from_utf8(bytes).is_err()
}

pub fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_string())
}

/// Read a path for the editor, whatever it turns out to be.
///
/// The caller stamps the result for change detection, so this stays a pure
/// read — see `crate::watch`.
pub fn read_any(path: &Path) -> Result<AnyFile, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.is_dir() {
        return Err(format!("{} is a directory", path.display()));
    }
    let size = meta.len();
    let file = |content| AnyFile {
        path: path.to_path_buf(),
        name: file_name_of(path),
        size,
        content,
    };

    if let Some(mime) = image_mime(path) {
        if size > IMAGE_LIMIT {
            return Ok(file(Content::Oversized));
        }
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        return Ok(file(Content::Image { bytes, mime }));
    }

    if size > TEXT_LIMIT {
        return Ok(file(Content::Oversized));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if looks_binary(&bytes) {
        return Ok(file(Content::Binary(bytes)));
    }
    // `looks_binary` already proved this is UTF-8.
    let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
    Ok(file(if is_markdown_ext(path) {
        Content::Markdown(text)
    } else {
        Content::Text(text)
    }))
}

/// Folders Quick Open doesn't look in: hidden ones, and the usual build
/// and dependency directories, which are large and never hand-edited.
fn skipped(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "node_modules" | "target" | "dist" | "build" | "__pycache__"
        )
}

/// Every file under `root`, for Quick Open: breadth-first, so a huge
/// subtree can't crowd out the top of the project, and at most `limit`.
pub fn walk_files(root: &Path, limit: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut queue = std::collections::VecDeque::from([root.to_path_buf()]);
    while let Some(dir) = queue.pop_front() {
        let Ok(entries) = list_dir(&dir) else {
            continue;
        };
        for e in entries {
            if e.dir {
                if !skipped(&e.name) {
                    queue.push_back(e.path);
                }
            } else {
                out.push(e.path);
                if out.len() >= limit {
                    return out;
                }
            }
        }
    }
    out
}

/// `1.5 MB`, `12.0 KB`, `300 bytes`.
pub fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    match bytes {
        b if b >= MB => format!("{:.1} MB", b as f64 / MB as f64),
        b if b >= KB => format!("{:.1} KB", b as f64 / KB as f64),
        1 => "1 byte".to_string(),
        b => format!("{b} bytes"),
    }
}

/// The path with symlinks and `..` resolved, which is how open tabs and
/// recent files are compared. A path that can't be resolved (not saved yet,
/// deleted) is kept as given.
pub fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_markdown_by_extension_only() {
        assert!(is_markdown_ext(Path::new("/x/notes.md")));
        assert!(is_markdown_ext(Path::new("/x/NOTES.MD")));
        assert!(is_markdown_ext(Path::new("/x/a.markdown")));
        // Plain text now that there is a text editor to open it in.
        assert!(!is_markdown_ext(Path::new("/x/notes.txt")));
        assert!(!is_markdown_ext(Path::new("/x/Makefile")));
    }

    #[test]
    fn images_are_matched_by_extension_including_svg() {
        assert_eq!(image_mime(Path::new("a.PNG")), Some("image/png"));
        assert_eq!(image_mime(Path::new("a.jpeg")), Some("image/jpeg"));
        assert_eq!(image_mime(Path::new("icon.svg")), Some("image/svg+xml"));
        assert_eq!(image_mime(Path::new("a.rs")), None);
    }

    #[test]
    fn walks_files_breadth_first_skipping_the_usual_suspects() {
        let dir = scratch("walk");
        std::fs::create_dir_all(dir.join("docs/deep")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules/x")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("a.md"), "").unwrap();
        std::fs::write(dir.join("docs/b.md"), "").unwrap();
        std::fs::write(dir.join("docs/deep/c.md"), "").unwrap();
        std::fs::write(dir.join("node_modules/x/d.js"), "").unwrap();
        std::fs::write(dir.join(".git/HEAD"), "").unwrap();
        let names: Vec<String> = walk_files(&dir, 100)
            .iter()
            .map(|p| p.strip_prefix(&dir).unwrap().display().to_string())
            .collect();
        assert_eq!(names, ["a.md", "docs/b.md", "docs/deep/c.md"]);
        assert_eq!(walk_files(&dir, 2).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(human_size(1), "1 byte");
        assert_eq!(human_size(300), "300 bytes");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 MB");
    }

    #[test]
    fn a_nul_byte_means_binary() {
        assert!(looks_binary(b"MZ\x00\x00program"));
        assert!(!looks_binary(b"#!/bin/sh\necho hi\n"));
        assert!(!looks_binary("héllo — em dash\n".as_bytes()));
    }

    /// Text that is not UTF-8 is treated as bytes rather than lossily
    /// converted: a save would otherwise write the mangling back to disk.
    #[test]
    fn invalid_utf8_means_binary() {
        assert!(looks_binary(&[0x68, 0x69, 0xff, 0xfe]));
    }

    /// A NUL anywhere in the sniffed window counts, but a file whose only NUL
    /// sits past it is still read as text — the same trade git makes.
    #[test]
    fn only_the_first_8k_is_sniffed_for_nul() {
        let mut bytes = vec![b'a'; 9000];
        bytes[8500] = 0;
        assert!(!looks_binary(&bytes));
        bytes[10] = 0;
        assert!(looks_binary(&bytes));
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mdedit-proj-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_each_kind() {
        let dir = scratch("kinds");
        std::fs::write(dir.join("a.md"), "# Hi\n").unwrap();
        std::fs::write(dir.join("b.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.join("c.bin"), [0u8, 1, 2, 3]).unwrap();
        std::fs::write(dir.join("d.png"), [0x89, b'P', b'N', b'G']).unwrap();

        assert_eq!(
            read_any(&dir.join("a.md")).unwrap().content.kind(),
            "markdown"
        );
        let text = read_any(&dir.join("b.rs")).unwrap();
        assert_eq!(text.content, Content::Text("fn main() {}\n".into()));
        let bin = read_any(&dir.join("c.bin")).unwrap();
        assert_eq!(bin.content, Content::Binary(vec![0u8, 1, 2, 3]));
        let img = read_any(&dir.join("d.png")).unwrap();
        assert_eq!(img.name, "d.png");
        assert!(matches!(
            img.content,
            Content::Image {
                mime: "image/png",
                ..
            }
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_to_load_what_is_too_big() {
        let dir = scratch("big");
        let big = dir.join("big.txt");
        std::fs::write(&big, vec![b'a'; TEXT_LIMIT as usize + 1]).unwrap();
        let f = read_any(&big).unwrap();
        assert_eq!(f.content, Content::Oversized);
        assert_eq!(f.size, TEXT_LIMIT + 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lists_directories_first_then_case_insensitively() {
        let dir = scratch("list");
        std::fs::create_dir(dir.join("zeta")).unwrap();
        std::fs::write(dir.join("Alpha.md"), "").unwrap();
        std::fs::write(dir.join("beta.md"), "").unwrap();
        std::fs::create_dir(dir.join("Apex")).unwrap();
        let names: Vec<String> = list_dir(&dir)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, vec!["Apex", "zeta", "Alpha.md", "beta.md"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
