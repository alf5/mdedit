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

mod code;
mod element;
mod embed;
pub mod find_bar;
mod grid;
mod highlight;
mod overlays;
pub mod style;
pub(crate) mod table;
pub(crate) mod widgets;

use std::cell::Cell;
use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::component::menu::PopupMenu;
use gpui_kit::*;
use mdedit_core::buffer::{self, Change, Edit, Selection};
use mdedit_core::display::{self, DisplayLine, Gutter, Reveal, Role, Row, RowWidget};
use mdedit_core::history::{History, Kind};
use mdedit_core::prefs::Mode;
use mdedit_core::syntax::{self, LineKind, Syntax, WidgetKind};
use mdedit_core::{edit, find, fold, os};

use self::element::EditorElement;
use self::style::DocStyle;
use crate::actions::*;

#[derive(Clone)]
pub enum EditorEvent {
    /// The text changed.
    Edited,
    /// The caret or selection moved, or the mode changed.
    Moved,
    /// A `[[wiki link]]` was followed: the file its note is in, or `None`
    /// when no note has that name.
    FollowWiki { note: String, path: Option<PathBuf> },
    /// Files dropped on the text that it doesn't take in (not images):
    /// open them.
    Open(Vec<PathBuf>),
    /// Something to tell the user.
    Notice(String),
}

/// A block drawn on a card, a rounded box behind its rows, in the rendered
/// modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Card {
    /// A fenced code block.
    Code,
    /// The frontmatter's properties.
    Meta,
    /// A `> [!NOTE]` alert.
    Callout(syntax::Alert),
}

/// A row's place on its card: which card, and whether the row is the
/// card's first and last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OnCard {
    pub card: Card,
    pub first: bool,
    pub last: bool,
}

/// How far a card's text sits in from its edges.
pub(crate) const CARD_PAD_X: f32 = 14.;
pub(crate) const CARD_PAD_Y: f32 = 8.;
/// A code block's text sits further in, as ZenNotes has it.
pub(crate) const CODE_PAD_X: f32 = 16.;
/// From a code block's right edge to its language label's: the copy
/// button, the label's chevron, and the gaps round them.
pub(crate) const CODE_HEADER_RIGHT: f32 = 50.;

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
    /// The row's whole height; a widget's is measured each frame, since an
    /// image can arrive after the first one.
    pub height: Cell<Pixels>,
    /// The text's height (zero for a closed widget).
    pub text_h: Pixels,
    pub widget: Option<RowWidget>,
    pub alert: Option<mdedit_core::syntax::Alert>,
    /// A list marker drawn in the margin before the text.
    pub gutter: Option<ShapedLine>,
    /// A table, drawn and edited as a grid; the text fields are unused.
    pub grid: Option<grid::Grid>,
    pub card: Option<OnCard>,
    /// Where its card's left edge is, from the left of the reading column.
    pub card_x: Pixels,
    /// How deep in lists it's drawn, for the guides.
    pub list: u8,
    /// Its inline formulas: where each is in the display text, and its
    /// picture.
    pub maths: Vec<InlineMath>,
}

/// An inline formula in a row: where it is in the display text, its
/// picture, the picture's size, and how far down its baseline is.
pub(crate) type InlineMath = (Range<usize>, Arc<Image>, Size<Pixels>, Pixels);

/// An inline formula drawn: its picture, its size, and how far down its
/// baseline is; or why it couldn't be.
pub(crate) type InlinePicture = Result<(Arc<Image>, Size<Pixels>, Pixels), String>;

impl LineLayout {
    /// How many display offsets the row has.
    pub(crate) fn len(&self) -> usize {
        match &self.grid {
            Some(g) => g.len(),
            None => self.display.text.len(),
        }
    }

    pub(crate) fn to_display(&self, src: usize) -> usize {
        match &self.grid {
            Some(g) => g.to_display(src),
            None => self.display.to_display(src),
        }
    }

    pub(crate) fn to_source(&self, d: usize) -> usize {
        match &self.grid {
            Some(g) => g.to_source(d),
            None => self.display.to_source(d),
        }
    }

    pub(crate) fn prev(&self, d: usize) -> usize {
        match &self.grid {
            Some(g) => g.prev(d),
            None => self.display.prev(d),
        }
    }

    pub(crate) fn next(&self, d: usize) -> usize {
        match &self.grid {
            Some(g) => g.next(d),
            None => self.display.next(d),
        }
    }

    pub(crate) fn role_at(&self, d: usize) -> Role {
        match &self.grid {
            Some(g) => g.role_at(d),
            None => self.display.role_at(d),
        }
    }

    pub(crate) fn source_at(&self, d: usize) -> Option<Range<usize>> {
        match &self.grid {
            Some(g) => g.source_at(d),
            None => self.display.source_at(d),
        }
    }

    /// Rectangles covering display range `a..b`, for a row drawn at
    /// `origin`; `width` is the reading column's, from the text's start.
    pub(crate) fn rects(
        &self,
        origin: Point<Pixels>,
        a: usize,
        b: usize,
        to_eol: bool,
        width: Pixels,
    ) -> Vec<Bounds<Pixels>> {
        match &self.grid {
            Some(g) => g.rects(origin, a, b),
            None => grid::wrapped_rects(
                origin + point(self.indent, self.pad_top),
                &self.shaped,
                self.line_height,
                a,
                b,
                to_eol,
                width,
            ),
        }
    }

    /// Where the caret goes a line up or down from `p` without leaving
    /// the row, if it can.
    fn step(&self, p: Point<Pixels>, step: i32) -> Option<Pixels> {
        if let Some(g) = &self.grid {
            return g.step_y(p, step);
        }
        let y = p.y + self.line_height * step as f32;
        (y >= self.pad_top && y < self.pad_top + self.text_height()).then_some(y)
    }

    /// The first and last lines' tops, for coming in from above or below.
    fn first_y(&self) -> Pixels {
        match &self.grid {
            Some(g) => g.cells.first().map_or(self.pad_top, |c| c.origin.y),
            None => self.pad_top,
        }
    }

    fn last_y(&self) -> Pixels {
        match &self.grid {
            Some(g) => g.last_line_y(),
            None => self.pad_top + self.text_height() - self.line_height,
        }
    }

    /// The point of display offset `d`, relative to the line box.
    fn point(&self, d: usize) -> Point<Pixels> {
        if let Some(g) = &self.grid {
            return g.point(d);
        }
        let p = self
            .shaped
            .position_for_index(d, self.line_height)
            .unwrap_or_else(|| point(self.shaped.width(), px(0.)));
        point(p.x + self.indent, p.y + self.pad_top)
    }

    /// The display offset nearest `p`, relative to the line box.
    fn index(&self, p: Point<Pixels>) -> usize {
        if let Some(g) = &self.grid {
            return g.index(p);
        }
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
    /// Source lines drawn together (see `display::rows`); one slot per row.
    rows: Vec<Row>,
    /// Where relative image paths are looked up: the document's folder.
    base_dir: Option<PathBuf>,
    /// The project folder open beside it, where `[[wiki links]]` look for
    /// notes; an index of its files, made when a link is drawn and kept a
    /// few seconds; and which notes were found.
    root: Option<PathBuf>,
    /// The inline formula the caret is in, shown as its source; the others'
    /// pictures; and how wide the spaces are that hold their room, by size.
    math_open: Option<Range<usize>>,
    inline_pictures: HashMap<u64, InlinePicture>,
    room_widths: HashMap<u32, Vec<(char, f32)>>,
    wiki_index: Option<(Instant, Rc<mdedit_core::wiki::Index>)>,
    wiki_found: HashMap<String, Option<PathBuf>>,
    /// The pointer's over a link with Ctrl/Cmd held: a click follows it,
    /// and the pointer's a hand.
    link_hover: bool,
    /// Folding: the blocks that can fold, the folded ones (by their head
    /// line's first byte, so edits carry them along), the lines that hides,
    /// the head under the pointer (its chevron shows), and last frame's
    /// chevrons and `…` pills with the head each toggles.
    foldable: Vec<fold::Fold>,
    folded: BTreeSet<usize>,
    hidden: Vec<Range<usize>>,
    fold_hover: Option<usize>,
    pub(crate) fold_buttons: Vec<(Bounds<Pixels>, usize)>,
    /// How deep in embeds this editor is (0: a tab's own); one inside is
    /// read-only. The notes it embeds, by target, and the height its
    /// content took last frame.
    pub(crate) embedded: u8,
    embeds: HashMap<String, embed::Embedded>,
    /// PDF pages drawn for `![[file.pdf]]`, by file, page, width and the
    /// file's modified time.
    #[allow(clippy::type_complexity)]
    pdf_pages: HashMap<(PathBuf, usize, u32, Option<std::time::SystemTime>), embed::PdfPage>,
    content_h: Pixels,
    /// The note previewed under the pointer (Ctrl/Cmd held over its wiki
    /// link), and where; and one about to be.
    preview: Option<(String, Point<Pixels>)>,
    preview_wait: Option<(String, Task<()>)>,
    max_scroll: Pixels,
    /// Drawn diagrams and formulas, by source and look.
    pictures: HashMap<u64, widgets::Picture>,
    /// Highlighted code blocks, by content and language.
    code_colors: HashMap<u64, Rc<Vec<highlight::Span>>>,
    /// The palette's highlighting theme.
    code_theme: Rc<syntect::highlighting::Theme>,
    /// Images from the web load only once asked for, per document: fetching
    /// one tells its server the document was opened.
    pub(crate) allow_remote: bool,
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
    /// What it offers, and its entries for what's typed.
    pop: overlays::PopKind,
    pop_items: Vec<overlays::PopItem>,
    /// A table under the pointer, its controls showing.
    table_hover: Option<table::Hover>,
    /// A press on one of those controls, until the button comes up.
    table_drag: Option<table::Drag>,
    /// Column widths while a border is dragged.
    table_resize: Option<table::ResizeDrag>,
    /// A table's right-click menu: the menu, where, and its dismissal.
    menu: Option<(Entity<PopupMenu>, Point<Pixels>, Subscription)>,
    /// The code block (its header row) just copied, its button lit a moment.
    copied: Option<usize>,
}

impl EventEmitter<EditorEvent> for MarkdownEditor {}

impl Focusable for MarkdownEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// The clipboard's HTML flavour, if it has one. GPUI's clipboard only
/// knows plain text, so this asks the platform directly.
fn clipboard_html() -> Option<String> {
    arboard::Clipboard::new().ok()?.get().html().ok()
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
        let code_theme = Rc::new(highlight::theme(&style.palette));
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
            base_dir: None,
            root: None,
            math_open: None,
            inline_pictures: HashMap::new(),
            room_widths: HashMap::new(),
            wiki_index: None,
            wiki_found: HashMap::new(),
            link_hover: false,
            foldable: Vec::new(),
            folded: BTreeSet::new(),
            hidden: Vec::new(),
            fold_hover: None,
            fold_buttons: Vec::new(),
            embedded: 0,
            embeds: HashMap::new(),
            pdf_pages: HashMap::new(),
            content_h: px(0.),
            preview: None,
            preview_wait: None,
            max_scroll: px(0.),
            pictures: HashMap::new(),
            code_colors: HashMap::new(),
            code_theme,
            allow_remote: crate::state::AppState::try_prefs(cx).is_some_and(|p| p.remote_images),
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
            pop: overlays::PopKind::Slash,
            pop_items: Vec::new(),
            table_hover: None,
            table_drag: None,
            table_resize: None,
            menu: None,
            copied: None,
        };
        this.refresh_folds();
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

    #[cfg(test)]
    pub fn style(&self) -> &DocStyle {
        &self.style
    }

    pub fn set_style(&mut self, style: DocStyle, cx: &mut Context<Self>) {
        if self.style != style {
            if self.style.palette != style.palette {
                self.code_theme = Rc::new(highlight::theme(&style.palette));
                self.code_colors.clear();
            }
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

    pub(crate) fn invalidate(&mut self) {
        for slot in &mut self.slots {
            slot.layout = None;
        }
        self.wiki_found.clear();
    }

    fn estimate(&self, kind: LineKind) -> Pixels {
        px(self.style.metrics(kind, self.mode).1)
    }

    fn rebuild_slots(&mut self) {
        let old = std::mem::take(&mut self.slots);
        let caret_line = self.syntax.line_at(self.sel.head.min(self.text.len()));
        self.rows = display::hide(
            display::rows(&self.text, &self.syntax, self.rich(), Some(caret_line)),
            &self.hidden,
        );
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
                        || {
                            self.estimate(self.syntax.lines[r.lines.start].kind)
                                * r.lines.len() as f32
                        },
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
            .partition_point(|r| r.lines.end <= line)
            .min(self.rows.len().saturating_sub(1))
    }

    // ---------- folding ----------

    /// After a parse: what can fold now, which folds still stand (a head
    /// that stopped being one unfolds), and the lines they hide.
    fn refresh_folds(&mut self) {
        self.foldable = fold::folds(&self.syntax);
        let keep: BTreeSet<usize> = self
            .folded
            .iter()
            .copied()
            .filter(|&p| {
                let l = self.syntax.line_at(p.min(self.text.len()));
                self.syntax.lines[l].range.start == p && self.foldable.iter().any(|f| f.head == l)
            })
            .collect();
        self.folded = keep;
        self.rehide();
    }

    fn rehide(&mut self) {
        let heads: Vec<usize> = self
            .folded
            .iter()
            .map(|&p| self.syntax.line_at(p.min(self.text.len())))
            .collect();
        self.hidden = fold::hidden(&self.foldable, heads);
    }

    /// Carry the folds' head positions through `edits`, made to the text as
    /// it is now.
    fn carry_folds(&mut self, edits: &[Edit]) {
        if !self.folded.is_empty() {
            self.folded = self
                .folded
                .iter()
                .map(|&p| buffer::map_pos(p, edits, false))
                .collect();
        }
    }

    fn is_folded(&self, line: usize) -> bool {
        self.syntax
            .lines
            .get(line)
            .is_some_and(|l| self.folded.contains(&l.range.start))
    }

    pub(crate) fn can_fold(&self, line: usize) -> bool {
        self.foldable.iter().any(|f| f.head == line)
    }

    /// Unfold whatever hides `pos`'s line. Whether anything did.
    fn unfold_at(&mut self, pos: usize) -> bool {
        let line = self.syntax.line_at(pos.min(self.text.len()));
        if !self.hidden.iter().any(|h| h.contains(&line)) {
            return false;
        }
        let lines = &self.syntax.lines;
        let covering: Vec<usize> = self
            .foldable
            .iter()
            .filter(|f| f.body.contains(&line))
            .map(|f| lines[f.head].range.start)
            .collect();
        for p in covering {
            self.folded.remove(&p);
        }
        self.rehide();
        true
    }

    /// Fold or unfold the block headed by `head`. A caret folded away
    /// moves to the end of the head's row.
    pub(crate) fn set_fold(&mut self, head: usize, on: bool, cx: &mut Context<Self>) {
        let Some(f) = self.foldable.iter().find(|f| f.head == head).cloned() else {
            return;
        };
        let at = self.syntax.lines[head].range.start;
        if on {
            self.folded.insert(at);
        } else {
            self.folded.remove(&at);
        }
        self.rehide();
        let line = self.syntax.line_at(self.sel.head);
        if self.hidden.iter().any(|h| h.contains(&line)) {
            let end = self.syntax.lines[f.body.start - 1].range.end;
            self.sel = Selection::caret(end);
        }
        self.rebuild_slots();
        self.update_reveal();
        cx.notify();
    }

    pub(crate) fn toggle_fold(&mut self, head: usize, cx: &mut Context<Self>) {
        let on = !self.is_folded(head);
        self.set_fold(head, on, cx);
    }

    /// The innermost block the caret's line heads or sits in.
    fn fold_around_caret(&self) -> Option<usize> {
        let line = self.syntax.line_at(self.sel.head);
        self.foldable
            .iter()
            .filter(|f| f.head <= line && line < f.body.end)
            .map(|f| f.head)
            .max()
    }

    fn fold_at_caret(&mut self, _: &FoldAtCaret, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(head) = self.fold_around_caret() {
            self.set_fold(head, true, cx);
        }
    }

    fn unfold_at_caret(&mut self, _: &UnfoldAtCaret, _: &mut Window, cx: &mut Context<Self>) {
        let line = self.syntax.line_at(self.sel.head);
        let head = self
            .foldable
            .iter()
            .filter(|f| f.head <= line && line < f.body.start && self.is_folded(f.head))
            .map(|f| f.head)
            .max();
        if let Some(head) = head {
            self.set_fold(head, false, cx);
        }
    }

    fn fold_all(&mut self, _: &FoldAll, _: &mut Window, cx: &mut Context<Self>) {
        let lines = &self.syntax.lines;
        self.folded = self
            .foldable
            .iter()
            .map(|f| lines[f.head].range.start)
            .collect();
        self.rehide();
        // The caret's own block stays open rather than lose it.
        self.unfold_at(self.sel.head);
        self.rebuild_slots();
        self.update_reveal();
        cx.notify();
    }

    fn unfold_all(&mut self, _: &UnfoldAll, _: &mut Window, cx: &mut Context<Self>) {
        self.folded.clear();
        self.rehide();
        self.rebuild_slots();
        self.update_reveal();
        cx.notify();
    }

    /// The pointer over a foldable row shows its chevron.
    pub(crate) fn hover_fold(&mut self, p: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let head = p
            .and_then(|p| {
                self.geometry.lines.iter().find(|(ix, origin, _)| {
                    let h = self.slots.get(*ix).map_or(px(0.), |s| s.height);
                    p.y >= origin.y && p.y < origin.y + h
                })
            })
            .map(|(ix, ..)| self.rows[*ix].lines.start)
            .filter(|&l| self.can_fold(l));
        if head != self.fold_hover {
            self.fold_hover = head;
            cx.notify();
        }
    }

    /// Which head's chevron shows: a folded one always, an open one under
    /// the pointer.
    pub(crate) fn fold_mark(&self, ix: usize) -> Option<(usize, bool)> {
        let head = self.rows.get(ix)?.lines.start;
        if !self.can_fold(head) {
            return None;
        }
        let folded = self.is_folded(head);
        (folded || self.fold_hover == Some(head)).then_some((head, folded))
    }

    /// Lines between row `ix` and the next drawn one that folding hides.
    fn folded_after(&self, ix: usize) -> Option<usize> {
        let end = self.rows.get(ix)?.lines.end;
        let next = self
            .rows
            .get(ix + 1)
            .map_or(self.syntax.lines.len(), |r| r.lines.start);
        (next > end).then_some(end)
    }

    /// The source a row covers, without its final newline.
    fn row_range(&self, ix: usize) -> Range<usize> {
        let r = &self.rows[ix].lines;
        self.syntax.lines[r.start].range.start..self.syntax.lines[r.end - 1].range.end
    }

    fn recount(&mut self) {
        self.words = self.text.split_whitespace().count();
        self.chars = self.text.chars().count();
    }

    /// Apply a change as one undo step of `kind`.
    pub fn apply(&mut self, change: Change, kind: Kind, cx: &mut Context<Self>) {
        // An embed shows its note; it can't change it.
        if change.edits.is_empty() || self.is_embedded() {
            self.set_selection(change.selection, cx);
            return;
        }
        let before = self.sel;
        self.carry_folds(&change.edits);
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
        self.refresh_folds();
        self.unfold_at(self.sel.head);
        self.rebuild_slots();
        self.recount();
        self.update_reveal();
        self.refresh_find();
        self.update_slash(None, cx);
        self.autoscroll = true;
        self.goal_x = None;
        cx.emit(EditorEvent::Edited);
        cx.notify();
    }

    /// An embed's text, read again from its note.
    pub(crate) fn load(&mut self, text: String, cx: &mut Context<Self>) {
        self.text = text;
        self.sel = Selection::caret(0);
        self.history = History::default();
        self.after_edit(cx);
    }

    /// The height the content took when last drawn (an estimate before
    /// that), padding included.
    pub(crate) fn content_height(&self) -> Pixels {
        if self.content_h > px(0.) {
            self.content_h
        } else {
            self.slots
                .iter()
                .map(|s| s.height)
                .fold(px(24.), |a, b| a + b)
        }
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
        // In a grid the caret is always in a cell, never on a pipe.
        let sel = Selection::new(self.snap_to_cell(sel.anchor), self.snap_to_cell(sel.head));
        if sel != self.sel {
            self.history.seal();
            if self.mode == Mode::Live {
                // Live shows the caret cell's markup: those grids re-lay.
                for pos in [self.sel.head, sel.head] {
                    let ix = self.row_at(pos);
                    if let Some(slot) = self.slots.get_mut(ix)
                        && slot.layout.as_ref().is_some_and(|l| l.grid.is_some())
                    {
                        slot.layout = None;
                    }
                }
            }
            let was_in = self.widget_at(self.sel.head);
            self.sel = sel;
            if self.unfold_at(sel.head) {
                self.rebuild_slots();
            }
            let now_in = self.widget_at(self.sel.head);
            if was_in != now_in && self.rich() {
                // Leaving a table pads its source again (which re-lays the
                // rows); otherwise just open or close the widget.
                let left_table = was_in.filter(|w| {
                    self.syntax
                        .widgets
                        .get(*w)
                        .is_some_and(|w| w.kind == mdedit_core::syntax::WidgetKind::Table)
                });
                if let Some(w) = left_table {
                    self.tidy_table(w, cx);
                }
                self.rebuild_slots();
            }
            self.update_reveal();
            self.update_slash(None, cx);
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
        // The inline formula with the caret in it shows its source.
        let head = self.sel.head;
        let open = self
            .rich()
            .then(|| {
                self.syntax
                    .maths
                    .iter()
                    .find(|f| f.start < head && head < f.end)
                    .cloned()
            })
            .flatten();
        if open != self.math_open {
            for f in self.math_open.iter().chain(&open) {
                let ix = self.row_at(f.start);
                if let Some(slot) = self.slots.get_mut(ix) {
                    slot.layout = None;
                }
            }
            self.math_open = open;
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

    /// Where `[[wiki links]]` look for notes beyond the document's folder.
    pub fn set_root(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.root != root {
            self.root = root;
            self.wiki_index = None;
            self.wiki_found.clear();
            self.invalidate();
            cx.notify();
        }
    }

    /// The index of files wiki links are looked up in: the open folder's,
    /// or the document's own folder's. Rebuilt when ten seconds old.
    pub(crate) fn wiki_files(&mut self) -> Option<Rc<mdedit_core::wiki::Index>> {
        let base = self.root.clone().or_else(|| self.base_dir.clone());
        let stale = self.wiki_index.as_ref().is_none_or(|(at, ix)| {
            at.elapsed().as_secs() > 10 || Some(ix.root()) != base.as_deref()
        });
        if stale {
            self.wiki_index =
                base.map(|b| (Instant::now(), Rc::new(mdedit_core::wiki::Index::build(&b))));
        }
        self.wiki_index.as_ref().map(|(_, ix)| ix.clone())
    }

    /// The file a wiki link's note is in, if there's one: in the document's
    /// folder, or by name anywhere in the project. Remembered until the
    /// layout is next rebuilt.
    pub(crate) fn wiki_note(&mut self, note: &str) -> Option<PathBuf> {
        if let Some(found) = self.wiki_found.get(note) {
            return found.clone();
        }
        self.wiki_files();
        let found = mdedit_core::wiki::resolve(
            note,
            self.base_dir.as_deref(),
            self.root.as_deref(),
            self.wiki_index.as_ref().map(|(_, ix)| ix.as_ref()),
        );
        self.wiki_found.insert(note.to_string(), found.clone());
        found
    }

    /// Whether a wiki link goes anywhere: to a note, or to a heading here.
    fn wiki_reaches(&mut self, target: &str) -> bool {
        match mdedit_core::wiki::split(target) {
            ("", Some(part)) => self.heading_named(part).is_some(),
            ("", None) => false,
            (note, _) => self.wiki_note(note).is_some(),
        }
    }

    /// The first line of this document's heading called `name`, whatever its
    /// case.
    fn heading_named(&self, name: &str) -> Option<usize> {
        self.syntax
            .headings
            .iter()
            .find(|h| h.text.eq_ignore_ascii_case(name))
            .map(|h| h.line)
    }

    /// Follow a wiki link: to a heading here, or out to its note.
    fn follow_wiki(&mut self, target: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (note, part) = mdedit_core::wiki::split(target);
        if note.is_empty() {
            if let Some(line) = part.and_then(|p| self.heading_named(p)) {
                let at = self.syntax.lines[line].range.start;
                self.reveal_offset(at, window, cx);
            }
            return;
        }
        // Look again: the note may have been made since it was drawn.
        self.wiki_found.remove(note);
        self.wiki_index = None;
        let path = self.wiki_note(note);
        cx.emit(EditorEvent::FollowWiki {
            note: note.to_string(),
            path,
        });
    }

    /// Copy the code block whose header is row `ix`: its lines between the
    /// fences, as they are.
    pub(crate) fn copy_code(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix) else {
            return;
        };
        let body: Vec<&str> = self.syntax.lines[row.lines.start + 1..]
            .iter()
            .take_while(|l| l.kind == LineKind::Code)
            .map(|l| &self.text[l.range.clone()])
            .collect();
        cx.write_to_clipboard(ClipboardItem::new_string(body.join("\n")));
        self.copied = Some(ix);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1200))
                .await;
            this.update(cx, |ed, cx| {
                if ed.copied == Some(ix) {
                    ed.copied = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// An inline formula's picture, at text size `size`, in the text's
    /// colour.
    fn inline_picture(&mut self, body: &str, text_size: f32) -> InlinePicture {
        let p = &self.style.palette;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&(body, text_size.to_bits(), p.text), &mut h);
        let key = std::hash::Hasher::finish(&h);
        if let Some(found) = self.inline_pictures.get(&key) {
            return found.clone();
        }
        // KaTeX's inline formulas are a little larger than their text.
        let size_pt = text_size * 0.75 * 1.1;
        let drawn = mdedit_core::math::draw(
            body,
            false,
            size_pt as f64,
            [p.text[0], p.text[1], p.text[2]],
        )
        .map(|m| {
            let pt = |v: f64| px(v as f32 * 4. / 3.);
            (
                Arc::new(Image::from_bytes(ImageFormat::Svg, m.svg.into_bytes())),
                size(pt(m.width_pt), pt(m.height_pt)),
                pt(m.baseline_pt),
            )
        });
        if self.inline_pictures.len() > 400 {
            self.inline_pictures.clear();
        }
        self.inline_pictures.insert(key, drawn.clone());
        drawn
    }

    /// Spaces as wide as `width` at text size `size`, to hold a picture's
    /// room in the text: spaces that don't break (figure, no-break, narrow
    /// no-break), then hair spaces for the last pixel or so.
    fn room(&mut self, width: Pixels, size: Pixels, window: &mut Window) -> String {
        let widths = self
            .room_widths
            .entry(f32::from(size).to_bits())
            .or_insert_with(|| {
                ['\u{2007}', '\u{00A0}', '\u{202F}', '\u{200A}']
                    .into_iter()
                    .map(|c| {
                        let run = TextRun {
                            len: c.len_utf8(),
                            font: font(crate::theme::UI_FONT),
                            color: Hsla::default(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let w = window
                            .text_system()
                            .shape_line(c.to_string().into(), size, &[run], None)
                            .width;
                        (c, f32::from(w))
                    })
                    .filter(|(_, w)| *w > 0.)
                    .collect()
            });
        let mut left = f32::from(width);
        let mut out = String::new();
        let last = widths.len().saturating_sub(1);
        for (i, (c, w)) in widths.iter().enumerate() {
            let n = if i == last {
                (left / w).round().max(0.)
            } else {
                (left / w).floor().max(0.)
            };
            for _ in 0..n as usize {
                out.push(*c);
            }
            left -= n * w;
        }
        if out.is_empty() {
            out.push('\u{200A}');
        }
        out
    }

    /// The inline formulas in source range `src` that are drawn, at text
    /// size `size`: each one's room and picture. The one the caret's in
    /// isn't, nor one that can't be drawn.
    pub(crate) fn formulas_in(
        &mut self,
        src: Range<usize>,
        size: f32,
        window: &mut Window,
    ) -> HashMap<Range<usize>, (String, InlinePicture)> {
        // They're in source order.
        let from = self.syntax.maths.partition_point(|f| f.start < src.start);
        let formulas: Vec<Range<usize>> = self.syntax.maths[from..]
            .iter()
            .take_while(|f| f.start < src.end)
            .filter(|f| f.end <= src.end)
            .filter(|f| Some(*f) != self.math_open.as_ref())
            .cloned()
            .collect();
        let mut drawn = HashMap::new();
        for f in formulas {
            let body = self.text[f.clone()].trim_matches('$').to_string();
            let picture = self.inline_picture(&body, size);
            if let Ok((_, s, _)) = &picture {
                let room = self.room(s.width, px(size), window);
                drawn.insert(f, (room, picture));
            }
        }
        drawn
    }

    /// How many `$`s open (and close) the formula at `f`.
    fn math_delims(&self, f: &Range<usize>) -> usize {
        self.text[f.clone()]
            .chars()
            .take_while(|c| *c == '$')
            .count()
            .clamp(1, 2)
    }

    /// The inline formula drawn under `p` in row `ix`, if one is.
    fn formula_at(&self, ix: usize, layout: &LineLayout, p: Point<Pixels>) -> Option<Range<usize>> {
        let (_, origin, _) = self.geometry.lines.iter().find(|(i, ..)| *i == ix)?;
        let width = self.content_w? - layout.indent;
        let cells = layout.grid.iter().flat_map(|g| &g.cells);
        let maths = layout.maths.iter().chain(cells.flat_map(|c| &c.maths));
        maths.into_iter().find_map(|(range, ..)| {
            let r = layout
                .rects(*origin, range.start, range.end, false, width)
                .into_iter()
                .next()?;
            if r.contains(&p) {
                layout.source_at(range.start)
            } else {
                None
            }
        })
    }

    /// The card source line `ix` is drawn on, if any.
    fn card_of(&self, ix: usize) -> Option<Card> {
        let l = self.syntax.lines.get(ix)?;
        match l.kind {
            LineKind::Code | LineKind::FenceOpen | LineKind::FenceClose => Some(Card::Code),
            LineKind::Meta => Some(Card::Meta),
            _ => l.alert.map(Card::Callout),
        }
    }

    /// Where a row sits on its card: a card starts where the line before
    /// isn't on one like it (or a fence opens) and ends where the line after
    /// isn't (or a fence closes).
    fn on_card(&self, row: &Row) -> Option<OnCard> {
        let (start, end) = (row.lines.start, row.lines.end);
        let card = self.card_of(start)?;
        let kind = |i: usize| self.syntax.lines.get(i).map(|l| l.kind);
        let first = start == 0
            || self.card_of(start - 1) != Some(card)
            || kind(start) == Some(LineKind::FenceOpen);
        let last = self.is_folded(start)
            || self.card_of(end) != Some(card)
            || kind(end - 1) == Some(LineKind::FenceClose)
            || kind(end) == Some(LineKind::FenceOpen);
        Some(OnCard { card, first, last })
    }

    // ---------- layout ----------

    fn layout(&mut self, ix: usize, window: &mut Window) -> Rc<LineLayout> {
        if let Some(l) = self.slots.get(ix).and_then(|s| s.layout.clone()) {
            return l;
        }
        let content_w = self.content_w.unwrap_or(px(700.));
        let row = self.rows[ix].clone();
        let line = self.syntax.lines[row.lines.start].clone();
        if let Some(RowWidget::Closed(w)) = row.widget
            && self.syntax.widgets[w].kind == WidgetKind::Table
            && let Some(g) = self.layout_grid(w, content_w, Some(self.sel.head), window)
        {
            let range = self.row_range(ix);
            let height = g.row_y.last().copied().unwrap_or_default() + px(grid::BOTTOM);
            let layout = Rc::new(LineLayout {
                display: DisplayLine {
                    segs: vec![display::Seg {
                        display: 0..0,
                        source: range.end..range.end,
                        copy: true,
                    }],
                    source: range,
                    ..Default::default()
                },
                shaped: WrappedLine::default(),
                kind: line.kind,
                quote: 0,
                line_height: g.line_height,
                pad_top: px(0.),
                indent: px(0.),
                height: Cell::new(height),
                text_h: height,
                widget: None,
                alert: None,
                gutter: None,
                grid: Some(g),
                card: None,
                card_x: px(0.),
                list: 0,
                maths: Vec::new(),
            });
            if let Some(slot) = self.slots.get_mut(ix) {
                slot.height = height;
                slot.layout = Some(layout.clone());
            }
            return layout;
        }
        if let Some(RowWidget::Closed(_)) = row.widget {
            // Drawn by `widget_element`; no text of its own. The caret never
            // rests inside it: going in opens it.
            let range = self.row_range(ix);
            let display = DisplayLine {
                segs: vec![display::Seg {
                    display: 0..0,
                    source: range.end..range.end,
                    copy: true,
                }],
                source: range,
                ..Default::default()
            };
            let (_, lh) = self.style.metrics(LineKind::Paragraph, self.mode);
            let layout = Rc::new(LineLayout {
                gutter: None,
                display,
                shaped: WrappedLine::default(),
                kind: line.kind,
                quote: 0,
                line_height: px(lh),
                pad_top: px(0.),
                indent: px(0.),
                height: Cell::new(self.slots.get(ix).map_or(px(lh), |s| s.height)),
                text_h: px(0.),
                widget: row.widget,
                alert: None,
                grid: None,
                card: None,
                card_x: px(0.),
                list: 0,
                maths: Vec::new(),
            });
            if let Some(slot) = self.slots.get_mut(ix) {
                slot.layout = Some(layout.clone());
            }
            return layout;
        }
        // Inline formulas: pictures, each in room held by spaces (the one
        // the caret's in shows its source).
        let drawn = if self.rich() && self.reveal_of(ix) == Reveal::Hide {
            let (size, _) = self.style.metrics(line.kind, self.mode);
            self.formulas_in(self.row_range(ix), size, window)
        } else {
            HashMap::new()
        };
        let hook = |f: Range<usize>| drawn.get(&f).map(|(room, _)| room.clone());
        let display = display::display_row_with(
            &self.text,
            &self.syntax,
            row.lines.clone(),
            self.reveal_of(ix),
            (self.rich() && self.reveal_of(ix) == Reveal::Hide)
                .then_some(&hook as display::MathHook),
        );
        let maths = display
            .segs
            .iter()
            .filter(|seg| !seg.copy)
            .filter_map(|seg| {
                let (_, picture) = drawn.get(&seg.source)?;
                let (image, s, base) = picture.as_ref().ok()?;
                Some((seg.display.clone(), image.clone(), *s, *base))
            })
            .collect::<Vec<_>>();
        let (size, lh) = self.style.metrics(line.kind, self.mode);
        let (mut size, lh) = (px(size), px(lh));
        let rich = self.rich();
        let revealed = self.reveal_of(ix) == Reveal::Show;
        // A code block's opening fence, hidden, is its card's header: the
        // language as a small label at the right, by the copy button.
        let header = rich && !revealed && line.kind == LineKind::FenceOpen;
        if header {
            size = px(11. * self.style.size / 16.);
        }
        let list_depth = match (rich, revealed) {
            (false, _) => 0,
            (true, true) => line.list.saturating_sub(1),
            (true, false) => line.list,
        };
        let card = if rich { self.on_card(&row) } else { None };
        // A card's text sits in from its edges; a callout's card takes the
        // place of its quote's bar.
        let (card_in, quote_levels) = match card.map(|c| c.card) {
            Some(Card::Code) => (px(CODE_PAD_X), line.quote as f32),
            Some(Card::Meta) => (px(CARD_PAD_X), 0.),
            Some(Card::Callout(_)) => (px(CARD_PAD_X), line.quote.saturating_sub(1) as f32),
            None => (px(0.), line.quote as f32),
        };
        let card_x = if rich {
            px(quote_levels * 20. + list_depth as f32 * LIST_INDENT)
        } else {
            px(0.)
        };
        let mut indent = card_x + card_in;
        let wrap = max(content_w - indent - card_in, px(40.));
        let head = card.is_some_and(|k| matches!(k.card, Card::Callout(_)) && k.first);
        let mut runs = self.text_runs(
            &display,
            line.kind,
            line.quote,
            (line.alert, head),
            revealed,
        );
        if header {
            for r in &mut runs {
                r.font = font(crate::theme::UI_FONT);
            }
        }
        // A wiki link that goes nowhere is drawn quietly, as ZenNotes does.
        if rich {
            let mut at = 0;
            for (i, r) in display.runs.iter().enumerate() {
                if display::has(r.style, display::WIKI) && r.role == Role::Text {
                    let src = display.to_source(at);
                    let target = self.syntax.link_at(src).map(|l| l.url.clone());
                    if let Some(t) = target
                        && !self.wiki_reaches(&t)
                        && let Some(run) = runs.get_mut(i)
                    {
                        run.color = self.style.colors.muted;
                        run.underline = Some(UnderlineStyle {
                            thickness: px(1.),
                            color: Some(self.style.colors.faint),
                            wavy: false,
                        });
                    }
                }
                at += r.len;
            }
        }
        if line.kind == LineKind::Code
            && let Some(spans) = self.code_spans(row.lines.start)
        {
            let start = self.syntax.lines[row.lines.start].range.start;
            let local: Vec<highlight::Span> = spans
                .iter()
                .filter_map(|s| {
                    let a = s.range.start.max(start);
                    let b = s.range.end.min(display.source.end);
                    (a < b).then(|| highlight::Span {
                        range: a - start..b - start,
                        ..s.clone()
                    })
                })
                .collect();
            runs = highlight::apply(runs, &local, display.text.len());
        }
        let shaped = window
            .text_system()
            .shape_text(display.text.clone().into(), size, &runs, Some(wrap), None)
            .ok()
            .and_then(|mut v| (!v.is_empty()).then(|| v.remove(0)))
            .unwrap_or_default();
        if header {
            let label = shaped.width();
            indent = max(content_w - px(CODE_HEADER_RIGHT) - label, card_x + card_in);
        }
        // ZenNotes' spacing: no more round a heading than its own leading,
        // and blank lines as tall as any other.
        let mut pad_top = px(0.);
        let mut pad_bottom = px(0.);
        if let Some(OnCard {
            card: Card::Callout(_),
            first,
            last,
        }) = card
        {
            if first {
                pad_top += px(CARD_PAD_Y);
            }
            if last {
                pad_bottom = px(CARD_PAD_Y);
            }
        }
        // Room for formulas taller than the line.
        let (above, below) = grid::overflow(
            &shaped,
            lh,
            maths.iter().map(|(r, _, s, base)| (r.start, *s, *base)),
        );
        pad_top += above;
        pad_bottom += below;
        let text_h = shaped.size(lh).height;
        let height = match line.kind {
            LineKind::FenceOpen if header => lh,
            // The frontmatter's hidden fences are its card's padding.
            LineKind::Meta if rich && display.text.is_empty() => px(CARD_PAD_Y),
            _ => text_h + pad_top + pad_bottom,
        };
        let c = &self.style.colors;
        let number = (rich && self.style.numbered && matches!(line.kind, LineKind::Heading(_)))
            .then(|| self.syntax.heading_numbers().remove(&row.lines.start))
            .flatten();
        let gutter_glyph = number.map(|n| (n, c.muted)).or_else(|| {
            display.gutter.as_ref().and_then(|g| match g {
                Gutter::Bullet => Some(("•".to_string(), c.muted)),
                Gutter::Number(n) => Some((n.clone(), c.accent_soft)),
                Gutter::Task { .. } => None,
            })
        });
        let gutter = gutter_glyph.map(|(glyph, color)| {
            let mut f = font(self.style.font.clone());
            if glyph == "•" {
                f.weight = FontWeight::BOLD;
            }
            let run = TextRun {
                len: glyph.len(),
                font: f,
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            window
                .text_system()
                .shape_line(glyph.into(), size, &[run], None)
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
            height: Cell::new(height),
            text_h: height,
            widget: row.widget,
            alert: line.alert,
            grid: None,
            card,
            card_x,
            list: list_depth,
            maths,
        });
        if let Some(slot) = self.slots.get_mut(ix) {
            if row.widget.is_none() {
                slot.height = height;
            }
            slot.layout = Some(layout.clone());
        }
        layout
    }

    /// The colours of the fenced code block holding line `line`, as spans
    /// in document offsets; `None` outside a block with a known language.
    fn code_spans(&mut self, line: usize) -> Option<Rc<Vec<highlight::Span>>> {
        let open = (0..line)
            .rev()
            .find(|&i| self.syntax.lines[i].kind != LineKind::Code)?;
        let fence = &self.syntax.lines[open];
        if fence.kind != LineKind::FenceOpen {
            return None;
        }
        let lang = self.text[fence.lang.clone()?]
            .split_whitespace()
            .next()?
            .to_string();
        let body_start = fence.range.end + 1;
        let close = (line..self.syntax.lines.len())
            .find(|&i| self.syntax.lines[i].kind != LineKind::Code)
            .unwrap_or(self.syntax.lines.len());
        let body_end = if close < self.syntax.lines.len() {
            self.syntax.lines[close].range.start
        } else {
            self.text.len()
        };
        let body = &self.text[body_start..body_end.max(body_start)];
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&(body, &lang), &mut h);
        let key = std::hash::Hasher::finish(&h);
        if let Some(s) = self.code_colors.get(&key) {
            return Some(s.clone());
        }
        let spans: Vec<highlight::Span> = highlight::highlight(body, &lang, &self.code_theme)
            .into_iter()
            .map(|s| highlight::Span {
                range: s.range.start + body_start..s.range.end + body_start,
                ..s
            })
            .collect();
        if self.code_colors.len() > 64 {
            self.code_colors.clear();
        }
        let spans = Rc::new(spans);
        self.code_colors.insert(key, spans.clone());
        Some(spans)
    }

    /// How a row's text is set, span by span: ZenNotes' editor's look.
    /// `alert` is the callout the row is in, and `head` whether it's the
    /// callout's title line; `shown` whether its markup is showing (Live
    /// mode's caret row).
    fn text_runs(
        &self,
        d: &DisplayLine,
        kind: LineKind,
        quote: u8,
        (alert, head): (Option<syntax::Alert>, bool),
        shown: bool,
    ) -> Vec<TextRun> {
        let c = &self.style.colors;
        let mono = self.style.monospace(kind, self.mode);
        let source = self.mode == Mode::Source;
        let heading = matches!(kind, LineKind::Heading(_) | LineKind::SetextUnderline);
        let meta = kind == LineKind::Meta && !source;
        let base_color = if source {
            if self.style.muted_body {
                c.muted
            } else {
                c.text
            }
        } else if alert.is_some() {
            c.text
        } else if quote > 0 || kind == LineKind::Heading(6) {
            c.soft
        } else if self.style.muted_body {
            c.muted
        } else {
            c.text
        };
        d.runs
            .iter()
            .map(|r| {
                let code = display::has(r.style, display::CODE);
                let mut f = font(if matches!(r.role, Role::Pad | Role::Math) {
                    // A thin space in a proportional face: a few pixels.
                    crate::theme::UI_FONT.into()
                } else if (mono || code) && !meta {
                    self.style.mono.clone()
                } else {
                    self.style.font.clone()
                });
                if mono || code {
                    // `<!--` should read as itself, not as an arrow.
                    f.features = FontFeatures::disable_ligatures();
                }
                if (heading && !source)
                    || display::has(r.style, display::BOLD)
                    || r.role == Role::Bullet
                {
                    f.weight = FontWeight::BOLD;
                } else if r.role == Role::Alert || (head && r.role == Role::Text) {
                    f.weight = FontWeight::SEMIBOLD;
                } else if display::has(r.style, display::KEY) {
                    f.weight = FontWeight::MEDIUM;
                }
                // Markup shown at the caret leans, as ZenNotes sets it; a
                // heading's `#` stays upright.
                if display::has(r.style, display::ITALIC)
                    || (r.role == Role::Markup && !source && !heading)
                {
                    f.style = FontStyle::Italic;
                }
                if r.role == Role::Math {
                    // A formula's room is measured upright and regular.
                    f.weight = FontWeight::NORMAL;
                    f.style = FontStyle::Normal;
                }
                let link = display::has(r.style, display::LINK | display::IMAGE);
                let color = match r.role {
                    Role::Markup if source => c.muted,
                    Role::Markup => c.accent_soft,
                    Role::Bullet => c.muted,
                    Role::Number if source => c.muted,
                    Role::Number => c.accent_soft,
                    // A fence's language: purple as written, grey as a label.
                    Role::Lang if shown || source => c.alerts[2],
                    Role::Lang => c.muted,
                    Role::Alert => alert.map_or(c.accent, |a| c.callout(a)),
                    Role::TaskOpen | Role::TaskDone => c.alerts[3],
                    Role::Text if display::has(r.style, display::KEY) => c.muted,
                    Role::Text if display::has(r.style, display::TAG) => c.accent,
                    Role::Text if link => c.accent,
                    Role::Text if display::has(r.style, display::MATH) => c.accent,
                    Role::Text if heading && source => c.accent,
                    // A callout's own title, in its colour.
                    Role::Text if head && !source => alert.map_or(base_color, |a| c.callout(a)),
                    Role::Text | Role::Break | Role::Pad | Role::Math => base_color,
                };
                TextRun {
                    len: r.len,
                    font: f,
                    color,
                    // Inline code's chip is drawn behind the text, rounded.
                    background_color: None,
                    underline: (link && r.role == Role::Text).then_some(UnderlineStyle {
                        thickness: px(1.),
                        color: Some(c.accent.opacity(0.4)),
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
        let d = layout.to_display(self.sel.head);
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
        // In a table, Left and Right stop at the edges of the cell; Tab is
        // what moves between cells.
        if let Some(g) = &layout.grid
            && !extend
        {
            let cell = &g.cells[g.cell_of(d)];
            if (forward && d >= cell.base + cell.display.text.len()) || (!forward && d <= cell.base)
            {
                return;
            }
        }
        // Into a formula: its source opens, the caret inside its `$`s.
        if !extend && self.rich() {
            let into = if forward {
                (layout.role_at(d) == Role::Math).then_some(d)
            } else {
                (d > 0 && layout.role_at(layout.prev(d)) == Role::Math).then(|| layout.prev(d))
            };
            if let Some(f) = into.and_then(|at| layout.source_at(at)) {
                let n = self.math_delims(&f);
                let pos = if forward { f.start + n } else { f.end - n };
                return self.move_to(pos, false, cx);
            }
        }
        let pos = if forward {
            if d < layout.len() {
                // Past the last character: the end of the source line, after
                // any markup closing it.
                layout.to_source(layout.next(d))
            } else if ix + 1 < self.rows.len() {
                let next = self.layout(ix + 1, window);
                next.to_source(0)
            } else {
                self.text.len()
            }
        } else if d > 0 {
            layout.to_source(layout.prev(d))
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
            if let Some(next_y) = layout.step(point(x, y), step) {
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
                layout.first_y()
            } else {
                layout.last_y()
            };
        }
        let idx = layout.index(point(x, y));
        let pos = layout.to_source(idx);
        let goal = self.goal_x;
        self.move_to(pos, extend, cx);
        self.goal_x = goal;
    }

    /// Home and End: the start and end of the wrapped line the caret is on.
    /// Home again goes to the very start, before any hidden markup.
    fn line_edge(&mut self, end: bool, extend: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.goal_x = None;
        let (ix, d, layout) = self.caret_display(window);
        if let Some(g) = &layout.grid {
            let pos = layout.to_source(g.line_edge(d, end));
            return self.move_to(pos, extend, cx);
        }
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
        let n = self.pop_items.len().max(1) as isize;
        self.slash_ix = (self.slash_ix as isize + delta).rem_euclid(n) as usize;
        cx.notify();
        true
    }

    /// Enter or Tab picks the highlighted `/` entry.
    fn slash_pick(&mut self, cx: &mut Context<Self>) -> bool {
        if self.slash.is_none() {
            return false;
        }
        self.apply_pop(self.slash_ix, cx);
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
        if let Some((edits, at)) = self.table_safe_edits(r.clone(), s) {
            return self.apply(
                Change {
                    edits,
                    selection: Selection::caret(at),
                },
                kind,
                cx,
            );
        }
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
        if let Some((edits, at)) = self.table_safe_edits(r.clone(), "") {
            return self.apply(
                Change {
                    edits,
                    selection: Selection::caret(at),
                },
                Kind::Typing,
                cx,
            );
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
            && self.grid_widget(self.sel.head).is_none()
            && let Some(c) = edit::backspace_prefix(&self.text, self.sel)
        {
            let c = edit::then_renumber(&self.text, c);
            return self.apply(c, Kind::Other, cx);
        }
        if self.rich()
            && !matches!(self.caret_kind(), LineKind::Code | LineKind::FenceOpen)
            && let Some(c) = edit::unpair(&self.text, self.sel)
        {
            return self.apply(c, Kind::Typing, cx);
        }
        let caret = self.sel.head;
        let (ix, d, layout) = self.caret_display(window);
        // A cell keeps its place: Backspace at its start does nothing.
        if let Some(g) = &layout.grid
            && d <= g.cells[g.cell_of(d)].base
        {
            return;
        }
        let range = if d == 0 {
            if ix == 0 {
                return;
            }
            // Backspace after a folded block opens it rather than eat it.
            if let Some(line) = self.folded_after(ix - 1) {
                let at = self.syntax.lines[line].range.start;
                if self.unfold_at(at) {
                    self.rebuild_slots();
                    cx.notify();
                }
                return;
            }
            self.row_range(ix - 1).end..caret
        } else {
            let prev = layout.prev(d);
            match layout.role_at(prev) {
                // A drawn element, or a cell's line break: all of its source.
                Role::Bullet | Role::TaskOpen | Role::TaskDone | Role::Break => {
                    layout.source_at(prev).unwrap_or(caret..caret)
                }
                _ => {
                    let s = layout.to_source(prev);
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
        // Nor Delete at its end.
        if let Some(g) = &layout.grid {
            let cell = &g.cells[g.cell_of(d)];
            if d >= cell.base + cell.display.text.len() {
                return;
            }
        }
        let range = if d >= layout.len() {
            // So does Delete before one.
            if let Some(line) = self.folded_after(ix) {
                let at = self.syntax.lines[line].range.start;
                if self.unfold_at(at) {
                    self.rebuild_slots();
                    cx.notify();
                }
                return;
            }
            let end = self.row_range(ix).end;
            caret..(end + 1).min(self.text.len()).max(caret)
        } else {
            match layout.role_at(d) {
                Role::Break => layout.source_at(d).unwrap_or(caret..caret),
                _ => {
                    let s = layout.to_source(d);
                    s..buffer::next_char(&self.text, s)
                }
            }
        };
        let range = range.start.min(caret)..range.end;
        self.delete_range(range, cx);
    }

    fn delete_word_left(&mut self, _: &DeleteWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let r = if self.sel.is_empty() {
            let mut from = buffer::prev_word(&self.text, self.sel.head);
            if let Some(cell) = self.cell_range(self.sel.head) {
                from = from.max(cell.start);
            }
            from..self.sel.head
        } else {
            self.sel.range()
        };
        self.delete_range(r, cx);
    }

    fn delete_word_right(&mut self, _: &DeleteWordRight, _: &mut Window, cx: &mut Context<Self>) {
        let r = if self.sel.is_empty() {
            let mut to = buffer::next_word(&self.text, self.sel.head);
            if let Some(cell) = self.cell_range(self.sel.head) {
                to = to.min(cell.end);
            }
            self.sel.head..to
        } else {
            self.sel.range()
        };
        self.delete_range(r, cx);
    }

    /// Brackets and quotes typed in prose come in pairs (`edit::pair`);
    /// Source mode and code blocks type exactly what is pressed.
    fn auto_pair(&self, typed: &str) -> Option<Change> {
        let mut chars = typed.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else {
            return None;
        };
        if !self.rich() || matches!(self.caret_kind(), LineKind::Code | LineKind::FenceOpen) {
            return None;
        }
        edit::pair(&self.text, self.sel, ch)
    }

    fn caret_kind(&self) -> LineKind {
        self.syntax.lines[self.syntax.line_at(self.sel.head)].kind
    }

    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        if self.slash_pick(cx) || self.table_key(table::TableKey::Down, cx) {
            return;
        }
        let in_code = matches!(self.caret_kind(), LineKind::Code | LineKind::FenceOpen);
        let c = edit::enter(&self.text, self.sel, self.rich(), in_code);
        let c = if in_code {
            c
        } else {
            edit::then_renumber(&self.text, c)
        };
        self.apply(c, Kind::Other, cx);
    }

    fn soft_break(&mut self, _: &SoftBreak, _: &mut Window, cx: &mut Context<Self>) {
        // In a table cell, Shift+Enter breaks the line the only way a cell
        // can hold one.
        if self.grid_widget(self.sel.head).is_some() {
            if self.cell_range(self.sel.start()).is_some() {
                self.replace_selection("<br>", Kind::Other, cx);
            }
            return;
        }
        let c = edit::soft_break(&self.text, self.sel);
        self.apply(c, Kind::Other, cx);
    }

    fn indent(&mut self, _: &Indent, _: &mut Window, cx: &mut Context<Self>) {
        if self.slash_pick(cx) || self.table_key(table::TableKey::Next, cx) {
            return;
        }
        match edit::indent(&self.text, self.sel) {
            Some(c) => {
                let c = edit::then_renumber(&self.text, c);
                self.apply(c, Kind::Other, cx)
            }
            None if !self.rich() || self.caret_kind() == LineKind::Code => {
                self.replace_selection("    ", Kind::Typing, cx)
            }
            None => {}
        }
    }

    fn outdent(&mut self, _: &Outdent, _: &mut Window, cx: &mut Context<Self>) {
        if self.table_key(table::TableKey::Prev, cx) {
            return;
        }
        if let Some(c) = edit::outdent(&self.text, self.sel) {
            let c = edit::then_renumber(&self.text, c);
            self.apply(c, Kind::Other, cx);
        }
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        let before = (!self.folded.is_empty()).then(|| self.text.clone());
        if let Some(sel) = self.history.undo(&mut self.text) {
            if let Some(before) = before {
                self.carry_folds(&[buffer::diff(&before, &self.text)]);
            }
            self.sel = sel.clamp(self.text.len());
            self.after_edit(cx);
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        let before = (!self.folded.is_empty()).then(|| self.text.clone());
        if let Some(sel) = self.history.redo(&mut self.text) {
            if let Some(before) = before {
                self.carry_folds(&[buffer::diff(&before, &self.text)]);
            }
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

    /// Smart paste: rich content (a web page, Word, Google Docs) arrives as
    /// HTML and goes in as markdown. Ctrl/Cmd+Shift+V pastes plain text.
    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        // An image (a screenshot, one copied in a browser) is saved beside
        // the document; copied image files are brought in as a drop is.
        if let Some(item) = cx.read_from_clipboard() {
            for entry in item.entries() {
                match entry {
                    ClipboardEntry::Image(image) => {
                        let image = image.clone();
                        return self.paste_image(&image, cx);
                    }
                    ClipboardEntry::ExternalPaths(paths) => {
                        let images: Vec<PathBuf> = paths
                            .paths()
                            .iter()
                            .filter(|p| mdedit_core::project::image_mime(p).is_some())
                            .cloned()
                            .collect();
                        if !images.is_empty() {
                            return self.insert_images(&images, cx);
                        }
                    }
                    _ => {}
                }
            }
        }
        // A web address pasted over a selection links it.
        if !self.sel.is_empty()
            && !self.text[self.sel.range()].contains('\n')
            && self.grid_widget(self.sel.head).is_none()
            && let Some(url) = cx.read_from_clipboard().and_then(|i| i.text())
            && !url.trim().contains(char::is_whitespace)
            && os::is_openable_link(url.trim())
        {
            let c = edit::insert_link(&self.text, self.sel, url.trim(), false);
            return self.apply(c, Kind::Other, cx);
        }
        let md = clipboard_html()
            .map(|h| mdedit_core::html_import::html_to_markdown(&h))
            .filter(|m| !m.trim().is_empty());
        match md {
            Some(md) => self.replace_selection(&md, Kind::Other, cx),
            None => self.paste_text(cx),
        }
    }

    fn paste_image(&mut self, image: &Image, cx: &mut Context<Self>) {
        let Some(dir) = self.base_dir.clone() else {
            cx.emit(EditorEvent::Notice(
                "Save the document first: pasted images are stored beside it.".into(),
            ));
            return;
        };
        let ext = match image.format {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Webp => "webp",
            ImageFormat::Gif => "gif",
            ImageFormat::Svg => "svg",
            ImageFormat::Bmp => "bmp",
            ImageFormat::Tiff => "tiff",
            ImageFormat::Ico => "ico",
            ImageFormat::Pnm => "pnm",
        };
        let stem = mdedit_core::attach::pasted_stem(std::time::SystemTime::now());
        match mdedit_core::attach::store(&dir, &stem, ext, &image.bytes) {
            Ok(file) => self.link_images(&dir, &[file], cx),
            Err(e) => cx.emit(EditorEvent::Notice(format!("Couldn't save the image: {e}"))),
        }
    }

    /// Link image files at the caret, copying in those from outside the
    /// document's folder.
    pub(crate) fn insert_images(&mut self, files: &[PathBuf], cx: &mut Context<Self>) {
        let Some(dir) = self.base_dir.clone() else {
            cx.emit(EditorEvent::Notice(
                "Save the document first: images are linked from where it is.".into(),
            ));
            return;
        };
        let mut brought = Vec::new();
        for f in files {
            match mdedit_core::attach::bring_in(&dir, f) {
                Ok(p) => brought.push(p),
                Err(e) => cx.emit(EditorEvent::Notice(format!(
                    "Couldn't copy in {}: {e}",
                    f.display()
                ))),
            }
        }
        self.link_images(&dir, &brought, cx);
    }

    fn link_images(&mut self, dir: &std::path::Path, files: &[PathBuf], cx: &mut Context<Self>) {
        if files.is_empty() || self.blocked_in_table(true) {
            return;
        }
        let links: Vec<String> = files
            .iter()
            .map(|f| format!("![]({})", mdedit_core::attach::link(dir, f)))
            .collect();
        self.replace_selection(&links.join("\n"), Kind::Other, cx);
    }

    /// Files dropped on the text: images go in where they land; anything
    /// else opens as usual.
    fn drop_files(&mut self, paths: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        let (images, others): (Vec<PathBuf>, Vec<PathBuf>) = paths
            .paths()
            .iter()
            .cloned()
            .partition(|p| mdedit_core::project::image_mime(p).is_some());
        if !images.is_empty() {
            if let Some((_, d, layout)) = self.hit(window.mouse_position()) {
                let pos = layout.to_source(d);
                self.set_selection(Selection::caret(pos), cx);
            }
            self.insert_images(&images, cx);
        }
        if !others.is_empty() {
            cx.emit(EditorEvent::Open(others));
        }
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

    /// Block commands make no sense in a table cell, and inline ones only
    /// within one cell.
    fn blocked_in_table(&self, inline: bool) -> bool {
        let Some(cell) = self.cell_range(self.sel.head) else {
            return self.grid_widget(self.sel.head).is_some();
        };
        !inline || !(cell.start <= self.sel.start() && self.sel.end() <= cell.end)
    }

    fn inline(&mut self, marker: &str, cx: &mut Context<Self>) {
        if self.blocked_in_table(true) {
            return;
        }
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
        if self.blocked_in_table(false) {
            return;
        }
        let c = edit::set_heading(&self.text, self.sel, a.level);
        self.apply(c, Kind::Other, cx);
    }
    fn paragraph(&mut self, _: &Paragraph, _: &mut Window, cx: &mut Context<Self>) {
        if self.blocked_in_table(false) {
            return;
        }
        let c = edit::set_heading(&self.text, self.sel, 0);
        self.apply(c, Kind::Other, cx);
    }

    fn block(&mut self, b: edit::Block, cx: &mut Context<Self>) {
        if self.blocked_in_table(false) {
            return;
        }
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
        if self.blocked_in_table(false) {
            return;
        }
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
        } else if self.table_key(table::TableKey::Leave, cx) {
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
            if p.y < origin.y + layout.height.get() {
                let local = point(p.x - origin.x, p.y - origin.y);
                return Some((*ix, layout.index(local), layout.clone()));
            }
        }
        Some((last.0, last.2.len(), last.2.clone()))
    }

    /// The link whose text is under `p`, if a Ctrl/Cmd+click there would
    /// follow it: a `[[wiki link]]`, or a web or mail address.
    fn link_under(&self, p: Point<Pixels>) -> Option<syntax::Link> {
        if !self.rich() {
            return None;
        }
        let (_, origin, layout) = self
            .geometry
            .lines
            .iter()
            .find(|(_, o, l)| p.y >= o.y && p.y < o.y + l.height.get())?;
        let width = self.content_w? - layout.indent;
        let src = layout.to_source(layout.index(p - *origin));
        // The nearest offset can be just past a link's last character.
        self.syntax
            .links
            .iter()
            .filter(|l| l.range.start <= src && src <= l.range.end)
            .filter(|l| l.wiki || os::is_openable_link(&l.url))
            .find(|l| {
                let (a, b) = (
                    layout.to_display(l.range.start),
                    layout.to_display(l.range.end),
                );
                a < b
                    && layout
                        .rects(*origin, a, b, false, width)
                        .iter()
                        .any(|r| r.contains(&p))
            })
            .cloned()
    }

    /// The pointer's at `p` with `modifiers` held: over a link with
    /// Ctrl/Cmd, it's a hand.
    pub(crate) fn hover_link(
        &mut self,
        p: Point<Pixels>,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        let held = modifiers.secondary(cx) && !modifiers.shift && !self.is_selecting();
        let link = held.then(|| self.link_under(p)).flatten();
        let on = link.is_some();
        if on != self.link_hover {
            self.link_hover = on;
            cx.notify();
        }
        // Held over a wiki link for a moment: a preview of its note.
        let note = link.filter(|l| l.wiki).map(|l| l.url);
        let shown = self.preview.as_ref().map(|(t, _)| t.clone());
        let waiting = self.preview_wait.as_ref().map(|(t, _)| t.clone());
        let Some(target) = note else {
            self.preview_wait = None;
            if self.preview.take().is_some() {
                cx.notify();
            }
            return;
        };
        if Some(&target) != shown.as_ref() && Some(&target) != waiting.as_ref() {
            self.preview = None;
            let wait = cx.spawn({
                let target = target.clone();
                async move |this, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(500))
                        .await;
                    this.update(cx, |e, cx| {
                        if e.preview_wait.as_ref().is_some_and(|(t, _)| *t == target) {
                            e.preview_wait = None;
                            e.preview = Some((target, p));
                            cx.notify();
                        }
                    })
                    .ok();
                }
            });
            self.preview_wait = Some((target, wait));
        }
    }

    pub(crate) fn link_hovered(&self) -> bool {
        self.link_hover
    }

    /// The task box at `p`, in a list line's margin.
    fn task_at(&self, p: Point<Pixels>) -> Option<Range<usize>> {
        let (_, origin, l) = self
            .geometry
            .lines
            .iter()
            .find(|(_, o, l)| p.y >= o.y && p.y < o.y + l.height.get())?;
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
        // A fold chevron, or a folded block's `…`.
        if let Some(&(_, head)) = self
            .fold_buttons
            .iter()
            .find(|(b, _)| b.contains(&e.position))
        {
            return self.toggle_fold(head, cx);
        }
        // A table's controls take the press before the text does.
        if self.table_press(e.position, cx) {
            return;
        }
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
        let mut pos = layout.to_source(d);
        // A press on a formula opens it.
        if self.rich()
            && e.click_count == 1
            && let Some(f) = self.formula_at(ix, &layout, e.position)
        {
            pos = f.start + self.math_delims(&f);
        }
        // A link opens with Ctrl/Cmd held.
        if e.click_count == 1
            && !e.modifiers.shift
            && e.modifiers.secondary(cx)
            && let Some(link) = self.link_under(e.position)
        {
            if link.wiki {
                let target = link.url.clone();
                return self.follow_wiki(&target, window, cx);
            }
            if os::is_openable_link(&link.url) {
                cx.open_url(&link.url);
                return;
            }
        }
        self.goal_x = None;
        let (grain, sel) = match e.click_count {
            1 if e.modifiers.shift => (Grain::Char, Selection::new(self.sel.anchor, pos)),
            1 => (Grain::Char, Selection::caret(pos)),
            2 => {
                let w = buffer::word_at(&self.text, pos);
                (Grain::Word, Selection::new(w.start, w.end))
            }
            _ => match self.cell_range(pos) {
                Some(cell) => (Grain::Line, Selection::new(cell.start, cell.end)),
                None => {
                    let l = &self.row_range(ix);
                    let end = (l.end + 1).min(self.text.len());
                    (Grain::Line, Selection::new(l.start, end))
                }
            },
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
        let pos = layout.to_source(d);
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

    /// Scroll by `dy`. Whether it moved (an embed at its end lets the
    /// document around it scroll instead).
    pub(crate) fn scroll(&mut self, dy: Pixels, cx: &mut Context<Self>) -> bool {
        let to = min(max(self.scroll_y - dy, px(0.)), self.max_scroll);
        let moved = to != self.scroll_y;
        if self.is_embedded() && !moved {
            return false;
        }
        self.scroll_y = if self.is_embedded() {
            to
        } else {
            max(self.scroll_y - dy, px(0.))
        };
        self.autoscroll = false;
        cx.notify();
        true
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
        let was_marked = self.marked.take().is_some();
        if !was_marked
            && r == self.sel.range()
            && let Some(c) = self.auto_pair(text)
        {
            self.apply(c, Kind::Typing, cx);
            self.update_slash(Some(text), cx);
            cx.notify();
            return;
        }
        let (edits, at) = self
            .table_safe_edits(r.clone(), text)
            .unwrap_or_else(|| (vec![Edit::replace(r.clone(), text)], r.start + text.len()));
        self.apply(
            Change {
                edits,
                selection: Selection::caret(at),
            },
            Kind::Typing,
            cx,
        );
        self.update_slash(Some(text), cx);
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
        let p = layout.point(layout.to_display(pos));
        Some(Bounds::new(origin + p, size(px(2.), layout.line_height)))
    }

    fn character_index_for_point(
        &mut self,
        p: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let (_, d, layout) = self.hit(p)?;
        Some(buffer::utf16_offset(&self.text, layout.to_source(d)))
    }
}

impl Render for MarkdownEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let overlay = self.render_overlays(window, cx);
        let preview = self.render_preview(cx);
        div()
            .key_context("MdEditor")
            .track_focus(&self.focus)
            // Ctrl/Cmd pressed or let go with the pointer still: over a
            // link, it's a hand or not.
            .on_modifiers_changed(cx.listener(|ed, e: &ModifiersChangedEvent, window, cx| {
                ed.hover_link(window.mouse_position(), e.modifiers, cx)
            }))
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
            .on_drop(
                cx.listener(|ed, paths: &ExternalPaths, window, cx| {
                    ed.drop_files(paths, window, cx)
                }),
            )
            .on_action(cx.listener(Self::toggle_source))
            .on_action(cx.listener(Self::fold_at_caret))
            .on_action(cx.listener(Self::unfold_at_caret))
            .on_action(cx.listener(Self::fold_all))
            .on_action(cx.listener(Self::unfold_all))
            .on_action(cx.listener(Self::set_mode_action))
            .on_action(cx.listener(Self::cancel))
            .size_full()
            .cursor(CursorStyle::IBeam)
            .child(EditorElement::new(cx.entity()))
            .children(overlay)
            .children(preview)
            .children(self.menu.as_ref().map(|(menu, at, _)| {
                deferred(
                    anchored()
                        .position(*at)
                        .snap_to_window_with_margin(px(8.))
                        .child(menu.clone()),
                )
                .with_priority(2)
            }))
    }
}

#[cfg(test)]
mod tests;
