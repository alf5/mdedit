//! "Show Markdown at Cursor": the block holding the caret turns back into its
//! markdown source, and turns back into rendered text when the caret leaves.
//!
//! The open block is a `div.md-raw` holding plain text. Nothing else records
//! that it's open. The DOM is the only state, so undo, which restores
//! `innerHTML` wholesale, can bring a block back open and everything still
//! works. The serializer emits a raw block's text verbatim, so saving with a
//! line open gives the same file as saving with it closed.
//!
//! A soft-wrapped paragraph opens as a whole because its lines aren't separate
//! in the DOM. A list item opens only its own text: its marker and any nested
//! lists stay rendered, so the list's structure survives the round trip.
//! Tables never open, and diagrams and display math have their own source
//! editors.

use std::cell::{Cell, RefCell};

use wasm_bindgen::{closure::Closure, JsCast};
use web_sys::{Document, Element, KeyboardEvent, MouseEvent, Node, Selection, Text, Window};

use crate::markdown::to_html;
use crate::serialize::dom_to_markdown;

pub const RAW_CLASS: &str = "md-raw";
const KIND_ATTR: &str = "data-md-raw";
const KIND_BLOCK: &str = "block";
const KIND_ITEM: &str = "item";
/// Private-use characters that mark the caret while text passes through the
/// serializer or the parser. Neither gives them any meaning.
const MARK: char = '\u{E000}';
const MARK_END: char = '\u{E001}';

pub struct Hooks {
    /// The setting is on and the live editor is showing a document, with no
    /// dialog over it.
    pub enabled: Box<dyn Fn() -> bool>,
    pub commit: Box<dyn Fn()>,
    /// The document changed.
    pub changed: Box<dyn Fn()>,
    /// The markup changed but the document didn't: a block opened or closed.
    pub settled: Box<dyn Fn()>,
    /// Open a diagram's or display formula's source editor.
    pub open_source: Box<dyn Fn(&Element)>,
}

thread_local! {
    static HOOKS: RefCell<Option<Hooks>> = const { RefCell::new(None) };
    static MOUSE_DOWN: Cell<bool> = const { Cell::new(false) };
    static COMPOSING: Cell<bool> = const { Cell::new(false) };
}

fn window() -> Window {
    web_sys::window().expect("window")
}

fn document() -> Document {
    window().document().expect("document")
}

fn with_hooks(f: impl FnOnce(&Hooks)) {
    HOOKS.with_borrow(|h| {
        if let Some(h) = h {
            f(h);
        }
    });
}

fn enabled() -> bool {
    HOOKS.with_borrow(|h| h.as_ref().is_some_and(|h| (h.enabled)()))
}

fn commit() {
    with_hooks(|h| (h.commit)());
}

fn changed() {
    with_hooks(|h| (h.changed)());
}

fn settled() {
    with_hooks(|h| (h.settled)());
}

fn editor() -> Option<Element> {
    document().query_selector(".editor.wysiwyg").ok().flatten()
}

fn selection() -> Option<Selection> {
    window().get_selection().ok().flatten()
}

fn element_of(node: &Node) -> Option<Element> {
    node.dyn_ref::<Element>()
        .cloned()
        .or_else(|| node.parent_element())
}

fn utf16(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

fn is_tag(el: &Element, tags: &[&str]) -> bool {
    let tag = el.tag_name().to_uppercase();
    tags.contains(&tag.as_str())
}

const BLOCK_CHILD: &[&str] = &[
    "UL",
    "OL",
    "P",
    "DIV",
    "PRE",
    "TABLE",
    "BLOCKQUOTE",
    "HR",
    "H1",
    "H2",
    "H3",
    "H4",
    "H5",
    "H6",
];

// ---------- the raw block ----------

fn raw_of(node: &Node) -> Option<Element> {
    element_of(node)?
        .closest(&format!(".{RAW_CLASS}"))
        .ok()
        .flatten()
}

fn all_raw() -> Vec<Element> {
    let Some(ed) = editor() else {
        return Vec::new();
    };
    let Ok(list) = ed.query_selector_all(&format!(".{RAW_CLASS}")) else {
        return Vec::new();
    };
    (0..list.length())
        .filter_map(|i| list.item(i)?.dyn_into::<Element>().ok())
        .collect()
}

fn text_of(raw: &Element) -> String {
    raw.text_content()
        .unwrap_or_default()
        .replace('\u{a0}', " ")
}

fn set_text(raw: &Element, text: &str) {
    raw.set_text_content(Some(text));
    keep_caret_room(raw);
}

/// An empty block has no line to put a caret on, and neither does the text
/// after a newline at the very end of a pre-wrapped block. A `<br>` makes the
/// line, and reading the text back with `textContent` ignores it.
fn keep_caret_room(raw: &Element) {
    let text = raw.text_content().unwrap_or_default();
    let has_br = raw.last_element_child().is_some();
    if (text.is_empty() || text.ends_with('\n')) && !has_br {
        if let Ok(br) = document().create_element("br") {
            let _ = raw.append_child(&br);
        }
    }
}

fn new_raw(kind: &str, text: &str) -> Option<Element> {
    let raw = document().create_element("div").ok()?;
    raw.set_class_name(RAW_CLASS);
    raw.set_attribute(KIND_ATTR, kind).ok()?;
    set_text(&raw, text);
    Some(raw)
}

/// Place the selection inside `raw`, at UTF-16 offsets into its text.
pub fn select_in_raw(raw: &Element, start: u32, end: u32) {
    let (Ok(range), Some(sel)) = (document().create_range(), selection()) else {
        return;
    };
    let place = |at: u32, set: &dyn Fn(&Node, u32)| {
        let list = raw.child_nodes();
        let mut left = at;
        let mut last_text = None;
        for i in 0..list.length() {
            let Some(n) = list.item(i) else { continue };
            if n.node_type() != Node::TEXT_NODE {
                continue;
            }
            let len = utf16(&n.text_content().unwrap_or_default());
            if left <= len {
                set(&n, left);
                return;
            }
            left -= len;
            last_text = Some((n, len));
        }
        match last_text {
            Some((n, len)) => set(&n, len),
            None => set(raw.unchecked_ref(), 0),
        }
    };
    place(start, &|n, o| {
        let _ = range.set_start(n, o);
    });
    place(end, &|n, o| {
        let _ = range.set_end(n, o);
    });
    let _ = sel.remove_all_ranges();
    let _ = sel.add_range(&range);
}

/// The selection's offsets into `raw`'s text, if it's entirely inside.
fn selection_in_raw(raw: &Element) -> Option<(u32, u32)> {
    let sel = selection()?;
    if sel.range_count() == 0 {
        return None;
    }
    let range = sel.get_range_at(0).ok()?;
    let node: &Node = raw.unchecked_ref();
    if !node.contains(Some(&range.start_container().ok()?))
        || !node.contains(Some(&range.end_container().ok()?))
    {
        return None;
    }
    let measure = document().create_range().ok()?;
    measure.select_node_contents(node).ok()?;
    measure
        .set_end(&range.start_container().ok()?, range.start_offset().ok()?)
        .ok()?;
    let start = utf16(&String::from(measure.to_string()));
    measure
        .set_end(&range.end_container().ok()?, range.end_offset().ok()?)
        .ok()?;
    let end = utf16(&String::from(measure.to_string()));
    Some((start, end))
}

// ---------- what can open ----------

/// A paragraph that's nothing but a display formula. It has its own editor, so
/// it isn't opened as text.
fn is_math_paragraph(el: &Element) -> bool {
    if !is_tag(el, &["P"]) || el.child_element_count() != 1 {
        return false;
    }
    let Some(only) = el.first_element_child() else {
        return false;
    };
    only.class_list().contains(crate::math::DISPLAY_CLASS)
        && el.text_content().unwrap_or_default().trim()
            == only.text_content().unwrap_or_default().trim()
}

/// The element whose source editor a caret arriving at `el` should open.
fn source_block(el: &Element) -> Option<Element> {
    if el.class_list().contains(crate::mermaid::WRAPPER_CLASS) {
        return Some(el.clone());
    }
    if is_math_paragraph(el) {
        return el.first_element_child();
    }
    None
}

/// The block to open for a caret at `anchor`.
fn openable(anchor: &Node, ed: &Element) -> Option<Element> {
    let el = element_of(anchor)?;
    let excluded = "td,th,pre,.mermaid-diagram,.math,textarea,hr";
    if el.closest(excluded).ok().flatten().is_some() {
        return None;
    }
    let block = el.closest("h1,h2,h3,h4,h5,h6,p,li,div").ok().flatten()?;
    if &block == ed || !ed.contains(Some(block.unchecked_ref())) {
        return None;
    }
    if is_math_paragraph(&block) {
        return None;
    }
    if is_tag(&block, &["DIV"]) && has_block_child(&block) {
        return None;
    }
    Some(block)
}

fn has_block_child(el: &Element) -> bool {
    let list = el.children();
    (0..list.length())
        .filter_map(|i| list.item(i))
        .any(|c| is_tag(&c, BLOCK_CHILD))
}

// ---------- opening ----------

/// A list item's own content: everything up to its nested blocks, without the
/// task checkbox.
fn own_inline(li: &Element) -> Vec<Node> {
    let list = li.child_nodes();
    let mut out = Vec::new();
    for i in 0..list.length() {
        let Some(n) = list.item(i) else { continue };
        if let Some(e) = n.dyn_ref::<Element>() {
            if is_tag(e, BLOCK_CHILD) {
                break;
            }
            if e.dyn_ref::<web_sys::HtmlInputElement>()
                .is_some_and(|i| i.type_() == "checkbox")
            {
                continue;
            }
        }
        out.push(n);
    }
    out
}

/// The source of a list item's own content.
fn item_source(li: &Element) -> String {
    let Ok(holder) = document().create_element("div") else {
        return String::new();
    };
    for n in own_inline(li) {
        if let Ok(copy) = n.clone_node_with_deep(true) {
            let _ = holder.append_child(&copy);
        }
    }
    source_of_holder(&holder)
}

fn source_of_holder(holder: &Element) -> String {
    dom_to_markdown(holder).trim_end_matches('\n').to_string()
}

fn block_source(block: &Element) -> String {
    let Ok(holder) = document().create_element("div") else {
        return String::new();
    };
    let Ok(copy) = block.clone_node_with_deep(true) else {
        return String::new();
    };
    let _ = holder.append_child(&copy);
    source_of_holder(&holder)
}

/// Turn `block` into a raw block with the caret on the matching character.
///
/// The caret is found by putting a marker at it, serializing, and seeing where
/// the marker lands, which accounts for every marker and escape the serializer
/// adds. The marker can itself change the result: at a line start it stops a
/// leading `#` being escaped. So the text comes from a clean pass, and if the
/// two differ the caret is measured from the end, where they still agree.
fn open(block: &Element) -> Option<Element> {
    let is_item = is_tag(block, &["LI"]);
    let source_now = || {
        if is_item {
            item_source(block)
        } else {
            block_source(block)
        }
    };
    let clean = source_now();

    let sel = selection()?;
    let range = sel.get_range_at(0).ok()?;
    let mark = document().create_text_node(&MARK.to_string());
    range.insert_node(&mark).ok()?;
    let marked = source_now();
    if let Some(p) = mark.parent_node() {
        let _ = p.remove_child(&mark);
    }

    let offset = match marked.find(MARK) {
        Some(at) => {
            let stripped: String = marked.chars().filter(|c| *c != MARK).collect();
            if stripped == clean {
                utf16(&marked[..at])
            } else {
                let tail = utf16(&marked[at + MARK.len_utf8()..]);
                utf16(&clean).saturating_sub(tail)
            }
        }
        None => utf16(&clean),
    };

    let kind = if is_item { KIND_ITEM } else { KIND_BLOCK };
    let raw = new_raw(kind, &clean)?;
    if is_item {
        let own = own_inline(block);
        let before = own.last().and_then(|n| n.next_sibling());
        for n in &own {
            let _ = block.remove_child(n);
        }
        block.insert_before(&raw, before.as_ref()).ok()?;
    } else {
        let tag = block.tag_name().to_lowercase();
        let _ = raw.set_attribute("data-md-from", &tag);
        block
            .parent_node()?
            .replace_child(raw.unchecked_ref(), block)
            .ok()?;
    }
    select_in_raw(&raw, offset, offset);
    Some(raw)
}

// ---------- closing ----------

/// Parse markup into a detached holder.
fn holder_for(html: &str) -> Option<Element> {
    let holder = document().create_element("div").ok()?;
    holder.set_inner_html(html);
    Some(holder)
}

fn rendered(text: &str) -> String {
    if text.trim().is_empty() {
        "<p><br></p>".to_string()
    } else {
        to_html(text)
    }
}

/// Swap `raw` for the holder's content.
fn install(raw: &Element, holder: &Element) -> Option<()> {
    let parent = raw.parent_node()?;
    let is_item = raw.get_attribute(KIND_ATTR).as_deref() == Some(KIND_ITEM);
    // A list item with one paragraph writes that paragraph's content straight
    // into the item, the way a tight list renders.
    let source: Element = if is_item
        && holder.child_element_count() == 1
        && holder
            .first_element_child()
            .is_some_and(|c| is_tag(&c, &["P"]))
    {
        holder.first_element_child()?
    } else {
        holder.clone()
    };
    while let Some(n) = source.first_child() {
        // The newlines between rendered blocks mean nothing in the live DOM.
        let blank = n.node_type() == Node::TEXT_NODE
            && source == *holder
            && n.text_content().unwrap_or_default().trim().is_empty();
        if blank {
            source.remove_child(&n).ok()?;
        } else {
            parent.insert_before(&n, Some(raw.unchecked_ref())).ok()?;
        }
    }
    parent.remove_child(raw.unchecked_ref()).ok()?;
    Some(())
}

fn close(raw: &Element) {
    let text = text_of(raw);
    if let Some(holder) = holder_for(&rendered(&text)) {
        install(raw, &holder);
    }
}

/// Take the marker out of a text node, and return where it was.
fn pull_mark(root: &Node, mark: char) -> Option<(Node, u32)> {
    if root.node_type() == Node::TEXT_NODE {
        let text = root.text_content().unwrap_or_default();
        let at = text.find(mark)?;
        let off = utf16(&text[..at]);
        let t = root.dyn_ref::<Text>()?;
        t.delete_data(off, 1).ok()?;
        return Some((root.clone(), off));
    }
    let list = root.child_nodes();
    for i in 0..list.length() {
        if let Some(found) = list.item(i).and_then(|c| pull_mark(&c, mark)) {
            return Some(found);
        }
    }
    None
}

/// Close `raw` and keep the selection on the same characters of the rendered
/// result. If the markers would change what gets rendered (a marker inside a
/// link's URL, say), the caret goes to the end of the block instead.
fn close_keeping_selection(raw: &Element) {
    let Some((start, end)) = selection_in_raw(raw) else {
        close(raw);
        return;
    };
    let text = text_of(raw);
    let units: Vec<u16> = text.encode_utf16().collect();
    let cut = |at: u32| at.min(units.len() as u32) as usize;
    let mut marked = String::from_utf16_lossy(&units[..cut(start)]);
    marked.push(MARK);
    if end != start {
        marked.push_str(&String::from_utf16_lossy(&units[cut(start)..cut(end)]));
        marked.push(MARK_END);
    }
    marked.push_str(&String::from_utf16_lossy(&units[cut(end)..]));

    let plain = rendered(&text);
    let Some(holder) = holder_for(&rendered(&marked)) else {
        close(raw);
        return;
    };
    let node: &Node = holder.unchecked_ref();
    let a = pull_mark(node, MARK);
    let b = pull_mark(node, MARK_END);
    let same = holder_for(&plain).is_some_and(|p| p.inner_html() == holder.inner_html());
    if !same {
        let Some(holder) = holder_for(&plain) else {
            return;
        };
        let last = raw.next_element_sibling();
        let parent = raw.parent_node();
        install(raw, &holder);
        caret_before_or_end(parent.as_ref(), last.as_ref());
        return;
    }
    install(raw, &holder);
    let (Some(sel), Ok(range)) = (selection(), document().create_range()) else {
        return;
    };
    let Some((sn, so)) = a else {
        return;
    };
    let _ = range.set_start(&sn, so);
    match b {
        Some((en, eo)) => {
            let _ = range.set_end(&en, eo);
        }
        None => range.collapse_with_to_start(true),
    }
    let _ = sel.remove_all_ranges();
    let _ = sel.add_range(&range);
}

/// A caret at the end of whatever comes just before `next`.
fn caret_before_or_end(parent: Option<&Node>, next: Option<&Element>) {
    let prev = match next {
        Some(n) => n.previous_element_sibling(),
        None => parent
            .and_then(|p| p.dyn_ref::<Element>().cloned())
            .and_then(|p| p.last_element_child()),
    };
    if let Some(p) = prev {
        caret_into(&p, true);
    }
}

fn caret_into(el: &Element, at_end: bool) {
    if let Some(ed) = editor().and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()) {
        let _ = ed.focus();
    }
    let (Ok(range), Some(sel)) = (document().create_range(), selection()) else {
        return;
    };
    if range.select_node_contents(el.unchecked_ref()).is_err() {
        return;
    }
    let empty = el.text_content().unwrap_or_default().is_empty();
    range.collapse_with_to_start(!at_end || empty);
    let _ = sel.remove_all_ranges();
    let _ = sel.add_range(&range);
}

/// Render every open block. For anything that's about to read or replace the
/// DOM wholesale: a tab switch, an export, the setting being turned off.
pub fn close_all() {
    let open = all_raw();
    if open.is_empty() {
        return;
    }
    for raw in &open {
        close(raw);
    }
    settled();
}

/// Close the block holding the caret, keeping the selection, so a formatting
/// command acts on rendered text as it would in WYSIWYG mode.
pub fn close_for_command() {
    let Some(raw) = selection()
        .and_then(|s| s.anchor_node())
        .and_then(|n| raw_of(&n))
    else {
        return;
    };
    close_keeping_selection(&raw);
    settled();
}

// ---------- following the caret ----------

/// Make the open block match where the caret is. Runs on every selection
/// change.
pub fn sync() {
    if !enabled() || MOUSE_DOWN.get() || COMPOSING.get() {
        return;
    }
    let Some(ed) = editor() else {
        return;
    };
    // Focus is in a source editor, the find bar, a dialog: the caret isn't
    // really here, whatever the selection says.
    if document().active_element().as_ref() != Some(&ed) {
        return;
    }
    let Some(sel) = selection() else {
        return;
    };
    // Mid selection, swapping the DOM would destroy what's being selected.
    if !sel.is_collapsed() {
        return;
    }
    let Some(anchor) = sel.anchor_node() else {
        return;
    };
    if !ed.contains(Some(&anchor)) {
        return;
    }
    let current = raw_of(&anchor);
    let target = if current.is_some() {
        None
    } else {
        openable(&anchor, &ed)
    };
    let others: Vec<Element> = all_raw()
        .into_iter()
        .filter(|r| Some(r) != current.as_ref())
        .collect();
    if others.is_empty() && target.is_none() {
        return;
    }
    for raw in &others {
        close(raw);
    }
    if let Some(block) = target.filter(|b| b.is_connected()) {
        open(&block);
    }
    settled();
}

/// Set up the listeners. Call once.
pub fn install_hooks(hooks: Hooks) {
    HOOKS.with_borrow_mut(|h| *h = Some(hooks));
    let win = window();

    // A block mustn't swap under a press, or a drag starting there loses its
    // anchor. It opens when the button comes up.
    let down = Closure::<dyn FnMut(MouseEvent)>::new(|_| MOUSE_DOWN.set(true));
    let _ = win.add_event_listener_with_callback_and_bool(
        "mousedown",
        down.as_ref().unchecked_ref(),
        true,
    );
    down.forget();

    let up = Closure::<dyn FnMut(MouseEvent)>::new(|e: MouseEvent| {
        MOUSE_DOWN.set(false);
        // A checkbox's click has yet to fire, and it needs its element.
        let on_input = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
            .is_some();
        if !on_input {
            later(sync);
        }
    });
    let _ =
        win.add_event_listener_with_callback_and_bool("mouseup", up.as_ref().unchecked_ref(), true);
    up.forget();

    // A button released outside the window sends no mouseup here.
    let blur = Closure::<dyn FnMut()>::new(|| MOUSE_DOWN.set(false));
    let _ = win.add_event_listener_with_callback("blur", blur.as_ref().unchecked_ref());
    blur.forget();

    let doc = document();
    let start = Closure::<dyn FnMut()>::new(|| COMPOSING.set(true));
    let _ =
        doc.add_event_listener_with_callback("compositionstart", start.as_ref().unchecked_ref());
    start.forget();
    let end = Closure::<dyn FnMut()>::new(|| {
        COMPOSING.set(false);
        later(sync);
    });
    let _ = doc.add_event_listener_with_callback("compositionend", end.as_ref().unchecked_ref());
    end.forget();
}

fn later(f: impl FnOnce() + 'static) {
    let cb = Closure::once_into_js(f);
    let _ = window().set_timeout_with_callback(cb.unchecked_ref());
}

// ---------- keys ----------

/// Insert text at the selection inside a raw block.
fn insert_text(raw: &Element, text: &str) {
    let Some((start, end)) = selection_in_raw(raw) else {
        return;
    };
    let current = text_of(raw);
    let units: Vec<u16> = current.encode_utf16().collect();
    let cut = |at: u32| at.min(units.len() as u32) as usize;
    let mut next = String::from_utf16_lossy(&units[..cut(start)]);
    next.push_str(text);
    next.push_str(&String::from_utf16_lossy(&units[cut(end)..]));
    set_text(raw, &next);
    let at = start + utf16(text);
    select_in_raw(raw, at, at);
}

/// Paste into an open block as plain text. Returns true when the caret was in
/// one. Rich paste would put markup inside what has to stay plain.
pub fn paste_plain(text: &str) -> bool {
    let Some(raw) = caret_raw() else {
        return false;
    };
    commit();
    insert_text(&raw, text);
    changed();
    true
}

fn caret_raw() -> Option<Element> {
    raw_of(&selection()?.anchor_node()?)
}

/// Is the source before the caret inside a fence that hasn't closed? Enter
/// there is another line of the block, not a new block.
fn in_open_fence(before: &str) -> bool {
    let first = before.lines().next().unwrap_or("").trim_start();
    if first.starts_with("```") || first.starts_with("~~~") {
        let marker = &first[..3];
        return !before
            .lines()
            .skip(1)
            .any(|l| l.trim_start().starts_with(marker));
    }
    if first.starts_with("$$") {
        return before.matches("$$").count() == 1;
    }
    false
}

/// Keys inside an open block. Returns true when it took the key.
pub fn handle_key(e: &KeyboardEvent) -> bool {
    if e.ctrl_key() || e.meta_key() || e.alt_key() || e.is_composing() {
        return false;
    }
    let key = e.key();
    if !matches!(key.as_str(), "Enter" | "Backspace" | "Delete") {
        return false;
    }
    let Some(raw) = caret_raw() else {
        return false;
    };
    let Some((start, end)) = selection_in_raw(&raw) else {
        return false;
    };
    let text = text_of(&raw);
    let units: Vec<u16> = text.encode_utf16().collect();
    let at = |o: u32| o.min(units.len() as u32) as usize;
    let before = String::from_utf16_lossy(&units[..at(start)]);
    let after = String::from_utf16_lossy(&units[at(end)..]);
    let is_item = raw.get_attribute(KIND_ATTR).as_deref() == Some(KIND_ITEM);
    match key.as_str() {
        "Enter" => {
            commit();
            if e.shift_key() || in_open_fence(&before) {
                insert_text(&raw, "\n");
            } else if is_item {
                split_item(&raw, &before, &after);
            } else {
                split_block(&raw, &before, &after);
            }
            changed();
            true
        }
        "Backspace" if start == 0 && end == 0 => {
            if is_item {
                backspace_item(&raw, &text);
            } else {
                backspace_block(&raw, &text);
            }
            true
        }
        "Delete" if start == end && at(start) == units.len() => {
            if is_item {
                delete_item(&raw, &text);
            } else {
                delete_block(&raw, &text);
            }
            true
        }
        _ => false,
    }
}

fn split_block(raw: &Element, before: &str, after: &str) {
    let Some(next) = new_raw(KIND_BLOCK, after) else {
        return;
    };
    let _ = raw.after_with_node_1(&next);
    set_text(raw, before);
    close(raw);
    select_in_raw(&next, 0, 0);
}

fn split_item(raw: &Element, before: &str, after: &str) {
    let Some(li) = raw.closest("li").ok().flatten() else {
        return;
    };
    let has_nested = own_inline(&li).len() + 1 < li.child_element_count() as usize;
    // Enter on an empty item leaves the list, as every editor does.
    if before.is_empty() && after.is_empty() && !has_nested {
        let Some(list) = li.parent_element() else {
            return;
        };
        let Some(para) = new_raw(KIND_BLOCK, "") else {
            return;
        };
        let _ = list.after_with_node_1(&para);
        li.remove();
        if list.child_element_count() == 0 {
            list.remove();
        }
        select_in_raw(&para, 0, 0);
        return;
    }
    let Ok(new_li) = document().create_element("li") else {
        return;
    };
    // A new task starts unticked, and a checkbox's state is a property.
    if li
        .query_selector(":scope > input[type=checkbox]")
        .ok()
        .flatten()
        .is_some()
    {
        if let Ok(input) = document().create_element("input") {
            let _ = input.set_attribute("type", "checkbox");
            let _ = new_li.append_child(&input);
        }
    }
    let Some(next) = new_raw(KIND_ITEM, after) else {
        return;
    };
    let _ = new_li.append_child(&next);
    let _ = li.after_with_node_1(&new_li);
    set_text(raw, before);
    close(raw);
    select_in_raw(&next, 0, 0);
}

/// Can this sibling's text merge with an open block?
fn mergeable(el: &Element) -> bool {
    is_tag(el, &["P", "H1", "H2", "H3", "H4", "H5", "H6", "DIV"])
        && !has_block_child(el)
        && !is_math_paragraph(el)
        && !el.class_list().contains(crate::mermaid::WRAPPER_CLASS)
        && !el.class_list().contains(RAW_CLASS)
}

fn backspace_block(raw: &Element, text: &str) {
    let Some(prev) = raw.previous_element_sibling() else {
        return;
    };
    if mergeable(&prev) {
        commit();
        let head = block_source(&prev);
        prev.remove();
        set_text(raw, &format!("{head}{text}"));
        let at = utf16(&head);
        select_in_raw(raw, at, at);
        changed();
    } else if text.is_empty() {
        commit();
        raw.remove();
        caret_into(&prev, true);
        changed();
    }
}

fn delete_block(raw: &Element, text: &str) {
    let Some(next) = raw.next_element_sibling() else {
        return;
    };
    if mergeable(&next) {
        commit();
        let tail = block_source(&next);
        next.remove();
        set_text(raw, &format!("{text}{tail}"));
        let at = utf16(text);
        select_in_raw(raw, at, at);
        changed();
    } else if text.is_empty() {
        commit();
        raw.remove();
        caret_into(&next, false);
        changed();
    }
}

/// Move an item's nested blocks onto the item that's absorbing it.
fn move_nested(from: &Element, to: &Element) {
    for n in own_inline(from) {
        let _ = from.remove_child(&n);
    }
    let list = from.children();
    let kids: Vec<Element> = (0..list.length()).filter_map(|i| list.item(i)).collect();
    for k in kids {
        if is_tag(&k, BLOCK_CHILD) && !k.class_list().contains(RAW_CLASS) {
            let _ = to.append_child(&k);
        }
    }
}

fn backspace_item(raw: &Element, text: &str) {
    let Some(li) = raw.closest("li").ok().flatten() else {
        return;
    };
    match li.previous_element_sibling() {
        Some(prev) => {
            commit();
            let head = item_source(&prev);
            let own = own_inline(&prev);
            let before = own.last().and_then(|n| n.next_sibling());
            for n in &own {
                let _ = prev.remove_child(n);
            }
            let _ = prev.insert_before(raw, before.as_ref());
            set_text(raw, &format!("{head}{text}"));
            move_nested(&li, &prev);
            li.remove();
            let at = utf16(&head);
            select_in_raw(raw, at, at);
            changed();
        }
        None => {
            // The first item leaves the list and becomes a paragraph before
            // it, unless it has nested lists that would be left stranded.
            if own_inline(&li).len() + 1 < li.child_element_count() as usize {
                return;
            }
            let Some(list) = li.parent_element() else {
                return;
            };
            commit();
            let _ = list.before_with_node_1(raw);
            let _ = raw.set_attribute(KIND_ATTR, KIND_BLOCK);
            li.remove();
            if list.child_element_count() == 0 {
                list.remove();
            }
            select_in_raw(raw, 0, 0);
            changed();
        }
    }
}

fn delete_item(raw: &Element, text: &str) {
    let Some(li) = raw.closest("li").ok().flatten() else {
        return;
    };
    let Some(next) = li.next_element_sibling() else {
        return;
    };
    commit();
    let tail = item_source(&next);
    set_text(raw, &format!("{text}{tail}"));
    move_nested(&next, &li);
    next.remove();
    let at = utf16(text);
    select_in_raw(raw, at, at);
    changed();
}

// ---------- arrowing into source editors ----------

fn top_block(node: &Node, ed: &Element) -> Option<Element> {
    let mut el = element_of(node)?;
    loop {
        let parent = el.parent_element()?;
        if &parent == ed {
            return Some(el);
        }
        el = parent;
    }
}

/// Up or down arrow: let the browser move the caret, then see whether it went
/// past a diagram or display formula, or couldn't go anywhere because one was
/// next. Either way, that's the one to open.
pub fn on_arrow(e: &KeyboardEvent) {
    let down = match e.key().as_str() {
        "ArrowDown" => true,
        "ArrowUp" => false,
        _ => return,
    };
    if e.shift_key() || e.ctrl_key() || e.meta_key() || e.alt_key() || !enabled() {
        return;
    }
    let (Some(ed), Some(sel)) = (editor(), selection()) else {
        return;
    };
    let Some(anchor) = sel.anchor_node() else {
        return;
    };
    // Up and Down belong to the table while the caret is in one.
    if element_of(&anchor).is_some_and(|el| el.closest("td,th").ok().flatten().is_some()) {
        return;
    }
    let offset = sel.anchor_offset();
    let Some(from) = top_block(&anchor, &ed) else {
        return;
    };
    later(move || {
        let Some(sel) = selection() else {
            return;
        };
        let Some(now) = sel.anchor_node() else {
            return;
        };
        let stayed = now == anchor && sel.anchor_offset() == offset;
        let to = top_block(&now, &ed);
        if !stayed && to.as_ref() == Some(&from) {
            return; // moved within the block
        }
        let step = |el: &Element| {
            if down {
                el.next_element_sibling()
            } else {
                el.previous_element_sibling()
            }
        };
        let mut walk = step(&from);
        while let Some(el) = walk {
            if let Some(block) = source_block(&el) {
                with_hooks(|h| (h.open_source)(&block));
                return;
            }
            if Some(&el) == to.as_ref() {
                return;
            }
            walk = step(&el);
        }
    });
}

/// Arrow keys at the edge of an open source editor leave it, in reveal mode.
/// `source` is the textarea. Returns true when it left.
pub fn leave_source(source: &web_sys::HtmlTextAreaElement, e: &KeyboardEvent) -> bool {
    if !enabled() || e.shift_key() || e.ctrl_key() || e.meta_key() || e.alt_key() {
        return false;
    }
    let len = source.value().encode_utf16().count() as u32;
    let start = source.selection_start().ok().flatten().unwrap_or(0);
    let end = source.selection_end().ok().flatten().unwrap_or(0);
    let down = match e.key().as_str() {
        "ArrowDown" if end == len => true,
        "ArrowUp" if start == 0 => false,
        _ => return false,
    };
    let Some(ed) = editor() else {
        return false;
    };
    let Some(block) = top_block(source.unchecked_ref(), &ed) else {
        return false;
    };
    let neighbour = if down {
        block.next_element_sibling()
    } else {
        block.previous_element_sibling()
    };
    let _ = source.blur();
    let Some(next) = neighbour else {
        let _ = ed.dyn_ref::<web_sys::HtmlElement>().map(|h| h.focus());
        return true;
    };
    if let Some(open_next) = source_block(&next) {
        with_hooks(|h| (h.open_source)(&open_next));
    } else {
        caret_into(&next, !down);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unclosed_fence_keeps_enter_in_the_block() {
        assert!(in_open_fence("```rust"));
        assert!(in_open_fence("```\nlet a = 1;"));
        assert!(!in_open_fence("```\nx\n```"));
        assert!(in_open_fence("~~~"));
        assert!(in_open_fence("$$"));
        assert!(in_open_fence("$$\nx"));
        assert!(!in_open_fence("$$x$$"));
        assert!(!in_open_fence("plain text"));
        assert!(!in_open_fence(""));
    }
}
