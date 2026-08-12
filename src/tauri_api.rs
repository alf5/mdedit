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
}

#[derive(Serialize)]
pub struct PathArgs {
    pub path: String,
}

pub async fn invoke_no_args(cmd: &str) -> Result<JsValue, JsValue> {
    invoke(cmd, JsValue::UNDEFINED).await
}

pub fn parse_file_info(v: JsValue) -> Option<FileInfo> {
    serde_wasm_bindgen::from_value::<Option<FileInfo>>(v).ok().flatten()
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
