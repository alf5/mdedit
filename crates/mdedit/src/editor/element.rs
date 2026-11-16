//! Laying out and painting the editor: only the lines on screen are
//! shaped; the rest keep their last measured (or estimated) height so the
//! scroll position adds up.

use gpui_kit::*;
use mdedit_core::prefs::Mode;
use mdedit_core::syntax::LineKind;

use mdedit_core::display::{Gutter, RowWidget};

use super::{Geometry, LIST_INDENT, LineLayout, MarkdownEditor, Placed, max, min};

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
}

impl IntoElement for EditorElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

/// Rectangles covering display range `a..b` of a (possibly wrapped) line.
fn range_rects(
    origin: Point<Pixels>,
    l: &LineLayout,
    a: usize,
    b: usize,
    to_eol: bool,
    width: Pixels,
) -> Vec<Bounds<Pixels>> {
    let pa = l.point(a);
    let pb = l.point(b);
    let lh = l.line_height;
    let eol = if to_eol { px(6.) } else { px(0.) };
    let mut out = Vec::new();
    if pa.y == pb.y {
        out.push(Bounds::from_corners(
            origin + point(pa.x, pa.y),
            origin + point(pb.x + eol, pb.y + lh),
        ));
        return out;
    }
    let right = l.indent + width;
    out.push(Bounds::from_corners(
        origin + pa,
        origin + point(right, pa.y + lh),
    ));
    let mut y = pa.y + lh;
    while y < pb.y {
        out.push(Bounds::from_corners(
            origin + point(l.indent, y),
            origin + point(right, y + lh),
        ));
        y += lh;
    }
    out.push(Bounds::from_corners(
        origin + point(l.indent, pb.y),
        origin + point(pb.x + eol, pb.y + lh),
    ));
    out
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
            let p = layout.point(layout.display.to_display(self.sel.head));
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
                        let a = l.display.to_display(m.start.max(src.start));
                        let b = l.display.to_display(m.end.min(src.end));
                        let color = if f.current == Some(first + i) {
                            colors.find_current
                        } else {
                            colors.find
                        };
                        for r in range_rects(*origin, l, a, b, false, content_w - l.indent) {
                            rects.push((r, color));
                        }
                    }
                }
                if !sel.is_empty() && sel.start <= src.end && sel.end >= src.start {
                    let a = l.display.to_display(sel.start.max(src.start));
                    let b = l.display.to_display(sel.end.min(src.end));
                    let to_eol = sel.end > src.end;
                    for r in range_rects(*origin, l, a, b, to_eol, content_w - l.indent) {
                        rects.push((r, colors.selection));
                    }
                }
            }

            let caret_ix = ed.row_at(ed.sel.head);
            let caret = lines
                .iter()
                .find(|(ix, ..)| *ix == caret_ix)
                .map(|(_, origin, l)| {
                    let p = l.point(l.display.to_display(ed.sel.head));
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
        window.set_cursor_style(CursorStyle::IBeam, &frame.hitbox);

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
            let editor = self.editor.clone();
            window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                if phase.bubble() && editor.read(cx).is_selecting() {
                    if e.pressed_button == Some(MouseButton::Left) {
                        editor.update(cx, |ed, cx| ed.mouse_drag(e.position, cx));
                    } else {
                        editor.update(cx, |ed, _| ed.mouse_up());
                    }
                }
            });
            let editor = self.editor.clone();
            window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                if phase.bubble() {
                    editor.update(cx, |ed, _| ed.mouse_up());
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
