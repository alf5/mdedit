//! Export the current document as a single standalone HTML file — and print
//! that same file, which is how **Print…** and **Export as PDF…** work too.
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
//!
//! Nothing here knows how to paginate. That is `@media print` at the foot of
//! `document.css`, which every one of these routes carries with it: printing
//! from the app, exporting a PDF (`src-tauri/src/pdf.rs`), and printing an
//! exported `.html` from a browser all go through the same rules.

use leptos::prelude::document;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Element, HtmlDocument, HtmlIFrameElement, HtmlInputElement};

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

/* The page box supplies the margin when printing; see document.css. */
@media print {
  body {
    padding: 0;
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

/// Wrap a cleaned body in the standalone page. `extra_class` is appended to
/// `markdown-body` so an export keeps the editor's style and full-width
/// setting; pass `""` for the default sheet.
pub fn build_document(title: &str, body: &str, extra_class: &str) -> String {
    let title = crate::mermaid::escape_text(display_title(title));
    let class = if extra_class.is_empty() {
        "markdown-body".to_string()
    } else {
        format!("markdown-body {extra_class}")
    };
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
         <article class=\"{class}\">\n{body}\n</article>\n\
         </body>\n\
         </html>\n"
    )
}

/// The off-screen frame that print jobs are staged in.
const PRINT_FRAME_ID: &str = "mdedit-print-frame";

/// Hand a standalone document to the platform's print dialog.
///
/// The page is written into an off-screen iframe and *that frame* is printed:
/// printing a subframe prints only the subframe, so what reaches the paper is
/// the exported document — not the editor around it, with its chrome, its tab
/// strip, its `contenteditable`, and, in source mode, a textarea full of raw
/// markdown. How the pages then break is `document.css`'s `@media print`,
/// which travels inside the document like the rest of the stylesheet.
///
/// The frame is written to synchronously rather than through `srcdoc`, which
/// loads asynchronously and would leave the print racing a load event. That is
/// only safe because an exported document fetches nothing — the stylesheet is
/// inline and every diagram is already SVG.
///
/// It is also left in the DOM afterwards: `print()` returns before the dialog
/// it opened has finished with the frame, so removing it would be a race. The
/// next print overwrites the contents instead.
pub fn print_document(html: &str) -> Result<(), &'static str> {
    let frame = print_frame()?;
    let staged = frame
        .content_document()
        .and_then(|d| d.dyn_into::<HtmlDocument>().ok())
        .ok_or("the print frame has no document")?;

    staged
        .open()
        .map_err(|_| "could not open the print frame")?;
    staged
        .write(&js_sys::Array::of1(&JsValue::from_str(html)))
        .map_err(|_| "could not write to the print frame")?;
    staged
        .close()
        .map_err(|_| "could not close the print frame")?;

    frame
        .content_window()
        .ok_or("the print frame has no window")?
        .print()
        .map_err(|_| "the print dialog could not be opened")
}

/// The staging frame, made on the first print and reused after that.
fn print_frame() -> Result<HtmlIFrameElement, &'static str> {
    let doc = document();
    if let Some(existing) = doc
        .get_element_by_id(PRINT_FRAME_ID)
        .and_then(|el| el.dyn_into::<HtmlIFrameElement>().ok())
    {
        return Ok(existing);
    }

    let frame = doc
        .create_element("iframe")
        .ok()
        .and_then(|el| el.dyn_into::<HtmlIFrameElement>().ok())
        .ok_or("could not create the print frame")?;
    frame.set_id(PRINT_FRAME_ID);
    // Pushed off the edge rather than hidden with `display: none`: a frame
    // that is not rendered is never laid out, and a frame with no layout
    // prints blank.
    frame
        .set_attribute(
            "style",
            "position:fixed;right:0;bottom:0;width:1px;height:1px;\
             opacity:0;border:0;pointer-events:none;",
        )
        .map_err(|_| "could not place the print frame")?;
    doc.body()
        .ok_or("the page has no body")?
        .append_child(&frame)
        .map_err(|_| "could not attach the print frame")?;
    Ok(frame)
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
        let out = build_document("notes.md", "<p>hi</p>", "");
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
        let out = build_document("x", "", "");
        assert!(out.contains(".markdown-body table th"));
        assert!(out.contains("--mmd-node-fill"));
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
    fn extra_classes_land_on_the_article() {
        let out = build_document("x", "<p>hi</p>", "md-numbered md-full-width");
        assert!(out.contains("<article class=\"markdown-body md-numbered md-full-width\">"));
        assert!(out.contains("<p>hi</p>"));
    }

    #[test]
    fn titles_are_escaped_and_de_extensioned() {
        assert!(build_document("a & b.md", "", "").contains("<title>a &amp; b</title>"));
        assert!(build_document("", "", "").contains("<title>Untitled</title>"));
        assert!(build_document("Untitled", "", "").contains("<title>Untitled</title>"));
        // Only the markdown extensions are stripped, not any trailing dot-word.
        assert!(build_document("v1.2 notes", "", "").contains("<title>v1.2 notes</title>"));
    }
}
