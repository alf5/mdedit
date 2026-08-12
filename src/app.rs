use std::cell::RefCell;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlTextAreaElement};

use crate::history::{self, Snap};
use crate::markdown::to_html;
use crate::serialize::{dom_to_markdown, html_to_markdown};
use crate::tauri_api::{self, ContentArgs, DirtyArgs};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Wysiwyg,
    Source,
}

#[derive(Clone, PartialEq)]
enum Pending {
    New,
    Open,
    OpenPath(String),
    ImportHtml,
    ImportDocx,
    Close,
}

#[derive(Clone, Copy, PartialEq)]
enum UrlKind {
    Link,
    Image,
}

#[derive(Clone, Copy)]
struct Ctl {
    mode: RwSignal<Mode>,
    dirty: RwSignal<bool>,
    doc_name: RwSignal<String>,
    doc_path: RwSignal<String>,
    source: RwSignal<String>,
    find_open: RwSignal<bool>,
    replace_open: RwSignal<bool>,
    find_q: RwSignal<String>,
    replace_q: RwSignal<String>,
    case_sens: RwSignal<bool>,
    match_count: RwSignal<usize>,
    words: RwSignal<usize>,
    chars: RwSignal<usize>,
    pending: RwSignal<Option<Pending>>,
    url_modal: RwSignal<Option<UrlKind>>,
    url_val: RwSignal<String>,
    url_text: RwSignal<String>,
    in_table: RwSignal<bool>,
    ctx_menu: RwSignal<Option<(f64, f64)>>,
    editor_ref: NodeRef<html::Div>,
    ta_ref: NodeRef<html::Textarea>,
    find_ref: NodeRef<html::Input>,
    url_ref: NodeRef<html::Input>,
    saved_range: StoredValue<Option<web_sys::Range>, LocalStorage>,
    ctx_cell: StoredValue<Option<Element>, LocalStorage>,
    /// Set by Ctrl+Shift+V just before the paste event it triggers.
    plain_paste: StoredValue<bool>,
    hist: StoredValue<Hist>,
    can_undo: RwSignal<bool>,
    can_redo: RwSignal<bool>,
}

#[derive(Default)]
struct Hist {
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    /// Copy of the current editor state, i.e. what a commit would push.
    /// Kept in sync after every mutation so commits can snapshot the
    /// *pre-change* state even though input events fire post-change.
    shadow: Option<Snap>,
    last_input: f64,
}

const HISTORY_LIMIT: usize = 200;
const TYPING_BURST_MS: f64 = 800.0;

thread_local! {
    static LAST_ACTION: RefCell<(String, f64)> = RefCell::new((String::new(), 0.0));
}

/// Menu accelerators and the in-page keydown handler can both fire for one
/// keypress (platform-dependent); collapse duplicates within a short window.
/// Undo/redo use a tighter window so held-key repeats still get through.
fn dedup_ok(action: &str) -> bool {
    let window_ms = if matches!(action, "undo" | "redo") {
        75.0
    } else {
        200.0
    };
    let now = js_sys::Date::now();
    LAST_ACTION.with(|l| {
        let mut l = l.borrow_mut();
        if l.0 == action && now - l.1 < window_ms {
            return false;
        }
        *l = (action.to_string(), now);
        true
    })
}

// ---------- small DOM helpers ----------

fn html_doc() -> web_sys::HtmlDocument {
    document().unchecked_into()
}

fn exec(cmd: &str) {
    let _ = html_doc().exec_command(cmd);
}

fn exec_val(cmd: &str, val: &str) {
    let _ = html_doc().exec_command_with_show_ui_and_value(cmd, false, val);
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn selection_string() -> String {
    window()
        .get_selection()
        .ok()
        .flatten()
        .map(|s| String::from(s.to_string()))
        .unwrap_or_default()
}

fn selection_in(tag: &str) -> bool {
    let Some(sel) = window().get_selection().ok().flatten() else {
        return false;
    };
    let Some(node) = sel.anchor_node() else {
        return false;
    };
    let el = node
        .dyn_ref::<Element>()
        .cloned()
        .or_else(|| node.parent_element());
    el.and_then(|e| e.closest(tag).ok().flatten()).is_some()
}

fn chars_eq(a: char, b: char, case_sensitive: bool) -> bool {
    if case_sensitive {
        a == b
    } else {
        a == b || a.to_lowercase().eq(b.to_lowercase())
    }
}

fn find_in_chars(hay: &[char], needle: &[char], from: usize, cs: bool) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (from..=hay.len() - needle.len())
        .find(|&i| hay[i..i + needle.len()].iter().zip(needle).all(|(&a, &b)| chars_eq(a, b, cs)))
}

fn rfind_in_chars(hay: &[char], needle: &[char], before: usize, cs: bool) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    let end = before.min(hay.len() - needle.len() + 1);
    (0..end)
        .rev()
        .find(|&i| hay[i..i + needle.len()].iter().zip(needle).all(|(&a, &b)| chars_eq(a, b, cs)))
}

fn count_matches(hay: &str, needle: &str, cs: bool) -> usize {
    let hay: Vec<char> = hay.chars().collect();
    let needle: Vec<char> = needle.chars().collect();
    if needle.is_empty() {
        return 0;
    }
    let mut count = 0;
    let mut i = 0;
    while let Some(pos) = find_in_chars(&hay, &needle, i, cs) {
        count += 1;
        i = pos + needle.len();
    }
    count
}

fn replace_all_str(hay: &str, needle: &str, rep: &str, cs: bool) -> (String, usize) {
    let hchars: Vec<char> = hay.chars().collect();
    let nchars: Vec<char> = needle.chars().collect();
    if nchars.is_empty() {
        return (hay.to_string(), 0);
    }
    let mut out = String::new();
    let mut count = 0;
    let mut i = 0;
    while let Some(pos) = find_in_chars(&hchars, &nchars, i, cs) {
        out.extend(&hchars[i..pos]);
        out.push_str(rep);
        count += 1;
        i = pos + nchars.len();
    }
    out.extend(&hchars[i..]);
    (out, count)
}

fn utf16_len(chars: &[char]) -> u32 {
    chars.iter().map(|c| c.len_utf16() as u32).sum()
}

// ---------- controller helpers ----------

fn editor_el(ctl: Ctl) -> Option<web_sys::HtmlDivElement> {
    ctl.editor_ref.get_untracked()
}

fn ta_el(ctl: Ctl) -> Option<HtmlTextAreaElement> {
    ctl.ta_ref.get_untracked()
}

fn focus_current(ctl: Ctl) {
    match ctl.mode.get_untracked() {
        Mode::Wysiwyg => {
            if let Some(el) = editor_el(ctl) {
                let _ = el.focus();
            }
        }
        Mode::Source => {
            if let Some(el) = ta_el(ctl) {
                let _ = el.focus();
            }
        }
    }
}

fn enable_checkboxes(ctl: Ctl) {
    if let Some(el) = editor_el(ctl) {
        if let Ok(list) = el.query_selector_all("input[type=checkbox]") {
            for i in 0..list.length() {
                if let Some(n) = list.item(i) {
                    if let Some(e) = n.dyn_ref::<Element>() {
                        let _ = e.remove_attribute("disabled");
                        // Keep the caret from landing inside the control.
                        let _ = e.set_attribute("contenteditable", "false");
                    }
                }
            }
        }
    }
}

fn visible_text(ctl: Ctl) -> String {
    match ctl.mode.get_untracked() {
        Mode::Wysiwyg => editor_el(ctl)
            .and_then(|e| e.text_content())
            .unwrap_or_default(),
        Mode::Source => ta_el(ctl).map(|t| t.value()).unwrap_or_default(),
    }
}

/// The text content of every rendered diagram in the WYSIWYG editor.
fn diagram_texts(ctl: Ctl) -> Vec<String> {
    let Some(el) = editor_el(ctl) else {
        return Vec::new();
    };
    let sel = format!(".{}", crate::mermaid::WRAPPER_CLASS);
    let Ok(list) = el.query_selector_all(&sel) else {
        return Vec::new();
    };
    (0..list.length())
        .filter_map(|i| list.item(i).and_then(|n| n.text_content()))
        .collect()
}

fn recount(ctl: Ctl) {
    let text = visible_text(ctl);
    let mut words = text.split_whitespace().count();
    let mut chars = text.chars().count();
    // A rendered diagram contributes its node labels to the editor's
    // textContent. Those aren't prose, so take them back out (approximate at
    // the edges, where a label can run into adjacent text).
    if ctl.mode.get_untracked() == Mode::Wysiwyg {
        for t in diagram_texts(ctl) {
            words = words.saturating_sub(t.split_whitespace().count());
            chars = chars.saturating_sub(t.chars().count());
        }
    }
    ctl.words.set(words);
    ctl.chars.set(chars);
    let q = ctl.find_q.get_untracked();
    if ctl.find_open.get_untracked() && !q.is_empty() {
        ctl.match_count
            .set(count_matches(&text, &q, ctl.case_sens.get_untracked()));
    }
}

fn current_markdown(ctl: Ctl) -> String {
    match ctl.mode.get_untracked() {
        Mode::Wysiwyg => editor_el(ctl)
            .map(|e| dom_to_markdown(e.unchecked_ref::<Element>()))
            .unwrap_or_default(),
        Mode::Source => {
            let mut s = ta_el(ctl).map(|t| t.value()).unwrap_or_default();
            if !s.is_empty() && !s.ends_with('\n') {
                s.push('\n');
            }
            s
        }
    }
}

fn load_markdown(ctl: Ctl, md: &str) {
    match ctl.mode.get_untracked() {
        Mode::Wysiwyg => {
            if let Some(el) = editor_el(ctl) {
                let html = to_html(md);
                el.set_inner_html(if html.is_empty() { "<p><br></p>" } else { &html });
                enable_checkboxes(ctl);
            }
        }
        Mode::Source => {
            ctl.source.set(md.to_string());
        }
    }
    recount(ctl);
    history_reset(ctl);
}

fn mark_dirty(ctl: Ctl) {
    if !ctl.dirty.get_untracked() {
        ctl.dirty.set(true);
        spawn_local(async move {
            let args = serde_wasm_bindgen::to_value(&DirtyArgs { dirty: true }).unwrap();
            let _ = tauri_api::invoke("set_dirty", args).await;
        });
    }
}

fn mark_clean(ctl: Ctl) {
    ctl.dirty.set(false);
}

fn toggle_mode(ctl: Ctl) {
    match ctl.mode.get_untracked() {
        Mode::Wysiwyg => {
            let md = current_markdown(ctl);
            ctl.source.set(md);
            ctl.mode.set(Mode::Source);
        }
        Mode::Source => {
            let md = ta_el(ctl).map(|t| t.value()).unwrap_or_default();
            ctl.source.set(md.clone());
            ctl.mode.set(Mode::Wysiwyg);
            if let Some(el) = editor_el(ctl) {
                let html = to_html(&md);
                el.set_inner_html(if html.is_empty() { "<p><br></p>" } else { &html });
                enable_checkboxes(ctl);
            }
        }
    }
    recount(ctl);
    focus_current(ctl);
    update_in_table(ctl);
    history_sync(ctl);
}

// ---------- undo / redo ----------

fn capture_current(ctl: Ctl) -> Option<Snap> {
    match ctl.mode.get_untracked() {
        Mode::Wysiwyg => {
            editor_el(ctl).map(|el| history::capture_wys(el.unchecked_ref::<Element>()))
        }
        Mode::Source => ta_el(ctl).map(|ta| Snap::Src {
            text: ta.value(),
            sel_start: ta.selection_start().ok().flatten().unwrap_or(0),
            sel_end: ta.selection_end().ok().flatten().unwrap_or(0),
        }),
    }
}

fn history_flags(ctl: Ctl) {
    let (u, r) = ctl
        .hist
        .with_value(|h| (!h.undo.is_empty(), !h.redo.is_empty()));
    if ctl.can_undo.get_untracked() != u {
        ctl.can_undo.set(u);
    }
    if ctl.can_redo.get_untracked() != r {
        ctl.can_redo.set(r);
    }
}

/// Record the current state as the baseline the next commit will push.
fn history_sync(ctl: Ctl) {
    if let Some(s) = capture_current(ctl) {
        ctl.hist.update_value(|h| h.shadow = Some(s));
    }
}

/// Push the pre-change state (the shadow) onto the undo stack. Duplicate
/// consecutive states are dropped, so double-fires are harmless.
fn history_commit(ctl: Ctl) {
    ctl.hist.update_value(|h| {
        let Some(s) = h.shadow.clone() else { return };
        if h.undo.last() != Some(&s) {
            h.undo.push(s);
            if h.undo.len() > HISTORY_LIMIT {
                h.undo.remove(0);
            }
        }
        h.redo.clear();
    });
    history_flags(ctl);
}

/// Typing input: commit once per burst, using the shadow taken before the
/// burst started.
fn history_on_input(ctl: Ctl) {
    let now = js_sys::Date::now();
    let new_burst = ctl.hist.with_value(|h| now - h.last_input > TYPING_BURST_MS);
    if new_burst {
        history_commit(ctl);
    }
    ctl.hist.update_value(|h| h.last_input = now);
}

/// Fresh document: history starts over.
fn history_reset(ctl: Ctl) {
    let shadow = capture_current(ctl);
    ctl.hist.update_value(|h| {
        h.undo.clear();
        h.redo.clear();
        h.shadow = shadow;
        h.last_input = 0.0;
    });
    history_flags(ctl);
}

fn restore_snap(ctl: Ctl, snap: Snap) {
    match &snap {
        Snap::Wys { html, caret } => {
            if ctl.mode.get_untracked() != Mode::Wysiwyg {
                ctl.mode.set(Mode::Wysiwyg);
            }
            if let Some(el) = editor_el(ctl) {
                el.set_inner_html(html);
                enable_checkboxes(ctl);
                let caret = *caret;
                request_animation_frame(move || {
                    let _ = el.focus();
                    history::restore_caret_at(el.unchecked_ref::<Element>(), caret);
                });
            }
        }
        Snap::Src {
            text,
            sel_start,
            sel_end,
        } => {
            if ctl.mode.get_untracked() != Mode::Source {
                ctl.mode.set(Mode::Source);
            }
            ctl.source.set(text.clone());
            if let Some(ta) = ta_el(ctl) {
                ta.set_value(text);
                let (s, e) = (*sel_start, *sel_end);
                request_animation_frame(move || {
                    let _ = ta.focus();
                    let _ = ta.set_selection_range(s, e);
                });
            }
        }
    }
    ctl.hist.update_value(|h| h.shadow = Some(snap));
    mark_dirty(ctl);
    recount(ctl);
    update_in_table(ctl);
}

fn history_undo(ctl: Ctl) {
    let mut prev = None;
    ctl.hist.update_value(|h| prev = h.undo.pop());
    let Some(prev) = prev else { return };
    if let Some(cur) = capture_current(ctl) {
        ctl.hist.update_value(|h| h.redo.push(cur));
    }
    restore_snap(ctl, prev);
    history_flags(ctl);
}

fn history_redo(ctl: Ctl) {
    let mut next = None;
    ctl.hist.update_value(|h| next = h.redo.pop());
    let Some(next) = next else { return };
    if let Some(cur) = capture_current(ctl) {
        ctl.hist.update_value(|h| h.undo.push(cur));
    }
    restore_snap(ctl, next);
    history_flags(ctl);
}

// ---------- file operations ----------

fn do_new(ctl: Ctl) {
    spawn_local(async move {
        let _ = tauri_api::invoke_no_args("new_doc").await;
        load_markdown(ctl, "");
        mark_clean(ctl);
        ctl.doc_name.set("Untitled".to_string());
        ctl.doc_path.set(String::new());
    });
}

fn do_open(ctl: Ctl) {
    spawn_local(async move {
        if let Ok(v) = tauri_api::invoke_no_args("open_doc").await {
            if let Some(info) = tauri_api::parse_file_info(v) {
                load_markdown(ctl, &info.content);
                mark_clean(ctl);
                ctl.doc_name.set(info.name);
                ctl.doc_path.set(info.path);
            }
        }
    });
}

fn do_save(ctl: Ctl, save_as: bool, then: Option<Pending>) {
    spawn_local(async move {
        let content = current_markdown(ctl);
        let args = serde_wasm_bindgen::to_value(&ContentArgs { content }).unwrap();
        let cmd = if save_as { "save_doc_as" } else { "save_doc" };
        match tauri_api::invoke(cmd, args).await {
            Ok(v) => {
                if let Some(info) = tauri_api::parse_file_info(v) {
                    mark_clean(ctl);
                    ctl.doc_name.set(info.name);
                    ctl.doc_path.set(info.path);
                    if let Some(p) = then {
                        perform_pending(ctl, p);
                    }
                }
                // None: user cancelled the save dialog — stay put.
            }
            Err(e) => {
                let msg = e.as_string().unwrap_or_else(|| "unknown error".into());
                let _ = window().alert_with_message(&format!("Could not save file: {msg}"));
            }
        }
    });
}

fn do_open_path(ctl: Ctl, path: String) {
    spawn_local(async move {
        let args = serde_wasm_bindgen::to_value(&tauri_api::PathArgs { path }).unwrap();
        match tauri_api::invoke("open_path", args).await {
            Ok(v) => {
                if let Some(info) = tauri_api::parse_file_info(v) {
                    load_markdown(ctl, &info.content);
                    mark_clean(ctl);
                    ctl.doc_name.set(info.name);
                    ctl.doc_path.set(info.path);
                }
            }
            Err(e) => {
                let msg = e.as_string().unwrap_or_else(|| "unknown error".into());
                let _ = window().alert_with_message(&format!("Could not open file: {msg}"));
            }
        }
    });
}

fn do_import_html(ctl: Ctl) {
    spawn_local(async move {
        match tauri_api::invoke_no_args("import_html").await {
            Ok(v) => {
                let Ok(Some(html)) = serde_wasm_bindgen::from_value::<Option<String>>(v) else {
                    return; // dialog cancelled
                };
                let md = html_to_markdown(&html);
                let _ = tauri_api::invoke_no_args("new_doc").await;
                ctl.doc_name.set("Untitled".to_string());
                ctl.doc_path.set(String::new());
                load_markdown(ctl, &md);
                ctl.dirty.set(false); // force the dirty transition below
                mark_dirty(ctl); // imported content is unsaved
            }
            Err(e) => {
                let msg = e.as_string().unwrap_or_else(|| "unknown error".into());
                let _ = window().alert_with_message(&format!("Could not import file: {msg}"));
            }
        }
    });
}

fn do_import_docx(ctl: Ctl) {
    spawn_local(async move {
        match tauri_api::invoke_no_args("import_docx").await {
            Ok(v) => {
                let Ok(Some(md)) = serde_wasm_bindgen::from_value::<Option<String>>(v) else {
                    return; // dialog cancelled
                };
                let _ = tauri_api::invoke_no_args("new_doc").await;
                ctl.doc_name.set("Untitled".to_string());
                ctl.doc_path.set(String::new());
                load_markdown(ctl, &md);
                ctl.dirty.set(false);
                mark_dirty(ctl); // imported content is unsaved
            }
            Err(e) => {
                let msg = e.as_string().unwrap_or_else(|| "unknown error".into());
                let _ = window().alert_with_message(&format!("Could not import file: {msg}"));
            }
        }
    });
}

fn perform_pending(ctl: Ctl, p: Pending) {
    match p {
        Pending::New => do_new(ctl),
        Pending::Open => do_open(ctl),
        Pending::OpenPath(path) => do_open_path(ctl, path),
        Pending::ImportHtml => do_import_html(ctl),
        Pending::ImportDocx => do_import_docx(ctl),
        Pending::Close => spawn_local(async move {
            let _ = tauri_api::invoke_no_args("force_close").await;
        }),
    }
}

fn guarded(ctl: Ctl, p: Pending) {
    if ctl.dirty.get_untracked() {
        ctl.pending.set(Some(p));
    } else {
        perform_pending(ctl, p);
    }
}

// ---------- find / replace ----------

fn open_find(ctl: Ctl, with_replace: bool) {
    ctl.find_open.set(true);
    ctl.replace_open.set(with_replace);
    let sel = selection_string();
    if !sel.is_empty() && !sel.contains('\n') {
        ctl.find_q.set(sel);
    }
    recount(ctl);
    request_animation_frame(move || {
        if let Some(inp) = ctl.find_ref.get_untracked() {
            let _ = inp.focus();
            inp.select();
        }
    });
}

fn close_overlays(ctl: Ctl) -> bool {
    let mut closed = false;
    if ctl.ctx_menu.get_untracked().is_some() {
        close_ctx_menu(ctl);
        closed = true;
    } else if ctl.url_modal.get_untracked().is_some() {
        ctl.url_modal.set(None);
        closed = true;
    } else if ctl.pending.get_untracked().is_some() {
        ctl.pending.set(None);
        closed = true;
    } else if ctl.find_open.get_untracked() {
        ctl.find_open.set(false);
        ctl.replace_open.set(false);
        closed = true;
        focus_current(ctl);
    }
    closed
}

fn ta_find(ctl: Ctl, backwards: bool) -> bool {
    let Some(ta) = ta_el(ctl) else { return false };
    let q: Vec<char> = ctl.find_q.get_untracked().chars().collect();
    if q.is_empty() {
        return false;
    }
    let cs = ctl.case_sens.get_untracked();
    let value = ta.value();
    let chars: Vec<char> = value.chars().collect();
    // Selection offsets are UTF-16; map to char indices.
    let sel_start = ta.selection_start().ok().flatten().unwrap_or(0);
    let sel_end = ta.selection_end().ok().flatten().unwrap_or(0);
    let mut u16 = 0u32;
    let mut start_ci = chars.len();
    let mut end_ci = chars.len();
    for (ci, c) in chars.iter().enumerate() {
        if u16 >= sel_start && start_ci == chars.len() {
            start_ci = ci;
        }
        if u16 >= sel_end && end_ci == chars.len() {
            end_ci = ci;
        }
        u16 += c.len_utf16() as u32;
    }
    let pos = if backwards {
        rfind_in_chars(&chars, &q, start_ci, cs)
            .or_else(|| rfind_in_chars(&chars, &q, chars.len(), cs))
    } else {
        find_in_chars(&chars, &q, end_ci, cs).or_else(|| find_in_chars(&chars, &q, 0, cs))
    };
    let Some(pos) = pos else { return false };
    let from = utf16_len(&chars[..pos]);
    let to = from + utf16_len(&chars[pos..pos + q.len()]);
    let _ = ta.focus();
    let _ = ta.set_selection_range(from, to);
    true
}

fn find_next(ctl: Ctl, backwards: bool) {
    let q = ctl.find_q.get_untracked();
    if q.is_empty() {
        return;
    }
    match ctl.mode.get_untracked() {
        Mode::Wysiwyg => {
            tauri_api::window_find(&q, ctl.case_sens.get_untracked(), backwards, true);
        }
        Mode::Source => {
            ta_find(ctl, backwards);
        }
    }
}

fn selection_matches_query(ctl: Ctl) -> bool {
    let q: Vec<char> = ctl.find_q.get_untracked().chars().collect();
    let sel: Vec<char> = selection_string().chars().collect();
    !q.is_empty()
        && sel.len() == q.len()
        && sel
            .iter()
            .zip(&q)
            .all(|(&a, &b)| chars_eq(a, b, ctl.case_sens.get_untracked()))
}

fn replace_one(ctl: Ctl) {
    let q = ctl.find_q.get_untracked();
    if q.is_empty() {
        return;
    }
    let rep = ctl.replace_q.get_untracked();
    match ctl.mode.get_untracked() {
        Mode::Wysiwyg => {
            if selection_matches_query(ctl) {
                history_commit(ctl);
                exec_val("insertText", &rep);
                mark_dirty(ctl);
                recount(ctl);
                history_sync(ctl);
            }
            find_next(ctl, false);
        }
        Mode::Source => {
            let Some(ta) = ta_el(ctl) else { return };
            let start = ta.selection_start().ok().flatten().unwrap_or(0);
            let end = ta.selection_end().ok().flatten().unwrap_or(0);
            let sel: Vec<char> = ctl.find_q.get_untracked().chars().collect();
            let value = ta.value();
            let chars: Vec<char> = value.chars().collect();
            // Check the current selection matches before replacing.
            let all: Vec<char> = value.chars().collect();
            let _ = all;
            let sel_text: String = {
                // slice by utf16 range
                let mut out = String::new();
                let mut u = 0u32;
                for c in &chars {
                    let l = c.len_utf16() as u32;
                    if u >= start && u + l <= end {
                        out.push(*c);
                    }
                    u += l;
                }
                out
            };
            let matches = {
                let a: Vec<char> = sel_text.chars().collect();
                a.len() == sel.len()
                    && a.iter()
                        .zip(&sel)
                        .all(|(&x, &y)| chars_eq(x, y, ctl.case_sens.get_untracked()))
            };
            if matches {
                history_commit(ctl);
                let _ = ta.set_range_text_with_start_and_end(&rep, start, end);
                let new_pos = start + rep.encode_utf16().count() as u32;
                let _ = ta.set_selection_range(new_pos, new_pos);
                ctl.source.set(ta.value());
                mark_dirty(ctl);
                recount(ctl);
                history_sync(ctl);
            }
            ta_find(ctl, false);
        }
    }
}

fn replace_all(ctl: Ctl) {
    let q = ctl.find_q.get_untracked();
    if q.is_empty() {
        return;
    }
    let rep = ctl.replace_q.get_untracked();
    let cs = ctl.case_sens.get_untracked();
    match ctl.mode.get_untracked() {
        Mode::Wysiwyg => {
            let total = count_matches(&visible_text(ctl), &q, cs);
            if total > 0 {
                history_commit(ctl);
            }
            focus_current(ctl);
            for _ in 0..total {
                if !tauri_api::window_find(&q, cs, false, true) {
                    break;
                }
                if selection_matches_query(ctl) {
                    exec_val("insertText", &rep);
                }
            }
            if total > 0 {
                mark_dirty(ctl);
                recount(ctl);
                history_sync(ctl);
            }
        }
        Mode::Source => {
            let Some(ta) = ta_el(ctl) else { return };
            let (new_val, count) = replace_all_str(&ta.value(), &q, &rep, cs);
            if count > 0 {
                history_commit(ctl);
                ta.set_value(&new_val);
                ctl.source.set(new_val);
                mark_dirty(ctl);
                recount(ctl);
                history_sync(ctl);
            }
        }
    }
}

// ---------- formatting ----------

fn save_selection(ctl: Ctl) {
    let range = window()
        .get_selection()
        .ok()
        .flatten()
        .and_then(|s| s.get_range_at(0).ok())
        .and_then(|r| r.clone_range().dyn_into::<web_sys::Range>().ok());
    ctl.saved_range.set_value(range);
}

fn restore_selection(ctl: Ctl) {
    focus_current(ctl);
    if let Some(range) = ctl.saved_range.get_value() {
        if let Some(sel) = window().get_selection().ok().flatten() {
            let _ = sel.remove_all_ranges();
            let _ = sel.add_range(&range);
        }
    }
}

fn open_url_modal(ctl: Ctl, kind: UrlKind) {
    save_selection(ctl);
    ctl.url_text.set(selection_string());
    ctl.url_val.set(String::new());
    ctl.url_modal.set(Some(kind));
    request_animation_frame(move || {
        if let Some(inp) = ctl.url_ref.get_untracked() {
            let _ = inp.focus();
        }
    });
}

fn confirm_url_modal(ctl: Ctl) {
    let Some(kind) = ctl.url_modal.get_untracked() else {
        return;
    };
    let url = ctl.url_val.get_untracked();
    let text = ctl.url_text.get_untracked();
    ctl.url_modal.set(None);
    if url.is_empty() {
        return;
    }
    match ctl.mode.get_untracked() {
        Mode::Wysiwyg => {
            restore_selection(ctl);
            history_commit(ctl);
            match kind {
                UrlKind::Link => {
                    let sel = selection_string();
                    if !sel.is_empty() && sel == text {
                        exec_val("createLink", &url);
                    } else {
                        let label = if text.is_empty() { url.clone() } else { text };
                        exec_val(
                            "insertHTML",
                            &format!(
                                "<a href=\"{}\">{}</a>",
                                html_escape(&url),
                                html_escape(&label)
                            ),
                        );
                    }
                }
                UrlKind::Image => {
                    exec_val(
                        "insertHTML",
                        &format!(
                            "<img src=\"{}\" alt=\"{}\">",
                            html_escape(&url),
                            html_escape(&text)
                        ),
                    );
                }
            }
            mark_dirty(ctl);
            recount(ctl);
            history_sync(ctl);
        }
        Mode::Source => {
            let md = match kind {
                UrlKind::Link => format!("[{}]({})", if text.is_empty() { &url } else { &text }, url),
                UrlKind::Image => format!("![{text}]({url})"),
            };
            ta_replace_selection(ctl, &md);
        }
    }
}

fn ta_replace_selection(ctl: Ctl, text: &str) {
    let Some(ta) = ta_el(ctl) else { return };
    history_commit(ctl);
    let _ = ta.focus();
    let start = ta.selection_start().ok().flatten().unwrap_or(0);
    let end = ta.selection_end().ok().flatten().unwrap_or(0);
    let _ = ta.set_range_text_with_start_and_end(text, start, end);
    let pos = start + text.encode_utf16().count() as u32;
    let _ = ta.set_selection_range(pos, pos);
    ctl.source.set(ta.value());
    mark_dirty(ctl);
    recount(ctl);
    history_sync(ctl);
}

fn ta_surround(ctl: Ctl, prefix: &str, suffix: &str) {
    let Some(ta) = ta_el(ctl) else { return };
    history_commit(ctl);
    let _ = ta.focus();
    let start = ta.selection_start().ok().flatten().unwrap_or(0);
    let end = ta.selection_end().ok().flatten().unwrap_or(0);
    let _ = ta.set_range_text_with_start_and_end(suffix, end, end);
    let _ = ta.set_range_text_with_start_and_end(prefix, start, start);
    let plen = prefix.encode_utf16().count() as u32;
    let _ = ta.set_selection_range(start + plen, end + plen);
    ctl.source.set(ta.value());
    mark_dirty(ctl);
    recount(ctl);
    history_sync(ctl);
}

/// Apply a transformation to every line touched by the selection.
fn ta_lines(ctl: Ctl, f: &dyn Fn(usize, &str) -> String) {
    let Some(ta) = ta_el(ctl) else { return };
    history_commit(ctl);
    let _ = ta.focus();
    let value = ta.value();
    let chars: Vec<char> = value.chars().collect();
    let sel_start = ta.selection_start().ok().flatten().unwrap_or(0) as usize;
    let sel_end = ta.selection_end().ok().flatten().unwrap_or(0) as usize;
    // Map utf16 offsets to char indices.
    let mut u = 0usize;
    let mut start_ci = chars.len();
    let mut end_ci = chars.len();
    for (ci, c) in chars.iter().enumerate() {
        if u >= sel_start && start_ci == chars.len() {
            start_ci = ci;
        }
        if u >= sel_end && end_ci == chars.len() {
            end_ci = ci;
        }
        u += c.len_utf16();
    }
    // Expand to line boundaries.
    let line_start = chars[..start_ci]
        .iter()
        .rposition(|&c| c == '\n')
        .map(|p| p + 1)
        .unwrap_or(0);
    let line_end = chars[end_ci..]
        .iter()
        .position(|&c| c == '\n')
        .map(|p| end_ci + p)
        .unwrap_or(chars.len());
    let segment: String = chars[line_start..line_end].iter().collect();
    let new_segment = segment
        .split('\n')
        .enumerate()
        .map(|(i, l)| f(i, l))
        .collect::<Vec<_>>()
        .join("\n");
    let from = utf16_len(&chars[..line_start]);
    let to = utf16_len(&chars[..line_end]);
    let _ = ta.set_range_text_with_start_and_end(&new_segment, from, to);
    let end_pos = from + new_segment.encode_utf16().count() as u32;
    let _ = ta.set_selection_range(from, end_pos);
    ctl.source.set(ta.value());
    mark_dirty(ctl);
    recount(ctl);
    history_sync(ctl);
}

fn strip_block_prefix(l: &str) -> &str {
    let mut s = l.trim_start();
    loop {
        let before = s;
        s = s.trim_start_matches('#').trim_start();
        if let Some(r) = s.strip_prefix("> ") {
            s = r;
        }
        if let Some(r) = s.strip_prefix("- [ ] ").or_else(|| s.strip_prefix("- [x] ")) {
            s = r;
        } else if let Some(r) = s.strip_prefix("- ") {
            s = r;
        }
        if s == before {
            break;
        }
    }
    s
}

/// Starter diagram for Insert → Mermaid Diagram.
const MERMAID_MD: &str = "graph TD\n  A[Start] --> B{Choice}\n  B -->|yes| C[Do it]\n  B -->|no| D[Stop]\n";

const TABLE_MD: &str = "\n| Col 1 | Col 2 | Col 3 |\n| --- | --- | --- |\n|  |  |  |\n|  |  |  |\n";
const TABLE_HTML: &str = "<table><thead><tr><th>Col 1</th><th>Col 2</th><th>Col 3</th></tr></thead><tbody><tr><td><br></td><td><br></td><td><br></td></tr><tr><td><br></td><td><br></td><td><br></td></tr></tbody></table><p><br></p>";

fn fmt_action(ctl: Ctl, action: &str) {
    // Modal-opening actions mutate nothing yet; they commit on confirm.
    if matches!(action, "fmt_link" | "fmt_image") {
        open_url_modal(
            ctl,
            if action == "fmt_image" {
                UrlKind::Image
            } else {
                UrlKind::Link
            },
        );
        return;
    }
    let wys = ctl.mode.get_untracked() == Mode::Wysiwyg;
    if wys {
        focus_current(ctl);
        history_commit(ctl);
        match action {
            "fmt_bold" => exec("bold"),
            "fmt_italic" => exec("italic"),
            "fmt_strike" => exec("strikeThrough"),
            "fmt_code" => {
                let sel = selection_string();
                if !sel.is_empty() {
                    exec_val("insertHTML", &format!("<code>{}</code>", html_escape(&sel)));
                }
            }
            "fmt_h1" => exec_val("formatBlock", "<h1>"),
            "fmt_h2" => exec_val("formatBlock", "<h2>"),
            "fmt_h3" => exec_val("formatBlock", "<h3>"),
            "fmt_p" => exec_val("formatBlock", "<p>"),
            "fmt_ul" => exec("insertUnorderedList"),
            "fmt_ol" => exec("insertOrderedList"),
            "fmt_task" => exec_val(
                "insertHTML",
                "<ul><li><input type=\"checkbox\"> task</li></ul>",
            ),
            "fmt_quote" => exec_val("formatBlock", "<blockquote>"),
            "fmt_codeblock" => exec_val("formatBlock", "<pre>"),
            "fmt_table" => exec_val("insertHTML", TABLE_HTML),
            "fmt_mermaid" => exec_val(
                "insertHTML",
                &format!("{}<p><br></p>", crate::mermaid::render_block(MERMAID_MD)),
            ),
            "fmt_hr" => exec("insertHorizontalRule"),
            _ => return,
        }
        mark_dirty(ctl);
        recount(ctl);
        history_sync(ctl);
    } else {
        match action {
            "fmt_bold" => ta_surround(ctl, "**", "**"),
            "fmt_italic" => ta_surround(ctl, "*", "*"),
            "fmt_strike" => ta_surround(ctl, "~~", "~~"),
            "fmt_code" => ta_surround(ctl, "`", "`"),
            "fmt_h1" => ta_lines(ctl, &|_, l| format!("# {}", strip_block_prefix(l))),
            "fmt_h2" => ta_lines(ctl, &|_, l| format!("## {}", strip_block_prefix(l))),
            "fmt_h3" => ta_lines(ctl, &|_, l| format!("### {}", strip_block_prefix(l))),
            "fmt_p" => ta_lines(ctl, &|_, l| strip_block_prefix(l).to_string()),
            "fmt_ul" => ta_lines(ctl, &|_, l| format!("- {}", strip_block_prefix(l))),
            "fmt_ol" => ta_lines(ctl, &|i, l| format!("{}. {}", i + 1, strip_block_prefix(l))),
            "fmt_task" => ta_lines(ctl, &|_, l| format!("- [ ] {}", strip_block_prefix(l))),
            "fmt_quote" => ta_lines(ctl, &|_, l| format!("> {l}")),
            "fmt_codeblock" => ta_surround(ctl, "```\n", "\n```"),
            "fmt_table" => ta_replace_selection(ctl, TABLE_MD),
            "fmt_mermaid" => {
                ta_replace_selection(ctl, &format!("\n```mermaid\n{MERMAID_MD}```\n"))
            }
            "fmt_hr" => ta_replace_selection(ctl, "\n\n---\n\n"),
            _ => {}
        }
    }
}

// ---------- mermaid diagrams ----------

/// Class on the textarea a diagram is swapped for while being edited.
const MERMAID_SOURCE_CLASS: &str = "mermaid-source";
/// The source that textarea opened with, for change detection on close.
const MERMAID_ORIG_ATTR: &str = "data-mermaid-orig";

/// The open diagram-source textarea, if one has focus.
fn focused_mermaid_source() -> Option<HtmlTextAreaElement> {
    let el = document().active_element()?;
    if !el.class_list().contains(MERMAID_SOURCE_CLASS) {
        return None;
    }
    el.dyn_into::<HtmlTextAreaElement>().ok()
}

/// Swap a rendered diagram for a textarea holding its markdown source.
/// Clicking the diagram is the way in; losing focus puts the diagram back.
fn open_mermaid_source(ctl: Ctl, wrapper: &Element) -> Option<()> {
    let src = wrapper.get_attribute(crate::mermaid::SOURCE_ATTR)?;
    let src = src.trim_end_matches('\n');
    let parent = wrapper.parent_node()?;
    // Snapshot the diagram as it stands — this is what undo returns to.
    history_sync(ctl);
    let ta = document()
        .create_element("textarea")
        .ok()?
        .dyn_into::<HtmlTextAreaElement>()
        .ok()?;
    ta.set_class_name(MERMAID_SOURCE_CLASS);
    // Keep the surrounding contenteditable from treating this as text.
    let _ = ta.set_attribute("contenteditable", "false");
    // Kept so closing an untouched diagram doesn't push a dead undo step.
    let _ = ta.set_attribute(MERMAID_ORIG_ATTR, src);
    let _ = ta.set_attribute("rows", &(src.lines().count() + 1).max(4).to_string());
    ta.set_spellcheck(false);
    ta.set_value(src);
    parent.replace_child(ta.unchecked_ref(), wrapper).ok()?;
    let _ = ta.focus();
    Some(())
}

/// Render the open textarea back into a diagram. Blanking the source deletes
/// the block, which is the only way to remove a diagram from WYSIWYG mode.
fn close_mermaid_source(ctl: Ctl, ta: &HtmlTextAreaElement) -> Option<()> {
    let src = ta.value();
    let changed = ta.get_attribute(MERMAID_ORIG_ATTR).as_deref() != Some(src.as_str());
    let parent = ta.parent_node()?;
    if src.trim().is_empty() {
        parent.remove_child(ta).ok()?;
    } else {
        let holder = document().create_element("div").ok()?;
        holder.set_inner_html(&crate::mermaid::render_block(&src));
        let node = holder.first_child()?;
        parent.replace_child(&node, ta).ok()?;
    }
    if !changed {
        return Some(());
    }
    history_commit(ctl);
    mark_dirty(ctl);
    recount(ctl);
    history_sync(ctl);
    Some(())
}

// ---------- table operations ----------

fn selection_element() -> Option<Element> {
    let sel = window().get_selection().ok().flatten()?;
    let node = sel.anchor_node()?;
    node.dyn_ref::<Element>()
        .cloned()
        .or_else(|| node.parent_element())
}

/// (cell, row, table) — only for cells inside the WYSIWYG editor.
fn cell_from(el: &Element) -> Option<(Element, Element, Element)> {
    let cell = el.closest("td,th").ok().flatten()?;
    cell.closest(".editor.wysiwyg").ok().flatten()?;
    let row = cell.closest("tr").ok().flatten()?;
    let table = cell.closest("table").ok().flatten()?;
    Some((cell, row, table))
}

/// The cell an operation targets: the right-clicked cell while the context
/// menu is open, otherwise the selection's cell.
fn active_cell(ctl: Ctl) -> Option<(Element, Element, Element)> {
    if let Some(c) = ctl.ctx_cell.get_value() {
        return cell_from(&c);
    }
    selection_element().and_then(|e| cell_from(&e))
}

fn row_cells(row: &Element) -> Vec<Element> {
    children(row.unchecked_ref())
        .into_iter()
        .filter_map(|n| n.dyn_into::<Element>().ok())
        .filter(|e| {
            let t = e.tag_name().to_uppercase();
            t == "TD" || t == "TH"
        })
        .collect()
}

fn children(node: &web_sys::Node) -> Vec<web_sys::Node> {
    let list = node.child_nodes();
    (0..list.length()).filter_map(|i| list.item(i)).collect()
}

fn caret_into(el: &Element) {
    let _ = el.scroll_into_view_with_bool(false);
    if let (Ok(range), Ok(Some(sel))) = (document().create_range(), window().get_selection()) {
        let _ = range.set_start(el.unchecked_ref::<web_sys::Node>(), 0);
        range.collapse_with_to_start(true);
        let _ = sel.remove_all_ranges();
        let _ = sel.add_range(&range);
    }
}

fn new_cell(tag: &str) -> Option<Element> {
    let el = document().create_element(tag).ok()?;
    el.set_inner_html("<br>");
    Some(el)
}

fn table_changed(ctl: Ctl) {
    mark_dirty(ctl);
    recount(ctl);
    history_sync(ctl);
}

fn table_insert_row(ctl: Ctl, below: bool) {
    let Some((_, row, table)) = active_cell(ctl) else {
        return;
    };
    history_commit(ctl);
    let cols = row_cells(&row).len().max(1);
    let Ok(tr) = document().create_element("tr") else {
        return;
    };
    for _ in 0..cols {
        if let Some(td) = new_cell("td") {
            let _ = tr.append_child(&td);
        }
    }
    let in_head = row.closest("thead").ok().flatten().is_some();
    if in_head {
        // The header must stay first; both directions insert at the top of
        // the body.
        if let Some(tbody) = table.query_selector("tbody").ok().flatten() {
            let _ = tbody.insert_before(&tr, tbody.first_child().as_ref());
        } else {
            let _ = table.append_child(&tr);
        }
    } else if below {
        let _ = row.after_with_node_1(&tr);
    } else {
        let _ = row.before_with_node_1(&tr);
    }
    if let Some(first) = row_cells(&tr).into_iter().next() {
        caret_into(&first);
    }
    table_changed(ctl);
}

fn table_insert_col(ctl: Ctl, right: bool) {
    let Some((cell, row, table)) = active_cell(ctl) else {
        return;
    };
    let cells = row_cells(&row);
    let Some(idx) = cells.iter().position(|c| c == &cell) else {
        return;
    };
    history_commit(ctl);
    let insert_at = if right { idx + 1 } else { idx };
    let Ok(rows) = table.query_selector_all("tr") else {
        return;
    };
    let mut caret_target: Option<Element> = None;
    for i in 0..rows.length() {
        let Some(tr) = rows.item(i).and_then(|n| n.dyn_into::<Element>().ok()) else {
            continue;
        };
        let in_head = tr.closest("thead").ok().flatten().is_some();
        let Some(nc) = new_cell(if in_head { "th" } else { "td" }) else {
            continue;
        };
        let tr_cells = row_cells(&tr);
        let at = insert_at.min(tr_cells.len());
        if at >= tr_cells.len() {
            let _ = tr.append_child(&nc);
        } else {
            let _ = tr.insert_before(&nc, Some(tr_cells[at].unchecked_ref()));
        }
        if tr == row {
            caret_target = Some(nc);
        }
    }
    if let Some(c) = caret_target {
        caret_into(&c);
    }
    table_changed(ctl);
}

fn table_delete_row(ctl: Ctl) {
    let Some((_, row, _)) = active_cell(ctl) else {
        return;
    };
    if row.closest("thead").ok().flatten().is_some() {
        return; // a GFM table cannot lose its header row
    }
    history_commit(ctl);
    row.remove();
    table_changed(ctl);
}

fn table_delete_col(ctl: Ctl) {
    let Some((cell, row, table)) = active_cell(ctl) else {
        return;
    };
    let cells = row_cells(&row);
    if cells.len() <= 1 {
        history_commit(ctl);
        table.remove();
        table_changed(ctl);
        return;
    }
    let Some(idx) = cells.iter().position(|c| c == &cell) else {
        return;
    };
    let Ok(rows) = table.query_selector_all("tr") else {
        return;
    };
    history_commit(ctl);
    for i in 0..rows.length() {
        let Some(tr) = rows.item(i).and_then(|n| n.dyn_into::<Element>().ok()) else {
            continue;
        };
        let tr_cells = row_cells(&tr);
        if let Some(c) = tr_cells.get(idx) {
            c.remove();
        }
    }
    table_changed(ctl);
}

fn table_delete_table(ctl: Ctl) {
    let Some((_, _, table)) = active_cell(ctl) else {
        return;
    };
    history_commit(ctl);
    table.remove();
    table_changed(ctl);
}

fn update_in_table(ctl: Ctl) {
    let inside = ctl.mode.get_untracked() == Mode::Wysiwyg
        && selection_element().and_then(|e| cell_from(&e)).is_some();
    if ctl.in_table.get_untracked() != inside {
        ctl.in_table.set(inside);
    }
}

fn close_ctx_menu(ctl: Ctl) {
    ctl.ctx_menu.set(None);
    ctl.ctx_cell.set_value(None);
}

// ---------- smart paste ----------

/// Insert markdown at the caret of the WYSIWYG editor, rendering it first.
/// A single inline-only block is unwrapped so small pastes don't split the
/// current paragraph. Returns true for that inline case.
fn insert_markdown_at_caret(md: &str) -> bool {
    let html = to_html(md);
    let trimmed = html.trim();
    let single_paragraph = !md.trim().contains("\n\n")
        && trimmed.starts_with("<p>")
        && trimmed.ends_with("</p>")
        && trimmed.matches("<p>").count() == 1;
    let insert = if single_paragraph {
        &trimmed[3..trimmed.len() - 4]
    } else {
        trimmed
    };
    exec_val("insertHTML", insert);
    single_paragraph
}

/// Returns the markdown conversion of the clipboard's HTML flavor, unless
/// plain paste was requested or there is no usable HTML.
fn paste_markdown(ctl: Ctl, dt: &web_sys::DataTransfer) -> Option<String> {
    let plain = ctl.plain_paste.get_value();
    ctl.plain_paste.set_value(false);
    if plain {
        return None;
    }
    let html = dt.get_data("text/html").ok()?;
    if html.trim().is_empty() {
        return None;
    }
    let md = html_to_markdown(&html);
    let trimmed = md.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

// ---------- action dispatch ----------

fn do_action(ctl: Ctl, action: &str) {
    if !dedup_ok(action) {
        return;
    }
    match action {
        "new" => guarded(ctl, Pending::New),
        "open" => guarded(ctl, Pending::Open),
        "import_html" => guarded(ctl, Pending::ImportHtml),
        "import_docx" => guarded(ctl, Pending::ImportDocx),
        "save" => do_save(ctl, false, None),
        "save_as" => do_save(ctl, true, None),
        "undo" => history_undo(ctl),
        "redo" => history_redo(ctl),
        "select_all" => {
            focus_current(ctl);
            exec("selectAll");
        }
        "find" => open_find(ctl, false),
        "replace" => open_find(ctl, true),
        "toggle_mode" => toggle_mode(ctl),
        "tbl_row_above" => table_insert_row(ctl, false),
        "tbl_row_below" => table_insert_row(ctl, true),
        "tbl_col_left" => table_insert_col(ctl, false),
        "tbl_col_right" => table_insert_col(ctl, true),
        "tbl_del_row" => table_delete_row(ctl),
        "tbl_del_col" => table_delete_col(ctl),
        "tbl_del_table" => table_delete_table(ctl),
        a if a.starts_with("fmt_") => fmt_action(ctl, a),
        _ => {}
    }
}

// ---------- component ----------

fn tb_btn(
    ctl: Ctl,
    action: &'static str,
    class: &'static str,
    label: &'static str,
    tip: &'static str,
) -> impl IntoView {
    view! {
        <button
            class=format!("tb-btn {class}")
            title=tip
            on:mousedown=|e| e.prevent_default()
            on:click=move |_| do_action(ctl, action)
        >
            {label}
        </button>
    }
}

#[component]
pub fn App() -> impl IntoView {
    let ctl = Ctl {
        mode: RwSignal::new(Mode::Wysiwyg),
        dirty: RwSignal::new(false),
        doc_name: RwSignal::new("Untitled".to_string()),
        doc_path: RwSignal::new(String::new()),
        source: RwSignal::new(String::new()),
        find_open: RwSignal::new(false),
        replace_open: RwSignal::new(false),
        find_q: RwSignal::new(String::new()),
        replace_q: RwSignal::new(String::new()),
        case_sens: RwSignal::new(false),
        match_count: RwSignal::new(0),
        words: RwSignal::new(0),
        chars: RwSignal::new(0),
        pending: RwSignal::new(None),
        url_modal: RwSignal::new(None),
        url_val: RwSignal::new(String::new()),
        url_text: RwSignal::new(String::new()),
        in_table: RwSignal::new(false),
        ctx_menu: RwSignal::new(None),
        editor_ref: NodeRef::new(),
        ta_ref: NodeRef::new(),
        find_ref: NodeRef::new(),
        url_ref: NodeRef::new(),
        saved_range: StoredValue::new_local(None),
        ctx_cell: StoredValue::new_local(None),
        plain_paste: StoredValue::new(false),
        hist: StoredValue::new(Hist::default()),
        can_undo: RwSignal::new(false),
        can_redo: RwSignal::new(false),
    };

    // Track whether the caret sits inside a table (drives the table toolbar
    // group).
    {
        let cb = Closure::<dyn FnMut()>::new(move || update_in_table(ctl));
        let _ = document()
            .add_event_listener_with_callback("selectionchange", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Initialize the editor element once it exists.
    Effect::new(move |_| {
        if let Some(el) = ctl.editor_ref.get() {
            exec_val("defaultParagraphSeparator", "p");
            exec_val("styleWithCSS", "false");
            if el.child_element_count() == 0 {
                el.set_inner_html("<p><br></p>");
                history_reset(ctl);
            }
            let _ = el.focus();
        }
    });

    // Load a CLI-passed file and hook up backend events.
    spawn_local(async move {
        if let Ok(v) = tauri_api::invoke_no_args("init_doc").await {
            if let Some(info) = tauri_api::parse_file_info(v) {
                load_markdown(ctl, &info.content);
                ctl.doc_name.set(info.name);
                ctl.doc_path.set(info.path);
            }
        }
        let menu_cb = Closure::<dyn FnMut(JsValue)>::new(move |ev: JsValue| {
            if let Ok(p) = js_sys::Reflect::get(&ev, &JsValue::from_str("payload")) {
                if let Some(id) = p.as_string() {
                    do_action(ctl, &id);
                }
            }
        });
        tauri_api::listen("menu", menu_cb.as_ref().unchecked_ref()).await;
        menu_cb.forget();
        let close_cb = Closure::<dyn FnMut(JsValue)>::new(move |_ev: JsValue| {
            ctl.pending.set(Some(Pending::Close));
        });
        tauri_api::listen("close-requested", close_cb.as_ref().unchecked_ref()).await;
        close_cb.forget();
        // A markdown file was dropped on the window or opened via the OS
        // file association while we're running.
        let drop_cb = Closure::<dyn FnMut(JsValue)>::new(move |ev: JsValue| {
            if let Ok(p) = js_sys::Reflect::get(&ev, &JsValue::from_str("payload")) {
                if let Some(path) = p.as_string() {
                    if ctl.dirty.get_untracked() {
                        ctl.pending.set(Some(Pending::OpenPath(path)));
                    } else {
                        do_open_path(ctl, path);
                    }
                }
            }
        });
        tauri_api::listen("drop-open", drop_cb.as_ref().unchecked_ref()).await;
        drop_cb.forget();
    });

    // Global shortcuts (also reachable via native menu accelerators; the
    // dedup guard collapses double-fires).
    window_event_listener(leptos::ev::keydown, move |e| {
        let key = e.key();
        // While a diagram's source is open, formatting and mode shortcuts
        // would act on the document behind it. Escape commits and closes;
        // the file actions below stay live (the serializer reads an open
        // textarea as its fence, so saving mid-edit is still correct).
        if let Some(ta) = focused_mermaid_source() {
            if key == "Escape" {
                e.prevent_default();
                let _ = ta.blur();
                return;
            }
            let ctrl = e.ctrl_key() || e.meta_key();
            let k = key.to_lowercase();
            if !ctrl || !matches!(k.as_str(), "n" | "o" | "s") {
                return;
            }
        }
        if key == "Escape" {
            if close_overlays(ctl) {
                e.prevent_default();
            }
            return;
        }
        let ctrl = e.ctrl_key() || e.meta_key();
        if !ctrl {
            return;
        }
        let k = key.to_lowercase();
        if k == "v" && e.shift_key() && !e.alt_key() {
            // Ctrl+Shift+V: the paste event this keystroke triggers should
            // insert plain text. Don't prevent default.
            ctl.plain_paste.set_value(true);
            return;
        }
        let action = match (k.as_str(), e.shift_key(), e.alt_key()) {
            ("n", false, false) => "new",
            ("o", false, false) => "open",
            ("s", false, false) => "save",
            ("s", true, false) => "save_as",
            ("z", false, false) => "undo",
            ("z", true, false) => "redo",
            ("y", false, false) => "redo",
            ("f", false, false) => "find",
            ("f", false, true) => "replace",
            ("h", false, false) => "replace",
            ("m", true, false) => "toggle_mode",
            ("b", false, false) => "fmt_bold",
            ("i", false, false) => "fmt_italic",
            ("e", false, false) => "fmt_code",
            ("k", false, false) => "fmt_link",
            ("x", true, false) => "fmt_strike",
            ("1", false, false) => "fmt_h1",
            ("2", false, false) => "fmt_h2",
            ("3", false, false) => "fmt_h3",
            ("0", false, false) => "fmt_p",
            ("u", false, false) => {
                // no underline in markdown
                e.prevent_default();
                return;
            }
            _ => return,
        };
        e.prevent_default();
        do_action(ctl, action);
    });

    let on_editor_input = move |_| {
        history_on_input(ctl);
        mark_dirty(ctl);
        recount(ctl);
        history_sync(ctl);
    };

    // The webview keeps its own history; reroute it to ours (context-menu
    // Undo, macOS gesture undo, etc.).
    let on_before_input = move |e: web_sys::InputEvent| {
        match e.input_type().as_str() {
            "historyUndo" => {
                e.prevent_default();
                do_action(ctl, "undo");
            }
            "historyRedo" => {
                e.prevent_default();
                do_action(ctl, "redo");
            }
            _ => {}
        }
    };

    let on_editor_keydown = move |e: web_sys::KeyboardEvent| {
        // Inside a diagram's source the textarea owns the keyboard; Tab there
        // means "indent this line", not "indent the surrounding list".
        if focused_mermaid_source().is_some() {
            return;
        }
        if e.key() == "Tab" {
            e.prevent_default();
            history_commit(ctl);
            let in_li = selection_in("li");
            if e.shift_key() {
                if in_li {
                    exec("outdent");
                }
            } else if in_li {
                exec("indent");
            } else {
                exec_val("insertText", "    ");
            }
            mark_dirty(ctl);
        }
    };

    let on_editor_paste = move |e: web_sys::ClipboardEvent| {
        e.prevent_default();
        if let Some(dt) = e.clipboard_data() {
            history_commit(ctl);
            if let Some(md) = paste_markdown(ctl, &dt) {
                let inline = insert_markdown_at_caret(&md);
                if inline {
                    // Markdown conversion trims edges; restore a trailing
                    // space so words don't get glued together.
                    let plain = dt.get_data("text/plain").unwrap_or_default();
                    if plain.ends_with(' ') || plain.ends_with('\u{a0}') {
                        exec_val("insertText", " ");
                    }
                }
                enable_checkboxes(ctl);
            } else if let Ok(text) = dt.get_data("text/plain") {
                if !text.is_empty() {
                    exec_val("insertText", &text);
                }
            }
        }
        mark_dirty(ctl);
        recount(ctl);
    };

    let on_editor_click = move |e: web_sys::MouseEvent| {
        if let Some(t) = e.target() {
            // Toggle task checkboxes ourselves — native toggling inside
            // contenteditable is engine-dependent. The checkbox pre-toggles
            // before `click` fires and preventDefault reverts it after the
            // handler returns, so the flip must happen post-dispatch.
            if let Some(input) = t.dyn_ref::<web_sys::HtmlInputElement>() {
                if input.type_() == "checkbox" {
                    e.prevent_default();
                    history_commit(ctl);
                    let input = input.clone();
                    set_timeout(
                        move || {
                            input.set_checked(!input.checked());
                            history_sync(ctl);
                        },
                        std::time::Duration::ZERO,
                    );
                    mark_dirty(ctl);
                    return;
                }
            }
            if let Some(el) = t.dyn_ref::<Element>() {
                // Clicking a diagram opens its source for editing. The target
                // may be an SVG shape, so match on the nearest wrapper.
                let sel = format!(".{}", crate::mermaid::WRAPPER_CLASS);
                if let Some(w) = el.closest(&sel).ok().flatten() {
                    e.prevent_default();
                    open_mermaid_source(ctl, &w);
                    return;
                }
                if el.closest("a").ok().flatten().is_some() {
                    e.prevent_default();
                }
            }
        }
    };

    // A diagram's source textarea re-renders when it loses focus. `focusout`
    // bubbles (unlike `blur`), so one handler on the editor covers it.
    let on_editor_focusout = move |e: web_sys::FocusEvent| {
        if let Some(ta) = e
            .target()
            .and_then(|t| t.dyn_into::<HtmlTextAreaElement>().ok())
        {
            if ta.class_name().contains(MERMAID_SOURCE_CLASS) {
                close_mermaid_source(ctl, &ta);
            }
        }
    };

    let on_ta_keydown = move |e: web_sys::KeyboardEvent| {
        if e.key() == "Tab" && !e.shift_key() {
            e.prevent_default();
            ta_replace_selection(ctl, "  ");
        }
    };

    let on_ta_paste = move |e: web_sys::ClipboardEvent| {
        if let Some(dt) = e.clipboard_data() {
            if let Some(md) = paste_markdown(ctl, &dt) {
                e.prevent_default();
                ta_replace_selection(ctl, &md);
            }
            // otherwise: let the native plain-text paste happen
        }
    };

    let on_editor_ctxmenu = move |e: web_sys::MouseEvent| {
        if ctl.mode.get_untracked() != Mode::Wysiwyg {
            return;
        }
        let Some(el) = e
            .target()
            .and_then(|t| t.dyn_into::<Element>().ok())
        else {
            return;
        };
        let Some((cell, _, _)) = cell_from(&el) else {
            return; // outside a table: keep the default context menu
        };
        e.prevent_default();
        ctl.ctx_cell.set_value(Some(cell));
        let win = window();
        let vw = win.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(800.0);
        let vh = win.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(600.0);
        let x = f64::from(e.client_x()).min(vw - 200.0).max(0.0);
        let y = f64::from(e.client_y()).min(vh - 270.0).max(0.0);
        ctl.ctx_menu.set(Some((x, y)));
    };

    let ctx_item = move |action: &'static str, label: &'static str| {
        view! {
            <button
                class="ctx-item"
                on:click=move |_| {
                    do_action(ctl, action);
                    close_ctx_menu(ctl);
                }
            >
                {label}
            </button>
        }
    };

    let mode_is = move |m: Mode| ctl.mode.get() == m;

    view! {
        <div class="app">
            <div class="toolbar">
                <button
                    class="tb-btn"
                    title="Undo (Ctrl+Z)"
                    prop:disabled=move || !ctl.can_undo.get()
                    on:mousedown=|e| e.prevent_default()
                    on:click=move |_| do_action(ctl, "undo")
                >
                    "↶"
                </button>
                <button
                    class="tb-btn"
                    title="Redo (Ctrl+Shift+Z)"
                    prop:disabled=move || !ctl.can_redo.get()
                    on:mousedown=|e| e.prevent_default()
                    on:click=move |_| do_action(ctl, "redo")
                >
                    "↷"
                </button>
                <span class="tb-sep"></span>
                {tb_btn(ctl, "fmt_bold", "tb-b", "B", "Bold (Ctrl+B)")}
                {tb_btn(ctl, "fmt_italic", "tb-i", "I", "Italic (Ctrl+I)")}
                {tb_btn(ctl, "fmt_strike", "tb-s", "S", "Strikethrough (Ctrl+Shift+X)")}
                {tb_btn(ctl, "fmt_code", "tb-mono", "</>", "Inline code (Ctrl+E)")}
                <span class="tb-sep"></span>
                {tb_btn(ctl, "fmt_h1", "", "H1", "Heading 1 (Ctrl+1)")}
                {tb_btn(ctl, "fmt_h2", "", "H2", "Heading 2 (Ctrl+2)")}
                {tb_btn(ctl, "fmt_h3", "", "H3", "Heading 3 (Ctrl+3)")}
                {tb_btn(ctl, "fmt_p", "", "¶", "Paragraph (Ctrl+0)")}
                <span class="tb-sep"></span>
                {tb_btn(ctl, "fmt_ul", "", "•", "Bullet list")}
                {tb_btn(ctl, "fmt_ol", "", "1.", "Numbered list")}
                {tb_btn(ctl, "fmt_task", "", "☑", "Task list")}
                <span class="tb-sep"></span>
                {tb_btn(ctl, "fmt_quote", "", "❝", "Blockquote")}
                {tb_btn(ctl, "fmt_codeblock", "tb-mono", "{ }", "Code block")}
                {tb_btn(ctl, "fmt_link", "", "🔗", "Insert link (Ctrl+K)")}
                {tb_btn(ctl, "fmt_image", "", "🖼", "Insert image")}
                {tb_btn(ctl, "fmt_table", "", "⊞", "Insert table")}
                {tb_btn(ctl, "fmt_hr", "", "―", "Horizontal rule")}
                <span class="tb-group" class:hidden=move || !ctl.in_table.get()>
                    <span class="tb-sep"></span>
                    {tb_btn(ctl, "tbl_row_above", "tb-text", "Row ↑", "Insert row above")}
                    {tb_btn(ctl, "tbl_row_below", "tb-text", "Row ↓", "Insert row below")}
                    {tb_btn(ctl, "tbl_col_left", "tb-text", "Col ←", "Insert column left")}
                    {tb_btn(ctl, "tbl_col_right", "tb-text", "Col →", "Insert column right")}
                    {tb_btn(ctl, "tbl_del_row", "tb-text", "− Row", "Delete row")}
                    {tb_btn(ctl, "tbl_del_col", "tb-text", "− Col", "Delete column")}
                </span>
                <span class="tb-spacer"></span>
                <button
                    class="tb-btn tb-mode"
                    class:active=move || mode_is(Mode::Source)
                    title="Toggle markdown source (Ctrl+Shift+M)"
                    on:mousedown=|e| e.prevent_default()
                    on:click=move |_| do_action(ctl, "toggle_mode")
                >
                    {move || if mode_is(Mode::Source) { "WYSIWYG" } else { "Markdown" }}
                </button>
            </div>

            <div class="findbar" class:hidden=move || !ctl.find_open.get()>
                <input
                    type="text"
                    class="find-input"
                    placeholder="Find"
                    node_ref=ctl.find_ref
                    prop:value=move || ctl.find_q.get()
                    on:input=move |e| {
                        ctl.find_q.set(event_target_value(&e));
                        recount(ctl);
                    }
                    on:keydown=move |e: web_sys::KeyboardEvent| {
                        if e.key() == "Enter" {
                            e.prevent_default();
                            find_next(ctl, e.shift_key());
                        }
                    }
                />
                <span class="match-count">
                    {move || {
                        let q = ctl.find_q.get();
                        if q.is_empty() {
                            String::new()
                        } else {
                            format!("{}", ctl.match_count.get())
                        }
                    }}
                </span>
                <button class="tb-btn" title="Previous (Shift+Enter)" on:click=move |_| find_next(ctl, true)>"↑"</button>
                <button class="tb-btn" title="Next (Enter)" on:click=move |_| find_next(ctl, false)>"↓"</button>
                <label class="case-toggle" title="Match case">
                    <input
                        type="checkbox"
                        prop:checked=move || ctl.case_sens.get()
                        on:change=move |e| {
                            ctl.case_sens.set(event_target_checked(&e));
                            recount(ctl);
                        }
                    />
                    "Aa"
                </label>
                <span class="replace-group" class:hidden=move || !ctl.replace_open.get()>
                    <input
                        type="text"
                        class="find-input"
                        placeholder="Replace with"
                        prop:value=move || ctl.replace_q.get()
                        on:input=move |e| ctl.replace_q.set(event_target_value(&e))
                        on:keydown=move |e: web_sys::KeyboardEvent| {
                            if e.key() == "Enter" {
                                e.prevent_default();
                                replace_one(ctl);
                            }
                        }
                    />
                    <button class="tb-btn tb-text" on:click=move |_| replace_one(ctl)>"Replace"</button>
                    <button class="tb-btn tb-text" on:click=move |_| replace_all(ctl)>"All"</button>
                </span>
                <span class="tb-spacer"></span>
                <button class="tb-btn" title="Close (Esc)" on:click=move |_| { close_overlays(ctl); }>"✕"</button>
            </div>

            <div class="editor-wrap">
                <div
                    class="editor wysiwyg markdown-body"
                    class:hidden=move || !mode_is(Mode::Wysiwyg)
                    contenteditable="true"
                    spellcheck="true"
                    node_ref=ctl.editor_ref
                    on:input=on_editor_input
                    on:beforeinput=on_before_input
                    on:keydown=on_editor_keydown
                    on:paste=on_editor_paste
                    on:click=on_editor_click
                    on:focusout=on_editor_focusout
                    on:contextmenu=on_editor_ctxmenu
                    on:change=move |_| {
                        mark_dirty(ctl);
                    }
                ></div>
                <textarea
                    class="editor source"
                    class:hidden=move || !mode_is(Mode::Source)
                    spellcheck="false"
                    node_ref=ctl.ta_ref
                    prop:value=move || ctl.source.get()
                    on:input=move |e| {
                        history_on_input(ctl);
                        ctl.source.set(event_target_value(&e));
                        mark_dirty(ctl);
                        recount(ctl);
                        history_sync(ctl);
                    }
                    on:beforeinput=on_before_input
                    on:keydown=on_ta_keydown
                    on:paste=on_ta_paste
                ></textarea>
            </div>

            <div class="statusbar">
                <span class="status-path">
                    {move || {
                        let p = ctl.doc_path.get();
                        if p.is_empty() { "unsaved".to_string() } else { p }
                    }}
                </span>
                <span class="tb-spacer"></span>
                <span>{move || format!("{} words", ctl.words.get())}</span>
                <span class="status-sep">"·"</span>
                <span>{move || format!("{} chars", ctl.chars.get())}</span>
                <span class="status-sep">"·"</span>
                <span>{move || if mode_is(Mode::Source) { "Markdown" } else { "WYSIWYG" }}</span>
            </div>

            <Show when=move || ctl.ctx_menu.get().is_some()>
                <div
                    class="ctx-backdrop"
                    on:mousedown=move |_| close_ctx_menu(ctl)
                    on:contextmenu=move |e: web_sys::MouseEvent| {
                        e.prevent_default();
                        close_ctx_menu(ctl);
                    }
                ></div>
                <div
                    class="ctxmenu"
                    style=move || {
                        let (x, y) = ctl.ctx_menu.get().unwrap_or((0.0, 0.0));
                        format!("left:{x}px;top:{y}px")
                    }
                >
                    {ctx_item("tbl_row_above", "Insert row above")}
                    {ctx_item("tbl_row_below", "Insert row below")}
                    {ctx_item("tbl_col_left", "Insert column left")}
                    {ctx_item("tbl_col_right", "Insert column right")}
                    <div class="ctx-sep"></div>
                    {ctx_item("tbl_del_row", "Delete row")}
                    {ctx_item("tbl_del_col", "Delete column")}
                    {ctx_item("tbl_del_table", "Delete table")}
                </div>
            </Show>

            <Show when=move || ctl.pending.get().is_some()>
                <div class="overlay">
                    <div class="modal">
                        <p class="modal-title">
                            {move || format!("Save changes to {}?", ctl.doc_name.get())}
                        </p>
                        <p class="modal-sub">"Your changes will be lost if you don't save them."</p>
                        <div class="modal-btns">
                            <button
                                class="btn primary"
                                on:click=move |_| {
                                    let p = ctl.pending.get_untracked();
                                    ctl.pending.set(None);
                                    if let Some(p) = p {
                                        do_save(ctl, false, Some(p));
                                    }
                                }
                            >
                                "Save"
                            </button>
                            <button
                                class="btn"
                                on:click=move |_| {
                                    let p = ctl.pending.get_untracked();
                                    ctl.pending.set(None);
                                    if let Some(p) = p {
                                        perform_pending(ctl, p);
                                    }
                                }
                            >
                                "Don't Save"
                            </button>
                            <button class="btn" on:click=move |_| ctl.pending.set(None)>
                                "Cancel"
                            </button>
                        </div>
                    </div>
                </div>
            </Show>

            <Show when=move || ctl.url_modal.get().is_some()>
                <div class="overlay">
                    <div class="modal">
                        <p class="modal-title">
                            {move || {
                                match ctl.url_modal.get() {
                                    Some(UrlKind::Image) => "Insert Image",
                                    _ => "Insert Link",
                                }
                            }}
                        </p>
                        <label class="modal-label">"URL"</label>
                        <input
                            type="text"
                            class="modal-input"
                            node_ref=ctl.url_ref
                            placeholder="https://…"
                            prop:value=move || ctl.url_val.get()
                            on:input=move |e| ctl.url_val.set(event_target_value(&e))
                            on:keydown=move |e: web_sys::KeyboardEvent| {
                                if e.key() == "Enter" {
                                    e.prevent_default();
                                    confirm_url_modal(ctl);
                                }
                            }
                        />
                        <label class="modal-label">
                            {move || {
                                match ctl.url_modal.get() {
                                    Some(UrlKind::Image) => "Alt text",
                                    _ => "Text",
                                }
                            }}
                        </label>
                        <input
                            type="text"
                            class="modal-input"
                            prop:value=move || ctl.url_text.get()
                            on:input=move |e| ctl.url_text.set(event_target_value(&e))
                            on:keydown=move |e: web_sys::KeyboardEvent| {
                                if e.key() == "Enter" {
                                    e.prevent_default();
                                    confirm_url_modal(ctl);
                                }
                            }
                        />
                        <div class="modal-btns">
                            <button class="btn primary" on:click=move |_| confirm_url_modal(ctl)>
                                "Insert"
                            </button>
                            <button class="btn" on:click=move |_| ctl.url_modal.set(None)>
                                "Cancel"
                            </button>
                        </div>
                    </div>
                </div>
            </Show>
        </div>
    }
}
