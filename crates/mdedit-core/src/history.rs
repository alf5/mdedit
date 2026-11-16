//! Undo and redo, as edits rather than snapshots.
//!
//! A burst of typing (keystrokes less than 800 ms apart, with no caret jump
//! in between) undoes as one step, as in v1; everything else is a step of
//! its own. The history keeps the last 200 steps.

use std::time::{Duration, Instant};

use crate::buffer::{Edit, Selection, apply};

pub const LIMIT: usize = 200;
pub const TYPING_BURST: Duration = Duration::from_millis(800);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Typing,
    Other,
}

struct Step {
    forward: Vec<Edit>,
    inverse: Vec<Edit>,
}

struct Transaction {
    steps: Vec<Step>,
    before: Selection,
    after: Selection,
    kind: Kind,
    at: Instant,
    /// No more typing joins this one.
    sealed: bool,
}

#[derive(Default)]
pub struct History {
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
}

impl History {
    /// Record edits that were just applied (`inverse` is what `apply`
    /// returned).
    pub fn record(
        &mut self,
        forward: Vec<Edit>,
        inverse: Vec<Edit>,
        before: Selection,
        after: Selection,
        kind: Kind,
        now: Instant,
    ) {
        self.redo.clear();
        let step = Step { forward, inverse };
        if kind == Kind::Typing
            && let Some(last) = self.undo.last_mut()
            && last.kind == Kind::Typing
            && !last.sealed
            && now.duration_since(last.at) < TYPING_BURST
        {
            last.steps.push(step);
            last.after = after;
            last.at = now;
            return;
        }
        self.undo.push(Transaction {
            steps: vec![step],
            before,
            after,
            kind,
            at: now,
            sealed: false,
        });
        if self.undo.len() > LIMIT {
            self.undo.remove(0);
        }
    }

    /// The caret moved or something else happened: the next keystroke
    /// starts a new step.
    pub fn seal(&mut self) {
        if let Some(last) = self.undo.last_mut() {
            last.sealed = true;
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Undo one step on `text`; returns the selection to restore.
    pub fn undo(&mut self, text: &mut String) -> Option<Selection> {
        let mut t = self.undo.pop()?;
        // Undoing a step returns the edits that redo it, in the coordinates
        // they'll be applied in.
        for step in t.steps.iter_mut().rev() {
            step.forward = apply(text, &step.inverse);
        }
        t.sealed = true;
        let sel = t.before;
        self.redo.push(t);
        Some(sel)
    }

    pub fn redo(&mut self, text: &mut String) -> Option<Selection> {
        let mut t = self.redo.pop()?;
        for step in t.steps.iter_mut() {
            step.inverse = apply(text, &step.forward);
        }
        let sel = t.after;
        self.undo.push(t);
        Some(sel)
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(h: &mut History, text: &mut String, e: Edit, kind: Kind, now: Instant) {
        let before = Selection::caret(e.range.start);
        let after = Selection::caret(e.range.start + e.text.len());
        let inverse = apply(text, std::slice::from_ref(&e));
        h.record(vec![e], inverse, before, after, kind, now);
    }

    #[test]
    fn a_typing_burst_undoes_at_once() {
        let mut h = History::default();
        let mut text = String::new();
        let t0 = Instant::now();
        edit(&mut h, &mut text, Edit::insert(0, "a"), Kind::Typing, t0);
        edit(
            &mut h,
            &mut text,
            Edit::insert(1, "b"),
            Kind::Typing,
            t0 + Duration::from_millis(100),
        );
        edit(
            &mut h,
            &mut text,
            Edit::insert(2, "c"),
            Kind::Typing,
            t0 + Duration::from_millis(1200),
        );
        assert_eq!(text, "abc");
        assert_eq!(h.undo(&mut text), Some(Selection::caret(2)));
        assert_eq!(text, "ab");
        assert_eq!(h.undo(&mut text), Some(Selection::caret(0)));
        assert_eq!(text, "");
        assert!(!h.can_undo());
        assert_eq!(h.redo(&mut text), Some(Selection::caret(2)));
        assert_eq!(text, "ab");
        assert_eq!(h.redo(&mut text), Some(Selection::caret(3)));
        assert_eq!(text, "abc");
    }

    #[test]
    fn sealing_and_other_edits_break_bursts() {
        let mut h = History::default();
        let mut text = String::new();
        let t0 = Instant::now();
        edit(&mut h, &mut text, Edit::insert(0, "a"), Kind::Typing, t0);
        h.seal();
        edit(&mut h, &mut text, Edit::insert(1, "b"), Kind::Typing, t0);
        edit(&mut h, &mut text, Edit::insert(0, "# "), Kind::Other, t0);
        edit(&mut h, &mut text, Edit::insert(4, "c"), Kind::Typing, t0);
        assert_eq!(text, "# abc");
        h.undo(&mut text);
        assert_eq!(text, "# ab");
        h.undo(&mut text);
        assert_eq!(text, "ab");
        h.undo(&mut text);
        assert_eq!(text, "a");
        // A new edit after undoing drops what could have been redone.
        edit(&mut h, &mut text, Edit::insert(1, "z"), Kind::Typing, t0);
        assert!(!h.can_redo());
    }

    #[test]
    fn undo_redo_survive_replacements() {
        let mut h = History::default();
        let mut text = "one two three".to_string();
        let t0 = Instant::now();
        let edits = vec![Edit::replace(0..3, "1"), Edit::replace(8..13, "3")];
        let inverse = apply(&mut text, &edits);
        h.record(
            edits,
            inverse,
            Selection::caret(0),
            Selection::caret(0),
            Kind::Other,
            t0,
        );
        assert_eq!(text, "1 two 3");
        h.undo(&mut text);
        assert_eq!(text, "one two three");
        h.redo(&mut text);
        assert_eq!(text, "1 two 3");
        h.undo(&mut text);
        assert_eq!(text, "one two three");
    }

    #[test]
    fn keeps_at_most_the_limit() {
        let mut h = History::default();
        let mut text = String::new();
        let t0 = Instant::now();
        for i in 0..LIMIT + 5 {
            edit(&mut h, &mut text, Edit::insert(i, "x"), Kind::Other, t0);
        }
        let mut n = 0;
        while h.undo(&mut text).is_some() {
            n += 1;
        }
        assert_eq!(n, LIMIT);
        assert_eq!(text.len(), 5);
    }
}
