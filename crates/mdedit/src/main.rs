//! mdedit 2: a Markdown editor on GPUI. Work in progress on the `v2-gpui`
//! branch; see `docs/v2-notes.md`.
//!
//! `mdedit [--detach] [files…]`: files open as tabs (a folder opens in the
//! sidebar). A second launch hands its files to the running window and
//! exits; `--detach` returns the shell prompt immediately.

mod actions;
mod assets;
mod doc;
mod files;
mod hex_view;
mod single;
mod state;
mod theme;
mod workspace;

use std::path::PathBuf;

use gpui_kit::component::TitleBar;
use gpui_kit::*;

use crate::state::AppState;
use crate::workspace::Workspace;

/// The files named on the command line, absolute: a second instance hands
/// them to a first that may have a different working directory.
fn paths_from_args() -> Vec<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_default();
    std::env::args_os()
        .skip(1)
        .filter(|a| !a.to_string_lossy().starts_with('-'))
        .map(|a| {
            let p = PathBuf::from(a);
            if p.is_absolute() { p } else { cwd.join(p) }
        })
        .map(|p| p.canonicalize().unwrap_or(p))
        .collect()
}

/// `%20` and friends in a `file://` URL's path.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn main() {
    // Before anything starts a thread: `fork` is only safe single-threaded.
    mdedit_core::detach::maybe_detach();
    let paths = paths_from_args();
    let (finder, incoming) = match single::claim(&paths) {
        single::Role::Primary(tx, rx) => (tx, rx),
        single::Role::Forwarded => return,
    };

    let app = application().with_assets(assets::Assets);
    // macOS hands files from Finder ("Open With", double-click) to the
    // running app as URLs; elsewhere this never fires.
    app.on_open_urls(move |urls| {
        let paths: Vec<PathBuf> = urls
            .iter()
            .filter_map(|u| u.strip_prefix("file://"))
            .map(|p| PathBuf::from(percent_decode(p)))
            .collect();
        if !paths.is_empty() {
            let _ = finder.send_blocking(paths);
        }
    });
    app.run(move |cx| {
        gpui_kit::init(cx);
        theme::register_fonts(cx);
        AppState::init(cx);
        actions::bind_keys(cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1180.), px(780.)),
                cx,
            ))),
            window_min_size: Some(size(px(680.), px(440.))),
            app_id: Some("mdedit".into()),
            window_decorations: Some(WindowDecorations::Client),
            ..TitleBar::window_options()
        };
        let opened = gpui_kit::open_window(options, cx, move |window, cx| {
            let id = AppState::prefs(cx).theme().to_string();
            theme::apply(&id, window, cx);
            cx.new(|cx| Workspace::new(paths, incoming, window, cx))
        });
        if let Err(e) = opened {
            eprintln!("mdedit: couldn't open a window: {e}");
            cx.quit();
        }
    });
}
