//! The bytes behind the hex editor, and their undo history.
//!
//! Kept free of the DOM so it can be tested natively: everything here is a
//! `Vec<u8>` and an offset. The view in `app.rs` owns the cursor, the nibble
//! and the scroll position; this owns the content and what was done to it.
//!
//! Undo is operation-based, not snapshot-based. The rest of the editor keeps
//! 200 whole-document snapshots per tab, which is fine for markdown and
//! ruinous for a megabyte of binary — 200 MB per tab. An operation records
//! only the bytes it touched.

/// Bytes per displayed row. The view's geometry assumes it, and so does every
/// hex dump anyone has ever read.
pub const ROW: usize = 16;

const HISTORY_LIMIT: usize = 500;

#[derive(Clone)]
enum Op {
    /// `at` held `old`, and now holds `new`. Same length, by construction.
    Overwrite {
        at: usize,
        old: Vec<u8>,
        new: Vec<u8>,
    },
    Insert {
        at: usize,
        bytes: Vec<u8>,
    },
    Delete {
        at: usize,
        bytes: Vec<u8>,
    },
}

impl Op {
    /// Where the cursor belongs after this operation is applied.
    fn cursor_after(&self) -> usize {
        match self {
            Op::Overwrite { at, new, .. } => at + new.len(),
            Op::Insert { at, bytes } => at + bytes.len(),
            Op::Delete { at, .. } => *at,
        }
    }

    /// Where the cursor belongs after this operation is *undone*.
    fn cursor_before(&self) -> usize {
        match self {
            Op::Overwrite { at, .. } | Op::Insert { at, .. } => *at,
            Op::Delete { at, bytes } => at + bytes.len(),
        }
    }
}

#[derive(Clone, Default)]
pub struct HexDoc {
    bytes: Vec<u8>,
    undo: Vec<Op>,
    redo: Vec<Op>,
}

impl HexDoc {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn rows(&self) -> usize {
        // One row even when empty, so the cursor has somewhere to sit.
        self.bytes.len().div_ceil(ROW).max(1)
    }

    pub fn byte(&self, at: usize) -> Option<u8> {
        self.bytes.get(at).copied()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn push(&mut self, op: Op) {
        self.redo.clear();
        self.undo.push(op);
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
    }

    /// Replace the byte at `at`. Out of range is a no-op: overwrite mode never
    /// changes the file's length, so there is nothing past the end to write.
    ///
    /// Two consecutive writes to the same offset collapse into one undo step,
    /// which is what makes typing a byte — high nibble then low — a single
    /// thing to take back rather than two.
    pub fn overwrite(&mut self, at: usize, byte: u8) {
        let Some(slot) = self.bytes.get_mut(at) else {
            return;
        };
        let old = *slot;
        if old == byte {
            return;
        }
        *slot = byte;
        match self.undo.last_mut() {
            Some(Op::Overwrite { at: prev, new, .. }) if *prev == at && new.len() == 1 => {
                new[0] = byte;
                self.redo.clear();
            }
            // An insert followed by editing that same byte is still one step:
            // the low nibble of a byte you just typed in.
            Some(Op::Insert { at: prev, bytes }) if *prev == at && bytes.len() == 1 => {
                bytes[0] = byte;
                self.redo.clear();
            }
            _ => self.push(Op::Overwrite {
                at,
                old: vec![old],
                new: vec![byte],
            }),
        }
    }

    /// Insert a byte, growing the file. `at` may be the end.
    pub fn insert(&mut self, at: usize, byte: u8) {
        let at = at.min(self.bytes.len());
        self.bytes.insert(at, byte);
        self.push(Op::Insert {
            at,
            bytes: vec![byte],
        });
    }

    /// Remove the byte at `at`. False when there is nothing there.
    pub fn delete(&mut self, at: usize) -> bool {
        if at >= self.bytes.len() {
            return false;
        }
        let byte = self.bytes.remove(at);
        self.push(Op::Delete {
            at,
            bytes: vec![byte],
        });
        true
    }

    /// Take back the last operation, returning where the cursor should go.
    pub fn undo(&mut self) -> Option<usize> {
        let op = self.undo.pop()?;
        match &op {
            Op::Overwrite { at, old, .. } => {
                self.bytes[*at..*at + old.len()].copy_from_slice(old);
            }
            Op::Insert { at, bytes } => {
                self.bytes.drain(*at..*at + bytes.len());
            }
            Op::Delete { at, bytes } => {
                let tail = self.bytes.split_off(*at);
                self.bytes.extend_from_slice(bytes);
                self.bytes.extend_from_slice(&tail);
            }
        }
        let cursor = op.cursor_before();
        self.redo.push(op);
        Some(cursor)
    }

    pub fn redo(&mut self) -> Option<usize> {
        let op = self.redo.pop()?;
        match &op {
            Op::Overwrite { at, new, .. } => {
                self.bytes[*at..*at + new.len()].copy_from_slice(new);
            }
            Op::Insert { at, bytes } => {
                let tail = self.bytes.split_off(*at);
                self.bytes.extend_from_slice(bytes);
                self.bytes.extend_from_slice(&tail);
            }
            Op::Delete { at, bytes } => {
                self.bytes.drain(*at..*at + bytes.len());
            }
        }
        let cursor = op.cursor_after();
        self.undo.push(op);
        Some(cursor)
    }

    /// Everything is saved, so nothing before this point is worth an undo
    /// step's memory. Called after a successful write.
    pub fn mark_saved(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

/// `0000abcd`, the offset column.
pub fn offset_label(at: usize) -> String {
    format!("{at:08x}")
}

pub fn hex_byte(b: u8) -> String {
    format!("{b:02x}")
}

/// The ASCII column: printable ASCII as itself, everything else as a dot. Not
/// the byte's Unicode meaning — a hex dump's right-hand column is a crib for
/// spotting strings, and `·` for anything non-printable is the convention.
pub fn ascii_byte(b: u8) -> char {
    if (0x20..0x7f).contains(&b) {
        b as char
    } else {
        '·'
    }
}

/// A hex digit's value, or None if the key wasn't one.
pub fn hex_digit(key: &str) -> Option<u8> {
    let mut chars = key.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    c.to_digit(16).map(|d| d as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overwrite_replaces_without_changing_length() {
        let mut h = HexDoc::new(vec![1, 2, 3]);
        h.overwrite(1, 0xff);
        assert_eq!(h.bytes(), &[1, 0xff, 3]);
        assert_eq!(h.len(), 3);
    }

    /// Overwrite mode must never grow the file, so the end of it is a wall.
    #[test]
    fn overwrite_past_the_end_does_nothing() {
        let mut h = HexDoc::new(vec![1]);
        h.overwrite(1, 0xff);
        h.overwrite(99, 0xff);
        assert_eq!(h.bytes(), &[1]);
        assert!(!h.can_undo());
    }

    #[test]
    fn insert_and_delete_move_the_tail() {
        let mut h = HexDoc::new(vec![1, 2, 3]);
        h.insert(1, 0xaa);
        assert_eq!(h.bytes(), &[1, 0xaa, 2, 3]);
        assert!(h.delete(0));
        assert_eq!(h.bytes(), &[0xaa, 2, 3]);
        assert!(!h.delete(3));
    }

    #[test]
    fn insert_at_the_end_appends() {
        let mut h = HexDoc::new(vec![1]);
        h.insert(1, 2);
        h.insert(99, 3);
        assert_eq!(h.bytes(), &[1, 2, 3]);
    }

    /// Typing one byte is high nibble then low nibble. That is one edit to a
    /// person, so it has to be one press of Ctrl+Z.
    #[test]
    fn the_two_nibbles_of_a_byte_are_one_undo_step() {
        let mut h = HexDoc::new(vec![0x00]);
        h.overwrite(0, 0xa0); // high nibble typed
        h.overwrite(0, 0xab); // low nibble typed
        assert_eq!(h.bytes(), &[0xab]);
        assert_eq!(h.undo(), Some(0));
        assert_eq!(h.bytes(), &[0x00]);
        assert!(!h.can_undo());
    }

    /// Same for a byte typed in insert mode: inserted on the high nibble, then
    /// completed on the low one.
    #[test]
    fn an_inserted_byte_and_its_low_nibble_are_one_undo_step() {
        let mut h = HexDoc::new(vec![0xff]);
        h.insert(0, 0xa0);
        h.overwrite(0, 0xab);
        assert_eq!(h.bytes(), &[0xab, 0xff]);
        assert_eq!(h.undo(), Some(0));
        assert_eq!(h.bytes(), &[0xff]);
    }

    #[test]
    fn edits_at_different_offsets_stay_separate() {
        let mut h = HexDoc::new(vec![0, 0]);
        h.overwrite(0, 1);
        h.overwrite(1, 2);
        assert_eq!(h.bytes(), &[1, 2]);
        h.undo();
        assert_eq!(h.bytes(), &[1, 0]);
        h.undo();
        assert_eq!(h.bytes(), &[0, 0]);
    }

    #[test]
    fn undo_and_redo_round_trip_every_operation() {
        let mut h = HexDoc::new(vec![1, 2, 3]);
        h.overwrite(0, 9);
        h.insert(3, 4);
        h.delete(1);
        assert_eq!(h.bytes(), &[9, 3, 4]);

        assert_eq!(h.undo(), Some(2));
        assert_eq!(h.bytes(), &[9, 2, 3, 4]);
        assert_eq!(h.undo(), Some(3));
        assert_eq!(h.bytes(), &[9, 2, 3]);
        assert_eq!(h.undo(), Some(0));
        assert_eq!(h.bytes(), &[1, 2, 3]);
        assert!(!h.can_undo());

        assert_eq!(h.redo(), Some(1));
        assert_eq!(h.bytes(), &[9, 2, 3]);
        assert_eq!(h.redo(), Some(4));
        assert_eq!(h.bytes(), &[9, 2, 3, 4]);
        assert_eq!(h.redo(), Some(1));
        assert_eq!(h.bytes(), &[9, 3, 4]);
        assert!(!h.can_redo());
    }

    #[test]
    fn a_new_edit_discards_the_redo_stack() {
        let mut h = HexDoc::new(vec![1, 2]);
        h.overwrite(0, 9);
        h.undo();
        assert!(h.can_redo());
        h.overwrite(1, 8);
        assert!(!h.can_redo());
    }

    #[test]
    fn rows_cover_every_byte_and_never_reach_zero() {
        assert_eq!(HexDoc::new(vec![]).rows(), 1);
        assert_eq!(HexDoc::new(vec![0; 1]).rows(), 1);
        assert_eq!(HexDoc::new(vec![0; 16]).rows(), 1);
        assert_eq!(HexDoc::new(vec![0; 17]).rows(), 2);
    }

    #[test]
    fn formats_the_columns() {
        assert_eq!(offset_label(0), "00000000");
        assert_eq!(offset_label(0xdeadbeef), "deadbeef");
        assert_eq!(hex_byte(0x0f), "0f");
        assert_eq!(ascii_byte(b'A'), 'A');
        assert_eq!(ascii_byte(0x00), '·');
        assert_eq!(ascii_byte(0x7f), '·');
        assert_eq!(ascii_byte(0x20), ' ');
    }

    #[test]
    fn reads_hex_digits_but_not_other_keys() {
        assert_eq!(hex_digit("0"), Some(0));
        assert_eq!(hex_digit("f"), Some(15));
        assert_eq!(hex_digit("F"), Some(15));
        assert_eq!(hex_digit("g"), None);
        assert_eq!(hex_digit("Enter"), None);
        assert_eq!(hex_digit(""), None);
    }

    #[test]
    fn saving_clears_the_history() {
        let mut h = HexDoc::new(vec![1]);
        h.overwrite(0, 2);
        h.mark_saved();
        assert!(!h.can_undo());
        assert!(!h.can_redo());
        assert_eq!(h.bytes(), &[2]);
    }
}
