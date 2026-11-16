//! Math: `$…$` and `$$…$$`.
//!
//! Two renderers, for two consumers:
//!
//! * [`draw`] typesets with latex-rust into an SVG whose glyphs are paths
//!   (STIX Two Math outlines, bundled in the crate), for the app and the PDF.
//!   No fonts to install, nothing to fall back to.
//! * [`render`] writes MathML for HTML export, which every current browser
//!   draws natively. The source rides along in `data-math`, so Import HTML
//!   gets back exactly what was typed.

use std::cell::OnceCell;

use math_core::{LatexToMathML, MathCoreConfig, MathDisplay};

use crate::mermaid::{escape_attr, escape_text};

/// Class on every exported formula; Import HTML's hook.
pub const CLASS: &str = "math";
/// Attribute holding the verbatim source, without its `$` delimiters.
pub const SOURCE_ATTR: &str = "data-math";
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
        Ok(mathml) => format!("<span class=\"{classes}\" {SOURCE_ATTR}=\"{attr}\">{mathml}</span>"),
        Err(e) => format!(
            "<span class=\"{classes} math-error\" \
             {SOURCE_ATTR}=\"{attr}\" title=\"{}\"><code>{}</code></span>",
            escape_attr(&e),
            escape_text(src)
        ),
    }
}

/// A formula typeset for the app or the PDF. Sizes are in points, measured
/// down from the image's top edge for the baseline.
#[derive(Clone, Debug, PartialEq)]
pub struct MathDrawing {
    pub svg: String,
    pub width_pt: f64,
    pub height_pt: f64,
    pub baseline_pt: f64,
}

thread_local! {
    /// Parsing the bundled STIX Two Math tables isn't free; once per thread.
    static FONT: OnceCell<Option<latex_rust::MathFont>> = const { OnceCell::new() };
}

fn dim_f64(d: &latex_rust::Dim) -> f64 {
    d.as_ratio()
        .map(|(n, den)| n as f64 / den as f64)
        .unwrap_or(0.0)
}

/// Typeset `src` (without its `$` delimiters) at `size_pt` in `rgb`.
pub fn draw(src: &str, display: bool, size_pt: f64, rgb: [u8; 3]) -> Result<MathDrawing, String> {
    use latex_rust::{Color, Dim, MathStyle, SvgOptions};
    FONT.with(|cell| {
        let font = cell
            .get_or_init(|| latex_rust::MathFont::stix_two_math().ok())
            .as_ref()
            .ok_or_else(|| "The math font could not be loaded.".to_string())?;
        let ast = latex_rust::parse(src.trim()).map_err(|e| e.to_string())?;
        let style = if display {
            MathStyle::Display
        } else {
            MathStyle::Text
        };
        let size = Dim::ratio((size_pt * 100.0).round() as i64, 100);
        let tree = latex_rust::layout_with_em_size_pt(&ast, font, style, &size)
            .map_err(|e| e.to_string())?;
        let options = SvgOptions {
            font_size_pt: size,
            color: Color::rgb(rgb[0], rgb[1], rgb[2]),
            display,
        };
        let image = latex_rust::render_svg_with_metrics(&tree, font, &options)
            .map_err(|e| e.to_string())?;
        Ok(MathDrawing {
            svg: image.svg,
            width_pt: dim_f64(&image.width_pt),
            height_pt: dim_f64(&image.height_pt),
            baseline_pt: dim_f64(&image.baseline_pt),
        })
    })
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

    #[test]
    fn draws_paths_with_a_baseline() {
        let d = draw(r"\frac{1}{2} + x^2", false, 16.0, [0, 0, 0]).unwrap();
        assert!(d.svg.contains("<path"));
        assert!(!d.svg.contains("<text"));
        assert!(d.width_pt > 0.0 && d.height_pt > 0.0);
        assert!(d.baseline_pt > 0.0 && d.baseline_pt <= d.height_pt);
    }

    #[test]
    fn a_bad_formula_is_an_error() {
        assert!(draw(r"\frac{1", false, 16.0, [0, 0, 0]).is_err());
    }
}
