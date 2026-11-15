//! The colour themes in `themes.css`, for the View > Theme menu.
//!
//! Generated alongside `themes.css` from ZenNotes-rs (MIT); don't edit by
//! hand. Grouped by family in ZenNotes' order.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_is_in_the_stylesheet() {
        let css = include_str!("../../themes.css");
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
}
