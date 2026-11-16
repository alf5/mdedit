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

#[gpui_kit::test]
fn slash_opens_a_block_menu(cx: &mut TestAppContext) {
    let (h, ed) = open(cx, "intro\n\n", Mode::Rendered);
    caret(cx, &ed, 7);
    typed(cx, h, "/");
    assert!(
        ed.read_with(cx, |e, _| e.slash.is_some()),
        "a slash starting a line opens it"
    );
    typed(cx, h, "h2");
    keys(cx, h, &["enter"]);
    assert_eq!(text(cx, &ed), "intro\n\n## ");
    // Mid-sentence it's just a slash.
    typed(cx, h, "a/b");
    assert!(ed.read_with(cx, |e, _| e.slash.is_none()));
    assert_eq!(text(cx, &ed), "intro\n\n## a/b");
}

#[gpui_kit::test]
fn the_slash_menu_walks_and_escapes(cx: &mut TestAppContext) {
    let (h, ed) = open(cx, "", Mode::Rendered);
    typed(cx, h, "/");
    keys(cx, h, &["down", "down", "down"]);
    keys(cx, h, &["enter"]);
    assert_eq!(text(cx, &ed), "- ", "the fourth entry is a bullet list");
    typed(cx, h, "x");
    keys(cx, h, &["enter"]);
    typed(cx, h, "/");
    keys(cx, h, &["escape"]);
    assert!(ed.read_with(cx, |e, _| e.slash.is_none()));
    assert_eq!(text(cx, &ed), "- x\n- /");
}

#[gpui_kit::test]
fn hard_wrapped_paragraphs_flow(cx: &mut TestAppContext) {
    let (h, ed) = open(cx, "one\ntwo\n\nnext", Mode::Rendered);
    let rows = ed.read_with(cx, |e, _| e.rows.len());
    assert_eq!(rows, 3, "the paragraph's two lines are one row");
    caret(cx, &ed, 3);
    keys(cx, h, &["right"]);
    assert_eq!(head(cx, &ed), 4, "across the newline, shown as a space");
    keys(cx, h, &["backspace"]);
    assert_eq!(text(cx, &ed), "onetwo\n\nnext");
    ed.update(cx, |e, cx| e.set_mode(Mode::Source, cx));
    let rows = ed.read_with(cx, |e, _| e.rows.len());
    assert_eq!(rows, 3, "source mode: a row per line");
}

#[gpui_kit::test]
fn tables_open_as_source_and_tab_between_cells(cx: &mut TestAppContext) {
    let src = "| a | b |\n|---|---|\n| 1 | 2 |\n\nafter";
    let (h, ed) = open(cx, src, Mode::Rendered);
    caret(cx, &ed, src.len());
    let closed = ed.read_with(cx, |e, _| e.rows.iter().any(|r| r.widget.is_some()));
    assert!(closed, "with the caret elsewhere the table is a grid");
    // Leaving the table padded it, so find the cell in today's text.
    let one = text(cx, &ed).find('1').unwrap();
    caret(cx, &ed, one);
    let open_now = ed.read_with(cx, |e, _| e.rows.iter().all(|r| r.widget.is_none()));
    assert!(open_now, "the caret in it shows its source");
    keys(cx, h, &["tab"]);
    typed(cx, h, "x");
    keys(cx, h, &["tab"]);
    typed(cx, h, "new");
    keys(cx, h, &["escape"]);
    assert_eq!(
        text(cx, &ed),
        "| a   | b   |\n| --- | --- |\n| 1   | x2  |\n| new |     |\n\nafter"
    );
    // Out of the table, it's a grid again.
    let closed = ed.read_with(cx, |e, _| e.rows.iter().any(|r| r.widget.is_some()));
    assert!(closed);
}

#[gpui_kit::test]
fn table_menu_commands_rewrite_the_table(cx: &mut TestAppContext) {
    let src = "| n |\n|---|\n| 2 |\n| 1 |";
    let (_, ed) = open(cx, src, Mode::Rendered);
    ed.update(cx, |e, cx| {
        e.table_op(0, super::widgets::TableOp::Sort(false), 1, 0, cx)
    });
    assert_eq!(text(cx, &ed), "| n   |\n| --- |\n| 1   |\n| 2   |");
    ed.update(cx, |e, cx| {
        e.table_op(0, super::widgets::TableOp::ColRight, 1, 0, cx)
    });
    assert!(text(cx, &ed).starts_with("| n   | Column 2 |"));
}
