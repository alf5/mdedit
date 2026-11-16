//! Following a `[[wiki link]]` to its note, as Obsidian does: by name, in the
//! document's own folder first, then anywhere in the project.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A link's target split into the note it names and the heading (or `^block`)
/// after a `#`: `Note#Part` is `("Note", Some("Part"))`. An empty note is
/// the document the link is in.
pub fn split(target: &str) -> (&str, Option<&str>) {
    match target.split_once('#') {
        Some((note, part)) => (note.trim(), Some(part.trim()).filter(|p| !p.is_empty())),
        None => (target.trim(), None),
    }
}

/// The file names a note could be stored under: as written when it has an
/// extension of its own (`photo.png`, `plan.md`), otherwise with `.md`.
fn candidates(note: &str) -> Vec<String> {
    let name = note.rsplit('/').next().unwrap_or(note);
    let has_ext = name
        .rsplit_once('.')
        .is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty() && !ext.contains(' '));
    if has_ext {
        vec![note.to_string(), format!("{note}.md")]
    } else {
        vec![format!("{note}.md"), format!("{note}.markdown")]
    }
}

/// Every file under a folder, by lower-cased name, for finding a note by name
/// wherever it is. Hidden folders and build output are skipped, and the walk
/// stops at a few levels and some thousands of files: it's built when a
/// link is drawn.
#[derive(Debug, Default)]
pub struct Index {
    root: PathBuf,
    by_name: HashMap<String, Vec<PathBuf>>,
}

const MAX_DEPTH: usize = 8;
const MAX_FILES: usize = 20_000;

impl Index {
    pub fn build(root: &Path) -> Index {
        let mut index = Index {
            root: root.to_path_buf(),
            by_name: HashMap::new(),
        };
        let mut stack = vec![(root.to_path_buf(), 0)];
        let mut seen = 0;
        while let Some((dir, depth)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') || name == "node_modules" || name == "target" {
                    continue;
                }
                let Ok(kind) = e.file_type() else { continue };
                if kind.is_dir() {
                    if depth < MAX_DEPTH {
                        stack.push((e.path(), depth + 1));
                    }
                } else {
                    seen += 1;
                    index
                        .by_name
                        .entry(name.to_lowercase())
                        .or_default()
                        .push(e.path());
                }
                if seen >= MAX_FILES {
                    return index;
                }
            }
        }
        index
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// The file a link's note is in: `dir/Note.md` when the document's folder
/// has it; a path (`folder/Note`) from that folder or from the project's
/// `root`; or a file of that name anywhere in `index`, the one nearest
/// `dir` if there are several. Names match whatever their case.
pub fn resolve(
    note: &str,
    dir: Option<&Path>,
    root: Option<&Path>,
    index: Option<&Index>,
) -> Option<PathBuf> {
    let note = note.trim().trim_start_matches('/');
    if note.is_empty() {
        return None;
    }
    let names = candidates(note);
    for base in [dir, root].into_iter().flatten() {
        for name in &names {
            let p = base.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let index = index?;
    for name in &names {
        let file = name.rsplit('/').next().unwrap_or(name).to_lowercase();
        let Some(found) = index.by_name.get(&file) else {
            continue;
        };
        // A path in the link has to match the end of the file's path.
        let tail = name.to_lowercase();
        let mut fits: Vec<&PathBuf> = found
            .iter()
            .filter(|p| {
                let p = p.to_string_lossy().to_lowercase().replace('\\', "/");
                p.ends_with(&format!("/{tail}")) || p == tail
            })
            .collect();
        fits.sort_by_key(|p| {
            let near = dir.is_some_and(|d| p.starts_with(d));
            (!near, p.components().count())
        });
        if let Some(p) = fits.first() {
            return Some((*p).clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_names_a_note_and_maybe_a_part_of_it() {
        assert_eq!(split("Note"), ("Note", None));
        assert_eq!(split("Note#Part two"), ("Note", Some("Part two")));
        assert_eq!(split("#Here"), ("", Some("Here")));
        assert_eq!(split("Note#"), ("Note", None));
    }

    #[test]
    fn notes_are_found_by_name_nearest_first() {
        let tmp = std::env::temp_dir().join(format!("mdedit-wiki-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let docs = tmp.join("docs");
        std::fs::create_dir_all(docs.join("deep")).unwrap();
        std::fs::create_dir_all(tmp.join("other")).unwrap();
        std::fs::create_dir_all(tmp.join(".hidden")).unwrap();
        for f in [
            "docs/Here.md",
            "docs/deep/Linked Note.md",
            "other/Linked Note.md",
            "other/Only There.md",
            "other/photo.png",
            ".hidden/Secret.md",
        ] {
            std::fs::write(tmp.join(f), "x").unwrap();
        }
        let index = Index::build(&tmp);
        let find = |n: &str| resolve(n, Some(&docs), Some(&tmp), Some(&index));
        assert_eq!(find("Here"), Some(docs.join("Here.md")));
        // Whatever the case (as spelt on a filesystem that ignores it).
        assert!(find("here").is_some_and(|p| p.parent() == Some(docs.as_path())));
        // The nearer of two of the same name.
        assert_eq!(find("Linked Note"), Some(docs.join("deep/Linked Note.md")));
        assert_eq!(find("linked note"), Some(docs.join("deep/Linked Note.md")));
        assert_eq!(
            find("other/Linked Note"),
            Some(tmp.join("other/Linked Note.md"))
        );
        assert_eq!(find("Only There"), Some(tmp.join("other/Only There.md")));
        assert_eq!(find("photo.png"), Some(tmp.join("other/photo.png")));
        assert_eq!(find("Secret"), None);
        assert_eq!(find("Missing"), None);
        assert_eq!(find(""), None);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
