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
                } else {
                    seg.source.end
                };
            }
        }
        // Past the last character: the end of the line, after any markup
        // that closes it.
        self.source.end
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
    fn an_empty_line_maps_to_itself() {
        let text = "a\n\nb";
        let s = parse(text);
        let d = display_line(text, &s, 1, Reveal::Hide);
        assert_eq!(d.text, "");
        assert_eq!(d.to_source(0), 2);
        assert_eq!(d.to_display(2), 0);
    }
}
