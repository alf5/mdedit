//! A look over every Markdown file in the open folder: the `#tags` each
//! uses and the links each makes, for tag completion and backlinks. Built in
//! the background; nothing here watches for changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::syntax;
use crate::wiki;

/// One link out of a note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkOut {
    /// A wiki link's note (and `#part`), or a Markdown link's URL.
    pub target: String,
    pub wiki: bool,
    /// The line it's on (from 0), and that line's text, trimmed.
    pub line: usize,
    pub text: String,
}

#[derive(Clone, Debug, Default)]
pub struct Note {
    pub path: PathBuf,
    pub tags: Vec<String>,
    pub links: Vec<LinkOut>,
}

#[derive(Debug, Default)]
pub struct Scan {
    pub root: PathBuf,
    pub notes: Vec<Note>,
    /// Every file, for resolving wiki links as the editor does.
    pub index: wiki::Index,
}

/// Files bigger than this aren't notes anyone links from.
const MAX_BYTES: u64 = 1 << 20;
const MAX_FILES: usize = 5000;

fn is_markdown(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "md" | "markdown" | "mdown" | "mkd"
        )
    })
}

/// What one note's text holds.
pub fn note(path: PathBuf, text: &str) -> Note {
    let s = syntax::parse(text);
    let mut tags: Vec<String> = s.tags(text).into_iter().map(str::to_string).collect();
    tags.sort();
    tags.dedup();
    let links = s
        .links
        .iter()
        .filter(|l| {
            l.wiki
                || !l.url.contains("://")
                    && !l.url.starts_with("mailto:")
                    && !l.url.starts_with('#')
        })
        .map(|l| {
            let line = s.line_at(l.range.start);
            LinkOut {
                target: l.url.clone(),
                wiki: l.wiki,
                line,
                text: text[s.lines[line].range.clone()].trim().to_string(),
            }
        })
        .collect();
    Note { path, tags, links }
}

impl Scan {
    pub fn build(root: &Path) -> Scan {
        let notes = crate::project::walk_files(root, MAX_FILES)
            .into_iter()
            .filter(|p| is_markdown(p))
            .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() <= MAX_BYTES))
            .filter_map(|p| {
                let text = std::fs::read_to_string(&p).ok()?;
                Some(note(p, &text))
            })
            .collect();
        Scan {
            root: root.to_path_buf(),
            notes,
            index: wiki::Index::build(root),
        }
    }

    /// Every tag, most used first, then by name.
    pub fn tags(&self) -> Vec<(String, usize)> {
        let mut count: HashMap<&str, usize> = HashMap::new();
        for n in &self.notes {
            for t in &n.tags {
                *count.entry(t).or_default() += 1;
            }
        }
        let mut out: Vec<(String, usize)> =
            count.into_iter().map(|(t, n)| (t.to_string(), n)).collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out
    }

    /// The links into `target` from the other notes: which note, and the
    /// link. A wiki link counts when it resolves to `target` the way the
    /// editor follows it; a Markdown link when its path, from its note's
    /// folder, is `target`.
    pub fn backlinks(&self, target: &Path) -> Vec<(&Note, &LinkOut)> {
        let same = |p: &Path| p == target || same_file(p, target);
        let mut out = Vec::new();
        for n in &self.notes {
            if same(&n.path) {
                continue;
            }
            let dir = n.path.parent();
            for l in &n.links {
                let hit = if l.wiki {
                    let (note, _) = wiki::split(&l.target);
                    wiki::resolve(note, dir, Some(&self.root), Some(&self.index))
                        .is_some_and(|p| same(&p))
                } else {
                    let path = l.target.split('#').next().unwrap_or("").replace("%20", " ");
                    !path.is_empty() && dir.is_some_and(|d| same(&d.join(&path)))
                };
                if hit {
                    out.push((n, l));
                }
            }
        }
        out
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_give_their_tags_and_links() {
        let n = note(
            "a.md".into(),
            "# Title #nottag\n\nSome #rust and #gpui/kit, #rust again.\n\nSee [[Other note#Part]] and [b](b.md), [web](https://x.y).\n",
        );
        assert_eq!(n.tags, vec!["gpui/kit", "rust"]);
        assert_eq!(n.links.len(), 2);
        assert_eq!(n.links[0].target, "Other note#Part");
        assert!(n.links[0].wiki);
        assert_eq!(n.links[0].line, 4);
        assert_eq!(n.links[1].target, "b.md");
    }

    #[test]
    fn a_folder_gives_tags_and_backlinks() {
        let dir = std::env::temp_dir().join(format!("mdedit-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("Target.md"), "# Target\n\nTagged #x\n").unwrap();
        std::fs::write(dir.join("one.md"), "Links [[Target]] here #x #y\n").unwrap();
        std::fs::write(
            dir.join("sub/two.md"),
            "Up [t](../Target.md)\n\nand [[Elsewhere]]\n",
        )
        .unwrap();
        std::fs::write(dir.join("sub/three.md"), "Also [[target#Top|alias]]\n").unwrap();
        let scan = Scan::build(&dir);
        assert_eq!(scan.tags()[0], ("x".to_string(), 2));
        let mut from: Vec<String> = scan
            .backlinks(&dir.join("Target.md"))
            .iter()
            .map(|(n, l)| {
                format!(
                    "{}:{}",
                    n.path.file_name().unwrap().to_string_lossy(),
                    l.line
                )
            })
            .collect();
        from.sort();
        assert_eq!(from, vec!["one.md:0", "three.md:0", "two.md:0"]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
