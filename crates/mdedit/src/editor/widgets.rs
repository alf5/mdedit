//! Blocks drawn as something other than text: tables, mermaid diagrams,
//! display formulas and images. Each is drawn in place of its source until
//! the caret goes into it; then the source shows (a diagram or formula
//! keeps a live preview under it). A table is edited as padded source, with
//! Tab, Enter and Escape moving between cells, and is re-padded when the
//! caret leaves it.

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mdedit_core::buffer::{Change, Edit, Selection, map_pos};
use mdedit_core::display::{self, Reveal};
use mdedit_core::history::Kind;
use mdedit_core::mermaid::{self, Colors, DrawOptions, fontdb};
use mdedit_core::syntax::{self, LineKind, WidgetKind};
use mdedit_core::table::{Align, TableModel};
use mdedit_core::{math, os};

use super::MarkdownEditor;
use crate::theme::{FONT_FILES, MONO_FONT, UI_FONT};

/// A drawn diagram or formula, or why there isn't one.
pub(crate) type Picture = Result<(Arc<Image>, f32, f32), String>;

/// The diagram renderer's fonts: the app's own, so labels are measured and
/// outlined in the face the rest of the UI uses.
pub(crate) fn fonts() -> Arc<fontdb::Database> {
    static DB: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        for f in FONT_FILES {
            db.load_font_data(f.to_vec());
        }
        db.set_sans_serif_family(UI_FONT);
        db.set_monospace_family(MONO_FONT);
        Arc::new(db)
    })
    .clone()
}

fn hash_of(parts: &[&dyn Fn(&mut std::collections::hash_map::DefaultHasher)]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in parts {
        p(&mut h);
    }
    h.finish()
}

/// A palette's colours as a diagram's.
pub(crate) fn diagram_colors(p: &mdedit_core::themes::Palette) -> Colors {
    Colors {
        bg: p.bg,
        fg: p.text,
        muted: p.text_muted,
        line: p.text,
        surface: p.bg_soft,
        surface_alt: p.diagram_surface_alt,
        border: p.diagram_border,
        accent: p.accent,
        critical: p.danger,
        series: [
            p.accent,
            p.alert_tip,
            p.alert_warning,
            p.alert_important,
            p.alert_caution,
            p.alert_note,
            p.text_muted,
            p.diagram_border,
        ],
    }
}

/// A table command from the cell menu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TableOp {
    RowAbove,
    RowBelow,
    ColLeft,
    ColRight,
    DuplicateRow,
    DeleteRow,
    DeleteCol,
    DeleteTable,
    Align(Align),
    Sort(bool),
    Copy,
}

impl MarkdownEditor {
    /// Where the document's relative images are looked up.
    pub fn set_base_dir(&mut self, dir: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.base_dir != dir {
            self.base_dir = dir;
            cx.notify();
        }
    }

    /// The widget (index into `syntax.widgets`) whose lines hold `pos`.
    pub(crate) fn widget_at(&self, pos: usize) -> Option<usize> {
        let line = self.syntax.line_at(pos);
        self.syntax
            .widgets
            .iter()
            .position(|w| w.lines.contains(&line))
    }

    fn widget_lines(&self, w: usize) -> Vec<Range<usize>> {
        self.syntax.widgets[w]
            .lines
            .clone()
            .map(|l| self.syntax.lines[l].range.clone())
            .collect()
    }

    fn table_model(&self, w: usize) -> Option<TableModel> {
        TableModel::parse(&self.text, &self.widget_lines(w))
    }

    fn picture(&mut self, body: &str, diagram: bool) -> Picture {
        let c = &self.style.colors;
        let key = hash_of(&[
            &|h| body.hash(h),
            &|h| diagram.hash(h),
            &|h| format!("{:?}{:?}", c.text, c.code_bg).hash(h),
            &|h| self.style.size.to_bits().hash(h),
        ]);
        if let Some(p) = self.pictures.get(&key) {
            return p.clone();
        }
        let p = if diagram {
            self.draw_diagram(body)
        } else {
            self.draw_math(body)
        };
        if self.pictures.len() > 200 {
            self.pictures.clear();
        }
        self.pictures.insert(key, p.clone());
        p
    }

    fn draw_diagram(&self, body: &str) -> Picture {
        let opts = DrawOptions {
            colors: diagram_colors(&self.style.palette),
            font_family: UI_FONT.into(),
            font_family_mono: MONO_FONT.into(),
            fontdb: fonts(),
        };
        let d = mermaid::draw(body, &opts)?;
        Ok((
            Arc::new(Image::from_bytes(ImageFormat::Svg, d.svg.into_bytes())),
            d.width as f32,
            d.height as f32,
        ))
    }

    fn draw_math(&self, body: &str) -> Picture {
        let p = &self.style.palette;
        // Points to pixels: CSS's 96 / 72.
        let size_pt = self.style.size * 0.75 * 1.15;
        let m = math::draw(
            body,
            true,
            size_pt as f64,
            [p.text[0], p.text[1], p.text[2]],
        )?;
        Ok((
            Arc::new(Image::from_bytes(ImageFormat::Svg, m.svg.into_bytes())),
            m.width_pt as f32 * 4. / 3.,
            m.height_pt as f32 * 4. / 3.,
        ))
    }

    /// The element widget `w` is drawn as, `width` wide at most.
    pub(crate) fn widget_element(
        &mut self,
        w: usize,
        width: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kind = self.syntax.widgets[w].kind.clone();
        let c = self.style.colors.clone();
        match kind {
            WidgetKind::Table => self.table_element(w, width, cx),
            WidgetKind::Diagram { body } | WidgetKind::Math { body } => {
                let diagram = matches!(self.syntax.widgets[w].kind, WidgetKind::Diagram { .. });
                let src = self.text[body.clone()].to_string();
                let start = body.start;
                let editor = cx.entity().downgrade();
                let open = move |_: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                    editor
                        .update(cx, |e, cx| e.reveal_offset(start, window, cx))
                        .ok();
                };
                match self.picture(&src, diagram) {
                    Ok((image, w_px, h_px)) => {
                        let scale = (f32::from(width) / w_px).min(1.0);
                        div()
                            .w(width)
                            .flex()
                            .justify_center()
                            .py(px(10.))
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, open)
                            .child(img(image).w(px(w_px * scale)).h(px(h_px * scale)))
                            .into_any_element()
                    }
                    Err(e) => div()
                        .w(width)
                        .my(px(6.))
                        .px(px(12.))
                        .py(px(8.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(c.danger)
                        .text_color(c.danger)
                        .text_sm()
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, open)
                        .child(if diagram {
                            format!("Diagram: {e}")
                        } else {
                            format!("Formula: {e}")
                        })
                        .into_any_element(),
                }
            }
            WidgetKind::Image { url, alt } => {
                let remote_waiting;
                let source: Option<ImageSource> =
                    match os::resolve_image(&url, self.base_dir.as_deref()) {
                        os::ImageRef::Local(path) => {
                            remote_waiting = false;
                            Some(path.into())
                        }
                        os::ImageRef::Remote(u) if self.allow_remote => {
                            remote_waiting = false;
                            Some(SharedUri::from(u).into())
                        }
                        os::ImageRef::Remote(_) => {
                            remote_waiting = true;
                            None
                        }
                        os::ImageRef::Refused => {
                            remote_waiting = false;
                            None
                        }
                    };
                let line = self.syntax.widgets[w].lines.start;
                let start = self.syntax.lines[line].range.start;
                let editor = cx.entity().downgrade();
                let placeholder = {
                    let c = c.clone();
                    let alt = if alt.is_empty() {
                        url.clone()
                    } else {
                        alt.clone()
                    };
                    move || {
                        div()
                            .px(px(10.))
                            .py(px(6.))
                            .rounded(px(5.))
                            .bg(c.code_bg)
                            .text_color(c.muted)
                            .text_sm()
                            .child(if remote_waiting {
                                format!("🖼 {alt} — click to load images from the web")
                            } else {
                                format!("🖼 {alt}")
                            })
                            .into_any_element()
                    }
                };
                div()
                    .w(width)
                    .py(px(8.))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        editor
                            .update(cx, |e, cx| {
                                if remote_waiting {
                                    // Asked for: this document's web images load.
                                    e.allow_remote = true;
                                    cx.notify();
                                } else {
                                    e.reveal_offset(start, window, cx);
                                }
                            })
                            .ok();
                    })
                    .child(match source {
                        Some(s) => img(s)
                            .max_w(width)
                            .max_h(px(560.))
                            .object_fit(ObjectFit::ScaleDown)
                            .with_fallback(placeholder)
                            .into_any_element(),
                        None => placeholder(),
                    })
                    .into_any_element()
            }
        }
    }

    // ---------- tables ----------

    /// A cell's text as styled runs, its markup hidden.
    fn cell_text(&self, text: &str, header: bool) -> StyledText {
        let s = syntax::parse(text);
        if s.lines.is_empty() {
            return StyledText::new(text.to_string());
        }
        let d = display::display_row(text, &s, 0..s.lines.len(), Reveal::Hide);
        let mut runs = self.text_runs(&d, LineKind::Paragraph, 0);
        if header {
            for r in &mut runs {
                r.font.weight = FontWeight::SEMIBOLD;
            }
        }
        StyledText::new(d.text).with_runs(runs)
    }

    fn table_element(&mut self, w: usize, width: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let c = self.style.colors.clone();
        let Some(model) = self.table_model(w) else {
            return div()
                .child("This table couldn't be read.")
                .into_any_element();
        };
        let cols = model.cols().max(1);
        // Saved widths win; otherwise columns share the width by content.
        let natural: Vec<f32> = (0..cols)
            .map(|col| {
                model
                    .rows
                    .iter()
                    .filter_map(|r| r.get(col))
                    .map(|t| t.chars().count())
                    .max()
                    .unwrap_or(3)
                    .clamp(3, 60) as f32
                    * self.style.size
                    * 0.68
                    + 28.
            })
            .collect();
        let saved: Vec<Option<f32>> = (0..cols)
            .map(|i| model.widths.get(i).copied().flatten().map(|v| v as f32))
            .collect();
        let mut widths: Vec<f32> = (0..cols).map(|i| saved[i].unwrap_or(natural[i])).collect();
        let total: f32 = widths.iter().sum();
        let avail = f32::from(width);
        if total > avail {
            for w in &mut widths {
                *w *= avail / total;
            }
        }
        let table_line = self.syntax.widgets[w].lines.start;
        let editor = cx.entity().downgrade();
        let rows = model.rows.len();
        div()
            .my(px(8.))
            .flex()
            .flex_col()
            .border_1()
            .border_color(c.border)
            .rounded(px(6.))
            .overflow_hidden()
            .text_size(px(self.style.size * 0.95))
            .children((0..rows).map(|r| {
                div()
                    .flex()
                    .when(r == 0, |d| d.bg(c.code_bg))
                    .when(r > 0, |d| d.border_t_1().border_color(c.border))
                    .children((0..cols).map(|col| {
                        let text = model.rows[r].get(col).cloned().unwrap_or_default();
                        let pos = model.cells[r]
                            .get(col)
                            .map_or(model.range.start, |c| c.start);
                        let align = model.aligns.get(col).copied().unwrap_or(Align::None);
                        let editor_click = editor.clone();
                        let editor_menu = editor.clone();
                        div()
                            .id(("cell", (table_line * 1000 + r) * 100 + col))
                            .w(px(widths[col]))
                            .flex_none()
                            .px(px(10.))
                            .py(px(6.))
                            .when(col > 0, |d| d.border_l_1().border_color(c.border))
                            .map(|d| match align {
                                Align::Center => d.text_center(),
                                Align::Right => d.text_right(),
                                _ => d,
                            })
                            .child(self.cell_text(&text, r == 0))
                            .on_mouse_down(MouseButton::Left, move |e, window, cx| {
                                // A link in a cell opens; anything else edits.
                                editor_click
                                    .update(cx, |ed, cx| {
                                        if e.modifiers.secondary(cx)
                                            && let Some(link) = ed.syntax.link_at(pos)
                                            && os::is_openable_link(&link.url)
                                        {
                                            cx.open_url(&link.url);
                                        } else {
                                            ed.focus.focus(window, cx);
                                            ed.set_selection(Selection::caret(pos), cx);
                                        }
                                    })
                                    .ok();
                            })
                            .context_menu(move |menu, _, _| {
                                table_menu(menu, editor_menu.clone(), table_line, r, col)
                            })
                    }))
            }))
            .into_any_element()
    }

    /// Apply `op` to the table starting at line `line`, at cell (r, c).
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
        match op {
            TableOp::RowAbove => t.insert_row(r.max(1)),
            TableOp::RowBelow => t.insert_row(r + 1),
            TableOp::ColLeft => t.insert_col(c),
            TableOp::ColRight => t.insert_col(c + 1),
            TableOp::DuplicateRow => t.duplicate_row(r),
            TableOp::DeleteRow => t.delete_row(r),
            TableOp::DeleteCol => t.delete_col(c),
            TableOp::Align(a) => t.set_align(c, a),
            TableOp::Sort(desc) => t.sort_by(c, desc),
            TableOp::Copy => {
                cx.write_to_clipboard(ClipboardItem::new_string(t.render()));
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
        }
        let rendered = t.render();
        let edits = vec![Edit::replace(t.range.clone(), rendered)];
        let sel = Selection::caret(map_pos(self.sel.head, &edits, false));
        self.apply(
            Change {
                edits,
                selection: sel,
            },
            Kind::Other,
            cx,
        );
    }

    /// Tab, Shift+Tab, Enter and Escape in an open table. `None` when the
    /// caret isn't in one.
    pub(crate) fn table_key(&mut self, key: TableKey, cx: &mut Context<Self>) -> bool {
        let Some(w) = self.widget_at(self.sel.head) else {
            return false;
        };
        if self.syntax.widgets[w].kind != WidgetKind::Table || !self.rich() {
            return false;
        }
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
        let rendered = t.render();
        let mut edits = vec![Edit::replace(t.range.clone(), rendered.clone())];
        let at = match target {
            Some((r, c)) => t.range.start + t.rendered_cell(r, c).unwrap_or(0),
            None => {
                // Out of the table, onto the line after it (made if needed).
                if !self.text[t.range.end..].starts_with('\n') {
                    edits.push(Edit::insert(t.range.end, "\n"));
                }
                t.range.start + rendered.len() + 1
            }
        };
        self.apply(
            Change {
                edits,
                selection: Selection::caret(at),
            },
            Kind::Other,
            cx,
        );
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
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum TableKey {
    Next,
    Prev,
    Down,
    Leave,
}

fn table_menu(
    menu: PopupMenu,
    editor: WeakEntity<MarkdownEditor>,
    line: usize,
    r: usize,
    c: usize,
) -> PopupMenu {
    let item = |label: &'static str, op: TableOp| {
        let editor = editor.clone();
        PopupMenuItem::new(label).on_click(move |_, _, cx| {
            editor
                .update(cx, |e, cx| e.table_op(line, op, r, c, cx))
                .ok();
        })
    };
    menu.item(item("Insert Row Above", TableOp::RowAbove))
        .item(item("Insert Row Below", TableOp::RowBelow))
        .item(item("Insert Column Left", TableOp::ColLeft))
        .item(item("Insert Column Right", TableOp::ColRight))
        .item(item("Duplicate Row", TableOp::DuplicateRow))
        .separator()
        .item(item("Align Left", TableOp::Align(Align::Left)))
        .item(item("Align Center", TableOp::Align(Align::Center)))
        .item(item("Align Right", TableOp::Align(Align::Right)))
        .item(item("Sort Ascending", TableOp::Sort(false)))
        .item(item("Sort Descending", TableOp::Sort(true)))
        .separator()
        .item(item("Copy Table as Markdown", TableOp::Copy))
        .separator()
        .item(item("Delete Row", TableOp::DeleteRow))
        .item(item("Delete Column", TableOp::DeleteCol))
        .item(item("Delete Table", TableOp::DeleteTable))
}
