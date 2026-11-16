//! Settings and recent files, kept between runs.
//!
//! One JSON file, `<config>/mdedit/settings.json`. Every field has a default,
//! so a file from an older or newer version still loads, and a missing,
//! unreadable or corrupt one just means the defaults: losing preferences is
//! never worth an error. v1 kept a smaller file under the Tauri identifier
//! (`<config>/com.alf5.mdedit/recent.json`); it is read once, the first time
//! there is no v2 file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Open Recent keeps this many.
pub const RECENT_MAX: usize = 10;

/// How a markdown tab is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Formatting drawn, markup hidden everywhere: WYSIWYG.
    #[default]
    Rendered,
    /// Markup shown in the block holding the caret ("Show Markdown at
    /// Cursor" in v1).
    Live,
    /// The markdown as text.
    Source,
}

/// The document typography, from `document.css`'s `md-style-*` sheets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    Ogma,
    #[default]
    GitHub,
    Manuscript,
    Compact,
    Typewriter,
}

impl Style {
    pub const ALL: [Style; 5] = [
        Style::Ogma,
        Style::GitHub,
        Style::Manuscript,
        Style::Compact,
        Style::Typewriter,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Style::Ogma => "Ogma",
            Style::GitHub => "GitHub",
            Style::Manuscript => "Manuscript",
            Style::Compact => "Compact",
            Style::Typewriter => "Typewriter",
        }
    }

    /// The class an HTML export puts on its article, `""` for the default.
    pub fn css_class(self) -> &'static str {
        match self {
            Style::Ogma => "md-style-ogma",
            Style::GitHub => "",
            Style::Manuscript => "md-style-manuscript",
            Style::Compact => "md-style-compact",
            Style::Typewriter => "md-style-typewriter",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Canonical paths, most recent first, at most [`RECENT_MAX`].
    pub recent: Vec<PathBuf>,
    /// A theme id from `themes.css`; empty follows the system.
    pub theme: String,
    pub style: Style,
    /// What a markdown tab opens in.
    pub mode: Mode,
    /// Outline-style heading and list numbers.
    pub numbered: bool,
    /// The reading column's width in px; `None` fills the pane.
    pub reading_width: Option<f32>,
    /// Body text size in px, before the style's own scaling.
    pub font_size: f32,
    pub sidebar_open: bool,
    pub sidebar_width: f32,
    /// Load documents' images from the web without asking.
    pub remote_images: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            recent: Vec::new(),
            theme: String::new(),
            style: Style::default(),
            mode: Mode::default(),
            numbered: false,
            reading_width: Some(760.0),
            font_size: 16.0,
            sidebar_open: true,
            sidebar_width: 240.0,
            remote_images: false,
        }
    }
}

/// The v1 file: recent files, the reveal toggle and the theme.
#[derive(Deserialize, Default)]
#[serde(default)]
struct V1 {
    recent: Vec<String>,
    reveal: bool,
    theme: String,
}

impl Prefs {
    /// Where settings live, if the platform has a config directory.
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("mdedit").join("settings.json"))
    }

    fn v1_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("com.alf5.mdedit").join("recent.json"))
    }

    /// The saved settings, or v1's, or the defaults.
    pub fn load() -> Prefs {
        match Self::default_path() {
            Some(p) if p.exists() => Self::load_from(&p),
            _ => Self::v1_path()
                .and_then(|p| Self::from_v1(&p))
                .unwrap_or_default(),
        }
    }

    pub fn load_from(path: &Path) -> Prefs {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn from_v1(path: &Path) -> Option<Prefs> {
        let text = std::fs::read_to_string(path).ok()?;
        let v1: V1 = serde_json::from_str(&text).ok()?;
        let theme = if crate::themes::is_known(&v1.theme) {
            v1.theme
        } else {
            String::new()
        };
        Some(Prefs {
            recent: v1.recent.into_iter().map(PathBuf::from).collect(),
            theme,
            mode: if v1.reveal {
                Mode::Live
            } else {
                Mode::Rendered
            },
            ..Prefs::default()
        })
    }

    /// Best effort: a read-only or missing config directory must not break
    /// anything.
    pub fn save(&self) {
        if let Some(p) = Self::default_path() {
            self.save_to(&p);
        }
    }

    pub fn save_to(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, text);
        }
    }

    /// Move to front, dedup by canonical path, cap at [`RECENT_MAX`]. In
    /// order, so the *last* path of a batch ends up most recent — which is
    /// also the tab left focused after opening them.
    pub fn push_recent<'a>(&mut self, paths: impl IntoIterator<Item = &'a Path>) {
        for path in paths {
            let key = crate::project::canonical(path);
            self.recent.retain(|p| p != &key);
            self.recent.insert(0, key);
        }
        self.recent.truncate(RECENT_MAX);
    }

    pub fn forget_recent(&mut self, path: &Path) {
        self.recent.retain(|p| p != path);
    }

    /// The rendered mode to return to from Source: the default, unless
    /// the default *is* Source.
    pub fn mode_or_rendered(&self) -> Mode {
        if self.mode == Mode::Source {
            Mode::Rendered
        } else {
            self.mode
        }
    }

    /// The theme, if it still exists; an unknown one follows the system.
    pub fn theme(&self) -> &str {
        if crate::themes::is_known(&self.theme) {
            &self.theme
        } else {
            ""
        }
    }
}

/// Keep the tail: the last path components identify a file far better than
/// the root does.
pub fn elide_start(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    format!(
        "…{}",
        chars[chars.len() - max + 1..].iter().collect::<String>()
    )
}

/// A recent file as (name, folder): the folder with the home directory as
/// `~` and its start elided, so five `README.md`s stay distinguishable.
pub fn recent_parts(path: &Path, max_dir: usize) -> (String, String) {
    let name = crate::project::file_name_of(path);
    let mut dir = path
        .parent()
        .map(|d| d.display().to_string())
        .unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        let home = home.display().to_string();
        if !home.is_empty() && dir.starts_with(&home) {
            dir = format!("~{}", &dir[home.len()..]);
        }
    }
    (name, elide_start(&dir, max_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mdedit-prefs-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trips_and_tolerates_junk() {
        let dir = scratch("rt");
        let path = dir.join("settings.json");
        let prefs = Prefs {
            theme: "dark-hard".into(),
            mode: Mode::Live,
            style: Style::Manuscript,
            numbered: true,
            ..Prefs::default()
        };
        prefs.save_to(&path);
        assert_eq!(Prefs::load_from(&path), prefs);

        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(Prefs::load_from(&path), Prefs::default());
        // A file from a version with fewer (or other) fields still loads.
        std::fs::write(&path, r#"{"theme":"apple-dark","later":1}"#).unwrap();
        let p = Prefs::load_from(&path);
        assert_eq!(p.theme, "apple-dark");
        assert_eq!(p.font_size, Prefs::default().font_size);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrates_v1() {
        let dir = scratch("v1");
        let path = dir.join("recent.json");
        std::fs::write(
            &path,
            r#"{"recent":["/a/x.md","/b/y.md"],"reveal":true,"theme":"apple-dark"}"#,
        )
        .unwrap();
        let p = Prefs::from_v1(&path).unwrap();
        assert_eq!(
            p.recent,
            vec![PathBuf::from("/a/x.md"), PathBuf::from("/b/y.md")]
        );
        assert_eq!(p.mode, Mode::Live);
        assert_eq!(p.theme, "apple-dark");

        std::fs::write(&path, r#"{"theme":"gone-theme"}"#).unwrap();
        let p = Prefs::from_v1(&path).unwrap();
        assert_eq!(p.theme, "");
        assert_eq!(p.mode, Mode::Rendered);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recent_moves_to_front_and_caps() {
        let mut p = Prefs::default();
        let paths: Vec<PathBuf> = (0..12)
            .map(|i| PathBuf::from(format!("/nope/{i}.md")))
            .collect();
        p.push_recent(paths.iter().map(PathBuf::as_path));
        assert_eq!(p.recent.len(), RECENT_MAX);
        assert_eq!(p.recent[0], PathBuf::from("/nope/11.md"));
        p.push_recent([Path::new("/nope/5.md")]);
        assert_eq!(p.recent[0], PathBuf::from("/nope/5.md"));
        assert_eq!(p.recent.iter().filter(|r| r.ends_with("5.md")).count(), 1);
        p.forget_recent(Path::new("/nope/5.md"));
        assert!(!p.recent.iter().any(|r| r.ends_with("5.md")));
    }

    #[test]
    fn elides_from_the_start() {
        assert_eq!(elide_start("abc", 5), "abc");
        assert_eq!(elide_start("abcdefgh", 5), "…efgh");
        let (name, dir) = recent_parts(Path::new("/x/y/notes.md"), 40);
        assert_eq!(name, "notes.md");
        assert_eq!(dir, "/x/y");
    }
}
