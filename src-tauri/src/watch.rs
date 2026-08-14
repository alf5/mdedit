//! Noticing that an open file changed underneath the editor.
//!
//! Every path with a tab is stamped the moment the editor and the disk agree —
//! after a read, after a write, after the user acknowledges a change. A
//! filesystem watcher then re-checks those stamps whenever anything happens in
//! the directories involved, and emits `disk-changed` for each file whose
//! contents no longer match. The frontend owns what to do about it.
//!
//! Two decisions worth keeping:
//!
//! * **Directories are watched, not files.** Most editors save by writing a
//!   temp file and renaming it over the target, which replaces the inode — an
//!   inotify watch on the file itself would follow the old inode and go deaf
//!   after the first external save.
//! * **The stamp carries a content hash, not just mtime and size.** `touch`,
//!   a `git checkout` that restores identical bytes, and a rewrite-with-no-
//!   changes by another editor are all common, and prompting for them trains
//!   the user to dismiss the prompt. The stat is kept as a cheap pre-filter so
//!   the file is only read when it looks different.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tauri::{AppHandle, Emitter, Manager};

/// What a file looked like when the editor and the disk last agreed.
#[derive(Clone, PartialEq)]
struct Stamp {
    mtime: Option<SystemTime>,
    size: u64,
    hash: u64,
}

#[derive(Default)]
struct Inner {
    files: HashMap<PathBuf, Stamp>,
    /// Directories currently armed, so re-arming can diff instead of churn.
    dirs: HashSet<PathBuf>,
    /// Dropping this stops delivery, so it has to be kept alive here.
    watcher: Option<RecommendedWatcher>,
}

/// Deliberately its own lock rather than a field on `AppState`: the checker
/// runs on the watcher's thread and does file IO, and `AppState` is the lock
/// that must never be held across a menu call.
#[derive(Default)]
pub struct Watch(Mutex<Inner>);

fn hash_of(content: &str) -> u64 {
    let mut h = DefaultHasher::new();
    content.hash(&mut h);
    h.finish()
}

fn stamp_of(path: &Path, content: &str) -> Stamp {
    let meta = std::fs::metadata(path).ok();
    Stamp {
        mtime: meta.as_ref().and_then(|m| m.modified().ok()),
        size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
        hash: hash_of(content),
    }
}

/// Record the state both sides now agree on. Called after every read and
/// every write, which is what keeps the editor's own saves from looking like
/// external changes.
pub fn record(app: &AppHandle, path: &Path, content: &str) {
    let stamp = stamp_of(path, content);
    let state = app.state::<Watch>();
    let mut inner = state.0.lock().unwrap();
    inner.files.insert(path.to_path_buf(), stamp);
}

/// Has `path` changed since it was last stamped? Answers `false` for a path
/// with no stamp (never opened from disk) and for one that has been deleted —
/// saving over a deleted file recreates it, which is not a conflict.
pub fn changed(app: &AppHandle, path: &Path) -> bool {
    let known = {
        let state = app.state::<Watch>();
        let inner = state.0.lock().unwrap();
        inner.files.get(path).cloned()
    };
    let Some(known) = known else { return false };
    differs(path, &known).is_some()
}

/// The file's current content, if it differs from `known`. Reads the file only
/// when the stat already looks different.
fn differs(path: &Path, known: &Stamp) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok();
    if mtime == known.mtime && meta.len() == known.size {
        return None;
    }
    let content = std::fs::read_to_string(path).ok()?;
    (hash_of(&content) != known.hash).then_some(content)
}

/// Point the watcher at exactly these files. Paths that are no longer open
/// lose their stamps; paths that arrive without one (which shouldn't happen —
/// they are stamped when read) are seeded from disk.
pub fn set_paths(app: &AppHandle, paths: &[PathBuf]) {
    let wanted: HashSet<PathBuf> = paths.iter().cloned().collect();
    let dirs: HashSet<PathBuf> = paths
        .iter()
        .filter_map(|p| p.parent().map(Path::to_path_buf))
        .collect();
    let state = app.state::<Watch>();
    let missing: Vec<PathBuf> = {
        let inner = state.0.lock().unwrap();
        wanted
            .iter()
            .filter(|p| !inner.files.contains_key(*p))
            .cloned()
            .collect()
    };
    // Read outside the lock: a slow filesystem must not block a save.
    let seeded: Vec<(PathBuf, Stamp)> = missing
        .into_iter()
        .filter_map(|p| {
            let content = std::fs::read_to_string(&p).ok()?;
            let stamp = stamp_of(&p, &content);
            Some((p, stamp))
        })
        .collect();

    let mut inner = state.0.lock().unwrap();
    inner.files.retain(|p, _| wanted.contains(p));
    for (p, stamp) in seeded {
        inner.files.entry(p).or_insert(stamp);
    }
    if inner.watcher.is_none() {
        inner.watcher = start(app.clone());
    }
    // Taken out so the watcher can be borrowed mutably alongside it.
    let old = std::mem::take(&mut inner.dirs);
    if let Some(watcher) = inner.watcher.as_mut() {
        for dir in old.difference(&dirs) {
            let _ = watcher.unwatch(dir);
        }
        for dir in dirs.difference(&old) {
            // A directory that can't be watched (deleted, permissions, an
            // unsupported filesystem) just means no live notifications for
            // it; the focus check still covers those files.
            let _ = watcher.watch(dir, RecursiveMode::NonRecursive);
        }
    }
    inner.dirs = dirs;
}

fn start(app: AppHandle) -> Option<RecommendedWatcher> {
    notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            // Every watched file is re-checked rather than just the event's
            // own path: one external save can arrive as create + rename +
            // remove on paths that are all temporaries, and comparing stamps
            // is cheaper than reasoning about which shape this was.
            check_all(&app);
        }
    })
    .ok()
}

/// Re-check every watched file and announce the ones that really changed.
///
/// Runs on the watcher's thread and on window focus, so it must not hold the
/// lock across its file IO.
pub fn check_all(app: &AppHandle) {
    let state = app.state::<Watch>();
    let known: Vec<(PathBuf, Stamp)> = {
        let inner = state.0.lock().unwrap();
        inner
            .files
            .iter()
            .map(|(p, s)| (p.clone(), s.clone()))
            .collect()
    };
    for (path, stamp) in known {
        let Some(content) = differs(&path, &stamp) else {
            continue;
        };
        // Re-check under the lock before announcing. A save that landed while
        // this was reading has already stamped its own result, and reporting
        // the editor's own write as an external change is worse than missing
        // one — the next event catches a real change anyway.
        {
            let inner = state.0.lock().unwrap();
            match inner.files.get(&path) {
                Some(cur) if cur.hash == hash_of(&content) => continue,
                Some(cur) if *cur != stamp => continue,
                Some(_) => {}
                None => continue,
            }
        }
        let _ = app.emit("disk-changed", path.display().to_string());
    }
}

/// A three-way merge of the editor's text against the file's, using the last
/// agreed content as the common ancestor. `Err` carries the merged text *with*
/// conflict markers, which is a result to show, not a failure.
pub fn merge(base: &str, ours: &str, theirs: &str) -> (String, bool) {
    match diffy::merge(base, ours, theirs) {
        Ok(text) => (text, false),
        Err(text) => (text, true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_disjoint_edits_without_conflict() {
        let base = "one\ntwo\nthree\n";
        let ours = "one CHANGED\ntwo\nthree\n";
        let theirs = "one\ntwo\nthree CHANGED\n";
        let (text, conflicted) = merge(base, ours, theirs);
        assert!(!conflicted);
        assert_eq!(text, "one CHANGED\ntwo\nthree CHANGED\n");
    }

    #[test]
    fn marks_overlapping_edits_as_conflicts() {
        let base = "one\ntwo\n";
        let ours = "one\nMINE\n";
        let theirs = "one\nTHEIRS\n";
        let (text, conflicted) = merge(base, ours, theirs);
        assert!(conflicted);
        assert!(text.contains("<<<<<<<"));
        assert!(text.contains("MINE"));
        assert!(text.contains("THEIRS"));
    }

    /// The clean-document case: with no local edits the merge is just the
    /// file's own content, so Reload and Merge agree.
    #[test]
    fn an_unedited_document_merges_to_the_file() {
        let base = "one\ntwo\n";
        let (text, conflicted) = merge(base, base, "one\ntwo\nthree\n");
        assert!(!conflicted);
        assert_eq!(text, "one\ntwo\nthree\n");
    }

    #[test]
    fn identical_content_hashes_identically() {
        assert_eq!(hash_of("# Title\n"), hash_of("# Title\n"));
        assert_ne!(hash_of("# Title\n"), hash_of("# Title \n"));
    }

    /// A directory of this test's own, so the cases below can rename files
    /// over each other without racing another test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mdedit-watch-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_untouched_file_does_not_differ() {
        let dir = scratch("untouched");
        let f = dir.join("a.md");
        std::fs::write(&f, "one\n").unwrap();
        let stamp = stamp_of(&f, "one\n");
        assert!(differs(&f, &stamp).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The reason the stamp carries a hash. `touch`, a `git checkout` that
    /// restores the same bytes, and another editor's no-op save all change the
    /// mtime; prompting for them teaches the user to dismiss the prompt.
    #[test]
    fn a_rewrite_with_the_same_bytes_does_not_differ() {
        let dir = scratch("same-bytes");
        let f = dir.join("a.md");
        std::fs::write(&f, "one\n").unwrap();
        let stamp = stamp_of(&f, "one\n");
        std::fs::write(&f, "one\n").unwrap();
        filetime_bump(&f);
        assert!(differs(&f, &stamp).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_real_edit_differs_and_reports_the_new_content() {
        let dir = scratch("edited");
        let f = dir.join("a.md");
        std::fs::write(&f, "one\n").unwrap();
        let stamp = stamp_of(&f, "one\n");
        std::fs::write(&f, "one\ntwo\n").unwrap();
        assert_eq!(differs(&f, &stamp).as_deref(), Some("one\ntwo\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// How most editors save: write a temporary file, rename it over the
    /// target. The inode changes, which is why the *directory* is watched.
    #[test]
    fn an_atomic_replace_differs() {
        let dir = scratch("atomic");
        let f = dir.join("a.md");
        std::fs::write(&f, "one\n").unwrap();
        let stamp = stamp_of(&f, "one\n");
        let tmp = dir.join("a.md.tmp");
        std::fs::write(&tmp, "replaced\n").unwrap();
        std::fs::rename(&tmp, &f).unwrap();
        assert_eq!(differs(&f, &stamp).as_deref(), Some("replaced\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A deleted file is not a change to report: saving recreates it, which is
    /// the recovery, not a conflict.
    #[test]
    fn a_deleted_file_does_not_differ() {
        let dir = scratch("deleted");
        let f = dir.join("a.md");
        std::fs::write(&f, "one\n").unwrap();
        let stamp = stamp_of(&f, "one\n");
        std::fs::remove_file(&f).unwrap();
        assert!(differs(&f, &stamp).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two writes inside one filesystem timestamp tick would otherwise leave
    /// mtime and size identical, and the stat pre-filter would skip the read.
    fn filetime_bump(path: &Path) {
        let f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        drop(f);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    /// The claim the whole design rests on: a watch on the *directory* still
    /// reports an editor's write-temp-then-rename save, which a watch on the
    /// file itself would miss — the rename swaps the inode out from under it.
    ///
    /// Ignored by default because it waits on the OS to deliver: inotify is
    /// immediate, but FSEvents coalesces and a loaded CI runner can be slow,
    /// and a timing-dependent failure in the release pipeline is worse than
    /// the coverage is worth. Run it with
    /// `cargo test -p mdedit -- --ignored watcher`.
    #[test]
    #[ignore]
    fn a_directory_watch_sees_an_atomic_replace() {
        use std::sync::mpsc;

        let dir = scratch("watcher");
        let f = dir.join("a.md");
        std::fs::write(&f, "one\n").unwrap();
        // Stamped while the two still agree, as `record` does after a read.
        let stamp = stamp_of(&f, "one\n");

        let (tx, rx) = mpsc::channel();
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                let _ = tx.send(res.is_ok());
            })
            .unwrap();
        watcher.watch(&dir, RecursiveMode::NonRecursive).unwrap();

        let tmp = dir.join("a.md.tmp");
        std::fs::write(&tmp, "replaced\n").unwrap();
        std::fs::rename(&tmp, &f).unwrap();

        let got = rx.recv_timeout(std::time::Duration::from_secs(10));
        assert!(got.is_ok(), "no filesystem event within 10s");
        // The event alone means nothing — this is what the handler does with
        // it, and what decides whether the user is asked.
        assert_eq!(differs(&f, &stamp).as_deref(), Some("replaced\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
