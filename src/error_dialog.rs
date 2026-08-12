//! Error reporting, for both kinds of failure the app can have.
//!
//! A **fatal** error is a Rust panic. In WASM that is a hard stop: the release
//! profile builds with `panic = "abort"`, so the module traps and every
//! signal, handler and effect dies with it. Until this existed the window
//! simply sat there — no dialog, no hint, an empty editor — which is exactly
//! how a gantt chart calling `SystemTime::now()` managed to look like "the
//! file won't open" (see [`crate::mermaid::disarm_today_marker`]).
//!
//! A **recoverable** error is a backend command that came back `Err` — a bad
//! path, a permission problem. The app is still alive; the dialog just says
//! what failed.
//!
//! Both are built straight from `web_sys`, not from Leptos: for the fatal
//! case the reactive runtime can't be trusted to render anything by the time
//! it is wanted. Nothing in here may panic, or the hook recurses.

use std::cell::Cell;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Document, Element, HtmlElement};

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// The app is dead; offer a reload.
    Fatal,
    /// The app is fine; the operation failed.
    Recoverable,
}

thread_local! {
    /// At most one dialog at a time — a panic often arrives with company,
    /// and stacked overlays bury the first (most useful) message.
    static OPEN: Cell<bool> = const { Cell::new(false) };
}

/// Install the panic hook and expose the dialog to the page's own error
/// listeners (see `index.html`), which catch what Rust can't: a WASM module
/// that fails to fetch or instantiate, so no Rust ever runs.
pub fn install() {
    std::panic::set_hook(Box::new(|info| {
        // Keep the console's full stack trace — the dialog only has room for
        // the headline, and the stack is what makes a report actionable.
        console_error_panic_hook::hook(info);
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panicked".to_string());
        let detail = match info.location() {
            Some(l) => format!("{payload}\n\nat {}:{}:{}", l.file(), l.line(), l.column()),
            None => payload,
        };
        show_fatal("mdedit hit an internal error", &detail);
    }));

    // window.__MDEDIT_FATAL__(title, detail)
    let cb = Closure::<dyn Fn(String, String)>::new(|title: String, detail: String| {
        show_fatal(&title, &detail);
    });
    if let Some(w) = web_sys::window() {
        let _ = js_sys::Reflect::set(
            &w,
            &JsValue::from_str("__MDEDIT_FATAL__"),
            cb.as_ref().unchecked_ref(),
        );
    }
    cb.forget();
}

/// The app has stopped working and won't recover.
pub fn show_fatal(title: &str, detail: &str) {
    show(title, detail, Kind::Fatal);
}

/// One operation failed; the app carries on.
pub fn show_error(title: &str, detail: &str) {
    show(title, detail, Kind::Recoverable);
}

/// Silently gives up if the DOM is unusable — there is nowhere left to
/// report to at that point.
fn show(title: &str, detail: &str, kind: Kind) {
    if OPEN.with(|s| s.replace(true)) {
        return;
    }
    if build(title, detail, kind).is_none() {
        OPEN.with(|s| s.set(false));
    }
}

fn build(title: &str, detail: &str, kind: Kind) -> Option<()> {
    let doc = web_sys::window()?.document()?;
    let body = doc.body()?;

    let overlay = el(&doc, "div", "overlay error-overlay")?;
    let modal = el(&doc, "div", "modal error-modal")?;

    let h = el(&doc, "p", "modal-title")?;
    h.set_text_content(Some(title));
    modal.append_child(&h).ok()?;

    if kind == Kind::Fatal {
        let sub = el(&doc, "p", "modal-sub")?;
        // Deliberately doesn't promise the document is still there: a panic
        // during load traps before anything is rendered.
        sub.set_text_content(Some(
            "The editor has stopped responding and can no longer save. Dismiss \
             this to copy anything still on screen, or reload to start over.",
        ));
        modal.append_child(&sub).ok()?;
    }

    // textContent, never innerHTML: the detail can hold document content.
    let pre = el(&doc, "pre", "error-detail")?;
    pre.set_text_content(Some(detail));
    modal.append_child(&pre).ok()?;

    let btns = el(&doc, "div", "modal-btns")?;
    let dismiss = button(
        &doc,
        if kind == Kind::Fatal { "Dismiss" } else { "OK" },
        if kind == Kind::Fatal { "btn" } else { "btn primary" },
    )?;
    let ov = overlay.clone();
    on_click(&dismiss, move || {
        ov.remove();
        OPEN.with(|s| s.set(false));
    });
    btns.append_child(&dismiss).ok()?;

    if kind == Kind::Fatal {
        let reload = button(&doc, "Reload", "btn primary")?;
        on_click(&reload, || {
            if let Some(w) = web_sys::window() {
                let _ = w.location().reload();
            }
        });
        btns.append_child(&reload).ok()?;
    }

    modal.append_child(&btns).ok()?;
    overlay.append_child(&modal).ok()?;
    body.append_child(&overlay).ok()?;
    Some(())
}

fn el(doc: &Document, tag: &str, class: &str) -> Option<Element> {
    let e = doc.create_element(tag).ok()?;
    e.set_class_name(class);
    Some(e)
}

fn button(doc: &Document, label: &str, class: &str) -> Option<Element> {
    let b = el(doc, "button", class)?;
    b.set_text_content(Some(label));
    let _ = b.set_attribute("type", "button");
    Some(b)
}

fn on_click(el: &Element, f: impl Fn() + 'static) {
    let cb = Closure::<dyn Fn()>::new(f);
    if let Some(t) = el.dyn_ref::<HtmlElement>() {
        let _ = t.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
    }
    cb.forget();
}
