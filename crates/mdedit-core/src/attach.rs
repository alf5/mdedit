//! Images brought into a document, by pasting or dropping: stored in an
//! `assets` folder beside it (as ZenNotes and Obsidian do) unless they
//! already live under the document's folder, and linked relatively.

use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The folder pasted and copied-in images go to, beside the document.
pub const ASSETS: &str = "assets";

/// `pasted-20261117-120501`: when, in UTC, to the second.
pub fn pasted_stem(now: SystemTime) -> String {
    let secs = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil(days as i64);
    format!(
        "pasted-{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// Days since 1970-01-01 as a (year, month, day): Howard Hinnant's
/// `civil_from_days`.
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// A name in `dir` like `stem.ext` that isn't taken: `stem-2.ext` and on.
fn free_name(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let dot = if ext.is_empty() {
        String::new()
    } else {
        format!(".{ext}")
    };
    let first = dir.join(format!("{stem}{dot}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{stem}-{n}{dot}")))
        .find(|p| !p.exists())
        .expect("some name is free")
}

/// Write `bytes` into `doc_dir/assets` as `stem.ext` (or the first free
/// variant). The file written.
pub fn store(doc_dir: &Path, stem: &str, ext: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    let dir = doc_dir.join(ASSETS);
    std::fs::create_dir_all(&dir)?;
    let path = free_name(&dir, stem, ext);
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// The file a dropped `file` is linked as: itself when it is under
/// `doc_dir` already, else a copy in `doc_dir/assets`.
pub fn bring_in(doc_dir: &Path, file: &Path) -> io::Result<PathBuf> {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    if canon(file).starts_with(canon(doc_dir)) {
        return Ok(file.to_path_buf());
    }
    let stem = file
        .file_stem()
        .map_or("image".into(), |s| s.to_string_lossy().to_string());
    let ext = file
        .extension()
        .map_or(String::new(), |e| e.to_string_lossy().to_string());
    let dir = doc_dir.join(ASSETS);
    std::fs::create_dir_all(&dir)?;
    let path = free_name(&dir, &stem, &ext);
    std::fs::copy(file, &path)?;
    Ok(path)
}

/// How a document in `doc_dir` links to `file`: a relative path with `/`
/// between parts, spaces as `%20` (which image loading decodes).
pub fn link(doc_dir: &Path, file: &Path) -> String {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let (base, target) = (canon(doc_dir), canon(file));
    let rel = relative(&base, &target).unwrap_or(target);
    rel.components()
        .map(|c| match c {
            Component::ParentDir => "..".to_string(),
            c => c.as_os_str().to_string_lossy().replace(' ', "%20"),
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// `target` relative to `base` (both absolute), climbing with `..`.
fn relative(base: &Path, target: &Path) -> Option<PathBuf> {
    let b: Vec<Component> = base.components().collect();
    let t: Vec<Component> = target.components().collect();
    if b.first() != t.first() {
        return None;
    }
    let common = b.iter().zip(&t).take_while(|(x, y)| x == y).count();
    let mut out = PathBuf::new();
    for _ in common..b.len() {
        out.push("..");
    }
    for c in &t[common..] {
        out.push(c.as_os_str());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn pasted_names_carry_the_time() {
        let t = UNIX_EPOCH + Duration::from_secs(1_795_435_501);
        assert_eq!(pasted_stem(t), "pasted-20261123-120501");
        assert_eq!(pasted_stem(UNIX_EPOCH), "pasted-19700101-000000");
    }

    #[test]
    fn images_are_stored_and_linked_relatively() {
        let root = std::env::temp_dir().join(format!("mdedit-attach-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let doc = root.join("notes");
        std::fs::create_dir_all(doc.join("pics")).unwrap();
        let a = store(&doc, "shot", "png", b"1").unwrap();
        let b = store(&doc, "shot", "png", b"2").unwrap();
        assert_eq!(link(&doc, &a), "assets/shot.png");
        assert_eq!(link(&doc, &b), "assets/shot-2.png");
        // Under the folder already: linked where it is.
        std::fs::write(doc.join("pics/my cat.jpg"), b"c").unwrap();
        let c = bring_in(&doc, &doc.join("pics/my cat.jpg")).unwrap();
        assert_eq!(link(&doc, &c), "pics/my%20cat.jpg");
        // From elsewhere: copied in.
        std::fs::write(root.join("away.gif"), b"g").unwrap();
        let d = bring_in(&doc, &root.join("away.gif")).unwrap();
        assert_eq!(link(&doc, &d), "assets/away.gif");
        assert_eq!(std::fs::read(&d).unwrap(), b"g");
        assert_eq!(link(&doc.join("pics"), &a), "../assets/shot.png");
        std::fs::remove_dir_all(&root).ok();
    }
}
