//! Syntax colours for fenced code blocks, by the fence's language.
//!
//! syntect, with two-face's larger set of languages; both already ship
//! inside gpui-kit. A block is highlighted whole (a string literal can span
//! lines) and cached; each of its lines then takes its slice. The colours
//! are the theme's own, given to tokens as ZenNotes gives them.

use std::ops::Range;
use std::sync::OnceLock;

use gpui_kit::*;
use mdedit_core::themes::Palette;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color, FontStyle, StyleModifier, Theme, ThemeItem, ThemeSettings};
use syntect::parsing::SyntaxSet;

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

/// A highlighting theme from a palette: keywords in the accent and bold,
/// strings green, numbers and constants yellow, types purple, functions
/// blue, comments, operators and punctuation in the soft text colour.
pub fn theme(p: &Palette) -> Theme {
    let color = |c: [u8; 4]| Color {
        r: c[0],
        g: c[1],
        b: c[2],
        a: c[3],
    };
    let item = |scopes: &str, c: [u8; 4], font_style: Option<FontStyle>| ThemeItem {
        scope: scopes.parse().expect("a valid scope selector"),
        style: StyleModifier {
            foreground: Some(color(c)),
            background: None,
            font_style,
        },
    };
    let (blue, green, purple, yellow, red) = (
        p.alert_note,
        p.alert_tip,
        p.alert_important,
        p.alert_warning,
        p.alert_caution,
    );
    Theme {
        name: Some("palette".into()),
        author: None,
        settings: ThemeSettings {
            foreground: Some(color(p.text)),
            ..ThemeSettings::default()
        },
        scopes: vec![
            item(
                "keyword, storage, storage.type.function, storage.modifier",
                p.accent,
                Some(FontStyle::BOLD),
            ),
            item("keyword.operator, punctuation", p.text_soft, None),
            item(
                "string, string.regexp, constant.character.escape",
                green,
                None,
            ),
            item(
                "constant.numeric, constant.language, constant.character, variable.language",
                yellow,
                None,
            ),
            item(
                "entity.name.type, entity.name.class, entity.name.namespace, entity.name.struct, \
                 entity.name.enum, support.type, support.class, storage.type, \
                 entity.other.attribute-name",
                purple,
                None,
            ),
            item(
                "entity.name.function, support.function, variable.function, meta.function-call.identifier",
                blue,
                None,
            ),
            item("entity.name.tag", red, None),
            item(
                "meta.annotation, meta.attribute, meta.decorator",
                p.accent_soft,
                Some(FontStyle::ITALIC),
            ),
            item(
                "comment, punctuation.definition.comment",
                p.text_soft,
                Some(FontStyle::ITALIC),
            ),
            item("invalid", red, None),
        ],
    }
}

/// Words that declare something: keywords, whatever scope a grammar gives
/// them.
/// Where the keywords' and types' looks are in [`theme`]'s list.
const KEYWORDS: usize = 0;
const TYPES: usize = 4;

const DECLARING: &[&str] = &[
    "let", "const", "static", "struct", "enum", "union", "impl", "trait", "type", "mod", "fn",
    "def", "class", "var", "val", "func",
];

/// Colour spans for `code` in language `lang` (a fence info word: `rust`,
/// `py`, `json`…), offsets relative to `code`. Empty when the language is
/// unknown.
pub fn highlight(code: &str, lang: &str, theme: &Theme) -> Vec<Span> {
    let ss = syntaxes();
    let Some(syntax) = ss
        .find_syntax_by_token(lang)
        .or_else(|| ss.find_syntax_by_name(lang))
    else {
        return Vec::new();
    };
    let mut h = HighlightLines::new(syntax, theme);
    // The keywords' look, for declaring words some grammars scope as types
    // (Rust's `let` is `storage.type`, as `u32` is).
    let colour = |i: usize| theme.scopes.get(i).and_then(|k| k.style.foreground);
    let (keyword, types) = (colour(KEYWORDS), colour(TYPES));
    let mut out = Vec::new();
    let mut at = 0;
    for line in syntect::util::LinesWithEndings::from(code) {
        let Ok(pieces) = h.highlight_line(line, ss) else {
            break;
        };
        for (style, text) in pieces {
            let declares = types == Some(style.foreground) && DECLARING.contains(&text.trim());
            let c = match keyword {
                Some(k) if declares => k,
                _ => style.foreground,
            };
            out.push(Span {
                range: at..at + text.len(),
                color: gpui_kit::Rgba {
                    r: c.r as f32 / 255.,
                    g: c.g as f32 / 255.,
                    b: c.b as f32 / 255.,
                    a: c.a as f32 / 255.,
                }
                .into(),
                bold: declares || style.font_style.contains(FontStyle::BOLD),
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

#[cfg(test)]
mod tests {
    use super::{Span, highlight, theme};
    use gpui_kit::Hsla;

    fn colour_of(spans: &[Span], code: &str, word: &str) -> (Hsla, bool) {
        let at = code.find(word).unwrap();
        let s = spans.iter().find(|s| s.range.contains(&at)).unwrap();
        (s.color, s.bold)
    }

    #[test]
    fn tokens_take_the_palettes_colours() {
        let p = mdedit_core::themes::palette("dark-medium").unwrap();
        let theme = theme(&p);
        let code = "fn main() { let n: u32 = 42; let s = \"type\"; }";
        let spans = highlight(code, "rust", &theme);
        let hsla = |c: [u8; 4]| crate::editor::style::hsla(c);
        assert_eq!(colour_of(&spans, code, "fn"), (hsla(p.accent), true));
        // `let` is scoped as a type in Rust's grammar, but it's a keyword.
        assert_eq!(colour_of(&spans, code, "let"), (hsla(p.accent), true));
        assert_eq!(colour_of(&spans, code, "u32").0, hsla(p.alert_important));
        assert_eq!(colour_of(&spans, code, "42").0, hsla(p.alert_warning));
        assert_eq!(colour_of(&spans, code, "main").0, hsla(p.alert_note));
        // A declaring word in a string is still the string's.
        assert_eq!(colour_of(&spans, code, "type").0, hsla(p.alert_tip));
    }
}
