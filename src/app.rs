use std::cell::RefCell;

use base64::prelude::{Engine, BASE64_STANDARD};
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlTextAreaElement};

use crate::export;
use crate::hex;
use crate::history::{self, Snap};
use crate::markdown::to_html;
use crate::serialize::{dom_to_markdown, html_to_markdown};
use crate::tauri_api::{self, SaveArgs, TitleArgs};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Wysiwyg,
    Source,
}

/// What a tab holds. mdedit is a markdown editor, so everything else it will
/// show is deliberately narrow: a picture, some bytes, or plain text.
#[derive(Clone, Copy, PartialEq)]
enum DocKind {
    Markdown,
    Text,
    Image,
    Binary,
    /// Past the size limit for its kind. Nothing was read; the tab explains
    /// why rather than pretending the file is empty.
    Oversized,
}

impl DocKind {
    fn from_str(s: &str) -> Self {
        match s {
            "text" => DocKind::Text,
            "image" => DocKind::Image,
            "binary" => DocKind::Binary,
            "oversized" => DocKind::Oversized,
            _ => DocKind::Markdown,
        }
    }

    /// Held as text, and so served by the editor pair, undo history, find and
    /// the three-way merge.
    fn is_text(self) -> bool {
        matches!(self, DocKind::Markdown | DocKind::Text)
    }
}

/// The only two operations that can still destroy unsaved work. Opening and
/// importing never discard anything — they open a new tab.
#[derive(Clone, PartialEq)]
enum Pending {
    /// Doc id, not index: indices shift when a tab closes.
    CloseTab(u32),
    CloseWindow,
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
    /// Last (name, dirty, any_dirty) pushed to the backend; suppresses
    /// redundant title IPC.
    last_status: StoredValue<Option<Status>>,
    /// Every open document, in tab order. Index-addressed by `active`.
    ///
    /// Two rules, both of which bite silently when broken:
    /// never write a signal while a `docs` borrow is live (`StoredValue` is a
    /// `RefCell`, and a synchronous notification that reads `docs` panics on
    /// double-borrow); and never reorder `docs` without `publish_tabs`.
    docs: StoredValue<Vec<Doc>>,
    /// Reactive projection of `docs` for the tab strip. Republished on
    /// open / close / reorder only — renaming a tab repaints one span.
    tabs: RwSignal<Vec<TabView>>,
    /// `None` = no document open, i.e. the empty frame.
    active: RwSignal<Option<usize>>,
    next_id: StoredValue<u32>,
    /// Bumped by every `activate`; a queued frame that finds a stale
    /// generation drops its caret/scroll restore.
    switch_gen: StoredValue<u32>,
    tab_menu: RwSignal<Option<(f64, f64)>>,
    tabs_overflow: RwSignal<bool>,
    tabstrip_ref: NodeRef<html::Div>,
    /// Id of the document whose "changed on disk" prompt is showing. An id
    /// rather than a bool because the answer is applied asynchronously, by
    /// which time the active tab may have moved.
    disk_modal: RwSignal<Option<u32>>,
    // ---- the active document, beyond markdown ----
    kind: RwSignal<DocKind>,
    /// Bytes on disk. Only shown, but shown for every kind.
    doc_size: RwSignal<u64>,
    /// `data:` URL of the active image.
    data: RwSignal<String>,
    /// The active binary. A `StoredValue` because a megabyte of `Vec<u8>` must
    /// not be cloned on every read, with `hex_gen` standing in as the signal
    /// that something in it changed.
    hex: StoredValue<hex::HexDoc>,
    hex_gen: RwSignal<u32>,
    /// Cursor offset in bytes, and which nibble of that byte is next.
    hex_cursor: RwSignal<usize>,
    hex_high: RwSignal<bool>,
    /// Overwrite rather than insert. The default, and the safe one: it cannot
    /// shift every offset in a structured file.
    hex_overwrite: RwSignal<bool>,
    /// First rendered row, and how many fit. A megabyte is 65536 rows, so only
    /// the visible ones exist in the DOM.
    hex_top: RwSignal<usize>,
    hex_rows: RwSignal<usize>,
    hex_ref: NodeRef<html::Div>,
    // ---- the project pane ----
    project_open: RwSignal<bool>,
    project_root: RwSignal<String>,
    /// The tree, flattened to its visible rows. Expanding a directory splices
    /// its children in behind it; collapsing removes everything deeper that
    /// follows. One `<For>`, no recursion, no nested components.
    tree: RwSignal<Vec<TreeNode>>,
    /// Last path set pushed to the watcher; suppresses redundant IPC the same
    /// way `last_status` does.
    last_watched: StoredValue<Vec<String>>,
    /// The app's root reactive owner. A `Doc`'s signals must be created under
    /// it rather than under whatever owner happens to be current: a tab
    /// opened from the empty-state button would otherwise belong to that
    /// `<Show>` branch, and be disposed the instant a document exists.
    owner: StoredValue<Option<Owner>, LocalStorage>,
}

/// The stashed state of one document. The *active* document's live state
/// stays in the plain `Ctl` fields, so every existing `fn foo(ctl: Ctl)` keeps
/// working on "the current document" unchanged; `stash_active` / `activate`
/// move it here and back.
///
/// `snap`, `scroll` and `hist` of the active entry are stale by definition —
/// only `stash_active` writes them.
struct Doc {
    /// Stable across close and reorder. Keys the `<For>` and names a tab in
    /// `Pending::CloseTab`.
    id: u32,
    name: RwSignal<String>,
    /// Empty for a never-saved document; the save target.
    path: RwSignal<String>,
    dirty: RwSignal<bool>,
    /// The file changed on disk and the user hasn't answered for it yet. Only
    /// the active document is ever prompted about, so a background tab carries
    /// this until you switch to it.
    stale: RwSignal<bool>,
    /// The markdown this tab and its file last agreed on — set on open, save,
    /// reload and merge. The common ancestor of a three-way merge, and
    /// meaningless (empty) for a document with no path.
    base: String,
    /// Content, mode and caret as verbatim HTML — never markdown. Round-
    /// tripping through `dom_to_markdown` would quietly rewrite the document
    /// on every tab switch.
    snap: Snap,
    scroll: i32,
    hist: Hist,
    /// Fixed when the tab opens: nothing turns a picture into markdown.
    kind: DocKind,
    size: u64,
    /// `data:` URL, for an image.
    data: String,
    /// Bytes and their own operation-based undo, for a binary.
    hex: hex::HexDoc,
}

/// What the backend was last told about the active tab: the window title, and
/// which menu items mean anything for it.
#[derive(Clone, PartialEq)]
struct Status {
    name: String,
    dirty: bool,
    any_dirty: bool,
    markdown: bool,
    text: bool,
}

/// One visible row of the project tree.
#[derive(Clone, PartialEq)]
struct TreeNode {
    path: String,
    name: String,
    dir: bool,
    /// Nesting level: the row's indent, and how a collapse knows how much of
    /// what follows to remove.
    depth: usize,
    open: bool,
}

/// The cheap `Copy` projection the tab strip iterates over. `Doc` itself can
/// never live in a signal: `RwSignal::get` requires `Clone`, and cloning
/// `Vec<Doc>` on every repaint would copy every document's HTML and its
/// entire undo stack.
#[derive(Clone, Copy, PartialEq)]
struct TabView {
    id: u32,
    name: RwSignal<String>,
    dirty: RwSignal<bool>,
    stale: RwSignal<bool>,
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
    // Holding Ctrl and tapping Tab is a deliberate sub-200ms repeat, so tab
    // cycling shares undo/redo's tighter window. `close_tab` deliberately
    // stays at 200ms: it is the one action where a double-fire would destroy
    // a second document.
    let window_ms = if matches!(action, "undo" | "redo" | "next_tab" | "prev_tab") {
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

/// The mode the active document is actually shown in.
///
/// The WYSIWYG/source toggle is a *markdown* idea: a plain text file has no
/// rendered form, so it is always source, whatever the toggle last said. This
/// keeps `ctl.mode` meaning "how markdown is shown" rather than becoming a
/// per-tab setting that a `.rs` file could leave switched the wrong way.
fn eff_mode(ctl: Ctl) -> Mode {
    match ctl.kind.get_untracked() {
        DocKind::Text => Mode::Source,
        _ => ctl.mode.get_untracked(),
    }
}

/// Reactive counterparts, for the view.
fn shows_wysiwyg(ctl: Ctl) -> bool {
    has_doc(ctl) && ctl.kind.get() == DocKind::Markdown && ctl.mode.get() == Mode::Wysiwyg
}

fn shows_source(ctl: Ctl) -> bool {
    has_doc(ctl)
        && match ctl.kind.get() {
            DocKind::Text => true,
            DocKind::Markdown => ctl.mode.get() == Mode::Source,
            _ => false,
        }
}

fn editor_el(ctl: Ctl) -> Option<web_sys::HtmlDivElement> {
    ctl.editor_ref.get_untracked()
}

fn ta_el(ctl: Ctl) -> Option<HtmlTextAreaElement> {
    ctl.ta_ref.get_untracked()
}

fn focus_current(ctl: Ctl) {
    match ctl.kind.get_untracked() {
        DocKind::Binary => {
            if let Some(el) = ctl.hex_ref.get_untracked() {
                let _ = el.focus();
            }
            return;
        }
        DocKind::Image | DocKind::Oversized => return,
        _ => {}
    }
    match eff_mode(ctl) {
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
    match eff_mode(ctl) {
        Mode::Wysiwyg => editor_el(ctl)
            .and_then(|e| e.text_content())
            .unwrap_or_default(),
        // The signal, not the textarea: `prop:value` is applied by a queued
        // render effect, so just after a document is installed the element
        // still holds the previous one's text. Every writer keeps the signal
        // in step, so it is the version that is always current.
        Mode::Source => ctl.source.get_untracked(),
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
    // A picture has no words and a binary has no characters; the status bar
    // shows their size instead.
    if !ctl.kind.get_untracked().is_text() {
        ctl.words.set(0);
        ctl.chars.set(0);
        ctl.match_count.set(0);
        return;
    }
    let text = visible_text(ctl);
    let mut words = text.split_whitespace().count();
    let mut chars = text.chars().count();
    // A rendered diagram contributes its node labels to the editor's
    // textContent. Those aren't prose, so take them back out (approximate at
    // the edges, where a label can run into adjacent text).
    if eff_mode(ctl) == Mode::Wysiwyg {
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
    match eff_mode(ctl) {
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

/// What a save writes.
///
/// Markdown goes through the serializer, which ends the file with a newline
/// among other tidying. A plain text file does not: appending a byte the user
/// did not type shows up as a diff in whatever they open it with next.
fn save_payload(ctl: Ctl) -> String {
    match ctl.kind.get_untracked() {
        DocKind::Text => ta_el(ctl).map(|t| t.value()).unwrap_or_default(),
        _ => current_markdown(ctl),
    }
}

fn load_markdown(ctl: Ctl, md: &str) {
    match eff_mode(ctl) {
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

/// Push the window title and the global unsaved flag to the backend. The
/// frontend owns the tab list, so it owns both.
///
/// This has to fire on tab switches and closes, not just on a local dirty
/// transition: closing the only dirty tab lowers `any_dirty` without any
/// single document changing, and missing that means the app either prompts on
/// quit forever or quits with unsaved work.
fn push_status(ctl: Ctl) {
    // Ahead of the early return below, and self-deduping: every open, close,
    // save and Save As passes through here, and those are exactly the moments
    // the watched set changes.
    sync_watched(ctl);
    let name = ctl.doc_name.get_untracked();
    let dirty = ctl.dirty.get_untracked();
    let any_dirty = dirty
        || ctl
            .docs
            .with_value(|d| d.iter().any(|doc| doc.dirty.get_untracked()));
    let kind = ctl.kind.get_untracked();
    let markdown = has_doc_now(ctl) && kind == DocKind::Markdown;
    let text = has_doc_now(ctl) && kind.is_text();
    // Redundant IPC on every keystroke is pointless, and the comparison is
    // cheap next to a round trip.
    let next = Status {
        name: name.clone(),
        dirty,
        any_dirty,
        markdown,
        text,
    };
    if ctl.last_status.get_value().as_ref() == Some(&next) {
        return;
    }
    ctl.last_status.set_value(Some(next));
    spawn_local(async move {
        let args = serde_wasm_bindgen::to_value(&TitleArgs {
            name,
            dirty,
            any_dirty,
            markdown,
            text,
        })
        .unwrap();
        let _ = tauri_api::invoke("set_title", args).await;
    });
}

fn mark_dirty(ctl: Ctl) {
    if !ctl.dirty.get_untracked() {
        ctl.dirty.set(true);
    }
    sync_active_doc(ctl);
}

fn mark_clean(ctl: Ctl) {
    ctl.dirty.set(false);
    sync_active_doc(ctl);
}

fn toggle_mode(ctl: Ctl) {
    match eff_mode(ctl) {
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
    match eff_mode(ctl) {
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
    let (u, r) = match ctl.kind.get_untracked() {
        DocKind::Binary => ctl
            .hex
            .with_value(|h| (h.can_undo(), h.can_redo())),
        DocKind::Image | DocKind::Oversized => (false, false),
        _ => ctl
            .hist
            .with_value(|h| (!h.undo.is_empty(), !h.redo.is_empty())),
    };
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

/// Put a snapshot's mode and content into the live editor. The synchronous
/// half of a restore; the caret needs a frame of its own, see
/// [`restore_caret_from`].
fn apply_snap_now(ctl: Ctl, snap: &Snap) {
    match snap {
        Snap::Wys { html, .. } => {
            if eff_mode(ctl) != Mode::Wysiwyg {
                ctl.mode.set(Mode::Wysiwyg);
            }
            if let Some(el) = editor_el(ctl) {
                el.set_inner_html(html);
                enable_checkboxes(ctl);
            }
        }
        Snap::Src { text, .. } => {
            if eff_mode(ctl) != Mode::Source {
                ctl.mode.set(Mode::Source);
            }
            ctl.source.set(text.clone());
            if let Some(ta) = ta_el(ctl) {
                ta.set_value(text);
            }
        }
    }
}

/// Focus the right surface and put the caret back. Must run inside a
/// `request_animation_frame`: `class:hidden` is applied by a queued render
/// effect and `focus()` on a `display:none` element is a no-op, and assigning
/// a textarea's value resets its selection. A frame lands after both.
fn restore_caret_from(ctl: Ctl, snap: &Snap) {
    match snap {
        Snap::Wys { caret, .. } => {
            if let Some(el) = editor_el(ctl) {
                let _ = el.focus();
                history::restore_caret_at(el.unchecked_ref::<Element>(), *caret);
            }
        }
        Snap::Src {
            sel_start, sel_end, ..
        } => {
            if let Some(ta) = ta_el(ctl) {
                let _ = ta.focus();
                let _ = ta.set_selection_range(*sel_start, *sel_end);
            }
        }
    }
}

/// Scroll offset of whichever surface is showing. Captured per document so a
/// tab comes back where it was left.
fn scroll_top(ctl: Ctl) -> i32 {
    match eff_mode(ctl) {
        Mode::Wysiwyg => editor_el(ctl).map(|e| e.scroll_top()).unwrap_or(0),
        Mode::Source => ta_el(ctl).map(|t| t.scroll_top()).unwrap_or(0),
    }
}

fn set_scroll_top(ctl: Ctl, top: i32) {
    match eff_mode(ctl) {
        Mode::Wysiwyg => {
            if let Some(e) = editor_el(ctl) {
                e.set_scroll_top(top);
            }
        }
        Mode::Source => {
            if let Some(t) = ta_el(ctl) {
                t.set_scroll_top(top);
            }
        }
    }
}

fn restore_snap(ctl: Ctl, snap: Snap) {
    apply_snap_now(ctl, &snap);
    let queued = snap.clone();
    request_animation_frame(move || restore_caret_from(ctl, &queued));
    ctl.hist.update_value(|h| h.shadow = Some(snap));
    // Undo/redo always leaves the document modified. A tab switch reuses the
    // machinery above but must *not* come through here, or looking at a tab
    // would dirty it.
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
    new_tab(ctl, "Untitled", "", "", false);
}

/// Report a failed backend command. These are expected failures — a bad path,
/// a permission error — so they use the styled dialog rather than the fatal
/// one, and the app stays usable afterwards.
fn report_backend_error(what: &str, e: &JsValue) {
    let msg = e.as_string().unwrap_or_else(|| "unknown error".into());
    crate::error_dialog::show_error(what, &msg);
}

fn do_open(ctl: Ctl) {
    spawn_local(async move {
        match tauri_api::invoke_no_args("open_doc").await {
            Ok(v) => {
                // Empty: user cancelled the file dialog — stay put.
                for info in tauri_api::parse_file_infos(v) {
                    open_document(ctl, info);
                }
            }
            Err(e) => report_backend_error("Could not open file", &e),
        }
    });
}

fn do_save(ctl: Ctl, save_as: bool, then: Option<Pending>) {
    match ctl.kind.get_untracked() {
        // Bytes go back as bytes; Save As would need its own dialog and a
        // second path through the watcher, and a hex editor that can only
        // write back where it read from is the safer instrument anyway.
        DocKind::Binary => {
            if !save_as {
                hex_save(ctl, then);
            }
            return;
        }
        DocKind::Image | DocKind::Oversized => return,
        DocKind::Markdown | DocKind::Text => {}
    }
    spawn_local(async move {
        let content = save_payload(ctl);
        // The backend no longer tracks "the current document", so the target
        // comes from the tab.
        let path = Some(ctl.doc_path.get_untracked()).filter(|p| !p.is_empty());
        let args = serde_wasm_bindgen::to_value(&SaveArgs {
            content: content.clone(),
            path,
        })
        .unwrap();
        let cmd = if save_as { "save_doc_as" } else { "save_doc" };
        match tauri_api::invoke(cmd, args).await {
            Ok(v) => {
                let Some(r) = tauri_api::parse::<tauri_api::SaveResult>(v) else {
                    return;
                };
                if r.conflict {
                    // Nothing was written: the file moved under us and the
                    // watcher hadn't told us yet. Ask before anything is lost,
                    // and drop `then` — a quit must not step past this.
                    if let Some(id) = active_doc_id(ctl) {
                        set_stale(ctl, id, true);
                    }
                    maybe_prompt_disk(ctl);
                    return;
                }
                if let Some(info) = r.file {
                    // Name and path first: `mark_clean` pushes the title, and
                    // Save As renames the tab.
                    ctl.doc_name.set(info.name);
                    ctl.doc_path.set(info.path);
                    mark_clean(ctl);
                    // Editor and file agree again — the baseline for any
                    // later merge.
                    set_doc_base(ctl, &content);
                    if let Some(p) = then {
                        perform_pending(ctl, p);
                    }
                }
                // None: user cancelled the save dialog — stay put.
            }
            Err(e) => report_backend_error("Could not save file", &e),
        }
    });
}

/// A path that arrived from outside the tree: dropped on the window, opened
/// by the OS, or picked from Open Recent. Same door as the tree, so a file's
/// kind never depends on how it was opened.
fn do_open_path(ctl: Ctl, path: String) {
    open_tree_file(ctl, path);
}

/// Show a loaded file, reusing an existing tab where that is the right thing.
/// Opening never discards anything now, so there is no unsaved-changes gate.
fn open_document(ctl: Ctl, info: tauri_api::FileInfo) {
    if !info.path.is_empty() {
        if let Some(i) = ctl.docs.with_value(|d| {
            d.iter()
                .position(|doc| doc.path.get_untracked() == info.path)
        }) {
            // Deliberately does *not* reload from disk: that tab may hold
            // unsaved edits, and silently discarding them is the exact
            // failure this design exists to prevent.
            switch_to(ctl, i);
            return;
        }
    }
    if is_pristine(ctl) {
        load_markdown(ctl, &info.content);
        ctl.doc_name.set(info.name);
        ctl.doc_path.set(info.path);
        set_doc_base(ctl, &info.content);
        mark_clean(ctl);
        return;
    }
    new_tab(ctl, &info.name, &info.path, &info.content, false);
}

fn do_import_html(ctl: Ctl) {
    spawn_local(async move {
        match tauri_api::invoke_no_args("import_html").await {
            Ok(v) => {
                let Ok(Some(html)) = serde_wasm_bindgen::from_value::<Option<String>>(v) else {
                    return; // dialog cancelled
                };
                // Imported content is unsaved, hence dirty from the start.
                new_tab(ctl, "Untitled", "", &html_to_markdown(&html), true);
            }
            Err(e) => report_backend_error("Could not import file", &e),
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
                new_tab(ctl, "Untitled", "", &md, true);
            }
            Err(e) => report_backend_error("Could not import file", &e),
        }
    });
}

/// Write the document out as a standalone HTML file.
///
/// The body comes from whichever half of the editor is live, so this exports
/// what is on screen rather than what is on disk. In WYSIWYG mode that is the
/// rendered DOM itself — no markdown round-trip, so nothing `dom_to_markdown`
/// normalizes away can go missing on the way out.
fn do_export_html(ctl: Ctl) {
    // A diagram open for editing keeps its source in a textarea's value
    // property, which `inner_html` does not serialize — the same reason
    // `stash_active` quiesces before it snapshots.
    quiesce(ctl);
    let body = match eff_mode(ctl) {
        Mode::Wysiwyg => match editor_el(ctl) {
            Some(el) => export::body_from_editor(el.unchecked_ref::<Element>()),
            None => return,
        },
        Mode::Source => {
            let md = ta_el(ctl).map(|t| t.value()).unwrap_or_default();
            export::body_from_markdown(&md)
        }
    };
    let content = export::build_document(&ctl.doc_name.get_untracked(), &body);
    // The tab's path only seeds the dialog's directory and filename: an export
    // never overwrites the markdown, and never becomes the tab's own path.
    let path = Some(ctl.doc_path.get_untracked()).filter(|p| !p.is_empty());
    spawn_local(async move {
        let args = serde_wasm_bindgen::to_value(&SaveArgs { content, path }).unwrap();
        if let Err(e) = tauri_api::invoke("export_html", args).await {
            report_backend_error("Could not export file", &e);
        }
    });
}

fn perform_pending(ctl: Ctl, p: Pending) {
    match p {
        Pending::CloseTab(id) => close_tab_forced(ctl, id),
        Pending::CloseWindow => close_window_step(ctl),
    }
}

// ---------- the project tree ----------

fn do_open_folder(ctl: Ctl) {
    spawn_local(async move {
        match tauri_api::invoke_no_args("pick_folder").await {
            Ok(v) => {
                let Ok(Some(root)) = serde_wasm_bindgen::from_value::<Option<String>>(v) else {
                    return; // dialog cancelled
                };
                set_project_root(ctl, root);
            }
            Err(e) => report_backend_error("Could not open folder", &e),
        }
    });
}

fn set_project_root(ctl: Ctl, root: String) {
    ctl.project_root.set(root.clone());
    ctl.project_open.set(true);
    ctl.tree.set(Vec::new());
    spawn_local(async move {
        let rows = read_dir(&root, 0).await;
        ctl.tree.set(rows);
    });
}

async fn read_dir(path: &str, depth: usize) -> Vec<TreeNode> {
    let args = serde_wasm_bindgen::to_value(&tauri_api::PathArgs {
        path: path.to_string(),
    })
    .unwrap();
    match tauri_api::invoke("list_dir", args).await {
        Ok(v) => tauri_api::parse::<Vec<tauri_api::Entry>>(v)
            .unwrap_or_default()
            .into_iter()
            .map(|e| TreeNode {
                path: e.path,
                name: e.name,
                dir: e.dir,
                depth,
                open: false,
            })
            .collect(),
        Err(e) => {
            // A folder that cannot be listed — permissions, or it went away —
            // reports itself rather than silently collapsing to nothing.
            report_backend_error("Could not read folder", &e);
            Vec::new()
        }
    }
}

/// Expand or collapse a directory row in place.
fn toggle_dir(ctl: Ctl, path: String) {
    let Some(i) = ctl
        .tree
        .with_untracked(|t| t.iter().position(|n| n.path == path))
    else {
        return;
    };
    let (open, depth) = ctl
        .tree
        .with_untracked(|t| (t[i].open, t[i].depth))
        .to_owned();
    if open {
        // Everything deeper that follows belongs to this row.
        ctl.tree.update(|t| {
            t[i].open = false;
            let end = t[i + 1..]
                .iter()
                .position(|n| n.depth <= depth)
                .map(|p| i + 1 + p)
                .unwrap_or(t.len());
            t.drain(i + 1..end);
        });
        return;
    }
    spawn_local(async move {
        let children = read_dir(&path, depth + 1).await;
        ctl.tree.update(|t| {
            // The row may have moved while the directory was being read.
            let Some(i) = t.iter().position(|n| n.path == path) else {
                return;
            };
            t[i].open = true;
            for (n, child) in children.into_iter().enumerate() {
                t.insert(i + 1 + n, child);
            }
        });
    });
}

/// Open a file from the tree. Whatever it is, the backend has already decided.
fn open_tree_file(ctl: Ctl, path: String) {
    spawn_local(async move {
        let args = serde_wasm_bindgen::to_value(&tauri_api::PathArgs { path }).unwrap();
        match tauri_api::invoke("open_file", args).await {
            Ok(v) => {
                if let Some(f) = tauri_api::parse::<tauri_api::AnyFile>(v) {
                    open_any(ctl, f);
                }
            }
            Err(e) => report_backend_error("Could not open file", &e),
        }
    });
}

/// Show a file the backend has read, reusing a tab where that is right — the
/// same rules as opening a document, because to the tab strip it is one.
fn open_any(ctl: Ctl, f: tauri_api::AnyFile) {
    if let Some(i) = ctl
        .docs
        .with_value(|d| d.iter().position(|doc| doc.path.get_untracked() == f.path))
    {
        // Deliberately not reloaded: that tab may hold unsaved edits.
        switch_to(ctl, i);
        return;
    }
    let kind = DocKind::from_str(&f.kind);
    if is_pristine(ctl) {
        set_doc_kind(ctl, kind);
        install(ctl, &f);
        return;
    }
    stash_active(ctl);
    let id = {
        let n = ctl.next_id.get_value();
        ctl.next_id.set_value(n + 1);
        n
    };
    let doc = make_doc(ctl, id, &f.name, &f.path, false, kind);
    let i = ctl.docs.with_value(Vec::len);
    ctl.docs.update_value(|d| d.push(doc));
    publish_tabs(ctl);
    ctl.active.set(Some(i));
    ctl.hist.set_value(Hist::default());
    install(ctl, &f);
    set_scroll_top(ctl, 0);
    history_flags(ctl);
    focus_current(ctl);
}

/// `Doc.kind` is fixed at creation, so refilling a pristine tab with something
/// that isn't markdown has to rewrite it.
fn set_doc_kind(ctl: Ctl, kind: DocKind) {
    if let Some(i) = ctl.active.get_untracked() {
        ctl.docs.update_value(|d| {
            if let Some(x) = d.get_mut(i) {
                x.kind = kind;
            }
        });
    }
}

/// Load a file the backend has read into the *active* tab.
fn install(ctl: Ctl, f: &tauri_api::AnyFile) {
    let kind = DocKind::from_str(&f.kind);
    ctl.kind.set(kind);
    ctl.doc_size.set(f.size);
    ctl.doc_name.set(f.name.clone());
    ctl.doc_path.set(f.path.clone());
    ctl.data.set(String::new());
    ctl.hex.set_value(hex::HexDoc::default());
    ctl.hex_cursor.set(0);
    ctl.hex_high.set(true);
    ctl.hex_top.set(0);
    match kind {
        DocKind::Markdown | DocKind::Text => load_markdown(ctl, &f.text),
        DocKind::Image => {
            ctl.data.set(format!("data:{};base64,{}", f.mime, f.data));
            clear_editor(ctl);
        }
        DocKind::Binary => {
            match BASE64_STANDARD.decode(&f.data) {
                Ok(bytes) => ctl.hex.set_value(hex::HexDoc::new(bytes)),
                Err(e) => {
                    report_backend_error("Could not read file", &JsValue::from_str(&e.to_string()))
                }
            }
            clear_editor(ctl);
        }
        DocKind::Oversized => clear_editor(ctl),
    }
    ctl.hex_gen.update(|g| *g = g.wrapping_add(1));
    set_doc_size(ctl, f.size);
    set_doc_base(ctl, &f.text);
    mark_clean(ctl);
    recount(ctl);
    history_flags(ctl);
}

fn clear_editor(ctl: Ctl) {
    if let Some(el) = editor_el(ctl) {
        el.set_inner_html("");
    }
    ctl.source.set(String::new());
    ctl.hist.set_value(Hist::default());
}

fn set_doc_size(ctl: Ctl, size: u64) {
    if let Some(i) = ctl.active.get_untracked() {
        ctl.docs.update_value(|d| {
            if let Some(x) = d.get_mut(i) {
                x.size = size;
            }
        });
    }
}

// ---------- the hex editor ----------

fn hex_len(ctl: Ctl) -> usize {
    ctl.hex.with_value(hex::HexDoc::len)
}

/// The furthest the cursor may go. Insert mode may sit one past the last byte,
/// which is where an append happens; overwrite mode may not, because there is
/// nothing there to write over.
fn hex_max_cursor(ctl: Ctl) -> usize {
    let len = hex_len(ctl);
    if ctl.hex_overwrite.get_untracked() {
        len.saturating_sub(1)
    } else {
        len
    }
}

fn hex_touched(ctl: Ctl) {
    ctl.hex_gen.update(|g| *g = g.wrapping_add(1));
    mark_dirty(ctl);
    history_flags(ctl);
}

fn hex_set_cursor(ctl: Ctl, at: usize) {
    let at = at.min(hex_max_cursor(ctl));
    ctl.hex_cursor.set(at);
    ctl.hex_high.set(true);
    hex_scroll_into_view(ctl, at);
}

fn hex_move(ctl: Ctl, delta: isize) {
    let cur = ctl.hex_cursor.get_untracked() as isize;
    hex_set_cursor(ctl, cur.saturating_add(delta).max(0) as usize);
}

/// Keep the cursor's row on screen, scrolling only when it has left it.
fn hex_scroll_into_view(ctl: Ctl, at: usize) {
    let Some(el) = ctl.hex_ref.get_untracked() else {
        return;
    };
    let row = at / hex::ROW;
    let top = ctl.hex_top.get_untracked();
    let rows = ctl.hex_rows.get_untracked().max(1);
    let el: &Element = el.unchecked_ref();
    if row < top {
        el.set_scroll_top((row * HEX_ROW_H) as i32);
    } else if row >= top + rows {
        el.set_scroll_top(((row + 1 - rows) * HEX_ROW_H) as i32);
    }
}

/// A hex digit typed at the cursor. Half a byte at a time: the high nibble
/// then the low one, which is how every hex editor behaves and why the two
/// collapse into a single undo step.
fn hex_type(ctl: Ctl, v: u8) {
    let at = ctl.hex_cursor.get_untracked();
    let high = ctl.hex_high.get_untracked();
    let overwrite = ctl.hex_overwrite.get_untracked();
    if overwrite && at >= hex_len(ctl) {
        return; // nothing here to write over
    }
    ctl.hex.update_value(|h| {
        let old = h.byte(at).unwrap_or(0);
        if high {
            if overwrite {
                h.overwrite(at, (v << 4) | (old & 0x0f));
            } else {
                h.insert(at, v << 4);
            }
        } else {
            h.overwrite(at, (old & 0xf0) | v);
        }
    });
    if high {
        ctl.hex_high.set(false);
    } else {
        ctl.hex_high.set(true);
        let next = (at + 1).min(hex_max_cursor(ctl));
        ctl.hex_cursor.set(next);
        hex_scroll_into_view(ctl, next);
    }
    hex_touched(ctl);
}

fn hex_delete(ctl: Ctl, backwards: bool) {
    // Deleting shortens the file, which overwrite mode exists to prevent.
    if ctl.hex_overwrite.get_untracked() {
        return;
    }
    let at = ctl.hex_cursor.get_untracked();
    let at = if backwards {
        if at == 0 {
            return;
        }
        at - 1
    } else {
        at
    };
    let removed = ctl.hex.try_update_value(|h| h.delete(at)).unwrap_or(false);
    if removed {
        ctl.hex_cursor.set(at.min(hex_max_cursor(ctl)));
        ctl.hex_high.set(true);
        hex_touched(ctl);
    }
}

fn hex_undo(ctl: Ctl) {
    if let Some(Some(at)) = ctl.hex.try_update_value(hex::HexDoc::undo) {
        ctl.hex_cursor.set(at.min(hex_max_cursor(ctl)));
        ctl.hex_high.set(true);
        hex_scroll_into_view(ctl, at);
        hex_touched(ctl);
    }
}

fn hex_redo(ctl: Ctl) {
    if let Some(Some(at)) = ctl.hex.try_update_value(hex::HexDoc::redo) {
        ctl.hex_cursor.set(at.min(hex_max_cursor(ctl)));
        ctl.hex_high.set(true);
        hex_scroll_into_view(ctl, at);
        hex_touched(ctl);
    }
}

/// Insert / overwrite. Leaving insert mode can strand the cursor one past the
/// end, which only insert mode allows.
fn hex_toggle_overwrite(ctl: Ctl) {
    if ctl.kind.get_untracked() != DocKind::Binary {
        return;
    }
    let next = !ctl.hex_overwrite.get_untracked();
    ctl.hex_overwrite.set(next);
    ctl.hex_high.set(true);
    let cur = ctl.hex_cursor.get_untracked();
    let max = hex_max_cursor(ctl);
    if cur > max {
        ctl.hex_cursor.set(max);
    }
}

fn hex_save(ctl: Ctl, then: Option<Pending>) {
    let path = ctl.doc_path.get_untracked();
    if path.is_empty() {
        return; // a binary always came from a file
    }
    let data = ctl.hex.with_value(|h| BASE64_STANDARD.encode(h.bytes()));
    spawn_local(async move {
        let args = serde_wasm_bindgen::to_value(&tauri_api::BytesArgs { path, data }).unwrap();
        match tauri_api::invoke("save_bytes", args).await {
            Ok(v) => {
                let Some(r) = tauri_api::parse::<tauri_api::SaveResult>(v) else {
                    return;
                };
                if r.conflict {
                    if let Some(id) = active_doc_id(ctl) {
                        set_stale(ctl, id, true);
                    }
                    maybe_prompt_disk(ctl);
                    return;
                }
                if r.file.is_some() {
                    // The bytes on disk are these bytes now, so there is
                    // nothing before this point worth an undo step's memory.
                    ctl.hex.update_value(hex::HexDoc::mark_saved);
                    mark_clean(ctl);
                    history_flags(ctl);
                    if let Some(p) = then {
                        perform_pending(ctl, p);
                    }
                }
            }
            Err(e) => report_backend_error("Could not save file", &e),
        }
    });
}

/// Rows are a fixed height so the visible window can be found by division
/// rather than by measuring 65536 of them. Must match `.hex-row` in the CSS.
const HEX_ROW_H: usize = 20;

/// Both size limits are the backend's; repeating them is how the oversized
/// tab explains itself.
const TEXT_LIMIT: u64 = 1024 * 1024;
const IMAGE_LIMIT: u64 = 16 * 1024 * 1024;

fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    match bytes {
        b if b >= MB => format!("{:.1} MB", b as f64 / MB as f64),
        b if b >= KB => format!("{:.1} KB", b as f64 / KB as f64),
        b => format!("{b} bytes"),
    }
}

/// One rendered row of the hex dump: offset, sixteen bytes, sixteen glyphs.
///
/// Both columns are per-byte elements rather than one run of text, because
/// both are click targets and either can hold the cursor.
fn hex_row(ctl: Ctl, row: usize) -> impl IntoView {
    ctl.hex_gen.get();
    let cursor = ctl.hex_cursor.get();
    let start = row * hex::ROW;
    let bytes: Vec<Option<u8>> = ctl
        .hex
        .with_value(|h| (0..hex::ROW).map(|i| h.byte(start + i)).collect());
    let len = ctl.hex.with_value(hex::HexDoc::len);
    let cells: Vec<_> = bytes
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let at = start + i;
            let here = at == cursor;
            let label = b.map(hex::hex_byte).unwrap_or_else(|| "  ".to_string());
            view! {
                <span
                    class="hex-cell"
                    class:cursor=here
                    class:gap=move || i % 8 == 7
                    on:mousedown=move |e: web_sys::MouseEvent| {
                        e.prevent_default();
                        hex_set_cursor(ctl, at);
                    }
                >
                    {label}
                </span>
            }
        })
        .collect();
    let ascii: Vec<_> = bytes
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let at = start + i;
            let here = at == cursor;
            let ch = b.map(hex::ascii_byte).unwrap_or(' ').to_string();
            view! {
                <span
                    class="hex-char"
                    class:cursor=here
                    on:mousedown=move |e: web_sys::MouseEvent| {
                        e.prevent_default();
                        hex_set_cursor(ctl, at);
                    }
                >
                    {ch}
                </span>
            }
        })
        .collect();
    // One past the last byte is where an append lands, and it needs somewhere
    // visible to sit.
    let at_end = cursor == len && start + hex::ROW > len;
    view! {
        <div class="hex-row">
            <span class="hex-offset">{hex::offset_label(start)}</span>
            <span class="hex-bytes">{cells}<span class="hex-eof" class:cursor=at_end></span></span>
            <span class="hex-ascii">{ascii}</span>
        </div>
    }
}

/// Recompute which rows are on screen, after a scroll or a resize.
fn hex_recompute_window(ctl: Ctl) {
    let Some(el) = ctl.hex_ref.get_untracked() else {
        return;
    };
    let el: &Element = el.unchecked_ref();
    let top = (el.scroll_top().max(0) as usize) / HEX_ROW_H;
    // One row of slack at each end, so a partially visible row is still drawn.
    let rows = (el.client_height().max(0) as usize) / HEX_ROW_H + 2;
    if ctl.hex_top.get_untracked() != top {
        ctl.hex_top.set(top);
    }
    if ctl.hex_rows.get_untracked() != rows {
        ctl.hex_rows.set(rows);
    }
}

// ---------- changes on disk ----------

/// Tell the backend which files to watch. Deduped like `push_status`: it rides
/// on every status push, and most of those are keystrokes.
fn sync_watched(ctl: Ctl) {
    let paths: Vec<String> = ctl.docs.with_value(|d| {
        d.iter()
            .map(|x| x.path.get_untracked())
            .filter(|p| !p.is_empty())
            .collect()
    });
    if ctl.last_watched.with_value(|w| *w == paths) {
        return;
    }
    ctl.last_watched.set_value(paths.clone());
    spawn_local(async move {
        let args = serde_wasm_bindgen::to_value(&tauri_api::PathsArgs { paths }).unwrap();
        let _ = tauri_api::invoke("watch_paths", args).await;
    });
}

/// The markdown the active tab and its file last agreed on.
fn doc_base(ctl: Ctl) -> String {
    let Some(i) = ctl.active.get_untracked() else {
        return String::new();
    };
    ctl.docs
        .with_value(|d| d.get(i).map(|x| x.base.clone()))
        .unwrap_or_default()
}

/// Record a new agreement point: after an open, a save, a reload or a merge.
fn set_doc_base(ctl: Ctl, md: &str) {
    let Some(i) = ctl.active.get_untracked() else {
        return;
    };
    ctl.docs.update_value(|d| {
        if let Some(x) = d.get_mut(i) {
            x.base = md.to_string();
        }
    });
}

fn set_stale(ctl: Ctl, id: u32, stale: bool) {
    let Some(sig) = index_of(ctl, id).and_then(|i| {
        ctl.docs
            .with_value(|d| d.get(i).map(|x| x.stale))
    }) else {
        return;
    };
    if sig.get_untracked() != stale {
        sig.set(stale);
    }
}

fn active_doc_id(ctl: Ctl) -> Option<u32> {
    let i = ctl.active.get_untracked()?;
    ctl.docs.with_value(|d| d.get(i).map(|x| x.id))
}

/// A watched file changed underneath its tab.
fn on_disk_changed(ctl: Ctl, path: String) {
    let Some(i) = ctl
        .docs
        .with_value(|d| d.iter().position(|x| x.path.get_untracked() == path))
    else {
        return;
    };
    let Some(stale) = ctl.docs.with_value(|d| d.get(i).map(|x| x.stale)) else {
        return;
    };
    // One unanswered question per file: a burst of filesystem events, or a
    // second external save while the prompt is already up, is still one prompt.
    if stale.get_untracked() {
        return;
    }
    stale.set(true);
    maybe_prompt_disk(ctl);
}

/// Prompt only about the document being looked at. A background tab keeps its
/// marker and asks when you switch to it — yanking you to a tab you weren't
/// editing, mid-keystroke, is worse than waiting.
fn maybe_prompt_disk(ctl: Ctl) {
    // One modal at a time, and never over an unanswered question about
    // something else.
    if ctl.disk_modal.get_untracked().is_some()
        || ctl.pending.get_untracked().is_some()
        || ctl.url_modal.get_untracked().is_some()
    {
        return;
    }
    let Some(i) = ctl.active.get_untracked() else {
        return;
    };
    let Some((id, stale)) = ctl
        .docs
        .with_value(|d| d.get(i).map(|x| (x.id, x.stale)))
    else {
        return;
    };
    if stale.get_untracked() {
        ctl.disk_modal.set(Some(id));
    }
}

/// Close the prompt and hand back the document it was about, made active so
/// the answer lands on it. `None` means there is nothing to do — the tab was
/// closed while the prompt was up.
fn take_disk_target(ctl: Ctl) -> Option<(u32, String)> {
    let id = ctl.disk_modal.get_untracked()?;
    let Some(i) = index_of(ctl, id) else {
        ctl.disk_modal.set(None);
        return None;
    };
    // Switch *before* clearing: `activate` re-prompts for a stale document,
    // and the modal still being open is what stops it re-opening the very
    // prompt being answered.
    switch_to(ctl, i);
    ctl.disk_modal.set(None);
    let path = ctl.doc_path.get_untracked();
    if path.is_empty() {
        return None;
    }
    // Answered, so the marker goes now; it comes back if the answer fails.
    set_stale(ctl, id, false);
    Some((id, path))
}

/// Make `id` active again after an await, or report that it is gone. Every
/// answer to the prompt is a round trip, and a tab switch can happen inside it.
fn focus_doc(ctl: Ctl, id: u32) -> bool {
    match index_of(ctl, id) {
        Some(i) => {
            switch_to(ctl, i);
            true
        }
        None => false,
    }
}

/// Swap the whole document's content, in place and undoably. Unlike
/// `load_markdown` this keeps the undo stack, so a reload or a merge can be
/// taken back.
fn replace_document(ctl: Ctl, md: &str) {
    // An open diagram's source lives in a textarea we are about to replace.
    quiesce(ctl);
    history_commit(ctl);
    match eff_mode(ctl) {
        Mode::Wysiwyg => {
            if let Some(el) = editor_el(ctl) {
                let html = to_html(md);
                el.set_inner_html(if html.is_empty() { "<p><br></p>" } else { &html });
                enable_checkboxes(ctl);
            }
        }
        Mode::Source => {
            if let Some(ta) = ta_el(ctl) {
                ta.set_value(md);
            }
            ctl.source.set(md.to_string());
        }
    }
    recount(ctl);
    history_sync(ctl);
    history_flags(ctl);
}

/// Throw away the tab's version and take the file's.
fn disk_reload(ctl: Ctl) {
    let Some((id, path)) = take_disk_target(ctl) else {
        return;
    };
    spawn_local(async move {
        let args = serde_wasm_bindgen::to_value(&tauri_api::PathArgs { path }).unwrap();
        match tauri_api::invoke("open_file", args).await {
            Ok(v) => {
                let Some(f) = tauri_api::parse::<tauri_api::AnyFile>(v) else {
                    return;
                };
                if !focus_doc(ctl, id) {
                    return;
                }
                // A binary or an image is replaced wholesale — there is no
                // undoable text edit to express, and `install` resets the
                // hex history along with everything else.
                if DocKind::from_str(&f.kind).is_text() {
                    replace_document(ctl, &f.text);
                    set_doc_base(ctl, &f.text);
                    set_doc_size(ctl, f.size);
                    ctl.doc_size.set(f.size);
                    mark_clean(ctl);
                } else {
                    install(ctl, &f);
                }
            }
            Err(e) => {
                set_stale(ctl, id, true);
                report_backend_error("Could not reload file", &e);
            }
        }
    });
}

/// Combine both versions against the content they last had in common.
fn disk_merge(ctl: Ctl) {
    let Some((id, path)) = take_disk_target(ctl) else {
        return;
    };
    let args = tauri_api::MergeArgs {
        path,
        base: doc_base(ctl),
        mine: current_markdown(ctl),
    };
    spawn_local(async move {
        let args = serde_wasm_bindgen::to_value(&args).unwrap();
        match tauri_api::invoke("merge_doc", args).await {
            Ok(v) => {
                let Some(r) = tauri_api::parse::<tauri_api::MergeResult>(v) else {
                    return;
                };
                if !focus_doc(ctl, id) {
                    return;
                }
                // Conflict markers are line noise in a rendered view, and
                // resolving them is a text edit — so hand over the text.
                if r.conflicted && eff_mode(ctl) == Mode::Wysiwyg {
                    toggle_mode(ctl);
                }
                replace_document(ctl, &r.text);
                // The file's content is what the two now have in common: a
                // later change on disk merges against this, not against what
                // the tab was opened with.
                set_doc_base(ctl, &r.disk);
                mark_dirty(ctl);
            }
            Err(e) => {
                set_stale(ctl, id, true);
                report_backend_error("Could not merge file", &e);
            }
        }
    });
}

/// Leave the tab alone. The file's new contents become the baseline anyway, so
/// this same change is never raised twice — and the tab goes dirty, because it
/// no longer matches the file and saving it will overwrite.
fn disk_keep(ctl: Ctl) {
    let Some((id, path)) = take_disk_target(ctl) else {
        return;
    };
    spawn_local(async move {
        let args = serde_wasm_bindgen::to_value(&tauri_api::PathArgs { path }).unwrap();
        match tauri_api::invoke("ack_disk_change", args).await {
            Ok(_) => {
                if focus_doc(ctl, id) {
                    mark_dirty(ctl);
                }
            }
            Err(e) => {
                set_stale(ctl, id, true);
                report_backend_error("Could not re-check file", &e);
            }
        }
    });
}

// ---------- tabs ----------

/// Reactive — for the view.
fn has_doc(ctl: Ctl) -> bool {
    ctl.active.get().is_some()
}

/// Non-reactive — for handlers.
fn has_doc_now(ctl: Ctl) -> bool {
    ctl.active.get_untracked().is_some()
}

fn publish_tabs(ctl: Ctl) {
    // Build outside the borrow, then set. Writing a signal inside
    // `with_value` risks a synchronous read back into `docs`.
    let tabs = ctl.docs.with_value(|d| {
        d.iter()
            .map(|doc| TabView {
                id: doc.id,
                name: doc.name,
                dirty: doc.dirty,
                stale: doc.stale,
            })
            .collect::<Vec<_>>()
    });
    ctl.tabs.set(tabs);
}

fn index_of(ctl: Ctl, id: u32) -> Option<usize> {
    ctl.docs.with_value(|d| d.iter().position(|x| x.id == id))
}

fn active_id(ctl: Ctl) -> Option<u32> {
    let i = ctl.active.get()?;
    ctl.tabs.with(|t| t.get(i).map(|x| x.id))
}

/// Write the live per-document signals through to the active tab, then push
/// the title. Every writer of `doc_name`, `doc_path` or `dirty` ends here.
///
/// Deliberately synchronous rather than an `Effect`: Leptos effects are
/// queued, so a rename and a tab switch in the same tick would flush once
/// with the *new* `active` and write the old name into the new document.
fn sync_active_doc(ctl: Ctl) {
    let Some(i) = ctl.active.get_untracked() else {
        push_status(ctl);
        return;
    };
    let Some((name, path, dirty)) = ctl
        .docs
        .with_value(|d| d.get(i).map(|x| (x.name, x.path, x.dirty)))
    else {
        return;
    };
    // Borrow dropped: safe to write signals now.
    let (n, p, dt) = (
        ctl.doc_name.get_untracked(),
        ctl.doc_path.get_untracked(),
        ctl.dirty.get_untracked(),
    );
    if name.get_untracked() != n {
        name.set(n);
    }
    if path.get_untracked() != p {
        path.set(p);
    }
    if dirty.get_untracked() != dt {
        dirty.set(dt);
    }
    push_status(ctl);
}

/// Bring the live DOM to rest so it can be handed to another document.
fn quiesce(ctl: Ctl) {
    // A diagram open for editing keeps its source in the textarea's *value
    // property*, and `inner_html()` serializes a textarea's initial child
    // text, not its value. Stashing over an open diagram would silently blank
    // it. Rendering it back first also runs its history commit.
    if let Some(el) = editor_el(ctl) {
        if let Ok(list) = el.query_selector_all("textarea.mermaid-source") {
            for i in 0..list.length() {
                if let Some(ta) = list
                    .item(i)
                    .and_then(|n| n.dyn_into::<HtmlTextAreaElement>().ok())
                {
                    close_mermaid_source(ctl, &ta);
                }
            }
        }
    }
    // These hold live handles into the DOM we are about to replace.
    close_ctx_menu(ctl);
    ctl.saved_range.set_value(None);
    ctl.url_modal.set(None);
}

fn stash_active(ctl: Ctl) {
    let Some(i) = ctl.active.get_untracked() else {
        return;
    };
    quiesce(ctl);
    let text = ctl.kind.get_untracked().is_text();
    // The editor pair holds nothing for a picture or a binary, so there is no
    // snapshot to take and no scroll position worth keeping.
    let snap = if text {
        capture_current(ctl).unwrap_or(Snap::Wys {
            html: String::new(),
            caret: 0,
        })
    } else {
        Snap::Wys {
            html: String::new(),
            caret: 0,
        }
    };
    let scroll = if text { scroll_top(ctl) } else { 0 };
    // Neither `Hist` nor `HexDoc` is worth cloning: move them out and leave
    // defaults behind.
    let mut hist = Hist::default();
    ctl.hist.update_value(|h| hist = std::mem::take(h));
    let mut hexdoc = hex::HexDoc::default();
    ctl.hex.update_value(|h| hexdoc = std::mem::take(h));
    let data = ctl.data.get_untracked();

    ctl.docs.update_value(|docs| {
        if let Some(d) = docs.get_mut(i) {
            d.snap = snap;
            d.scroll = scroll;
            d.hist = hist;
            d.hex = hexdoc;
            d.data = data;
            // name / path / dirty are already current: `sync_active_doc`
            // writes them through on every change.
        }
    });

    // Ends any IME composition, and stops the outgoing document's `focusout`
    // from firing after the incoming one is installed.
    if let Some(el) = editor_el(ctl) {
        let _ = el.blur();
    }
}

fn activate(ctl: Ctl, i: usize) {
    // Pull everything out under one borrow; the signal handles are Copy, so
    // they are read after the borrow is dropped.
    let Some((snap, scroll, hist, name, path, dirty, kind, size, data, hexdoc)) = ctl
        .docs
        .try_update_value(|docs| {
            let d = docs.get_mut(i)?;
            Some((
                d.snap.clone(),
                d.scroll,
                std::mem::take(&mut d.hist),
                d.name,
                d.path,
                d.dirty,
                d.kind,
                d.size,
                std::mem::take(&mut d.data),
                std::mem::take(&mut d.hex),
            ))
        })
        .flatten()
    else {
        return;
    };

    ctl.active.set(Some(i));
    ctl.doc_name.set(name.get_untracked());
    ctl.doc_path.set(path.get_untracked());
    ctl.dirty.set(dirty.get_untracked());
    ctl.hist.set_value(hist);
    ctl.kind.set(kind);
    ctl.doc_size.set(size);
    ctl.data.set(data);
    ctl.hex.set_value(hexdoc);
    ctl.hex_gen.update(|g| *g = g.wrapping_add(1));

    if kind.is_text() {
        // Deliberately not `restore_snap`: that dirties unconditionally, which
        // would mark every tab you merely look at as modified.
        apply_snap_now(ctl, &snap);
    } else if let Some(el) = editor_el(ctl) {
        // The editor is hidden for these, but it must not keep the last
        // document's markup: `recount` and `window.find` both read it.
        el.set_inner_html("");
    }
    ctl.hist.update_value(|h| h.shadow = Some(snap.clone()));

    recount(ctl);
    history_flags(ctl);
    push_status(ctl);
    // A tab whose file changed while it sat in the background asks now.
    maybe_prompt_disk(ctl);

    let generation = ctl.switch_gen.get_value().wrapping_add(1);
    ctl.switch_gen.set_value(generation);
    request_animation_frame(move || {
        // A second switch may have overtaken this frame.
        if ctl.switch_gen.get_value() != generation {
            return;
        }
        restore_caret_from(ctl, &snap);
        // Last: focusing and restoring the caret can scroll the container.
        set_scroll_top(ctl, scroll);
        update_in_table(ctl);
    });
}

fn switch_to(ctl: Ctl, i: usize) {
    if ctl.active.get_untracked() == Some(i) || i >= ctl.docs.with_value(Vec::len) {
        return;
    }
    stash_active(ctl);
    activate(ctl, i);
}

fn switch_to_id(ctl: Ctl, id: u32) {
    if let Some(i) = index_of(ctl, id) {
        switch_to(ctl, i);
    }
}

/// Build a `Doc` whose signals belong to the app's root owner. Creating them
/// under the ambient owner is a trap: a tab opened from a handler inside a
/// `<Show>` or `<For>` gets signals that are disposed when that branch
/// unmounts, and every later read panics with "already disposed".
fn make_doc(ctl: Ctl, id: u32, name: &str, path: &str, dirty: bool, kind: DocKind) -> Doc {
    let build = || Doc {
        id,
        name: RwSignal::new(name.to_string()),
        path: RwSignal::new(path.to_string()),
        dirty: RwSignal::new(dirty),
        stale: RwSignal::new(false),
        base: String::new(),
        snap: Snap::Wys {
            html: String::new(),
            caret: 0,
        },
        scroll: 0,
        hist: Hist::default(),
        kind,
        size: 0,
        data: String::new(),
        hex: hex::HexDoc::default(),
    };
    match ctl.owner.get_value() {
        Some(o) => o.with(build),
        None => build(),
    }
}

/// Open a document in a new tab and focus it.
fn new_tab(ctl: Ctl, name: &str, path: &str, md: &str, dirty: bool) {
    stash_active(ctl);
    let id = {
        let n = ctl.next_id.get_value();
        ctl.next_id.set_value(n + 1);
        n
    };
    let doc = make_doc(ctl, id, name, path, dirty, DocKind::Markdown);
    let i = ctl.docs.with_value(Vec::len);
    ctl.docs.update_value(|d| d.push(doc));
    publish_tabs(ctl);

    ctl.active.set(Some(i));
    ctl.kind.set(DocKind::Markdown);
    ctl.doc_size.set(0);
    ctl.data.set(String::new());
    ctl.hex.set_value(hex::HexDoc::default());
    ctl.doc_name.set(name.to_string());
    ctl.doc_path.set(path.to_string());
    // What this tab and its file agree on right now. An import or a new
    // document has no file, and the empty baseline is never consulted.
    set_doc_base(ctl, md);
    ctl.hist.set_value(Hist::default());
    // Mode is inherited: someone working in Markdown gets a Markdown tab.
    // `load_markdown` is mode-aware and resets history.
    if eff_mode(ctl) == Mode::Source {
        if let Some(el) = editor_el(ctl) {
            el.set_inner_html("<p><br></p>");
        }
    }
    load_markdown(ctl, md);
    ctl.dirty.set(false);
    if dirty {
        mark_dirty(ctl); // imports arrive unsaved
    }
    sync_active_doc(ctl);
    set_scroll_top(ctl, 0);
    history_flags(ctl);
    focus_current(ctl);
}

/// A single untouched blank document is a slot to fill, not a document to
/// keep — this is what makes `mdedit file.md` land in tab 0 rather than
/// leaving an empty tab beside it.
fn is_pristine(ctl: Ctl) -> bool {
    ctl.docs.with_value(Vec::len) == 1
        && !ctl.dirty.get_untracked()
        && ctl.doc_path.get_untracked().is_empty()
        && visible_text(ctl).trim().is_empty()
}

/// Close a tab, asking first if it has unsaved changes.
fn close_tab(ctl: Ctl, id: u32) {
    let Some(i) = index_of(ctl, id) else { return };
    let is_dirty = ctl
        .docs
        .with_value(|d| d.get(i).map(|x| x.dirty))
        .map(|s| s.get_untracked())
        .unwrap_or(false);
    if is_dirty {
        // The prompt names the *active* document and Save writes it, so make
        // it active before asking.
        switch_to(ctl, i);
        ctl.pending.set(Some(Pending::CloseTab(id)));
    } else {
        close_tab_forced(ctl, id);
    }
}

fn close_tab_forced(ctl: Ctl, id: u32) {
    let Some(i) = index_of(ctl, id) else { return };
    let was_active = ctl.active.get_untracked() == Some(i);
    if was_active {
        // No stash: the state is being discarded.
        quiesce(ctl);
    }
    ctl.docs.update_value(|d| {
        d.remove(i);
    });
    publish_tabs(ctl);

    let n = ctl.docs.with_value(Vec::len);
    if n == 0 {
        enter_empty_state(ctl);
        return;
    }
    let cur = ctl.active.get_untracked().unwrap_or(0);
    if was_active {
        let next = i.min(n - 1);
        ctl.active.set(Some(next));
        activate(ctl, next);
    } else {
        if cur > i {
            ctl.active.set(Some(cur - 1)); // indices shifted under us
        }
        // `any_dirty` may have changed even though the active tab didn't.
        sync_active_doc(ctl);
    }
}

/// The window with nothing in it. Every per-document signal goes back to its
/// zero value so no scrap of the closed document can leak into the title, the
/// status bar, the undo buttons or the next save.
fn enter_empty_state(ctl: Ctl) {
    ctl.active.set(None);
    ctl.doc_name.set(String::new());
    ctl.doc_path.set(String::new());
    ctl.dirty.set(false);
    ctl.source.set(String::new());
    // Back to markdown, or closing the last tab of a picture or a binary
    // would leave its pane drawn over the empty frame.
    ctl.kind.set(DocKind::Markdown);
    ctl.doc_size.set(0);
    ctl.data.set(String::new());
    ctl.hex.set_value(hex::HexDoc::default());
    ctl.hex_cursor.set(0);
    ctl.hex_top.set(0);
    ctl.hex_gen.update(|g| *g = g.wrapping_add(1));
    // Not "<p><br></p>": there is no paragraph, because there is no document.
    if let Some(el) = editor_el(ctl) {
        el.set_inner_html("");
    }
    ctl.hist.set_value(Hist::default());
    ctl.can_undo.set(false);
    ctl.can_redo.set(false);
    ctl.words.set(0);
    ctl.chars.set(0);
    ctl.in_table.set(false);
    // `window.find` searches the whole page, so leaving find open here would
    // let Enter start selecting toolbar chrome.
    ctl.find_open.set(false);
    ctl.replace_open.set(false);
    ctl.match_count.set(0);
    push_status(ctl);
}

/// One step of the quit sequence. The backend emits a single
/// `close-requested` however many documents are dirty, so the frontend walks
/// them one at a time, re-entering through `perform_pending`.
fn close_window_step(ctl: Ctl) {
    let next = ctl
        .docs
        .with_value(|d| d.iter().position(|doc| doc.dirty.get_untracked()));
    match next {
        Some(i) => {
            switch_to(ctl, i);
            ctl.pending.set(Some(Pending::CloseWindow));
        }
        None => spawn_local(async move {
            let _ = tauri_api::invoke_no_args("force_close").await;
        }),
    }
}

/// Wraps around; a single tab is a no-op.
fn cycle_tab(ctl: Ctl, delta: isize) {
    let n = ctl.docs.with_value(Vec::len);
    if n < 2 {
        return;
    }
    let cur = ctl.active.get_untracked().unwrap_or(0) as isize;
    let n = n as isize;
    let next = ((cur + delta) % n + n) % n;
    switch_to(ctl, next as usize);
}

/// Show the `»` button only when the strip actually overflows. The button's
/// width is permanently reserved in CSS (`visibility`, not `display`), so
/// revealing it cannot shrink the strip and create the overflow that
/// justified it — that feedback loop flickers at a knife-edge window width.
fn recompute_overflow(ctl: Ctl) {
    let Some(strip) = ctl.tabstrip_ref.get_untracked() else {
        return;
    };
    let el: &Element = strip.unchecked_ref();
    let over = el.scroll_width() > el.client_width();
    if ctl.tabs_overflow.get_untracked() != over {
        ctl.tabs_overflow.set(over);
    }
}

/// Keep the active tab in view, scrolling only when it is actually outside
/// the strip (`scroll_into_view` re-aligns even when it is already visible).
fn scroll_active_tab_into_view(ctl: Ctl) {
    let Some(strip) = ctl.tabstrip_ref.get_untracked() else {
        return;
    };
    let Ok(Some(tab)) = strip.query_selector(".tab.active") else {
        return;
    };
    let Some(tab) = tab.dyn_ref::<web_sys::HtmlElement>() else {
        return;
    };
    // `.tabstrip` is `position: relative`, so offset_left is already a
    // strip-content coordinate directly comparable to scroll_left.
    let (l, w) = (tab.offset_left(), tab.offset_width());
    let strip: &Element = strip.unchecked_ref();
    let (sl, cw) = (strip.scroll_left(), strip.client_width());
    if l < sl {
        strip.set_scroll_left(l);
    } else if l + w > sl + cw {
        strip.set_scroll_left(l + w - cw);
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
    } else if ctl.disk_modal.get_untracked().is_some() {
        // Dismissed, not answered: the tab keeps its marker and asks again
        // when you come back to it.
        ctl.disk_modal.set(None);
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
    match eff_mode(ctl) {
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
    match eff_mode(ctl) {
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
    match eff_mode(ctl) {
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
    match eff_mode(ctl) {
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
    let wys = eff_mode(ctl) == Mode::Wysiwyg;
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
    let inside = eff_mode(ctl) == Mode::Wysiwyg
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
    // With no document open, only the actions that *create* one are live.
    // Everything else would act on an editor that isn't there: `save` would
    // write an empty file over a real path, `select_all` would select the
    // toolbar, `find` would run `window.find` over the app chrome.
    if !has_doc_now(ctl)
        && !matches!(
            action,
            "new" | "open" | "import_html" | "import_docx" | "open_folder" | "toggle_project"
        )
    {
        return;
    }
    // A picture is not a document. Everything that writes, searches or
    // formats is meant for one, and the tab strip now holds things that are
    // none of those — the toolbar is disabled to match, but menu accelerators
    // reach past it.
    let kind = ctl.kind.get_untracked();
    if kind != DocKind::Markdown
        && (action.starts_with("fmt_")
            || action.starts_with("tbl_")
            || matches!(action, "toggle_mode" | "export_html"))
    {
        return;
    }
    if !kind.is_text() && matches!(action, "find" | "replace" | "select_all" | "save_as") {
        return;
    }
    match action {
        "new" => do_new(ctl),
        "open" => do_open(ctl),
        "open_folder" => do_open_folder(ctl),
        "toggle_project" => ctl.project_open.update(|v| *v = !*v),
        "toggle_overwrite" => hex_toggle_overwrite(ctl),
        "import_html" => do_import_html(ctl),
        "import_docx" => do_import_docx(ctl),
        "save" => do_save(ctl, false, None),
        "save_as" => do_save(ctl, true, None),
        "export_html" => do_export_html(ctl),
        "close_tab" => {
            if let Some(id) = active_id(ctl) {
                close_tab(ctl, id);
            }
        }
        "next_tab" => cycle_tab(ctl, 1),
        "prev_tab" => cycle_tab(ctl, -1),
        // The hex editor keeps its own history: 200 snapshots of a megabyte is
        // not an undo stack, it is a memory leak with a keyboard shortcut.
        "undo" if kind == DocKind::Binary => hex_undo(ctl),
        "redo" if kind == DocKind::Binary => hex_redo(ctl),
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


// The hex editor's own keyboard. It is a grid, not a text field, so
// nothing here is inherited: every key that does something is listed.
fn on_hex_keydown(ctl: Ctl, e: web_sys::KeyboardEvent) {
    if e.ctrl_key() || e.meta_key() || e.alt_key() {
        return; // Ctrl+S, Ctrl+Z and friends belong to the window handler
    }
    let key = e.key();
    let rows = ctl.hex_rows.get_untracked().saturating_sub(2).max(1) as isize;
    match key.as_str() {
        "ArrowRight" => hex_move(ctl, 1),
        "ArrowLeft" => hex_move(ctl, -1),
        "ArrowDown" => hex_move(ctl, hex::ROW as isize),
        "ArrowUp" => hex_move(ctl, -(hex::ROW as isize)),
        "PageDown" => hex_move(ctl, rows * hex::ROW as isize),
        "PageUp" => hex_move(ctl, -(rows * hex::ROW as isize)),
        "Home" => hex_set_cursor(ctl, 0),
        "End" => hex_set_cursor(ctl, hex_len(ctl)),
        "Insert" => hex_toggle_overwrite(ctl),
        "Backspace" => hex_delete(ctl, true),
        "Delete" => hex_delete(ctl, false),
        _ => {
            let Some(v) = hex::hex_digit(&key) else {
                return; // not ours; let the browser have it
            };
            hex_type(ctl, v);
        }
    }
    e.prevent_default();
}

// ---------- panes ----------

/// The directory tree. Split out of `App`'s `view!` deliberately: the macro
/// builds one deeply nested type per element, and a debug build overflows the
/// wasm stack long before the browser complains about anything else.
fn project_pane(ctl: Ctl) -> impl IntoView {
    view! {
        <aside class="project" class:hidden=move || !ctl.project_open.get()>
            <div class="project-head">
                <span class="project-root" title=move || ctl.project_root.get()>
                    {move || {
                        let r = ctl.project_root.get();
                        if r.is_empty() {
                            "No folder".to_string()
                        } else {
                            r.rsplit(['/', '\\']).next().unwrap_or(&r).to_string()
                        }
                    }}
                </span>
                <button
                    class="project-btn"
                    title="Choose folder (Ctrl+Shift+K)"
                    on:mousedown=|e: web_sys::MouseEvent| e.prevent_default()
                    on:click=move |_| do_open_folder(ctl)
                >
                    "…"
                </button>
            </div>
            <div class="project-tree">
                <Show
                    when=move || !ctl.project_root.get().is_empty()
                    fallback=move || {
                        view! {
                            <button
                                class="btn project-empty"
                                on:mousedown=|e: web_sys::MouseEvent| e.prevent_default()
                                on:click=move |_| do_open_folder(ctl)
                            >
                                "Open Folder…"
                            </button>
                        }
                    }
                >
                    // Keyed on the open state as well as the path: a row's
                    // markup is built once from the captured `TreeNode`, so
                    // without this its disclosure triangle would never change.
                    <For
                        each=move || ctl.tree.get()
                        key=|n| (n.path.clone(), n.open)
                        let(node)
                    >
                        {
                            let path = node.path.clone();
                            let open_path = node.path.clone();
                            view! {
                                <div
                                    class="tree-row"
                                    class:dir=node.dir
                                    class:open=node.open
                                    class:active=move || {
                                        ctl.doc_path.get() == path
                                    }
                                    style=format!("padding-left:{}px", 6 + node.depth * 12)
                                    title=node.path.clone()
                                    // Same reason as the tab strip: a bare
                                    // click here would collapse the caret
                                    // that is about to be stashed.
                                    on:mousedown=move |e: web_sys::MouseEvent| {
                                        if e.button() != 0 {
                                            return;
                                        }
                                        e.prevent_default();
                                        if node.dir {
                                            toggle_dir(ctl, open_path.clone());
                                        } else {
                                            open_tree_file(ctl, open_path.clone());
                                        }
                                    }
                                >
                                    <span class="tree-mark"></span>
                                    <span class="tree-name">{node.name.clone()}</span>
                                </div>
                            }
                        }
                    </For>
                </Show>
            </div>
        </aside>
    }
}

/// The three tabs that are not an editor: a picture, a hex dump, and a file
/// too big for either.
fn asset_panes(ctl: Ctl) -> impl IntoView {
    view! {
            // Read-only by construction: there is no image editor here,
            // and `object-fit` keeps a large one inside the pane.
            <Show when=move || ctl.kind.get() == DocKind::Image>
                <div class="asset-view">
                    <img class="asset-image" src=move || ctl.data.get() alt="" />
                </div>
            </Show>

            <Show when=move || ctl.kind.get() == DocKind::Binary>
                <div
                    class="hex-view"
                    tabindex="0"
                    node_ref=ctl.hex_ref
                    on:scroll=move |_| hex_recompute_window(ctl)
                    on:keydown=move |e| on_hex_keydown(ctl, e)
                >
                    // Holds the scrollbar open for the whole file; only the
                    // rows in view are ever built.
                    <div
                        class="hex-spacer"
                        style=move || {
                            ctl.hex_gen.get();
                            format!(
                                "height:{}px",
                                ctl.hex.with_value(hex::HexDoc::rows) * HEX_ROW_H,
                            )
                        }
                    >
                        <div
                            class="hex-rows"
                            style=move || {
                                format!("transform:translateY({}px)", ctl.hex_top.get() * HEX_ROW_H)
                            }
                        >
                            <For
                                each=move || {
                                    ctl.hex_gen.get();
                                    let rows = ctl.hex.with_value(hex::HexDoc::rows);
                                    let top = ctl.hex_top.get().min(rows.saturating_sub(1));
                                    (top..(top + ctl.hex_rows.get()).min(rows)).collect::<Vec<_>>()
                                }
                                key=|r| *r
                                let(row)
                            >
                                {move || hex_row(ctl, row)}
                            </For>
                        </div>
                    </div>
                </div>
            </Show>

            <Show when=move || ctl.kind.get() == DocKind::Oversized>
                <div class="asset-view">
                    <div class="empty-state">
                        <p class="empty-hint">"Too large to open"</p>
                        <p class="empty-sub">
                            {move || {
                                format!(
                                    "{} is {}. The limit is {} for text and binaries, {} for images.",
                                    ctl.doc_name.get(),
                                    human_size(ctl.doc_size.get()),
                                    human_size(TEXT_LIMIT),
                                    human_size(IMAGE_LIMIT),
                                )
                            }}
                        </p>
                    </div>
                </div>
            </Show>
    }
}

/// The right-hand end of the status bar, which says something different for
/// every kind of tab.
fn status_metrics(ctl: Ctl) -> impl IntoView {
    view! {
            <span class="status-metrics" class:hidden=move || !has_doc(ctl)>
                <Show
                    when=move || ctl.kind.get().is_text()
                    fallback=move || {
                        view! {
                            // For a binary this is its own live length, which
                            // inserting and deleting move away from the size
                            // on disk; for the rest, the size on disk is all
                            // there is to say.
                            <span>
                                {move || {
                                    ctl.hex_gen.get();
                                    if ctl.kind.get() == DocKind::Binary {
                                        human_size(ctl.hex.with_value(hex::HexDoc::len) as u64)
                                    } else {
                                        human_size(ctl.doc_size.get())
                                    }
                                }}
                            </span>
                            <Show when=move || ctl.kind.get() == DocKind::Binary>
                                <span class="status-sep">"·"</span>
                                <span>
                                    {move || {
                                        format!("0x{:08x}", ctl.hex_cursor.get())
                                    }}
                                </span>
                                <span class="status-sep">"·"</span>
                                <button
                                    class="status-btn"
                                    title="Insert / overwrite (Ins, or Ctrl+Shift+O)"
                                    on:mousedown=|e: web_sys::MouseEvent| e.prevent_default()
                                    on:click=move |_| hex_toggle_overwrite(ctl)
                                >
                                    {move || {
                                        if ctl.hex_overwrite.get() { "OVR" } else { "INS" }
                                    }}
                                </button>
                            </Show>
                        }
                    }
                >
                    <span>{move || format!("{} words", ctl.words.get())}</span>
                    <span class="status-sep">"·"</span>
                    <span>{move || format!("{} chars", ctl.chars.get())}</span>
                    <span class="status-sep">"·"</span>
                    <span>
                        {move || {
                            match ctl.kind.get() {
                                DocKind::Text => "Text",
                                _ if ctl.mode.get() == Mode::Source => "Markdown",
                                _ => "WYSIWYG",
                            }
                        }}
                    </span>
                </Show>
            </span>
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
            // One line disables all 20 format and table buttons in the empty
            // frame, and again for a tab holding something that has no
            // markdown to format.
            prop:disabled=move || !has_doc(ctl) || ctl.kind.get() != DocKind::Markdown
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
        last_status: StoredValue::new(None),
        docs: StoredValue::new(Vec::new()),
        tabs: RwSignal::new(Vec::new()),
        active: RwSignal::new(None),
        next_id: StoredValue::new(0),
        switch_gen: StoredValue::new(0),
        tab_menu: RwSignal::new(None),
        tabs_overflow: RwSignal::new(false),
        tabstrip_ref: NodeRef::new(),
        disk_modal: RwSignal::new(None),
        last_watched: StoredValue::new(Vec::new()),
        kind: RwSignal::new(DocKind::Markdown),
        doc_size: RwSignal::new(0),
        data: RwSignal::new(String::new()),
        hex: StoredValue::new(hex::HexDoc::default()),
        hex_gen: RwSignal::new(0),
        hex_cursor: RwSignal::new(0),
        hex_high: RwSignal::new(true),
        hex_overwrite: RwSignal::new(true),
        hex_top: RwSignal::new(0),
        hex_rows: RwSignal::new(40),
        hex_ref: NodeRef::new(),
        project_open: RwSignal::new(false),
        project_root: RwSignal::new(String::new()),
        tree: RwSignal::new(Vec::new()),
        owner: StoredValue::new_local(Owner::current()),
    };

    // The first document exists before the view does: `sync_active_doc`
    // indexes `docs[active]`, and the editor's init effect must not run
    // against an empty tab list.
    let first = make_doc(ctl, 0, "Untitled", "", false, DocKind::Markdown);
    ctl.docs.update_value(|d| d.push(first));
    ctl.next_id.set_value(1);
    ctl.active.set(Some(0));
    publish_tabs(ctl);

    // Track whether the caret sits inside a table (drives the table toolbar
    // group).
    {
        let cb = Closure::<dyn FnMut()>::new(move || {
            if has_doc_now(ctl) {
                update_in_table(ctl);
            }
        });
        let _ = document()
            .add_event_listener_with_callback("selectionchange", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Per-webview execCommand defaults. Seeding "<p><br></p>" and resetting
    // history used to live here; those are *document* concerns now and belong
    // to `new_tab` -> `load_markdown` and to `activate`, which set the
    // innerHTML themselves. Leaving them here would let a re-fire wipe the
    // undo stack of whichever document was being installed.
    Effect::new(move |_| {
        let Some(el) = ctl.editor_ref.get() else {
            return;
        };
        exec_val("defaultParagraphSeparator", "p");
        exec_val("styleWithCSS", "false");
        if el.child_element_count() == 0 {
            el.set_inner_html("<p><br></p>");
            history_reset(ctl);
        }
        if has_doc_now(ctl) {
            let _ = el.focus();
        }
    });

    // Keep the active tab in view, and re-measure the strip when the tab list
    // or the window changes. Layout is only settled next frame.
    Effect::new(move |_| {
        let _ = ctl.tabs.get();
        let _ = ctl.active.get();
        request_animation_frame(move || {
            scroll_active_tab_into_view(ctl);
            recompute_overflow(ctl);
        });
    });
    // A resize can push the active tab out of view without the tab list
    // changing at all, so it re-runs both.
    window_event_listener(leptos::ev::resize, move |_| {
        scroll_active_tab_into_view(ctl);
        recompute_overflow(ctl);
        hex_recompute_window(ctl);
    });

    // How many hex rows fit is a question about a pane that may have only just
    // been rendered, so it is asked a frame after the tab becomes a binary —
    // and again when the project pane appears and takes width from it.
    Effect::new(move |_| {
        let _ = ctl.kind.get();
        let _ = ctl.project_open.get();
        request_animation_frame(move || {
            hex_recompute_window(ctl);
            // The grid is not a text field: without focus it gets no keys.
            if ctl.kind.get_untracked() == DocKind::Binary {
                if let Some(el) = ctl.hex_ref.get_untracked() {
                    let _ = el.focus();
                }
            }
        });
    });

    // Load a CLI-passed file and hook up backend events.
    spawn_local(async move {
        match tauri_api::invoke_no_args("init_doc").await {
            Ok(v) => {
                for f in tauri_api::parse::<Vec<tauri_api::AnyFile>>(v).unwrap_or_default() {
                    open_any(ctl, f);
                }
            }
            // A file named on the command line that can't be read would
            // otherwise open as a blank Untitled document.
            Err(e) => report_backend_error("Could not open file", &e),
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
            close_window_step(ctl);
        });
        tauri_api::listen("close-requested", close_cb.as_ref().unchecked_ref()).await;
        close_cb.forget();
        // A markdown file was dropped on the window or opened via the OS
        // file association while we're running.
        let drop_cb = Closure::<dyn FnMut(JsValue)>::new(move |ev: JsValue| {
            if let Ok(p) = js_sys::Reflect::get(&ev, &JsValue::from_str("payload")) {
                // Opening never discards anything now, so no dirty gate.
                for path in tauri_api::parse_paths(p) {
                    do_open_path(ctl, path);
                }
            }
        });
        tauri_api::listen("drop-open", drop_cb.as_ref().unchecked_ref()).await;
        drop_cb.forget();
        // A file open in a tab was changed by something else.
        let disk_cb = Closure::<dyn FnMut(JsValue)>::new(move |ev: JsValue| {
            if let Ok(p) = js_sys::Reflect::get(&ev, &JsValue::from_str("payload")) {
                if let Some(path) = p.as_string() {
                    on_disk_changed(ctl, path);
                }
            }
        });
        tauri_api::listen("disk-changed", disk_cb.as_ref().unchecked_ref()).await;
        disk_cb.forget();
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
            ("e", true, false) => "export_html",
            ("k", true, false) => "open_folder",
            ("p", true, false) => "toggle_project",
            // The hex editor's mode switch. `Insert` is handled by the hex
            // pane's own keydown; this is the one that exists on a Mac.
            ("o", true, false) => "toggle_overwrite",
            ("w", false, false) => "close_tab",
            // Ctrl+Tab is unreliable as a *native* accelerator on WebKitGTK
            // (focus traversal eats it), so it lives here; the menu carries
            // PageUp/PageDown for discoverability.
            ("tab", false, false) => "next_tab",
            ("tab", true, false) => "prev_tab",
            ("pagedown", false, false) => "next_tab",
            ("pageup", false, false) => "prev_tab",
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
        // The modifier check matters: without it Ctrl+Tab indents a list here
        // and the window listener never sees the tab-cycling shortcut.
        if e.key() == "Tab" && !e.ctrl_key() && !e.meta_key() {
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
        if e.key() == "Tab" && !e.shift_key() && !e.ctrl_key() && !e.meta_key() {
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
        if eff_mode(ctl) != Mode::Wysiwyg {
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
                    prop:disabled=move || !has_doc(ctl) || ctl.kind.get() != DocKind::Markdown
                    on:mousedown=|e| e.prevent_default()
                    on:click=move |_| do_action(ctl, "toggle_mode")
                >
                    {move || if mode_is(Mode::Source) { "WYSIWYG" } else { "Markdown" }}
                </button>
            </div>

            <div class="body">
            {project_pane(ctl)}
            <div class="main">
            <div class="tabbar" class:hidden=move || !has_doc(ctl)>
                <div class="tabstrip" node_ref=ctl.tabstrip_ref>
                    <For each=move || ctl.tabs.get() key=|t| t.id let(tab)>
                        <div
                            class="tab"
                            class:active=move || active_id(ctl) == Some(tab.id)
                            class:dirty=move || tab.dirty.get()
                            class:stale=move || tab.stale.get()
                            title=move || {
                                let p = index_of(ctl, tab.id)
                                    .and_then(|i| ctl.docs.with_value(|d| d.get(i).map(|x| x.path)))
                                    .map(|s| s.get())
                                    .unwrap_or_default();
                                let base = if p.is_empty() {
                                    format!("{} — unsaved", tab.name.get())
                                } else {
                                    p
                                };
                                if tab.stale.get() {
                                    format!("{base}\nChanged on disk")
                                } else {
                                    base
                                }
                            }
                            // Keep the selection inside the editor: it is the
                            // caret we are about to stash, and a bare click on
                            // the strip would collapse it to offset 0.
                            on:mousedown=move |e: web_sys::MouseEvent| {
                                match e.button() {
                                    // Middle click closes. preventDefault here
                                    // (not on auxclick) beats X11 autoscroll.
                                    1 => {
                                        e.prevent_default();
                                        close_tab(ctl, tab.id);
                                    }
                                    0 => {
                                        e.prevent_default();
                                        switch_to_id(ctl, tab.id);
                                    }
                                    _ => {}
                                }
                            }
                            on:auxclick=|e: web_sys::MouseEvent| e.prevent_default()
                        >
                            <span class="tab-name">{move || tab.name.get()}</span>
                            <button
                                class="tab-x"
                                title="Close (Ctrl+W)"
                                on:mousedown=move |e: web_sys::MouseEvent| {
                                    e.stop_propagation();
                                    e.prevent_default();
                                }
                                on:click=move |e: web_sys::MouseEvent| {
                                    e.stop_propagation();
                                    close_tab(ctl, tab.id);
                                }
                            ></button>
                        </div>
                    </For>
                </div>
                <button
                    class="tb-btn tab-more"
                    class:invisible=move || !ctl.tabs_overflow.get()
                    title="All open documents"
                    on:mousedown=|e: web_sys::MouseEvent| e.prevent_default()
                    on:click=move |e: web_sys::MouseEvent| {
                        let Some(el) = e
                            .current_target()
                            .and_then(|t| t.dyn_into::<Element>().ok()) else { return };
                        let r = el.get_bounding_client_rect();
                        let vw = window()
                            .inner_width()
                            .ok()
                            .and_then(|v| v.as_f64())
                            .unwrap_or(800.0);
                        // Anchored from the right so the menu grows leftward
                        // and can never spill off-screen.
                        ctl.tab_menu.set(Some(((vw - r.right()).max(4.0), r.bottom() + 2.0)));
                    }
                >
                    "»"
                </button>
            </div>

            <div class="findbar" class:hidden=move || !ctl.find_open.get() || !has_doc(ctl)>
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
                    class:hidden=move || !shows_wysiwyg(ctl)
                    // Not merely hidden: an editor that is momentarily visible
                    // or Tab-reachable must not accept typing into a document
                    // that does not exist.
                    contenteditable=move || if has_doc(ctl) { "true" } else { "false" }
                    spellcheck="true"
                    // Never move this behind a <Show>. NodeRef has no unmount
                    // hook (`load` is its only writer), so unmounting would
                    // leave `editor_el` returning a *detached* element
                    // forever, and every `if let Some(el)` guard in this file
                    // would pass and then write into nothing.
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
                    class:hidden=move || !shows_source(ctl)
                    prop:disabled=move || !has_doc(ctl)
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
                {asset_panes(ctl)}
                <Show when=move || !has_doc(ctl)>
                    <div class="empty-state">
                        <p class="empty-hint">"No document open"</p>
                        <div class="empty-actions">
                            <button
                                class="btn primary"
                                on:mousedown=|e: web_sys::MouseEvent| e.prevent_default()
                                on:click=move |_| do_action(ctl, "new")
                            >
                                "New document"
                            </button>
                            <button
                                class="btn"
                                on:mousedown=|e: web_sys::MouseEvent| e.prevent_default()
                                on:click=move |_| do_action(ctl, "open")
                            >
                                "Open…"
                            </button>
                        </div>
                        // Honest: DragDrop is a native window event, so it
                        // fires over this div exactly as over the editor.
                        <p class="empty-sub">"or drop a Markdown file here"</p>
                    </div>
                </Show>
            </div>
            </div>
            </div>

            <div class="statusbar">
                <span class="status-path">
                    {move || {
                        if !has_doc(ctl) {
                            return "No document".to_string();
                        }
                        let p = ctl.doc_path.get();
                        if p.is_empty() { "unsaved".to_string() } else { p }
                    }}
                </span>
                <span class="tb-spacer"></span>
                // Words and characters mean nothing for a picture or a
                // binary; those get their size, and the hex editor gets the
                // two things it is useless without — where the cursor is, and
                // whether typing will overwrite or insert.
                {status_metrics(ctl)}
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

            <Show when=move || ctl.tab_menu.get().is_some()>
                <div
                    class="ctx-backdrop"
                    on:mousedown=move |_| ctl.tab_menu.set(None)
                    on:contextmenu=move |e: web_sys::MouseEvent| {
                        e.prevent_default();
                        ctl.tab_menu.set(None);
                    }
                ></div>
                <div
                    class="ctxmenu tabmenu"
                    style=move || {
                        let (right, top) = ctl.tab_menu.get().unwrap_or((6.0, 75.0));
                        format!("right:{right}px;top:{top}px")
                    }
                >
                    <For each=move || ctl.tabs.get() key=|t| t.id let(tab)>
                        <button
                            class="ctx-item"
                            class:active=move || active_id(ctl) == Some(tab.id)
                            on:click=move |_| {
                                ctl.tab_menu.set(None);
                                switch_to_id(ctl, tab.id);
                            }
                        >
                            {move || {
                                format!(
                                    "{}{}",
                                    if tab.dirty.get() { "• " } else { "" },
                                    tab.name.get(),
                                )
                            }}
                        </button>
                    </For>
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
                                    match p {
                                        Some(Pending::CloseTab(id)) => close_tab_forced(ctl, id),
                                        // Clearing dirty first is what ends
                                        // the walk: otherwise the next step
                                        // finds this same document forever.
                                        Some(Pending::CloseWindow) => {
                                            mark_clean(ctl);
                                            close_window_step(ctl);
                                        }
                                        None => {}
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

            <Show when=move || ctl.disk_modal.get().is_some()>
                <div class="overlay">
                    <div class="modal">
                        <p class="modal-title">
                            {move || format!("{} changed on disk", ctl.doc_name.get())}
                        </p>
                        <p class="modal-sub">
                            {move || {
                                if ctl.dirty.get() {
                                    "Another program wrote to this file while you had \
                                     unsaved changes. Reloading discards yours; merging \
                                     combines both and marks any clashes."
                                } else {
                                    "Another program wrote to this file. Reloading picks \
                                     up its contents."
                                }
                            }}
                        </p>
                        <div class="modal-btns">
                            <button class="btn primary" on:click=move |_| disk_reload(ctl)>
                                "Reload"
                            </button>
                            // Nothing to merge in a document with no local
                            // edits: the merge would just be the reload.
                            // Merging is a text operation; there is no sensible
                            // three-way merge of a PNG.
                            <Show when=move || ctl.dirty.get() && ctl.kind.get().is_text()>
                                <button class="btn" on:click=move |_| disk_merge(ctl)>
                                    "Merge"
                                </button>
                            </Show>
                            <button class="btn" on:click=move |_| disk_keep(ctl)>
                                "Keep Mine"
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
