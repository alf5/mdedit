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
//!
//! And one hazard that watching directories creates: **this module's own
//! reads are watched too.** inotify reports an `open()` on a watched
//! directory's file, so a checker that reacts to every event feeds itself —
//! read, event, read — spinning for as long as any file stays unreconciled
//! and burying the frontend under `disk-changed`. Two independent guards keep
//! that shut: `is_write` drops the access events a read can produce, and
//! `announced` makes a divergence emit once however many events arrive.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use notify::event::{AccessKind, AccessMode};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
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
    /// The content hash each path was last announced to the frontend with,
    /// cleared the moment the two sides agree again. One save arrives as
    /// several filesystem events and the user's answer is a round trip, so
    /// without this the same divergence is announced over and over — and
    /// every announcement after the first reopens a prompt already answered.
    announced: HashMap<PathBuf, u64>,
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

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

fn stamp_of(path: &Path, bytes: &[u8]) -> Stamp {
    let meta = std::fs::metadata(path).ok();
    Stamp {
        mtime: meta.as_ref().and_then(|m| m.modified().ok()),
        size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
        hash: hash_bytes(bytes),
    }
}

/// Record the state both sides now agree on. Called after every read and
/// every write, which is what keeps the editor's own saves from looking like
/// external changes.
pub fn record(app: &AppHandle, path: &Path, content: &str) {
    record_bytes(app, path, content.as_bytes());
}

/// The same, for the files the editor holds as bytes rather than text.
pub fn record_bytes(app: &AppHandle, path: &Path, bytes: &[u8]) {
    let stamp = stamp_of(path, bytes);
    let state = app.state::<Watch>();
    let mut inner = state.0.lock().unwrap();
    inner.files.insert(path.to_path_buf(), stamp);
    // The two agree again, so the next divergence is news even if it happens
    // to restore the very content that was announced last time.
    inner.announced.remove(path);
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
    matches!(look(path, &known), Look::Changed(_))
}

/// What a fresh look at a file says about its stamp.
enum Look {
    /// The stat still matches; the file was not even opened.
    Unchanged,
    /// The stat moved but the bytes did not — a `touch`, a `git checkout`
    /// that restored the same content, another editor's no-op save. Carries
    /// the refreshed stamp, which has to replace the old one: leaving the
    /// stale mtime in place defeats the pre-filter, so every later event
    /// would read the whole file again to learn nothing.
    Restat(Stamp),
    /// Gone or unreadable. Not a change to report: saving over a deleted file
    /// recreates it, which is the recovery, not a conflict.
    Absent,
    /// Really different, with the file's current bytes.
    Changed(Vec<u8>),
}

/// Compare `path` against `known`, reading it only when the stat already
/// looks different.
///
/// Bytes rather than text throughout: a binary open in the hex editor deserves
/// the same protection as a document, and `read_to_string` would simply fail
/// on it and report "unchanged" forever.
fn look(path: &Path, known: &Stamp) -> Look {
    let Ok(meta) = std::fs::metadata(path) else {
        return Look::Absent;
    };
    let mtime = meta.modified().ok();
    if mtime == known.mtime && meta.len() == known.size {
        return Look::Unchanged;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return Look::Absent;
    };
    let hash = hash_bytes(&bytes);
    if hash == known.hash {
        return Look::Restat(Stamp {
            mtime,
            size: meta.len(),
            hash,
        });
    }
    Look::Changed(bytes)
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
            let bytes = std::fs::read(&p).ok()?;
            let stamp = stamp_of(&p, &bytes);
            Some((p, stamp))
        })
        .collect();

    let mut inner = state.0.lock().unwrap();
    inner.files.retain(|p, _| wanted.contains(p));
    inner.announced.retain(|p, _| wanted.contains(p));
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

/// Could this event mean someone *wrote*? Excluding reads is not an
/// optimisation, it is what stops the watcher feeding itself: `check_all`
/// opens the file, inotify reports that open back to this same callback as
/// `Access(Open)`, and treating it as news re-enters `check_all` — a hot loop
/// of reads and `disk-changed` emits that runs for as long as a file stays
/// unreconciled, which is exactly as long as the user leaves the prompt up.
///
/// `Access(Close(Write))` is the one access a read cannot produce, and it is
/// a genuine save signal, so it stays. Everything that is not an access —
/// create, modify, rename, remove — is a write until proven otherwise.
fn is_write(kind: &EventKind) -> bool {
    match kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) => false,
        _ => true,
    }
}

fn start(app: AppHandle) -> Option<RecommendedWatcher> {
    notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        if !is_write(&event.kind) {
            return;
        }
        // Every watched file is re-checked rather than just the event's
        // own path: one external save can arrive as create + rename +
        // remove on paths that are all temporaries, and comparing stamps
        // is cheaper than reasoning about which shape this was.
        check_all(&app);
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
        let bytes = match look(&path, &stamp) {
            Look::Unchanged | Look::Absent => continue,
            // Nothing to tell the frontend, but the fresh stat is worth
            // keeping: it is what lets the next check stop at the stat.
            Look::Restat(fresh) => {
                let mut inner = state.0.lock().unwrap();
                if let Some(cur) = inner.files.get_mut(&path) {
                    // Only if nothing re-stamped it while this was reading.
                    if *cur == stamp {
                        *cur = fresh;
                    }
                }
                continue;
            }
            Look::Changed(bytes) => bytes,
        };
        let hash = hash_bytes(&bytes);
        // Re-check under the lock before announcing. A save that landed while
        // this was reading has already stamped its own result, and reporting
        // the editor's own write as an external change is worse than missing
        // one — the next event catches a real change anyway.
        {
            let mut inner = state.0.lock().unwrap();
            match inner.files.get(&path) {
                Some(cur) if cur.hash == hash => continue,
                Some(cur) if *cur != stamp => continue,
                Some(_) => {}
                None => continue,
            }
            // One question per divergence, however many events carried it.
            if inner.announced.insert(path.clone(), hash) == Some(hash) {
                continue;
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
        assert_eq!(hash_bytes(b"# Title\n"), hash_bytes(b"# Title\n"));
        assert_ne!(hash_bytes(b"# Title\n"), hash_bytes(b"# Title \n"));
    }

    /// Text and bytes have to agree: a markdown file is stamped from a `&str`
    /// and a binary from a `&[u8]`, and a document that switches hands between
    /// the two must not look changed for it.
    #[test]
    fn text_and_bytes_hash_the_same_content_alike() {
        assert_eq!(hash_bytes("hello".as_bytes()), hash_bytes(b"hello"));
    }

    /// The changed bytes, for the cases that assert on content.
    fn changed_bytes(path: &Path, known: &Stamp) -> Option<Vec<u8>> {
        match look(path, known) {
            Look::Changed(bytes) => Some(bytes),
            _ => None,
        }
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
    fn an_untouched_file_is_not_even_opened() {
        let dir = scratch("untouched");
        let f = dir.join("a.md");
        std::fs::write(&f, "one\n").unwrap();
        let stamp = stamp_of(&f, b"one\n");
        assert!(matches!(look(&f, &stamp), Look::Unchanged));
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
        let stamp = stamp_of(&f, b"one\n");
        std::fs::write(&f, "one\n").unwrap();
        filetime_bump(&f);
        assert!(matches!(look(&f, &stamp), Look::Restat(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The refreshed stamp has to make the *next* look stop at the stat.
    /// Without it a touched file is read on every single event for the rest
    /// of the session, which is invisible until it is the thing making the
    /// machine hot.
    #[test]
    fn a_restat_settles_the_file_for_good() {
        let dir = scratch("restat");
        let f = dir.join("a.md");
        std::fs::write(&f, "one\n").unwrap();
        let stamp = stamp_of(&f, b"one\n");
        std::fs::write(&f, "one\n").unwrap();
        filetime_bump(&f);
        let Look::Restat(fresh) = look(&f, &stamp) else {
            panic!("expected a restat");
        };
        assert!(matches!(look(&f, &fresh), Look::Unchanged));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_real_edit_differs_and_reports_the_new_content() {
        let dir = scratch("edited");
        let f = dir.join("a.md");
        std::fs::write(&f, "one\n").unwrap();
        let stamp = stamp_of(&f, b"one\n");
        std::fs::write(&f, "one\ntwo\n").unwrap();
        assert_eq!(
            changed_bytes(&f, &stamp).as_deref(),
            Some(&b"one\ntwo\n"[..])
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// How most editors save: write a temporary file, rename it over the
    /// target. The inode changes, which is why the *directory* is watched.
    #[test]
    fn an_atomic_replace_differs() {
        let dir = scratch("atomic");
        let f = dir.join("a.md");
        std::fs::write(&f, "one\n").unwrap();
        let stamp = stamp_of(&f, b"one\n");
        let tmp = dir.join("a.md.tmp");
        std::fs::write(&tmp, "replaced\n").unwrap();
        std::fs::rename(&tmp, &f).unwrap();
        assert_eq!(
            changed_bytes(&f, &stamp).as_deref(),
            Some(&b"replaced\n"[..])
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A deleted file is not a change to report: saving recreates it, which is
    /// the recovery, not a conflict.
    #[test]
    fn a_deleted_file_does_not_differ() {
        let dir = scratch("deleted");
        let f = dir.join("a.md");
        std::fs::write(&f, "one\n").unwrap();
        let stamp = stamp_of(&f, b"one\n");
        std::fs::remove_file(&f).unwrap();
        assert!(matches!(look(&f, &stamp), Look::Absent));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The guard that keeps the watcher from feeding itself. `check_all`
    /// reads every file it is unsure about, and on Linux that read comes
    /// straight back as `Access(Open)` on the watched directory; acting on it
    /// re-enters `check_all`, which reads again. Left open, one external edit
    /// spun this loop 170k times in three seconds.
    #[test]
    fn a_read_of_our_own_is_not_a_write() {
        assert!(!is_write(&EventKind::Access(AccessKind::Open(
            AccessMode::Any
        ))));
        assert!(!is_write(&EventKind::Access(AccessKind::Read)));
        assert!(!is_write(&EventKind::Access(AccessKind::Close(
            AccessMode::Read
        ))));
    }

    /// …while every shape a real save arrives in still gets through. A write
    /// is the one access that cannot come from a read.
    #[test]
    fn every_shape_of_a_save_is_a_write() {
        use notify::event::{CreateKind, ModifyKind, RemoveKind, RenameMode};
        assert!(is_write(&EventKind::Access(AccessKind::Close(
            AccessMode::Write
        ))));
        assert!(is_write(&EventKind::Modify(ModifyKind::Data(
            notify::event::DataChange::Any
        ))));
        assert!(is_write(&EventKind::Modify(ModifyKind::Name(
            RenameMode::To
        ))));
        assert!(is_write(&EventKind::Create(CreateKind::File)));
        assert!(is_write(&EventKind::Remove(RemoveKind::File)));
        // FSEvents and the polling fallback report changes without a shape.
        assert!(is_write(&EventKind::Any));
        assert!(is_write(&EventKind::Modify(ModifyKind::Any)));
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
    /// `cargo test -p mdedit --lib -- --ignored`.
    #[test]
    #[ignore]
    fn a_directory_watch_sees_an_atomic_replace() {
        use std::sync::mpsc;

        let dir = scratch("watcher");
        let f = dir.join("a.md");
        std::fs::write(&f, "one\n").unwrap();
        // Stamped while the two still agree, as `record` does after a read.
        let stamp = stamp_of(&f, b"one\n");

        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
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
        assert_eq!(
            changed_bytes(&f, &stamp).as_deref(),
            Some(&b"replaced\n"[..])
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The regression this module's worst bug leaves behind. `check_all`
    /// reads a file it is unsure about; inotify reports that read back as an
    /// event on the watched directory; a handler that acts on it reads again.
    /// One external edit spun that loop about 170,000 times in three seconds,
    /// re-announcing `disk-changed` every turn — which reopened the prompt as
    /// fast as the user could answer it, and heated the machine while doing
    /// it. The loop stays shut only while none of the events a *read* provokes
    /// counts as a write.
    ///
    /// Ignored by default for the same reason as the test above: it waits on
    /// the OS to deliver. Run it with
    /// `cargo test -p mdedit --lib -- --ignored`.
    #[test]
    #[ignore]
    fn our_own_reads_do_not_look_like_writes() {
        use std::sync::mpsc;
        use std::time::Duration;

        let dir = scratch("self-feed");
        let f = dir.join("a.md");
        std::fs::write(&f, "one\n").unwrap();
        // Stale on purpose, and stale in the stat too, so every `look` gets
        // past the pre-filter and actually opens the file. That is the state
        // that made the old code read on every event: an unanswered prompt,
        // or a plain `touch`.
        let stamp = Stamp {
            mtime: None,
            size: 0,
            hash: hash_bytes(b"different\n"),
        };

        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                let _ = tx.send(ev.kind);
            }
        })
        .unwrap();
        watcher.watch(&dir, RecursiveMode::NonRecursive).unwrap();

        // Let the events from the setup write drain away.
        while rx.recv_timeout(Duration::from_millis(300)).is_ok() {}

        // Now do what the watcher's own handler does, repeatedly.
        for _ in 0..20 {
            assert!(matches!(look(&f, &stamp), Look::Changed(_)));
        }

        // Whatever those reads provoked, none of it may re-enter the handler.
        let mut seen = 0;
        while let Ok(kind) = rx.recv_timeout(Duration::from_millis(300)) {
            seen += 1;
            assert!(
                !is_write(&kind),
                "a read of our own came back as a write ({kind:?}): the watcher feeds itself"
            );
        }
        // Not an assertion that `seen` is large — macOS reports no reads at
        // all — but the Linux case this guards is worth naming.
        eprintln!("{seen} event(s) provoked by 20 reads, none of them writes");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
