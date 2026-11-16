//! What the editor needs to know about the markdown it's drawing: for each
//! source line, what kind of block it belongs to; the inline styles; and
//! every range of *markup* (`**`, `# `, `> `, a link's `](url)`) that the
//! rendered modes hide.
//!
//! Built from pulldown-cmark's source offsets, so it agrees with how the
//! document exports. The editor draws source lines one by one, styled by
//! this, which is what lets the markdown stay the document.

use std::ops::Range;

use pulldown_cmark::{
    BlockQuoteKind, CodeBlockKind, Event, LinkType, Options, Parser, Tag, TagEnd,
};

/// Inline style flags.
pub mod style {
    pub const BOLD: u16 = 1;
    pub const ITALIC: u16 = 1 << 1;
    pub const STRIKE: u16 = 1 << 2;
    pub const CODE: u16 = 1 << 3;
    pub const LINK: u16 = 1 << 4;
    pub const IMAGE: u16 = 1 << 5;
    pub const MATH: u16 = 1 << 6;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Blank,
    Paragraph,
    Heading(u8),
    /// The `===` / `---` under a setext heading.
    SetextUnderline,
    FenceOpen,
    FenceClose,
    Code,
    Hr,
    Table,
    Html,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkupKind {
    /// Emphasis, code and link delimiters, escapes.
    Inline,
    HeadingPrefix,
    QuotePrefix,
    /// `- `, `* `, `+ `: drawn as a bullet when hidden.
    Bullet,
    /// `1. `: stays visible, it says something.
    Number,
    /// `[ ] ` / `[x] `: drawn as a checkbox when hidden.
    TaskBox(bool),
    /// A code fence's backticks, or a whole closing fence.
    Fence,
    /// A thematic break's `---`: drawn as a rule.
    Rule,
    /// An alert's `[!NOTE]`: drawn as its title.
    Alert(Alert),
}

/// GitHub's alert kinds, `> [!NOTE]` and friends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alert {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl Alert {
    pub fn title(self) -> &'static str {
        match self {
            Alert::Note => "Note",
            Alert::Tip => "Tip",
            Alert::Important => "Important",
            Alert::Warning => "Warning",
            Alert::Caution => "Caution",
        }
    }
}

/// A block the rendered modes draw as something other than text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WidgetKind {
    /// A pipe table (with its `zen:cols` comment, if one follows).
    Table,
    /// A ```` ```mermaid ```` fence; `body` is the diagram source.
    Diagram { body: Range<usize> },
    /// A paragraph that is only `$$…$$`; `body` is the formula.
    Math { body: Range<usize> },
    /// A paragraph that is only an image.
    Image { url: String, alt: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Widget {
    /// The source lines it covers.
    pub lines: Range<usize>,
    pub kind: WidgetKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Markup {
    pub range: Range<usize>,
    pub kind: MarkupKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub style: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// The link's visible text.
    pub range: Range<usize>,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// Without its newline.
    pub range: Range<usize>,
    pub kind: LineKind,
    /// Blockquote nesting.
    pub quote: u8,
    /// List nesting, counting the item this line belongs to.
    pub list: u8,
    /// A fence's info string, on its opening line.
    pub lang: Option<Range<usize>>,
    /// The paragraph (or tight list item's text) this line is part of.
    /// Lines sharing one flow together in the rendered modes.
    pub para: Option<u32>,
    pub alert: Option<Alert>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    pub level: u8,
    /// The heading's first line.
    pub line: usize,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Syntax {
    pub lines: Vec<Line>,
    pub spans: Vec<Span>,
    pub markup: Vec<Markup>,
    pub links: Vec<Link>,
    pub headings: Vec<Heading>,
    pub widgets: Vec<Widget>,
}

pub fn options() -> Options {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_GFM);
    opts.insert(Options::ENABLE_MATH);
    opts
}

struct Open {
    style: u16,
    range: Range<usize>,
    first_child: Option<usize>,
    last_child_end: Option<usize>,
    /// Inline element whose delimiters become markup.
    delimited: bool,
    autolink: bool,
    url: Option<String>,
    paragraph: bool,
    /// Direct children seen, and the image if the only one was an image.
    kids: u32,
    image: Option<(String, String)>,
    alt: String,
}

impl Syntax {
    /// The line holding byte `pos`.
    pub fn line_at(&self, pos: usize) -> usize {
        match self.lines.binary_search_by(|l| l.range.start.cmp(&pos)) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
    }

    /// Markup overlapping `range`, in order.
    pub fn markup_in(&self, range: Range<usize>) -> impl Iterator<Item = &Markup> {
        let first = self.markup.partition_point(|m| m.range.end <= range.start);
        self.markup[first..]
            .iter()
            .take_while(move |m| {
                m.range.start < range.end || (m.range.is_empty() && m.range.start == range.end)
            })
            .filter(move |m| m.range.end > range.start || m.range.start == range.start)
    }

    /// Spans overlapping `range`, in order.
    pub fn spans_in(&self, range: Range<usize>) -> impl Iterator<Item = &Span> {
        let first = self.spans.partition_point(|s| s.range.end <= range.start);
        self.spans[first..]
            .iter()
            .take_while(move |s| s.range.start < range.end)
    }

    /// Outline numbers for the headings, `1.`, `1.1.`, … counted from the
    /// shallowest level used, keyed by heading line.
    pub fn heading_numbers(&self) -> std::collections::HashMap<usize, String> {
        let top = self.headings.iter().map(|h| h.level).min().unwrap_or(1);
        let mut counters = [0usize; 6];
        let mut out = std::collections::HashMap::new();
        for h in &self.headings {
            let depth = (h.level - top) as usize;
            counters[depth] += 1;
            for c in counters.iter_mut().skip(depth + 1) {
                *c = 0;
            }
            let n: Vec<String> = counters[..=depth].iter().map(|c| c.to_string()).collect();
            out.insert(h.line, format!("{}.", n.join(".")));
        }
        out
    }

    /// The widget whose source lines hold line `line`.
    pub fn widget_at(&self, line: usize) -> Option<&Widget> {
        self.widgets.iter().find(|w| w.lines.contains(&line))
    }

    /// The link whose text holds `pos`.
    pub fn link_at(&self, pos: usize) -> Option<&Link> {
        self.links
            .iter()
            .find(|l| l.range.start <= pos && pos < l.range.end)
    }
}

pub fn parse(text: &str) -> Syntax {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            lines.push(start..i);
            start = i + 1;
        }
    }
    lines.push(start..text.len());
    let mut lines: Vec<Line> = lines
        .into_iter()
        .map(|range| Line {
            kind: if text[range.clone()].trim().is_empty() {
                LineKind::Blank
            } else {
                LineKind::Paragraph
            },
            range,
            quote: 0,
            list: 0,
            lang: None,
            para: None,
            alert: None,
        })
        .collect();
    let mut widgets: Vec<Widget> = Vec::new();
    let mut para_id = 0u32;
    let line_at =
        |pos: usize, lines: &[Line]| match lines.binary_search_by(|l| l.range.start.cmp(&pos)) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };

    let mut spans = Vec::new();
    let mut markup = Vec::new();
    let mut links = Vec::new();
    let mut headings = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut heading_text: Option<(u8, usize, String)> = None;

    let count = |stack: &mut Vec<Open>| {
        if let Some(top) = stack.last_mut() {
            top.kids += 1;
        }
    };
    let touch = |stack: &mut Vec<Open>, r: &Range<usize>| {
        if let Some(top) = stack.last_mut() {
            top.first_child.get_or_insert(r.start);
            top.last_child_end = Some(top.last_child_end.map_or(r.end, |e| e.max(r.end)));
        }
    };
    let style_now = |stack: &[Open]| stack.iter().fold(0, |acc, o| acc | o.style);

    for (event, r) in Parser::new_ext(text, options()).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                touch(&mut stack, &r);
                count(&mut stack);
                let mut open = Open {
                    style: 0,
                    range: r.clone(),
                    first_child: None,
                    last_child_end: None,
                    delimited: false,
                    autolink: false,
                    url: None,
                    paragraph: false,
                    kids: 0,
                    image: None,
                    alt: String::new(),
                };
                let (first, last) = (
                    line_at(r.start, &lines),
                    line_at(r.end.saturating_sub(1).max(r.start), &lines),
                );
                match tag {
                    Tag::Heading { level, .. } => {
                        let level = level as u8;
                        for l in &mut lines[first..=last] {
                            l.kind = LineKind::Heading(level);
                        }
                        // A setext heading's underline is its last line.
                        let last_text = text[lines[last].range.clone()].trim();
                        if last > first
                            && (last_text.chars().all(|c| c == '=')
                                || last_text.chars().all(|c| c == '-'))
                        {
                            lines[last].kind = LineKind::SetextUnderline;
                        }
                        heading_text = Some((level, first, String::new()));
                        open.delimited = true;
                    }
                    Tag::CodeBlock(kind) => {
                        for l in &mut lines[first..=last] {
                            l.kind = LineKind::Code;
                        }
                        if let CodeBlockKind::Fenced(info) = &kind
                            && info
                                .split_whitespace()
                                .next()
                                .is_some_and(|w| w.eq_ignore_ascii_case("mermaid"))
                        {
                            let body_start = (lines[first].range.end + 1).min(text.len());
                            let closed = last > first && {
                                let ct = text[lines[last].range.clone()].trim();
                                !ct.is_empty() && ct.chars().all(|c| c == '`' || c == '~')
                            };
                            let body_end = if closed {
                                lines[last].range.start
                            } else {
                                r.end
                            };
                            widgets.push(Widget {
                                lines: first..last + 1,
                                kind: WidgetKind::Diagram {
                                    body: body_start..body_end.max(body_start),
                                },
                            });
                        }
                        if let CodeBlockKind::Fenced(_) = kind {
                            let open_line = &mut lines[first];
                            open_line.kind = LineKind::FenceOpen;
                            let lt = &text[open_line.range.clone()];
                            let indent = lt.len() - lt.trim_start().len();
                            let fence_char = lt[indent..].chars().next().unwrap_or('`');
                            let run = lt[indent..]
                                .chars()
                                .take_while(|c| *c == fence_char)
                                .count();
                            let fence = open_line.range.start + indent
                                ..open_line.range.start + indent + run;
                            let info = &text[fence.end..open_line.range.end];
                            let info_trim = info.trim();
                            if !info_trim.is_empty() {
                                let s = fence.end + (info.len() - info.trim_start().len());
                                open_line.lang = Some(s..s + info_trim.len());
                            }
                            markup.push(Markup {
                                range: fence,
                                kind: MarkupKind::Fence,
                            });
                            if last > first {
                                let close = &lines[last];
                                let ct = text[close.range.clone()].trim();
                                if !ct.is_empty()
                                    && ct.chars().all(|c| c == fence_char)
                                    && ct.len() >= run
                                {
                                    lines[last].kind = LineKind::FenceClose;
                                    markup.push(Markup {
                                        range: lines[last].range.clone(),
                                        kind: MarkupKind::Fence,
                                    });
                                }
                            }
                        }
                    }
                    Tag::BlockQuote(kind) => {
                        let alert = kind.map(|k| match k {
                            BlockQuoteKind::Note => Alert::Note,
                            BlockQuoteKind::Tip => Alert::Tip,
                            BlockQuoteKind::Important => Alert::Important,
                            BlockQuoteKind::Warning => Alert::Warning,
                            BlockQuoteKind::Caution => Alert::Caution,
                        });
                        for l in &mut lines[first..=last] {
                            l.quote += 1;
                            if alert.is_some() {
                                l.alert = alert;
                            }
                        }
                        if let Some(alert) = alert {
                            let lt = &text[lines[first].range.clone()];
                            if let Some(i) = lt.find("[!") {
                                let end = lt[i..].find(']').map_or(lt.len(), |j| i + j + 1);
                                let s = lines[first].range.start;
                                markup.push(Markup {
                                    range: s + i..s + end,
                                    kind: MarkupKind::Alert(alert),
                                });
                            }
                        }
                    }
                    Tag::Item => {
                        para_id += 1;
                        for l in &mut lines[first..=last] {
                            l.list += 1;
                            // A tight item's text has no paragraph of its
                            // own; nested items and paragraphs re-mark theirs.
                            l.para = Some(para_id);
                        }
                        let rest = &text[r.start..lines[first].range.end];
                        let marker_len = if rest.starts_with(['-', '*', '+']) {
                            1
                        } else {
                            rest.find(['.', ')']).map_or(0, |i| i + 1)
                        };
                        let spaces = rest[marker_len..]
                            .chars()
                            .take_while(|c| *c == ' ')
                            .count()
                            .min(4);
                        let kind = if rest.starts_with(['-', '*', '+']) {
                            MarkupKind::Bullet
                        } else {
                            MarkupKind::Number
                        };
                        if marker_len > 0 {
                            markup.push(Markup {
                                range: r.start..r.start + marker_len + spaces,
                                kind,
                            });
                        }
                    }
                    Tag::Paragraph => {
                        open.paragraph = true;
                        para_id += 1;
                        for l in &mut lines[first..=last] {
                            l.para = Some(para_id);
                        }
                    }
                    Tag::Table(_) => {
                        for l in &mut lines[first..=last] {
                            l.kind = LineKind::Table;
                        }
                        // The saved column widths ride along with the table.
                        let mut end = last + 1;
                        if let Some(next) = lines.get(last + 1)
                            && crate::table::parse_cols_comment(&text[next.range.clone()]).is_some()
                        {
                            end += 1;
                        }
                        widgets.push(Widget {
                            lines: first..end,
                            kind: WidgetKind::Table,
                        });
                    }
                    Tag::HtmlBlock => {
                        for l in &mut lines[first..=last] {
                            l.kind = LineKind::Html;
                        }
                    }
                    Tag::Emphasis => {
                        open.style = style::ITALIC;
                        open.delimited = true;
                    }
                    Tag::Strong => {
                        open.style = style::BOLD;
                        open.delimited = true;
                    }
                    Tag::Strikethrough => {
                        open.style = style::STRIKE;
                        open.delimited = true;
                    }
                    Tag::Link {
                        link_type,
                        dest_url,
                        ..
                    } => {
                        open.style = style::LINK;
                        open.delimited = true;
                        open.autolink = matches!(link_type, LinkType::Autolink | LinkType::Email);
                        open.url = Some(dest_url.to_string());
                    }
                    Tag::Image { dest_url, .. } => {
                        open.style = style::IMAGE;
                        open.delimited = true;
                        open.url = Some(dest_url.to_string());
                    }
                    _ => {}
                }
                stack.push(open);
            }
            Event::End(tag) => {
                let Some(open) = stack.pop() else { continue };
                touch(&mut stack, &open.range);
                if matches!(tag, TagEnd::Image)
                    && let Some(parent) = stack.last_mut()
                {
                    parent.image = Some((open.url.clone().unwrap_or_default(), open.alt.clone()));
                }
                if open.paragraph
                    && open.kids == 1
                    && let Some((url, alt)) = open.image.clone()
                {
                    let first = line_at(open.range.start, &lines);
                    let last = line_at(
                        open.range.end.saturating_sub(1).max(open.range.start),
                        &lines,
                    );
                    widgets.push(Widget {
                        lines: first..last + 1,
                        kind: WidgetKind::Image { url, alt },
                    });
                }
                if matches!(tag, TagEnd::Heading(_))
                    && let Some((level, line, text)) = heading_text.take()
                {
                    headings.push(Heading {
                        level,
                        line,
                        text: text.trim().to_string(),
                    });
                }
                if !open.delimited {
                    continue;
                }
                let kind = if matches!(tag, TagEnd::Heading(_)) {
                    MarkupKind::HeadingPrefix
                } else {
                    MarkupKind::Inline
                };
                match (open.first_child, open.last_child_end) {
                    (Some(first), Some(last)) => {
                        if first > open.range.start {
                            markup.push(Markup {
                                range: open.range.start..first,
                                kind,
                            });
                        }
                        let end = trim_newline(text, last, open.range.end);
                        if end > last {
                            markup.push(Markup {
                                range: last..end,
                                kind,
                            });
                        }
                        if let Some(url) = open.url
                            && open.style == style::LINK
                        {
                            links.push(Link {
                                range: first..last,
                                url,
                            });
                        }
                    }
                    _ if kind == MarkupKind::HeadingPrefix => {
                        // An empty heading: `#` alone.
                        let end = trim_newline(text, open.range.start, open.range.end);
                        markup.push(Markup {
                            range: open.range.start..end,
                            kind,
                        });
                    }
                    _ => {}
                }
                let _ = open.autolink;
            }
            Event::Text(t) => {
                touch(&mut stack, &r);
                count(&mut stack);
                if let Some(top) = stack.last_mut()
                    && top.style == style::IMAGE
                {
                    top.alt.push_str(&t);
                }
                if let Some((_, _, h)) = heading_text.as_mut() {
                    h.push_str(&t);
                }
                // A backslash escape: pulldown starts the text after it.
                if r.start > 0
                    && text.as_bytes()[r.start - 1] == b'\\'
                    && t.starts_with(|c: char| c.is_ascii_punctuation())
                    && text[r.start..].starts_with(&t[..1])
                    && !(r.start > 1 && text.as_bytes()[r.start - 2] == b'\\')
                {
                    markup.push(Markup {
                        range: r.start - 1..r.start,
                        kind: MarkupKind::Inline,
                    });
                }
                let s = style_now(&stack);
                if s != 0 {
                    spans.push(Span { range: r, style: s });
                }
            }
            Event::Code(t) => {
                touch(&mut stack, &r);
                count(&mut stack);
                if let Some((_, _, h)) = heading_text.as_mut() {
                    h.push_str(&t);
                }
                let n = text[r.clone()].chars().take_while(|c| *c == '`').count();
                if r.len() >= 2 * n {
                    markup.push(Markup {
                        range: r.start..r.start + n,
                        kind: MarkupKind::Inline,
                    });
                    markup.push(Markup {
                        range: r.end - n..r.end,
                        kind: MarkupKind::Inline,
                    });
                    spans.push(Span {
                        range: r.start + n..r.end - n,
                        style: style_now(&stack) | style::CODE,
                    });
                }
            }
            Event::InlineMath(_) | Event::DisplayMath(_) => {
                touch(&mut stack, &r);
                count(&mut stack);
                let n = if matches!(event, Event::DisplayMath(_)) {
                    2
                } else {
                    1
                };
                // A paragraph that is just `$$…$$` is a display formula.
                if n == 2
                    && let Some(top) = stack.last()
                    && top.paragraph
                    && text[top.range.clone()].trim() == text[r.clone()].trim()
                {
                    let first = line_at(r.start, &lines);
                    let last = line_at(r.end.saturating_sub(1).max(r.start), &lines);
                    widgets.push(Widget {
                        lines: first..last + 1,
                        kind: WidgetKind::Math {
                            body: r.start + 2..r.end - 2,
                        },
                    });
                }
                if r.len() >= 2 * n {
                    markup.push(Markup {
                        range: r.start..r.start + n,
                        kind: MarkupKind::Inline,
                    });
                    markup.push(Markup {
                        range: r.end - n..r.end,
                        kind: MarkupKind::Inline,
                    });
                    spans.push(Span {
                        range: r.start + n..r.end - n,
                        style: style::MATH,
                    });
                }
            }
            Event::HardBreak => {
                touch(&mut stack, &r);
                if text.as_bytes().get(r.start) == Some(&b'\\') {
                    markup.push(Markup {
                        range: r.start..r.start + 1,
                        kind: MarkupKind::Inline,
                    });
                }
            }
            Event::Rule => {
                let l = line_at(r.start, &lines);
                lines[l].kind = LineKind::Hr;
                markup.push(Markup {
                    range: lines[l].range.clone(),
                    kind: MarkupKind::Rule,
                });
            }
            Event::TaskListMarker(checked) => {
                touch(&mut stack, &r);
                let end = if text.as_bytes().get(r.end) == Some(&b' ') {
                    r.end + 1
                } else {
                    r.end
                };
                markup.push(Markup {
                    range: r.start..end,
                    kind: MarkupKind::TaskBox(checked),
                });
            }
            _ => {
                touch(&mut stack, &r);
                count(&mut stack);
            }
        }
    }
    widgets.sort_by_key(|w| w.lines.start);

    // Quote prefixes, which pulldown doesn't report per line.
    for line in &lines {
        if line.quote == 0 {
            continue;
        }
        let lt = &text[line.range.clone()];
        let mut at = 0;
        let mut found = 0;
        while found < line.quote {
            let spaces = lt[at..].chars().take_while(|c| *c == ' ').count();
            if spaces > 3 || !lt[at + spaces..].starts_with('>') {
                break;
            }
            at += spaces + 1;
            if lt[at..].starts_with(' ') {
                at += 1;
            }
            found += 1;
        }
        if at > 0 {
            markup.push(Markup {
                range: line.range.start..line.range.start + at,
                kind: MarkupKind::QuotePrefix,
            });
        }
    }

    markup.retain(|m| !m.range.is_empty() || m.kind == MarkupKind::HeadingPrefix);
    markup.sort_by_key(|m| (m.range.start, m.range.end));
    markup.dedup();
    spans.sort_by_key(|s| s.range.start);
    Syntax {
        lines,
        spans,
        markup,
        links,
        headings,
        widgets,
    }
}

/// `end`, pulled back over a trailing newline so markup stays on its line.
fn trim_newline(text: &str, floor: usize, end: usize) -> usize {
    let mut e = end;
    while e > floor && matches!(text.as_bytes()[e - 1], b'\n' | b'\r') {
        e -= 1;
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hidden(text: &str) -> Vec<&str> {
        parse(text)
            .markup
            .iter()
            .map(|m| &text[m.range.clone()])
            .collect()
    }

    #[test]
    fn inline_delimiters_are_markup() {
        assert_eq!(
            hidden("a **b** *c* ~~d~~ `e`"),
            ["**", "**", "*", "*", "~~", "~~", "`", "`"]
        );
        let s = parse("**b** `c`");
        assert_eq!(
            s.spans[0],
            Span {
                range: 2..3,
                style: style::BOLD
            }
        );
        assert_eq!(s.spans[1].style, style::CODE);
    }

    #[test]
    fn links_hide_their_target_and_remember_it() {
        let text = "see [docs](https://x.y) now";
        assert_eq!(hidden(text), ["[", "](https://x.y)"]);
        let s = parse(text);
        assert_eq!(&text[s.links[0].range.clone()], "docs");
        assert_eq!(s.links[0].url, "https://x.y");
        assert!(s.link_at(6).is_some());
    }

    #[test]
    fn headings_and_their_outline() {
        let text = "# One\n\nbody\n\n## Two ##\n";
        let s = parse(text);
        assert_eq!(s.lines[0].kind, LineKind::Heading(1));
        assert_eq!(s.lines[1].kind, LineKind::Blank);
        assert_eq!(s.lines[2].kind, LineKind::Paragraph);
        assert_eq!(hidden(text), ["# ", "## ", " ##"]);
        assert_eq!(
            s.headings,
            vec![
                Heading {
                    level: 1,
                    line: 0,
                    text: "One".into()
                },
                Heading {
                    level: 2,
                    line: 4,
                    text: "Two".into()
                },
            ]
        );
    }

    #[test]
    fn headings_are_numbered_like_an_outline() {
        let s = parse("## A\n\n### B\n\n### C\n\n## D\n\n#### E\n");
        let n = s.heading_numbers();
        let mut v: Vec<_> = n.into_iter().collect();
        v.sort();
        let v: Vec<&str> = v.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(v, ["1.", "1.1.", "1.2.", "2.", "2.0.1."]);
    }

    #[test]
    fn setext_headings_mark_their_underline() {
        let s = parse("Title\n=====\n");
        assert_eq!(s.lines[0].kind, LineKind::Heading(1));
        assert_eq!(s.lines[1].kind, LineKind::SetextUnderline);
    }

    #[test]
    fn lists_quotes_and_tasks() {
        let text = "- a\n- [x] done\n1. one\n\n> quoted\n> > deeper\n";
        let s = parse(text);
        let kinds: Vec<_> = s
            .markup
            .iter()
            .map(|m| (&text[m.range.clone()], m.kind))
            .collect();
        assert_eq!(
            kinds,
            [
                ("- ", MarkupKind::Bullet),
                ("- ", MarkupKind::Bullet),
                ("[x] ", MarkupKind::TaskBox(true)),
                ("1. ", MarkupKind::Number),
                ("> ", MarkupKind::QuotePrefix),
                ("> > ", MarkupKind::QuotePrefix),
            ]
        );
        assert_eq!(s.lines[0].list, 1);
        assert_eq!(s.lines[5].quote, 2);
    }

    #[test]
    fn fences_code_and_rules() {
        let text = "```rust\nfn x() {}\n```\n\n---\n";
        let s = parse(text);
        assert_eq!(s.lines[0].kind, LineKind::FenceOpen);
        assert_eq!(&text[s.lines[0].lang.clone().unwrap()], "rust");
        assert_eq!(s.lines[1].kind, LineKind::Code);
        assert_eq!(s.lines[2].kind, LineKind::FenceClose);
        assert_eq!(s.lines[4].kind, LineKind::Hr);
        assert_eq!(hidden(text), ["```", "```", "---"]);
    }

    #[test]
    fn widgets_and_alerts() {
        let text = "| a | b |\n|---|---|\n| 1 | 2 |\n<!-- zen:cols=80,auto -->\n\n```mermaid\ngraph TD\n  A-->B\n```\n\n$$\nx^2\n$$\n\n![a cat](cat.png)\n\nsee ![inline](x.png) here\n\n> [!WARNING]\n> careful\n";
        let s = parse(text);
        let kinds: Vec<_> = s
            .widgets
            .iter()
            .map(|w| (w.lines.clone(), &w.kind))
            .collect();
        assert_eq!(kinds.len(), 4, "{kinds:?}");
        assert_eq!(kinds[0], (0..4, &WidgetKind::Table));
        match kinds[1].1 {
            WidgetKind::Diagram { body } => assert_eq!(&text[body.clone()], "graph TD\n  A-->B\n"),
            k => panic!("{k:?}"),
        }
        match kinds[2].1 {
            WidgetKind::Math { body } => assert_eq!(text[body.clone()].trim(), "x^2"),
            k => panic!("{k:?}"),
        }
        assert_eq!(
            kinds[3].1,
            &WidgetKind::Image {
                url: "cat.png".into(),
                alt: "a cat".into()
            }
        );
        let alert_line = s.lines.iter().position(|l| l.alert.is_some()).unwrap();
        assert_eq!(s.lines[alert_line].alert, Some(Alert::Warning));
        assert!(
            s.markup
                .iter()
                .any(|m| m.kind == MarkupKind::Alert(Alert::Warning)
                    && &text[m.range.clone()] == "[!WARNING]")
        );
        assert!(s.widget_at(1).is_some());
        assert!(s.widget_at(4).is_none());
    }

    #[test]
    fn escapes_hide_their_backslash() {
        assert_eq!(hidden(r"a \* b"), ["\\"]);
        assert!(hidden("a * b").is_empty());
    }

    #[test]
    fn math_and_tables() {
        let text = "$x^2$ and\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        let s = parse(text);
        assert_eq!(s.spans[0].style, style::MATH);
        assert_eq!(s.lines[2].kind, LineKind::Table);
        assert_eq!(s.lines[4].kind, LineKind::Table);
        assert_eq!(s.line_at(0), 0);
        assert_eq!(s.line_at(text.len()), s.lines.len() - 1);
    }
}
