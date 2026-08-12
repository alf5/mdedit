use pulldown_cmark::{html, CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd};

/// Render GitHub-flavored markdown to HTML.
///
/// Extensions match GitHub's: tables, task lists, strikethrough, footnotes,
/// and alerts (`> [!NOTE]` etc., via ENABLE_GFM). ```` ```mermaid ```` fences
/// become inline SVG diagrams (see [`crate::mermaid`]).
pub fn to_html(md: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_GFM);
    let parser = Parser::new_ext(md, opts);
    let mut out = String::new();
    html::push_html(&mut out, MermaidFences::new(parser));
    out
}

/// Is this fence's info string a mermaid one? GitHub matches the first word,
/// case-insensitively, so ` ```Mermaid ` and ` ```mermaid title=x ` both count.
fn is_mermaid(info: &str) -> bool {
    info.split_whitespace()
        .next()
        .is_some_and(|w| w.eq_ignore_ascii_case("mermaid"))
}

/// Swaps each mermaid code block in an event stream for the rendered diagram.
struct MermaidFences<'a, I> {
    inner: I,
    /// Fence body collected while inside a mermaid block.
    buf: Option<String>,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a, I: Iterator<Item = Event<'a>>> MermaidFences<'a, I> {
    fn new(inner: I) -> Self {
        Self {
            inner,
            buf: None,
            _marker: std::marker::PhantomData,
        }
    }
}

impl<'a, I: Iterator<Item = Event<'a>>> Iterator for MermaidFences<'a, I> {
    type Item = Event<'a>;

    fn next(&mut self) -> Option<Event<'a>> {
        loop {
            let ev = self.inner.next()?;
            match (&mut self.buf, ev) {
                // Entering a mermaid fence: swallow events until it closes.
                (None, Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))))
                    if is_mermaid(&info) =>
                {
                    self.buf = Some(String::new());
                }
                (Some(buf), Event::Text(t)) => buf.push_str(&t),
                (Some(_), Event::End(TagEnd::CodeBlock)) => {
                    let src = self.buf.take().unwrap_or_default();
                    return Some(Event::Html(CowStr::Boxed(
                        crate::mermaid::render_block(&src).into_boxed_str(),
                    )));
                }
                // Anything else inside the fence (soft breaks, stray inlines)
                // can't appear in a code block, but drop it rather than emit
                // it out of place.
                (Some(_), _) => {}
                (None, ev) => return Some(ev),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_gfm_extensions() {
        let html = to_html(
            "| a |\n|---|\n| b |\n\n- [x] task\n\n~~gone~~\n\nfoot[^1]\n\n[^1]: note\n\n> [!NOTE]\n> alert",
        );
        assert!(html.contains("<table>"));
        assert!(html.contains("type=\"checkbox\""));
        assert!(html.contains("<del>"));
        assert!(html.contains("footnote-reference"));
        assert!(html.contains("markdown-alert-note"));
    }

    #[test]
    fn renders_mermaid_fences_as_svg() {
        let html = to_html("text\n\n```mermaid\ngraph TD\n  A --> B\n```\n\nmore");
        assert!(html.contains("<svg"));
        assert!(html.contains("mermaid-diagram"));
        // The fence itself is gone — no <pre><code> left behind.
        assert!(!html.contains("language-mermaid"));
        // Surrounding content is untouched.
        assert!(html.contains("<p>text</p>"));
        assert!(html.contains("<p>more</p>"));
    }

    #[test]
    fn matches_the_info_string_loosely() {
        assert!(to_html("```Mermaid\ngraph TD\n A-->B\n```").contains("<svg"));
        assert!(to_html("```mermaid  \ngraph TD\n A-->B\n```").contains("<svg"));
    }

    #[test]
    fn leaves_other_code_blocks_alone() {
        let html = to_html("```rust\nfn main() {}\n```");
        assert!(html.contains("language-rust"));
        assert!(!html.contains("mermaid-diagram"));
        let html = to_html("```mermaidx\nnope\n```");
        assert!(!html.contains("mermaid-diagram"));
    }

    #[test]
    fn indented_code_blocks_are_not_mermaid() {
        let html = to_html("    mermaid\n    graph TD\n");
        assert!(!html.contains("mermaid-diagram"));
    }
}
