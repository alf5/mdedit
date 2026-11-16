//! The colour themes in `themes.css`: their names, for the theme picker, and
//! their palettes, read out of the stylesheet itself so the app and an HTML
//! export can never disagree about a colour.
//!
//! `FAMILIES` was generated alongside `themes.css` from ZenNotes-rs (MIT);
//! don't edit by hand. Grouped by family in ZenNotes' order.

use std::collections::HashMap;

const THEMES_CSS: &str = include_str!("../../../themes.css");
const DOC_CSS: &str = include_str!("../../../document.css");

/// (family, [(theme id, menu label)]).
pub const FAMILIES: &[(&str, &[(&str, &str)])] = &[
    ("Apple", &[("apple-light", "Light"), ("apple-dark", "Dark")]),
    (
        "Gruvbox",
        &[
            ("light-hard", "Light · Hard"),
            ("light-medium", "Light · Medium"),
            ("light-soft", "Light · Soft"),
            ("dark-hard", "Dark · Hard"),
            ("dark-medium", "Dark · Medium"),
            ("dark-soft", "Dark · Soft"),
        ],
    ),
    (
        "Catppuccin",
        &[
            ("catppuccin-latte", "Latte"),
            ("catppuccin-frappe", "Frappé"),
            ("catppuccin-macchiato", "Macchiato"),
            ("catppuccin-mocha", "Mocha"),
        ],
    ),
    (
        "GitHub",
        &[
            ("github-light", "Light"),
            ("github-light-high-contrast", "Light · High Contrast"),
            ("github-dark", "Dark"),
            ("github-dark-dimmed", "Dark · Dimmed"),
            ("github-dark-high-contrast", "Dark · High Contrast"),
        ],
    ),
    (
        "Solarized",
        &[("solarized-light", "Light"), ("solarized-dark", "Dark")],
    ),
    ("One", &[("one-light", "Light"), ("one-dark", "Dark")]),
    ("Nord", &[("nord-light", "Light"), ("nord-dark", "Dark")]),
    (
        "Tokyo Night",
        &[("tokyo-night-day", "Day"), ("tokyo-night-storm", "Storm")],
    ),
    (
        "Kanagawa",
        &[
            ("kanagawa-wave", "Wave"),
            ("kanagawa-dragon", "Dragon"),
            ("kanagawa-paper-ink", "Paper Ink (Custom)"),
            ("kanagawa-lotus", "Lotus"),
        ],
    ),
    (
        "Black Metal",
        &[("black-metal", "Black"), ("black-metal-day", "Day")],
    ),
    (
        "Rosé Pine",
        &[
            ("rose-pine-main", "Rosé Pine"),
            ("rose-pine-moon", "Moon"),
            ("rose-pine-dawn", "Dawn"),
        ],
    ),
];

/// Is this a theme `themes.css` defines? Empty means none: the system's.
pub fn is_known(id: &str) -> bool {
    FAMILIES
        .iter()
        .any(|(_, items)| items.iter().any(|(t, _)| *t == id))
}

/// An sRGB colour with alpha.
pub type Rgba = [u8; 4];

/// One theme's colours: the tokens every theme in `themes.css` sets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    pub dark: bool,
    pub bg: Rgba,
    pub bg_soft: Rgba,
    pub bg_raised: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub text_muted: Rgba,
    pub accent: Rgba,
    pub accent_text: Rgba,
    pub code_bg: Rgba,
    pub quote_border: Rgba,
    pub danger: Rgba,
    pub alert_note: Rgba,
    pub alert_tip: Rgba,
    pub alert_important: Rgba,
    pub alert_warning: Rgba,
    pub alert_caution: Rgba,
    /// The second surface and the firmer edge diagrams draw shapes with.
    pub diagram_surface_alt: Rgba,
    pub diagram_border: Rgba,
}

/// `#rgb`, `#rrggbb` or `#rrggbbaa`.
pub fn parse_hex(s: &str) -> Option<Rgba> {
    let hex = s.trim().strip_prefix('#')?;
    let digit = |i: usize, len: usize| u8::from_str_radix(hex.get(i..i + len)?, 16).ok();
    match hex.len() {
        3 => {
            let d = |i| digit(i, 1).map(|v| v * 17);
            Some([d(0)?, d(1)?, d(2)?, 255])
        }
        6 => Some([digit(0, 2)?, digit(2, 2)?, digit(4, 2)?, 255]),
        8 => Some([digit(0, 2)?, digit(2, 2)?, digit(4, 2)?, digit(6, 2)?]),
        _ => None,
    }
}

/// `--name: value;` declarations inside one `{ … }` block, plus
/// `color-scheme`. Values are kept as written.
fn declarations(block: &str) -> HashMap<&str, &str> {
    block
        .split(';')
        .filter_map(|d| {
            let (k, v) = d.split_once(':')?;
            Some((k.trim(), v.trim()))
        })
        .collect()
}

/// The body of the first `{ … }` after `selector` in `css`.
fn block_after<'a>(css: &'a str, selector: &str) -> Option<&'a str> {
    let start = css.find(selector)? + selector.len();
    let open = start + css[start..].find('{')? + 1;
    let close = open + css[open..].find('}')?;
    Some(&css[open..close])
}

/// GitHub's alert colours: what `document.css` falls back to when the palette
/// has none, in light and dark alike.
const GITHUB_ALERTS: [&str; 5] = ["#0969da", "#1a7f37", "#8250df", "#9a6700", "#d1242f"];

fn palette_from(decls: &HashMap<&str, &str>, base: Option<&Palette>) -> Option<Palette> {
    let get = |name: &str, fallback: Option<Rgba>| {
        decls.get(name).and_then(|v| parse_hex(v)).or(fallback)
    };
    let alerts = GITHUB_ALERTS.map(|h| parse_hex(h).unwrap());
    let dark = match decls.get("color-scheme") {
        Some(v) => v.trim() == "dark",
        None => base.is_some_and(|b| b.dark),
    };
    Some(Palette {
        dark,
        bg: get("--bg", base.map(|b| b.bg))?,
        bg_soft: get("--bg-soft", base.map(|b| b.bg_soft))?,
        bg_raised: get("--bg-raised", base.map(|b| b.bg_raised))?,
        border: get("--border", base.map(|b| b.border))?,
        text: get("--text", base.map(|b| b.text))?,
        text_muted: get("--text-muted", base.map(|b| b.text_muted))?,
        accent: get("--accent", base.map(|b| b.accent))?,
        accent_text: get("--accent-text", base.map(|b| b.accent_text))?,
        code_bg: get("--code-bg", base.map(|b| b.code_bg))?,
        quote_border: get("--quote-border", base.map(|b| b.quote_border))?,
        danger: get("--danger", base.map(|b| b.danger))?,
        alert_note: get("--alert-note", Some(alerts[0]))?,
        alert_tip: get("--alert-tip", Some(alerts[1]))?,
        alert_important: get("--alert-important", Some(alerts[2]))?,
        alert_warning: get("--alert-warning", Some(alerts[3]))?,
        alert_caution: get("--alert-caution", Some(alerts[4]))?,
        diagram_surface_alt: get("--bezel-surface-alt", base.map(|b| b.diagram_surface_alt))?,
        diagram_border: get("--bezel-border", base.map(|b| b.diagram_border))?,
    })
}

/// A theme's palette, by id. `None` for an unknown id.
pub fn palette(id: &str) -> Option<Palette> {
    if !is_known(id) {
        return None;
    }
    let block = block_after(THEMES_CSS, &format!(":root[data-theme=\"{id}\"]"))?;
    palette_from(&declarations(block), None)
}

/// The default palette, GitHub's, which follows the system's light or dark
/// setting: `document.css`'s own `:root` and its dark-scheme override.
pub fn system_palette(dark: bool) -> Palette {
    let light = block_after(DOC_CSS, ":root")
        .and_then(|b| palette_from(&declarations(b), None))
        .map(|mut p| {
            p.dark = false;
            p
        })
        .expect("document.css defines the default palette");
    if !dark {
        return light;
    }
    let media = DOC_CSS
        .find("@media (prefers-color-scheme: dark)")
        .expect("document.css has a dark scheme");
    block_after(&DOC_CSS[media..], ":root")
        .and_then(|b| palette_from(&declarations(b), Some(&light)))
        .map(|mut p| {
            p.dark = true;
            p
        })
        .expect("document.css's dark scheme parses")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_is_in_the_stylesheet() {
        let css = THEMES_CSS;
        let mut count = 0;
        for (_, items) in FAMILIES {
            for (id, _) in *items {
                count += 1;
                let selector = format!(":root[data-theme=\"{id}\"]");
                assert!(css.contains(&selector), "{id}");
            }
        }
        assert_eq!(count, 34);
        assert!(is_known("dark-hard"));
        assert!(!is_known("nope"));
        assert!(!is_known(""));
    }

    #[test]
    fn every_theme_has_a_full_palette() {
        for (_, items) in FAMILIES {
            for (id, _) in *items {
                let p = palette(id).unwrap_or_else(|| panic!("{id}"));
                assert_ne!(p.bg, p.text, "{id}");
            }
        }
        let gruvbox = palette("dark-hard").unwrap();
        assert!(gruvbox.dark);
        assert_eq!(gruvbox.bg, [0x1d, 0x20, 0x21, 255]);
        assert!(!palette("apple-light").unwrap().dark);
        assert_eq!(palette("nope"), None);
    }

    #[test]
    fn the_system_palette_follows_the_scheme() {
        let light = system_palette(false);
        let dark = system_palette(true);
        assert!(!light.dark && dark.dark);
        assert_eq!(light.bg, [255, 255, 255, 255]);
        assert_eq!(dark.bg, [0x0d, 0x11, 0x17, 255]);
        // Alerts fall back to GitHub's in both, as document.css does.
        assert_eq!(dark.alert_note, light.alert_note);
    }

    #[test]
    fn hex_colours_parse() {
        assert_eq!(parse_hex("#fff"), Some([255, 255, 255, 255]));
        assert_eq!(parse_hex("#0969da"), Some([9, 105, 218, 255]));
        assert_eq!(parse_hex("#00000080"), Some([0, 0, 0, 128]));
        assert_eq!(parse_hex("var(--bg)"), None);
        assert_eq!(parse_hex("#12"), None);
    }
}
