//! Folding: which lines can fold, and what each hides. A heading folds its
//! section (up to the next heading of its level or above), a list item its
//! nested lines, and a callout everything under its title. Trailing blank
//! lines stay visible, so the spacing before what follows is kept.

use std::ops::Range;

use crate::display::starts_item;
use crate::syntax::{LineKind, Syntax};

/// A foldable block: its head line, which stays drawn, and the lines
/// folding it hides (indices into `Syntax::lines`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fold {
    pub head: usize,
    pub body: Range<usize>,
}

/// Every foldable block, in order of head line.
pub fn folds(syntax: &Syntax) -> Vec<Fold> {
    let lines = &syntax.lines;
    let blank = |i: usize| lines[i].kind == LineKind::Blank;
    // The body without its trailing blank lines; `None` if nothing is left.
    let trim = |head: usize, mut body: Range<usize>| {
        while body.end > body.start && blank(body.end - 1) {
            body.end -= 1;
        }
        (!body.is_empty()).then_some(Fold { head, body })
    };
    let mut out = Vec::new();

    for (n, h) in syntax.headings.iter().enumerate() {
        let mut start = h.line + 1;
        // A setext heading's underline belongs to the head.
        while start < lines.len() && lines[start].kind == LineKind::SetextUnderline {
            start += 1;
        }
        let end = syntax.headings[n + 1..]
            .iter()
            .find(|o| o.level <= h.level)
            .map_or(lines.len(), |o| o.line);
        out.extend(trim(h.line, start..end.max(start)));
    }

    for i in 0..lines.len() {
        let depth = lines[i].list;
        if depth > 0 && starts_item(syntax, i) {
            // Past the item's own text (a paragraph flowing over several
            // lines), then whatever belongs to it.
            let mut start = i + 1;
            while start < lines.len()
                && lines[i].para.is_some()
                && lines[start].para == lines[i].para
            {
                start += 1;
            }
            let mut end = start;
            while end < lines.len()
                && (blank(end)
                    || (lines[end].list > depth)
                    || (lines[end].list == depth && !starts_item(syntax, end)))
                && !(blank(end) && lines.get(end + 1).is_none_or(|l| l.list < depth))
            {
                end += 1;
            }
            out.extend(trim(i, start..end));
        }
        // A callout: the run of lines on it, from its title line.
        if lines[i].alert.is_some() && (i == 0 || lines[i - 1].alert.is_none()) {
            let mut end = i + 1;
            while end < lines.len() && lines[end].alert.is_some() {
                end += 1;
            }
            out.extend(trim(i, i + 1..end));
        }
    }
    out.sort_by_key(|f| f.head);
    out.dedup_by_key(|f| f.head);
    out
}

/// The lines hidden when the blocks headed by `heads` are folded, merged
/// and in order.
pub fn hidden(folds: &[Fold], heads: impl IntoIterator<Item = usize>) -> Vec<Range<usize>> {
    let mut spans: Vec<Range<usize>> = heads
        .into_iter()
        .filter_map(|h| folds.iter().find(|f| f.head == h).map(|f| f.body.clone()))
        .collect();
    spans.sort_by_key(|r| r.start);
    let mut out: Vec<Range<usize>> = Vec::new();
    for r in spans {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parse;

    fn heads(text: &str) -> Vec<(usize, Range<usize>)> {
        folds(&parse(text))
            .into_iter()
            .map(|f| (f.head, f.body))
            .collect()
    }

    #[test]
    fn headings_fold_their_sections() {
        let t = "# A\ntext\n\n## B\nmore\n\n# C\nend";
        assert_eq!(heads(t), vec![(0, 1..5), (3, 4..5), (6, 7..8)]);
        // An empty section has nothing to fold.
        assert_eq!(heads("# A\n\n# B"), vec![]);
        // A setext heading keeps its underline.
        assert_eq!(heads("Title\n=====\nbody"), vec![(0, 2..3)]);
    }

    #[test]
    fn list_items_fold_their_children() {
        let t = "- a\n  - b\n  - c\n- d\n";
        assert_eq!(heads(t), vec![(0, 1..3)]);
        // A loose list: the blank line between items stays outside.
        let t = "1. a\n\n   more\n\n2. b";
        assert_eq!(heads(t), vec![(0, 1..3)]);
    }

    #[test]
    fn callouts_fold_under_their_title() {
        let t = "> [!note] Title\n> one\n> two\n\nafter";
        assert_eq!(heads(t), vec![(0, 1..3)]);
    }

    #[test]
    fn folded_spans_merge() {
        let f = folds(&parse("# A\n## B\nx\n## C\ny"));
        assert_eq!(hidden(&f, [1, 0]), vec![1..5]);
        assert_eq!(hidden(&f, [1, 3]), vec![2..3, 4..5]);
        assert_eq!(hidden(&f, [7]), vec![]);
    }
}
