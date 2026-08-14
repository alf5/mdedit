//! Mermaid diagram rendering.
//!
//! A ```` ```mermaid ```` fence is rendered to inline SVG by `mermaid-svg`
//! (pure Rust — no Node, no `mermaid.js`) and wrapped in a
//! `div.mermaid-diagram` that carries the original source in `data-mermaid`.
//! The serializer reads that attribute back, so the diagram round-trips to
//! markdown exactly as typed no matter what the SVG looks like.

use mermaid_svg::{ast::Diagram, parse, render_diagram_with, Theme};
use std::borrow::Cow;

/// Class on the wrapper div; also the serializer's and the editor's hook.
pub const WRAPPER_CLASS: &str = "mermaid-diagram";
/// Attribute holding the verbatim fence body.
pub const SOURCE_ATTR: &str = "data-mermaid";

/// The diagram's colors come from CSS custom properties so a diagram follows
/// the app's light/dark scheme without re-rendering. `styles.css` defines the
/// `--mmd-*` properties under both schemes; the literal after each comma is
/// the upstream Mermaid default, used if a property is ever missing.
fn theme() -> Theme {
    let v = |name: &str, fallback: &str| -> Cow<'static, str> {
        Cow::Owned(format!("var({name}, {fallback})"))
    };
    Theme {
        fg: v("--mmd-fg", "#333"),
        fg_muted: v("--mmd-fg-muted", "#666"),
        bg: v("--mmd-bg", "#fff"),
        actor_fill: v("--mmd-node-fill", "#ECECFF"),
        actor_stroke: v("--mmd-node-stroke", "#9370DB"),
        lifeline: v("--mmd-node-stroke", "#9370DB"),
        arrow_stroke: v("--mmd-edge", "#333"),
        note_fill: v("--mmd-note-fill", "#FFF5AD"),
        note_stroke: v("--mmd-note-stroke", "#aaaa33"),
        activation_fill: v("--mmd-node-fill", "#ECECFF"),
        activation_stroke: v("--mmd-node-stroke", "#9370DB"),
        frame_label_fill: v("--mmd-node-fill", "#ECECFF"),
        flow_node_fill: v("--mmd-node-fill", "#ECECFF"),
        flow_node_stroke: v("--mmd-node-stroke", "#9370DB"),
        flow_edge_stroke: v("--mmd-edge", "#333"),
        flow_label_bg: v("--mmd-label-bg", "#e8e8e8"),
        flow_cluster_fill: v("--mmd-cluster-fill", "#ffffde"),
        flow_cluster_stroke: v("--mmd-cluster-stroke", "#aaaa33"),
        font_family: Cow::Borrowed("inherit"),
        // The categorical scales (pie, git, journey…) are saturated hues that
        // read on either background, so they keep their upstream values.
        ..Theme::default_theme()
    }
}

/// Parse and draw one diagram.
///
/// This goes the long way round — parse, then render — rather than calling
/// `render_with`, because a gantt chart has to be adjusted in between. See
/// [`disarm_today_marker`].
fn render_svg(src: &str) -> Result<String, String> {
    let mut diagram = parse(src).map_err(|e| e.to_string())?;
    disarm_today_marker(&mut diagram);
    render_diagram_with(&diagram, &theme()).map_err(|e| e.to_string())
}

/// Turn off a gantt chart's `todayMarker`.
///
/// `wasm32-unknown-unknown` has no clock: `SystemTime::now()` is compiled in
/// as an unconditional panic. mermaid-svg reads it to place the marker, and
/// with `panic = "abort"` that trap takes down the whole app — one gantt
/// chart anywhere in a document and nothing renders at all. `todayMarker off`
/// is the only lever the crate gives us to skip that call, so the cost of
/// staying alive is that gantt charts draw no "today" line.
fn disarm_today_marker(diagram: &mut Diagram) {
    if let Diagram::Gantt(g) = diagram {
        g.today_marker = Some("off".to_string());
    }
}

/// Render one fence body to the wrapper div. A diagram that fails to parse
/// keeps its source visible with the error above it, rather than vanishing.
pub fn render_block(src: &str) -> String {
    let attr = escape_attr(src);
    match render_svg(src) {
        Ok(svg) => format!(
            "<div class=\"{WRAPPER_CLASS}\" contenteditable=\"false\" \
             {SOURCE_ATTR}=\"{attr}\">{svg}</div>"
        ),
        Err(e) => format!(
            "<div class=\"{WRAPPER_CLASS} mermaid-error\" contenteditable=\"false\" \
             {SOURCE_ATTR}=\"{attr}\"><p class=\"mermaid-error-msg\">{}</p>\
             <pre><code class=\"language-mermaid\">{}</code></pre></div>",
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
    fn renders_a_flowchart_to_svg() {
        let html = render_block("graph TD\n  A[Start] --> B[End]\n");
        assert!(html.contains("<svg"));
        assert!(html.contains(WRAPPER_CLASS));
        assert!(html.contains("contenteditable=\"false\""));
        assert!(!html.contains("mermaid-error"));
    }

    #[test]
    fn keeps_the_source_in_the_data_attribute() {
        let html = render_block("graph LR\n  A[\"x & y\"] --> B\n");
        assert!(html.contains("data-mermaid=\"graph LR&#10;  A[&quot;x &amp; y&quot;] --&gt; B&#10;\""));
    }

    #[test]
    fn bad_source_degrades_to_an_error_block() {
        let html = render_block("not a diagram at all\n");
        assert!(html.contains("mermaid-error"));
        // The source is still recoverable from the attribute.
        assert!(html.contains("data-mermaid=\"not a diagram at all&#10;\""));
    }

    /// A gantt chart must never reach `SystemTime::now()` — on wasm that call
    /// is a trap that aborts the app. Renders natively here, so it can only
    /// check that the marker is off; the wasm behaviour rides on that.
    #[test]
    fn gantt_today_marker_is_disarmed() {
        let src = "gantt\n  dateFormat YYYY-MM-DD\n  section S\n  Task :a1, 2026-01-01, 30d\n";
        let mut d = parse(src).unwrap();
        disarm_today_marker(&mut d);
        match &d {
            Diagram::Gantt(g) => assert_eq!(g.today_marker.as_deref(), Some("off")),
            _ => panic!("expected a gantt diagram"),
        }
        let html = render_block(src);
        assert!(html.contains("<svg"));
        assert!(!html.contains("mermaid-error"));
        // The marker element must not be emitted at all.
        assert!(!html.contains("\"today\""));
    }

    /// An explicit `todayMarker` in the source is overridden, not honoured —
    /// any value other than `off` still reaches the clock.
    #[test]
    fn explicit_today_marker_is_overridden() {
        let src = "gantt\n  dateFormat YYYY-MM-DD\n  todayMarker stroke:red\n  section S\n  T :a1, 2026-01-01, 9d\n";
        let mut d = parse(src).unwrap();
        disarm_today_marker(&mut d);
        match &d {
            Diagram::Gantt(g) => assert_eq!(g.today_marker.as_deref(), Some("off")),
            _ => panic!("expected a gantt diagram"),
        }
    }

    #[test]
    fn colors_come_from_css_properties() {
        let html = render_block("graph TD\n  A --> B\n");
        assert!(html.contains("var(--mmd-node-fill"));
    }
}
