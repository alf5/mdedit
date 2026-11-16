//! The editor driven headlessly: real keystrokes, real layout.

use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds,
    WindowOptions, px, size,
};
use mdedit_core::buffer::Selection;
use mdedit_core::prefs::{Mode, Prefs};
use mdedit_core::themes;

use super::MarkdownEditor;
use super::style::DocStyle;

fn open(
    cx: &mut TestAppContext,
    text: &str,
    mode: Mode,
) -> (AnyWindowHandle, Entity<MarkdownEditor>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::register_fonts(cx);
        crate::actions::bind_keys(cx);
    });
    let text = text.to_string();
    let (handle, editor) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(900.), px(600.)),
                })),
                ..Default::default()
            },
            cx,
            move |window, cx| {
                let style = DocStyle::new(&Prefs::default(), &themes::system_palette(false));
                let ed = cx.new(|cx| MarkdownEditor::new(text, mode, style, cx));
                let focus = ed.read(cx).focus.clone();
                focus.focus(window, cx);
                ed
            },
        )
        .expect("open the test window")
    });
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    (handle, editor)
}

fn caret(cx: &mut TestAppContext, ed: &Entity<MarkdownEditor>, at: usize) {
    ed.update(cx, |e, cx| e.set_selection(Selection::caret(at), cx));
}

fn keys(cx: &mut TestAppContext, h: AnyWindowHandle, keys: &[&str]) {
    cx.update_window(h, |_, window, cx| {
        for k in keys {
            window.render_frame(cx);
            window.press(k, cx);
        }
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn typed(cx: &mut TestAppContext, h: AnyWindowHandle, s: &str) {
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        window.input(s, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn text(cx: &mut TestAppContext, ed: &Entity<MarkdownEditor>) -> String {
    ed.read_with(cx, |e, _| e.text().to_string())
}

fn head(cx: &mut TestAppContext, ed: &Entity<MarkdownEditor>) -> usize {
    ed.read_with(cx, |e, _| e.selection().head)
}

#[gpui_kit::test]
fn typing_and_undo(cx: &mut TestAppContext) {
    let (h, ed) = open(cx, "", Mode::Rendered);
    typed(cx, h, "Hello");
    typed(cx, h, " world");
    assert_eq!(text(cx, &ed), "Hello world");
    keys(cx, h, &["ctrl-z"]);
    assert_eq!(text(cx, &ed), "", "one typing burst is one undo step");
    keys(cx, h, &["ctrl-shift-z"]);
    assert_eq!(text(cx, &ed), "Hello world");
}

#[gpui_kit::test]
fn arrows_step_over_hidden_markup(cx: &mut TestAppContext) {
    let (h, ed) = open(cx, "a **b** c", Mode::Rendered);
    caret(cx, &ed, 1);
    keys(cx, h, &["right"]);
    // Past the space, into the bold: the hidden `**` take no keystroke.
    assert_eq!(head(cx, &ed), 4);
    keys(cx, h, &["right"]);
    assert_eq!(head(cx, &ed), 7, "after `b`, before the closing `**`");
    keys(cx, h, &["right"]);
    assert_eq!(head(cx, &ed), 8);
    keys(cx, h, &["left", "left"]);
    assert_eq!(head(cx, &ed), 4);
}

#[gpui_kit::test]
fn source_mode_steps_through_every_character(cx: &mut TestAppContext) {
    let (h, ed) = open(cx, "a **b** c", Mode::Source);
    caret(cx, &ed, 1);
    keys(cx, h, &["right", "right"]);
    assert_eq!(head(cx, &ed), 3);
}

#[gpui_kit::test]
fn enter_makes_paragraphs_and_continues_lists(cx: &mut TestAppContext) {
    let (h, ed) = open(cx, "- one", Mode::Rendered);
    caret(cx, &ed, 5);
    keys(cx, h, &["enter"]);
    typed(cx, h, "two");
    assert_eq!(text(cx, &ed), "- one\n- two");
    keys(cx, h, &["enter", "enter"]);
    typed(cx, h, "para");
    keys(cx, h, &["enter"]);
    typed(cx, h, "next");
    assert_eq!(text(cx, &ed), "- one\n- two\npara\n\nnext");
}

#[gpui_kit::test]
fn backspace_unwraps_a_bullet_then_joins(cx: &mut TestAppContext) {
    let (h, ed) = open(cx, "intro\n- item", Mode::Rendered);
    caret(cx, &ed, 8);
    keys(cx, h, &["backspace"]);
    assert_eq!(text(cx, &ed), "intro\nitem");
    keys(cx, h, &["backspace"]);
    assert_eq!(text(cx, &ed), "introitem");
}

#[gpui_kit::test]
fn backspace_after_hidden_markup_deletes_the_visible_character(cx: &mut TestAppContext) {
    let (h, ed) = open(cx, "**bold** x", Mode::Rendered);
    caret(cx, &ed, 8);
    keys(cx, h, &["backspace"]);
    assert_eq!(text(cx, &ed), "**bol** x");
}

#[gpui_kit::test]
fn formatting_shortcuts(cx: &mut TestAppContext) {
    let (h, ed) = open(cx, "word", Mode::Rendered);
    ed.update(cx, |e, cx| e.set_selection(Selection::new(0, 4), cx));
    keys(cx, h, &["ctrl-b"]);
    assert_eq!(text(cx, &ed), "**word**");
    keys(cx, h, &["ctrl-2"]);
    assert_eq!(text(cx, &ed), "## **word**");
    keys(cx, h, &["ctrl-0"]);
    assert_eq!(text(cx, &ed), "**word**");
}

#[gpui_kit::test]
fn up_and_down_keep_their_column(cx: &mut TestAppContext) {
    let (h, ed) = open(cx, "abcdef\nab\nabcdef", Mode::Source);
    caret(cx, &ed, 5);
    keys(cx, h, &["down"]);
    assert_eq!(head(cx, &ed), 9, "clamped to the short line's end");
    keys(cx, h, &["down"]);
    assert_eq!(head(cx, &ed), 15, "back at column 5");
    keys(cx, h, &["up", "up"]);
    assert_eq!(head(cx, &ed), 5);
}

#[gpui_kit::test]
fn find_and_replace_all(cx: &mut TestAppContext) {
    let (_, ed) = open(cx, "cat hat cat", Mode::Rendered);
    ed.update(cx, |e, cx| {
        e.set_find("cat".into(), false, cx);
        e.replace_all("dog", cx);
    });
    assert_eq!(text(cx, &ed), "dog hat dog");
}

#[gpui_kit::test]
fn clicking_a_task_box_ticks_it(cx: &mut TestAppContext) {
    let (h, ed) = open(cx, "- [ ] todo", Mode::Rendered);
    // The box sits in the margin before the text.
    let at = ed.read_with(cx, |e, _| {
        let (_, origin, l) = e.geometry.lines[0].clone();
        origin + gpui_kit::point(l.indent - px(14.), l.pad_top + l.line_height / 2.)
    });
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        window.pointer_down(at, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(at, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(text(cx, &ed), "- [x] todo");
}
