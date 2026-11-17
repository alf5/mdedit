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
    /// A frontmatter key, `title:`.
    pub const KEY: u16 = 1 << 7;
    /// An Obsidian `#tag`.
    pub const TAG: u16 = 1 << 8;
    /// A `[[wiki link]]`.
    pub const WIKI: u16 = 1 << 9;
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
    /// The YAML frontmatter opening a document, its `---` fences included:
    /// drawn as a card of properties.
    Meta,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkupKind {
    /// Emphasis and link delimiters, escapes.
    Inline,
    /// An inline code span's backticks: drawn as a little padding inside
    /// its chip when hidden.
    CodeTick,
    /// An inline formula's `$`s.
    MathDelim,
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

/// A callout's kind, as ZenNotes groups them by colour: GitHub's alerts
/// (`> [!NOTE]` and friends) and Obsidian's callouts (`> [!info]`, `> [!bug]
/// Title`, …) alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alert {
    /// Blue: note, info, abstract, summary, tldr, todo, and anything else.
    Note,
    /// Green: tip, hint, important, success, check, done.
    Tip,
    /// Purple: question, help, faq, example.
    Question,
    /// Yellow: warning, warn, caution, attention.
    Warning,
    /// Red: danger, error, bug, failure, fail, missing.
    Danger,
    /// Grey: quote, cite.
    Quote,
}

impl Alert {
    /// The group a callout's type word belongs to.
    pub fn of(word: &str) -> Alert {
        match word.to_ascii_lowercase().as_str() {
            "tip" | "hint" | "important" | "success" | "check" | "done" => Alert::Tip,
            "question" | "help" | "faq" | "example" => Alert::Question,
            "warning" | "warn" | "caution" | "attention" => Alert::Warning,
            "danger" | "error" | "bug" | "failure" | "fail" | "missing" => Alert::Danger,
            "quote" | "cite" => Alert::Quote,
            _ => Alert::Note,
        }
    }
}

/// Where a callout's `[!type]` is at the start of `s`, and its type word:
/// `[!info]`, `[!faq]-` (Obsidian's fold mark is taken along).
pub fn callout_marker(s: &str) -> Option<(usize, &str)> {
    let rest = s.strip_prefix("[!")?;
    let close = rest.find(']')?;
    let word = &rest[..close];
    let ok = word.starts_with(|c: char| c.is_ascii_alphabetic())
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !ok {
        return None;
    }
    let mut end = 2 + close + 1;
    if s[end..].starts_with(['+', '-']) {
        end += 1;
    }
    Some((end, word))
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
    /// A paragraph that is only an image: `![alt](url)`, or Obsidian's
    /// `![[url]]` (`wiki`), which is found by name like a wiki link. `size`
    /// is a width and maybe a height in pixels, from `|300` or `|300x200`.
    Image {
        url: String,
        alt: String,
        wiki: bool,
        size: Option<(u32, Option<u32>)>,
    },
    /// A paragraph that is only Obsidian's `![[Note]]` or `![[Note#Part]]`
    /// (or `![[file.pdf]]`): the note, or a part of it, drawn in place.
    Embed { target: String },
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
    /// A `[[wiki link]]`: `url` names a note.
    pub wiki: bool,
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
    /// Inline formulas, `$…$` (and `$$…$$` within a paragraph), their
    /// delimiters included.
    pub maths: Vec<Range<usize>>,
}

pub fn options() -> Options {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_GFM);
    opts.insert(Options::ENABLE_MATH);
    opts.insert(Options::ENABLE_WIKILINKS);
    opts
}

/// Where a document's frontmatter ends: the YAML block that may open it,
/// between `---` lines, as Jekyll and Obsidian read it. The offset is past
/// the closing line and its newline. Only at the very start: pulldown-cmark's
/// own option takes a `---` pair anywhere, which would swallow a rule and
/// the setext heading after it.
pub fn frontmatter_end(text: &str) -> Option<usize> {
    let first = text.lines().next()?;
    if first.trim_end() != "---" {
        return None;
    }
    let mut at = first.len() + 1;
    if text.as_bytes().get(first.len()) == Some(&b'\r') {
        at += 1;
    }
    for line in text.get(at..)?.split_inclusive('\n') {
        at += line.len();
        let t = line.trim_end();
        if t == "---" || t == "..." {
            return Some(at.min(text.len()));
        }
    }
    None
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
    image: Option<(String, String, bool)>,
    alt: String,
    /// An image written `![[…]]`.
    wiki: bool,
}

impl Syntax {
    /// The `#tags` drawn as pills, without their `#`, in order.
    pub fn tags<'a>(&self, text: &'a str) -> Vec<&'a str> {
        self.spans
            .iter()
            .filter(|s| s.style & style::TAG != 0)
            .map(|s| &text[s.range.start + 1..s.range.end])
            .collect()
    }

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
    let mut maths = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut heading_text: Option<(u8, usize, String)> = None;
    // Obsidian callouts' title lines, which share a paragraph with the text
    // after them but are drawn on their own.
    let mut callout_heads: Vec<usize> = Vec::new();

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

    // The frontmatter is a card of its own; the markdown starts after it.
    let base = frontmatter_end(text).unwrap_or(0);
    if base > 0 {
        let first = 0;
        let last = line_at(base - 1, &lines);
        for l in &mut lines[first..=last] {
            l.kind = LineKind::Meta;
        }
        // The fences are hidden; each `key:` is set apart from its value.
        for f in [first, last] {
            markup.push(Markup {
                range: lines[f].range.clone(),
                kind: MarkupKind::Fence,
            });
        }
        for l in lines.iter().take(last).skip(first + 1) {
            if let Some(key) = meta_key(&text[l.range.clone()]) {
                spans.push(Span {
                    range: l.range.start + key.start..l.range.start + key.end,
                    style: style::KEY,
                });
            }
        }
    }
    let events = Parser::new_ext(&text[base..], options())
        .into_offset_iter()
        .map(|(e, r)| (e, r.start + base..r.end + base));
    for (event, r) in events {
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
                    wiki: false,
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
                        // A GitHub alert, or an Obsidian callout: `[!type]`
                        // opening the quote's first line.
                        let lt = &text[r.start..lines[first].range.end];
                        let after = lt.trim_start_matches('>').trim_start_matches([' ', '\t']);
                        let at = r.start + (lt.len() - after.len());
                        let marker = callout_marker(after);
                        let alert = match (kind, marker) {
                            (Some(k), _) => Some(match k {
                                BlockQuoteKind::Note => Alert::Note,
                                BlockQuoteKind::Tip | BlockQuoteKind::Important => Alert::Tip,
                                BlockQuoteKind::Warning | BlockQuoteKind::Caution => Alert::Warning,
                            }),
                            (None, Some((_, word))) => Some(Alert::of(word)),
                            (None, None) => None,
                        };
                        for l in &mut lines[first..=last] {
                            l.quote += 1;
                            if alert.is_some() {
                                l.alert = alert;
                            }
                        }
                        if marker.is_some() && kind.is_none() {
                            callout_heads.push(first);
                        }
                        if let (Some(alert), Some((len, _))) = (alert, marker) {
                            // With a title of its own after it, the marker's
                            // spaces go too.
                            let rest = &after[len..];
                            let len = if rest.trim().is_empty() {
                                len
                            } else {
                                len + (rest.len() - rest.trim_start().len())
                            };
                            markup.push(Markup {
                                range: at..at + len,
                                kind: MarkupKind::Alert(alert),
                            });
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
                        open.style = if matches!(link_type, LinkType::WikiLink { .. }) {
                            style::LINK | style::WIKI
                        } else {
                            style::LINK
                        };
                        open.delimited = true;
                        open.autolink = matches!(link_type, LinkType::Autolink | LinkType::Email);
                        open.url = Some(dest_url.to_string());
                    }
                    Tag::Image {
                        dest_url,
                        link_type,
                        ..
                    } => {
                        open.style = style::IMAGE;
                        open.delimited = true;
                        open.url = Some(dest_url.to_string());
                        open.wiki = matches!(link_type, LinkType::WikiLink { .. });
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
                    parent.image = Some((
                        open.url.clone().unwrap_or_default(),
                        open.alt.clone(),
                        open.wiki,
                    ));
                }
                if open.paragraph
                    && open.kids == 1
                    && let Some((url, _, true)) = &open.image
                    && crate::project::image_mime(std::path::Path::new(url)).is_none()
                {
                    // `![[Note]]` embeds a note, not a picture.
                    let first = line_at(open.range.start, &lines);
                    let last = line_at(
                        open.range.end.saturating_sub(1).max(open.range.start),
                        &lines,
                    );
                    widgets.push(Widget {
                        lines: first..last + 1,
                        kind: WidgetKind::Embed {
                            target: url.clone(),
                        },
                    });
                } else if open.paragraph
                    && open.kids == 1
                    && let Some((url, alt, wiki)) = open.image.clone()
                {
                    let (alt, size) = image_size(&alt, wiki);
                    let alt = if wiki && (alt.is_empty() || alt == url) {
                        String::new()
                    } else {
                        alt.to_string()
                    };
                    let first = line_at(open.range.start, &lines);
                    let last = line_at(
                        open.range.end.saturating_sub(1).max(open.range.start),
                        &lines,
                    );
                    widgets.push(Widget {
                        lines: first..last + 1,
                        kind: WidgetKind::Image {
                            url,
                            alt,
                            wiki,
                            size,
                        },
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
                            && open.style & style::LINK != 0
                        {
                            links.push(Link {
                                range: first..last,
                                url,
                                wiki: open.style & style::WIKI != 0,
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
                // `#tags` in prose (not headings, links or code) are pills.
                let prose = heading_text.is_none()
                    && s & (style::LINK | style::IMAGE) == 0
                    && matches!(
                        lines[line_at(r.start, &lines)].kind,
                        LineKind::Paragraph | LineKind::Table
                    );
                let mut at = r.start;
                if prose {
                    for tag in hashtags(text, r.clone()) {
                        if tag.start > at && s != 0 {
                            spans.push(Span {
                                range: at..tag.start,
                                style: s,
                            });
                        }
                        at = tag.end;
                        spans.push(Span {
                            range: tag,
                            style: s | style::TAG,
                        });
                    }
                }
                if r.end > at && s != 0 {
                    spans.push(Span {
                        range: at..r.end,
                        style: s,
                    });
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
                        kind: MarkupKind::CodeTick,
                    });
                    markup.push(Markup {
                        range: r.end - n..r.end,
                        kind: MarkupKind::CodeTick,
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
                let display = n == 2
                    && stack.last().is_some_and(|top| {
                        top.paragraph && text[top.range.clone()].trim() == text[r.clone()].trim()
                    });
                if display {
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
                    let kind = if display {
                        MarkupKind::Inline
                    } else {
                        maths.push(r.clone());
                        MarkupKind::MathDelim
                    };
                    markup.push(Markup {
                        range: r.start..r.start + n,
                        kind,
                    });
                    markup.push(Markup {
                        range: r.end - n..r.end,
                        kind,
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
    for l in callout_heads {
        para_id += 1;
        lines[l].para = Some(para_id);
    }
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
        maths,
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

/// An image's size in Obsidian's way: `|300` or `|300x200` after its alt
/// text (`![cat|300](a.png)`), or a wiki embed's whole alt
/// (`![[a.png|300]]`). The alt without it, and width and height in pixels.
pub fn image_size(alt: &str, wiki: bool) -> (&str, Option<(u32, Option<u32>)>) {
    let parse = |s: &str| {
        let (w, h) = match s.split_once('x') {
            Some((w, h)) => (w, Some(h)),
            None => (s, None),
        };
        let w = w.trim().parse::<u32>().ok().filter(|w| *w > 0)?;
        let h = match h {
            Some(h) => Some(h.trim().parse::<u32>().ok().filter(|h| *h > 0)?),
            None => None,
        };
        Some((w, h))
    };
    if let Some((rest, tail)) = alt.rsplit_once('|')
        && let Some(size) = parse(tail)
    {
        return (rest.trim_end(), Some(size));
    }
    if wiki && let Some(size) = parse(alt) {
        return ("", Some(size));
    }
    (alt, None)
}

/// The `#tags` in `text[r]`, as ZenNotes finds them: a `#` at the start or
/// after a space, then a letter, then letters, digits, `_`, `-` or `/`.
pub fn hashtags(text: &str, r: Range<usize>) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    for (i, _) in text[r.clone()].match_indices('#') {
        let at = r.start + i;
        if text[..at]
            .chars()
            .next_back()
            .is_some_and(|c| !c.is_whitespace())
        {
            continue;
        }
        let rest = &text[at + 1..r.end];
        if !rest.starts_with(char::is_alphabetic) {
            continue;
        }
        let len: usize = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/'))
            .map(char::len_utf8)
            .sum();
        out.push(at..at + 1 + len);
    }
    out
}

/// Where a frontmatter line's key is, colon included: `title:` in
/// `title: Notes`, `- name:` in a list of maps. A comment line is all key, so
/// it's drawn as quietly. `None` for a value on its own (`  - item`).
fn meta_key(line: &str) -> Option<Range<usize>> {
    let lead = line.len() - line.trim_start().len();
    let rest = &line[lead..];
    if rest.starts_with('#') {
        return Some(lead..line.len());
    }
    let item = if rest.starts_with("- ") { 2 } else { 0 };
    let body = &rest[item..];
    let colon = body
        .find(": ")
        .or_else(|| body.ends_with(':').then(|| body.len() - 1))?;
    let key = &body[..colon];
    let quoted = key.starts_with('"') || key.starts_with('\'');
    (!key.is_empty() && (quoted || !key.contains(['"', '\'', '[', '{'])))
        .then(|| lead..lead + item + colon + 1)
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
    fn frontmatter_is_a_card_of_keys_and_values() {
        let t = "---\ntitle: Notes\ntags:\n  - a\n# a comment\n---\n\n# Head\n";
        let s = parse(t);
        let kinds: Vec<_> = s.lines.iter().map(|l| l.kind).collect();
        assert!(kinds[..6].iter().all(|k| *k == LineKind::Meta), "{kinds:?}");
        assert_eq!(kinds[7], LineKind::Heading(1));
        assert_eq!(hidden(t)[..2], ["---", "---"]);
        let keys: Vec<_> = s
            .spans
            .iter()
            .filter(|sp| sp.style == style::KEY)
            .map(|sp| &t[sp.range.clone()])
            .collect();
        assert_eq!(keys, ["title:", "tags:", "# a comment"]);
        // Only at the very start: a later `---` pair is a rule and a heading.
        let later = parse("Text\n\n---\ntitle: x\n---\n");
        assert!(later.lines.iter().all(|l| l.kind != LineKind::Meta));
        assert_eq!(later.lines[2].kind, LineKind::Hr);
        assert_eq!(later.lines[3].kind, LineKind::Heading(2));
        // And it must close.
        assert_eq!(frontmatter_end("---\ntitle: x\n"), None);
        assert_eq!(frontmatter_end("---\r\na: 1\r\n...\r\nText"), Some(16));
    }

    #[test]
    fn frontmatter_keys() {
        assert_eq!(meta_key("title: A"), Some(0..6));
        assert_eq!(meta_key("tags:"), Some(0..5));
        assert_eq!(meta_key("  sub: x"), Some(2..6));
        assert_eq!(meta_key("- name: x"), Some(0..7));
        assert_eq!(meta_key("  - item"), None);
        assert_eq!(meta_key("url: https://a.b"), Some(0..4));
        assert_eq!(meta_key("plain"), None);
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
                alt: "a cat".into(),
                wiki: false,
                size: None,
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

    #[test]
    fn images_take_obsidian_sizes_and_embeds() {
        let image = |t: &str| match &parse(t).widgets.first().map(|w| w.kind.clone()) {
            Some(WidgetKind::Image {
                url,
                alt,
                wiki,
                size,
            }) => Some((url.clone(), alt.clone(), *wiki, *size)),
            _ => None,
        };
        assert_eq!(
            image("![[photo.png|300]]"),
            Some(("photo.png".into(), "".into(), true, Some((300, None))))
        );
        assert_eq!(
            image("![[My pic.png|a view|300x200]]"),
            Some((
                "My pic.png".into(),
                "a view".into(),
                true,
                Some((300, Some(200)))
            ))
        );
        assert_eq!(
            image("![cat|200](a.png)"),
            Some(("a.png".into(), "cat".into(), false, Some((200, None))))
        );
        assert_eq!(
            image("![2024](a.png)").unwrap().3,
            None,
            "a number alone is alt text"
        );
        assert_eq!(image("![[Note]]"), None, "a note embed is no picture");
        assert_eq!(
            parse("![[Note#Part]]").widgets[0].kind,
            WidgetKind::Embed {
                target: "Note#Part".into()
            }
        );
    }
}
