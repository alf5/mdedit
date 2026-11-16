use std::collections::VecDeque;
use std::iter::Peekable;

use pulldown_cmark::{CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd, html};

/// Render GitHub-flavored markdown to HTML, for export. (The app draws
/// documents itself; this is what a standalone `.html` contains.)
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
    opts.insert(Options::ENABLE_MATH);
    let parser = Parser::new_ext(md, opts);
    let mut out = String::new();
    let events = TableWidths::new(MermaidFences::new(parser)).map(render_math);
    html::push_html(&mut out, events);
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

/// Swap a math event for the rendered formula.
fn render_math(ev: Event<'_>) -> Event<'_> {
    let (src, display) = match &ev {
        Event::InlineMath(src) => (src, false),
        Event::DisplayMath(src) => (src, true),
        _ => return ev,
    };
    let html = crate::math::render(src, display);
    Event::InlineHtml(CowStr::Boxed(html.into_boxed_str()))
}

/// Turns the `<!-- zen:cols=… -->` comment after a table into a `<colgroup>`
/// inside it. The comment only arrives after the table's last row, so each
/// table is held back until what follows it is known.
struct TableWidths<'a, I: Iterator<Item = Event<'a>>> {
    inner: Peekable<I>,
    pending: VecDeque<Event<'a>>,
}

impl<'a, I: Iterator<Item = Event<'a>>> TableWidths<'a, I> {
    fn new(inner: I) -> Self {
        Self {
            inner: inner.peekable(),
            pending: VecDeque::new(),
        }
    }

    /// Queue a whole table, given its opening event, with any widths applied.
    fn hold_table(&mut self, start: Event<'a>) {
        let mut table = vec![start];
        for ev in self.inner.by_ref() {
            let done = matches!(ev, Event::End(TagEnd::Table));
            table.push(ev);
            if done {
                break;
            }
        }
        // Only an HTML block can be the comment, and it arrives as start, text,
        // end. Anything else stays unread, so a second table right after this
        // one still comes through `next` and gets its own widths.
        let mut after = Vec::new();
        let mut widths = None;
        if matches!(self.inner.peek(), Some(Event::Start(Tag::HtmlBlock))) {
            let mut text = String::new();
            for ev in self.inner.by_ref() {
                let done = matches!(ev, Event::End(TagEnd::HtmlBlock));
                if let Event::Html(t) = &ev {
                    text.push_str(t);
                }
                after.push(ev);
                if done {
                    break;
                }
            }
            widths = crate::table::parse_cols_comment(&text);
        }
        match widths {
            Some(w) => {
                let col = crate::table::colgroup_html(&w);
                table.insert(1, Event::Html(CowStr::Boxed(col.into_boxed_str())));
                self.pending.extend(table);
            }
            None => {
                self.pending.extend(table);
                self.pending.extend(after);
            }
        }
    }
}

impl<'a, I: Iterator<Item = Event<'a>>> Iterator for TableWidths<'a, I> {
    type Item = Event<'a>;

    fn next(&mut self) -> Option<Event<'a>> {
        if let Some(ev) = self.pending.pop_front() {
            return Some(ev);
        }
        let ev = self.inner.next()?;
        if matches!(ev, Event::Start(Tag::Table(_))) {
            self.hold_table(ev);
            return self.pending.pop_front();
        }
        Some(ev)
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

    #[test]
    fn renders_math() {
        let html = to_html("Energy $E = mc^2$ here\n\n$$\\frac{a}{b}$$\n");
        assert!(html.contains("data-math=\"E = mc^2\""));
        assert!(html.contains("<math"));
        assert!(html.contains("math-display"));
        assert!(html.contains("<mfrac"));
    }

    #[test]
    fn math_in_a_table_cell_renders_and_keeps_its_pipe() {
        let html = to_html("| a |\n|---|\n| $\\|x\\|$ |\n");
        assert!(html.contains("data-math=\"|x|\""), "{html}");
    }

    #[test]
    fn prices_are_not_math() {
        for md in [
            "It costs $5 and $6 today",
            "$5",
            "from $5 to $10.",
            "a $ b $ c",
        ] {
            let html = to_html(md);
            assert!(!html.contains("data-math"), "{md} became {html}");
        }
    }

    #[test]
    fn an_escaped_dollar_is_not_math() {
        assert!(!to_html(r"\$a\$").contains("data-math"));
    }

    #[test]
    fn a_width_comment_becomes_a_colgroup() {
        let html = to_html("| a | b |\n|---|---|\n| 1 | 2 |\n<!-- zen:cols=120,auto -->\n\nafter");
        assert!(html.contains("<table><colgroup><col style=\"width: 120px\"><col></colgroup>"));
        assert!(!html.contains("zen:cols"));
        assert!(html.contains("<td>2</td>"));
        assert!(html.contains("<p>after</p>"));
    }

    #[test]
    fn a_width_comment_after_a_blank_line_still_counts() {
        let html = to_html("| a |\n|---|\n| 1 |\n\n<!-- zen:cols=90 -->\n");
        assert!(html.contains("width: 90px"));
    }

    #[test]
    fn an_unrelated_comment_after_a_table_stays() {
        let html = to_html("| a |\n|---|\n| 1 |\n\n<!-- hello -->\n\nafter");
        assert!(!html.contains("<colgroup>"));
        assert!(html.contains("<!-- hello -->"));
        assert!(html.contains("<p>after</p>"));
    }

    #[test]
    fn whatever_follows_a_table_survives() {
        let html = to_html("| a |\n|---|\n| 1 |\n\nafter\n\n| b |\n|---|\n| 2 |\n");
        assert!(html.contains("<p>after</p>"));
        assert_eq!(html.matches("<table>").count(), 2);
        assert!(html.contains("<td>2</td>"));
    }

    #[test]
    fn adjacent_tables_each_keep_their_own_widths() {
        let html = to_html("| a |\n|---|\n| 1 |\n\n| b |\n|---|\n| 2 |\n<!-- zen:cols=90 -->\n");
        assert_eq!(html.matches("<table>").count(), 2);
        assert_eq!(html.matches("<colgroup>").count(), 1);
        assert!(html.find("<td>1</td>").unwrap() < html.find("<colgroup>").unwrap());
    }

    #[test]
    fn a_table_at_the_end_of_the_file_survives() {
        assert!(to_html("| a |\n|---|\n| 1 |").ends_with("</table>\n"));
    }
}
