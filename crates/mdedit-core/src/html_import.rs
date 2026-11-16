//! HTML to markdown, for smart paste and **Import HTML**.
//!
//! htmd does the conversion; on top of it:
//!
//! * A diagram from an mdedit export (`div[data-mermaid]`) comes back as its
//!   ```` ```mermaid ```` fence, and a formula (`span[data-math]`) as
//!   `$…$` or `$$…$$`, instead of as SVG or MathML.
//! * Scripts, styles and the document head are dropped.
//! * Images embedded as `data:` URIs become their alt text: a megabyte of
//!   base64 in the middle of a paragraph is nobody's idea of markdown.
//! * Non-breaking spaces become spaces and runs of blank lines collapse.

use htmd::element_handler::Handlers;
use htmd::{Element, HtmlToMarkdown};

fn attr(el: &Element, name: &str) -> Option<String> {
    el.attrs
        .iter()
        .find(|a| &*a.name.local == name)
        .map(|a| a.value.to_string())
}

fn converter() -> HtmlToMarkdown {
    HtmlToMarkdown::builder()
        .skip_tags(vec!["script", "style", "head", "noscript", "template"])
        .add_handler(vec!["div", "figure"], |h: &dyn Handlers, el: Element| {
            if let Some(src) = attr(&el, "data-mermaid") {
                return Some(format!("\n\n```mermaid\n{}\n```\n\n", src.trim_end()).into());
            }
            h.fallback(el)
        })
        .add_handler(vec!["span"], |h: &dyn Handlers, el: Element| {
            if let Some(src) = attr(&el, "data-math") {
                let display = attr(&el, "class")
                    .is_some_and(|c| c.split_whitespace().any(|c| c == "math-display"));
                return Some(
                    if display {
                        format!("\n\n$$\n{}\n$$\n\n", src.trim())
                    } else {
                        format!("${}$", src.trim())
                    }
                    .into(),
                );
            }
            h.fallback(el)
        })
        .add_handler(vec!["img"], |h: &dyn Handlers, el: Element| {
            if attr(&el, "src").is_some_and(|s| s.trim_start().starts_with("data:")) {
                return Some(attr(&el, "alt").unwrap_or_default().into());
            }
            h.fallback(el)
        })
        .build()
}

/// Convert `html` to markdown.
pub fn html_to_markdown(html: &str) -> String {
    let md = converter().convert(html).unwrap_or_default();
    tidy(&md)
}

fn tidy(md: &str) -> String {
    let md = md.replace('\u{a0}', " ");
    let mut out = String::with_capacity(md.len());
    let mut blanks = 0;
    for line in md.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
        } else {
            blanks = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_html_becomes_markdown() {
        let md = html_to_markdown(
            "<h1>Title</h1><p>Some <b>bold</b> and <a href=\"https://x.y\">a link</a>.</p><ul><li>one</li><li>two</li></ul>",
        );
        assert!(md.starts_with("# Title"), "{md}");
        assert!(md.contains("**bold**"), "{md}");
        assert!(md.contains("[a link](https://x.y)"), "{md}");
        assert!(md.contains("one") && md.contains("two"), "{md}");
    }

    #[test]
    fn exported_diagrams_and_formulas_come_back_as_source() {
        let html = concat!(
            "<p>See</p>",
            "<div class=\"mermaid-diagram\" data-mermaid=\"graph TD&#10;  A --&gt; B&#10;\"><svg><text>A</text></svg></div>",
            "<p>and <span class=\"math\" data-math=\"x^2\"><math><mi>x</mi></math></span> or</p>",
            "<span class=\"math math-display\" data-math=\"\\int_0^1 x\"><math></math></span>",
        );
        let md = html_to_markdown(html);
        assert!(md.contains("```mermaid\ngraph TD\n  A --> B\n```"), "{md}");
        assert!(md.contains("and $x^2$ or"), "{md}");
        assert!(md.contains("$$\n\\int_0^1 x\n$$"), "{md}");
        assert!(!md.contains("<svg"), "{md}");
    }

    #[test]
    fn scripts_go_and_inline_images_become_their_alt_text() {
        let md = html_to_markdown(
            "<style>p{}</style><script>alert(1)</script><p>A&nbsp;B <img src=\"data:image/png;base64,AAAA\" alt=\"logo\"> <img src=\"cat.png\" alt=\"cat\"></p>",
        );
        assert!(!md.contains("alert"), "{md}");
        assert!(md.contains("A B logo"), "{md}");
        assert!(md.contains("![cat](cat.png)"), "{md}");
        assert!(!md.contains("base64"), "{md}");
    }
}
