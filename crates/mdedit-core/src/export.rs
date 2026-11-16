//! Export a document as a single standalone HTML file.
//!
//! Nothing is fetched at view time: the stylesheet is embedded and every
//! mermaid diagram is already inline SVG (see [`crate::mermaid`]). The file
//! therefore opens identically offline, in any browser, and follows the
//! reader's light/dark preference — diagram colors included, since they
//! resolve through the `--bezel-*` custom properties rather than being baked
//! in. Math is MathML, which every current browser draws natively.
//!
//! The diagrams also keep their `data-mermaid` source attribute, so an
//! exported file fed back through **Import HTML** comes home as
//! ```` ```mermaid ```` fences rather than as unrecoverable SVG.
//!
//! Nothing here knows how to paginate. That is `@media print` at the foot of
//! `document.css`, so an exported `.html` printed from a browser breaks its
//! pages sensibly. The app's own PDF export is typeset separately (Typst).

/// The document stylesheet, verbatim, at the repository root.
const DOC_CSS: &str = include_str!("../../../document.css");
const THEMES_CSS: &str = include_str!("../../../themes.css");

/// One theme's rule out of `themes.css`, so an export carries the palette it
/// was written in without all the others. Empty for an unknown theme, or none.
fn theme_rule(theme: &str) -> &'static str {
    if theme.is_empty() {
        return "";
    }
    let selector = format!(":root[data-theme=\"{theme}\"]");
    let Some(start) = THEMES_CSS.find(&selector) else {
        return "";
    };
    match THEMES_CSS[start..].find('}') {
        Some(end) => &THEMES_CSS[start..start + end + 1],
        None => "",
    }
}

/// What a standalone page needs around `document.css`.
const PAGE_CSS: &str = "\
* {
  box-sizing: border-box;
}

html {
  background: var(--bg);
}

body {
  margin: 0;
  padding: 32px 24px 64px;
  background: var(--bg);
  color: var(--text);
}

/* The page box supplies the margin when printing; see document.css. */
@media print {
  body {
    padding: 0;
  }
}
";

/// The exportable body of a markdown document. pulldown-cmark already writes
/// task checkboxes `disabled`, which is right for a record.
pub fn body_from_markdown(md: &str) -> String {
    crate::markdown::to_html(md)
}

/// A whole standalone page for `md`.
pub fn export_html(title: &str, md: &str, extra_class: &str, theme: &str) -> String {
    build_document(title, &body_from_markdown(md), extra_class, theme)
}

/// Wrap a cleaned body in the standalone page. `extra_class` is appended to
/// `markdown-body` so an export keeps the editor's style and full-width
/// setting; pass `""` for the default sheet.
pub fn build_document(title: &str, body: &str, extra_class: &str, theme: &str) -> String {
    let title = crate::mermaid::escape_text(display_title(title));
    let class = if extra_class.is_empty() {
        "markdown-body".to_string()
    } else {
        format!("markdown-body {extra_class}")
    };
    let rule = theme_rule(theme);
    let html_attrs = if rule.is_empty() {
        String::new()
    } else {
        format!(" data-theme=\"{}\"", crate::mermaid::escape_attr(theme))
    };
    format!(
        "<!doctype html>\n\
         <html lang=\"en\"{html_attrs}>\n\
         <head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"generator\" content=\"mdedit\">\n\
         <title>{title}</title>\n\
         <style>\n{PAGE_CSS}\n{DOC_CSS}\n{rule}\n</style>\n\
         </head>\n\
         <body>\n\
         <article class=\"{class}\">\n{body}\n</article>\n\
         </body>\n\
         </html>\n"
    )
}

/// `notes.md` names a document `notes`; an unsaved tab gets a title anyway,
/// because a browser tab labelled with a bare path fragment is worse.
pub fn display_title(name: &str) -> &str {
    let stem = name
        .strip_suffix(".md")
        .or_else(|| name.strip_suffix(".markdown"))
        .or_else(|| name.strip_suffix(".mdown"))
        .or_else(|| name.strip_suffix(".mkd"))
        .unwrap_or(name)
        .trim();
    if stem.is_empty() { "Untitled" } else { stem }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carries_only_the_chosen_theme() {
        let out = build_document("x", "", "", "dark-hard");
        assert!(out.contains("<html lang=\"en\" data-theme=\"dark-hard\">"));
        assert!(out.contains(":root[data-theme=\"dark-hard\"] {"));
        assert!(out.contains("--bg: #1d2021;"));
        assert!(!out.contains("data-theme=\"apple-light\""));
    }

    #[test]
    fn no_theme_or_an_unknown_one_adds_nothing() {
        for theme in ["", "no-such-theme"] {
            let out = build_document("x", "", "", theme);
            assert!(out.contains("<html lang=\"en\">"), "{theme}");
            assert!(
                !out.contains(":root[data-theme=\"apple-light\"]"),
                "{theme}"
            );
            assert!(!out.contains(":root[data-theme=\"dark-hard\"]"), "{theme}");
        }
    }

    #[test]
    fn wraps_the_body_in_a_standalone_page() {
        let out = build_document("notes.md", "<p>hi</p>", "", "");
        assert!(out.starts_with("<!doctype html>"));
        assert!(out.contains("<title>notes</title>"));
        assert!(out.contains("<article class=\"markdown-body\">"));
        assert!(out.contains("<p>hi</p>"));
        // Self-contained: no external stylesheet, script or font.
        assert!(!out.contains("<link"));
        assert!(!out.contains("<script"));
    }

    /// The whole point of embedding `document.css`: an exported file must not
    /// depend on the app being installed to look right.
    #[test]
    fn embeds_the_document_stylesheet() {
        let out = build_document("x", "", "", "");
        assert!(out.contains(".markdown-body table th"));
        assert!(out.contains("--bezel-surface-alt"));
        assert!(out.contains("prefers-color-scheme: dark"));
        // Alternate render styles travel with the file.
        assert!(out.contains("md-numbered"));
        assert!(out.contains("counter(md-h1)"));
        assert!(out.contains("md-style-ogma"));
        assert!(out.contains("md-style-manuscript"));
        assert!(out.contains("md-style-compact"));
        assert!(out.contains("md-style-typewriter"));
        assert!(out.contains("md-full-width"));
        // The print rules travel with the file too: an exported document is
        // also something a browser prints, and it must paginate rather than
        // hand a dark-mode reader a page of white ink.
        assert!(out.contains("@media print"));
        assert!(out.contains("@page"));
        assert!(out.contains("print-color-adjust: exact"));
        assert!(out.contains("break-inside: avoid"));
    }

    #[test]
    fn exports_markdown_with_read_only_tasks() {
        let out = export_html("t.md", "- [x] done\n- [ ] todo\n", "", "");
        assert!(out.contains("<title>t</title>"));
        assert!(out.contains("disabled"));
        assert!(out.contains("checked"));
        assert!(!out.contains("contenteditable"));
    }

    #[test]
    fn extra_classes_land_on_the_article() {
        let out = build_document("x", "<p>hi</p>", "md-numbered md-full-width", "");
        assert!(out.contains("<article class=\"markdown-body md-numbered md-full-width\">"));
        assert!(out.contains("<p>hi</p>"));
    }

    #[test]
    fn titles_are_escaped_and_de_extensioned() {
        assert!(build_document("a & b.md", "", "", "").contains("<title>a &amp; b</title>"));
        assert!(build_document("", "", "", "").contains("<title>Untitled</title>"));
        assert!(build_document("Untitled", "", "", "").contains("<title>Untitled</title>"));
        // Only the markdown extensions are stripped, not any trailing dot-word.
        assert!(build_document("v1.2 notes", "", "", "").contains("<title>v1.2 notes</title>"));
    }
}
