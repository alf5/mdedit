//! Mermaid diagram rendering.
//!
//! A ```` ```mermaid ```` fence is rendered to inline SVG by Pele and wrapped
//! in a `div.mermaid-diagram` that carries the original source in
//! `data-mermaid`. The serializer reads that attribute back, so the diagram
//! round-trips to markdown exactly as typed no matter what the SVG looks like.
//!
//! Pele is JavaScript — the one part of mdedit that is — vendored at
//! `vendor/pele/pele.js` and imported through wasm-bindgen, which ships it in
//! the build's `snippets/`. It renders synchronously, so a fence still becomes
//! markup in the same pass as the markdown around it. Colors and fonts come
//! from the `--pele-*` custom properties `document.css` sets, so a diagram
//! follows the light/dark scheme, and print, without being re-rendered.

/// Class on the wrapper div; also the serializer's and the editor's hook.
pub const WRAPPER_CLASS: &str = "mermaid-diagram";
/// Attribute holding the verbatim fence body.
pub const SOURCE_ATTR: &str = "data-mermaid";

/// The faces Pele measures label text in, which have to be the faces the
/// labels are then drawn in or text spills out of its box. `document.css`
/// sets `--pele-font` and `--pele-font-mono` to these same stacks: the
/// document's own body face, and its code face.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const FONT: &str =
    "-apple-system, BlinkMacSystemFont, \"Segoe UI\", \"Noto Sans\", Helvetica, Arial, sans-serif";
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const FONT_MONO: &str = "ui-monospace, SFMono-Regular, \"SF Mono\", Menlo, Consolas, monospace";

#[cfg(target_arch = "wasm32")]
mod pele {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(module = "/vendor/pele/pele.js")]
    extern "C" {
        /// `render(source, options)`: `{ svg, width, height, type, … }`, or a
        /// thrown `PeleError` whose message already says where the source
        /// went wrong.
        #[wasm_bindgen(catch)]
        pub fn render(source: &str, options: &JsValue) -> Result<JsValue, JsValue>;
    }
}

/// Draw one diagram with Pele.
#[cfg(target_arch = "wasm32")]
fn render_svg(src: &str) -> Result<String, String> {
    use js_sys::{Object, Reflect};
    use wasm_bindgen::JsValue;

    thread_local! {
        static DRAWN: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }
    // Anything Pele does give an id is prefixed with this. Every drawing gets
    // its own, so two diagrams on one page — or one in each side of a split
    // window — can never answer for each other's ids.
    let n = DRAWN.with(|c| {
        c.set(c.get().wrapping_add(1));
        c.get()
    });
    let opts = Object::new();
    for (key, value) in [
        ("idPrefix", format!("mmd{n}")),
        ("fontFamily", FONT.to_string()),
        ("fontFamilyMono", FONT_MONO.to_string()),
    ] {
        let _ = Reflect::set(&opts, &key.into(), &JsValue::from_str(&value));
    }
    let drawn = pele::render(src, &opts).map_err(|e| {
        Reflect::get(&e, &"message".into())
            .ok()
            .and_then(|m| m.as_string())
            .or_else(|| e.as_string())
            .unwrap_or_else(|| "The diagram could not be drawn.".to_string())
    })?;
    Reflect::get(&drawn, &"svg".into())
        .ok()
        .and_then(|s| s.as_string())
        .ok_or_else(|| "The diagram could not be drawn.".to_string())
}

/// Off wasm — under `cargo test` — there is no JavaScript to run. An empty
/// drawing stands in for Pele so that everything around it (finding fences,
/// the wrapper, the source attribute) stays testable on the host. Pele itself
/// is exercised in the app.
#[cfg(not(target_arch = "wasm32"))]
fn render_svg(_src: &str) -> Result<String, String> {
    Ok("<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>".to_string())
}

/// Render one fence body to the wrapper div. A diagram that fails to parse
/// keeps its source visible with the error above it, rather than vanishing.
pub fn render_block(src: &str) -> String {
    wrap(src, render_svg(src))
}

/// The wrapper around a drawing, or around the reason there isn't one.
///
/// The newlines just inside it draw nothing, but they keep the block's text
/// apart from its neighbours'. Pele's SVG has no whitespace at its edges, and
/// the word count — which takes each diagram's labels back out of the page's
/// text — would otherwise see two adjacent diagrams' labels as one word.
fn wrap(src: &str, drawn: Result<String, String>) -> String {
    let attr = escape_attr(src);
    match drawn {
        Ok(svg) => format!(
            "<div class=\"{WRAPPER_CLASS}\" contenteditable=\"false\" \
             {SOURCE_ATTR}=\"{attr}\">\n{svg}\n</div>"
        ),
        Err(e) => format!(
            "<div class=\"{WRAPPER_CLASS} mermaid-error\" contenteditable=\"false\" \
             {SOURCE_ATTR}=\"{attr}\">\n<p class=\"mermaid-error-msg\">{}</p>\
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
/// survives the browser's own re-serialization of `innerHTML` (which undo
/// snapshots and clipboard copies go through).
fn escape_attr(s: &str) -> String {
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
    fn a_drawing_is_wrapped_as_an_uneditable_block() {
        let html = wrap("graph TD\n  A --> B\n", Ok("<svg></svg>".to_string()));
        assert!(html.contains("<svg></svg>"));
        assert!(html.contains(WRAPPER_CLASS));
        assert!(html.contains("contenteditable=\"false\""));
        assert!(!html.contains("mermaid-error"));
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
