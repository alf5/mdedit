//! The live-preview markdown editor.
//!
//! The document is the markdown text itself. Each source line is drawn on
//! its own, styled from `mdedit_core::syntax`, with its markup hidden
//! (Rendered), hidden except on the caret's lines (Live), or shown (Source):
//! see `mdedit_core::display`. Every keystroke is an edit to the text, so
//! nothing is ever rebuilt from a rendering, and saving can't rewrite what
//! wasn't touched.
//!
//! The caret moves in display offsets, which steps over hidden markup.
//! Layout is per line and only for the lines on screen (`element.rs`).

mod element;
pub mod find_bar;
mod overlays;
pub mod style;

use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::*;
use mdedit_core::buffer::{self, Change, Edit, Selection};
use mdedit_core::display::{self, DisplayLine, Gutter, Reveal, Role};
use mdedit_core::history::{History, Kind};
use mdedit_core::prefs::Mode;
use mdedit_core::syntax::{self, LineKind, Syntax};
use mdedit_core::{edit, find, os};

use self::element::EditorElement;
use self::style::DocStyle;
use crate::actions::*;

pub enum EditorEvent {
    /// The text changed.
    Edited,
    /// The caret or selection moved, or the mode changed.
    Moved,
}

/// One line, shaped for the current width and mode.
pub(crate) struct LineLayout {
    pub display: DisplayLine,
    pub shaped: WrappedLine,
    pub kind: LineKind,
    pub quote: u8,
    pub line_height: Pixels,
    pub pad_top: Pixels,
    /// Where the text starts, from the left of the reading column.
    pub indent: Pixels,
    pub height: Pixels,
    /// A list marker drawn in the margin before the text.
    pub gutter: Option<ShapedLine>,
}

impl LineLayout {
    /// The point of display offset `d`, relative to the line box.
    fn point(&self, d: usize) -> Point<Pixels> {
        let p = self
            .shaped
            .position_for_index(d, self.line_height)
            .unwrap_or_else(|| point(self.shaped.width(), px(0.)));
        point(p.x + self.indent, p.y + self.pad_top)
    }

    /// The display offset nearest `p`, relative to the line box.
    fn index(&self, p: Point<Pixels>) -> usize {
        let local = point(p.x - self.indent, (p.y - self.pad_top).max(px(0.)));
        match self
            .shaped
            .closest_index_for_position(local, self.line_height)
        {
            Ok(i) | Err(i) => i.min(self.display.text.len()),
        }
    }

    fn text_height(&self) -> Pixels {
        self.shaped.size(self.line_height).height
    }
}

#[derive(Clone)]
struct Slot {
    height: Pixels,
    layout: Option<Rc<LineLayout>>,
}

/// A visible line: its index, where its box starts, and its layout.
pub(crate) type Placed = (usize, Point<Pixels>, Rc<LineLayout>);

/// Where things were drawn last frame, for the mouse and input methods.
#[derive(Clone, Default)]
pub(crate) struct Geometry {
    pub bounds: Bounds<Pixels>,
    pub lines: Vec<Placed>,
}

#[derive(Clone, Copy, PartialEq)]
enum Grain {
    Char,
    Word,
    Line,
}

pub struct FindState {
    pub query: String,
    pub case: bool,
    pub matches: Vec<Range<usize>>,
    pub current: Option<usize>,
}

pub struct MarkdownEditor {
    text: String,
    syntax: Syntax,
    sel: Selection,
    marked: Option<Range<usize>>,
    history: History,
    mode: Mode,
    style: DocStyle,
    focus: FocusHandle,
    /// Source lines drawn together, as ranges of line indices (see
    /// `display::rows`); one slot per row.
    rows: Vec<Range<usize>>,
    slots: Vec<Slot>,
    content_w: Option<Pixels>,
    scroll_y: Pixels,
    autoscroll: bool,
    /// The next autoscroll puts the caret near the top, not just in view.
    scroll_to_top: bool,
    goal_x: Option<Pixels>,
    selecting: Option<(Grain, Selection)>,
    pub(crate) geometry: Geometry,
    /// Lines whose markup Live mode shows.
    revealed: Range<usize>,
    words: usize,
    chars: usize,
    pub find: Option<FindState>,
    /// The `/` menu: where its slash is, and the highlighted entry.
    slash: Option<usize>,
    slash_ix: usize,
}

impl EventEmitter<EditorEvent> for MarkdownEditor {}

impl Focusable for MarkdownEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// How far each level of a list is set in.
pub(crate) const LIST_INDENT: f32 = 26.;

fn max(a: Pixels, b: Pixels) -> Pixels {
    if a > b { a } else { b }
}

fn min(a: Pixels, b: Pixels) -> Pixels {
    if a < b { a } else { b }
}

impl MarkdownEditor {
    pub fn new(text: String, mode: Mode, style: DocStyle, cx: &mut Context<Self>) -> Self {
        let syntax = syntax::parse(&text);
        let mut this = MarkdownEditor {
            text,
            syntax,
            sel: Selection::caret(0),
            marked: None,
            history: History::default(),
            mode,
            style,
            focus: cx.focus_handle(),
            rows: Vec::new(),
            slots: Vec::new(),
            content_w: None,
            scroll_y: px(0.),
            autoscroll: false,
            scroll_to_top: false,
            goal_x: None,
            selecting: None,
            geometry: Geometry::default(),
            revealed: 0..0,
            words: 0,
            chars: 0,
            find: None,
            slash: None,
            slash_ix: 0,
        };
        this.rebuild_slots();
        this.recount();
        this.update_reveal();
        this
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn selection(&self) -> Selection {
        self.sel
    }

    pub fn syntax(&self) -> &Syntax {
        &self.syntax
    }

    pub fn counts(&self) -> (usize, usize) {
        (self.words, self.chars)
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if self.mode != mode {
            self.mode = mode;
            // Source draws every line on its own; the rendered modes flow
            // paragraphs.
            self.rebuild_slots();
            self.update_reveal();
            self.autoscroll = true;
            cx.emit(EditorEvent::Moved);
            cx.notify();
        }
    }

    pub fn set_style(&mut self, style: DocStyle, cx: &mut Context<Self>) {
        if self.style != style {
            self.style = style;
            self.invalidate();
            cx.notify();
        }
    }

    /// Put the caret at `pos` and bring it into view.
    pub fn reveal_offset(&mut self, pos: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.set_selection(Selection::caret(pos.min(self.text.len())), cx);
        self.focus.focus(window, cx);
        self.autoscroll = true;
        self.scroll_to_top = true;
        cx.notify();
    }

    // ---------- model ----------

    fn invalidate(&mut self) {
        for slot in &mut self.slots {
            slot.layout = None;
        }
    }

    fn estimate(&self, kind: LineKind) -> Pixels {
        let (_, lh) = self.style.metrics(kind, self.mode);
        let rich = self.mode != Mode::Source;
        px(match kind {
            LineKind::Blank if rich => lh * 0.6,
            LineKind::Heading(_) if rich => lh * 1.4,
            _ => lh,
        })
    }

    fn rebuild_slots(&mut self) {
        let old = std::mem::take(&mut self.slots);
        self.rows = display::rows(&self.text, &self.syntax, self.rich());
        self.slots = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| Slot {
                // Keep what we measured where we can: the rows around an
                // edit rarely change height, and keeping them steadies the
                // scroll position.
                height: old
                    .get(i)
                    .filter(|_| old.len() == self.rows.len())
                    .map_or_else(
                        || self.estimate(self.syntax.lines[r.start].kind) * r.len() as f32,
                        |s| s.height,
                    ),
                layout: None,
            })
            .collect();
    }

    /// The row holding byte `pos`.
    fn row_at(&self, pos: usize) -> usize {
        let line = self.syntax.line_at(pos);
        self.rows
            .partition_point(|r| r.end <= line)
            .min(self.rows.len().saturating_sub(1))
    }

    /// The source a row covers, without its final newline.
    fn row_range(&self, ix: usize) -> Range<usize> {
        let r = &self.rows[ix];
        self.syntax.lines[r.start].range.start..self.syntax.lines[r.end - 1].range.end
    }

    fn recount(&mut self) {
        self.words = self.text.split_whitespace().count();
        self.chars = self.text.chars().count();
    }

    /// Apply a change as one undo step of `kind`.
    pub fn apply(&mut self, change: Change, kind: Kind, cx: &mut Context<Self>) {
        if change.edits.is_empty() {
            self.set_selection(change.selection, cx);
            return;
        }
        let before = self.sel;
        let inverse = buffer::apply(&mut self.text, &change.edits);
        self.history.record(
            change.edits,
            inverse,
            before,
            change.selection,
            kind,
            Instant::now(),
        );
        self.sel = change.selection.clamp(self.text.len());
        self.after_edit(cx);
    }

    fn after_edit(&mut self, cx: &mut Context<Self>) {
        self.syntax = syntax::parse(&self.text);
        self.rebuild_slots();
        self.recount();
        self.update_reveal();
        self.refresh_find();
        self.update_slash(None);
        self.autoscroll = true;
        self.goal_x = None;
        cx.emit(EditorEvent::Edited);
        cx.notify();
    }

    /// Replace the whole text (a reload from disk), keeping the caret as
    /// near as it can.
    pub fn set_text(&mut self, text: String, cx: &mut Context<Self>) {
        let pos = self.sel.head.min(text.len());
        let pos = (0..=pos)
            .rev()
            .find(|p| text.is_char_boundary(*p))
            .unwrap_or(0);
        let change = Change {
            edits: vec![Edit::replace(0..self.text.len(), text)],
            selection: Selection::caret(pos),
        };
        self.apply(change, Kind::Other, cx);
    }

    fn set_selection(&mut self, sel: Selection, cx: &mut Context<Self>) {
        let sel = sel.clamp(self.text.len());
        if sel != self.sel {
            self.history.seal();
            self.sel = sel;
            self.update_reveal();
            self.update_slash(None);
            self.autoscroll = true;
            cx.emit(EditorEvent::Moved);
        }
        cx.notify();
    }

    /// In Live mode, the lines the selection touches show their markup.
    fn update_reveal(&mut self) {
        let new = if self.mode == Mode::Live {
            let a = self.row_at(self.sel.start());
            let b = self.row_at(self.sel.end());
            a..b + 1
        } else {
            0..0
        };
        if new != self.revealed {
            for ix in self.revealed.clone().chain(new.clone()) {
                if let Some(slot) = self.slots.get_mut(ix) {
                    slot.layout = None;
                }
            }
            self.revealed = new;
        }
    }

    fn reveal_of(&self, ix: usize) -> Reveal {
        match self.mode {
            Mode::Source => Reveal::Show,
            Mode::Rendered => Reveal::Hide,
            Mode::Live if self.revealed.contains(&ix) => Reveal::Show,
            Mode::Live => Reveal::Hide,
        }
    }

    fn rich(&self) -> bool {
        self.mode != Mode::Source
    }

    // ---------- layout ----------

    fn layout(&mut self, ix: usize, window: &mut Window) -> Rc<LineLayout> {
        if let Some(l) = self.slots.get(ix).and_then(|s| s.layout.clone()) {
            return l;
        }
        let content_w = self.content_w.unwrap_or(px(700.));
        let line = self.syntax.lines[self.rows[ix].start].clone();
        let display = display::display_row(
            &self.text,
            &self.syntax,
            self.rows[ix].clone(),
            self.reveal_of(ix),
        );
        let (size, lh) = self.style.metrics(line.kind, self.mode);
        let (size, lh) = (px(size), px(lh));
        let rich = self.rich();
        let code_block = rich
            && matches!(
                line.kind,
                LineKind::Code | LineKind::FenceOpen | LineKind::FenceClose
            );
        let revealed = self.reveal_of(ix) == Reveal::Show;
        let list_depth = match (rich, revealed) {
            (false, _) => 0,
            (true, true) => line.list.saturating_sub(1),
            (true, false) => line.list,
        };
        let indent = if rich {
            px(line.quote as f32 * 20. + list_depth as f32 * LIST_INDENT)
                + if code_block { px(16.) } else { px(0.) }
        } else {
            px(0.)
        };
        let wrap = max(
            content_w - indent - if code_block { px(16.) } else { px(0.) },
            px(40.),
        );
        let runs = self.text_runs(&display, line.kind, line.quote);
        let shaped = window
            .text_system()
            .shape_text(display.text.clone().into(), size, &runs, Some(wrap), None)
            .ok()
            .and_then(|mut v| (!v.is_empty()).then(|| v.remove(0)))
            .unwrap_or_default();
        let pad_top = match line.kind {
            LineKind::Heading(l) if rich => size * if l <= 2 { 0.55 } else { 0.35 },
            _ => px(0.),
        };
        let text_h = shaped.size(lh).height;
        let height = match line.kind {
            LineKind::Blank if rich => lh * 0.6,
            LineKind::FenceClose if rich && display.text.is_empty() => lh * 0.6,
            _ => text_h + pad_top,
        };
        let c = &self.style.colors;
        let gutter = display.gutter.as_ref().and_then(|g| {
            let (glyph, color) = match g {
                Gutter::Bullet => ("•".to_string(), c.muted),
                Gutter::Number(n) => (n.clone(), c.muted),
                // Drawn as a box in `element.rs`.
                Gutter::Task { .. } => return None,
            };
            let run = TextRun {
                len: glyph.len(),
                font: font(self.style.font.clone()),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            Some(
                window
                    .text_system()
                    .shape_line(glyph.into(), size, &[run], None),
            )
        });
        let layout = Rc::new(LineLayout {
            gutter,
            display,
            shaped,
            kind: line.kind,
            quote: line.quote,
            line_height: lh,
            pad_top,
            indent,
            height,
        });
        if let Some(slot) = self.slots.get_mut(ix) {
            slot.height = layout.height;
            slot.layout = Some(layout.clone());
        }
        layout
    }

    fn text_runs(&self, d: &DisplayLine, kind: LineKind, quote: u8) -> Vec<TextRun> {
        let c = &self.style.colors;
        let mono = self.style.monospace(kind, self.mode);
        let source = self.mode == Mode::Source;
        let heading = matches!(kind, LineKind::Heading(_) | LineKind::SetextUnderline);
        let base_color = if quote > 0 || self.style.muted_body {
            c.muted
        } else {
            c.text
        };
        let base_color = if quote > 0 && !source {
            c.muted
        } else {
            base_color
        };
        d.runs
            .iter()
            .map(|r| {
                let code = display::has(r.style, display::CODE);
                let mut f = font(if mono || code {
                    self.style.mono.clone()
                } else {
                    self.style.font.clone()
                });
                if mono || code {
                    // `<!--` should read as itself, not as an arrow.
                    f.features = FontFeatures::disable_ligatures();
                }
                if heading || display::has(r.style, display::BOLD) {
                    f.weight = if heading && !source {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::BOLD
                    };
                }
                if display::has(r.style, display::ITALIC) {
                    f.style = FontStyle::Italic;
                }
                let link = display::has(r.style, display::LINK | display::IMAGE);
                let color = match r.role {
                    Role::Markup | Role::Bullet | Role::Number | Role::Lang => c.muted,
                    Role::TaskOpen | Role::TaskDone => c.accent,
                    Role::Text if link => c.accent,
                    Role::Text if display::has(r.style, display::MATH) => c.accent,
                    Role::Text if heading && source => c.accent,
                    Role::Text => base_color,
                };
                TextRun {
                    len: r.len,
                    font: f,
                    color,
                    background_color: (code && !source && kind != LineKind::Code)
                        .then_some(c.code_bg),
                    underline: (link && r.role == Role::Text).then_some(UnderlineStyle {
                        thickness: px(1.),
                        color: Some(c.accent.opacity(0.5)),
                        wavy: false,
                    }),
                    strikethrough: display::has(r.style, display::STRIKE).then_some(
                        StrikethroughStyle {
                            thickness: px(1.),
                            color: Some(color),
                        },
                    ),
                }
            })
            .collect()
    }

    /// The caret's line and display offset.
    fn caret_display(&mut self, window: &mut Window) -> (usize, usize, Rc<LineLayout>) {
        let ix = self.row_at(self.sel.head);
        let layout = self.layout(ix, window);
        let d = layout.display.to_display(self.sel.head);
        (ix, d, layout)
    }

    // ---------- movement ----------

    fn move_to(&mut self, pos: usize, extend: bool, cx: &mut Context<Self>) {
        let sel = if extend {
            Selection::new(self.sel.anchor, pos)
        } else {
            Selection::caret(pos)
        };
        self.set_selection(sel, cx);
    }

    fn horizontal(
        &mut self,
        forward: bool,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.goal_x = None;
        if !extend && !self.sel.is_empty() {
            let pos = if forward {
                self.sel.end()
            } else {
                self.sel.start()
            };
            return self.move_to(pos, false, cx);
        }
        let (ix, d, layout) = self.caret_display(window);
        let pos = if forward {
            if d < layout.display.text.len() {
                let next = layout.display.next(d);
                // Past the last character: the end of the source line, after
                // any markup closing it.
                if next == layout.display.text.len() {
                    self.row_range(ix).end
                } else {
                    layout.display.to_source(next)
                }
            } else if ix + 1 < self.rows.len() {
                let next = self.layout(ix + 1, window);
                next.display.to_source(0)
            } else {
                self.text.len()
            }
        } else if d > 0 {
            layout.display.to_source(layout.display.prev(d))
        } else if ix > 0 {
            self.row_range(ix - 1).end
        } else {
            0
        };
        // Never stall: a mapping that lands where we are steps one source
        // character instead.
        let pos = if pos == self.sel.head {
            if forward {
                buffer::next_char(&self.text, pos)
            } else {
                buffer::prev_char(&self.text, pos)
            }
        } else {
            pos
        };
        self.move_to(pos, extend, cx);
    }

    fn vertical(&mut self, rows: i32, extend: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (mut ix, d, mut layout) = self.caret_display(window);
        let here = layout.point(d);
        let x = *self.goal_x.get_or_insert(here.x);
        let mut y = here.y;
        let step = rows.signum();
        for _ in 0..rows.abs() {
            let next_y = y + layout.line_height * step as f32;
            if next_y >= layout.pad_top && next_y < layout.pad_top + layout.text_height() {
                y = next_y;
                continue;
            }
            let next = ix as i64 + step as i64;
            if next < 0 || next as usize >= self.rows.len() {
                let pos = if step < 0 { 0 } else { self.text.len() };
                self.move_to(pos, extend, cx);
                return;
            }
            ix = next as usize;
            layout = self.layout(ix, window);
            y = if step > 0 {
                layout.pad_top
            } else {
                layout.pad_top + layout.text_height() - layout.line_height
            };
        }
        let idx = layout.index(point(x, y));
        let pos = layout.display.to_source(idx);
        let goal = self.goal_x;
        self.move_to(pos, extend, cx);
        self.goal_x = goal;
    }

    /// Home and End: the start and end of the wrapped line the caret is on.
    /// Home again goes to the very start, before any hidden markup.
    fn line_edge(&mut self, end: bool, extend: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.goal_x = None;
        let (ix, d, layout) = self.caret_display(window);
        let y = layout.point(d).y;
        let range = self.row_range(ix);
        let pos = if end {
            let e = layout.index(point(layout.indent + px(1e6), y));
            if e >= layout.display.text.len() {
                range.end
            } else {
                layout.display.to_source(e)
            }
        } else {
            let st = layout.index(point(layout.indent, y));
            let start = layout.display.to_source(st);
            if self.sel.head == start && st == 0 {
                range.start
            } else {
                start
            }
        };
        self.move_to(pos, extend, cx);
    }

    fn page(&self) -> i32 {
        let h = f32::from(self.geometry.bounds.size.height);
        let lh = self.style.size * self.style.leading;
        ((h / lh) as i32 - 2).max(1)
    }

    fn move_left(&mut self, _: &MoveLeft, w: &mut Window, cx: &mut Context<Self>) {
        self.horizontal(false, false, w, cx);
    }
    fn move_right(&mut self, _: &MoveRight, w: &mut Window, cx: &mut Context<Self>) {
        self.horizontal(true, false, w, cx);
    }
    fn select_left(&mut self, _: &SelectLeft, w: &mut Window, cx: &mut Context<Self>) {
        self.horizontal(false, true, w, cx);
    }
    fn select_right(&mut self, _: &SelectRight, w: &mut Window, cx: &mut Context<Self>) {
        self.horizontal(true, true, w, cx);
    }
    /// Up and Down walk the `/` menu while it's open.
    fn slash_step(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        if self.slash.is_none() {
            return false;
        }
        let n = self.slash_matches().len().max(1) as isize;
        self.slash_ix = (self.slash_ix as isize + delta).rem_euclid(n) as usize;
        cx.notify();
        true
    }

    /// Enter or Tab picks the highlighted `/` entry.
    fn slash_pick(&mut self, cx: &mut Context<Self>) -> bool {
        if self.slash.is_none() {
            return false;
        }
        if let Some(item) = self.slash_matches().get(self.slash_ix).copied() {
            self.apply_slash(item, cx);
        }
        true
    }

    fn move_up(&mut self, _: &MoveUp, w: &mut Window, cx: &mut Context<Self>) {
        if !self.slash_step(-1, cx) {
            self.vertical(-1, false, w, cx);
        }
    }
    fn move_down(&mut self, _: &MoveDown, w: &mut Window, cx: &mut Context<Self>) {
        if !self.slash_step(1, cx) {
            self.vertical(1, false, w, cx);
        }
    }
    fn select_up(&mut self, _: &SelectUp, w: &mut Window, cx: &mut Context<Self>) {
        self.vertical(-1, true, w, cx);
    }
    fn select_down(&mut self, _: &SelectDown, w: &mut Window, cx: &mut Context<Self>) {
        self.vertical(1, true, w, cx);
    }
    fn page_up(&mut self, _: &PageUp, w: &mut Window, cx: &mut Context<Self>) {
        let n = self.page();
        self.vertical(-n, false, w, cx);
    }
    fn page_down(&mut self, _: &PageDown, w: &mut Window, cx: &mut Context<Self>) {
        let n = self.page();
        self.vertical(n, false, w, cx);
    }
    fn select_page_up(&mut self, _: &SelectPageUp, w: &mut Window, cx: &mut Context<Self>) {
        let n = self.page();
        self.vertical(-n, true, w, cx);
    }
    fn select_page_down(&mut self, _: &SelectPageDown, w: &mut Window, cx: &mut Context<Self>) {
        let n = self.page();
        self.vertical(n, true, w, cx);
    }
    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let p = buffer::prev_word(&self.text, self.sel.head);
        self.move_to(p, false, cx);
    }
    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        let p = buffer::next_word(&self.text, self.sel.head);
        self.move_to(p, false, cx);
    }
    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let p = buffer::prev_word(&self.text, self.sel.head);
        self.move_to(p, true, cx);
    }
    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        let p = buffer::next_word(&self.text, self.sel.head);
        self.move_to(p, true, cx);
    }
    fn line_start(&mut self, _: &LineStart, w: &mut Window, cx: &mut Context<Self>) {
        self.line_edge(false, false, w, cx);
    }
    fn line_end_(&mut self, _: &LineEnd, w: &mut Window, cx: &mut Context<Self>) {
        self.line_edge(true, false, w, cx);
    }
    fn select_line_start(&mut self, _: &SelectLineStart, w: &mut Window, cx: &mut Context<Self>) {
        self.line_edge(false, true, w, cx);
    }
    fn select_line_end(&mut self, _: &SelectLineEnd, w: &mut Window, cx: &mut Context<Self>) {
        self.line_edge(true, true, w, cx);
    }
    fn doc_start(&mut self, _: &DocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, false, cx);
    }
    fn doc_end(&mut self, _: &DocEnd, _: &mut Window, cx: &mut Context<Self>) {
        let n = self.text.len();
        self.move_to(n, false, cx);
    }
    fn select_doc_start(&mut self, _: &SelectDocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, true, cx);
    }
    fn select_doc_end(&mut self, _: &SelectDocEnd, _: &mut Window, cx: &mut Context<Self>) {
        let n = self.text.len();
        self.move_to(n, true, cx);
    }
    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.set_selection(Selection::new(0, self.text.len()), cx);
    }

    // ---------- editing ----------

    fn replace_selection(&mut self, s: &str, kind: Kind, cx: &mut Context<Self>) {
        let r = self.sel.range();
        let at = r.start + s.len();
        self.apply(
            Change {
                edits: vec![Edit::replace(r, s)],
                selection: Selection::caret(at),
            },
            kind,
            cx,
        );
    }

    fn delete_range(&mut self, r: Range<usize>, cx: &mut Context<Self>) {
        if r.is_empty() {
            return;
        }
        let at = r.start;
        self.apply(
            Change {
                edits: vec![Edit::delete(r)],
                selection: Selection::caret(at),
            },
            Kind::Typing,
            cx,
        );
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if !self.sel.is_empty() {
            return self.delete_range(self.sel.range(), cx);
        }
        if self.rich()
            && let Some(c) = edit::backspace_prefix(&self.text, self.sel)
        {
            return self.apply(c, Kind::Other, cx);
        }
        let caret = self.sel.head;
        let (ix, d, layout) = self.caret_display(window);
        let range = if d == 0 {
            if ix == 0 {
                return;
            }
            self.row_range(ix - 1).end..caret
        } else {
            let prev = layout.display.prev(d);
            match layout.display.role_at(prev) {
                Role::Bullet | Role::TaskOpen | Role::TaskDone => {
                    layout.display.source_at(prev).unwrap_or(caret..caret)
                }
                _ => {
                    let s = layout.display.to_source(prev);
                    s..buffer::next_char(&self.text, s).min(caret.max(s + 1))
                }
            }
        };
        self.delete_range(range, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if !self.sel.is_empty() {
            return self.delete_range(self.sel.range(), cx);
        }
        let caret = self.sel.head;
        let (ix, d, layout) = self.caret_display(window);
        let range = if d >= layout.display.text.len() {
            let end = self.row_range(ix).end;
            caret..(end + 1).min(self.text.len()).max(caret)
        } else {
            let s = layout.display.to_source(d);
            s..buffer::next_char(&self.text, s)
        };
        let range = range.start.min(caret)..range.end;
        self.delete_range(range, cx);
    }

    fn delete_word_left(&mut self, _: &DeleteWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let r = if self.sel.is_empty() {
            buffer::prev_word(&self.text, self.sel.head)..self.sel.head
        } else {
            self.sel.range()
        };
        self.delete_range(r, cx);
    }

    fn delete_word_right(&mut self, _: &DeleteWordRight, _: &mut Window, cx: &mut Context<Self>) {
        let r = if self.sel.is_empty() {
            self.sel.head..buffer::next_word(&self.text, self.sel.head)
        } else {
            self.sel.range()
        };
        self.delete_range(r, cx);
    }

    fn caret_kind(&self) -> LineKind {
        self.syntax.lines[self.syntax.line_at(self.sel.head)].kind
    }

    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        if self.slash_pick(cx) {
            return;
        }
        let in_code = matches!(self.caret_kind(), LineKind::Code | LineKind::FenceOpen);
        let c = edit::enter(&self.text, self.sel, self.rich(), in_code);
        self.apply(c, Kind::Other, cx);
    }

    fn soft_break(&mut self, _: &SoftBreak, _: &mut Window, cx: &mut Context<Self>) {
        let c = edit::soft_break(&self.text, self.sel);
        self.apply(c, Kind::Other, cx);
    }

    fn indent(&mut self, _: &Indent, _: &mut Window, cx: &mut Context<Self>) {
        if self.slash_pick(cx) {
            return;
        }
        match edit::indent(&self.text, self.sel) {
            Some(c) => self.apply(c, Kind::Other, cx),
            None if !self.rich() || self.caret_kind() == LineKind::Code => {
                self.replace_selection("    ", Kind::Typing, cx)
            }
            None => {}
        }
    }

    fn outdent(&mut self, _: &Outdent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(c) = edit::outdent(&self.text, self.sel) {
            self.apply(c, Kind::Other, cx);
        }
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(sel) = self.history.undo(&mut self.text) {
            self.sel = sel.clamp(self.text.len());
            self.after_edit(cx);
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(sel) = self.history.redo(&mut self.text) {
            self.sel = sel.clamp(self.text.len());
            self.after_edit(cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.sel.is_empty() {
            let s = self.text[self.sel.range()].to_string();
            cx.write_to_clipboard(ClipboardItem::new_string(s));
        }
    }

    fn cut(&mut self, _: &Cut, w: &mut Window, cx: &mut Context<Self>) {
        if !self.sel.is_empty() {
            self.copy(&Copy, w, cx);
            self.delete_range(self.sel.range(), cx);
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        self.paste_text(cx);
    }

    fn paste_plain(&mut self, _: &PastePlain, _: &mut Window, cx: &mut Context<Self>) {
        self.paste_text(cx);
    }

    fn paste_text(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
            let text = text.replace("\r\n", "\n");
            self.replace_selection(&text, Kind::Other, cx);
        }
    }

    fn inline(&mut self, marker: &str, cx: &mut Context<Self>) {
        let c = edit::toggle_inline(&self.text, self.sel, marker);
        self.apply(c, Kind::Other, cx);
    }

    fn bold(&mut self, _: &Bold, _: &mut Window, cx: &mut Context<Self>) {
        self.inline("**", cx);
    }
    fn italic(&mut self, _: &Italic, _: &mut Window, cx: &mut Context<Self>) {
        self.inline("*", cx);
    }
    fn strike(&mut self, _: &Strikethrough, _: &mut Window, cx: &mut Context<Self>) {
        self.inline("~~", cx);
    }
    fn code(&mut self, _: &InlineCode, _: &mut Window, cx: &mut Context<Self>) {
        self.inline("`", cx);
    }

    fn heading(&mut self, a: &Heading, _: &mut Window, cx: &mut Context<Self>) {
        let c = edit::set_heading(&self.text, self.sel, a.level);
        self.apply(c, Kind::Other, cx);
    }
    fn paragraph(&mut self, _: &Paragraph, _: &mut Window, cx: &mut Context<Self>) {
        let c = edit::set_heading(&self.text, self.sel, 0);
        self.apply(c, Kind::Other, cx);
    }

    fn block(&mut self, b: edit::Block, cx: &mut Context<Self>) {
        let c = edit::toggle_block(&self.text, self.sel, b);
        self.apply(c, Kind::Other, cx);
    }
    fn bullets(&mut self, _: &BulletList, _: &mut Window, cx: &mut Context<Self>) {
        self.block(edit::Block::Bullet, cx);
    }
    fn numbers(&mut self, _: &NumberedList, _: &mut Window, cx: &mut Context<Self>) {
        self.block(edit::Block::Numbered, cx);
    }
    fn tasks(&mut self, _: &TaskList, _: &mut Window, cx: &mut Context<Self>) {
        self.block(edit::Block::Task, cx);
    }
    fn quote(&mut self, _: &Blockquote, _: &mut Window, cx: &mut Context<Self>) {
        self.block(edit::Block::Quote, cx);
    }

    fn insert_block(&mut self, block: &str, caret: usize, cx: &mut Context<Self>) {
        let c = edit::insert_block(&self.text, self.sel, block, caret);
        self.apply(c, Kind::Other, cx);
    }
    fn code_block(&mut self, _: &CodeBlock, _: &mut Window, cx: &mut Context<Self>) {
        self.insert_block(edit::CODE_MD, 4, cx);
    }
    fn table(&mut self, _: &InsertTable, _: &mut Window, cx: &mut Context<Self>) {
        self.insert_block(edit::TABLE_MD, 2, cx);
    }
    fn rule(&mut self, _: &InsertRule, _: &mut Window, cx: &mut Context<Self>) {
        self.insert_block(edit::RULE_MD, edit::RULE_MD.len(), cx);
    }
    fn diagram(&mut self, _: &InsertDiagram, _: &mut Window, cx: &mut Context<Self>) {
        self.insert_block(edit::MERMAID_MD, 11, cx);
    }
    fn math(&mut self, _: &InsertMath, _: &mut Window, cx: &mut Context<Self>) {
        self.insert_block(edit::MATH_MD, 3, cx);
    }

    /// Ctrl/Cmd+K: a link around the selection. The URL comes from the
    /// clipboard when it holds one, so copy-then-link is two keystrokes;
    /// otherwise the caret lands in the `()` to type it.
    fn link(&mut self, _: &InsertLink, _: &mut Window, cx: &mut Context<Self>) {
        let clip = cx
            .read_from_clipboard()
            .and_then(|i| i.text())
            .filter(|t| os::is_openable_link(t.trim()));
        match clip {
            Some(url) => {
                let c = edit::insert_link(&self.text, self.sel, url.trim(), false);
                self.apply(c, Kind::Other, cx);
            }
            None => {
                let r = self.sel.range();
                let label = self.text[r.clone()].to_string();
                let s = format!("[{label}]()");
                let at = r.start + s.len() - 1;
                self.apply(
                    Change {
                        edits: vec![Edit::replace(r, s)],
                        selection: Selection::caret(at),
                    },
                    Kind::Other,
                    cx,
                );
            }
        }
    }

    fn toggle_source(&mut self, _: &ToggleSource, _: &mut Window, cx: &mut Context<Self>) {
        let mode = if self.mode == Mode::Source {
            crate::state::AppState::prefs(cx).mode_or_rendered()
        } else {
            Mode::Source
        };
        self.set_mode(mode, cx);
    }

    fn set_mode_action(&mut self, a: &SetMode, _: &mut Window, cx: &mut Context<Self>) {
        self.set_mode(a.mode, cx);
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        if self.slash.take().is_some() {
            cx.notify();
        } else if self.find.is_some() {
            self.find = None;
            cx.emit(EditorEvent::Moved);
            cx.notify();
        } else if !self.sel.is_empty() {
            self.set_selection(Selection::caret(self.sel.head), cx);
        }
    }

    // ---------- find ----------

    pub fn set_find(&mut self, query: String, case: bool, cx: &mut Context<Self>) {
        let find = self.find.get_or_insert(FindState {
            query: String::new(),
            case,
            matches: Vec::new(),
            current: None,
        });
        find.query = query;
        find.case = case;
        self.refresh_find();
        // Search as you type: the first match from the selection on becomes
        // the selection.
        let hit = self.find.as_mut().and_then(|f| {
            f.current = find::next_match(&f.matches, self.sel.start(), true);
            f.current.map(|i| f.matches[i].clone())
        });
        if let Some(r) = hit {
            self.set_selection(Selection::new(r.start, r.end), cx);
        }
        self.autoscroll = true;
        cx.notify();
    }

    pub fn close_find(&mut self, cx: &mut Context<Self>) {
        self.find = None;
        cx.notify();
    }

    fn refresh_find(&mut self) {
        if let Some(f) = &mut self.find {
            f.matches = find::find_all(&self.text, &f.query, f.case);
            f.current = f.current.filter(|c| *c < f.matches.len());
        }
    }

    pub fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(f) = &self.find else { return };
        let from = if forward {
            self.sel.end()
        } else {
            self.sel.start()
        };
        let Some(i) = find::next_match(&f.matches, from, forward) else {
            return;
        };
        let r = f.matches[i].clone();
        if let Some(f) = &mut self.find {
            f.current = Some(i);
        }
        self.set_selection(Selection::new(r.start, r.end), cx);
    }

    pub fn replace_current(&mut self, with: &str, cx: &mut Context<Self>) {
        let Some(f) = &self.find else { return };
        let hit = f.matches.iter().any(|m| *m == self.sel.range());
        if hit {
            self.replace_selection(with, Kind::Other, cx);
        }
        self.find_step(true, cx);
    }

    pub fn replace_all(&mut self, with: &str, cx: &mut Context<Self>) {
        let Some(f) = &self.find else { return };
        let edits = find::replace_all(&self.text, &f.query, f.case, with);
        let n = edits.len();
        if n > 0 {
            let sel = Selection::caret(buffer::map_pos(self.sel.head, &edits, false));
            self.apply(
                Change {
                    edits,
                    selection: sel,
                },
                Kind::Other,
                cx,
            );
        }
    }

    // ---------- mouse ----------

    /// The source offset under `p`, from last frame's layout.
    fn hit(&self, p: Point<Pixels>) -> Option<(usize, usize, Rc<LineLayout>)> {
        let lines = &self.geometry.lines;
        let (first, last) = (lines.first()?, lines.last()?);
        if p.y < first.1.y {
            return Some((first.0, 0, first.2.clone()));
        }
        for (ix, origin, layout) in lines {
            if p.y < origin.y + layout.height {
                let local = point(p.x - origin.x, p.y - origin.y);
                return Some((*ix, layout.index(local), layout.clone()));
            }
        }
        Some((last.0, last.2.display.text.len(), last.2.clone()))
    }

    /// The task box at `p`, in a list line's margin.
    fn task_at(&self, p: Point<Pixels>) -> Option<Range<usize>> {
        let (_, origin, l) = self
            .geometry
            .lines
            .iter()
            .find(|(_, o, l)| p.y >= o.y && p.y < o.y + l.height)?;
        let local = p - *origin;
        match &l.display.gutter {
            Some(Gutter::Task { source, .. })
                if local.x < l.indent
                    && local.x > l.indent - px(LIST_INDENT)
                    && local.y < l.pad_top + l.line_height =>
            {
                Some(source.clone())
            }
            _ => None,
        }
    }

    pub(crate) fn mouse_down(
        &mut self,
        e: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus.focus(window, cx);
        if self.rich()
            && e.click_count == 1
            && let Some(src) = self.task_at(e.position)
            && let Some(edit) = edit::toggle_task(&self.text, src.start)
        {
            let sel = self.sel;
            return self.apply(
                Change {
                    edits: vec![edit],
                    selection: sel,
                },
                Kind::Other,
                cx,
            );
        }
        let Some((ix, d, layout)) = self.hit(e.position) else {
            return;
        };
        let pos = if d >= layout.display.text.len() {
            self.row_range(ix).end
        } else {
            layout.display.to_source(d)
        };
        // A link opens with Ctrl/Cmd held.
        if self.rich()
            && e.click_count == 1
            && !e.modifiers.shift
            && e.modifiers.secondary(cx)
            && let Some(link) = self.syntax.link_at(pos)
            && os::is_openable_link(&link.url)
        {
            cx.open_url(&link.url);
            return;
        }
        self.goal_x = None;
        let (grain, sel) = match e.click_count {
            1 if e.modifiers.shift => (Grain::Char, Selection::new(self.sel.anchor, pos)),
            1 => (Grain::Char, Selection::caret(pos)),
            2 => {
                let w = buffer::word_at(&self.text, pos);
                (Grain::Word, Selection::new(w.start, w.end))
            }
            _ => {
                let l = &self.row_range(ix);
                let end = (l.end + 1).min(self.text.len());
                (Grain::Line, Selection::new(l.start, end))
            }
        };
        self.selecting = Some((grain, sel));
        self.set_selection(sel, cx);
    }

    pub(crate) fn mouse_drag(&mut self, p: Point<Pixels>, cx: &mut Context<Self>) {
        let Some((grain, origin)) = self.selecting else {
            return;
        };
        let Some((ix, d, layout)) = self.hit(p) else {
            return;
        };
        let pos = if d >= layout.display.text.len() {
            self.row_range(ix).end
        } else {
            layout.display.to_source(d)
        };
        let sel = match grain {
            Grain::Char => Selection::new(origin.anchor, pos),
            Grain::Word => {
                let w = buffer::word_at(&self.text, pos);
                if pos < origin.start() {
                    Selection::new(origin.end(), w.start)
                } else {
                    Selection::new(origin.start(), w.end)
                }
            }
            Grain::Line => {
                let l = &self.row_range(ix);
                if pos < origin.start() {
                    Selection::new(origin.end(), l.start)
                } else {
                    Selection::new(origin.start(), (l.end + 1).min(self.text.len()))
                }
            }
        };
        self.set_selection(sel, cx);
    }

    pub(crate) fn mouse_up(&mut self) {
        self.selecting = None;
    }

    pub(crate) fn is_selecting(&self) -> bool {
        self.selecting.is_some()
    }

    pub(crate) fn scroll(&mut self, dy: Pixels, cx: &mut Context<Self>) {
        self.scroll_y = max(self.scroll_y - dy, px(0.));
        self.autoscroll = false;
        cx.notify();
    }
}

impl EntityInputHandler for MarkdownEditor {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let s = buffer::from_utf16(&self.text, range.start);
        let e = buffer::from_utf16(&self.text, range.end);
        *adjusted = Some(buffer::utf16_offset(&self.text, s)..buffer::utf16_offset(&self.text, e));
        Some(self.text[s..e].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let r = self.sel.range();
        Some(UTF16Selection {
            range: buffer::utf16_offset(&self.text, r.start)
                ..buffer::utf16_offset(&self.text, r.end),
            reversed: self.sel.reversed(),
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| {
            buffer::utf16_offset(&self.text, r.start)..buffer::utf16_offset(&self.text, r.end)
        })
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let r = range
            .map(|r| buffer::from_utf16(&self.text, r.start)..buffer::from_utf16(&self.text, r.end))
            .or(self.marked.clone())
            .unwrap_or(self.sel.range());
        self.marked = None;
        let at = r.start + text.len();
        self.apply(
            Change {
                edits: vec![Edit::replace(r, text)],
                selection: Selection::caret(at),
            },
            Kind::Typing,
            cx,
        );
        self.update_slash(Some(text));
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        new_selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let r = range
            .map(|r| buffer::from_utf16(&self.text, r.start)..buffer::from_utf16(&self.text, r.end))
            .or(self.marked.clone())
            .unwrap_or(self.sel.range());
        let sel = match new_selected {
            Some(s) => Selection::new(
                r.start + buffer::from_utf16(text, s.start),
                r.start + buffer::from_utf16(text, s.end),
            ),
            None => Selection::caret(r.start + text.len()),
        };
        self.apply(
            Change {
                edits: vec![Edit::replace(r.clone(), text)],
                selection: sel,
            },
            Kind::Typing,
            cx,
        );
        self.marked = (!text.is_empty()).then(|| r.start..r.start + text.len());
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let pos = buffer::from_utf16(&self.text, range.start);
        let ix = self.row_at(pos);
        let (_, origin, layout) = self.geometry.lines.iter().find(|(i, ..)| *i == ix)?.clone();
        let _ = window;
        let p = layout.point(layout.display.to_display(pos));
        Some(Bounds::new(origin + p, size(px(2.), layout.line_height)))
    }

    fn character_index_for_point(
        &mut self,
        p: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let (_, d, layout) = self.hit(p)?;
        Some(buffer::utf16_offset(
            &self.text,
            layout.display.to_source(d),
        ))
    }
}

impl Render for MarkdownEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let overlay = self.render_overlays(window, cx);
        div()
            .key_context("MdEditor")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::move_left))
            .on_action(cx.listener(Self::move_right))
            .on_action(cx.listener(Self::move_up))
            .on_action(cx.listener(Self::move_down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::page_up))
            .on_action(cx.listener(Self::page_down))
            .on_action(cx.listener(Self::select_page_up))
            .on_action(cx.listener(Self::select_page_down))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::line_start))
            .on_action(cx.listener(Self::line_end_))
            .on_action(cx.listener(Self::select_line_start))
            .on_action(cx.listener(Self::select_line_end))
            .on_action(cx.listener(Self::doc_start))
            .on_action(cx.listener(Self::doc_end))
            .on_action(cx.listener(Self::select_doc_start))
            .on_action(cx.listener(Self::select_doc_end))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_word_right))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::soft_break))
            .on_action(cx.listener(Self::indent))
            .on_action(cx.listener(Self::outdent))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::paste_plain))
            .on_action(cx.listener(Self::bold))
            .on_action(cx.listener(Self::italic))
            .on_action(cx.listener(Self::strike))
            .on_action(cx.listener(Self::code))
            .on_action(cx.listener(Self::heading))
            .on_action(cx.listener(Self::paragraph))
            .on_action(cx.listener(Self::bullets))
            .on_action(cx.listener(Self::numbers))
            .on_action(cx.listener(Self::tasks))
            .on_action(cx.listener(Self::quote))
            .on_action(cx.listener(Self::code_block))
            .on_action(cx.listener(Self::table))
            .on_action(cx.listener(Self::rule))
            .on_action(cx.listener(Self::diagram))
            .on_action(cx.listener(Self::math))
            .on_action(cx.listener(Self::link))
            .on_action(cx.listener(Self::toggle_source))
            .on_action(cx.listener(Self::set_mode_action))
            .on_action(cx.listener(Self::cancel))
            .size_full()
            .cursor(CursorStyle::IBeam)
            .child(EditorElement::new(cx.entity()))
            .children(overlay)
    }
}

#[cfg(test)]
mod tests;
