use std::cell::RefCell;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlTextAreaElement};

use crate::markdown::to_html;
use crate::serialize::dom_to_markdown;
use crate::tauri_api::{self, ContentArgs, DirtyArgs};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Wysiwyg,
    Source,
}

#[derive(Clone, Copy, PartialEq)]
enum Pending {
    New,
    Open,
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
    editor_ref: NodeRef<html::Div>,
    ta_ref: NodeRef<html::Textarea>,
    find_ref: NodeRef<html::Input>,
    url_ref: NodeRef<html::Input>,
    saved_range: StoredValue<Option<web_sys::Range>, LocalStorage>,
}

thread_local! {
    static LAST_ACTION: RefCell<(String, f64)> = RefCell::new((String::new(), 0.0));
}

/// Menu accelerators and the in-page keydown handler can both fire for one
/// keypress (platform-dependent); collapse duplicates within a short window.
fn dedup_ok(action: &str) -> bool {
    let now = js_sys::Date::now();
    LAST_ACTION.with(|l| {
        let mut l = l.borrow_mut();
        if l.0 == action && now - l.1 < 200.0 {
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

fn recount(ctl: Ctl) {
    let text = visible_text(ctl);
    ctl.words.set(text.split_whitespace().count());
    ctl.chars.set(text.chars().count());
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

fn perform_pending(ctl: Ctl, p: Pending) {
    match p {
        Pending::New => do_new(ctl),
        Pending::Open => do_open(ctl),
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
    if ctl.url_modal.get_untracked().is_some() {
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
                exec_val("insertText", &rep);
                mark_dirty(ctl);
                recount(ctl);
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
                let _ = ta.set_range_text_with_start_and_end(&rep, start, end);
                let new_pos = start + rep.encode_utf16().count() as u32;
                let _ = ta.set_selection_range(new_pos, new_pos);
                ctl.source.set(ta.value());
                mark_dirty(ctl);
                recount(ctl);
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
            }
        }
        Mode::Source => {
            let Some(ta) = ta_el(ctl) else { return };
            let (new_val, count) = replace_all_str(&ta.value(), &q, &rep, cs);
            if count > 0 {
                ta.set_value(&new_val);
                ctl.source.set(new_val);
                mark_dirty(ctl);
                recount(ctl);
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
    let _ = ta.focus();
    let start = ta.selection_start().ok().flatten().unwrap_or(0);
    let end = ta.selection_end().ok().flatten().unwrap_or(0);
    let _ = ta.set_range_text_with_start_and_end(text, start, end);
    let pos = start + text.encode_utf16().count() as u32;
    let _ = ta.set_selection_range(pos, pos);
    ctl.source.set(ta.value());
    mark_dirty(ctl);
    recount(ctl);
}

fn ta_surround(ctl: Ctl, prefix: &str, suffix: &str) {
    let Some(ta) = ta_el(ctl) else { return };
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
}

/// Apply a transformation to every line touched by the selection.
fn ta_lines(ctl: Ctl, f: &dyn Fn(usize, &str) -> String) {
    let Some(ta) = ta_el(ctl) else { return };
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

const TABLE_MD: &str = "\n| Col 1 | Col 2 | Col 3 |\n| --- | --- | --- |\n|  |  |  |\n|  |  |  |\n";
const TABLE_HTML: &str = "<table><thead><tr><th>Col 1</th><th>Col 2</th><th>Col 3</th></tr></thead><tbody><tr><td><br></td><td><br></td><td><br></td></tr><tr><td><br></td><td><br></td><td><br></td></tr></tbody></table><p><br></p>";

fn fmt_action(ctl: Ctl, action: &str) {
    let wys = ctl.mode.get_untracked() == Mode::Wysiwyg;
    if wys {
        focus_current(ctl);
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
            "fmt_link" => {
                open_url_modal(ctl, UrlKind::Link);
                return;
            }
            "fmt_image" => {
                open_url_modal(ctl, UrlKind::Image);
                return;
            }
            "fmt_table" => exec_val("insertHTML", TABLE_HTML),
            "fmt_hr" => exec("insertHorizontalRule"),
            _ => return,
        }
        mark_dirty(ctl);
        recount(ctl);
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
            "fmt_link" => open_url_modal(ctl, UrlKind::Link),
            "fmt_image" => open_url_modal(ctl, UrlKind::Image),
            "fmt_table" => ta_replace_selection(ctl, TABLE_MD),
            "fmt_hr" => ta_replace_selection(ctl, "\n\n---\n\n"),
            _ => {}
        }
    }
}

// ---------- action dispatch ----------

fn do_action(ctl: Ctl, action: &str) {
    if !dedup_ok(action) {
        return;
    }
    match action {
        "new" => guarded(ctl, Pending::New),
        "open" => guarded(ctl, Pending::Open),
        "save" => do_save(ctl, false, None),
        "save_as" => do_save(ctl, true, None),
        "undo" => {
            focus_current(ctl);
            exec("undo");
            recount(ctl);
        }
        "redo" => {
            focus_current(ctl);
            exec("redo");
            recount(ctl);
        }
        "select_all" => {
            focus_current(ctl);
            exec("selectAll");
        }
        "find" => open_find(ctl, false),
        "replace" => open_find(ctl, true),
        "toggle_mode" => toggle_mode(ctl),
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
        editor_ref: NodeRef::new(),
        ta_ref: NodeRef::new(),
        find_ref: NodeRef::new(),
        url_ref: NodeRef::new(),
        saved_range: StoredValue::new_local(None),
    };

    // Initialize the editor element once it exists.
    Effect::new(move |_| {
        if let Some(el) = ctl.editor_ref.get() {
            exec_val("defaultParagraphSeparator", "p");
            exec_val("styleWithCSS", "false");
            if el.inner_html().is_empty() {
                el.set_inner_html("<p><br></p>");
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
    });

    // Global shortcuts (also reachable via native menu accelerators; the
    // dedup guard collapses double-fires).
    window_event_listener(leptos::ev::keydown, move |e| {
        let key = e.key();
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
        let action = match (k.as_str(), e.shift_key(), e.alt_key()) {
            ("n", false, false) => "new",
            ("o", false, false) => "open",
            ("s", false, false) => "save",
            ("s", true, false) => "save_as",
            ("f", false, false) => "find",
            ("f", false, true) => "replace",
            ("h", false, false) => "replace",
            ("m", true, false) => "toggle_mode",
            ("b", false, false) => "fmt_bold",
            ("i", false, false) => "fmt_italic",
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
        mark_dirty(ctl);
        recount(ctl);
    };

    let on_editor_keydown = move |e: web_sys::KeyboardEvent| {
        if e.key() == "Tab" {
            e.prevent_default();
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
            if let Ok(text) = dt.get_data("text/plain") {
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
                    let input = input.clone();
                    set_timeout(
                        move || input.set_checked(!input.checked()),
                        std::time::Duration::ZERO,
                    );
                    mark_dirty(ctl);
                    return;
                }
            }
            if let Some(el) = t.dyn_ref::<Element>() {
                if el.closest("a").ok().flatten().is_some() {
                    e.prevent_default();
                }
            }
        }
    };

    let on_ta_keydown = move |e: web_sys::KeyboardEvent| {
        if e.key() == "Tab" && !e.shift_key() {
            e.prevent_default();
            ta_replace_selection(ctl, "  ");
        }
    };

    let mode_is = move |m: Mode| ctl.mode.get() == m;

    view! {
        <div class="app">
            <div class="toolbar">
                {tb_btn(ctl, "fmt_bold", "tb-b", "B", "Bold (Ctrl+B)")}
                {tb_btn(ctl, "fmt_italic", "tb-i", "I", "Italic (Ctrl+I)")}
                {tb_btn(ctl, "fmt_strike", "tb-s", "S", "Strikethrough")}
                {tb_btn(ctl, "fmt_code", "tb-mono", "</>", "Inline code")}
                <span class="tb-sep"></span>
                {tb_btn(ctl, "fmt_h1", "", "H1", "Heading 1")}
                {tb_btn(ctl, "fmt_h2", "", "H2", "Heading 2")}
                {tb_btn(ctl, "fmt_h3", "", "H3", "Heading 3")}
                {tb_btn(ctl, "fmt_p", "", "¶", "Paragraph")}
                <span class="tb-sep"></span>
                {tb_btn(ctl, "fmt_ul", "", "•", "Bullet list")}
                {tb_btn(ctl, "fmt_ol", "", "1.", "Numbered list")}
                {tb_btn(ctl, "fmt_task", "", "☑", "Task list")}
                <span class="tb-sep"></span>
                {tb_btn(ctl, "fmt_quote", "", "❝", "Blockquote")}
                {tb_btn(ctl, "fmt_codeblock", "tb-mono", "{ }", "Code block")}
                {tb_btn(ctl, "fmt_link", "", "🔗", "Insert link")}
                {tb_btn(ctl, "fmt_image", "", "🖼", "Insert image")}
                {tb_btn(ctl, "fmt_table", "", "⊞", "Insert table")}
                {tb_btn(ctl, "fmt_hr", "", "―", "Horizontal rule")}
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
                    on:keydown=on_editor_keydown
                    on:paste=on_editor_paste
                    on:click=on_editor_click
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
                        ctl.source.set(event_target_value(&e));
                        mark_dirty(ctl);
                        recount(ctl);
                    }
                    on:keydown=on_ta_keydown
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
