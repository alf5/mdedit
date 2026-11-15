//! Editing a table in the WYSIWYG editor: keyboard navigation, the operations
//! behind the context menu, and the controls that appear on hover.
//!
//! The hover controls live in one `position: fixed` element on `<body>`, never
//! inside the editor. Whatever is in the editor ends up in `innerHTML`, so it
//! would otherwise have to be filtered out of undo snapshots, tab stashes,
//! exports and the serializer.
//!
//! The interaction design comes from ZenNotes (MIT), minus its Vim layer.

use std::cell::RefCell;

use wasm_bindgen::{closure::Closure, JsCast};
use web_sys::{Document, DomRect, Element, HtmlElement, KeyboardEvent, MouseEvent, Node, Window};

use crate::table::{moved, sort_order, Align, MenuFacts};

/// The narrowest a column can be dragged.
const MIN_COL_WIDTH: f64 = 48.0;
/// The width a new column gets in a table whose columns are all pinned.
const NEW_COL_WIDTH: u32 = 120;
/// How far the pointer can stray from a table before its controls go away:
/// (above, left, right, below). These are the gutters the controls sit in.
const HOVER_MARGIN: (f64, f64, f64, f64) = (14.0, 16.0, 9.0, 9.0);
/// How thick the reorder grips and the add rails are.
const GRIP: f64 = 12.0;
const RAIL: f64 = 9.0;

/// What the app does around a change. Undo needs the state from before it, so
/// `commit` runs first and only once the change is certain to happen.
pub struct Hooks {
    pub commit: Box<dyn Fn()>,
    pub changed: Box<dyn Fn()>,
}

#[derive(Clone, Copy)]
enum Drag {
    Row {
        from: usize,
        to: usize,
    },
    Col {
        from: usize,
        to: usize,
    },
    Resize {
        col: usize,
        start_x: f64,
        start_width: f64,
        /// Pinning and the undo step wait for the first movement, so clicking
        /// a border without dragging changes nothing.
        touched: bool,
    },
}

#[derive(Default)]
struct State {
    hooks: Option<Hooks>,
    root: Option<Element>,
    table: Option<Element>,
    drag: Option<Drag>,
    /// The header under the pointer: its reorder grip and resize line show.
    hover_head: Option<usize>,
    /// The body row under the pointer: its reorder grip shows.
    hover_row: Option<usize>,
    /// Holds the focused cell's highlight, clipped to the editor.
    focus_layer: Option<Element>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

fn window() -> Window {
    web_sys::window().expect("window")
}

fn document() -> Document {
    window().document().expect("document")
}

fn commit() {
    STATE.with_borrow(|s| {
        if let Some(h) = &s.hooks {
            (h.commit)();
        }
    });
}

fn changed() {
    STATE.with_borrow(|s| {
        if let Some(h) = &s.hooks {
            (h.changed)();
        }
    });
    refresh();
}

// ---------- reading the table ----------

fn elements(list: &web_sys::NodeList) -> Vec<Element> {
    (0..list.length())
        .filter_map(|i| list.item(i)?.dyn_into::<Element>().ok())
        .collect()
}

fn rows(table: &Element) -> Vec<Element> {
    table
        .query_selector_all("tr")
        .map(|l| elements(&l))
        .unwrap_or_default()
}

fn in_head(row: &Element) -> bool {
    row.closest("thead").ok().flatten().is_some()
}

fn body_rows(table: &Element) -> Vec<Element> {
    rows(table).into_iter().filter(|r| !in_head(r)).collect()
}

fn cells(row: &Element) -> Vec<Element> {
    let list = row.children();
    (0..list.length())
        .filter_map(|i| list.item(i))
        .filter(|e| matches!(e.tag_name().to_uppercase().as_str(), "TD" | "TH"))
        .collect()
}

fn cols(table: &Element) -> Vec<Element> {
    table
        .query_selector_all("colgroup > col")
        .map(|l| elements(&l))
        .unwrap_or_default()
}

/// A cell's table, row index among all rows, and column.
fn locate(cell: &Element) -> Option<(Element, usize, usize)> {
    let row = cell.closest("tr").ok().flatten()?;
    let table = cell.closest("table").ok().flatten()?;
    let r = rows(&table).iter().position(|x| x == &row)?;
    let c = cells(&row).iter().position(|x| x == cell)?;
    Some((table, r, c))
}

fn selection_cell() -> Option<Element> {
    let sel = window().get_selection().ok().flatten()?;
    let node = sel.anchor_node()?;
    let el = node
        .dyn_ref::<Element>()
        .cloned()
        .or_else(|| node.parent_element())?;
    let cell = el.closest("td,th").ok().flatten()?;
    cell.closest(".editor.wysiwyg").ok().flatten()?;
    Some(cell)
}

fn editor_of(el: &Element) -> Option<HtmlElement> {
    el.closest(".editor.wysiwyg")
        .ok()
        .flatten()?
        .dyn_into::<HtmlElement>()
        .ok()
}

// ---------- moving the caret ----------

/// Put the caret at the end of a block, scrolling to it only if it's out of
/// view, so walking along a row doesn't make the page jump.
fn caret_to(el: &Element, at_end: bool) {
    if let Some(editor) = editor_of(el) {
        let _ = editor.focus();
        let (e, r) = (
            editor.get_bounding_client_rect(),
            el.get_bounding_client_rect(),
        );
        if r.top() < e.top() || r.bottom() > e.bottom() {
            el.scroll_into_view_with_bool(false);
        }
    }
    let (Ok(range), Ok(Some(sel))) = (document().create_range(), window().get_selection()) else {
        return;
    };
    let node: &Node = el.unchecked_ref();
    let empty = el.text_content().unwrap_or_default().is_empty();
    if range.select_node_contents(node).is_err() {
        return;
    }
    // An empty cell's only child is its placeholder <br>, and a caret after
    // that sits on a line that doesn't exist.
    range.collapse_with_to_start(!at_end || empty);
    let _ = sel.remove_all_ranges();
    let _ = sel.add_range(&range);
}

/// A row of empty cells carrying the alignment of the row it's modelled on.
fn new_row_like(row: &Element) -> Option<Element> {
    let doc = document();
    let tr = doc.create_element("tr").ok()?;
    let source = cells(row);
    for c in source.iter() {
        let td = doc.create_element("td").ok()?;
        if let Some(style) = c.get_attribute("style") {
            let _ = td.set_attribute("style", &style);
        }
        td.set_inner_html("<br>");
        let _ = tr.append_child(&td);
    }
    if source.is_empty() {
        let td = doc.create_element("td").ok()?;
        td.set_inner_html("<br>");
        let _ = tr.append_child(&td);
    }
    Some(tr)
}

/// Append an empty row and return it. A header-only table has no `<tbody>`
/// yet; a row outside one would parse back in a different place.
fn append_row(table: &Element) -> Option<Element> {
    let last = rows(table).pop()?;
    let tr = new_row_like(&last)?;
    let tbody = match table.query_selector("tbody").ok().flatten() {
        Some(b) => b,
        None => {
            let b = document().create_element("tbody").ok()?;
            table.append_child(&b).ok()?;
            b
        }
    };
    tbody.append_child(&tr).ok()?;
    Some(tr)
}

fn append_col(table: &Element) -> Option<Element> {
    let mut head = None;
    for (i, row) in rows(table).iter().enumerate() {
        let tag = if in_head(row) { "th" } else { "td" };
        let cell = document().create_element(tag).ok()?;
        cell.set_inner_html("<br>");
        row.append_child(&cell).ok()?;
        if i == 0 {
            head = Some(cell);
        }
    }
    // A table with no `<colgroup>` needs nothing: content sizes its columns.
    if let Some(group) = table.query_selector("colgroup").ok().flatten() {
        let col = document().create_element("col").ok()?;
        col.set_attribute("style", &format!("width: {NEW_COL_WIDTH}px"))
            .ok()?;
        group.append_child(&col).ok()?;
    }
    head
}

/// Keep a `<colgroup>` in step after column `at` was removed.
fn col_removed(table: &Element, at: usize) {
    if let Some(col) = cols(table).get(at) {
        col.remove();
    }
}

/// Table navigation. Returns true when it took the key. Shift+Enter isn't
/// handled here: the browser's own line break is what's wanted in a cell.
pub fn handle_key(e: &KeyboardEvent) -> bool {
    if e.ctrl_key() || e.meta_key() || e.alt_key() || e.is_composing() {
        return false;
    }
    let key = e.key();
    if !matches!(
        key.as_str(),
        "Tab" | "Enter" | "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" | "Escape"
    ) || (e.shift_key() && key != "Tab")
    {
        return false;
    }
    let Some(cell) = selection_cell() else {
        return false;
    };
    let Some((table, r, c)) = locate(&cell) else {
        return false;
    };
    let all = rows(&table);
    let cell_at = |r: usize, c: usize| all.get(r).and_then(|row| cells(row).get(c).cloned());
    match key.as_str() {
        "Tab" if e.shift_key() => {
            let prev = if c > 0 {
                cell_at(r, c - 1)
            } else if r > 0 {
                all.get(r - 1).and_then(|row| cells(row).pop())
            } else {
                None
            };
            if let Some(p) = prev {
                caret_to(&p, true);
            }
        }
        "Tab" => {
            let next = cell_at(r, c + 1).or_else(|| cell_at(r + 1, 0));
            match next {
                Some(n) => caret_to(&n, true),
                None => add_row_and_focus(&table, 0),
            }
        }
        "Enter" => match cell_at(r + 1, c) {
            Some(n) => caret_to(&n, true),
            None => add_row_and_focus(&table, c),
        },
        "ArrowDown" => {
            if let Some(n) = cell_at(r + 1, c) {
                caret_to(&n, true);
            }
        }
        "ArrowUp" => {
            if r > 0 {
                if let Some(n) = cell_at(r - 1, c) {
                    caret_to(&n, true);
                }
            }
        }
        // Inside the text the browser moves the caret. At the edge of the cell
        // it would leave, so the key stops there.
        "ArrowLeft" | "ArrowRight" => return caret_at_cell_edge(&cell, key == "ArrowLeft"),
        "Escape" => caret_after(&table),
        _ => return false,
    }
    true
}

/// Is a collapsed caret at the start (or end) of this cell's text?
fn caret_at_cell_edge(cell: &Element, start: bool) -> bool {
    let (Some(sel), Ok(range)) = (
        window().get_selection().ok().flatten(),
        document().create_range(),
    ) else {
        return false;
    };
    if !sel.is_collapsed() || range.select_node_contents(cell.unchecked_ref()).is_err() {
        return false;
    }
    let Some(anchor) = sel.anchor_node() else {
        return false;
    };
    let placed = if start {
        range.set_end(&anchor, sel.anchor_offset())
    } else {
        range.set_start(&anchor, sel.anchor_offset())
    };
    placed.is_ok() && String::from(range.to_string()).is_empty()
}

fn add_row_and_focus(table: &Element, col: usize) {
    commit();
    if let Some(row) = append_row(table) {
        if let Some(c) = cells(&row).get(col) {
            caret_to(c, false);
        }
        changed();
    }
}

/// Leave the table downward, adding a paragraph if there's nowhere to go.
fn caret_after(table: &Element) {
    if let Some(next) = table.next_element_sibling() {
        let editable = next.get_attribute("contenteditable").as_deref() != Some("false");
        if editable && next.tag_name().to_uppercase() != "TABLE" {
            caret_to(&next, false);
            return;
        }
    }
    let Ok(p) = document().create_element("p") else {
        return;
    };
    commit();
    p.set_inner_html("<br>");
    let _ = table.after_with_node_1(&p);
    caret_to(&p, false);
    changed();
}

// ---------- context menu operations ----------

fn tbody_of(table: &Element) -> Option<Element> {
    table.query_selector("tbody").ok().flatten()
}

fn reorder_rows(table: &Element, from: usize, to: usize) {
    let mut list = body_rows(table);
    if from >= list.len() || to >= list.len() || from == to {
        return;
    }
    let Some(tbody) = tbody_of(table) else {
        return;
    };
    commit();
    moved(&mut list, from, to);
    for row in &list {
        let _ = tbody.append_child(row);
    }
    changed();
}

fn reorder_cols(table: &Element, from: usize, to: usize) {
    let count = rows(table).first().map_or(0, |r| cells(r).len());
    if from >= count || to >= count || from == to {
        return;
    }
    commit();
    for row in rows(table) {
        let mut list = cells(&row);
        moved(&mut list, from, to);
        for c in &list {
            let _ = row.append_child(c);
        }
    }
    if let Some(group) = table.query_selector("colgroup").ok().flatten() {
        let mut list = cols(table);
        moved(&mut list, from, to);
        for c in &list {
            let _ = group.append_child(c);
        }
    }
    changed();
}

/// Add a row at the end of the cell's table and put the caret in it. This is
/// the one way in besides the `+` rail; moving it somewhere else is a separate
/// step.
pub fn add_row(cell: &Element) {
    if let Some((table, _, _)) = locate(cell) {
        add_row_and_focus(&table, 0);
    }
}

/// Add a column at the end of the cell's table and put the caret in its header.
pub fn add_col(cell: &Element) {
    if let Some((table, _, _)) = locate(cell) {
        add_col_and_focus(&table);
    }
}

fn add_col_and_focus(table: &Element) {
    commit();
    if let Some(head) = append_col(table) {
        caret_to(&head, false);
    }
    changed();
}

pub fn delete_row(cell: &Element) {
    let Some(row) = cell.closest("tr").ok().flatten() else {
        return;
    };
    commit();
    row.remove();
    changed();
}

pub fn delete_col(cell: &Element) {
    let Some((table, _, col)) = locate(cell) else {
        return;
    };
    commit();
    for row in rows(&table) {
        if let Some(c) = cells(&row).get(col) {
            c.remove();
        }
    }
    col_removed(&table, col);
    changed();
}

pub fn delete_table(cell: &Element) {
    let Some((table, _, _)) = locate(cell) else {
        return;
    };
    commit();
    table.remove();
    changed();
}

/// Move the cell's row up or down one. The header row stays where it is.
pub fn move_row(cell: &Element, delta: i32) {
    let Some(row) = cell.closest("tr").ok().flatten() else {
        return;
    };
    if in_head(&row) {
        return;
    }
    let Some((table, _, _)) = locate(cell) else {
        return;
    };
    let Some(from) = body_rows(&table).iter().position(|r| r == &row) else {
        return;
    };
    if let Some(to) = from.checked_add_signed(delta as isize) {
        reorder_rows(&table, from, to);
    }
}

/// Move the cell's column left or right one.
pub fn move_col(cell: &Element, delta: i32) {
    let Some((table, _, c)) = locate(cell) else {
        return;
    };
    if let Some(to) = c.checked_add_signed(delta as isize) {
        reorder_cols(&table, c, to);
    }
}

pub fn duplicate_row(cell: &Element) {
    let Some(row) = cell.closest("tr").ok().flatten() else {
        return;
    };
    if in_head(&row) {
        return; // a GFM table has exactly one header row
    }
    let Ok(copy) = row.clone_node_with_deep(true) else {
        return;
    };
    commit();
    let _ = row.after_with_node_1(&copy);
    changed();
}

pub fn duplicate_col(cell: &Element) {
    let Some((table, _, c)) = locate(cell) else {
        return;
    };
    commit();
    for row in rows(&table) {
        if let Some(src) = cells(&row).get(c) {
            if let Ok(copy) = src.clone_node_with_deep(true) {
                let _ = src.after_with_node_1(&copy);
            }
        }
    }
    if let Some(src) = cols(&table).get(c) {
        if let Ok(copy) = src.clone_node_with_deep(true) {
            let _ = src.after_with_node_1(&copy);
        }
    }
    changed();
}

pub fn set_align(cell: &Element, align: Align) {
    let Some((table, _, c)) = locate(cell) else {
        return;
    };
    commit();
    for row in rows(&table) {
        let Some(target) = cells(&row).get(c).cloned() else {
            continue;
        };
        let _ = target.remove_attribute("align");
        let Ok(html) = target.dyn_into::<HtmlElement>() else {
            continue;
        };
        let style = html.style();
        let _ = match align {
            Align::None => style.remove_property("text-align").map(|_| ()),
            Align::Left => style.set_property("text-align", "left"),
            Align::Center => style.set_property("text-align", "center"),
            Align::Right => style.set_property("text-align", "right"),
        };
    }
    changed();
}

pub fn sort_by_col(cell: &Element, descending: bool) {
    let Some((table, _, c)) = locate(cell) else {
        return;
    };
    let list = body_rows(&table);
    let values: Vec<String> = list
        .iter()
        .map(|r| {
            cells(r)
                .get(c)
                .and_then(|x| x.text_content())
                .unwrap_or_default()
        })
        .collect();
    let order = sort_order(&values, descending);
    if order.iter().enumerate().all(|(i, &o)| i == o) {
        return; // already in order, so no undo step
    }
    let Some(tbody) = tbody_of(&table) else {
        return;
    };
    commit();
    for i in order {
        let _ = tbody.append_child(&list[i]);
    }
    changed();
}

pub fn copy_markdown(cell: &Element) {
    let Some((table, _, _)) = locate(cell) else {
        return;
    };
    let md = crate::serialize::table_to_md(&table);
    let _ = window().navigator().clipboard().write_text(&md);
}

pub fn duplicate_table(cell: &Element) {
    let Some((table, _, _)) = locate(cell) else {
        return;
    };
    let Ok(copy) = table.clone_node_with_deep(true) else {
        return;
    };
    commit();
    let _ = table.after_with_node_1(&copy);
    changed();
}

/// What the right-click menu needs to know about this cell.
pub fn menu_facts(cell: &Element) -> Option<MenuFacts> {
    let (table, _, col) = locate(cell)?;
    let row = cell.closest("tr").ok().flatten()?;
    let body = body_rows(&table);
    let head = rows(&table).into_iter().next()?;
    let head_cells = cells(&head);
    let top = head_cells.get(col)?;
    let attrs = format!(
        "{} {}",
        top.get_attribute("style").unwrap_or_default(),
        top.get_attribute("align").unwrap_or_default()
    );
    Some(MenuFacts {
        body_row: body.iter().position(|r| r == &row),
        body_rows: body.len(),
        col,
        cols: head_cells.len(),
        align: Align::from_attrs(&attrs),
    })
}

/// The link a left press on a table cell should open, if any. Only addresses
/// the OS can be asked to open count, so a footnote link or a relative path in
/// a cell just places the caret. The backend checks again before opening.
pub fn link_on_press(e: &MouseEvent) -> Option<String> {
    if e.button() != 0 {
        return None;
    }
    let el = e.target()?.dyn_into::<Element>().ok()?;
    el.closest("td,th").ok().flatten()?;
    el.closest(".editor.wysiwyg").ok().flatten()?;
    let href = el.closest("a").ok().flatten()?.get_attribute("href")?;
    let scheme = href.split(':').next()?.to_ascii_lowercase();
    matches!(scheme.as_str(), "http" | "https" | "mailto")
        .then_some(href)
        .filter(|h| h.contains(':'))
}

// ---------- hover controls ----------

/// Set up the controls. Call once.
pub fn install(hooks: Hooks) {
    let doc = document();
    let Ok(root) = doc.create_element("div") else {
        return;
    };
    root.set_class_name("table-chrome hidden");
    let Ok(focus_layer) = doc.create_element("div") else {
        return;
    };
    focus_layer.set_class_name("table-focus hidden");
    if let Some(body) = doc.body() {
        let _ = body.append_child(&focus_layer);
        let _ = body.append_child(&root);
    }

    // One listener on the root rather than one per control: the controls are
    // rebuilt on every scroll, and a closure per rebuild would leak.
    let down = Closure::<dyn FnMut(MouseEvent)>::new(on_mouse_down);
    let _ = root.add_event_listener_with_callback("mousedown", down.as_ref().unchecked_ref());
    down.forget();
    let click = Closure::<dyn FnMut(MouseEvent)>::new(on_click);
    let _ = root.add_event_listener_with_callback("click", click.as_ref().unchecked_ref());
    click.forget();

    let win = window();
    let moved = Closure::<dyn FnMut(MouseEvent)>::new(on_mouse_move);
    let _ = win.add_event_listener_with_callback("mousemove", moved.as_ref().unchecked_ref());
    moved.forget();

    let up = Closure::<dyn FnMut(MouseEvent)>::new(on_mouse_up);
    let _ = win.add_event_listener_with_callback("mouseup", up.as_ref().unchecked_ref());
    up.forget();

    // Capturing, since scroll doesn't bubble and the editor is what scrolls.
    let again = Closure::<dyn FnMut()>::new(refresh);
    let _ = win.add_event_listener_with_callback_and_bool(
        "scroll",
        again.as_ref().unchecked_ref(),
        true,
    );
    let _ = win.add_event_listener_with_callback("resize", again.as_ref().unchecked_ref());
    again.forget();

    STATE.with_borrow_mut(|s| {
        s.hooks = Some(hooks);
        s.root = Some(root);
        s.focus_layer = Some(focus_layer);
    });
}

fn hide() {
    let root = STATE.with_borrow_mut(|s| {
        s.table = None;
        s.drag = None;
        s.hover_head = None;
        s.hover_row = None;
        s.root.clone()
    });
    if let Some(root) = root {
        root.set_inner_html("");
        root.set_class_name("table-chrome hidden");
    }
    set_body_drag(None);
}

/// Redraw the controls for the current table, or take them away if it has
/// gone: undo, a tab switch and a mode change all replace or hide the DOM.
pub fn refresh() {
    sync_focus();
    let (table, dragging) = STATE.with_borrow(|s| (s.table.clone(), s.drag.is_some()));
    if dragging {
        return;
    }
    let Some(table) = table else {
        return;
    };
    let rect = table.get_bounding_client_rect();
    if !table.is_connected() || rect.width() == 0.0 {
        hide();
    } else {
        draw(&table);
    }
}

/// Highlight the cell holding the caret. It's drawn over the cell rather than
/// styled on it: a class on the cell would land in `innerHTML`, and CSS can't
/// tell by itself, since it's the editor that has the focus, never the cell.
pub fn sync_focus() {
    let Some(layer) = STATE.with_borrow(|s| s.focus_layer.clone()) else {
        return;
    };
    let cell = selection_cell().filter(|c| {
        let active = document().active_element();
        editor_of(c).is_some_and(|ed| active.as_ref() == Some(ed.unchecked_ref()))
            || active.is_some_and(|a| a.closest(".ctxmenu").ok().flatten().is_some())
    });
    let shown = cell.and_then(|c| {
        let frame = editor_of(&c)?;
        let f = frame.get_bounding_client_rect();
        (f.width() > 0.0).then_some((c.get_bounding_client_rect(), f, frame.client_width()))
    });
    let Some((r, f, inner_width)) = shown else {
        layer.set_inner_html("");
        layer.set_class_name("table-focus hidden");
        return;
    };
    layer.set_class_name("table-focus");
    let _ = layer.set_attribute(
        "style",
        &format!(
            "left:{}px;top:{}px;width:{inner_width}px;height:{}px",
            f.left(),
            f.top(),
            f.height()
        ),
    );
    let mark = match layer.first_element_child() {
        Some(m) => m,
        None => {
            let Ok(m) = document().create_element("div") else {
                return;
            };
            m.set_class_name("tc-focus");
            let _ = layer.append_child(&m);
            m
        }
    };
    let _ = mark.set_attribute(
        "style",
        &format!(
            "left:{}px;top:{}px;width:{}px;height:{}px",
            r.left() - f.left(),
            r.top() - f.top(),
            r.width(),
            r.height()
        ),
    );
}

fn near(rect: &DomRect, x: f64, y: f64) -> bool {
    let (above, left, right, below) = HOVER_MARGIN;
    x >= rect.left() - left
        && x <= rect.right() + right
        && y >= rect.top() - above
        && y <= rect.bottom() + below
}

/// Which header and body row the pointer is on, for a target inside the
/// controls or the table.
fn hover_from(target: &Element, table: &Element) -> (Option<usize>, Option<usize>) {
    if let Some(control) = target.closest("[data-tc]").ok().flatten() {
        let i = control
            .get_attribute("data-i")
            .and_then(|v| v.parse::<usize>().ok());
        return match control.get_attribute("data-tc").as_deref() {
            // A resize handle belongs to its header, as if it were inside it.
            Some("col" | "size") => (i, None),
            Some("row") => (None, i),
            _ => (None, None),
        };
    }
    let Some(cell) = target.closest("td,th").ok().flatten() else {
        return (None, None);
    };
    let Some(row) = cell.closest("tr").ok().flatten() else {
        return (None, None);
    };
    if in_head(&row) {
        (cells(&row).iter().position(|c| c == &cell), None)
    } else {
        (None, body_rows(table).iter().position(|r| r == &row))
    }
}

fn set_hover(head: Option<usize>, row: Option<usize>) {
    let same = STATE.with_borrow_mut(|s| {
        let same = s.hover_head == head && s.hover_row == row;
        s.hover_head = head;
        s.hover_row = row;
        same
    });
    if !same {
        apply_hover();
    }
}

/// Light the controls for what's under the pointer. Only classes change, so
/// the resize line's fade gets to run.
fn apply_hover() {
    let (root, head, row) = STATE.with_borrow(|s| (s.root.clone(), s.hover_head, s.hover_row));
    let Some(root) = root else {
        return;
    };
    let Ok(list) = root.query_selector_all("[data-tc]") else {
        return;
    };
    for el in elements(&list) {
        let i = el
            .get_attribute("data-i")
            .and_then(|v| v.parse::<usize>().ok());
        let on = match el.get_attribute("data-tc").as_deref() {
            Some("col" | "size") => head.is_some() && i == head,
            Some("row") => row.is_some() && i == row,
            _ => false,
        };
        let _ = el.class_list().toggle_with_force("on", on);
    }
}

fn on_mouse_move(e: MouseEvent) {
    let (drag, current) = STATE.with_borrow(|s| (s.drag, s.table.clone()));
    if let Some(drag) = drag {
        on_drag(drag, &e);
        return;
    }
    // Mid text selection, or mid split-handle drag: don't pop up under it.
    if e.buttons() != 0 {
        return;
    }
    let Some(target) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) else {
        return;
    };
    if target.closest(".table-chrome").ok().flatten().is_some() {
        if let Some(c) = &current {
            let (head, row) = hover_from(&target, c);
            set_hover(head, row);
        }
        return;
    }
    let hovered = target
        .closest(".editor.wysiwyg table")
        .ok()
        .flatten()
        .filter(|_| {
            target
                .closest("[contenteditable=true]")
                .ok()
                .flatten()
                .is_some()
        });
    match (hovered, current) {
        (Some(t), Some(c)) if t == c => {
            let (head, row) = hover_from(&target, &t);
            set_hover(head, row);
        }
        (Some(t), _) => {
            let (head, row) = hover_from(&target, &t);
            STATE.with_borrow_mut(|s| {
                s.table = Some(t.clone());
                s.hover_head = head;
                s.hover_row = row;
            });
            draw(&t);
        }
        (None, Some(c)) => {
            let rect = c.get_bounding_client_rect();
            let over_editor = target.closest(".editor.wysiwyg").ok().flatten().is_some();
            if over_editor && near(&rect, e.client_x().into(), e.client_y().into()) {
                set_hover(None, None);
            } else {
                hide();
            }
        }
        (None, None) => {}
    }
}

fn add_control(
    root: &Element,
    kind: &str,
    index: usize,
    (left, top, width, height): (f64, f64, f64, f64),
    title: &str,
    glyph: &str,
) {
    let Ok(el) = document().create_element("div") else {
        return;
    };
    el.set_class_name(&format!("tc tc-{kind}"));
    let _ = el.set_attribute("data-tc", kind);
    let _ = el.set_attribute("data-i", &index.to_string());
    let _ = el.set_attribute("title", title);
    let _ = el.set_attribute(
        "style",
        &format!("left:{left}px;top:{top}px;width:{width}px;height:{height}px"),
    );
    if !glyph.is_empty() {
        el.set_text_content(Some(glyph));
    }
    let _ = root.append_child(&el);
}

fn draw(table: &Element) {
    let Some(root) = STATE.with_borrow(|s| s.root.clone()) else {
        return;
    };
    let Some(editor) = editor_of(table) else {
        hide();
        return;
    };
    // The root covers the editor exactly and clips, so controls for a table
    // scrolled partly out of view don't draw over the toolbar.
    let frame = editor.get_bounding_client_rect();
    let _ = root.set_attribute(
        "style",
        &format!(
            "left:{}px;top:{}px;width:{}px;height:{}px",
            frame.left(),
            frame.top(),
            editor.client_width(),
            frame.height()
        ),
    );
    let _ = root.class_list().remove_1("hidden");
    root.set_inner_html("");

    let all = rows(table);
    let Some(head) = all.first() else {
        return;
    };
    let head_cells = cells(head);
    let Some(last_head) = head_cells.last() else {
        return;
    };
    let x = |v: f64| v - frame.left();
    let y = |v: f64| v - frame.top();
    let box_ = table.get_bounding_client_rect();
    let head_box = head.get_bounding_client_rect();
    let last_row = all
        .last()
        .map_or(box_.bottom(), |r| r.get_bounding_client_rect().bottom());
    let left = head_box.left().max(box_.left());
    let right = last_head
        .get_bounding_client_rect()
        .right()
        .min(box_.right());
    let top = head_box.top();

    for (i, cell) in head_cells.iter().enumerate() {
        let r = cell.get_bounding_client_rect();
        // A wide table scrolled sideways has columns outside its own box.
        if r.right() < box_.left() || r.left() > box_.right() {
            continue;
        }
        add_control(
            &root,
            "col",
            i,
            (x(r.left()), y(top) - GRIP, r.width(), GRIP),
            "Drag to move this column",
            "",
        );
        add_control(
            &root,
            "size",
            i,
            (x(r.right()) - 3.0, y(top), 7.0, head_box.height()),
            "Drag to resize",
            "",
        );
    }
    for (i, row) in body_rows(table).iter().enumerate() {
        let r = row.get_bounding_client_rect();
        add_control(
            &root,
            "row",
            i,
            (x(left) - GRIP, y(r.top()), GRIP, r.height()),
            "Drag to move this row",
            "",
        );
    }
    add_control(
        &root,
        "add-col",
        0,
        (x(right), y(top), RAIL, last_row - top),
        "Add a column",
        "+",
    );
    add_control(
        &root,
        "add-row",
        0,
        (x(left), y(last_row), right - left, RAIL),
        "Add a row",
        "+",
    );
    apply_hover();
}

fn set_body_drag(class: Option<&str>) {
    if let Some(body) = document().body() {
        let list = body.class_list();
        let _ = list.remove_2("table-dragging", "table-resizing");
        if let Some(c) = class {
            let _ = list.add_1(c);
        }
    }
}

fn on_mouse_down(e: MouseEvent) {
    let Some(control) = e
        .target()
        .and_then(|t| t.dyn_into::<Element>().ok())
        .and_then(|t| t.closest("[data-tc]").ok().flatten())
    else {
        return;
    };
    // The caret stays where it is, and no text selection starts.
    e.prevent_default();
    let (table, root) = STATE.with_borrow(|s| (s.table.clone(), s.root.clone()));
    let (Some(table), Some(root)) = (table, root) else {
        return;
    };
    let kind = control.get_attribute("data-tc").unwrap_or_default();
    let i: usize = control
        .get_attribute("data-i")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let drag = match kind.as_str() {
        "row" => Some(Drag::Row { from: i, to: i }),
        "col" => Some(Drag::Col { from: i, to: i }),
        "size" => {
            let width = rows(&table)
                .first()
                .and_then(|r| cells(r).get(i).cloned())
                .map_or(0.0, |c| c.get_bounding_client_rect().width());
            Some(Drag::Resize {
                col: i,
                start_x: e.client_x().into(),
                start_width: width,
                touched: false,
            })
        }
        _ => None,
    };
    match drag {
        Some(Drag::Resize { .. }) => {
            let _ = root.class_list().add_1("tc-resizing");
            set_body_drag(Some("table-resizing"));
        }
        Some(_) => {
            let _ = root.class_list().add_1("tc-dragging");
            set_body_drag(Some("table-dragging"));
        }
        None => {}
    }
    STATE.with_borrow_mut(|s| s.drag = drag);
}

/// The add rails act on click, so pressing one and moving away cancels.
fn on_click(e: MouseEvent) {
    let Some(control) = e
        .target()
        .and_then(|t| t.dyn_into::<Element>().ok())
        .and_then(|t| t.closest("[data-tc]").ok().flatten())
    else {
        return;
    };
    let Some(table) = STATE.with_borrow(|s| s.table.clone()) else {
        return;
    };
    match control.get_attribute("data-tc").as_deref() {
        Some("add-row") => add_row_and_focus(&table, 0),
        Some("add-col") => add_col_and_focus(&table),
        _ => {}
    }
}

/// Pin every column at its current width, so moving one border doesn't let
/// the others reflow. This is what the width comment then records. Rounded up,
/// because a column a fraction narrower than its text would wrap that text.
fn pin_widths(table: &Element) {
    let Some(head) = rows(table).into_iter().next() else {
        return;
    };
    let measured: Vec<f64> = cells(&head)
        .iter()
        .map(|c| c.get_bounding_client_rect().width())
        .collect();
    let doc = document();
    let group = match table.query_selector("colgroup").ok().flatten() {
        Some(g) => g,
        None => {
            let Ok(g) = doc.create_element("colgroup") else {
                return;
            };
            let _ = table.insert_before(&g, table.first_child().as_ref());
            g
        }
    };
    group.set_inner_html("");
    for w in measured {
        if let Ok(col) = doc.create_element("col") {
            let _ = col.set_attribute("style", &format!("width: {}px", w.ceil()));
            let _ = group.append_child(&col);
        }
    }
}

/// Show where a dragged row or column would land.
fn show_drop(left: f64, top: f64, width: f64, height: f64) {
    let Some(root) = STATE.with_borrow(|s| s.root.clone()) else {
        return;
    };
    let line = match root.query_selector(".tc-drop").ok().flatten() {
        Some(l) => l,
        None => {
            let Ok(l) = document().create_element("div") else {
                return;
            };
            l.set_class_name("tc-drop");
            let _ = root.append_child(&l);
            l
        }
    };
    let f = root.get_bounding_client_rect();
    let _ = line.set_attribute(
        "style",
        &format!(
            "left:{}px;top:{}px;width:{width}px;height:{height}px",
            left - f.left(),
            top - f.top()
        ),
    );
}

/// Over its own slot, a dragged item isn't going anywhere.
fn hide_drop() {
    let line = STATE
        .with_borrow(|s| s.root.clone())
        .and_then(|r| r.query_selector(".tc-drop").ok().flatten());
    if let Some(l) = line {
        l.remove();
    }
}

fn on_drag(drag: Drag, e: &MouseEvent) {
    let Some(table) = STATE.with_borrow(|s| s.table.clone()) else {
        return;
    };
    if !table.is_connected() {
        hide();
        return;
    }
    let (px, py) = (f64::from(e.client_x()), f64::from(e.client_y()));
    let box_ = table.get_bounding_client_rect();
    let next = match drag {
        Drag::Row { from, .. } => {
            let list = body_rows(&table);
            let to = list
                .iter()
                .position(|r| py < r.get_bounding_client_rect().bottom())
                .unwrap_or(list.len().saturating_sub(1));
            match list.get(to).filter(|_| to != from) {
                Some(r) => {
                    let r = r.get_bounding_client_rect();
                    let edge = if to > from { r.bottom() } else { r.top() };
                    show_drop(box_.left(), edge - 1.0, box_.width(), 2.0);
                }
                None => hide_drop(),
            }
            Drag::Row { from, to }
        }
        Drag::Col { from, .. } => {
            let head = rows(&table).first().map(cells).unwrap_or_default();
            let to = head
                .iter()
                .position(|c| px < c.get_bounding_client_rect().right())
                .unwrap_or(head.len().saturating_sub(1));
            match head.get(to).filter(|_| to != from) {
                Some(c) => {
                    let r = c.get_bounding_client_rect();
                    let edge = if to > from { r.right() } else { r.left() };
                    show_drop(edge - 1.0, box_.top(), 2.0, box_.height());
                }
                None => hide_drop(),
            }
            Drag::Col { from, to }
        }
        Drag::Resize {
            col,
            start_x,
            start_width,
            touched,
        } => {
            if !touched {
                commit();
                pin_widths(&table);
            }
            let width = (start_width + px - start_x).max(MIN_COL_WIDTH).round();
            if let Some(c) = cols(&table).get(col) {
                let _ = c.set_attribute("style", &format!("width: {width}px"));
            }
            // The controls follow the column edges as they move.
            draw(&table);
            sync_focus();
            Drag::Resize {
                col,
                start_x,
                start_width,
                touched: true,
            }
        }
    };
    STATE.with_borrow_mut(|s| s.drag = Some(next));
}

fn on_mouse_up(_: MouseEvent) {
    let (drag, table, root) =
        STATE.with_borrow_mut(|s| (s.drag.take(), s.table.clone(), s.root.clone()));
    let Some(drag) = drag else {
        return;
    };
    set_body_drag(None);
    if let Some(root) = root {
        let _ = root.class_list().remove_2("tc-dragging", "tc-resizing");
    }
    hide_drop();
    let Some(table) = table else {
        return;
    };
    match drag {
        Drag::Row { from, to } => reorder_rows(&table, from, to),
        Drag::Col { from, to } => reorder_cols(&table, from, to),
        Drag::Resize { touched, .. } => {
            if touched {
                changed();
            }
        }
    }
    refresh();
}
