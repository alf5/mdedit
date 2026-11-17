//! Formatting and structural edits, as text transforms: each takes the
//! markdown and the selection and returns the [`Change`] to make. Nothing
//! here touches a UI, so every rule is tested in isolation.
//!
//! "Rich" means a rendered mode (Rendered or Live), where Enter starts a new
//! paragraph and Backspace at a list bullet unwraps the item; in Source
//! mode the keys behave as in a plain-text editor, plus list continuation.

use crate::buffer::{Change, Edit, Selection, line_end, line_start, map_pos};

pub const TABLE_MD: &str = "| Column 1 | Column 2 | Column 3 |\n| -------- | -------- | -------- |\n|          |          |          |\n";
pub const MERMAID_MD: &str = "```mermaid\ngraph TD\n  A[Start] --> B{Choice}\n  B -->|yes| C[Do it]\n  B -->|no| D[Stop]\n```\n";
pub const CODE_MD: &str = "```\n\n```\n";
pub const MATH_MD: &str = "$$\n\n$$\n";
pub const RULE_MD: &str = "---\n";

/// A line's block prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prefix {
    None,
    Heading(u8),
    Quote,
    Bullet(char),
    Ordered(u64, char),
    Task(char, bool),
}

/// Leading spaces, the prefix, and its length in bytes (with its space).
pub fn line_prefix(line: &str) -> (usize, Prefix, usize) {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let rest = &line[indent..];
    let hashes = rest.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && indent <= 3 {
        let after = &rest[hashes..];
        if after.is_empty() {
            return (indent, Prefix::Heading(hashes as u8), hashes);
        }
        if after.starts_with(' ') {
            return (indent, Prefix::Heading(hashes as u8), hashes + 1);
        }
    }
    if rest.starts_with('>') && indent <= 3 {
        let len = if rest[1..].starts_with(' ') { 2 } else { 1 };
        return (indent, Prefix::Quote, len);
    }
    if let Some(c) = rest.chars().next().filter(|c| matches!(c, '-' | '*' | '+')) {
        let after = &rest[1..];
        if after.is_empty() || after.starts_with(' ') {
            for (boxed, done) in [("[ ]", false), ("[x]", true), ("[X]", true)] {
                if let Some(tail) = after.strip_prefix(' ').and_then(|a| a.strip_prefix(boxed))
                    && (tail.is_empty() || tail.starts_with(' '))
                {
                    let len = 2 + boxed.len() + usize::from(tail.starts_with(' '));
                    return (indent, Prefix::Task(c, done), len);
                }
            }
            return (
                indent,
                Prefix::Bullet(c),
                1 + usize::from(!after.is_empty()),
            );
        }
    }
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if (1..=9).contains(&digits)
        && let Some(delim) = rest[digits..]
            .chars()
            .next()
            .filter(|c| matches!(c, '.' | ')'))
    {
        let after = &rest[digits + 1..];
        if after.is_empty() || after.starts_with(' ') {
            let n = rest[..digits].parse().unwrap_or(1);
            return (
                indent,
                Prefix::Ordered(n, delim),
                digits + 1 + usize::from(!after.is_empty()),
            );
        }
    }
    (indent, Prefix::None, 0)
}

/// The byte ranges of the lines `sel` touches.
fn lines_of(text: &str, sel: Selection) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut at = line_start(text, sel.start());
    let last = line_start(text, sel.end());
    loop {
        let end = line_end(text, at);
        out.push(at..end);
        if at >= last || end >= text.len() {
            break;
        }
        at = end + 1;
    }
    out
}

/// Map the selection through `edits`. A caret moves past text inserted
/// where it stands; a range keeps text inserted at its edges inside it.
fn change(edits: Vec<Edit>, sel: Selection) -> Change {
    let map = |pos: usize, is_start: bool| map_pos(pos, &edits, sel.is_empty() || !is_start);
    let selection = Selection::new(
        map(sel.anchor, sel.anchor <= sel.head),
        map(sel.head, sel.head < sel.anchor),
    );
    Change { edits, selection }
}

/// Bold, italic, strikethrough, inline code: wrap the selection (each line
/// of it, without its surrounding spaces) in `marker`, or unwrap it if it
/// already is. With nothing selected, either step out of an empty pair or
/// insert one with the caret inside.
pub fn toggle_inline(text: &str, sel: Selection, marker: &str) -> Change {
    let m = marker.len();
    if sel.is_empty() {
        let c = sel.head;
        if text[..c].ends_with(marker) && text[c..].starts_with(marker) {
            let edits = vec![Edit::delete(c - m..c + m)];
            return Change {
                edits,
                selection: Selection::caret(c - m),
            };
        }
        return Change {
            edits: vec![Edit::insert(c, format!("{marker}{marker}"))],
            selection: Selection::caret(c + m),
        };
    }
    let r = sel.range();
    // Already wrapped, just outside or just inside the selection.
    if r.start >= m && text[..r.start].ends_with(marker) && text[r.end..].starts_with(marker) {
        let edits = vec![
            Edit::delete(r.end..r.end + m),
            Edit::delete(r.start - m..r.start),
        ];
        return Change {
            edits,
            selection: Selection::new(
                if sel.reversed() {
                    r.end - m
                } else {
                    r.start - m
                },
                if sel.reversed() {
                    r.start - m
                } else {
                    r.end - m
                },
            ),
        };
    }
    let inner = &text[r.clone()];
    if inner.len() >= 2 * m
        && inner.starts_with(marker)
        && inner.ends_with(marker)
        && !inner.contains('\n')
    {
        let edits = vec![
            Edit::delete(r.end - m..r.end),
            Edit::delete(r.start..r.start + m),
        ];
        return change(edits, sel);
    }
    let mut edits = Vec::new();
    for line in lines_of(text, sel) {
        let s = line.start.max(r.start);
        let e = line.end.min(r.end);
        let piece = &text[s..e];
        let lead = piece.len() - piece.trim_start().len();
        let trail = piece.len() - piece.trim_end().len();
        if piece.trim().is_empty() {
            continue;
        }
        edits.push(Edit::insert(s + lead, marker));
        edits.push(Edit::insert(e - trail, marker));
    }
    let n = edits.len();
    let selection = if n == 2 {
        // One line: keep exactly the wrapped text selected.
        let start = edits[0].range.start + m;
        let end = edits[1].range.start + m;
        if sel.reversed() {
            Selection::new(end, start)
        } else {
            Selection::new(start, end)
        }
    } else {
        Selection::new(
            map_pos(sel.anchor, &edits, false),
            map_pos(sel.head, &edits, true),
        )
    };
    Change { edits, selection }
}

/// Replace each touched line's block prefix with `make(i, old)`; `i` counts
/// non-blank lines. Blank lines are left alone.
fn reprefix(text: &str, sel: Selection, make: impl Fn(usize, Prefix) -> Option<String>) -> Change {
    let mut edits = Vec::new();
    let mut i = 0;
    for line in lines_of(text, sel) {
        let lt = &text[line.clone()];
        if lt.trim().is_empty() && lines_of(text, sel).len() > 1 {
            continue;
        }
        let (indent, prefix, len) = line_prefix(lt);
        let keep_indent = matches!(
            prefix,
            Prefix::Bullet(_) | Prefix::Ordered(..) | Prefix::Task(..)
        );
        let start = line.start + if keep_indent { indent } else { 0 };
        let end = line.start + indent + len;
        if let Some(new) = make(i, prefix)
            && text[start..end] != new
        {
            edits.push(Edit::replace(start..end, new));
        }
        i += 1;
    }
    change(edits, sel)
}

fn all_lines(text: &str, sel: Selection, pred: impl Fn(Prefix) -> bool) -> bool {
    let lines = lines_of(text, sel);
    let many = lines.len() > 1;
    lines
        .iter()
        .filter(|l| !(many && text[(*l).clone()].trim().is_empty()))
        .all(|l| pred(line_prefix(&text[l.clone()]).1))
}

/// Heading 1–6, or a plain paragraph for 0. Setting a line's own level
/// again turns it back into a paragraph.
pub fn set_heading(text: &str, sel: Selection, level: u8) -> Change {
    let level = if level > 0 && all_lines(text, sel, |p| p == Prefix::Heading(level)) {
        0
    } else {
        level
    };
    reprefix(text, sel, |_, _| {
        Some(if level == 0 {
            String::new()
        } else {
            format!("{} ", "#".repeat(level as usize))
        })
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Block {
    Bullet,
    Numbered,
    Task,
    Quote,
}

/// Make the touched lines a bullet, numbered or task list, or a quote; if
/// they all already are, make them plain paragraphs again.
pub fn toggle_block(text: &str, sel: Selection, block: Block) -> Change {
    let is = |p: Prefix| match block {
        Block::Bullet => matches!(p, Prefix::Bullet(_)),
        Block::Numbered => matches!(p, Prefix::Ordered(..)),
        Block::Task => matches!(p, Prefix::Task(..)),
        Block::Quote => p == Prefix::Quote,
    };
    let off = all_lines(text, sel, is);
    reprefix(text, sel, |i, _| {
        Some(if off {
            String::new()
        } else {
            match block {
                Block::Bullet => "- ".into(),
                Block::Numbered => format!("{}. ", i + 1),
                Block::Task => "- [ ] ".into(),
                Block::Quote => "> ".into(),
            }
        })
    })
}

/// The prefix the next line of a list or quote gets.
fn continuation(prefix: Prefix) -> Option<String> {
    Some(match prefix {
        Prefix::Bullet(c) => format!("{c} "),
        Prefix::Ordered(n, d) => format!("{}{d} ", n + 1),
        Prefix::Task(c, _) => format!("{c} [ ] "),
        Prefix::Quote => "> ".into(),
        Prefix::Heading(_) | Prefix::None => return None,
    })
}

/// Enter. `in_code` is set inside a fenced or indented code block, where
/// Enter only keeps the indentation.
pub fn enter(text: &str, sel: Selection, rich: bool, in_code: bool) -> Change {
    let r = sel.range();
    let ls = line_start(text, r.start);
    let le = line_end(text, r.end);
    let line = &text[ls..le];
    let (indent, prefix, len) = line_prefix(line);
    let insert = |s: String| {
        let at = r.start + s.len();
        Change {
            edits: vec![Edit::replace(r.clone(), s)],
            selection: Selection::caret(at),
        }
    };
    if in_code {
        let ws: String = line
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        return insert(format!("\n{ws}"));
    }
    if let Some(next) = continuation(prefix) {
        let content_start = ls + indent + len;
        // An empty item: Enter leaves the list (or quote) instead.
        if sel.is_empty() && text[content_start..le].trim().is_empty() && r.start >= content_start {
            return Change {
                edits: vec![Edit::delete(ls..le)],
                selection: Selection::caret(ls),
            };
        }
        if r.start >= content_start {
            return insert(format!("\n{}{next}", " ".repeat(indent)));
        }
    }
    if rich && !line.trim().is_empty() && r.start > ls {
        return insert("\n\n".into());
    }
    insert("\n".into())
}

/// Shift+Enter: a line break that stays inside the paragraph or list item.
pub fn soft_break(text: &str, sel: Selection) -> Change {
    let r = sel.range();
    let ls = line_start(text, r.start);
    let (indent, prefix, len) = line_prefix(&text[ls..line_end(text, ls)]);
    let pad = match prefix {
        Prefix::Bullet(_) | Prefix::Ordered(..) | Prefix::Task(..) => " ".repeat(indent + len),
        Prefix::Quote => "> ".into(),
        _ => String::new(),
    };
    let s = format!("\n{pad}");
    let at = r.start + s.len();
    Change {
        edits: vec![Edit::replace(r, s)],
        selection: Selection::caret(at),
    }
}

/// Backspace right after a block prefix, in a rendered mode, takes the
/// prefix off (a list item becomes a paragraph) rather than joining the
/// line to the one above. `None` means an ordinary Backspace.
pub fn backspace_prefix(text: &str, sel: Selection) -> Option<Change> {
    if !sel.is_empty() {
        return None;
    }
    let ls = line_start(text, sel.head);
    let (indent, prefix, len) = line_prefix(&text[ls..line_end(text, ls)]);
    if prefix == Prefix::None || sel.head != ls + indent + len {
        return None;
    }
    Some(Change {
        edits: vec![Edit::delete(ls + indent..ls + indent + len)],
        selection: Selection::caret(ls + indent),
    })
}

/// Tab on list items: nest them one level, by their marker's width.
/// `None` when the selection holds no list item.
pub fn indent(text: &str, sel: Selection) -> Option<Change> {
    let mut edits = Vec::new();
    for line in lines_of(text, sel) {
        let (_, prefix, len) = line_prefix(&text[line.clone()]);
        if matches!(
            prefix,
            Prefix::Bullet(_) | Prefix::Ordered(..) | Prefix::Task(..)
        ) {
            let width = match prefix {
                Prefix::Ordered(..) => len,
                _ => 2,
            };
            edits.push(Edit::insert(line.start, " ".repeat(width)));
            // A nested numbered list starts at 1; `renumber` carries it on
            // from there, or from the item above at its new depth.
            if let Prefix::Ordered(n, _) = prefix
                && n != 1
            {
                let (indent, ..) = line_prefix(&text[line.clone()]);
                let digits = n.to_string().len();
                let at = line.start + indent;
                edits.push(Edit::replace(at..at + digits, "1"));
            }
        }
    }
    (!edits.is_empty()).then(|| change(edits, sel))
}

/// Shift+Tab: the reverse, as far as the indentation goes.
pub fn outdent(text: &str, sel: Selection) -> Option<Change> {
    let mut edits = Vec::new();
    for line in lines_of(text, sel) {
        let (indent, prefix, len) = line_prefix(&text[line.clone()]);
        if indent == 0 || prefix == Prefix::None {
            continue;
        }
        let width = match prefix {
            Prefix::Ordered(..) => len,
            _ => 2,
        }
        .min(indent);
        edits.push(Edit::delete(line.start..line.start + width));
    }
    (!edits.is_empty()).then(|| change(edits, sel))
}

/// Insert `block` (ending in a newline) on lines of its own, with a blank
/// line before it unless it starts the document or follows one; the caret
/// lands `caret` bytes into it.
pub fn insert_block(text: &str, sel: Selection, block: &str, caret: usize) -> Change {
    let r = sel.range();
    let ls = line_start(text, r.start);
    let le = line_end(text, r.end);
    let line_blank = text[ls..le].trim().is_empty();
    let (at, mut lead) = if line_blank {
        (ls, String::new())
    } else {
        (le, "\n".to_string())
    };
    let before = &text[..at];
    let needs_gap = !before.is_empty()
        && !before.ends_with("\n\n")
        && !(line_blank && before.ends_with('\n') && before[..before.len() - 1].ends_with('\n'));
    if needs_gap && !(line_blank && ls == 0) {
        lead.push('\n');
    }
    let end = if line_blank { le } else { at };
    let s = format!("{lead}{block}");
    let caret_at = at + lead.len() + caret;
    Change {
        edits: vec![Edit::replace(at..end, s)],
        selection: Selection::caret(caret_at),
    }
}

/// A link around the selection (or the URL itself, with nothing selected).
pub fn insert_link(text: &str, sel: Selection, url: &str, image: bool) -> Change {
    let r = sel.range();
    let label = if sel.is_empty() {
        if image { "" } else { url }
    } else {
        &text[r.clone()]
    };
    let bang = if image { "!" } else { "" };
    let s = format!("{bang}[{label}]({url})");
    let at = r.start + s.len();
    Change {
        edits: vec![Edit::replace(r, s)],
        selection: Selection::caret(at),
    }
}

/// The closer that goes with an opener `auto-pair` knows.
fn closer(open: char) -> Option<char> {
    Some(match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '"' => '"',
        '\'' => '\'',
        '`' => '`',
        _ => return None,
    })
}

/// Typing `ch` with brackets and quotes paired:
/// - with a selection, an opener (or `*`, `_`) wraps it and keeps it selected;
/// - with a caret, an opener inserts its pair around the caret, unless the
///   next character is a word (`(|word`), or for a quote, the previous one is
///   (an apostrophe: `don't`), or for a backquote, it follows another;
/// - a closer that is already the next character is stepped over.
///
/// `None` means type `ch` as it is.
pub fn pair(text: &str, sel: Selection, ch: char) -> Option<Change> {
    let r = sel.range();
    if !sel.is_empty() {
        let close = closer(ch).or(matches!(ch, '*' | '_' | '~').then_some(ch))?;
        if text[r.clone()].contains('\n') {
            return None;
        }
        let o = ch.len_utf8();
        let edits = vec![
            Edit::insert(r.start, ch.to_string()),
            Edit::insert(r.end, close.to_string()),
        ];
        let (a, b) = (r.start + o, r.end + o);
        let selection = if sel.reversed() {
            Selection::new(b, a)
        } else {
            Selection::new(a, b)
        };
        return Some(Change { edits, selection });
    }
    let at = r.start;
    let next = text[at..].chars().next();
    let prev = text[..at].chars().next_back();
    if next == Some(ch) && matches!(ch, ')' | ']' | '}' | '"' | '\'' | '`') {
        return Some(Change {
            edits: vec![],
            selection: Selection::caret(at + ch.len_utf8()),
        });
    }
    let close = closer(ch)?;
    let open_after = next.is_none_or(|n| n.is_whitespace() || ")]}.,;:!?".contains(n));
    if !open_after {
        return None;
    }
    let quote = matches!(ch, '"' | '\'' | '`');
    if quote && prev.is_some_and(|p| p.is_alphanumeric() || p == ch) {
        return None;
    }
    Some(Change {
        edits: vec![Edit::insert(at, format!("{ch}{close}"))],
        selection: Selection::caret(at + ch.len_utf8()),
    })
}

/// Backspace between an empty pair (`(|)`) takes both out.
pub fn unpair(text: &str, sel: Selection) -> Option<Change> {
    if !sel.is_empty() {
        return None;
    }
    let at = sel.head;
    let prev = text[..at].chars().next_back()?;
    let next = text[at..].chars().next()?;
    (closer(prev) == Some(next)).then(|| Change {
        edits: vec![Edit::delete(at - prev.len_utf8()..at + next.len_utf8())],
        selection: Selection::caret(at - prev.len_utf8()),
    })
}

/// The ordered list items around `pos`, at every depth the lines from one
/// line above to one line below touch, numbered again from each list's
/// first number. Items keep their delimiter. A nested list that an indent
/// has just made (its first item is not the list's continuation) starts at
/// 1. Empty when nothing needs changing.
pub fn renumber(text: &str, span: std::ops::Range<usize>) -> Vec<Edit> {
    // Every line, as (start, end, indent, prefix, prefix length).
    let mut lines = Vec::new();
    let mut at = 0;
    loop {
        let end = line_end(text, at);
        let (indent, prefix, len) = line_prefix(&text[at..end]);
        lines.push((at, end, indent, prefix, len));
        if end >= text.len() {
            break;
        }
        at = end + 1;
    }
    let line_of = |pos: usize| lines.partition_point(|l| l.1 < pos).min(lines.len() - 1);
    let first = line_of(span.start).saturating_sub(1);
    let last = (line_of(span.end) + 1).min(lines.len() - 1);
    let blank = |i: usize| text[lines[i].0..lines[i].1].trim().is_empty();
    // Is line `i` part of a list whose items sit at `indent`: an item at
    // that depth, or something nested deeper (a child item, a paragraph
    // under an item)?
    let inside = |i: usize, indent: usize| {
        let (_, _, ind, p, _) = lines[i];
        (ind == indent && matches!(p, Prefix::Ordered(..))) || (ind > indent && !blank(i))
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut edits = Vec::new();
    for i in first..=last {
        let (_, _, indent, Prefix::Ordered(..), _) = lines[i] else {
            continue;
        };
        // Walk up to the list's first item; one blank line between items
        // (a loose list) does not end it.
        let mut top = i;
        let mut j = i;
        while j > 0 {
            let k = j - 1;
            if inside(k, indent) {
                j = k;
            } else if blank(k) && k > 0 && inside(k - 1, indent) {
                j = k - 1;
            } else {
                break;
            }
            if lines[j].2 == indent && matches!(lines[j].3, Prefix::Ordered(..)) {
                top = j;
            }
        }
        if !seen.insert((top, indent)) {
            continue;
        }
        let Prefix::Ordered(start, _) = lines[top].3 else {
            continue;
        };
        let mut n = start;
        let mut j = top;
        while j < lines.len() {
            let (ls, _, ind, p, _) = lines[j];
            if ind == indent
                && let Prefix::Ordered(have, _) = p
            {
                if have != n {
                    let digits = text[ls + ind..]
                        .chars()
                        .take_while(char::is_ascii_digit)
                        .count();
                    edits.push(Edit::replace(ls + ind..ls + ind + digits, format!("{n}")));
                }
                n += 1;
            }
            let k = j + 1;
            if k < lines.len() && inside(k, indent) {
                j = k;
            } else if k + 1 < lines.len() && blank(k) && inside(k + 1, indent) {
                j = k + 1;
            } else {
                break;
            }
        }
    }
    edits
}

/// `change`, then the ordered lists around it renumbered, as one change:
/// a single edit over the stretch that differs, so it undoes in one step.
pub fn then_renumber(text: &str, change: Change) -> Change {
    let mut after = text.to_string();
    crate::buffer::apply(&mut after, &change.edits);
    let span = change
        .edits
        .iter()
        .map(|e| map_pos(e.range.start, &change.edits, false))
        .min()
        .unwrap_or(change.selection.start())
        .min(change.selection.start())..change.selection.end();
    let fix = renumber(&after, span);
    if fix.is_empty() {
        return change;
    }
    let selection = Selection::new(
        map_pos(change.selection.anchor, &fix, true),
        map_pos(change.selection.head, &fix, true),
    );
    crate::buffer::apply(&mut after, &fix);
    Change {
        edits: vec![crate::buffer::diff(text, &after)],
        selection,
    }
}

/// Tick or untick the task box whose `[ ]` / `[x]` starts at `at`.
pub fn toggle_task(text: &str, at: usize) -> Option<Edit> {
    let b = text.get(at..at + 3)?;
    let new = match b {
        "[ ]" => "[x]",
        "[x]" | "[X]" => "[ ]",
        _ => return None,
    };
    Some(Edit::replace(at..at + 3, new))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::apply;

    /// Run `f` on `text` with the selection written as `[` … `]` (or a lone
    /// `|` for a caret) and render the result the same way.
    fn run(marked: &str, f: impl Fn(&str, Selection) -> Change) -> String {
        let (text, sel) = parse_marked(marked);
        let c = f(&text, sel);
        let mut out = text.clone();
        apply(&mut out, &c.edits);
        render_marked(&out, c.selection)
    }

    fn parse_marked(s: &str) -> (String, Selection) {
        if let Some(i) = s.find('|') {
            return (s.replacen('|', "", 1), Selection::caret(i));
        }
        let a = s.find('[').unwrap();
        let t = s.replacen('[', "", 1);
        let b = t.rfind(']').unwrap();
        (t[..b].to_string() + &t[b + 1..], Selection::new(a, b))
    }

    fn render_marked(s: &str, sel: Selection) -> String {
        if sel.is_empty() {
            return format!("{}|{}", &s[..sel.head], &s[sel.head..]);
        }
        let r = sel.range();
        format!("{}[{}]{}", &s[..r.start], &s[r.clone()], &s[r.end..])
    }

    #[test]
    fn prefixes_are_recognised() {
        assert_eq!(line_prefix("## Title"), (0, Prefix::Heading(2), 3));
        assert_eq!(line_prefix("#nope"), (0, Prefix::None, 0));
        assert_eq!(line_prefix("  - item"), (2, Prefix::Bullet('-'), 2));
        assert_eq!(line_prefix("- [x] done"), (0, Prefix::Task('-', true), 6));
        assert_eq!(line_prefix("12. twelve"), (0, Prefix::Ordered(12, '.'), 4));
        assert_eq!(line_prefix("> q"), (0, Prefix::Quote, 2));
        assert_eq!(line_prefix("-"), (0, Prefix::Bullet('-'), 1));
        assert_eq!(line_prefix("-x"), (0, Prefix::None, 0));
    }

    #[test]
    fn inline_marks_wrap_and_unwrap() {
        let bold = |t: &str, s| toggle_inline(t, s, "**");
        assert_eq!(run("a [word ]b", bold), "a **[word]** b");
        assert_eq!(run("a **[word]** b", bold), "a [word] b");
        assert_eq!(run("a [**word**] b", bold), "a [word] b");
        assert_eq!(run("a |b", bold), "a **|**b");
        assert_eq!(run("a **|**b", bold), "a |b");
        assert_eq!(run("[one\ntwo]", bold), "[**one**\n**two**]");
    }

    #[test]
    fn headings_set_and_toggle() {
        assert_eq!(run("tit|le", |t, s| set_heading(t, s, 2)), "## tit|le");
        assert_eq!(run("## tit|le", |t, s| set_heading(t, s, 2)), "tit|le");
        assert_eq!(run("# tit|le", |t, s| set_heading(t, s, 3)), "### tit|le");
        assert_eq!(run("- it|em", |t, s| set_heading(t, s, 1)), "# it|em");
        assert_eq!(run("## tit|le", |t, s| set_heading(t, s, 0)), "tit|le");
    }

    #[test]
    fn lists_and_quotes_toggle() {
        assert_eq!(
            run("[a\nb]", |t, s| toggle_block(t, s, Block::Bullet)),
            "[- a\n- b]"
        );
        assert_eq!(
            run("[- a\n- b]", |t, s| toggle_block(t, s, Block::Bullet)),
            "[a\nb]"
        );
        assert_eq!(
            run("[a\n\nb]", |t, s| toggle_block(t, s, Block::Numbered)),
            "[1. a\n\n2. b]"
        );
        assert_eq!(
            run("- it|em", |t, s| toggle_block(t, s, Block::Task)),
            "- [ ] it|em"
        );
        assert_eq!(
            run("te|xt", |t, s| toggle_block(t, s, Block::Quote)),
            "> te|xt"
        );
    }

    #[test]
    fn enter_continues_and_leaves_lists() {
        assert_eq!(run("- one|", |t, s| enter(t, s, true, false)), "- one\n- |");
        assert_eq!(
            run("  1. one|", |t, s| enter(t, s, true, false)),
            "  1. one\n  2. |"
        );
        assert_eq!(
            run("- [x] done|", |t, s| enter(t, s, true, false)),
            "- [x] done\n- [ ] |"
        );
        assert_eq!(
            run("- one\n- |", |t, s| enter(t, s, true, false)),
            "- one\n|"
        );
        assert_eq!(run("> q|", |t, s| enter(t, s, true, false)), "> q\n> |");
        assert_eq!(
            run("para|graph", |t, s| enter(t, s, true, false)),
            "para\n\n|graph"
        );
        assert_eq!(
            run("para|graph", |t, s| enter(t, s, false, false)),
            "para\n|graph"
        );
        assert_eq!(
            run("    code|", |t, s| enter(t, s, true, true)),
            "    code\n    |"
        );
        assert_eq!(run("|text", |t, s| enter(t, s, true, false)), "\n|text");
    }

    #[test]
    fn soft_breaks_stay_in_the_item() {
        assert_eq!(run("- one|", soft_break), "- one\n  |");
        assert_eq!(run("plain|", soft_break), "plain\n|");
    }

    #[test]
    fn backspace_at_a_prefix_unwraps() {
        let bs = |t: &str, s| {
            backspace_prefix(t, s).unwrap_or(Change {
                edits: vec![],
                selection: s,
            })
        };
        assert_eq!(run("- |item", bs), "|item");
        assert_eq!(run("  - |item", bs), "  |item");
        assert_eq!(run("## |Title", bs), "|Title");
        assert_eq!(run("- it|em", bs), "- it|em");
    }

    #[test]
    fn tab_nests_list_items() {
        let tab = |t: &str, s| indent(t, s).unwrap();
        let untab = |t: &str, s| outdent(t, s).unwrap();
        assert_eq!(run("- a\n- |b", tab), "- a\n  - |b");
        assert_eq!(run("1. a\n2. |b", tab), "1. a\n   1. |b");
        assert_eq!(run("- a\n  - |b", untab), "- a\n- |b");
        assert!(indent("plain", Selection::caret(0)).is_none());
    }

    #[test]
    fn blocks_get_lines_of_their_own() {
        let rule = |t: &str, s| insert_block(t, s, RULE_MD, RULE_MD.len());
        assert_eq!(run("text|", rule), "text\n\n---\n|");
        assert_eq!(run("text\n\n|", rule), "text\n\n---\n|");
        assert_eq!(run("|", rule), "---\n|");
        let code = |t: &str, s| insert_block(t, s, CODE_MD, 4);
        assert_eq!(run("a|", code), "a\n\n```\n|\n```\n");
    }

    #[test]
    fn links_and_tasks() {
        assert_eq!(
            run("see [docs] now", |t, s| insert_link(
                t,
                s,
                "https://x.y",
                false
            )),
            "see [docs](https://x.y)| now"
        );
        assert_eq!(
            run("|", |t, s| insert_link(t, s, "a.png", true)),
            "![](a.png)|"
        );
        let mut t = "- [ ] x".to_string();
        let e = toggle_task(&t, 2).unwrap();
        apply(&mut t, &[e]);
        assert_eq!(t, "- [x] x");
    }

    #[test]
    fn brackets_and_quotes_pair() {
        let ty = |c: char| {
            move |t: &str, s| {
                pair(t, s, c).unwrap_or(Change {
                    edits: vec![Edit::replace(s.range(), c.to_string())],
                    selection: Selection::caret(s.start() + c.len_utf8()),
                })
            }
        };
        assert_eq!(run("a |", ty('(')), "a (|)");
        assert_eq!(run("a (|)", ty(')')), "a ()|");
        assert_eq!(run("|word", ty('(')), "(|word");
        assert_eq!(run("don|", ty('\'')), "don'|");
        assert_eq!(run("say |", ty('"')), "say \"|\"");
        assert_eq!(run("[|]", ty('[')), "[[|]]");
        assert_eq!(run("a [word] b", ty('*')), "a *[word]* b");
        assert_eq!(run("a [word] b", ty('(')), "a ([word]) b");
        assert_eq!(run("a [word] b", ty('x')), "a x| b");
        // Three backquotes make a fence, not a pile of pairs.
        let three = run(&run(&run("|", ty('`')), ty('`')), ty('`'));
        assert_eq!(three, "```|");
        assert_eq!(run("a (|)", |t, s| unpair(t, s).unwrap()), "a |");
        assert!(unpair("a (|x)".replace('|', "").as_str(), Selection::caret(3)).is_none());
    }

    #[test]
    fn numbered_lists_renumber() {
        let fix = |t: &str, s: Selection| then_renumber(t, enter(t, s, true, false));
        assert_eq!(run("1. a|\n2. b\n3. c", fix), "1. a\n2. |\n3. b\n4. c");
        assert_eq!(run("3. a|\n4. b", fix), "3. a\n4. |\n5. b");
        // Taking an item out of the list (Backspace at its marker).
        let unwrap = |t: &str, s| then_renumber(t, backspace_prefix(t, s).unwrap());
        assert_eq!(
            run("1. a\n2. |b\n3. c\n4. d", unwrap),
            "1. a\n|b\n3. c\n4. d"
        );
        // Indenting starts a nested list; outdenting rejoins the outer one.
        let tab = |t: &str, s| then_renumber(t, indent(t, s).unwrap());
        assert_eq!(run("1. a\n2. |b\n3. c", tab), "1. a\n   1. |b\n2. c");
        assert_eq!(
            run("1. a\n   1. x\n3. |b\n4. c", tab),
            "1. a\n   1. x\n   2. |b\n2. c"
        );
        let untab = |t: &str, s| then_renumber(t, outdent(t, s).unwrap());
        assert_eq!(run("1. a\n   1. |b\n2. c", untab), "1. a\n2. |b\n3. c");
        // A loose list, and digits growing past 9.
        assert_eq!(run("8. a|\n\n9. b", fix), "8. a\n9. |\n\n10. b");
        // Nothing to do leaves the change alone.
        let c = then_renumber("x", enter("x", Selection::caret(1), true, false));
        assert_eq!(c.edits.len(), 1);
    }
}
