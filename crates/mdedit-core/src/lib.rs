//! mdedit's model and logic: everything that needs no UI, so all of it can be
//! tested with plain `cargo test`.
//!
//! The app crate (`crates/mdedit`) draws these with GPUI; nothing here knows
//! GPUI exists.

pub mod detach;
pub mod docx;
pub mod export;
pub mod hex;
pub mod markdown;
pub mod math;
pub mod mermaid;
pub mod os;
pub mod prefs;
pub mod project;
pub mod table;
pub mod themes;
pub mod watch;
