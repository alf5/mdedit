//! The editor driven headlessly: real keystrokes, real layout.

use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds,
    WindowOptions, px, size,
};
use mdedit_core::buffer::Selection;
use mdedit_core::display::RowWidget;
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
fn tables_are_edited_in_their_grid(cx: &mut TestAppContext) {
    let src = "| a | b |\n|---|---|\n| 1 | 2 |\n\nafter";
    let (h, ed) = open(cx, src, Mode::Rendered);
    let grid = |cx: &mut TestAppContext| {
        ed.read_with(cx, |e, _| {
            e.rows
                .iter()
                .any(|r| matches!(r.widget, Some(RowWidget::Closed(_))))
                && e.geometry.lines.iter().any(|(_, _, l)| l.grid.is_some())
        })
    };
    let one = src.find('1').unwrap();
    caret(cx, &ed, one + 1);
    typed(cx, h, "x");
    assert!(text(cx, &ed).contains("| 1x | 2 |"));
    assert!(grid(cx), "the caret in it, the table is still a grid");
    keys(cx, h, &["tab"]);
    typed(cx, h, "y");
    keys(cx, h, &["tab"]);
    typed(cx, h, "new");
    keys(cx, h, &["escape"]);
    assert_eq!(
        text(cx, &ed),
        "| a   | b   |\n| --- | --- |\n| 1x  | 2y  |\n| new |     |\n\nafter"
    );
    assert!(grid(cx));
}

#[gpui_kit::test]
fn cells_keep_their_place(cx: &mut TestAppContext) {
    let src = "| ab | cd |\n|----|----|\n| ef | gh |";
    let (h, ed) = open(cx, src, Mode::Rendered);
    let cd = src.find("cd").unwrap();
    caret(cx, &ed, cd);
    keys(cx, h, &["backspace", "left"]);
    assert_eq!(
        (text(cx, &ed).as_str(), head(cx, &ed)),
        (src, cd),
        "the cell's start holds"
    );
    caret(cx, &ed, cd - 3);
    keys(cx, h, &["delete", "right"]);
    assert_eq!(
        (text(cx, &ed).as_str(), head(cx, &ed)),
        (src, cd - 3),
        "and its end"
    );
    // A caret put on a pipe lands in a cell.
    caret(cx, &ed, cd - 2);
    assert_eq!(head(cx, &ed), cd);
    // Deleting across cells empties them but keeps the table.
    let gh = src.find("gh").unwrap();
    ed.update(cx, |e, cx| e.set_selection(Selection::new(3, gh + 1), cx));
    keys(cx, h, &["backspace"]);
    assert_eq!(text(cx, &ed), "| a |  |\n|----|----|\n|  | h |");
    // A typed pipe is a character, not a new column.
    caret(cx, &ed, 3);
    typed(cx, h, "|");
    assert!(text(cx, &ed).starts_with("| a\\| |"), "{}", text(cx, &ed));
}

#[gpui_kit::test]
fn up_and_down_move_between_rows(cx: &mut TestAppContext) {
    let src = "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n\nend";
    let (h, ed) = open(cx, src, Mode::Rendered);
    caret(cx, &ed, src.find('b').unwrap() + 1);
    keys(cx, h, &["down"]);
    let two = src.find('2').unwrap();
    assert!(
        (two..=two + 1).contains(&head(cx, &ed)),
        "into the cell below"
    );
    keys(cx, h, &["down"]);
    let four = src.find('4').unwrap();
    assert!((four..=four + 1).contains(&head(cx, &ed)));
    keys(cx, h, &["down"]);
    assert!(
        head(cx, &ed) > src.find("|\n\n").unwrap(),
        "and out of the table"
    );
    keys(cx, h, &["up"]);
    // Leaving padded the table, so find the cell again.
    let four = text(cx, &ed).find('4').unwrap();
    assert!(
        (four..=four + 1).contains(&head(cx, &ed)),
        "back into its last row"
    );
}

#[gpui_kit::test]
fn table_menu_commands_rewrite_the_table(cx: &mut TestAppContext) {
    use super::table::TableOp;
    let src = "| n |\n|---|\n| 2 |\n| 1 |";
    let (_, ed) = open(cx, src, Mode::Rendered);
    ed.update(cx, |e, cx| e.table_op(0, TableOp::Sort(false), 1, 0, cx));
    assert_eq!(text(cx, &ed), "| n   |\n| --- |\n| 1   |\n| 2   |");
    ed.update(cx, |e, cx| e.table_op(0, TableOp::AddCol, 1, 0, cx));
    assert!(text(cx, &ed).starts_with("| n   | Column 2 |"));
    ed.update(cx, |e, cx| e.table_op(0, TableOp::MoveCol(1, 0), 0, 1, cx));
    assert!(text(cx, &ed).starts_with("| Column 2 | n   |"));
    ed.update(cx, |e, cx| e.table_op(0, TableOp::DuplicateRow, 1, 0, cx));
    assert_eq!(text(cx, &ed).lines().count(), 5);
}

/// Where the first grid is, and its column and row edges.
fn grid_geometry(
    cx: &mut TestAppContext,
    ed: &Entity<MarkdownEditor>,
) -> (
    Point<gpui_kit::Pixels>,
    Vec<gpui_kit::Pixels>,
    Vec<gpui_kit::Pixels>,
) {
    ed.read_with(cx, |e, _| {
        let (_, origin, l) = e
            .geometry
            .lines
            .iter()
            .find(|(_, _, l)| l.grid.is_some())
            .unwrap();
        let g = l.grid.as_ref().unwrap();
        (*origin, g.col_x.clone(), g.row_y.clone())
    })
}

fn drag(
    cx: &mut TestAppContext,
    h: AnyWindowHandle,
    from: Point<gpui_kit::Pixels>,
    to: Point<gpui_kit::Pixels>,
) {
    use gpui_kit::MouseButton::Left;
    cx.update_window(h, |_, window, cx| {
        window.pointer_move(from, None, cx);
        window.pointer_down(from, Left, cx);
        window.pointer_move(to, Some(Left), cx);
        window.pointer_up(to, Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn dragging_a_header_border_resizes_and_saves_widths(cx: &mut TestAppContext) {
    let (h, ed) = open(cx, "| a | b |\n|---|---|\n| 1 | 2 |", Mode::Rendered);
    let (o, cols, rows) = grid_geometry(cx, &ed);
    let border = o + gpui_kit::point(cols[1], (rows[0] + rows[1]) / 2.);
    let before = f32::from(cols[1] - cols[0]).ceil() as u32;
    drag(cx, h, border, border + gpui_kit::point(px(40.), px(0.)));
    let t = text(cx, &ed);
    let saved = t.lines().last().unwrap().to_string();
    assert!(saved.starts_with("<!-- zen:cols="), "{t}");
    let first: u32 = saved["<!-- zen:cols=".len()..]
        .split(',')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(first, before + 40, "{t}");
}

#[gpui_kit::test]
fn grips_reorder_rows_and_the_rail_adds_one(cx: &mut TestAppContext) {
    let (h, ed) = open(cx, "| h |\n|---|\n| 1 |\n| 2 |", Mode::Rendered);
    let (o, cols, rows) = grid_geometry(cx, &ed);
    let grip = super::table::GRIP;
    let first = o + gpui_kit::point(cols[0] - px(grip / 2.), (rows[1] + rows[2]) / 2.);
    let second = o + gpui_kit::point(cols[0] - px(grip / 2.), (rows[2] + rows[3]) / 2.);
    drag(cx, h, first, second);
    assert_eq!(text(cx, &ed), "| h   |\n| --- |\n| 2   |\n| 1   |");
    let (o, cols, rows) = grid_geometry(cx, &ed);
    let rail = o + gpui_kit::point(
        (cols[0] + cols[1]) / 2.,
        *rows.last().unwrap() + px(super::table::RAIL / 2.),
    );
    drag(cx, h, rail, rail);
    assert_eq!(text(cx, &ed).lines().count(), 5, "a row was added");
}

#[gpui_kit::test]
fn a_right_click_on_a_cell_opens_the_table_menu(cx: &mut TestAppContext) {
    let (h, ed) = open(
        cx,
        "| a | b |\n|---|---|\n| 1 | 2 |\n\nafter",
        Mode::Rendered,
    );
    let (o, cols, rows) = grid_geometry(cx, &ed);
    let cell = o + gpui_kit::point((cols[1] + cols[2]) / 2., (rows[1] + rows[2]) / 2.);
    cx.update_window(h, |_, window, cx| {
        window.pointer_down(cell, gpui_kit::MouseButton::Right, cx);
        window.pointer_up(cell, gpui_kit::MouseButton::Right, cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(ed.read_with(cx, |e, _| e.menu.is_some()));
    let two = text(cx, &ed).find('2').unwrap();
    assert_eq!(head(cx, &ed), two + 1, "the caret went into the cell");
    // Elsewhere, no menu of ours.
    ed.update(cx, |e, _| e.menu = None);
    let after = o + gpui_kit::point(px(4.), *rows.last().unwrap() + px(40.));
    cx.update_window(h, |_, window, cx| {
        window.pointer_down(after, gpui_kit::MouseButton::Right, cx);
    })
    .unwrap();
    assert!(ed.read_with(cx, |e, _| e.menu.is_none()));
}

#[gpui_kit::test]
fn live_mode_shows_the_caret_cells_markdown(cx: &mut TestAppContext) {
    let src = "| **a** | *b* |\n|---|---|\n| 1 | 2 |";
    let (h, ed) = open(cx, src, Mode::Live);
    caret(cx, &ed, src.find('a').unwrap());
    keys(cx, h, &[]);
    let shown = ed.read_with(cx, |e, _| {
        let (_, _, l) = e
            .geometry
            .lines
            .iter()
            .find(|(_, _, l)| l.grid.is_some())
            .unwrap();
        let g = l.grid.as_ref().unwrap();
        (
            g.cells[0].display.text.clone(),
            g.cells[1].display.text.clone(),
        )
    });
    assert_eq!(shown, ("**a**".to_string(), "b".to_string()));
}

#[gpui_kit::test]
fn shift_enter_breaks_a_line_in_a_cell(cx: &mut TestAppContext) {
    let src = "| a | b |\n|---|---|\n| 1 | 2 |";
    let (h, ed) = open(cx, src, Mode::Rendered);
    caret(cx, &ed, src.find('1').unwrap() + 1);
    keys(cx, h, &["shift-enter"]);
    typed(cx, h, "two");
    assert!(
        text(cx, &ed).contains("| 1<br>two | 2 |"),
        "{}",
        text(cx, &ed)
    );
    // Drawn as two lines in one cell, the row grown to hold them.
    let lines = ed.read_with(cx, |e, _| {
        let (_, _, l) = e
            .geometry
            .lines
            .iter()
            .find(|(_, _, l)| l.grid.is_some())
            .unwrap();
        let cell = &l.grid.as_ref().unwrap().cells[2];
        (
            cell.display.text.clone(),
            cell.lines.len(),
            cell.height > l.grid.as_ref().unwrap().line_height,
        )
    });
    assert_eq!(lines, ("1\ntwo".to_string(), 2, true));
    // Up stays in the cell, on its first line; End is that line's end.
    keys(cx, h, &["up", "end"]);
    let one = text(cx, &ed).find('1').unwrap();
    assert_eq!(head(cx, &ed), one + 1, "before the break");
    // Delete there takes the whole tag.
    keys(cx, h, &["delete"]);
    assert!(text(cx, &ed).contains("| 1two | 2 |"), "{}", text(cx, &ed));
    // Outside a table it's still a hard line break.
    let (h, ed) = open(cx, "para", Mode::Rendered);
    caret(cx, &ed, 4);
    keys(cx, h, &["shift-enter"]);
    assert!(!text(cx, &ed).contains("<br>"));
}

#[gpui_kit::test]
fn frontmatter_callouts_and_code_sit_on_cards(cx: &mut TestAppContext) {
    use super::Card;
    use mdedit_core::syntax::Alert;
    let doc = "---\ntitle: A\n---\n\n> [!NOTE]\n> body\n\n```rust\nlet x = 1;\n```\n";
    let (_, ed) = open(cx, doc, Mode::Rendered);
    let cards: Vec<_> = ed.read_with(cx, |e, _| {
        e.geometry
            .lines
            .iter()
            .map(|(_, _, l)| {
                (
                    l.card.map(|k| (k.card, k.first, k.last)),
                    l.display.text.clone(),
                )
            })
            .collect()
    });
    // The frontmatter: its fences hidden, as the card's top and bottom.
    assert_eq!(cards[0], (Some((Card::Meta, true, false)), String::new()));
    assert_eq!(cards[1].0, Some((Card::Meta, false, false)));
    assert_eq!(cards[2], (Some((Card::Meta, false, true)), String::new()));
    assert_eq!(cards[3].0, None);
    // The callout: its title, then its body.
    assert_eq!(
        cards[4],
        (
            Some((Card::Callout(Alert::Note), true, false)),
            "Note".into()
        )
    );
    assert_eq!(cards[5].0, Some((Card::Callout(Alert::Note), false, true)));
    // The code block: its language as the header.
    assert_eq!(cards[7], (Some((Card::Code, true, false)), "rust".into()));
    assert_eq!(cards[9].0, Some((Card::Code, false, true)));
}

#[gpui_kit::test]
fn the_copy_button_copies_its_code_block(cx: &mut TestAppContext) {
    let doc = "Text\n\n```sh\necho one\necho two\n```\n";
    let (h, ed) = open(cx, doc, Mode::Rendered);
    let at = ed.read_with(cx, |e, _| {
        let (_, origin, l) = e
            .geometry
            .lines
            .iter()
            .find(|(_, _, l)| l.kind == mdedit_core::syntax::LineKind::FenceOpen)
            .cloned()
            .unwrap();
        let w = e.content_w.unwrap();
        gpui_kit::point(origin.x + w - px(18.), origin.y + l.height.get() / 2.)
    });
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        window.pointer_move(at, None, cx);
        window.render_frame(cx);
        window.pointer_down(at, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(at, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    let copied = cx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(copied.as_deref(), Some("echo one\necho two"));
    // The document is as it was, and the caret didn't move into the block.
    assert_eq!(text(cx, &ed), doc);
}

#[gpui_kit::test]
fn wiki_links_lead_to_their_notes(cx: &mut TestAppContext) {
    use super::EditorEvent;
    use std::cell::RefCell;
    use std::rc::Rc;
    let dir = std::env::temp_dir().join(format!("mdedit-wikilinks-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Linked Note.md"), "# Linked\n").unwrap();
    let doc = "See [[Linked Note|it]] and [[Nowhere]].\n\n## Later\n\n[[#later]]\n";
    let (h, ed) = open(cx, doc, Mode::Rendered);
    type Followed = Vec<(String, Option<std::path::PathBuf>)>;
    let seen: Rc<RefCell<Followed>> = Rc::default();
    let log = seen.clone();
    cx.update(|cx| {
        cx.subscribe(&ed, move |_, ev: &EditorEvent, _| {
            if let EditorEvent::FollowWiki { note, path } = ev {
                log.borrow_mut().push((note.clone(), path.clone()));
            }
        })
        .detach()
    });
    let dir2 = dir.clone();
    ed.update(cx, |e, cx| e.set_base_dir(Some(dir2), cx));
    assert!(ed.update(cx, |e, _| e.wiki_reaches("Linked Note")));
    assert!(!ed.update(cx, |e, _| e.wiki_reaches("Nowhere")));
    assert!(ed.update(cx, |e, _| e.wiki_reaches("#Later")));
    cx.update_window(h, |_, window, cx| {
        ed.update(cx, |e, cx| {
            e.follow_wiki("Linked Note", window, cx);
            e.follow_wiki("Nowhere", window, cx);
            // A heading here: the caret goes to it.
            e.follow_wiki("#later", window, cx);
        })
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        *seen.borrow(),
        [
            ("Linked Note".to_string(), Some(dir.join("Linked Note.md"))),
            ("Nowhere".to_string(), None)
        ]
    );
    let at = ed.read_with(cx, |e, _| e.selection().head);
    assert_eq!(at, doc.find("## Later").unwrap());
    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui_kit::test]
fn the_language_menu_rewrites_the_fence(cx: &mut TestAppContext) {
    let cases = [
        (
            "```rust\nfn x() {}\n```\n",
            "python",
            "```python\nfn x() {}\n```\n",
        ),
        (
            "```rust title=a.rs\nx\n```\n",
            "go",
            "```go title=a.rs\nx\n```\n",
        ),
        ("```\nx\n```\n", "toml", "```toml\nx\n```\n"),
        ("```rust\nx\n```\n", "", "```\nx\n```\n"),
        (
            "```rust title=a.rs\nx\n```\n",
            "",
            "```title=a.rs\nx\n```\n",
        ),
    ];
    for (doc, word, want) in cases {
        let (_, ed) = open(cx, doc, Mode::Rendered);
        let current = ed.read_with(cx, |e, _| e.fence_lang(0));
        assert!(current.is_some(), "{doc}");
        ed.update(cx, |e, cx| e.set_fence_lang(0, word, cx));
        assert_eq!(text(cx, &ed), want, "{doc} → {word}");
    }
    assert!(super::highlight::same_language("rs", "rust"));
    assert!(super::highlight::same_language("", ""));
    assert!(!super::highlight::same_language("rust", "python"));
}

#[gpui_kit::test]
fn inline_formulas_are_pictures_until_the_caret_goes_in(cx: &mut TestAppContext) {
    let doc = "Inline: $e^{i\\pi} + 1 = 0$ done\n";
    let (h, ed) = open(cx, doc, Mode::Rendered);
    let row = |cx: &mut TestAppContext| {
        ed.read_with(cx, |e, _| {
            let (_, _, l) = e.geometry.lines[0].clone();
            (l.display.text.clone(), l.maths.len())
        })
    };
    let (shown, pictures) = row(cx);
    assert_eq!(pictures, 1);
    assert!(!shown.contains('$') && shown.starts_with("Inline: ") && shown.ends_with(" done"));
    // Right from just before it: into its source, which shows.
    caret(cx, &ed, 8);
    keys(cx, h, &["right"]);
    assert_eq!(ed.read_with(cx, |e, _| e.selection().head), 9);
    let (shown, pictures) = row(cx);
    assert_eq!(
        (shown.as_str(), pictures),
        ("Inline: $e^{i\\pi} + 1 = 0$ done", 0)
    );
    // Out the other side: a picture again.
    let end = doc.find(" done").unwrap();
    caret(cx, &ed, end - 1);
    keys(cx, h, &["right"]);
    assert_eq!(ed.read_with(cx, |e, _| e.selection().head), end);
    assert_eq!(row(cx).1, 1);
    // Left from just after it: back in, at its end.
    keys(cx, h, &["left"]);
    assert_eq!(ed.read_with(cx, |e, _| e.selection().head), end - 1);
    assert_eq!(text(cx, &ed), doc);
}

#[gpui_kit::test]
fn a_table_cells_formulas_are_pictures_too(cx: &mut TestAppContext) {
    let doc =
        "Before\n\n| Law | Formula |\n| --- | --- |\n| Euler | $e^{i\\pi} + 1 = 0$ |\n\nAfter\n";
    let (h, ed) = open(cx, doc, Mode::Rendered);
    let cell = |cx: &mut TestAppContext| {
        ed.read_with(cx, |e, _| {
            let (_, _, l) = e
                .geometry
                .lines
                .iter()
                .find(|(_, _, l)| l.grid.is_some())
                .cloned()
                .expect("the table's on screen");
            let g = l.grid.as_ref().unwrap();
            let c = g.cells.last().unwrap();
            (
                c.display.text.clone(),
                c.maths.len(),
                g.pictures(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)))
                    .len(),
            )
        })
    };
    let (shown, maths, placed) = cell(cx);
    assert!(!shown.contains('$'), "{shown:?}");
    assert_eq!((maths, placed), (1, 1));
    // Right from just before it: its source opens.
    let at = doc.find('$').unwrap();
    caret(cx, &ed, at);
    keys(cx, h, &["right"]);
    assert_eq!(ed.read_with(cx, |e, _| e.selection().head), at + 1);
    let (shown, maths, _) = cell(cx);
    assert_eq!((shown.as_str(), maths), ("$e^{i\\pi} + 1 = 0$", 0));
    // Out the other side: a picture again.
    let end = doc.rfind('$').unwrap() + 1;
    caret(cx, &ed, end);
    assert_eq!(cell(cx).1, 1);
    assert_eq!(text(cx, &ed), doc);
}

#[gpui_kit::test]
fn ctrl_over_a_link_makes_the_pointer_a_hand(cx: &mut TestAppContext) {
    use gpui_kit::{
        InputEvent as _, Modifiers, ModifiersChangedEvent, MouseMoveEvent, Pixels, point,
    };
    let doc = "Go to [the site](https://example.com) or [[Note]], not [here](notes.md).\n";
    let (h, ed) = open(cx, doc, Mode::Rendered);
    // The middle of the first character of `text` (as drawn).
    let at = |cx: &mut TestAppContext, text: &str| -> Point<Pixels> {
        ed.read_with(cx, |e, _| {
            let (_, origin, l) = e.geometry.lines[0].clone();
            let src = doc.find(text).unwrap();
            let d = l.to_display(src);
            let r = l.rects(origin, d, l.next(d), false, px(800.))[0];
            r.center()
        })
    };
    let ctrl = cx.update(|cx| Modifiers::secondary_key(cx));
    let send = |cx: &mut TestAppContext, p: Point<Pixels>, modifiers: Modifiers| {
        cx.update_window(h, |_, window, cx| {
            window.dispatch_event(
                MouseMoveEvent {
                    position: p,
                    pressed_button: None,
                    modifiers,
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
        })
        .unwrap();
    };
    let modifiers = |cx: &mut TestAppContext, modifiers: Modifiers| {
        cx.update_window(h, |_, window, cx| {
            window.dispatch_event(
                ModifiersChangedEvent {
                    modifiers,
                    capslock: window.capslock(),
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
        })
        .unwrap();
    };
    let hand = |cx: &mut TestAppContext| ed.read_with(cx, |e, _| e.link_hovered());

    let site = at(cx, "site");
    assert!(
        ed.read_with(cx, |e, _| e.link_under(site).is_some()),
        "{site:?}"
    );
    send(cx, site, Modifiers::none());
    assert!(!hand(cx), "no Ctrl: the text cursor");
    modifiers(cx, ctrl);
    assert!(hand(cx), "Ctrl pressed over a web link");
    modifiers(cx, Modifiers::none());
    assert!(!hand(cx), "Ctrl let go");
    let p = at(cx, "Note");
    send(cx, p, ctrl);
    assert!(hand(cx), "a wiki link");
    let p = at(cx, "Go");
    send(cx, p, ctrl);
    assert!(!hand(cx), "plain text");
    // A link to a file isn't followed by a click, so isn't a hand.
    let p = at(cx, "here");
    send(cx, p, ctrl);
    assert!(!hand(cx), "a relative link");
    // Past the end of the line, level with a link: not over it.
    let end = point(site.x + px(700.), site.y);
    send(cx, end, ctrl);
    assert!(!hand(cx), "past the line's end");
    // And a Ctrl/Cmd+click where the hand was follows the link.
    let followed = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = followed.clone();
    cx.update(|cx| {
        cx.subscribe(&ed, move |_, ev: &super::EditorEvent, _| {
            if let super::EditorEvent::FollowWiki { note, .. } = ev {
                log.borrow_mut().push(note.clone());
            }
        })
        .detach()
    });
    let note = at(cx, "Note");
    cx.update_window(h, |_, window, cx| {
        for event in [
            gpui_kit::MouseDownEvent {
                button: gpui_kit::MouseButton::Left,
                position: note,
                modifiers: ctrl,
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            gpui_kit::MouseUpEvent {
                button: gpui_kit::MouseButton::Left,
                position: note,
                modifiers: ctrl,
                click_count: 1,
            }
            .to_platform_input(),
        ] {
            window.dispatch_event(event, cx);
        }
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(*followed.borrow(), ["Note"]);
    assert_eq!(text(cx, &ed), doc);
    // Shown as source, nothing is a link.
    ed.update(cx, |e, cx| e.set_mode(Mode::Source, cx));
    cx.update_window(h, |_, window, cx| window.render_frame(cx))
        .unwrap();
    let p = at(cx, "site");
    send(cx, p, ctrl);
    assert!(!hand(cx), "source mode");
}

#[gpui_kit::test]
fn brackets_pair_in_prose_but_not_in_source(cx: &mut TestAppContext) {
    let (h, ed) = open(cx, "", Mode::Rendered);
    typed(cx, h, "f(");
    assert_eq!(text(cx, &ed), "f()");
    assert_eq!(head(cx, &ed), 2);
    typed(cx, h, "x)");
    assert_eq!(text(cx, &ed), "f(x)", "the closer steps over");
    keys(cx, h, &["end"]);
    typed(cx, h, " [");
    keys(cx, h, &["backspace"]);
    assert_eq!(text(cx, &ed), "f(x) ", "Backspace takes an empty pair out");

    let (h, ed) = open(cx, "", Mode::Source);
    typed(cx, h, "f(");
    assert_eq!(text(cx, &ed), "f(");
}

#[gpui_kit::test]
fn enter_renumbers_a_list_in_one_undo(cx: &mut TestAppContext) {
    let (h, ed) = open(cx, "1. a\n2. b\n3. c", Mode::Rendered);
    caret(cx, &ed, 4);
    keys(cx, h, &["enter"]);
    typed(cx, h, "new");
    assert_eq!(text(cx, &ed), "1. a\n2. new\n3. b\n4. c");
    keys(cx, h, &["ctrl-z", "ctrl-z"]);
    assert_eq!(text(cx, &ed), "1. a\n2. b\n3. c");
}

#[gpui_kit::test]
fn pasting_an_address_over_a_selection_links_it(cx: &mut TestAppContext) {
    let (h, ed) = open(cx, "see the docs", Mode::Rendered);
    ed.update(cx, |e, cx| e.set_selection(Selection::new(8, 12), cx));
    cx.update(|cx| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("https://x.y/z".into()))
    });
    keys(cx, h, &["ctrl-v"]);
    assert_eq!(text(cx, &ed), "see the [docs](https://x.y/z)");
}

fn visible_lines(cx: &mut TestAppContext, ed: &Entity<MarkdownEditor>) -> Vec<usize> {
    ed.read_with(cx, |e, _| e.rows.iter().map(|r| r.lines.start).collect())
}

#[gpui_kit::test]
fn sections_fold_by_key_and_open_for_the_caret(cx: &mut TestAppContext) {
    let doc = "# A\ntext\nmore\n\n# B\nend";
    let (h, ed) = open(cx, doc, Mode::Rendered);
    caret(cx, &ed, 1);
    keys(cx, h, &["ctrl-shift-["]);
    assert_eq!(
        visible_lines(cx, &ed),
        vec![0, 3, 4, 5],
        "lines 1 and 2 are folded"
    );
    assert_eq!(text(cx, &ed), doc);
    // Down steps over the fold.
    keys(cx, h, &["down"]);
    assert!(head(cx, &ed) >= 14, "the caret lands below the fold");
    // Backspace at the row after a fold opens it rather than eat it.
    keys(cx, h, &["ctrl-shift-[", "up"]);
    caret(cx, &ed, 1);
    keys(cx, h, &["ctrl-shift-["]);
    caret(cx, &ed, 14);
    keys(cx, h, &["backspace"]);
    assert_eq!(text(cx, &ed), doc);
    assert_eq!(
        visible_lines(cx, &ed),
        vec![0, 1, 3, 4, 5],
        "Backspace unfolded it"
    );
    // The caret moving into a fold opens it (as Find or the outline do).
    caret(cx, &ed, 1);
    keys(cx, h, &["ctrl-shift-["]);
    caret(cx, &ed, 6);
    assert_eq!(visible_lines(cx, &ed), vec![0, 1, 3, 4, 5]);
    // Fold All, then Unfold All.
    ed.update(cx, |e, cx| e.set_selection(Selection::caret(doc.len()), cx));
    cx.update_window(h, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::FoldAll), cx);
    })
    .unwrap();
    assert_eq!(
        visible_lines(cx, &ed),
        vec![0, 3, 4, 5],
        "B holds the caret, so it stays open"
    );
    cx.update_window(h, |_, window, cx| {
        window.dispatch_action(Box::new(crate::actions::UnfoldAll), cx);
    })
    .unwrap();
    assert_eq!(visible_lines(cx, &ed), vec![0, 1, 3, 4, 5]);
}

#[gpui_kit::test]
fn a_chevron_folds_and_the_dots_unfold(cx: &mut TestAppContext) {
    let (h, ed) = open(cx, "- a\n  - b\n  - c\n- d", Mode::Rendered);
    caret(cx, &ed, 19);
    // The chevron shows when the pointer is on the row.
    let row = ed.read_with(cx, |e, _| {
        let (_, origin, l) = e.geometry.lines[0].clone();
        origin + gpui_kit::point(l.indent + px(20.), l.pad_top + l.line_height / 2.)
    });
    let click = |cx: &mut TestAppContext, at| {
        cx.update_window(h, |_, window, cx| {
            window.render_frame(cx);
            window.pointer_down(at, gpui_kit::MouseButton::Left, cx);
            window.pointer_up(at, gpui_kit::MouseButton::Left, cx);
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
    };
    cx.update_window(h, |_, window, cx| {
        window.pointer_move(row, None, cx);
        window.render_frame(cx);
    })
    .unwrap();
    let chevron = ed.read_with(cx, |e, _| e.fold_buttons.first().map(|(b, _)| b.center()));
    let chevron = chevron.expect("a chevron on the hovered item");
    click(cx, chevron);
    assert_eq!(visible_lines(cx, &ed), vec![0, 3]);
    // The `…` after the folded item opens it again.
    let dots = ed.read_with(cx, |e, _| e.fold_buttons.get(1).map(|(b, _)| b.center()));
    click(cx, dots.expect("the dots after a folded item"));
    assert_eq!(visible_lines(cx, &ed), vec![0, 1, 2, 3]);
    assert_eq!(text(cx, &ed), "- a\n  - b\n  - c\n- d");
}

fn popup(cx: &mut TestAppContext, ed: &Entity<MarkdownEditor>) -> Vec<String> {
    ed.read_with(cx, |e, _| {
        if e.slash.is_some() {
            e.pop_items.iter().map(|i| i.label.clone()).collect()
        } else {
            Vec::new()
        }
    })
}

#[gpui_kit::test]
fn double_brackets_offer_notes_and_their_headings(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("mdedit-complete-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("deep")).unwrap();
    std::fs::write(
        dir.join("Garden plan.md"),
        "# Plan\n\n## Beds\n\n## Watering\n",
    )
    .unwrap();
    std::fs::write(dir.join("deep/Gardening tools.md"), "x").unwrap();
    std::fs::write(dir.join("photo.png"), "x").unwrap();
    let (h, ed) = open(cx, "", Mode::Rendered);
    let d = dir.clone();
    ed.update(cx, |e, cx| e.set_base_dir(Some(d), cx));
    typed(cx, h, "See [");
    typed(cx, h, "[");
    assert_eq!(text(cx, &ed), "See [[]]", "brackets pair");
    assert_eq!(popup(cx, &ed), vec!["Garden plan", "Gardening tools"]);
    typed(cx, h, "tool");
    assert_eq!(popup(cx, &ed), vec!["Gardening tools"]);
    keys(cx, h, &["enter"]);
    assert_eq!(text(cx, &ed), "See [[Gardening tools]]");
    assert_eq!(
        head(cx, &ed),
        text(cx, &ed).len(),
        "the caret is past the link"
    );
    // A heading of another note.
    typed(cx, h, " and [");
    typed(cx, h, "[");
    typed(cx, h, "Garden plan");
    typed(cx, h, "#");
    assert_eq!(popup(cx, &ed), vec!["Beds", "Plan", "Watering"]);
    typed(cx, h, "wat");
    keys(cx, h, &["tab"]);
    assert!(text(cx, &ed).ends_with("[[Garden plan#Watering]]"));
    // Embeds offer other files too; Escape closes the popup.
    typed(cx, h, " ![");
    typed(cx, h, "[");
    typed(cx, h, "pho");
    assert_eq!(popup(cx, &ed), vec!["photo.png"]);
    keys(cx, h, &["escape"]);
    assert!(popup(cx, &ed).is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[gpui_kit::test]
fn a_hash_offers_the_tags_in_use(cx: &mut TestAppContext) {
    let (h, ed) = open(cx, "Notes on #rust and #research.\n\n", Mode::Rendered);
    caret(cx, &ed, 31);
    typed(cx, h, "#");
    assert_eq!(popup(cx, &ed), vec!["rust", "research"]);
    typed(cx, h, "rus");
    keys(cx, h, &["enter"]);
    assert_eq!(text(cx, &ed), "Notes on #rust and #research.\n\n#rust");
    // A heading's `##` or `# ` offers nothing, and neither does a `#`
    // mid-word.
    let (h, ed) = open(cx, "#rust\n\n", Mode::Rendered);
    caret(cx, &ed, 7);
    typed(cx, h, "#");
    assert_eq!(popup(cx, &ed), vec!["rust"]);
    typed(cx, h, "#");
    assert!(popup(cx, &ed).is_empty());
    typed(cx, h, " a#");
    assert!(popup(cx, &ed).is_empty());
}

#[gpui_kit::test]
fn a_pasted_image_is_saved_beside_the_document(cx: &mut TestAppContext) {
    use super::EditorEvent;
    use std::cell::RefCell;
    use std::rc::Rc;
    let dir = std::env::temp_dir().join(format!("mdedit-paste-img-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let image = gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, b"PNGDATA".to_vec());

    // Untitled: nowhere to put it, so it says so.
    let (h, ed) = open(cx, "", Mode::Rendered);
    let notices: Rc<RefCell<Vec<String>>> = Rc::default();
    let log = notices.clone();
    cx.update(|cx| {
        cx.subscribe(&ed, move |_, ev: &EditorEvent, _| {
            if let EditorEvent::Notice(m) = ev {
                log.borrow_mut().push(m.clone());
            }
        })
        .detach()
    });
    cx.update(|cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_image(&image)));
    keys(cx, h, &["ctrl-v"]);
    assert_eq!(text(cx, &ed), "");
    assert_eq!(notices.borrow().len(), 1);

    let (h, ed) = open(cx, "Look:\n\n", Mode::Rendered);
    let d = dir.clone();
    ed.update(cx, |e, cx| e.set_base_dir(Some(d), cx));
    caret(cx, &ed, 7);
    cx.update(|cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_image(&image)));
    keys(cx, h, &["ctrl-v"]);
    let t = text(cx, &ed);
    assert!(
        t.starts_with("Look:\n\n![](assets/pasted-") && t.ends_with(".png)"),
        "{t}"
    );
    let name = &t["Look:\n\n![](".len()..t.len() - 1];
    assert_eq!(std::fs::read(dir.join(name)).unwrap(), b"PNGDATA");

    // Image files (dropped, or copied in a file manager) are linked, and
    // copied in when they live elsewhere.
    let away = std::env::temp_dir().join(format!("mdedit-away-{}.gif", std::process::id()));
    std::fs::write(&away, b"GIF").unwrap();
    keys(cx, h, &["enter"]);
    ed.update(cx, |e, cx| e.insert_images(std::slice::from_ref(&away), cx));
    let name = away.file_name().unwrap().to_string_lossy().to_string();
    assert!(text(cx, &ed).ends_with(&format!("![](assets/{name})")));
    assert!(dir.join("assets").join(&name).exists());
    std::fs::remove_file(&away).ok();
    std::fs::remove_dir_all(&dir).ok();
}

#[gpui_kit::test]
fn an_embed_shows_its_note_and_cannot_change_it(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("mdedit-embed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Other.md"),
        "# Other\n\n## Part\nhello\n## Next\nbye\n",
    )
    .unwrap();
    let (h, ed) = open(
        cx,
        "Before\n\n![[Other#Part]]\n\n![[Missing]]\n\nafter\n",
        Mode::Rendered,
    );
    let d = dir.clone();
    ed.update(cx, |e, cx| e.set_base_dir(Some(d), cx));
    cx.update_window(h, |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.run_until_parked();
    let inner = ed.read_with(cx, |e, _| {
        assert_eq!(e.embeds.len(), 1, "only the note that exists is embedded");
        e.embeds["Other#Part"].editor.clone()
    });
    assert_eq!(
        inner.read_with(cx, |e, _| e.text().to_string()),
        "## Part\nhello"
    );
    assert!(inner.read_with(cx, |e, _| e.content_height()) > px(20.));
    // Read-only: typing into it changes nothing.
    inner.update(cx, |e, cx| {
        e.replace_selection("x", mdedit_core::history::Kind::Typing, cx)
    });
    assert_eq!(
        inner.read_with(cx, |e, _| e.text().to_string()),
        "## Part\nhello"
    );
    // The note changing on disk shows.
    std::fs::write(dir.join("Other.md"), "# Other\n\n## Part\nhello again\n").unwrap();
    // A different modified time, even on a coarse clock.
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    std::fs::File::options()
        .write(true)
        .open(dir.join("Other.md"))
        .unwrap()
        .set_modified(later)
        .unwrap();
    ed.update(cx, |_, cx| cx.notify());
    cx.update_window(h, |_, window, cx| window.render_frame(cx))
        .unwrap();
    assert_eq!(
        inner.read_with(cx, |e, _| e.text().to_string()),
        "## Part\nhello again"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[gpui_kit::test]
fn holding_ctrl_over_a_wiki_link_previews_its_note(cx: &mut TestAppContext) {
    use gpui_kit::{InputEvent as _, Modifiers, MouseMoveEvent, Pixels};
    let dir = std::env::temp_dir().join(format!("mdedit-preview-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Note.md"), "# Note\n\nIts text.\n").unwrap();
    let doc = "See [[Note]] here.\n";
    let (h, ed) = open(cx, doc, Mode::Rendered);
    let d = dir.clone();
    ed.update(cx, |e, cx| e.set_base_dir(Some(d), cx));
    cx.update_window(h, |_, window, cx| window.render_frame(cx))
        .unwrap();
    let at: Point<Pixels> = ed.read_with(cx, |e, _| {
        let (_, origin, l) = e.geometry.lines[0].clone();
        let d = l.to_display(doc.find("Note").unwrap());
        l.rects(origin, d, l.next(d), false, px(800.))[0].center()
    });
    let ctrl = cx.update(|cx| Modifiers::secondary_key(cx));
    let send = |cx: &mut TestAppContext, modifiers: Modifiers| {
        cx.update_window(h, |_, window, cx| {
            window.dispatch_event(
                MouseMoveEvent {
                    position: at,
                    pressed_button: None,
                    modifiers,
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
        })
        .unwrap();
    };
    send(cx, ctrl);
    assert!(ed.read_with(cx, |e, _| e.preview.is_none()), "not at once");
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(600));
    cx.run_until_parked();
    assert_eq!(
        ed.read_with(cx, |e, _| e.preview.as_ref().map(|(t, _)| t.clone())),
        Some("Note".into())
    );
    cx.update_window(h, |_, window, cx| window.render_frame(cx))
        .unwrap();
    assert!(
        ed.read_with(cx, |e, _| e.embeds.contains_key("Note")),
        "drawn"
    );
    // Letting go of Ctrl takes it away.
    send(cx, Modifiers::default());
    assert!(ed.read_with(cx, |e, _| e.preview.is_none()));
    std::fs::remove_dir_all(&dir).ok();
}

#[gpui_kit::test]
fn misspellings_are_underlined_only_when_asked(cx: &mut TestAppContext) {
    cx.update(crate::state::AppState::init_ephemeral);
    let (h, ed) = open(cx, "A wierd word.\n", Mode::Rendered);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(500));
    cx.run_until_parked();
    assert!(
        ed.read_with(cx, |e, _| e.issues.is_empty()),
        "off by default"
    );

    cx.update(|cx| crate::state::AppState::update_prefs(cx, |p| p.spell_check = true));
    ed.update(cx, |e, cx| e.check_spelling(cx));
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(500));
    cx.run_until_parked();
    let found = ed.read_with(cx, |e, _| {
        e.issues
            .iter()
            .map(|i| e.text()[i.range.clone()].to_string())
            .collect::<Vec<_>>()
    });
    assert_eq!(found, vec!["wierd"]);
    // Typing elsewhere keeps the underline under its word.
    caret(cx, &ed, 0);
    typed(cx, h, "So: ");
    let found = ed.read_with(cx, |e, _| {
        e.issues
            .iter()
            .map(|i| e.text()[i.range.clone()].to_string())
            .collect::<Vec<_>>()
    });
    assert_eq!(found, vec!["wierd"]);
    // Ignored here: not drawn.
    ed.update(cx, |e, _| {
        e.ignored_words.insert("wierd".into());
    });
    assert_eq!(ed.read_with(cx, |e, _| e.shown_issues().count()), 0);
}

#[test]
fn line_spacing_scales_every_mode() {
    use mdedit_core::syntax::LineKind;
    let p = themes::system_palette(false);
    let normal = DocStyle::new(&Prefs::default(), &p);
    let relaxed = DocStyle::new(
        &Prefs {
            line_spacing: 1.15,
            ..Prefs::default()
        },
        &p,
    );
    for mode in [Mode::Rendered, Mode::Source] {
        let (_, a) = normal.metrics(LineKind::Paragraph, mode);
        let (_, b) = relaxed.metrics(LineKind::Paragraph, mode);
        assert!((b / a - 1.15).abs() < 0.001, "{mode:?}: {a} → {b}");
    }
}
