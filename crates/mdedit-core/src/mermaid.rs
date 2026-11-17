//! Mermaid diagrams, drawn by bezel-mermaid — a pure-Rust port of Pele
//! (github.com/obsidianmd/pele) — so no JavaScript runs anywhere.
//!
//! Two shapes of output, for two consumers:
//!
//! * [`draw`] is for the app and the PDF. Colours are written into the SVG,
//!   and the label text is turned into outlines against the app's own fonts,
//!   because GPUI's SVG renderer (usvg) ignores a font named in a `style`
//!   attribute and falls back to "Times New Roman" — which, where it isn't
//!   installed, can resolve to a symbol font and draw Latin labels as Greek.
//!   Outlines also make a drawing look the same in the editor and in a PDF.
//! * [`render_block`] is for HTML export. The SVG keeps its
//!   `var(--bezel-*)` colours and live text, so an exported page re-colors
//!   with the reader's light/dark setting and its labels stay selectable.
//!   The wrapper carries the source in `data-mermaid`, so Import HTML can
//!   bring the fence back.

use std::sync::Arc;

pub use pele::Colors;
pub use usvg::fontdb;

/// Class on the export wrapper div.
pub const WRAPPER_CLASS: &str = "mermaid-diagram";
/// Attribute holding the verbatim fence body.
pub const SOURCE_ATTR: &str = "data-mermaid";

/// The faces an exported diagram's labels are measured and drawn in; they
/// match `--md-*` in `document.css`.
const FONT: &str =
    "-apple-system, BlinkMacSystemFont, \"Segoe UI\", \"Noto Sans\", Helvetica, Arial, sans-serif";
const FONT_MONO: &str = "ui-monospace, SFMono-Regular, \"SF Mono\", Menlo, Consolas, monospace";

/// A diagram drawn for the app: colours resolved, text as outlines.
#[derive(Clone, Debug, PartialEq)]
pub struct Diagram {
    pub svg: String,
    pub width: f64,
    pub height: f64,
}

/// What [`draw`] needs from the app.
#[derive(Clone)]
pub struct DrawOptions {
    pub colors: Colors,
    /// The family labels are measured and outlined in. It has to be in
    /// `fontdb`.
    pub font_family: String,
    pub font_family_mono: String,
    pub fontdb: Arc<fontdb::Database>,
}

/// Draw one fence body for the app. `Err` carries Pele's message, which
/// already says where the source went wrong.
pub fn draw(src: &str, opts: &DrawOptions) -> Result<Diagram, String> {
    let options = pele::Options {
        colors: Some(opts.colors.clone()),
        font_family: Some(opts.font_family.clone()),
        font_family_mono: Some(opts.font_family_mono.clone()),
        ..Default::default()
    };
    let drawn = pele::render(src, &options, &pele::Metrics).map_err(|e| e.to_string())?;
    let svg = outline_text(&drawn.svg, &opts.font_family, &opts.fontdb)?;
    Ok(Diagram {
        svg,
        width: drawn.width,
        height: drawn.height,
    })
}

/// Resolve every `<text>` against `fontdb` and write it back as paths.
pub(crate) fn outline_text(
    svg: &str,
    family: &str,
    fontdb: &Arc<fontdb::Database>,
) -> Result<String, String> {
    let options = usvg::Options {
        fontdb: fontdb.clone(),
        font_family: family.to_string(),
        ..Default::default()
    };
    let tree = usvg::Tree::from_str(svg, &options).map_err(|e| e.to_string())?;
    Ok(tree.to_string(&usvg::WriteOptions::default()))
}

/// Draw one diagram for an exported page.
fn render_svg(src: &str) -> Result<String, String> {
    let options = pele::Options {
        font_family: Some(FONT.to_string()),
        font_family_mono: Some(FONT_MONO.to_string()),
        ..Default::default()
    };
    pele::render(src, &options, &pele::Metrics)
        .map(|d| d.svg)
        .map_err(|e| e.to_string())
}

/// Render one fence body to the export wrapper div. A diagram that fails to
/// parse keeps its source visible with the error above it, rather than
/// vanishing.
pub fn render_block(src: &str) -> String {
    wrap(src, render_svg(src))
}

/// The wrapper around a drawing, or around the reason there isn't one.
///
/// The newlines just inside it draw nothing, but they keep the block's text
/// apart from its neighbours' when the page is read as text.
fn wrap(src: &str, drawn: Result<String, String>) -> String {
    let attr = escape_attr(src);
    match drawn {
        Ok(svg) => {
            format!("<div class=\"{WRAPPER_CLASS}\" {SOURCE_ATTR}=\"{attr}\">\n{svg}\n</div>")
        }
        Err(e) => format!(
            "<div class=\"{WRAPPER_CLASS} mermaid-error\" {SOURCE_ATTR}=\"{attr}\">\n<p class=\"mermaid-error-msg\">{}</p>\
             <pre><code class=\"language-mermaid\">{}</code></pre>\n</div>",
            escape_text(&e),
            escape_text(src)
        ),
    }
}

pub(crate) fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escape for an attribute value. Newlines become `&#10;` so the source
/// survives any HTML re-serialization on its way back in through Import HTML.
pub(crate) fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#10;"),
            '\r' => {}
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drawing_is_wrapped_with_its_source() {
        let html = wrap("graph TD\n  A --> B\n", Ok("<svg></svg>".to_string()));
        assert!(html.contains("<svg></svg>"));
        assert!(html.contains(WRAPPER_CLASS));
        assert!(!html.contains("contenteditable"));
        assert!(!html.contains("mermaid-error"));
    }

    #[test]
    fn an_export_keeps_css_variables_and_live_text() {
        let html = render_block("flowchart LR\n  A[Write] --> B[Read]\n");
        assert!(!html.contains("mermaid-error"), "{html}");
        assert!(html.contains("var(--bezel-"));
        assert!(html.contains(">Write<"));
    }

    fn colors() -> Colors {
        let c = [10, 20, 30, 255];
        Colors {
            bg: [255, 255, 255, 255],
            fg: c,
            muted: c,
            line: c,
            surface: c,
            surface_alt: c,
            border: c,
            accent: c,
            critical: c,
            series: [c; 8],
        }
    }

    /// The app's drawing has no text left for a renderer to get wrong, and
    /// no CSS variables for one that can't resolve them.
    #[test]
    fn the_app_drawing_is_outlined_and_resolved() {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let opts = DrawOptions {
            colors: colors(),
            font_family: "sans-serif".into(),
            font_family_mono: "monospace".into(),
            fontdb: Arc::new(db),
        };
        let d = draw("flowchart LR\n  A[Write] --> B[Read]\n", &opts).unwrap();
        assert!(d.width > 0.0 && d.height > 0.0);
        assert!(d.svg.starts_with("<svg"));
        assert!(!d.svg.contains("<text"));
        assert!(!d.svg.contains("var("));
    }

    #[test]
    fn a_bad_source_is_an_error_not_a_blank() {
        let opts = DrawOptions {
            colors: colors(),
            font_family: "sans-serif".into(),
            font_family_mono: "monospace".into(),
            fontdb: Arc::new(fontdb::Database::new()),
        };
        assert!(draw("not a diagram at all", &opts).is_err());
    }

    #[test]
    fn keeps_the_source_in_the_data_attribute() {
        let html = render_block("graph LR\n  A[\"x & y\"] --> B\n");
        assert!(
            html.contains("data-mermaid=\"graph LR&#10;  A[&quot;x &amp; y&quot;] --&gt; B&#10;\"")
        );
    }

    #[test]
    fn a_failure_degrades_to_an_error_block() {
        let html = wrap(
            "not a diagram at all\n",
            Err("No diagram type detected for the <given> text.".to_string()),
        );
        assert!(html.contains("mermaid-error"));
        // The reason is shown, escaped, and the source is still recoverable
        // from the attribute.
        assert!(html.contains("for the &lt;given&gt; text."));
        assert!(html.contains("data-mermaid=\"not a diagram at all&#10;\""));
        assert!(html.contains("<code class=\"language-mermaid\">not a diagram at all\n</code>"));
    }
}
