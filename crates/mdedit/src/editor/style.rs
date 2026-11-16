//! How a document is typeset: the five styles from `document.css` (GitHub,
//! Ogma, Manuscript, Compact, Typewriter), turned into fonts, sizes and
//! colours for the editor.

use gpui_kit::*;
use mdedit_core::prefs::{Mode, Prefs, Style};
use mdedit_core::syntax::LineKind;
use mdedit_core::themes::{Palette, Rgba};

use crate::theme::{MONO_FONT, SERIF_FONT, UI_FONT};

#[derive(Clone, Debug, PartialEq)]
pub struct Colors {
    pub text: Hsla,
    pub muted: Hsla,
    pub accent: Hsla,
    pub code_bg: Hsla,
    pub quote: Hsla,
    pub border: Hsla,
    pub selection: Hsla,
    pub find: Hsla,
    pub find_current: Hsla,
    pub danger: Hsla,
    pub alerts: [Hsla; 5],
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
                Style::Typewriter => (MONO_FONT, base - 1., 1.6, 760., false),
            };
        // Each style has its own column; the setting only chooses between a
        // column and the full pane.
        let width = prefs.reading_width.map(|_| width);
        let accent = hsla(p.accent);
        DocStyle {
            font: font.into(),
            mono: MONO_FONT.into(),
            size,
            leading,
            width,
            muted_body,
            numbered: prefs.numbered,
            colors: Colors {
                text: hsla(p.text),
                muted: hsla(p.text_muted),
                accent,
                code_bg: hsla(p.code_bg),
                quote: hsla(p.quote_border),
                border: hsla(p.border),
                selection: accent.opacity(0.28),
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
            },
            palette: p.clone(),
        }
    }

    /// Font size and line height for a line, in pixels.
    pub fn metrics(&self, kind: LineKind, mode: Mode) -> (f32, f32) {
        if mode == Mode::Source {
            let s = self.size * 0.92;
            return (s, s * 1.55);
        }
        let scale = match kind {
            LineKind::Heading(1) => 2.0,
            LineKind::Heading(2) => 1.5,
            LineKind::Heading(3) => 1.25,
            LineKind::Heading(4) => 1.05,
            LineKind::Heading(5) => 0.95,
            LineKind::Heading(_) => 0.9,
            LineKind::Code | LineKind::Table | LineKind::Html => 0.88,
            LineKind::FenceOpen | LineKind::FenceClose => 0.78,
            _ => 1.0,
        };
        let s = self.size * scale;
        let leading = match kind {
            LineKind::Heading(_) => 1.3,
            LineKind::Code | LineKind::Table | LineKind::Html => 1.5,
            _ => self.leading,
        };
        (s, s * leading)
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
