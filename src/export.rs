//! Export the current document as a single standalone HTML file.
//!
//! Nothing is fetched at view time: the stylesheet is embedded and every
//! mermaid diagram is already inline SVG, because that is how the editor
//! draws it (see [`crate::mermaid`]). The file therefore opens identically
//! offline, in any browser, and follows the reader's light/dark preference —
//! diagram colors included, since they resolve through the `--mmd-*` custom
//! properties rather than being baked in.
//!
//! The diagrams also keep their `data-mermaid` source attribute, so an
//! exported file fed back through **Import HTML** comes home as
//! ```` ```mermaid ```` fences rather than as unrecoverable SVG.

use leptos::prelude::document;
use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlInputElement};

/// The editor's own document styles, verbatim. `index.html` links this same
/// file, so an exported document and the editor can never drift apart.
const DOC_CSS: &str = include_str!("../document.css");

/// What a standalone page needs and the app gets from `styles.css` instead
/// (which is all app chrome, and deliberately not embedded here).
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

@media print {
  body {
    padding: 0;
  }
  .markdown-body {
    max-width: none;
  }
}
";

/// The exportable body of a live WYSIWYG editor.
///
/// The caller must have closed any open diagram source first (`quiesce`):
/// `inner_html` serializes a textarea's *initial child text*, not its value,
/// so an open diagram would export as an empty box.
pub fn body_from_editor(editor: &Element) -> String {
    let html = editor.inner_html();
    let Some(holder) = parse(&html) else {
        return html;
    };
    mirror_checkbox_state(editor, &holder);
    clean(&holder);
    holder.inner_html()
}

/// The exportable body of markdown — source mode, where the WYSIWYG DOM is
/// stale. Renders through the same pipeline the editor uses, so diagrams come
/// out as the same SVG either way.
pub fn body_from_markdown(md: &str) -> String {
    let html = crate::markdown::to_html(md);
    let Some(holder) = parse(&html) else {
        return html;
    };
    clean(&holder);
    holder.inner_html()
}

/// Wrap a cleaned body in the standalone page.
pub fn build_document(title: &str, body: &str) -> String {
    let title = crate::mermaid::escape_text(display_title(title));
    format!(
        "<!doctype html>\n\
         <html lang=\"en\">\n\
         <head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"generator\" content=\"mdedit\">\n\
         <title>{title}</title>\n\
         <style>\n{PAGE_CSS}\n{DOC_CSS}</style>\n\
         </head>\n\
         <body>\n\
         <article class=\"markdown-body\">\n{body}\n</article>\n\
         </body>\n\
         </html>\n"
    )
}

/// `notes.md` names a document `notes`; an unsaved tab gets a title anyway,
/// because a browser tab labelled with a bare path fragment is worse.
fn display_title(name: &str) -> &str {
    let stem = name
        .strip_suffix(".md")
        .or_else(|| name.strip_suffix(".markdown"))
        .or_else(|| name.strip_suffix(".mdown"))
        .or_else(|| name.strip_suffix(".mkd"))
        .unwrap_or(name)
        .trim();
    if stem.is_empty() {
        "Untitled"
    } else {
        stem
    }
}

/// A detached div holding a re-parsed copy, so the live editor is never
/// touched. Queries and mutations work on detached nodes.
fn parse(html: &str) -> Option<Element> {
    let holder = document().create_element("div").ok()?;
    holder.set_inner_html(html);
    Some(holder)
}

/// Take the editing machinery back out.
fn clean(root: &Element) {
    for el in select(root, "[contenteditable]") {
        let _ = el.remove_attribute("contenteditable");
    }
    for el in select(root, "[spellcheck]") {
        let _ = el.remove_attribute("spellcheck");
    }
    // The editor enables task checkboxes so they can be ticked; an exported
    // document is a record, so they go back to being read-only.
    for el in select(root, "input[type=checkbox]") {
        let _ = el.set_attribute("disabled", "");
    }
}

/// Copy each checkbox's *checked property* onto the copy's attribute.
///
/// Ticking a checkbox sets the property, never the content attribute, and
/// `inner_html` serializes attributes — so without this a task ticked since
/// the document was last loaded would export unticked.
fn mirror_checkbox_state(live: &Element, copy: &Element) {
    let sel = "input[type=checkbox]";
    // Same markup parsed twice, so the two lists are in the same order.
    for (from, to) in select(live, sel).into_iter().zip(select(copy, sel)) {
        let checked = from
            .dyn_ref::<HtmlInputElement>()
            .is_some_and(HtmlInputElement::checked);
        if checked {
            let _ = to.set_attribute("checked", "");
        } else {
            let _ = to.remove_attribute("checked");
        }
    }
}

fn select(root: &Element, sel: &str) -> Vec<Element> {
    let Ok(list) = root.query_selector_all(sel) else {
        return Vec::new();
    };
    (0..list.length())
        .filter_map(|i| list.item(i).and_then(|n| n.dyn_into::<Element>().ok()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_the_body_in_a_standalone_page() {
        let out = build_document("notes.md", "<p>hi</p>");
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
        let out = build_document("x", "");
        assert!(out.contains(".markdown-body table th"));
        assert!(out.contains("--mmd-node-fill"));
        assert!(out.contains("prefers-color-scheme: dark"));
    }

    #[test]
    fn titles_are_escaped_and_de_extensioned() {
        assert!(build_document("a & b.md", "").contains("<title>a &amp; b</title>"));
        assert!(build_document("", "").contains("<title>Untitled</title>"));
        assert!(build_document("Untitled", "").contains("<title>Untitled</title>"));
        // Only the markdown extensions are stripped, not any trailing dot-word.
        assert!(build_document("v1.2 notes", "").contains("<title>v1.2 notes</title>"));
    }
}
