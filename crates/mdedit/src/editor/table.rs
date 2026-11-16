//! Editing a table in its grid, as v1 did: type in the cells, and use the
//! keys, the right-click menu, and the controls that show while the table is
//! hovered (a design from ZenNotes). Those controls are:
//! * a reorder grip above each header and left of each body row,
//! * a resize handle on each header's right border,
//! * `+` rails along the right and bottom edges.
//!
//! Every command rewrites the table's source as a whole (padded, with its
//! `zen:cols` widths), as one undo step. Typing goes into the cell's source
//! as it is, with `|` escaped, and the table is padded again on leaving it.

use std::ops::Range;

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;
use mdedit_core::buffer::{Change, Edit, Selection, map_pos};
use mdedit_core::history::Kind;
use mdedit_core::syntax::WidgetKind;
use mdedit_core::table::{self, Align, MenuEntry, MenuFacts, TableModel};

use super::MarkdownEditor;
use super::grid::{Grid, MIN_COL};

/// How thick the reorder grips and the add rails are.
pub(crate) const GRIP: f32 = 12.;
pub(crate) const RAIL: f32 = 9.;
/// How far the pointer can stray from a table before its controls go away:
/// above, left, right, below. The controls sit in these margins.
const MARGIN: (f32, f32, f32, f32) = (14., 16., 9., 9.);

/// One of a hovered table's controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Control {
    /// Above header `i`: drag to move the column.
    ColGrip(usize),
    /// Left of body row `i`: drag to move the row.
    RowGrip(usize),
    /// On header `i`'s right border: drag to resize.
    Resize(usize),
    AddCol,
    AddRow,
}

/// Where each control is, relative to the grid row's box.
pub(crate) fn controls(g: &Grid) -> Vec<(Control, Bounds<Pixels>)> {
    let b = g.bounds();
    let (top, head_bottom) = (g.row_y[0], g.row_y[1]);
    let mut out = Vec::new();
    for i in 0..g.cols {
        let (l, r) = (g.col_x[i], g.col_x[i + 1]);
        out.push((
            Control::Resize(i),
            Bounds::from_corners(point(r - px(3.), top), point(r + px(4.), head_bottom)),
        ));
        out.push((
            Control::ColGrip(i),
            Bounds::from_corners(point(l, top - px(GRIP)), point(r, top)),
        ));
    }
    for i in 0..g.row_y.len().saturating_sub(2) {
        out.push((
            Control::RowGrip(i),
            Bounds::from_corners(
                point(b.left() - px(GRIP), g.row_y[i + 1]),
                point(b.left(), g.row_y[i + 2]),
            ),
        ));
    }
    out.push((
        Control::AddCol,
        Bounds::from_corners(
            point(b.right(), b.top()),
            point(b.right() + px(RAIL), b.bottom()),
        ),
    ));
    out.push((
        Control::AddRow,
        Bounds::from_corners(
            point(b.left(), b.bottom()),
            point(b.right(), b.bottom() + px(RAIL)),
        ),
    ));
    out
}

/// What the pointer is over in a table: which (grid) row of the editor,
/// the header and body row it's on, and the control under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Hover {
    pub row: usize,
    pub head: Option<usize>,
    pub body: Option<usize>,
    pub control: Option<Control>,
}

/// A press on a control, carried until the button comes up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Drag {
    /// Body rows (or columns) `from` and the slot it would land in.
    Row {
        row: usize,
        from: usize,
        to: usize,
    },
    Col {
        row: usize,
        from: usize,
        to: usize,
    },
    Resize {
        row: usize,
        col: usize,
        start_x: Pixels,
        start_w: f32,
        moved: bool,
    },
    /// A rail acts on click: pressing one and moving off cancels.
    Rail {
        row: usize,
        control: Control,
    },
}

impl Drag {
    pub fn row(&self) -> usize {
        match *self {
            Drag::Row { row, .. }
            | Drag::Col { row, .. }
            | Drag::Resize { row, .. }
            | Drag::Rail { row, .. } => row,
        }
    }
}

/// Column widths while one is dragged, before they're written down.
pub(crate) struct ResizeDrag {
    pub widget: usize,
    pub widths: Vec<f32>,
}

/// A table command.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TableOp {
    AddRow,
    AddCol,
    /// Model rows (header is 0) or columns, from and to; the caret follows.
    MoveRow(usize, usize),
    MoveCol(usize, usize),
    /// The same, dragged: the caret stays put.
    DragRow(usize, usize),
    DragCol(usize, usize),
    DuplicateRow,
    DuplicateCol,
    DuplicateTable,
    DeleteRow,
    DeleteCol,
    DeleteTable,
    Align(Align),
    Sort(bool),
    Copy,
    Widths(Vec<Option<u32>>),
}

/// Tab, Shift+Tab, Enter and Escape in a table.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum TableKey {
    Next,
    Prev,
    Down,
    Leave,
}

/// The v1 menu's command names, as `mdedit_core::table::table_menu` gives
/// them, for cell (r, c).
fn op_for(action: &str, r: usize, c: usize) -> Option<TableOp> {
    Some(match action {
        "tbl_add_row" => TableOp::AddRow,
        "tbl_add_col" => TableOp::AddCol,
        "tbl_move_row_up" => TableOp::MoveRow(r, r.saturating_sub(1)),
        "tbl_move_row_down" => TableOp::MoveRow(r, r + 1),
        "tbl_move_col_left" => TableOp::MoveCol(c, c.saturating_sub(1)),
        "tbl_move_col_right" => TableOp::MoveCol(c, c + 1),
        "tbl_dup_row" => TableOp::DuplicateRow,
        "tbl_dup_col" => TableOp::DuplicateCol,
        "tbl_align_none" => TableOp::Align(Align::None),
        "tbl_align_left" => TableOp::Align(Align::Left),
        "tbl_align_center" => TableOp::Align(Align::Center),
        "tbl_align_right" => TableOp::Align(Align::Right),
        "tbl_sort_asc" => TableOp::Sort(false),
        "tbl_sort_desc" => TableOp::Sort(true),
        "tbl_copy_md" => TableOp::Copy,
        "tbl_dup_table" => TableOp::DuplicateTable,
        "tbl_del_row" => TableOp::DeleteRow,
        "tbl_del_col" => TableOp::DeleteCol,
        "tbl_del_table" => TableOp::DeleteTable,
        _ => return None,
    })
}

impl MarkdownEditor {
    // ---------- the model ----------

    /// The table widget holding `pos`, in the rendered modes, where tables
    /// are grids.
    pub(crate) fn grid_widget(&self, pos: usize) -> Option<usize> {
        if !self.rich() {
            return None;
        }
        self.widget_at(pos)
            .filter(|w| self.syntax.widgets[*w].kind == WidgetKind::Table)
    }

    pub(crate) fn table_model(&self, w: usize) -> Option<TableModel> {
        let lines: Vec<Range<usize>> = self.syntax.widgets[w]
            .lines
            .clone()
            .map(|l| self.syntax.lines[l].range.clone())
            .collect();
        TableModel::parse(&self.text, &lines)
    }

    /// The source of the cell holding `pos`, when it's in a grid.
    pub(crate) fn cell_range(&self, pos: usize) -> Option<Range<usize>> {
        let t = self.table_model(self.grid_widget(pos)?)?;
        t.cells
            .iter()
            .flatten()
            .find(|c| c.start <= pos && pos <= c.end)
            .cloned()
    }

    /// A position in a grid's source but not in a cell (a pipe, the
    /// delimiter row) moves into the next cell, or the last one.
    pub(crate) fn snap_to_cell(&self, pos: usize) -> usize {
        let Some(t) = self.grid_widget(pos).and_then(|w| self.table_model(w)) else {
            return pos;
        };
        let cells: Vec<&Range<usize>> = t.cells.iter().flatten().collect();
        if cells.iter().any(|c| c.start <= pos && pos <= c.end) {
            return pos;
        }
        cells
            .iter()
            .find(|c| c.start > pos)
            .map(|c| c.start)
            .or_else(|| cells.last().map(|c| c.end))
            .unwrap_or(pos)
    }

    /// The edits that replace `range` with `text` without breaking a
    /// table: in a cell, the text is escaped; across cells, each cell
    /// keeps its place and only loses the text that was selected. `None`
    /// when no table is involved. Also returns where the caret goes.
    pub(crate) fn table_safe_edits(
        &self,
        range: Range<usize>,
        text: &str,
    ) -> Option<(Vec<Edit>, usize)> {
        if !self.rich() {
            return None;
        }
        let tables: Vec<TableModel> = self
            .syntax
            .widgets
            .iter()
            .enumerate()
            .filter(|(_, w)| w.kind == WidgetKind::Table)
            .filter_map(|(i, _)| self.table_model(i))
            .filter(|t| t.range.start <= range.end && range.start <= t.range.end)
            .collect();
        if tables.is_empty() {
            return None;
        }
        let in_cell = |p: usize| {
            tables
                .iter()
                .flat_map(|t| t.cells.iter().flatten())
                .any(|c| c.start <= p && p <= c.end)
        };
        let insert = if in_cell(range.start) {
            table::escape_cell(text)
        } else {
            text.to_string()
        };
        // All in one cell: an ordinary edit, escaped.
        let one_cell = tables
            .iter()
            .flat_map(|t| t.cells.iter().flatten())
            .any(|c| c.start <= range.start && range.end <= c.end);
        if one_cell {
            let at = range.start + insert.len();
            return Some((vec![Edit::replace(range, insert)], at));
        }
        // Across cells: clear each cell's share, and outside the tables
        // delete as usual but keep the line breaks around them.
        let mut edits = Vec::new();
        let mut at = range.start;
        for t in &tables {
            if at < t.range.start {
                let end = t.range.start.saturating_sub(1).max(at);
                edits.push(Edit::delete(at..end));
            }
            for c in t.cells.iter().flatten() {
                let (s, e) = (c.start.max(range.start), c.end.min(range.end));
                if s < e {
                    edits.push(Edit::delete(s..e));
                }
            }
            at = (t.range.end + 1).min(self.text.len()).max(at);
        }
        if at < range.end {
            edits.push(Edit::delete(at..range.end));
        }
        edits.retain(|e| !e.range.is_empty());
        let start = edits.first().map_or(range.start, |e| e.range.start);
        let start = start.max(range.start);
        if !insert.is_empty() {
            match edits.iter_mut().find(|e| e.range.start == start) {
                Some(e) => e.text = insert.clone(),
                None => edits.push(Edit::insert(start, insert.clone())),
            }
        }
        let caret = map_pos(start, &edits, false) + insert.len();
        Some((edits, caret))
    }

    /// Write `t` back as padded source, as one undo step, with the caret
    /// at the end of cell `focus` (or kept where it was).
    fn apply_table(
        &mut self,
        t: &TableModel,
        focus: Option<(usize, usize)>,
        cx: &mut Context<Self>,
    ) {
        let rendered = t.render();
        let edits = vec![Edit::replace(t.range.clone(), rendered)];
        let caret = match focus {
            Some((r, c)) => {
                let r = r.min(t.rows.len().saturating_sub(1));
                let c = c.min(t.cols().saturating_sub(1));
                t.range.start + t.rendered_cell_range(r, c).map_or(0, |x| x.end)
            }
            None => map_pos(self.sel.head, &edits, false),
        };
        self.apply(
            Change {
                edits,
                selection: Selection::caret(caret),
            },
            Kind::Other,
            cx,
        );
    }

    /// Apply `op` to the table starting at line `line`, from cell (r, c),
    /// header row 0.
    pub(crate) fn table_op(
        &mut self,
        line: usize,
        op: TableOp,
        r: usize,
        c: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(w) = self
            .syntax
            .widgets
            .iter()
            .position(|w| w.lines.start == line)
        else {
            return;
        };
        let Some(mut t) = self.table_model(w) else {
            return;
        };
        let rows = t.rows.len();
        let focus = match op {
            TableOp::AddRow => {
                t.insert_row(rows);
                Some((rows, 0))
            }
            TableOp::AddCol => {
                let at = t.cols();
                t.insert_col(at);
                Some((0, at))
            }
            TableOp::MoveRow(from, to) => {
                t.move_row(from, to);
                Some((to, c))
            }
            TableOp::MoveCol(from, to) => {
                t.move_col(from, to);
                Some((r, to))
            }
            TableOp::DragRow(from, to) => {
                t.move_row(from, to);
                None
            }
            TableOp::DragCol(from, to) => {
                t.move_col(from, to);
                None
            }
            TableOp::DuplicateRow => {
                t.duplicate_row(r);
                Some((r + 1, c))
            }
            TableOp::DuplicateCol => {
                t.duplicate_col(c);
                Some((r, c + 1))
            }
            TableOp::DeleteRow => {
                t.delete_row(r);
                Some((r, c))
            }
            TableOp::DeleteCol => {
                t.delete_col(c);
                Some((r, c))
            }
            TableOp::Align(a) => {
                t.set_align(c, a);
                Some((r, c))
            }
            TableOp::Sort(desc) => {
                t.sort_by(c, desc);
                Some((r, c))
            }
            TableOp::Widths(widths) => {
                t.set_widths(widths);
                None
            }
            TableOp::Copy => {
                cx.write_to_clipboard(ClipboardItem::new_string(t.render()));
                return;
            }
            TableOp::DuplicateTable => {
                let copy = format!("\n\n{}", t.render());
                let at = t.range.end;
                self.apply(
                    Change {
                        edits: vec![Edit::insert(at, copy)],
                        selection: self.sel,
                    },
                    Kind::Other,
                    cx,
                );
                return;
            }
            TableOp::DeleteTable => {
                let end = (t.range.end + 1).min(self.text.len());
                let at = t.range.start;
                self.apply(
                    Change {
                        edits: vec![Edit::delete(t.range.start..end)],
                        selection: Selection::caret(at),
                    },
                    Kind::Other,
                    cx,
                );
                return;
            }
        };
        self.apply_table(&t, focus, cx);
    }

    /// Tab, Shift+Tab, Enter and Escape in a table. False when the caret
    /// isn't in one.
    pub(crate) fn table_key(&mut self, key: TableKey, cx: &mut Context<Self>) -> bool {
        let Some(w) = self.grid_widget(self.sel.head) else {
            return false;
        };
        let Some(mut t) = self.table_model(w) else {
            return false;
        };
        let Some((r, c)) = t.cell_at(self.sel.head) else {
            return false;
        };
        let cols = t.cols();
        let rows = t.rows.len();
        let target = match key {
            TableKey::Next if c + 1 < cols => Some((r, c + 1)),
            TableKey::Next if r + 1 < rows => Some((r + 1, 0)),
            TableKey::Next => {
                t.insert_row(rows);
                Some((rows, 0))
            }
            TableKey::Prev if c > 0 => Some((r, c - 1)),
            TableKey::Prev if r > 0 => Some((r - 1, cols - 1)),
            TableKey::Prev => Some((0, 0)),
            TableKey::Down if r + 1 < rows => Some((r + 1, c)),
            TableKey::Down => {
                t.insert_row(rows);
                Some((rows, c))
            }
            TableKey::Leave => None,
        };
        match target {
            Some(cell) => self.apply_table(&t, Some(cell), cx),
            None => {
                // Out of the table, onto the line after it (made if needed).
                let rendered = t.render();
                let mut edits = vec![Edit::replace(t.range.clone(), rendered.clone())];
                if !self.text[t.range.end..].starts_with('\n') {
                    edits.push(Edit::insert(t.range.end, "\n"));
                }
                let at = t.range.start + rendered.len() + 1;
                self.apply(
                    Change {
                        edits,
                        selection: Selection::caret(at),
                    },
                    Kind::Other,
                    cx,
                );
            }
        }
        true
    }

    /// The caret left table `w`: pad its source again, if it needs it.
    pub(crate) fn tidy_table(&mut self, w: usize, cx: &mut Context<Self>) {
        let Some(t) = self.table_model(w) else { return };
        let rendered = t.render();
        if self.text[t.range.clone()] == rendered {
            return;
        }
        let edits = vec![Edit::replace(t.range.clone(), rendered)];
        let sel = Selection::new(
            map_pos(self.sel.anchor, &edits, false),
            map_pos(self.sel.head, &edits, false),
        );
        self.apply(
            Change {
                edits,
                selection: sel,
            },
            Kind::Other,
            cx,
        );
    }

    // ---------- the pointer ----------

    /// The grid under `p` (a window point), with its controls' margins:
    /// the editor row, and `p` relative to that row's box.
    fn grid_under(&self, p: Point<Pixels>) -> Option<(usize, Point<Pixels>)> {
        let (above, left, right, below) = MARGIN;
        self.geometry.lines.iter().find_map(|(ix, origin, l)| {
            let g = l.grid.as_ref()?;
            let local = point(p.x - origin.x, p.y - origin.y);
            let b = g.bounds();
            let inside = local.x >= b.left() - px(left)
                && local.x <= b.right() + px(right)
                && local.y >= b.top() - px(above)
                && local.y <= b.bottom() + px(below);
            inside.then_some((*ix, local))
        })
    }

    fn grid_of(&self, row: usize) -> Option<(Point<Pixels>, &Grid)> {
        self.geometry
            .lines
            .iter()
            .find(|(ix, ..)| *ix == row)
            .and_then(|(_, o, l)| Some((*o, l.grid.as_ref()?)))
    }

    fn hover_at(&self, p: Point<Pixels>) -> Option<Hover> {
        let (row, local) = self.grid_under(p)?;
        let (_, g) = self.grid_of(row)?;
        let control = controls(g)
            .into_iter()
            .find(|(_, b)| b.contains(&local))
            .map(|(c, _)| c);
        let b = g.bounds();
        let in_table = b.contains(&local);
        let (r, c) = g.slot_at(local);
        let head = match control {
            Some(Control::ColGrip(i) | Control::Resize(i)) => Some(i),
            None if in_table && r == 0 => Some(c),
            _ => None,
        };
        let body = match control {
            Some(Control::RowGrip(i)) => Some(i),
            None if in_table && r > 0 => Some(r - 1),
            _ => None,
        };
        Some(Hover {
            row,
            head,
            body,
            control,
        })
    }

    /// The pointer moved, with or without a button down.
    pub(crate) fn table_pointer(&mut self, p: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(drag) = self.table_drag {
            self.table_drag_to(drag, p, cx);
            return;
        }
        let hover = self.hover_at(p);
        if hover != self.table_hover {
            self.table_hover = hover;
            cx.notify();
        }
    }

    fn table_drag_to(&mut self, drag: Drag, p: Point<Pixels>, cx: &mut Context<Self>) {
        let Some((origin, g)) = self.grid_of(drag.row()) else {
            return;
        };
        let local = point(p.x - origin.x, p.y - origin.y);
        let next = match drag {
            Drag::Row { row, from, .. } => {
                let bottoms = &g.row_y[2..];
                let to = bottoms
                    .iter()
                    .position(|y| local.y < *y)
                    .unwrap_or(bottoms.len().saturating_sub(1));
                Drag::Row { row, from, to }
            }
            Drag::Col { row, from, .. } => {
                let rights = &g.col_x[1..];
                let to = rights
                    .iter()
                    .position(|x| local.x < *x)
                    .unwrap_or(rights.len().saturating_sub(1));
                Drag::Col { row, from, to }
            }
            Drag::Resize {
                row,
                col,
                start_x,
                start_w,
                ..
            } => {
                let width = (start_w + f32::from(p.x - start_x)).max(MIN_COL).round();
                if let Some(r) = &mut self.table_resize
                    && let Some(w) = r.widths.get_mut(col)
                {
                    *w = width;
                }
                if let Some(slot) = self.slots.get_mut(row) {
                    slot.layout = None;
                }
                Drag::Resize {
                    row,
                    col,
                    start_x,
                    start_w,
                    moved: true,
                }
            }
            Drag::Rail { .. } => {
                self.table_hover = self.hover_at(p);
                drag
            }
        };
        self.table_drag = Some(next);
        cx.notify();
    }

    /// A left press: on one of a table's controls, it starts a drag (or a
    /// rail's click) and the text is left alone.
    pub(crate) fn table_press(&mut self, p: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let Some(hover) = self.hover_at(p) else {
            return false;
        };
        let Some(control) = hover.control else {
            return false;
        };
        let row = hover.row;
        let drag = match control {
            Control::ColGrip(i) => Drag::Col {
                row,
                from: i,
                to: i,
            },
            Control::RowGrip(i) => Drag::Row {
                row,
                from: i,
                to: i,
            },
            Control::Resize(i) => {
                let Some((_, g)) = self.grid_of(row) else {
                    return false;
                };
                // Every column is pinned at the width it has, so moving one
                // border doesn't let the others reflow. Rounded up: a hair
                // narrower than its text and a column would wrap it.
                let widths: Vec<f32> = g
                    .col_x
                    .windows(2)
                    .map(|w| f32::from(w[1] - w[0]).ceil())
                    .collect();
                let start_w = widths[i];
                self.table_resize = Some(super::table::ResizeDrag {
                    widget: g.widget,
                    widths,
                });
                Drag::Resize {
                    row,
                    col: i,
                    start_x: p.x,
                    start_w,
                    moved: false,
                }
            }
            Control::AddCol | Control::AddRow => Drag::Rail { row, control },
        };
        self.table_drag = Some(drag);
        self.table_hover = Some(hover);
        cx.notify();
        true
    }

    /// The button came up: finish what the press started.
    pub(crate) fn table_release(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.table_drag.take() else {
            return;
        };
        let resize = self.table_resize.take();
        let Some((_, g)) = self.grid_of(drag.row()) else {
            cx.notify();
            return;
        };
        let line = self.syntax.widgets[g.widget].lines.start;
        let op = match drag {
            Drag::Row { from, to, .. } if from != to => Some(TableOp::DragRow(from + 1, to + 1)),
            Drag::Col { from, to, .. } if from != to => Some(TableOp::DragCol(from, to)),
            Drag::Resize { moved: true, .. } => resize
                .map(|r| TableOp::Widths(r.widths.iter().map(|w| Some(w.ceil() as u32)).collect())),
            Drag::Rail { control, .. }
                if self.table_hover.and_then(|h| h.control) == Some(control) =>
            {
                Some(if control == Control::AddRow {
                    TableOp::AddRow
                } else {
                    TableOp::AddCol
                })
            }
            _ => None,
        };
        // A resize that went nowhere drew pinned widths; draw it as it is.
        if let Some(slot) = self.slots.get_mut(drag.row()) {
            slot.layout = None;
        }
        if let Some(op) = op {
            self.table_op(line, op, 0, 0, cx);
        }
        cx.notify();
    }

    pub(crate) fn table_dragging(&self) -> Option<Drag> {
        self.table_drag
    }

    /// A right press on a cell: the table menu, v1's, with what doesn't
    /// apply to this cell greyed out.
    pub(crate) fn table_menu_at(
        &mut self,
        p: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((row, local)) = self.grid_under(p) else {
            return false;
        };
        let Some((_, g)) = self.grid_of(row) else {
            return false;
        };
        if !g.bounds().contains(&local) {
            return false;
        }
        let (r, c) = g.slot_at(local);
        let t = &g.model;
        let facts = MenuFacts {
            body_row: r.checked_sub(1),
            body_rows: t.rows.len().saturating_sub(1),
            col: c,
            cols: t.cols(),
            align: t.aligns.get(c).copied().unwrap_or(Align::None),
        };
        let line = self.syntax.widgets[g.widget].lines.start;
        // The caret goes into the cell, so the commands act where it shows.
        if let Some(k) = g.cell_in(r, c) {
            let pos = g.cells[k].source().end;
            self.set_selection(Selection::caret(pos), cx);
        }
        let editor = cx.entity().downgrade();
        let menu = PopupMenu::build(window, cx, move |mut menu, _, _| {
            for entry in table::table_menu(&facts) {
                match entry {
                    MenuEntry::Sep => menu = menu.separator(),
                    MenuEntry::Item(item) => {
                        let Some(op) = op_for(item.action, r, c) else {
                            continue;
                        };
                        let editor = editor.clone();
                        menu = menu.item(
                            PopupMenuItem::new(item.label)
                                .disabled(!item.enabled)
                                .checked(item.ticked)
                                .on_click(move |_, _, cx| {
                                    editor
                                        .update(cx, |e, cx| e.table_op(line, op.clone(), r, c, cx))
                                        .ok();
                                }),
                        );
                    }
                }
            }
            menu
        });
        menu.focus_handle(cx).focus(window, cx);
        let sub = cx.subscribe_in(&menu, window, |this, _, _: &DismissEvent, window, cx| {
            this.table_menu = None;
            this.focus.focus(window, cx);
            cx.notify();
        });
        self.table_menu = Some((menu, p, sub));
        cx.notify();
        true
    }
}
