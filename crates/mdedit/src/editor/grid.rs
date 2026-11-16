//! A table drawn and edited as a grid. Each cell is laid out on its own,
//! wrapped to its column, and the caret moves through the cells as if they
//! were one line: cell after cell, row after row. A display offset in the
//! grid is a cell's start (`base`) plus an offset in that cell's text; one
//! extra between cells keeps every offset in exactly one cell.

use std::ops::Range;

use gpui_kit::*;
use mdedit_core::display::{self, DisplayLine, Reveal, Role};
use mdedit_core::prefs::Mode;
use mdedit_core::syntax::LineKind;
use mdedit_core::table::{Align, TableModel};

use super::MarkdownEditor;

/// Room above a table for its column grips, and below it for the add-row
/// rail.
pub(crate) const TOP: f32 = 14.;
pub(crate) const BOTTOM: f32 = 12.;
const PAD_X: f32 = 10.;
const PAD_Y: f32 = 5.;
/// The narrowest a column can be dragged.
pub(crate) const MIN_COL: f32 = 48.;

/// One line of a cell's text: a `<br>` starts another.
pub(crate) struct CellLine {
    /// Its first display offset in the cell.
    pub start: usize,
    /// Where it's drawn, from the cell's text origin (aligned).
    pub at: Point<Pixels>,
    pub shaped: WrappedLine,
}

pub(crate) struct GridCell {
    pub r: usize,
    pub c: usize,
    pub display: DisplayLine,
    pub lines: Vec<CellLine>,
    /// The text's height.
    pub height: Pixels,
    /// The cell's first display offset in the grid.
    pub base: usize,
    /// Where the cell's text starts, relative to the row's box.
    pub origin: Point<Pixels>,
    pub wrap: Pixels,
}

impl GridCell {
    fn len(&self) -> usize {
        self.display.text.len()
    }

    /// The line holding cell offset `d`.
    fn line_of(&self, d: usize) -> usize {
        self.lines
            .partition_point(|l| l.start <= d)
            .saturating_sub(1)
    }

    fn line_end(&self, i: usize) -> usize {
        self.lines
            .get(i + 1)
            .map_or(self.len(), |next| next.start - 1)
    }

    /// Where cell offset `d` is drawn, from the cell's origin.
    fn pos(&self, d: usize, lh: Pixels) -> Point<Pixels> {
        let Some(line) = self.lines.get(self.line_of(d)) else {
            return point(px(0.), px(0.));
        };
        let p = line
            .shaped
            .position_for_index(d - line.start, lh)
            .unwrap_or_else(|| point(line.shaped.width(), px(0.)));
        line.at + p
    }

    /// The cell offset nearest `p` (from the cell's origin).
    fn index(&self, p: Point<Pixels>, lh: Pixels) -> usize {
        let i = self.lines.iter().rposition(|l| l.at.y <= p.y).unwrap_or(0);
        let Some(line) = self.lines.get(i) else {
            return 0;
        };
        let local = point(p.x - line.at.x, (p.y - line.at.y).max(px(0.)));
        let at = match line.shaped.closest_index_for_position(local, lh) {
            Ok(i) | Err(i) => i,
        };
        (line.start + at).min(self.line_end(i))
    }

    /// Rectangles covering cell offsets `a..b`, drawn at `origin`.
    fn rects(&self, origin: Point<Pixels>, a: usize, b: usize, lh: Pixels) -> Vec<Bounds<Pixels>> {
        let mut out = Vec::new();
        for (i, line) in self.lines.iter().enumerate() {
            let (s, e) = (a.max(line.start), b.min(self.line_end(i)));
            if s >= e {
                continue;
            }
            out.extend(wrapped_rects(
                origin + line.at,
                &line.shaped,
                lh,
                s - line.start,
                e - line.start,
                false,
                self.wrap,
            ));
        }
        out
    }

    fn end(&self) -> usize {
        self.base + self.len()
    }

    /// The source the cell's text covers.
    pub fn source(&self) -> Range<usize> {
        self.display.source.clone()
    }
}

pub(crate) struct Grid {
    /// Which of `Syntax::widgets` this is.
    pub widget: usize,
    pub model: TableModel,
    /// Row-major, in source order. A short row has fewer cells.
    pub cells: Vec<GridCell>,
    pub cols: usize,
    /// Column edges (one more than columns), relative to the row's box.
    pub col_x: Vec<Pixels>,
    /// Table row edges (one more than rows), relative to the row's box.
    pub row_y: Vec<Pixels>,
    pub line_height: Pixels,
}

impl Grid {
    pub fn len(&self) -> usize {
        self.cells.last().map_or(0, GridCell::end)
    }

    pub fn width(&self) -> Pixels {
        self.col_x.last().copied().unwrap_or_default()
    }

    /// The table's own box (without the room for its controls).
    pub fn bounds(&self) -> Bounds<Pixels> {
        let top = self.row_y.first().copied().unwrap_or_default();
        let bottom = self.row_y.last().copied().unwrap_or_default();
        Bounds::new(point(px(0.), top), size(self.width(), bottom - top))
    }

    /// The box of cell (r, c), relative to the row's box.
    pub fn cell_bounds(&self, r: usize, c: usize) -> Bounds<Pixels> {
        let c = c.min(self.cols.saturating_sub(1));
        Bounds::from_corners(
            point(self.col_x[c], self.row_y[r]),
            point(self.col_x[c + 1], self.row_y[r + 1]),
        )
    }

    /// The cell holding display offset `d`.
    pub fn cell_of(&self, d: usize) -> usize {
        self.cells
            .partition_point(|c| c.base <= d)
            .saturating_sub(1)
    }

    /// The cell whose text holds source offset `src`, if any does.
    pub fn cell_at_source(&self, src: usize) -> Option<usize> {
        let k = self
            .cells
            .partition_point(|c| c.display.source.start <= src)
            .checked_sub(1)?;
        (src <= self.cells[k].display.source.end).then_some(k)
    }

    pub fn to_display(&self, src: usize) -> usize {
        let k = self
            .cells
            .partition_point(|c| c.display.source.start <= src);
        if k == 0 {
            return 0;
        }
        let cell = &self.cells[k - 1];
        if src <= cell.display.source.end {
            return cell.base + cell.display.to_display(src);
        }
        // Between cells (a pipe, the delimiter row): the next cell's start.
        self.cells.get(k).map_or(self.len(), |c| c.base)
    }

    pub fn to_source(&self, d: usize) -> usize {
        let Some(cell) = self.cells.get(self.cell_of(d)) else {
            return self.model.range.start;
        };
        cell.display.to_source((d - cell.base).min(cell.len()))
    }

    pub fn prev(&self, d: usize) -> usize {
        let k = self.cell_of(d);
        let cell = &self.cells[k];
        if d > cell.base {
            cell.base + cell.display.prev(d - cell.base)
        } else if k > 0 {
            self.cells[k - 1].end()
        } else {
            0
        }
    }

    pub fn next(&self, d: usize) -> usize {
        let k = self.cell_of(d);
        let cell = &self.cells[k];
        if d < cell.end() {
            cell.base + cell.display.next(d - cell.base)
        } else {
            self.cells.get(k + 1).map_or(d, |c| c.base)
        }
    }

    pub fn role_at(&self, d: usize) -> Role {
        let cell = &self.cells[self.cell_of(d)];
        cell.display.role_at(d - cell.base)
    }

    pub fn source_at(&self, d: usize) -> Option<Range<usize>> {
        let cell = &self.cells[self.cell_of(d)];
        cell.display.source_at(d - cell.base)
    }

    /// Where display offset `d` is drawn, relative to the row's box.
    pub fn point(&self, d: usize) -> Point<Pixels> {
        let Some(cell) = self.cells.get(self.cell_of(d)) else {
            return point(px(0.), px(TOP));
        };
        let local = (d - cell.base).min(cell.len());
        cell.origin + cell.pos(local, self.line_height)
    }

    /// The table row and column under `p` (relative to the row's box),
    /// nearest when outside.
    pub fn slot_at(&self, p: Point<Pixels>) -> (usize, usize) {
        let rows = self.row_y.len() - 1;
        let r = self.row_y[1..]
            .iter()
            .position(|&y| p.y < y)
            .unwrap_or(rows - 1);
        let c = self.col_x[1..]
            .iter()
            .position(|&x| p.x < x)
            .unwrap_or(self.cols - 1);
        (r, c)
    }

    /// The cell at table row `r`, column `c`, or the last one that row has.
    pub fn cell_in(&self, r: usize, c: usize) -> Option<usize> {
        let first = self.cells.iter().position(|cell| cell.r == r)?;
        let last = self.cells[first..]
            .iter()
            .take_while(|cell| cell.r == r)
            .count()
            + first
            - 1;
        Some((first + c).min(last))
    }

    /// The display offset nearest `p`, relative to the row's box.
    pub fn index(&self, p: Point<Pixels>) -> usize {
        let (r, c) = self.slot_at(p);
        let Some(k) = self.cell_in(r, c) else {
            return 0;
        };
        self.index_in(k, p)
    }

    /// The display offset in cell `k` nearest `p`.
    pub fn index_in(&self, k: usize, p: Point<Pixels>) -> usize {
        let cell = &self.cells[k];
        let local = point(p.x - cell.origin.x, p.y - cell.origin.y);
        cell.base + cell.index(local, self.line_height)
    }

    /// A line up or down from `p` (the caret's top): within its cell if
    /// it wraps, else the cell above or below in the same column. `None`
    /// off the top or bottom of the table.
    pub fn step_y(&self, p: Point<Pixels>, step: i32) -> Option<Pixels> {
        let lh = self.line_height;
        let (r, c) = self.slot_at(p);
        if let Some(k) = self.cell_in(r, c) {
            let cell = &self.cells[k];
            let y = p.y + lh * step as f32;
            if y >= cell.origin.y && y < cell.origin.y + cell.height {
                return Some(y);
            }
        }
        let next = r as i64 + step as i64;
        if next < 0 || next as usize + 1 >= self.row_y.len() {
            return None;
        }
        let cell = &self.cells[self.cell_in(next as usize, c)?];
        let h = cell.height;
        Some(if step > 0 || h <= lh {
            cell.origin.y
        } else {
            cell.origin.y + h - lh
        })
    }

    /// The top of the last row's text, for coming in from below.
    pub fn last_line_y(&self) -> Pixels {
        let last = self.row_y.len().saturating_sub(2);
        self.cells
            .iter()
            .rev()
            .find(|c| c.r == last)
            .map_or(px(TOP), |c| c.origin.y)
    }

    /// The start or end of the visual line `d` is on, within its cell.
    pub fn line_edge(&self, d: usize, end: bool) -> usize {
        let k = self.cell_of(d);
        let cell = &self.cells[k];
        let y = self.point(d).y;
        let x = if end { px(1e6) } else { px(-1e6) };
        self.index_in(k, point(cell.origin.x + x, y))
    }

    /// Rectangles covering display range `a..b`, cell by cell.
    pub fn rects(&self, origin: Point<Pixels>, a: usize, b: usize) -> Vec<Bounds<Pixels>> {
        let mut out = Vec::new();
        for cell in &self.cells {
            let (s, e) = (a.max(cell.base), b.min(cell.end()));
            if s >= e {
                continue;
            }
            out.extend(cell.rects(
                origin + cell.origin,
                s - cell.base,
                e - cell.base,
                self.line_height,
            ));
        }
        out
    }
}

/// Rectangles covering `a..b` of a (possibly wrapped) line drawn at
/// `origin`, `width` wide.
pub(crate) fn wrapped_rects(
    origin: Point<Pixels>,
    shaped: &WrappedLine,
    lh: Pixels,
    a: usize,
    b: usize,
    to_eol: bool,
    width: Pixels,
) -> Vec<Bounds<Pixels>> {
    let at = |i: usize| {
        shaped
            .position_for_index(i, lh)
            .unwrap_or_else(|| point(shaped.width(), px(0.)))
    };
    let (pa, pb) = (at(a), at(b));
    let eol = if to_eol { px(6.) } else { px(0.) };
    let mut out = Vec::new();
    if pa.y == pb.y {
        out.push(Bounds::from_corners(
            origin + pa,
            origin + point(pb.x + eol, pb.y + lh),
        ));
        return out;
    }
    out.push(Bounds::from_corners(
        origin + pa,
        origin + point(width, pa.y + lh),
    ));
    let mut y = pa.y + lh;
    while y < pb.y {
        out.push(Bounds::from_corners(
            origin + point(px(0.), y),
            origin + point(width, y + lh),
        ));
        y += lh;
    }
    out.push(Bounds::from_corners(
        origin + point(px(0.), pb.y),
        origin + point(pb.x + eol, pb.y + lh),
    ));
    out
}

impl MarkdownEditor {
    /// The column widths table `model` is drawn at in `avail`: saved widths
    /// where there are some (or the ones being dragged), otherwise as wide
    /// as their text (`natural`), and scaled down to fit.
    fn grid_widths(
        &self,
        w: usize,
        model: &TableModel,
        avail: Pixels,
        natural: &[f32],
    ) -> Vec<Pixels> {
        let cols = model.cols().max(1);
        if let Some(drag) = self.table_resize.as_ref().filter(|r| r.widget == w) {
            return drag.widths.iter().map(|w| px(*w)).collect();
        }
        let mut widths: Vec<f32> = (0..cols)
            .map(|col| {
                let natural = natural.get(col).copied().unwrap_or(MIN_COL);
                model
                    .widths
                    .get(col)
                    .copied()
                    .flatten()
                    .map_or(natural, |v| v as f32)
            })
            .collect();
        let total: f32 = widths.iter().sum();
        let avail = f32::from(avail);
        if total > avail {
            for w in &mut widths {
                *w *= avail / total;
            }
        }
        widths.into_iter().map(px).collect()
    }

    /// Lay out table `w` as a grid `width` wide at most.
    pub(crate) fn layout_grid(
        &self,
        w: usize,
        width: Pixels,
        caret: Option<usize>,
        window: &mut Window,
    ) -> Option<Grid> {
        let model = self.table_model(w)?;
        let cols = model.cols().max(1);
        // ZenNotes' table: a little smaller than the text, at 1.5, and in a
        // sans face when the text is monospaced.
        let (size, _) = self.style.metrics(LineKind::Paragraph, self.mode);
        let (size, lh) = (px(size * 0.94), px(size * 0.94 * 1.5));
        let sans = self.style.font.as_ref() == crate::theme::MONO_FONT;
        let live = self.mode == Mode::Live;

        // Each cell's text, and how wide it is unwrapped: a column is as
        // wide as its widest cell, as a browser would draw it.
        let mut natural = vec![MIN_COL; cols];
        let mut prepared = Vec::with_capacity(model.cells.len());
        for (r, row) in model.cells.iter().enumerate() {
            let mut out = Vec::with_capacity(row.len());
            for (c, range) in row.iter().enumerate().take(cols) {
                let text = &self.text[range.clone()];
                let here = caret.is_some_and(|p| range.start <= p && p <= range.end);
                let reveal = if live && here {
                    Reveal::Show
                } else {
                    Reveal::Hide
                };
                let d = display::fragment(text, range.start, reveal);
                let mut runs = self.text_runs(&d, LineKind::Paragraph, 0, (None, false), false);
                if sans {
                    for (run, src) in runs.iter_mut().zip(&d.runs) {
                        if !mdedit_core::display::has(src.style, mdedit_core::display::CODE)
                            && src.role != mdedit_core::display::Role::Pad
                        {
                            run.font.family = crate::theme::UI_FONT.into();
                            run.font.features = FontFeatures::default();
                        }
                    }
                }
                if r == 0 {
                    for run in &mut runs {
                        run.font.weight = FontWeight::SEMIBOLD;
                    }
                }
                let widest = window
                    .text_system()
                    .shape_text(d.text.clone().into(), size, &runs, None, None)
                    .map(|lines| {
                        lines
                            .iter()
                            .map(|l| f32::from(l.width()))
                            .fold(0., f32::max)
                    })
                    .unwrap_or(0.);
                let wide = widest.ceil() + PAD_X * 2. + 2.;
                natural[c] = natural[c].max(wide);
                out.push((range.clone(), d, runs));
            }
            prepared.push(out);
        }
        let widths = self.grid_widths(w, &model, width, &natural);
        let mut col_x = vec![px(0.)];
        for w in &widths {
            col_x.push(*col_x.last().unwrap() + *w);
        }

        let mut cells = Vec::new();
        let mut row_y = vec![px(TOP)];
        let mut base = 0;
        for (r, row) in prepared.into_iter().enumerate() {
            let mut tallest = lh;
            let first = cells.len();
            for (c, (_, d, runs)) in row.into_iter().enumerate() {
                let wrap = super::max(widths[c] - px(PAD_X * 2.), px(8.));
                let shaped = window
                    .text_system()
                    .shape_text(d.text.clone().into(), size, &runs, Some(wrap), None)
                    .map(|v| v.into_vec())
                    .unwrap_or_default();
                // Each line starts after the previous one's break.
                let mut starts = vec![0];
                starts.extend(d.text.match_indices('\n').map(|(i, _)| i + 1));
                let align = model.aligns.get(c).copied().unwrap_or(Align::None);
                let mut y = px(0.);
                let mut lines = Vec::with_capacity(shaped.len());
                for (shaped, start) in shaped.into_iter().zip(starts) {
                    let spare = wrap - shaped.width();
                    let x = match align {
                        _ if spare <= px(0.) => px(0.),
                        Align::Center => spare / 2.,
                        Align::Right => spare,
                        _ => px(0.),
                    };
                    let h = shaped.size(lh).height;
                    lines.push(CellLine {
                        start,
                        at: point(x, y),
                        shaped,
                    });
                    y += super::max(h, lh);
                }
                let height = super::max(y, lh);
                tallest = super::max(tallest, height);
                let len = d.text.len();
                cells.push(GridCell {
                    r,
                    c,
                    display: d,
                    lines,
                    height,
                    base,
                    origin: point(col_x[c] + px(PAD_X), px(0.)),
                    wrap,
                });
                base += len + 1;
            }
            let top = *row_y.last().unwrap();
            for cell in &mut cells[first..] {
                cell.origin.y = top + px(PAD_Y);
            }
            row_y.push(top + tallest + px(PAD_Y * 2.));
        }
        Some(Grid {
            widget: w,
            model,
            cells,
            cols,
            col_x,
            row_y,
            line_height: lh,
        })
    }
}
