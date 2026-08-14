//! The project tree, and opening whatever is in it.
//!
//! mdedit is a markdown editor, so everything else it will show is deliberately
//! limited: an image is a picture, a binary is bytes, and anything else is
//! text. What a file *is* comes from its extension for the kinds that have to
//! be recognised on sight (markdown, images) and from its contents otherwise —
//! extensions run out fast in a source tree, where `Makefile`, `.gitignore`
//! and `LICENSE` are all perfectly ordinary text.

use std::path::{Path, PathBuf};

use base64::prelude::{Engine, BASE64_STANDARD};
use serde::Serialize;

/// Text and binaries are held in memory, edited, and undone; a megabyte of
/// either is already a strange thing to be editing here.
pub const TEXT_LIMIT: u64 = 1024 * 1024;
/// An image is only ever displayed, so the ceiling is about not wedging the
/// webview with a data URL rather than about editing.
pub const IMAGE_LIMIT: u64 = 16 * 1024 * 1024;

/// One row of the tree. Directories are not recursed into here — the frontend
/// asks for a directory's children when it is expanded, so opening a folder
/// with a huge `node_modules` in it costs nothing until someone clicks on it.
#[derive(Serialize)]
pub struct Entry {
    name: String,
    path: String,
    dir: bool,
}

#[tauri::command]
pub fn list_dir(path: String) -> Result<Vec<Entry>, String> {
    let mut entries: Vec<Entry> = std::fs::read_dir(&path)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| {
            let path = e.path();
            Entry {
                name: e.file_name().to_string_lossy().into_owned(),
                // `file_type` rather than `metadata`: a symlink to a directory
                // that has gone missing should still list, as a file.
                dir: e.file_type().map(|t| t.is_dir()).unwrap_or(false),
                path: path.display().to_string(),
            }
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

/// What kind of thing a path holds, and enough of it to show.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnyFile {
    pub path: String,
    pub name: String,
    /// `markdown` | `text` | `image` | `binary` | `oversized`
    pub kind: &'static str,
    /// Markdown and text only.
    pub text: String,
    /// Base64: the image's bytes, or the binary's. Empty otherwise, because
    /// text does not need to make the trip twice.
    pub data: String,
    /// For the image's data URL.
    pub mime: &'static str,
    pub size: u64,
}

fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub fn is_markdown_ext(path: &Path) -> bool {
    matches!(ext_of(path).as_str(), "md" | "markdown" | "mdown" | "mkd")
}

/// The image types a webview will render. SVG is text, but nobody wants to
/// read an icon as source, so it is shown as the picture it is — its markup is
/// a keystroke away in any case, via the text editor of your choice.
fn image_mime(path: &Path) -> Option<&'static str> {
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

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_string())
}

fn oversized(path: &Path, size: u64) -> AnyFile {
    AnyFile {
        name: file_name_of(path),
        path: path.display().to_string(),
        kind: "oversized",
        text: String::new(),
        data: String::new(),
        mime: "",
        size,
    }
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
    let name = file_name_of(path);
    let path_s = path.display().to_string();

    if let Some(mime) = image_mime(path) {
        if size > IMAGE_LIMIT {
            return Ok(oversized(path, size));
        }
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        return Ok(AnyFile {
            path: path_s,
            name,
            kind: "image",
            text: String::new(),
            data: BASE64_STANDARD.encode(&bytes),
            mime,
            size,
        });
    }

    if size > TEXT_LIMIT {
        return Ok(oversized(path, size));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if looks_binary(&bytes) {
        return Ok(AnyFile {
            path: path_s,
            name,
            kind: "binary",
            text: String::new(),
            data: BASE64_STANDARD.encode(&bytes),
            mime: "",
            size,
        });
    }
    // `looks_binary` already proved this is UTF-8.
    let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
    Ok(AnyFile {
        path: path_s,
        name,
        kind: if is_markdown_ext(path) {
            "markdown"
        } else {
            "text"
        },
        text,
        data: String::new(),
        mime: "",
        size,
    })
}

pub fn decode(data: &str) -> Result<Vec<u8>, String> {
    BASE64_STANDARD.decode(data).map_err(|e| e.to_string())
}

pub fn canonical(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    p.canonicalize().unwrap_or(p)
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

        assert_eq!(read_any(&dir.join("a.md")).unwrap().kind, "markdown");
        let text = read_any(&dir.join("b.rs")).unwrap();
        assert_eq!(text.kind, "text");
        assert_eq!(text.text, "fn main() {}\n");
        let bin = read_any(&dir.join("c.bin")).unwrap();
        assert_eq!(bin.kind, "binary");
        assert_eq!(decode(&bin.data).unwrap(), vec![0u8, 1, 2, 3]);
        let img = read_any(&dir.join("d.png")).unwrap();
        assert_eq!(img.kind, "image");
        assert_eq!(img.mime, "image/png");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_to_load_what_is_too_big() {
        let dir = scratch("big");
        let big = dir.join("big.txt");
        std::fs::write(&big, vec![b'a'; TEXT_LIMIT as usize + 1]).unwrap();
        let f = read_any(&big).unwrap();
        assert_eq!(f.kind, "oversized");
        assert_eq!(f.size, TEXT_LIMIT + 1);
        assert!(f.text.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lists_directories_first_then_case_insensitively() {
        let dir = scratch("list");
        std::fs::create_dir(dir.join("zeta")).unwrap();
        std::fs::write(dir.join("Alpha.md"), "").unwrap();
        std::fs::write(dir.join("beta.md"), "").unwrap();
        std::fs::create_dir(dir.join("Apex")).unwrap();
        let names: Vec<String> = list_dir(dir.display().to_string())
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, vec!["Apex", "zeta", "Alpha.md", "beta.md"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
