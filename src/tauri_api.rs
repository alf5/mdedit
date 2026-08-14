//! Bindings to the Tauri IPC globals (withGlobalTauri) and window.find.

use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], js_name = invoke, catch)]
    pub async fn invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "event"], js_name = listen)]
    pub async fn listen(event: &str, handler: &js_sys::Function) -> JsValue;

    /// Non-standard but supported by WebKit and Chromium (WebView2).
    #[wasm_bindgen(js_namespace = window, js_name = find)]
    pub fn window_find(search: &str, case_sensitive: bool, backwards: bool, wrap: bool) -> bool;
}

#[derive(Deserialize, Clone, Debug)]
pub struct FileInfo {
    pub path: String,
    pub name: String,
    #[serde(default)]
    pub content: String,
}

#[derive(Serialize)]
pub struct SaveArgs {
    pub content: String,
    /// The tab's path, or None for a document that has never been saved.
    pub path: Option<String>,
}

/// Tauri v2 camelCases command arguments, so `any_dirty` has to be renamed or
/// it arrives as a missing argument and the invoke is rejected.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleArgs {
    pub name: String,
    pub dirty: bool,
    pub any_dirty: bool,
    /// The active tab holds markdown, so the formatting menus mean something.
    pub markdown: bool,
    /// It holds text of some kind, so Save As does.
    pub text: bool,
}

#[derive(Serialize)]
pub struct PathArgs {
    pub path: String,
}

#[derive(Serialize)]
pub struct PathsArgs {
    pub paths: Vec<String>,
}

/// A three-way merge request: `base` is the content the tab and the file last
/// agreed on, `mine` is the editor's.
#[derive(Serialize)]
pub struct MergeArgs {
    pub path: String,
    pub base: String,
    pub mine: String,
}

#[derive(Deserialize)]
pub struct MergeResult {
    pub text: String,
    pub disk: String,
    pub conflicted: bool,
}

/// One row of a directory listing.
#[derive(Deserialize)]
pub struct Entry {
    pub path: String,
    pub name: String,
    pub dir: bool,
}

/// Whatever the backend made of a path: markdown, plain text, an image, a
/// binary, or too big to show.
#[derive(Deserialize)]
pub struct AnyFile {
    pub path: String,
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub text: String,
    /// Base64 — the image's bytes or the binary's.
    #[serde(default)]
    pub data: String,
    #[serde(default)]
    pub mime: String,
    pub size: u64,
}

#[derive(Serialize)]
pub struct BytesArgs {
    pub path: String,
    /// Base64.
    pub data: String,
}

#[derive(Deserialize)]
pub struct SaveResult {
    pub file: Option<FileInfo>,
    /// Nothing was written: the file changed on disk first.
    pub conflict: bool,
}

pub async fn invoke_no_args(cmd: &str) -> Result<JsValue, JsValue> {
    invoke(cmd, JsValue::UNDEFINED).await
}

pub fn parse<T: serde::de::DeserializeOwned>(v: JsValue) -> Option<T> {
    serde_wasm_bindgen::from_value::<T>(v).ok()
}

pub fn parse_file_infos(v: JsValue) -> Vec<FileInfo> {
    serde_wasm_bindgen::from_value::<Vec<FileInfo>>(v).unwrap_or_default()
}

/// A `drop-open` payload: one path, or several from a multi-file drop.
pub fn parse_paths(v: JsValue) -> Vec<String> {
    if let Ok(list) = serde_wasm_bindgen::from_value::<Vec<String>>(v.clone()) {
        return list;
    }
    v.as_string().into_iter().collect()
}
