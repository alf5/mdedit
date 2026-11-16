//! mdedit's model and logic: everything that needs no UI, so all of it can be
//! tested with plain `cargo test`.
//!
//! The app crate (`crates/mdedit`) draws these with GPUI; nothing here knows
//! GPUI exists.

pub mod buffer;
pub mod detach;
pub mod display;
pub mod docx;
pub mod edit;
pub mod export;
pub mod find;
pub mod hex;
pub mod history;
pub mod html_import;
pub mod markdown;
pub mod math;
pub mod mermaid;
pub mod os;
pub mod prefs;
pub mod project;
pub mod syntax;
pub mod table;
pub mod themes;
pub mod watch;
