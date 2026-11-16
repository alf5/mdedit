//! How a document is typeset: the five styles from `document.css` (GitHub,
//! Ogma, Manuscript, Compact, Typewriter), turned into fonts, sizes and
//! colours for the editor.

use gpui_kit::*;
use mdedit_core::prefs::{Mode, Prefs, Style};
use mdedit_core::syntax::{Alert, LineKind};
use mdedit_core::themes::{Palette, Rgba};

use crate::theme::{MONO_FONT, SERIF_FONT, UI_FONT};

/// The document's colours. They follow ZenNotes' editor, which every theme
/// here was made from; its token names are in brackets.
#[derive(Clone, Debug, PartialEq)]
pub struct Colors {
    /// The page (bg).
    pub bg: Hsla,
    pub text: Hsla,
    /// Keys, bullets, a code block's language (grey-1).
    pub muted: Hsla,
    /// Quotes, comments, punctuation (fg-2).
    pub soft: Hsla,
    /// Faint edges and guides (grey-0).
    pub faint: Hsla,
    pub accent: Hsla,
    /// List numbers and markup shown at the caret (accent-soft).
    pub accent_soft: Hsla,
    /// What goes on an accent fill.
    pub accent_text: Hsla,
    /// The first and second surfaces above the page (bg-1, bg-2).
    pub surface: Hsla,
    pub surface_alt: Hsla,
    /// Inline code's chip.
    pub code_bg: Hsla,
    /// A quote's bar.
    pub quote: Hsla,
    /// Rules and edges (bg-3).
    pub border: Hsla,
    pub selection: Hsla,
    pub find: Hsla,
    pub find_current: Hsla,
    pub danger: Hsla,
    /// Blue, green, purple, yellow and red: the alerts' colours, and the
    /// highlighter's.
    pub alerts: [Hsla; 5],
    pub aqua: Hsla,
}

impl Colors {
    /// A callout's colour, by its group.
    pub fn callout(&self, alert: Alert) -> Hsla {
        match alert {
            Alert::Note => self.alerts[0],
            Alert::Tip => self.alerts[1],
            Alert::Question => self.alerts[2],
            Alert::Warning => self.alerts[3],
            Alert::Danger => self.alerts[4],
            Alert::Quote => self.faint,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DocStyle {
    pub font: SharedString,
    pub mono: SharedString,
    pub size: f32,
    /// Line height as a multiple of the font size.
    pub leading: f32,
    /// The reading column; `None` fills the pane.
    pub width: Option<f32>,
    pub muted_body: bool,
    /// Outline numbers on headings.
    pub numbered: bool,
    pub colors: Colors,
    /// The theme's palette as is, for what's drawn outside GPUI (diagrams,
    /// formulas).
    pub palette: Palette,
}

pub fn hsla(c: Rgba) -> Hsla {
    gpui_kit::Rgba {
        r: c[0] as f32 / 255.,
        g: c[1] as f32 / 255.,
        b: c[2] as f32 / 255.,
        a: c[3] as f32 / 255.,
    }
    .into()
}

impl DocStyle {
    pub fn new(prefs: &Prefs, p: &Palette) -> Self {
        let base = prefs.font_size;
        let (font, size, leading, width, muted_body): (&str, f32, f32, f32, bool) =
            match prefs.style {
                Style::GitHub => (UI_FONT, base, 1.6, 860., false),
                Style::Ogma => (UI_FONT, base, 1.65, 720., true),
                Style::Manuscript => (SERIF_FONT, base + 2., 1.75, 640., false),
                Style::Compact => (UI_FONT, base - 2., 1.35, 960., false),
                // ZenNotes' own: monospace at 16px and 1.7, in a 920px
                // column less its padding.
                Style::Typewriter => (MONO_FONT, base, 1.7, 856., false),
            };
        // Each style has its own column; the setting only chooses between a
        // column and the full pane.
        let width = prefs.reading_width.map(|_| width);
        let accent = hsla(p.accent);
        let bg = hsla(p.bg);
        let surface_alt = hsla(p.diagram_surface_alt);
        DocStyle {
            font: font.into(),
            mono: MONO_FONT.into(),
            size,
            leading,
            width,
            muted_body,
            numbered: prefs.numbered,
            colors: Colors {
                bg,
                text: hsla(p.text),
                muted: hsla(p.text_muted),
                soft: hsla(p.text_soft),
                faint: hsla(p.text_faint),
                accent,
                accent_soft: hsla(p.accent_soft),
                accent_text: hsla(p.accent_text),
                surface: hsla(p.bg_soft),
                surface_alt,
                code_bg: surface_alt,
                quote: accent.opacity(0.45),
                border: hsla(p.border),
                selection: accent.opacity(0.22),
                find: hsla(p.alert_warning).opacity(0.30),
                find_current: hsla(p.alert_warning).opacity(0.65),
                danger: hsla(p.danger),
                alerts: [
                    hsla(p.alert_note),
                    hsla(p.alert_tip),
                    hsla(p.alert_important),
                    hsla(p.alert_warning),
                    hsla(p.alert_caution),
                ],
                aqua: hsla(p.aqua),
            },
            palette: p.clone(),
        }
    }

    /// Font size and line height for a line, in pixels. ZenNotes' scale:
    /// headings keep the body's leading, and code its line height.
    pub fn metrics(&self, kind: LineKind, mode: Mode) -> (f32, f32) {
        if mode == Mode::Source {
            let s = self.size * 0.92;
            return (s, s * 1.55);
        }
        let body = self.size * self.leading;
        match kind {
            LineKind::Heading(l) => {
                let s = self.size
                    * match l {
                        1 => 2.0,
                        2 => 1.6,
                        3 => 1.3,
                        4 => 1.15,
                        5 => 1.0,
                        _ => 0.92,
                    };
                (s, s * self.leading)
            }
            LineKind::Code | LineKind::FenceOpen | LineKind::FenceClose => (self.size * 0.92, body),
            LineKind::Table | LineKind::Html => (self.size * 0.92, self.size * 0.92 * 1.5),
            // The frontmatter's card: small, and airy.
            LineKind::Meta => {
                let s = self.size * 0.84;
                (s, s * 1.85)
            }
            _ => (self.size, body),
        }
    }

    /// Whether the line is set in the code face.
    pub fn monospace(&self, kind: LineKind, mode: Mode) -> bool {
        mode == Mode::Source
            || matches!(
                kind,
                LineKind::Code
                    | LineKind::Table
                    | LineKind::Html
                    | LineKind::FenceOpen
                    | LineKind::FenceClose
            )
    }
}
