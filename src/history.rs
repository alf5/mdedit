//! Snapshot-based undo/redo primitives. The app keeps its own history —
//! native contenteditable undo can't see DOM table operations, checkbox
//! toggles, or replace-all, so it is suppressed and replaced wholesale.

use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlInputElement, Node};

use leptos::prelude::window;

/// One restorable editor state, tagged with the mode it was taken in.
#[derive(Clone, PartialEq)]
pub enum Snap {
    Wys {
        html: String,
        caret: u32,
    },
    Src {
        text: String,
        sel_start: u32,
        sel_end: u32,
    },
}

/// Capture the WYSIWYG editor. Checkbox `checked` properties are mirrored
/// into attributes first — innerHTML only serializes attributes.
pub fn capture_wys(editor: &Element) -> Snap {
    sync_checkbox_attrs(editor);
    Snap::Wys {
        html: editor.inner_html(),
        caret: caret_text_offset(editor),
    }
}

fn sync_checkbox_attrs(root: &Element) {
    if let Ok(list) = root.query_selector_all("input[type=checkbox]") {
        for i in 0..list.length() {
            if let Some(input) = list.item(i).and_then(|n| n.dyn_into::<HtmlInputElement>().ok())
            {
                if input.checked() {
                    let _ = input.set_attribute("checked", "");
                } else {
                    let _ = input.remove_attribute("checked");
                }
            }
        }
    }
}

fn utf16_len(node: &Node) -> u32 {
    node.text_content()
        .unwrap_or_default()
        .encode_utf16()
        .count() as u32
}

/// Absolute UTF-16 text offset of the selection anchor within `root`
/// (0 when there is no usable selection).
pub fn caret_text_offset(root: &Element) -> u32 {
    let Some(sel) = window().get_selection().ok().flatten() else {
        return 0;
    };
    let Some(anchor) = sel.anchor_node() else {
        return 0;
    };
    if !root.contains(Some(&anchor)) {
        return 0;
    }
    let mut acc = 0u32;
    if walk_to_anchor(root.unchecked_ref(), &anchor, sel.anchor_offset(), &mut acc) {
        acc
    } else {
        0
    }
}

fn walk_to_anchor(node: &Node, anchor: &Node, anchor_off: u32, acc: &mut u32) -> bool {
    if node.node_type() == Node::TEXT_NODE {
        if node == anchor {
            *acc += anchor_off;
            return true;
        }
        *acc += utf16_len(node);
        return false;
    }
    if node == anchor {
        // Element anchor: count the text of the first `anchor_off` children.
        let list = node.child_nodes();
        for i in 0..anchor_off.min(list.length()) {
            if let Some(c) = list.item(i) {
                *acc += utf16_len(&c);
            }
        }
        return true;
    }
    let list = node.child_nodes();
    for i in 0..list.length() {
        if let Some(c) = list.item(i) {
            if walk_to_anchor(&c, anchor, anchor_off, acc) {
                return true;
            }
        }
    }
    false
}

/// Place a collapsed caret at the given absolute text offset (end of
/// content when the offset lies beyond it).
pub fn restore_caret_at(root: &Element, offset: u32) {
    let Some(sel) = window().get_selection().ok().flatten() else {
        return;
    };
    let Ok(range) = leptos::prelude::document().create_range() else {
        return;
    };
    let mut remaining = offset;
    if let Some((node, off)) = find_offset(root.unchecked_ref(), &mut remaining) {
        let _ = range.set_start(&node, off);
    } else {
        let _ = range.select_node_contents(root.unchecked_ref::<Node>());
        range.collapse();
    }
    range.collapse_with_to_start(true);
    let _ = sel.remove_all_ranges();
    let _ = sel.add_range(&range);
}

fn find_offset(node: &Node, remaining: &mut u32) -> Option<(Node, u32)> {
    if node.node_type() == Node::TEXT_NODE {
        let len = utf16_len(node);
        if *remaining <= len {
            return Some((node.clone(), *remaining));
        }
        *remaining -= len;
        return None;
    }
    let list = node.child_nodes();
    for i in 0..list.length() {
        if let Some(c) = list.item(i) {
            if let Some(found) = find_offset(&c, remaining) {
                return Some(found);
            }
        }
    }
    None
}
