//! The hex editor: rows of sixteen bytes, an offset column and an ASCII crib.
//!
//! Hex digits type over the byte under the cursor, high nibble then low,
//! which is one undo step. **Overwrite** (the default) can't change the
//! file's length, so it can't shift every offset in a structured binary;
//! **insert** (`Insert` or `Ctrl/Cmd+Shift+O`) adds bytes, and Backspace and
//! Delete remove them. The bytes and their undo history are
//! `mdedit_core::hex::HexDoc`; this owns the cursor, the nibble and scroll.

use gpui_kit::component::ActiveTheme;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mdedit_core::hex::{self, HexDoc, ROW};

use crate::actions::{HexRedo, HexUndo, ToggleHexOverwrite};
use crate::theme::MONO_FONT;

const ROW_HEIGHT: f32 = 22.;

pub enum HexEvent {
    Changed,
}

pub struct HexView {
    pub doc: HexDoc,
    saved: Vec<u8>,
    cursor: usize,
    /// The next digit types the high nibble.
    high: bool,
    pub overwrite: bool,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
}

impl EventEmitter<HexEvent> for HexView {}

impl Focusable for HexView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl HexView {
    pub fn new(bytes: Vec<u8>, cx: &mut Context<Self>) -> Self {
        HexView {
            saved: bytes.clone(),
            doc: HexDoc::new(bytes),
            cursor: 0,
            high: true,
            overwrite: true,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.doc.bytes() != self.saved.as_slice()
    }

    pub fn mark_saved(&mut self) {
        self.saved = self.doc.bytes().to_vec();
        self.doc.mark_saved();
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Overwrite mode can't sit past the last byte: there is nothing there to
    /// write over.
    fn max_cursor(&self) -> usize {
        if self.overwrite {
            self.doc.len().saturating_sub(1)
        } else {
            self.doc.len()
        }
    }

    fn set_cursor(&mut self, at: usize, cx: &mut Context<Self>) {
        self.cursor = at.min(self.max_cursor());
        self.high = true;
        self.scroll
            .scroll_to_item(self.cursor / ROW, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn move_by(&mut self, delta: isize, cx: &mut Context<Self>) {
        let at = (self.cursor as isize + delta).clamp(0, self.max_cursor() as isize);
        self.set_cursor(at as usize, cx);
    }

    fn touched(&mut self, cx: &mut Context<Self>) {
        cx.emit(HexEvent::Changed);
        cx.notify();
    }

    fn type_digit(&mut self, v: u8, cx: &mut Context<Self>) {
        let at = self.cursor;
        if self.overwrite && at >= self.doc.len() {
            return;
        }
        let old = self.doc.byte(at).unwrap_or(0);
        if self.high {
            if self.overwrite {
                self.doc.overwrite(at, (v << 4) | (old & 0x0f));
            } else {
                self.doc.insert(at, v << 4);
            }
            self.high = false;
        } else {
            self.doc.overwrite(at, (old & 0xf0) | v);
            self.high = true;
            self.cursor = (at + 1).min(self.max_cursor());
            self.scroll
                .scroll_to_item(self.cursor / ROW, ScrollStrategy::Nearest);
        }
        self.touched(cx);
    }

    /// Deleting shortens the file, which overwrite mode exists to prevent.
    fn delete(&mut self, backwards: bool, cx: &mut Context<Self>) {
        if self.overwrite {
            return;
        }
        let at = if backwards {
            match self.cursor.checked_sub(1) {
                Some(at) => at,
                None => return,
            }
        } else {
            self.cursor
        };
        if self.doc.delete(at) {
            self.cursor = at.min(self.max_cursor());
            self.high = true;
            self.touched(cx);
        }
    }

    fn toggle_overwrite(&mut self, _: &ToggleHexOverwrite, _: &mut Window, cx: &mut Context<Self>) {
        self.set_overwrite(!self.overwrite, cx);
    }

    pub fn set_overwrite(&mut self, on: bool, cx: &mut Context<Self>) {
        self.overwrite = on;
        self.cursor = self.cursor.min(self.max_cursor());
        self.high = true;
        cx.emit(HexEvent::Changed);
        cx.notify();
    }

    fn undo(&mut self, _: &HexUndo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(at) = self.doc.undo() {
            self.set_cursor(at, cx);
            self.touched(cx);
        }
    }

    fn redo(&mut self, _: &HexRedo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(at) = self.doc.redo() {
            self.set_cursor(at, cx);
            self.touched(cx);
        }
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let m = &ev.keystroke.modifiers;
        if m.control || m.platform || m.alt {
            return; // shortcuts belong to the window
        }
        let page = 16 * ROW as isize;
        match ev.keystroke.key.as_str() {
            "right" => self.move_by(1, cx),
            "left" => self.move_by(-1, cx),
            "down" => self.move_by(ROW as isize, cx),
            "up" => self.move_by(-(ROW as isize), cx),
            "pagedown" => self.move_by(page, cx),
            "pageup" => self.move_by(-page, cx),
            "home" => self.set_cursor(0, cx),
            "end" => self.set_cursor(self.doc.len(), cx),
            "backspace" => self.delete(true, cx),
            "delete" => self.delete(false, cx),
            key => match hex::hex_digit(key) {
                Some(v) => self.type_digit(v, cx),
                None => return,
            },
        }
        cx.stop_propagation();
    }

    fn rows(
        &mut self,
        range: std::ops::Range<usize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (fg, muted, cursor_bg, nibble_bg) = (
            theme.foreground,
            theme.muted_foreground,
            theme.primary.opacity(0.28),
            theme.primary.opacity(0.55),
        );
        let focused_cursor = self.cursor;
        let high = self.high;
        let bytes = self.doc.bytes();
        range
            .map(|row| {
                let start = row * ROW;
                let mut hexes = div().flex().gap(px(6.));
                let mut ascii = String::with_capacity(ROW);
                for col in 0..ROW {
                    let at = start + col;
                    let cell = match bytes.get(at) {
                        Some(b) => {
                            ascii.push(hex::ascii_byte(*b));
                            hex::hex_byte(*b)
                        }
                        None => "  ".to_string(),
                    };
                    let here = at == focused_cursor;
                    hexes = hexes.child(
                        div()
                            .id(("byte", at))
                            .px(px(1.))
                            .rounded(px(2.))
                            .when(col == 8, |d| d.ml(px(8.)))
                            .when(here, |d| d.bg(if high { cursor_bg } else { nibble_bg }))
                            .child(cell)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    this.focus.focus(window, cx);
                                    this.set_cursor(at, cx);
                                }),
                            ),
                    );
                }
                div()
                    .id(("row", row))
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .gap(px(24.))
                    .child(div().text_color(muted).child(hex::offset_label(start)))
                    .child(hexes.text_color(fg))
                    .child(div().text_color(muted).child(ascii))
                    .into_any_element()
            })
            .collect()
    }
}

impl Render for HexView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.doc.rows();
        div()
            .key_context("HexView")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::toggle_overwrite))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .font_family(MONO_FONT)
            .text_size(px(13.))
            .px(px(24.))
            .py(px(12.))
            .child(
                uniform_list("hex-rows", rows, cx.processor(Self::rows))
                    .track_scroll(&self.scroll)
                    .size_full(),
            )
    }
}
