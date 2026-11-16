//! Laying out and painting the editor: only the lines on screen are
//! shaped; the rest keep their last measured (or estimated) height so the
//! scroll position adds up.

use gpui_kit::*;
use mdedit_core::prefs::Mode;
use mdedit_core::syntax::LineKind;

use mdedit_core::display::{Gutter, RowWidget};

use super::table::{Control, Drag, RAIL, controls};
use super::{Geometry, LIST_INDENT, MarkdownEditor, Placed, max, min};

const PAD_X: f32 = 32.;
const PAD_TOP: f32 = 28.;

pub(crate) struct EditorElement {
    editor: Entity<MarkdownEditor>,
}

impl EditorElement {
    pub fn new(editor: Entity<MarkdownEditor>) -> Self {
        EditorElement { editor }
    }
}

pub(crate) struct Frame {
    lines: Vec<Placed>,
    widgets: Vec<AnyElement>,
    content_x: Pixels,
    content_w: Pixels,
    rects: Vec<(Bounds<Pixels>, Hsla)>,
    caret: Option<Bounds<Pixels>>,
    thumb: Option<Bounds<Pixels>>,
    hitbox: Hitbox,
    rich: bool,
    code_bg: Hsla,
    quote: Hsla,
    rule: Hsla,
    caret_color: Hsla,
    thumb_color: Hsla,
    alerts: [Hsla; 5],
    box_border: Hsla,
    box_fill: Hsla,
    check: Hsla,
    /// Tables: what goes under the text (header, caret cell) and over it
    /// (rules, borders, the hover controls).
    under: Vec<PaintQuad>,
    over: Vec<PaintQuad>,
    /// Pointer shapes over the table controls, and for a drag in progress.
    cursors: Vec<(Hitbox, CursorStyle)>,
    drag_cursor: Option<CursorStyle>,
}

impl IntoElement for EditorElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

/// What `prepare` hands back: the visible rows, the widgets drawn among
/// them (laid out and placed), the reading column, and the content height.
type Prepared = (Vec<Placed>, Vec<AnyElement>, Pixels, Pixels, Pixels);

impl MarkdownEditor {
    fn prepare(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Prepared {
        let pad = px(PAD_X);
        let avail = max(bounds.size.width - pad * 2., px(160.));
        let content_w = match self.style.width {
            Some(w) => min(px(w), avail),
            None => avail,
        };
        if self.content_w != Some(content_w) {
            self.content_w = Some(content_w);
            self.invalidate();
        }
        let content_x = bounds.left() + (bounds.size.width - content_w) / 2.;
        let view_h = bounds.size.height;
        let top = px(PAD_TOP);

        if self.autoscroll {
            self.autoscroll = false;
            let ix = self.row_at(self.sel.head);
            let layout = self.layout(ix, window);
            let line_top: Pixels = top
                + self.slots[..ix]
                    .iter()
                    .map(|s| s.height)
                    .fold(px(0.), |a, b| a + b);
            let p = layout.point(layout.to_display(self.sel.head));
            let caret_top = line_top + p.y;
            let caret_bottom = caret_top + layout.line_height;
            let margin = min(px(48.), view_h / 4.);
            if std::mem::take(&mut self.scroll_to_top) {
                self.scroll_y = max(caret_top - margin, px(0.));
            } else if caret_top < self.scroll_y + margin {
                self.scroll_y = caret_top - margin;
            } else if caret_bottom > self.scroll_y + view_h - margin {
                self.scroll_y = caret_bottom - view_h + margin;
            }
        }

        let mut y = top;
        let mut lines = Vec::new();
        let mut widgets = Vec::new();
        let n = self.slots.len();
        for ix in 0..n {
            let h = self.slots[ix].height;
            if y + h >= self.scroll_y && y <= self.scroll_y + view_h {
                let layout = self.layout(ix, window);
                let origin = point(content_x, bounds.top() + y - self.scroll_y);
                if let Some(RowWidget::Closed(w) | RowWidget::Preview(w)) = layout.widget {
                    // Lay the widget out at the column's width, under the
                    // row's text (a preview) or in place of it.
                    let mut el = self.widget_element(w, content_w, cx);
                    let space = size(
                        AvailableSpace::Definite(content_w),
                        AvailableSpace::MinContent,
                    );
                    let measured = el.layout_as_root(space, window, cx);
                    el.prepaint_at(origin + point(px(0.), layout.text_h), window, cx);
                    widgets.push(el);
                    let height = layout.text_h + measured.height;
                    layout.height.set(height);
                    self.slots[ix].height = height;
                }
                lines.push((ix, origin, layout.clone()));
                y += layout.height.get();
            } else {
                y += h;
            }
        }
        // Room to scroll the last line up to the middle of the window.
        let total = y + view_h * 0.4;
        let max_scroll = max(total - view_h, px(0.));
        if self.scroll_y > max_scroll {
            self.scroll_y = max_scroll;
        }
        (lines, widgets, content_x, content_w, total)
    }

    /// The quads that draw the visible tables, and their controls when one
    /// is hovered: v1's look. Also the pointer shapes over those controls.
    #[allow(clippy::type_complexity)]
    fn paint_tables(
        &self,
        lines: &[Placed],
        focused: bool,
        window: &mut Window,
    ) -> (
        Vec<PaintQuad>,
        Vec<PaintQuad>,
        Vec<(Hitbox, CursorStyle)>,
        Option<CursorStyle>,
    ) {
        let c = &self.style.colors;
        let p = &self.style.palette;
        let accent_text = super::style::hsla(p.accent_text);
        // The controls take ZenNotes' colours, which every theme here was made
        // from: its bg-1 at 90% for the rails, bg-2 for the grips, and bg-3 at
        // 90% round both. Those are our soft background, the diagrams' second
        // surface, and the border.
        let bg = super::style::hsla(p.bg);
        let rail = bg.blend(super::style::hsla(p.bg_soft).opacity(0.9));
        let grip = super::style::hsla(p.diagram_surface_alt);
        let edge = super::style::hsla(p.border).opacity(0.9);
        let (mut under, mut over, mut cursors) = (Vec::new(), Vec::new(), Vec::new());
        let drag = self.table_dragging();
        let drag_cursor = match drag {
            Some(Drag::Row { .. } | Drag::Col { .. }) => Some(CursorStyle::ClosedHand),
            Some(Drag::Resize { .. }) => Some(CursorStyle::ResizeColumn),
            _ => None,
        };
        for (ix, origin, l) in lines {
            let Some(g) = &l.grid else { continue };
            let at = |b: Bounds<Pixels>| Bounds::new(*origin + b.origin, b.size);
            let table = at(g.bounds());
            let radius = px(6.);
            // The header's shading, then the caret's cell.
            let head = at(Bounds::from_corners(
                point(px(0.), g.row_y[0]),
                point(g.width(), g.row_y[1]),
            ));
            under.push(fill(head, c.code_bg).corner_radii(Corners {
                top_left: radius,
                top_right: radius,
                bottom_left: px(0.),
                bottom_right: px(0.),
            }));
            if focused
                && self.sel.is_empty()
                && let Some(k) = g.cell_at_source(self.sel.head)
            {
                let cell = at(g.cell_bounds(g.cells[k].r, g.cells[k].c));
                under.push(fill(cell, c.accent.opacity(0.10)));
                over.push(quad(
                    cell,
                    px(0.),
                    transparent_black(),
                    px(2.),
                    c.accent.opacity(0.45),
                    BorderStyle::Solid,
                ));
            }
            // Rules between rows and columns, and the border round it all.
            for y in &g.row_y[1..g.row_y.len() - 1] {
                over.push(fill(
                    Bounds::new(*origin + point(px(0.), *y), size(g.width(), px(1.))),
                    c.border,
                ));
            }
            for x in &g.col_x[1..g.col_x.len() - 1] {
                over.push(fill(
                    Bounds::new(
                        point(origin.x + *x, table.top()),
                        size(px(1.), table.size.height),
                    ),
                    c.border,
                ));
            }
            over.push(outline(table, c.border, BorderStyle::Solid).corner_radii(radius));

            // The controls, while the pointer is near (or a press is on).
            let hover = self.table_hover.filter(|h| h.row == *ix);
            let active = drag.filter(|d| d.row() == *ix);
            if hover.is_none() && active.is_none() {
                continue;
            }
            let moving = matches!(active, Some(Drag::Row { .. } | Drag::Col { .. }));
            let resizing = match active {
                Some(Drag::Resize { col, .. }) => Some(col),
                _ => None,
            };
            let under_pointer = hover.and_then(|h| h.control);
            for (control, b) in controls(g) {
                let b = at(b);
                match control {
                    // A pill along the edge with a fine `+` in its middle. The
                    // one under the pointer lights up.
                    Control::AddCol | Control::AddRow => {
                        let lit = under_pointer == Some(control)
                            || matches!(active, Some(Drag::Rail { control: k, .. }) if k == control);
                        let pill = px(RAIL / 2.);
                        over.push(if lit {
                            fill(b, c.accent.opacity(0.85)).corner_radii(pill)
                        } else {
                            fill(b, rail)
                                .corner_radii(pill)
                                .border_widths(px(1.))
                                .border_color(edge)
                        });
                        let ink = if lit { accent_text } else { c.muted };
                        let mid = b.center();
                        over.push(fill(
                            Bounds::new(mid - point(px(3.5), px(0.5)), size(px(7.), px(1.))),
                            ink,
                        ));
                        over.push(fill(
                            Bounds::new(mid - point(px(0.5), px(3.5)), size(px(1.), px(7.))),
                            ink,
                        ));
                        cursors.push((
                            window.insert_hitbox(b, HitboxBehavior::Normal),
                            CursorStyle::PointingHand,
                        ));
                    }
                    Control::ColGrip(i) | Control::RowGrip(i) => {
                        let col = matches!(control, Control::ColGrip(_));
                        let on = if col {
                            hover.and_then(|h| h.head) == Some(i)
                        } else {
                            hover.and_then(|h| h.body) == Some(i)
                        };
                        cursors.push((
                            window.insert_hitbox(b, HitboxBehavior::Normal),
                            CursorStyle::OpenHand,
                        ));
                        if !on || moving || resizing.is_some() {
                            continue;
                        }
                        // A tab the length of the column (or row), flush with
                        // the table: its inner edge lies on the table's border
                        // and its ends on the rules either side.
                        let pill = if col {
                            Bounds::from_corners(
                                point(b.left(), b.top() + px(1.)),
                                point((b.right() + px(1.)).min(table.right()), b.bottom() + px(1.)),
                            )
                        } else {
                            Bounds::from_corners(
                                point(b.left() + px(1.), b.top()),
                                point(
                                    b.right() + px(1.),
                                    (b.bottom() + px(1.)).min(table.bottom()),
                                ),
                            )
                        };
                        over.push(
                            fill(pill, grip)
                                .corner_radii(px(4.))
                                .border_widths(px(1.))
                                .border_color(edge),
                        );
                        // ⠿ in fine dots, on its side for a column.
                        let (across, down) = if col { (3, 2) } else { (2, 3) };
                        let (dot, pitch) = (1.5, 2.75);
                        let mid = pill.center();
                        for a in 0..across {
                            for d in 0..down {
                                let x = mid.x
                                    + px((a as f32 - (across - 1) as f32 / 2.) * pitch - dot / 2.);
                                let y = mid.y
                                    + px((d as f32 - (down - 1) as f32 / 2.) * pitch - dot / 2.);
                                over.push(
                                    fill(Bounds::new(point(x, y), size(px(dot), px(dot))), c.muted)
                                        .corner_radii(px(dot / 2.)),
                                );
                            }
                        }
                    }
                    Control::Resize(i) => {
                        cursors.push((
                            window.insert_hitbox(b, HitboxBehavior::Normal),
                            CursorStyle::ResizeColumn,
                        ));
                        let x = origin.x + g.col_x[i + 1] - px(1.);
                        if resizing == Some(i) {
                            over.push(fill(
                                Bounds::new(point(x, table.top()), size(px(2.), table.size.height)),
                                c.accent.opacity(0.55),
                            ));
                        } else if !moving
                            && resizing.is_none()
                            && hover.and_then(|h| h.head) == Some(i)
                        {
                            over.push(fill(
                                Bounds::new(point(x, b.top()), size(px(2.), b.size.height)),
                                c.accent.opacity(0.55),
                            ));
                        }
                    }
                }
            }
            // Where a dragged row or column would land.
            match active {
                Some(Drag::Row { from, to, .. }) if from != to => {
                    let y = if to > from {
                        g.row_y[to + 2]
                    } else {
                        g.row_y[to + 1]
                    };
                    over.push(fill(
                        Bounds::new(
                            point(table.left(), origin.y + y - px(1.)),
                            size(table.size.width, px(2.)),
                        ),
                        c.accent,
                    ));
                }
                Some(Drag::Col { from, to, .. }) if from != to => {
                    let x = if to > from {
                        g.col_x[to + 1]
                    } else {
                        g.col_x[to]
                    };
                    over.push(fill(
                        Bounds::new(
                            point(origin.x + x - px(1.), table.top()),
                            size(px(2.), table.size.height),
                        ),
                        c.accent,
                    ));
                }
                _ => {}
            }
        }
        (under, over, cursors, drag_cursor)
    }
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Frame;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Frame {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        self.editor.update(cx, |ed, cx| {
            let (lines, widgets, content_x, content_w, total) = ed.prepare(bounds, window, cx);
            let colors = ed.style.colors.clone();
            let mut rects = Vec::new();

            // Find matches, then the selection, over the visible lines.
            let sel = ed.sel.range();
            for (ix, origin, l) in &lines {
                let src = ed.row_range(*ix);
                if let Some(f) = &ed.find {
                    let first = f.matches.partition_point(|m| m.end < src.start);
                    for (i, m) in f.matches[first..].iter().enumerate() {
                        if m.start > src.end {
                            break;
                        }
                        let a = l.to_display(m.start.max(src.start));
                        let b = l.to_display(m.end.min(src.end));
                        let color = if f.current == Some(first + i) {
                            colors.find_current
                        } else {
                            colors.find
                        };
                        for r in l.rects(*origin, a, b, false, content_w - l.indent) {
                            rects.push((r, color));
                        }
                    }
                }
                if !sel.is_empty() && sel.start <= src.end && sel.end >= src.start {
                    let a = l.to_display(sel.start.max(src.start));
                    let b = l.to_display(sel.end.min(src.end));
                    let to_eol = sel.end > src.end && l.grid.is_none();
                    for r in l.rects(*origin, a, b, to_eol, content_w - l.indent) {
                        rects.push((r, colors.selection));
                    }
                }
            }

            let caret_ix = ed.row_at(ed.sel.head);
            let caret = lines
                .iter()
                .find(|(ix, ..)| *ix == caret_ix)
                .map(|(_, origin, l)| {
                    let p = l.point(l.to_display(ed.sel.head));
                    Bounds::new(*origin + p, size(px(2.), l.line_height))
                });

            let view_h = bounds.size.height;
            let thumb = (total > view_h + px(1.)).then(|| {
                let h = max(view_h * (view_h / total), px(28.));
                let y = (view_h - h) * (ed.scroll_y / (total - view_h));
                Bounds::new(
                    point(bounds.right() - px(9.), bounds.top() + y),
                    size(px(5.), h),
                )
            });

            let focused = ed.focus.is_focused(window);
            let (under, over, cursors, drag_cursor) = ed.paint_tables(&lines, focused, window);
            ed.geometry = Geometry {
                bounds,
                lines: lines.clone(),
            };
            Frame {
                lines,
                widgets,
                content_x,
                content_w,
                rects,
                caret,
                thumb,
                hitbox,
                rich: ed.mode != Mode::Source,
                code_bg: colors.code_bg,
                quote: colors.quote,
                rule: colors.border,
                caret_color: colors.accent,
                thumb_color: colors.muted.opacity(0.35),
                alerts: colors.alerts,
                box_border: colors.muted.opacity(0.7),
                box_fill: colors.accent,
                check: colors.text,
                under,
                over,
                cursors,
                drag_cursor,
            }
        })
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        frame: &mut Frame,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.editor.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );
        window.set_cursor_style(
            frame.drag_cursor.unwrap_or(CursorStyle::IBeam),
            &frame.hitbox,
        );
        if frame.drag_cursor.is_none() {
            for (hitbox, style) in &frame.cursors {
                window.set_cursor_style(*style, hitbox);
            }
        }

        // Mouse: press, drag-select (even outside), release, wheel.
        {
            let editor = self.editor.clone();
            let hitbox = frame.hitbox.clone();
            window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                if phase.bubble() && e.button == MouseButton::Left && hitbox.is_hovered(window) {
                    editor.update(cx, |ed, cx| ed.mouse_down(e, window, cx));
                    cx.stop_propagation();
                }
            });
            // A right press on a table cell: its menu.
            let editor = self.editor.clone();
            let hitbox = frame.hitbox.clone();
            window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                if phase.bubble()
                    && e.button == MouseButton::Right
                    && hitbox.is_hovered(window)
                    && editor.update(cx, |ed, cx| ed.table_menu_at(e.position, window, cx))
                {
                    cx.stop_propagation();
                }
            });
            let editor = self.editor.clone();
            window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                if !phase.bubble() {
                    return;
                }
                if editor.read(cx).is_selecting() {
                    if e.pressed_button == Some(MouseButton::Left) {
                        editor.update(cx, |ed, cx| ed.mouse_drag(e.position, cx));
                    } else {
                        editor.update(cx, |ed, _| ed.mouse_up());
                    }
                } else {
                    // Hovering a table shows its controls; a press on one
                    // drags.
                    editor.update(cx, |ed, cx| ed.table_pointer(e.position, cx));
                }
            });
            let editor = self.editor.clone();
            window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                if phase.bubble() {
                    editor.update(cx, |ed, cx| {
                        ed.mouse_up();
                        ed.table_release(cx);
                    });
                }
            });
            let editor = self.editor.clone();
            let hitbox = frame.hitbox.clone();
            window.on_mouse_event(move |e: &ScrollEvent, phase, window, cx| {
                if phase.bubble() && hitbox.should_handle_scroll(window) {
                    let dy = e.delta.pixel_delta(px(20.)).y;
                    editor.update(cx, |ed, cx| ed.scroll(dy, cx));
                    cx.stop_propagation();
                }
            });
        }

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            // Block backgrounds and rules, behind everything.
            if frame.rich {
                for (_, origin, l) in &frame.lines {
                    let full = Bounds::new(*origin, size(frame.content_w, l.height.get()));
                    match l.kind {
                        _ if matches!(l.widget, Some(RowWidget::Closed(_))) => {}
                        LineKind::Code | LineKind::FenceOpen | LineKind::FenceClose => {
                            window.paint_quad(fill(full, frame.code_bg));
                        }
                        LineKind::Hr => {
                            let y = origin.y + l.height.get() / 2.;
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(frame.content_x, y),
                                    size(frame.content_w, px(1.)),
                                ),
                                frame.rule,
                            ));
                        }
                        _ => {}
                    }
                    let bar = l.alert.map_or(frame.quote, |a| frame.alerts[a as usize]);
                    for q in 0..l.quote {
                        let x = origin.x + px(q as f32 * 20. + 2.);
                        window.paint_quad(fill(
                            Bounds::new(point(x, origin.y), size(px(3.), l.height.get())),
                            bar,
                        ));
                    }
                }
            }
            for q in &frame.under {
                window.paint_quad(q.clone());
            }
            for (r, color) in &frame.rects {
                window.paint_quad(fill(*r, *color));
            }
            // List markers and task boxes, in the margin.
            for (_, origin, l) in &frame.lines {
                let row = origin.y + l.pad_top;
                if let Some(g) = &l.gutter {
                    let x = origin.x + l.indent - px(8.) - g.width();
                    g.paint(
                        point(x, row),
                        l.line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    )
                    .ok();
                }
                if let Some(Gutter::Task { .. }) = &l.display.gutter {
                    let done = matches!(l.display.gutter, Some(Gutter::Task { done: true, .. }));
                    let side = px(14.);
                    let x = origin.x + l.indent - px(LIST_INDENT) + px(4.);
                    let y = row + (l.line_height - side) / 2.;
                    let b = Bounds::new(point(x, y), size(side, side));
                    if done {
                        window.paint_quad(fill(b, frame.box_fill).corner_radii(px(3.)));
                        // A tick: two strokes as thin rotated quads aren't
                        // available, so a small inner square reads as done.
                        let inner =
                            Bounds::new(point(x + px(4.), y + px(4.)), size(px(6.), px(6.)));
                        window.paint_quad(fill(inner, frame.check).corner_radii(px(1.5)));
                    } else {
                        window.paint_quad(
                            outline(b, frame.box_border, BorderStyle::Solid).corner_radii(px(3.)),
                        );
                    }
                }
            }
            for (_, origin, l) in &frame.lines {
                if let Some(g) = &l.grid {
                    for cell in &g.cells {
                        for line in &cell.lines {
                            line.shaped
                                .paint(
                                    *origin + cell.origin + line.at,
                                    g.line_height,
                                    TextAlign::Left,
                                    None,
                                    window,
                                    cx,
                                )
                                .ok();
                        }
                    }
                    continue;
                }
                let text_origin = *origin + point(l.indent, l.pad_top);
                l.shaped
                    .paint(
                        text_origin,
                        l.line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    )
                    .ok();
            }
            for el in &mut frame.widgets {
                el.paint(window, cx);
            }
            for q in &frame.over {
                window.paint_quad(q.clone());
            }
            if focus.is_focused(window)
                && let Some(c) = frame.caret
            {
                window.paint_quad(fill(c, frame.caret_color));
            }
            if let Some(t) = frame.thumb {
                window.paint_quad(fill(t, frame.thumb_color).corner_radii(px(3.)));
            }
        });
    }
}
