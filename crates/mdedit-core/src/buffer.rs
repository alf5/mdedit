//! Text, selections and edits: the editor's document model.
//!
//! The markdown text *is* the document. Every change is a list of [`Edit`]s
//! against it, so nothing is ever rebuilt from a rendering and saving can't
//! rewrite what wasn't touched. Offsets are UTF-8 byte offsets, always on
//! character boundaries.

use std::ops::Range;

/// A selection: `anchor` stays put while `head` (the caret) moves. Empty
/// when the two are equal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn caret(at: usize) -> Self {
        Selection {
            anchor: at,
            head: at,
        }
    }

    pub fn new(anchor: usize, head: usize) -> Self {
        Selection { anchor, head }
    }

    pub fn range(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }

    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }

    pub fn reversed(&self) -> bool {
        self.head < self.anchor
    }

    /// Clamped into `0..=len`.
    pub fn clamp(self, len: usize) -> Self {
        Selection {
            anchor: self.anchor.min(len),
            head: self.head.min(len),
        }
    }
}

/// Replace `range` with `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

impl Edit {
    pub fn insert(at: usize, text: impl Into<String>) -> Self {
        Edit {
            range: at..at,
            text: text.into(),
        }
    }

    pub fn delete(range: Range<usize>) -> Self {
        Edit {
            range,
            text: String::new(),
        }
    }

    pub fn replace(range: Range<usize>, text: impl Into<String>) -> Self {
        Edit {
            range,
            text: text.into(),
        }
    }
}

/// A command's result: edits against the current text (non-overlapping, any
/// order) and where the selection lands in the result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub edits: Vec<Edit>,
    pub selection: Selection,
}

/// Apply `edits` to `text`; returns the edits that undo them, against the
/// new text. The edits must not overlap.
pub fn apply(text: &mut String, edits: &[Edit]) -> Vec<Edit> {
    let mut sorted: Vec<&Edit> = edits.iter().collect();
    sorted.sort_by_key(|e| (e.range.start, e.range.end));
    // The inverse, in the new text's coordinates.
    let mut inverse = Vec::with_capacity(sorted.len());
    let mut delta: isize = 0;
    for e in &sorted {
        let start = (e.range.start as isize + delta) as usize;
        inverse.push(Edit {
            range: start..start + e.text.len(),
            text: text[e.range.clone()].to_string(),
        });
        delta += e.text.len() as isize - e.range.len() as isize;
    }
    for e in sorted.iter().rev() {
        text.replace_range(e.range.clone(), &e.text);
    }
    inverse
}

/// Where `pos` ends up after `edits` (against the text they were made for).
/// A position inside a replaced range moves to the end of the replacement
/// when `after` is set, to its start otherwise.
pub fn map_pos(pos: usize, edits: &[Edit], after: bool) -> usize {
    let mut delta: isize = 0;
    let mut sorted: Vec<&Edit> = edits.iter().collect();
    sorted.sort_by_key(|e| e.range.start);
    for e in sorted {
        if e.range.end < pos || (e.range.end == pos && !e.range.is_empty()) {
            delta += e.text.len() as isize - e.range.len() as isize;
        } else if e.range.start <= pos && (pos < e.range.end || (e.range.is_empty() && after)) {
            let start = (e.range.start as isize + delta) as usize;
            return if after { start + e.text.len() } else { start };
        } else {
            break;
        }
    }
    (pos as isize + delta) as usize
}

/// The start of the line holding `pos`.
pub fn line_start(text: &str, pos: usize) -> usize {
    text[..pos].rfind('\n').map_or(0, |i| i + 1)
}

/// The end of the line holding `pos`, before its newline.
pub fn line_end(text: &str, pos: usize) -> usize {
    text[pos..].find('\n').map_or(text.len(), |i| pos + i)
}

/// The previous character boundary, or 0.
pub fn prev_char(text: &str, pos: usize) -> usize {
    text[..pos].char_indices().next_back().map_or(0, |(i, _)| i)
}

/// The next character boundary, or the end.
pub fn next_char(text: &str, pos: usize) -> usize {
    text[pos..]
        .chars()
        .next()
        .map_or(text.len(), |c| pos + c.len_utf8())
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Where Ctrl+Left lands: the start of this word, or the previous one.
pub fn prev_word(text: &str, pos: usize) -> usize {
    let mut it = text[..pos].char_indices().rev().peekable();
    while let Some(&(_, c)) = it.peek() {
        if is_word(c) {
            break;
        }
        it.next();
    }
    let mut at = it.peek().map_or(0, |&(i, _)| i);
    for (i, c) in it {
        if !is_word(c) {
            break;
        }
        at = i;
    }
    at
}

/// Where Ctrl+Right lands: the end of this word, or the next one.
pub fn next_word(text: &str, pos: usize) -> usize {
    let mut it = text[pos..].char_indices().peekable();
    while let Some(&(_, c)) = it.peek() {
        if is_word(c) {
            break;
        }
        it.next();
    }
    for (i, c) in it {
        if !is_word(c) {
            return pos + i;
        }
    }
    text.len()
}

/// The word around `pos`, for double-click.
pub fn word_at(text: &str, pos: usize) -> Range<usize> {
    let start = text[..pos]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(pos, |(i, _)| i);
    let end = text[pos..]
        .char_indices()
        .find(|(_, c)| !is_word(*c))
        .map_or(text.len(), |(i, _)| pos + i);
    start..end
}

/// UTF-16 length of `text[..pos]`, for the platform's input methods.
pub fn utf16_offset(text: &str, pos: usize) -> usize {
    text[..pos].encode_utf16().count()
}

/// The byte offset `utf16` UTF-16 units into `text`.
pub fn from_utf16(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (i, c) in text.char_indices() {
        if units >= utf16 {
            return i;
        }
        units += c.len_utf16();
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_and_inverts_several_edits() {
        let mut text = "hello world".to_string();
        let edits = vec![Edit::replace(6..11, "there"), Edit::insert(0, ">> ")];
        let inverse = apply(&mut text, &edits);
        assert_eq!(text, ">> hello there");
        apply(&mut text, &inverse);
        assert_eq!(text, "hello world");
    }

    #[test]
    fn maps_positions_through_edits() {
        let edits = vec![Edit::insert(2, "XX"), Edit::replace(5..7, "y")];
        assert_eq!(map_pos(0, &edits, false), 0);
        assert_eq!(map_pos(3, &edits, false), 5);
        assert_eq!(map_pos(2, &edits, true), 4);
        assert_eq!(map_pos(2, &edits, false), 2);
        assert_eq!(map_pos(6, &edits, true), 8);
        assert_eq!(map_pos(9, &edits, false), 10);
    }

    #[test]
    fn walks_words_and_lines() {
        let t = "one two\nthree";
        assert_eq!(next_word(t, 0), 3);
        assert_eq!(next_word(t, 3), 7);
        assert_eq!(prev_word(t, 7), 4);
        assert_eq!(prev_word(t, 4), 0);
        assert_eq!(word_at(t, 5), 4..7);
        assert_eq!(line_start(t, 10), 8);
        assert_eq!(line_end(t, 2), 7);
        assert_eq!(line_end(t, 9), t.len());
    }

    #[test]
    fn utf16_round_trips() {
        let t = "a😀é";
        assert_eq!(utf16_offset(t, t.len()), 4);
        assert_eq!(from_utf16(t, 3), 5);
        assert_eq!(from_utf16(t, 1), 1);
        assert_eq!(prev_char(t, 5), 1);
        assert_eq!(next_char(t, 1), 5);
    }
}
