//! Math rendering: `$…$` and `$$…$$` become MathML, which the webview draws
//! itself, so there's no JavaScript or font loading involved and exports need
//! nothing extra.
//!
//! Like a diagram, a formula keeps its source on the element in `data-math`.
//! The serializer reads that back, so what was typed survives exactly.

use std::cell::OnceCell;

use math_core::{LatexToMathML, MathCoreConfig, MathDisplay};

use crate::mermaid::{escape_attr, escape_text};

/// Class on every formula; also the serializer's and the editor's hook.
pub const CLASS: &str = "math";
/// Attribute holding the verbatim source, without its `$` delimiters.
pub const SOURCE_ATTR: &str = "data-math";
/// On a formula's open source textarea: which kind it is, `DISPLAY_CLASS` or
/// `INLINE_KIND`. A diagram's textarea doesn't have this.
pub const SOURCE_KIND_ATTR: &str = "data-math-kind";
pub const INLINE_KIND: &str = "math-inline";
/// Class marking a formula that sits on its own line (`$$…$$`).
pub const DISPLAY_CLASS: &str = "math-display";

thread_local! {
    static CONVERTER: OnceCell<Option<LatexToMathML>> = const { OnceCell::new() };
}

fn convert(src: &str, display: bool) -> Result<String, String> {
    let mode = if display {
        MathDisplay::Block
    } else {
        MathDisplay::Inline
    };
    CONVERTER.with(|cell| {
        let converter =
            cell.get_or_init(|| LatexToMathML::new(MathCoreConfig::default(), vec![]).ok());
        let Some(converter) = converter else {
            return Err("The math renderer failed to start".to_string());
        };
        converter
            .convert_with_local_state(src, mode)
            .map(|r| r.mathml)
            .map_err(|e| e.to_string())
    })
}

/// The markup for one formula. It's a `<span>` in both cases, because `$$…$$`
/// arrives inside a paragraph and a `<div>` there would break the paragraph in
/// two when the browser parses it. CSS makes the display form a block.
pub fn render(src: &str, display: bool) -> String {
    let attr = escape_attr(src);
    let mut classes = String::from(CLASS);
    if display {
        classes.push(' ');
        classes.push_str(DISPLAY_CLASS);
    }
    match convert(src.trim(), display) {
        Ok(mathml) => format!(
            "<span class=\"{classes}\" contenteditable=\"false\" {SOURCE_ATTR}=\"{attr}\">{mathml}</span>"
        ),
        Err(e) => format!(
            "<span class=\"{classes} math-error\" contenteditable=\"false\" \
             {SOURCE_ATTR}=\"{attr}\" title=\"{}\"><code>{}</code></span>",
            escape_attr(&e),
            escape_text(src)
        ),
    }
}

/// A formula back to markdown. A display formula may run over several lines,
/// but a blank line or a line that looks like a list or a heading would end
/// the paragraph holding it and so the formula. LaTeX doesn't care about line
/// breaks, so those go.
pub fn to_markdown(src: &str, display: bool) -> String {
    if !display {
        return format!("${}$", src.trim());
    }
    let breaks_paragraph = |line: &str| {
        let t = line.trim_start();
        t.is_empty()
            || t.starts_with('#')
            || t.starts_with('>')
            || t.starts_with("- ")
            || t.starts_with("+ ")
            || t.starts_with("* ")
            || t.chars().next().is_some_and(|c| c.is_ascii_digit())
    };
    let inner = src.trim_matches('\n');
    if inner.lines().any(breaks_paragraph) {
        let joined: Vec<&str> = inner
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        format!("$${}$$", joined.join(" "))
    } else {
        // Exactly as written, so `$$` on its own lines stays that way.
        format!("$${src}$$")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_mathml_and_keeps_the_source() {
        let html = render("a_1 < b", false);
        assert!(html.contains("<math"));
        assert!(html.contains("data-math=\"a_1 &lt; b\""));
        assert!(!html.contains(DISPLAY_CLASS));
    }

    #[test]
    fn display_math_is_marked_and_stays_a_span() {
        let html = render(r"\frac{1}{2}", true);
        assert!(html.starts_with("<span"));
        assert!(html.contains(DISPLAY_CLASS));
        assert!(html.contains("<mfrac"));
    }

    #[test]
    fn a_bad_formula_shows_its_source() {
        let html = render(r"\frac{1", false);
        assert!(html.contains("math-error"));
        assert!(html.contains(r"<code>\frac{1</code>"));
        assert!(html.contains("data-math"));
    }

    #[test]
    fn writes_the_delimiters_back() {
        assert_eq!(to_markdown("a_1", false), "$a_1$");
        assert_eq!(to_markdown("x", true), "$$x$$");
        assert_eq!(to_markdown("\na\n", true), "$$\na\n$$");
        assert_eq!(to_markdown("\na\n\\\\ b\n", true), "$$\na\n\\\\ b\n$$");
    }

    #[test]
    fn lines_that_would_end_the_paragraph_are_joined() {
        assert_eq!(to_markdown("a\n\nb", true), "$$a b$$");
        assert_eq!(to_markdown("a\n- b", true), "$$a - b$$");
        assert_eq!(to_markdown("a\n1 + b", true), "$$a 1 + b$$");
    }
}
