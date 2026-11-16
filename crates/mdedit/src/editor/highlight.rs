//! Syntax colours for fenced code blocks, by the fence's language.
//!
//! syntect, with two-face's larger set of languages; both already ship
//! inside gpui-kit. A block is highlighted whole (a string literal can span
//! lines) and cached; each of its lines then takes its slice.

use std::ops::Range;
use std::sync::OnceLock;

use gpui_kit::*;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme};
use syntect::parsing::SyntaxSet;
use two_face::theme::{EmbeddedLazyThemeSet, EmbeddedThemeName};

#[derive(Clone, Debug)]
pub struct Span {
    pub range: Range<usize>,
    pub color: Hsla,
    pub bold: bool,
    pub italic: bool,
}

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

fn theme(dark: bool) -> &'static Theme {
    static THEMES: OnceLock<EmbeddedLazyThemeSet> = OnceLock::new();
    let set = THEMES.get_or_init(two_face::theme::extra);
    set.get(if dark {
        EmbeddedThemeName::OneHalfDark
    } else {
        EmbeddedThemeName::OneHalfLight
    })
}

/// Colour spans for `code` in language `lang` (a fence info word: `rust`,
/// `py`, `json`…), offsets relative to `code`. Empty when the language is
/// unknown.
pub fn highlight(code: &str, lang: &str, dark: bool) -> Vec<Span> {
    let ss = syntaxes();
    let Some(syntax) = ss
        .find_syntax_by_token(lang)
        .or_else(|| ss.find_syntax_by_name(lang))
    else {
        return Vec::new();
    };
    let mut h = HighlightLines::new(syntax, theme(dark));
    let mut out = Vec::new();
    let mut at = 0;
    for line in syntect::util::LinesWithEndings::from(code) {
        let Ok(pieces) = h.highlight_line(line, ss) else {
            break;
        };
        for (style, text) in pieces {
            let c = style.foreground;
            out.push(Span {
                range: at..at + text.len(),
                color: gpui_kit::Rgba {
                    r: c.r as f32 / 255.,
                    g: c.g as f32 / 255.,
                    b: c.b as f32 / 255.,
                    a: c.a as f32 / 255.,
                }
                .into(),
                bold: style.font_style.contains(FontStyle::BOLD),
                italic: style.font_style.contains(FontStyle::ITALIC),
            });
            at += text.len();
        }
    }
    out
}

/// `runs` (covering a line of `len` bytes) recoloured by `spans`, which are
/// relative to the line.
pub fn apply(runs: Vec<TextRun>, spans: &[Span], len: usize) -> Vec<TextRun> {
    let Some(base) = runs.first().cloned() else {
        return runs;
    };
    let mut out = Vec::new();
    let mut at = 0;
    for s in spans {
        let start = s.range.start.min(len);
        let end = s.range.end.min(len);
        if end <= at {
            continue;
        }
        if start > at {
            out.push(TextRun {
                len: start - at,
                ..base.clone()
            });
        }
        let mut run = TextRun {
            len: end - start.max(at),
            color: s.color,
            ..base.clone()
        };
        if s.bold {
            run.font.weight = FontWeight::BOLD;
        }
        if s.italic {
            run.font.style = gpui_kit::FontStyle::Italic;
        }
        out.push(run);
        at = end;
    }
    if at < len {
        out.push(TextRun {
            len: len - at,
            ..base
        });
    }
    out.retain(|r| r.len > 0);
    out
}
