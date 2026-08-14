mod app;
mod error_dialog;
mod export;
mod history;
mod markdown;
mod mermaid;
mod serialize;
mod tauri_api;

use app::*;
use leptos::prelude::*;

fn main() {
    // Before anything else, so a panic during mount still gets reported.
    error_dialog::install();
    mount_to_body(|| {
        view! {
            <App/>
        }
    })
}
