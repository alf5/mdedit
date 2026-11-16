//! What one source line looks like on screen: its markup hidden or shown,
//! bullets and task boxes drawn in place of `- ` and `[ ] `, and a map
//! between display and source offsets in both directions.
//!
//! The editor moves the caret in display offsets, so Left and Right step
//! over hidden markup as a single position and nothing special is needed to
//! keep the caret out of it.

use std::ops::Range;

use crate::syntax::{LineKind, MarkupKind, Syntax, style};

/// How a stretch of display text is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Text,
    /// Markup that is shown (Source mode, or Live mode's caret line).
    Markup,
    Bullet,
    Number,
    TaskOpen,
    TaskDone,
    /// A code fence's language.
    Lang,
    /// An alert's title, drawn in place of its `[!NOTE]`.
    Alert,
    /// A `<br>` in a table cell, drawn as a line break.
    Break,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub len: usize,
    pub style: u16,
    pub role: Role,
}

/// A stretch of display text and the source it stands for. `copy` stretches
/// are the source verbatim; the others are drawn in its place (a bullet).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seg {
    pub display: Range<usize>,
    pub source: Range<usize>,
    pub copy: bool,
}

/// What a hidden list marker is drawn as, in the margin before the text, so
/// an item's wrapped lines hang under its first word.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gutter {
    Bullet,
    Number(String),
    /// A task box, and the source of its `[ ]`.
    Task {
        done: bool,
        source: Range<usize>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayLine {
    pub text: String,
    pub runs: Vec<Run>,
    pub segs: Vec<Seg>,
    pub source: Range<usize>,
    pub gutter: Option<Gutter>,
}

/// How markup is treated on this line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reveal {
    /// Rendered: markup hidden.
    Hide,
    /// Live mode's caret line, or Source mode: markup shown, dimmed.
    Show,
}

impl DisplayLine {
    fn push(&mut self, s: &str, source: Range<usize>, copy: bool, style: u16, role: Role) {
        if s.is_empty() && source.is_empty() {
            return;
        }
        let d = self.text.len();
        self.text.push_str(s);
        self.segs.push(Seg {
            display: d..d + s.len(),
            source,
            copy,
        });
        if !s.is_empty() {
            match self.runs.last_mut() {
                Some(r) if r.style == style && r.role == role => r.len += s.len(),
                _ => self.runs.push(Run {
                    len: s.len(),
                    style,
                    role,
                }),
            }
        }
    }

    /// The display offset for source offset `src` (in this line). Inside
    /// hidden markup it is the edge the markup collapsed to; on a drawn
    /// bullet, after it.
    pub fn to_display(&self, src: usize) -> usize {
        let mut last = 0;
        for seg in &self.segs {
            // Just before a line break: the end of the line it ends.
            if src == seg.source.start && self.is_break(seg) {
                return seg.display.start;
            }
            if src < seg.source.start {
                return seg.display.start.min(last.max(seg.display.start));
            }
            if src < seg.source.end
                || (src == seg.source.end && seg.copy && seg.source.end == self.source.end)
            {
                return if seg.copy {
                    seg.display.start + (src - seg.source.start)
                } else {
                    seg.display.end
                };
            }
            last = seg.display.end;
        }
        self.text.len()
    }

    /// The source offset for display offset `d`. Between two stretches it
    /// lands at the start of the later one, so typing after hidden opening
    /// markup goes inside it.
    pub fn to_source(&self, d: usize) -> usize {
        for seg in &self.segs {
            if seg.display.contains(&d) {
                return if seg.copy {
                    seg.source.start + (d - seg.display.start)
                } else if self.is_break(seg) {
                    // Typing at the end of a line goes before its break.
                    seg.source.start
                } else {
                    seg.source.end
                };
            }
        }
        // Past the last character: the end of the line, after any markup
        // that closes it.
        self.source.end
    }

    /// A `<br>` drawn as a newline.
    fn is_break(&self, seg: &Seg) -> bool {
        !seg.copy && self.text.get(seg.display.clone()) == Some("\n")
    }

    /// The previous and next display character boundaries.
    pub fn prev(&self, d: usize) -> usize {
        self.text[..d]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    pub fn next(&self, d: usize) -> usize {
        self.text[d..]
            .chars()
            .next()
            .map_or(self.text.len(), |c| d + c.len_utf8())
    }

    /// The drawn element (bullet, task box) at display offset `d`, if any.
    pub fn role_at(&self, d: usize) -> Role {
        let mut at = 0;
        for r in &self.runs {
            if d < at + r.len {
                return r.role;
            }
            at += r.len;
        }
        Role::Text
    }

    /// The source stretch a drawn element stands for.
    pub fn source_at(&self, d: usize) -> Option<Range<usize>> {
        self.segs
            .iter()
            .find(|s| s.display.contains(&d))
            .map(|s| s.source.clone())
    }
}

/// Build the display form of line `ix`.
pub fn display_line(text: &str, syntax: &Syntax, ix: usize, reveal: Reveal) -> DisplayLine {
    let line = &syntax.lines[ix];
    let l = line.range.clone();
    let mut out = DisplayLine {
        source: l.clone(),
        ..Default::default()
    };
    let show = reveal == Reveal::Show;
    let markup: Vec<_> = syntax
        .markup_in(l.clone())
        .map(|m| (m.range.start.max(l.start)..m.range.end.min(l.end), m.kind))
        .filter(|(r, k)| !r.is_empty() || *k == MarkupKind::HeadingPrefix)
        .collect();
    let lang = line.lang.clone();
    // Inside a list, rendered: the indentation is drawn as a margin, so the
    // leading spaces are hidden (code keeps them; they're its content).
    let mut hide_indent = !show
        && line.list > 0
        && !matches!(
            line.kind,
            LineKind::Code | LineKind::FenceOpen | LineKind::FenceClose
        );

    // Text between markup, split by inline style.
    let text_piece = |out: &mut DisplayLine, r: Range<usize>| {
        let mut at = r.start;
        let code_line = matches!(line.kind, LineKind::Code | LineKind::Table | LineKind::Html);
        for span in syntax.spans_in(r.clone()) {
            if code_line {
                break;
            }
            let s = span.range.start.max(r.start);
            let e = span.range.end.min(r.end);
            if s > at {
                plain(out, text, at..s, &lang);
            }
            if e > s {
                out.push(&text[s..e], s..e, true, span.style, Role::Text);
            }
            at = at.max(e);
        }
        if at < r.end {
            plain(out, text, at..r.end, &lang);
        }
    };

    let mut at = l.start;
    if hide_indent {
        let lead = text[l.clone()].len() - text[l.clone()].trim_start_matches(' ').len();
        if lead > 0
            && !markup.iter().any(|(r, k)| {
                r.start == l.start && matches!(k, MarkupKind::Bullet | MarkupKind::Number)
            })
        {
            out.push("", l.start..l.start + lead, false, 0, Role::Text);
            at = l.start + lead;
        }
        hide_indent = false;
    }
    let _ = hide_indent;
    for (i, (r, kind)) in markup.iter().enumerate() {
        if r.start < at {
            continue;
        }
        let src = &text[r.clone()];
        let marker_first = !show
            && matches!(kind, MarkupKind::Bullet | MarkupKind::Number)
            && text[l.start..r.start].trim().is_empty();
        if r.start > at && !marker_first {
            text_piece(&mut out, at..r.start);
        }
        // Marker markup starts at the item, after its indentation.
        let lead = &text[l.start..r.start];
        let marker_line = lead.trim().is_empty();
        match kind {
            MarkupKind::Number if show => out.push(src, r.clone(), true, 0, Role::Number),
            _ if show => out.push(src, r.clone(), true, 0, Role::Markup),
            MarkupKind::Bullet | MarkupKind::Number if marker_line => {
                // Drawn in the margin; a task's bullet gives way to its box.
                let task_next = matches!(markup.get(i + 1), Some((n, MarkupKind::TaskBox(_))) if n.start == r.end);
                out.push("", at.min(r.start)..r.end, false, 0, Role::Text);
                if !task_next {
                    out.gutter = Some(if *kind == MarkupKind::Bullet {
                        Gutter::Bullet
                    } else {
                        Gutter::Number(src.trim().to_string())
                    });
                }
            }
            MarkupKind::Number => out.push(src, r.clone(), true, 0, Role::Number),
            MarkupKind::Bullet => out.push("", r.clone(), false, 0, Role::Text),
            MarkupKind::Alert(a) => out.push(a.title(), r.clone(), false, style::BOLD, Role::Alert),
            MarkupKind::TaskBox(done) => {
                out.push("", r.clone(), false, 0, Role::Text);
                out.gutter = Some(Gutter::Task {
                    done: *done,
                    source: r.clone(),
                });
            }
            _ => out.push("", r.clone(), false, 0, Role::Text),
        }
        at = r.end;
    }
    if at < l.end {
        text_piece(&mut out, at..l.end);
    }
    if out.segs.is_empty() {
        out.segs.push(Seg {
            display: 0..0,
            source: l.clone(),
            copy: true,
        });
    }
    out
}

fn plain(out: &mut DisplayLine, text: &str, r: Range<usize>, lang: &Option<Range<usize>>) {
    let role = match lang {
        Some(l) if l.start <= r.start && r.end <= l.end.max(r.start) => Role::Lang,
        _ => Role::Text,
    };
    out.push(&text[r.clone()], r, true, 0, role);
}

/// How a row is drawn when it isn't text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowWidget {
    /// The widget (an index into `Syntax::widgets`) in place of its source.
    Closed(usize),
    /// The source is shown (the caret is in it) and the widget is drawn
    /// under this row, its last line, as a live preview.
    Preview(usize),
}

/// One drawn row: the source lines it covers (indices) and, if it's a
/// widget, which.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub lines: Range<usize>,
    pub widget: Option<RowWidget>,
}

/// The rows a document is drawn as. In the rendered modes a paragraph's
/// lines flow as one row, so hard-wrapped text reads as prose (a hard line
/// break, `\\` or two spaces at the end, still ends a row), and widgets
/// (tables, diagrams, formulas, images) are drawn in place of their
/// source, except the one holding the caret line `open`, whose source is
/// shown. Tables are the exception: they're edited in place, as a grid, so
/// they stay drawn. In Source mode every line is its own row.
pub fn rows(text: &str, syntax: &Syntax, rich: bool, open: Option<usize>) -> Vec<Row> {
    let lines = &syntax.lines;
    let mut out: Vec<Row> = Vec::with_capacity(lines.len());
    // Widgets by first line, walked alongside the lines: looking each line
    // up among all the widgets made a long document quadratic.
    let mut by_start: Vec<usize> = (0..syntax.widgets.len()).collect();
    by_start.sort_by_key(|&w| syntax.widgets[w].lines.start);
    let mut next = by_start.iter().peekable();
    let mut i = 0;
    while i < lines.len() {
        while next
            .peek()
            .is_some_and(|&&w| syntax.widgets[w].lines.start < i)
        {
            next.next();
        }
        if rich
            && let Some(&&w_ix) = next.peek()
            && syntax.widgets[w_ix].lines.start == i
        {
            let w = &syntax.widgets[w_ix];
            next.next();
            let end = w.lines.end.min(lines.len());
            // A table is edited in its grid, so it never opens.
            let is_open = open.is_some_and(|o| w.lines.contains(&o))
                && !matches!(w.kind, crate::syntax::WidgetKind::Table);
            if !is_open {
                out.push(Row {
                    lines: i..end,
                    widget: Some(RowWidget::Closed(w_ix)),
                });
            } else {
                for l in i..end {
                    let last = l + 1 == end;
                    let preview = last && !matches!(w.kind, crate::syntax::WidgetKind::Table);
                    out.push(Row {
                        lines: l..l + 1,
                        widget: preview.then_some(RowWidget::Preview(w_ix)),
                    });
                }
            }
            i = end;
            continue;
        }
        let line = &lines[i];
        let joins = rich
            && i > 0
            && line.kind == LineKind::Paragraph
            && line.para.is_some()
            && lines[i - 1].para == line.para
            && lines[i - 1].kind == LineKind::Paragraph
            && out
                .last()
                .is_some_and(|r| r.widget.is_none() && r.lines.end == i)
            && {
                let prev = &text[lines[i - 1].range.clone()];
                !(prev.ends_with('\\') || prev.ends_with("  "))
            }
            && !starts_item(syntax, i);
        match out.last_mut() {
            Some(row) if joins => row.lines.end = i + 1,
            _ => out.push(Row {
                lines: i..i + 1,
                widget: None,
            }),
        }
        i += 1;
    }
    out
}

/// Whether line `i` starts a list item (its marker is the first markup).
fn starts_item(syntax: &Syntax, i: usize) -> bool {
    let l = &syntax.lines[i];
    syntax.markup_in(l.range.clone()).any(|m| {
        matches!(m.kind, MarkupKind::Bullet | MarkupKind::Number) && m.range.start >= l.range.start
    })
}

/// The display form of several source lines drawn as one row: each line's
/// own display, joined by a space standing in for the newline.
pub fn display_row(
    text: &str,
    syntax: &Syntax,
    lines: Range<usize>,
    reveal: Reveal,
) -> DisplayLine {
    let first = lines.start;
    let mut out = display_line(text, syntax, first, reveal);
    for ix in lines.start + 1..lines.end {
        let next = display_line(text, syntax, ix, reveal);
        let nl = syntax.lines[ix - 1].range.end;
        // The newline shows as a space: one byte for one byte.
        out.push(" ", nl..nl + 1, true, 0, Role::Text);
        let base = out.text.len();
        out.text.push_str(&next.text);
        for seg in next.segs {
            out.segs.push(Seg {
                display: seg.display.start + base..seg.display.end + base,
                source: seg.source,
                copy: seg.copy,
            });
        }
        for run in next.runs {
            match out.runs.last_mut() {
                Some(r) if r.style == run.style && r.role == run.role => r.len += run.len,
                _ => out.runs.push(run),
            }
        }
    }
    out.source = syntax.lines[first].range.start..syntax.lines[lines.end - 1].range.end;
    out
}

/// The display form of a table cell's text, `text`, which starts at `at`
/// in the document. A cell holds only inline markdown, so anything that
/// would read as a block on its own (`# x`, `- x`) is shown as it is.
pub fn fragment(text: &str, at: usize, reveal: Reveal) -> DisplayLine {
    // Rendered, a `<br>` breaks the line; each stretch between is drawn on
    // its own.
    let breaks = if reveal == Reveal::Hide {
        br_tags(text)
    } else {
        Vec::new()
    };
    if breaks.is_empty() {
        return fragment_piece(text, at, reveal);
    }
    let mut out = DisplayLine {
        source: at..at + text.len(),
        ..Default::default()
    };
    let mut start = 0;
    for br in breaks {
        append(
            &mut out,
            fragment_piece(&text[start..br.start], at + start, reveal),
        );
        out.push("\n", at + br.start..at + br.end, false, 0, Role::Break);
        start = br.end;
    }
    append(&mut out, fragment_piece(&text[start..], at + start, reveal));
    out
}

/// Where the `<br>`s (`<br/>`, `<br />`, any case) are in `text`.
fn br_tags(text: &str) -> Vec<Range<usize>> {
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = lower[from..].find("<br") {
        let start = from + i;
        let rest = &lower[start + 3..];
        let len = ["/>", " />", ">"]
            .iter()
            .find(|end| rest.starts_with(*end))
            .map(|end| 3 + end.len());
        match len {
            Some(n) => {
                out.push(start..start + n);
                from = start + n;
            }
            None => from = start + 3,
        }
    }
    out
}

/// `piece` added to the end of `out`.
fn append(out: &mut DisplayLine, piece: DisplayLine) {
    let base = out.text.len();
    out.text.push_str(&piece.text);
    for seg in piece.segs {
        if seg.display.is_empty() && seg.source.is_empty() {
            continue;
        }
        out.segs.push(Seg {
            display: seg.display.start + base..seg.display.end + base,
            source: seg.source,
            copy: seg.copy,
        });
    }
    for run in piece.runs {
        match out.runs.last_mut() {
            Some(r) if r.style == run.style && r.role == run.role => r.len += run.len,
            _ => out.runs.push(run),
        }
    }
}

fn fragment_piece(text: &str, at: usize, reveal: Reveal) -> DisplayLine {
    let plain = |text: &str| {
        let mut out = DisplayLine {
            source: at..at + text.len(),
            ..Default::default()
        };
        out.push(text, at..at + text.len(), true, 0, Role::Text);
        if out.segs.is_empty() {
            out.segs.push(Seg {
                display: 0..0,
                source: at..at,
                copy: true,
            });
        }
        out
    };
    let s = crate::syntax::parse(text);
    if s.lines.len() != 1 || s.lines[0].kind != LineKind::Paragraph {
        return plain(text);
    }
    let mut out = display_row(text, &s, 0..1, reveal);
    for seg in &mut out.segs {
        seg.source = seg.source.start + at..seg.source.end + at;
    }
    out.source = at..at + text.len();
    out
}

/// Whether a style has any of `flags`.
pub fn has(style: u16, flags: u16) -> bool {
    style & flags != 0
}

pub use style::{BOLD, CODE, IMAGE, ITALIC, LINK, MATH, STRIKE};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parse;

    fn show(text: &str, reveal: Reveal) -> Vec<String> {
        let s = parse(text);
        (0..s.lines.len())
            .map(|i| display_line(text, &s, i, reveal).text)
            .collect()
    }

    #[test]
    fn rendered_lines_hide_markup() {
        let text = "# Title\n\nSome **bold** and [a link](http://x).\n\n- item\n- [ ] task\n> quote\n\n---\n";
        assert_eq!(
            show(text, Reveal::Hide),
            [
                "Title",
                "",
                "Some bold and a link.",
                "",
                "item",
                "task",
                "quote",
                "",
                "",
                ""
            ]
        );
        let s = parse(text);
        assert_eq!(
            display_line(text, &s, 4, Reveal::Hide).gutter,
            Some(Gutter::Bullet)
        );
        assert!(matches!(
            display_line(text, &s, 5, Reveal::Hide).gutter,
            Some(Gutter::Task { done: false, .. })
        ));
        assert_eq!(
            show(text, Reveal::Show)[2],
            "Some **bold** and [a link](http://x)."
        );
    }

    #[test]
    fn offsets_map_both_ways_across_hidden_markup() {
        let text = "a **bold** b";
        let s = parse(text);
        let d = display_line(text, &s, 0, Reveal::Hide);
        assert_eq!(d.text, "a bold b");
        // Before the hidden `**`, inside, after.
        assert_eq!(d.to_display(2), 2);
        assert_eq!(d.to_display(4), 2);
        assert_eq!(d.to_display(8), 6);
        assert_eq!(d.to_display(12), 8);
        // Typing at the edge of hidden opening markup goes inside it.
        assert_eq!(d.to_source(2), 4);
        assert_eq!(d.to_source(6), 10);
        assert_eq!(d.to_source(8), 12);
        // Runs carry the style.
        assert_eq!(
            d.runs[1],
            Run {
                len: 4,
                style: BOLD,
                role: Role::Text
            }
        );
    }

    #[test]
    fn list_markers_move_to_the_margin() {
        let text = "- [x] done
  - nested
    more
3. third";
        let s = parse(text);
        let d = display_line(text, &s, 0, Reveal::Hide);
        assert_eq!(d.text, "done");
        assert_eq!(
            d.gutter,
            Some(Gutter::Task {
                done: true,
                source: 2..6
            })
        );
        // The caret at the start of the text is after the marker.
        assert_eq!(d.to_source(0), 6);
        assert_eq!(d.to_display(0), 0);
        assert_eq!(d.to_display(6), 0);
        let nested = display_line(text, &s, 1, Reveal::Hide);
        assert_eq!(nested.text, "nested");
        assert_eq!(nested.to_source(0), 15);
        // A continuation line drops its indentation too.
        assert_eq!(display_line(text, &s, 2, Reveal::Hide).text, "more");
        let third = display_line(text, &s, 3, Reveal::Hide);
        assert_eq!(third.text, "third");
        assert_eq!(third.gutter, Some(Gutter::Number("3.".into())));
        assert_eq!(display_line(text, &s, 3, Reveal::Show).text, "3. third");
    }

    #[test]
    fn numbers_and_fence_languages_stay() {
        let text = "1. one\n\n```rust\nx\n```";
        let s = parse(text);
        let lines = show(text, Reveal::Hide);
        assert_eq!(lines[0], "one");
        assert_eq!(lines[2], "rust");
        assert_eq!(lines[4], "");
        let d = display_line(text, &s, 2, Reveal::Hide);
        assert_eq!(d.runs[0].role, Role::Lang);
    }

    #[test]
    fn hard_wrapped_paragraphs_flow_as_one_row() {
        let text = "one\ntwo\n\n- item\n  more\n- next\n\nbreak\\\nhere\n> a\n> b";
        let s = parse(text);
        let rows = rows(text, &s, true, None);
        let shown: Vec<String> = rows
            .iter()
            .map(|r| display_row(text, &s, r.lines.clone(), Reveal::Hide).text)
            .collect();
        assert_eq!(
            shown,
            [
                "one two",
                "",
                "item more",
                "next",
                "",
                "break",
                "here",
                "a b"
            ]
        );
        assert_eq!(super::rows(text, &s, false, None).len(), s.lines.len());
        // The joining space maps to the newline, both ways.
        let d = display_row(text, &s, rows[0].lines.clone(), Reveal::Hide);
        assert_eq!(d.to_source(3), 3);
        assert_eq!(d.to_source(4), 4);
        assert_eq!(d.to_display(4), 4);
        assert_eq!(d.to_display(7), 7);
        assert_eq!(d.source, 0..7);
    }

    #[test]
    fn a_cell_is_drawn_inline_and_mapped_into_the_document() {
        let d = fragment("**b** \\| c", 10, Reveal::Hide);
        assert_eq!(d.text, "b | c");
        assert_eq!(d.source, 10..20);
        assert_eq!(d.to_source(0), 12, "inside the bold markup");
        assert_eq!(d.to_display(17), 2, "the escaped pipe");
        assert_eq!(
            fragment("# not a heading", 0, Reveal::Hide).text,
            "# not a heading"
        );
        let empty = fragment("", 5, Reveal::Hide);
        assert_eq!((empty.text.as_str(), empty.to_source(0)), ("", 5));
    }

    #[test]
    fn a_br_in_a_cell_breaks_the_line() {
        let d = fragment("one<br>**two**", 10, Reveal::Hide);
        assert_eq!(d.text, "one\ntwo");
        assert_eq!(
            d.to_source(3),
            13,
            "the end of the first line is before the tag"
        );
        assert_eq!(d.to_display(13), 3);
        assert_eq!(d.to_source(4), 19, "the second line starts inside the bold");
        assert_eq!(d.role_at(3), Role::Break);
        assert_eq!(
            d.source_at(3),
            Some(13..17),
            "deleting it takes the whole tag"
        );
        assert_eq!(fragment("a<BR/>b<br />c", 0, Reveal::Hide).text, "a\nb\nc");
        assert_eq!(
            fragment("a<br>b", 0, Reveal::Show).text,
            "a<br>b",
            "shown as written"
        );
        assert_eq!(fragment("<bread>", 0, Reveal::Hide).text, "<bread>");
    }

    #[test]
    fn widgets_close_into_one_row_and_open_around_the_caret() {
        let text = "intro\n\n```mermaid\ngraph TD\n```\n\n| a |\n|---|\n| 1 |";
        let s = parse(text);
        let closed = rows(text, &s, true, Some(0));
        let w: Vec<_> = closed.iter().map(|r| (r.lines.clone(), r.widget)).collect();
        assert_eq!(
            w,
            [
                (0..1, None),
                (1..2, None),
                (2..5, Some(RowWidget::Closed(0))),
                (5..6, None),
                (6..9, Some(RowWidget::Closed(1))),
            ]
        );
        // The caret in the diagram: its lines show, the last with the preview.
        let open = rows(text, &s, true, Some(3));
        assert_eq!(open[2].widget, None);
        assert_eq!(open[4].lines, 4..5);
        assert_eq!(open[4].widget, Some(RowWidget::Preview(0)));
        // In a table: still the grid, which is where it's edited.
        let table = rows(text, &s, true, Some(7));
        assert_eq!(table.last().unwrap().widget, Some(RowWidget::Closed(1)));
        // Source mode never draws widgets.
        assert!(
            rows(text, &s, false, None)
                .iter()
                .all(|r| r.widget.is_none())
        );
    }

    #[test]
    fn an_empty_line_maps_to_itself() {
        let text = "a\n\nb";
        let s = parse(text);
        let d = display_line(text, &s, 1, Reveal::Hide);
        assert_eq!(d.text, "");
        assert_eq!(d.to_source(0), 2);
        assert_eq!(d.to_display(2), 0);
    }
}
