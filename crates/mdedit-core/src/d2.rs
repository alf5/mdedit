//! D2 diagrams (```` ```d2 ````), drawn in pure Rust by d2-render's native
//! backend: its layered layout and themes, most shapes and styles. What it
//! can't draw is reported as a warning and skipped, not an error.

use crate::mermaid::{Diagram, DrawOptions, outline_text};

/// D2's own themes: Neutral Default on a light page, Dark Mauve on a dark.
fn theme(dark: bool) -> i64 {
    if dark { 200 } else { 0 }
}

fn render(src: &str, dark: bool) -> Result<String, String> {
    let options = d2_render::RenderOptions::new().theme(theme(dark)).pad(12);
    d2_render::native::render_svg(src, &options)
        .map(|out| out.svg)
        .map_err(|e| match e {
            d2_render::Error::Syntax { diagnostics, .. } => diagnostics
                .iter()
                .map(|d| match d.line {
                    Some(line) => format!("line {line}: {}", d.message),
                    None => d.message.clone(),
                })
                .collect::<Vec<_>>()
                .join("; "),
            e => e.to_string(),
        })
}

/// Draw one fence body for the app: text outlined in the app's fonts, as
/// Mermaid's are (see `mermaid`).
pub fn draw(src: &str, dark: bool, opts: &DrawOptions) -> Result<Diagram, String> {
    let svg = outline_text(&render(src, dark)?, &opts.font_family, &opts.fontdb)?;
    let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).map_err(|e| e.to_string())?;
    let size = tree.size();
    Ok(Diagram {
        svg,
        width: size.width() as f64,
        height: size.height() as f64,
    })
}

/// An exported page's drawing, with live text; a failure keeps the source.
pub fn render_block(src: &str) -> String {
    let attr = crate::mermaid::escape_attr(src);
    match render(src, false) {
        Ok(svg) => format!("<div class=\"d2-diagram\" data-d2=\"{attr}\">\n{svg}\n</div>"),
        Err(e) => format!(
            "<div class=\"d2-diagram mermaid-error\" data-d2=\"{attr}\">\n<p class=\"mermaid-error-msg\">{}</p>\
             <pre><code class=\"language-d2\">{}</code></pre>\n</div>",
            crate::mermaid::escape_text(&e),
            crate::mermaid::escape_text(src)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diagram_draws_and_a_bad_one_says_where() {
        let svg = render("a -> b: hello\nb -> c\n", false).unwrap();
        assert!(svg.contains("<svg"), "{svg}");
        let err = render("a: {\n", false).unwrap_err();
        assert!(err.contains("line"), "{err}");
        assert!(render_block("x -> y").contains("data-d2=\"x -&gt; y\""));
    }
}
