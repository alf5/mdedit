//! Export the document as PDF, by printing it.
//!
//! There is no second renderer here. The frontend hands over the very same
//! standalone HTML that **Export as HTML** writes — `document.css` embedded,
//! mermaid already inline SVG — and this module paginates it with whatever
//! print machinery the platform's webview already owns: `WebKitPrintOperation`
//! on Linux, `ICoreWebView2_7::PrintToPdf` on Windows, `NSPrintOperation` on
//! macOS. A PDF is therefore the exported HTML with page breaks in it, and the
//! two can no more drift apart than the export and the editor can.
//!
//! The pagination itself is CSS: see the `@media print` block at the foot of
//! `document.css`. Margins are the one thing that sheet cannot state once and
//! be done with, because WebKit ignores `@page { margin }` — measurably: a
//! page laid out with 18mm in the stylesheet and zero in the print settings
//! comes out with its text against the paper edge. So the two WebKit backends
//! are told the margin directly, in [`MARGIN_MM`], and Windows is left at zero
//! because Chromium does honour the rule. All three numbers have to agree.
//!
//! The document is rendered in a **hidden window of its own**, never the
//! editor's webview. The editor's holds `contenteditable` markup, the app's
//! chrome, and — in source mode — a textarea of raw markdown; none of that is
//! what should come out of a printer.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;
use std::time::Duration;

use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_dialog::DialogExt;

/// How long the hidden window gets to parse and lay out the document. Ample
/// for a local file with no network fetches in it — the export embeds
/// everything — so hitting this means something is wrong, not slow.
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the print backend gets to produce the file.
const PRINT_TIMEOUT: Duration = Duration::from_secs(120);

/// The page margin in millimetres, vertical then horizontal, for the backends
/// that have to be told. Keep in step with `@page` in `document.css`, which is
/// the same margin for Chromium and for a browser printing an exported file.
#[cfg(not(windows))]
const MARGIN_MM: (f64, f64) = (18.0, 16.0);

/// Ask where the PDF goes, then render the document into it.
///
/// `content` is the standalone HTML from `export::build_document`; `path` is
/// the tab's markdown path, used only to seed the dialog. Like `export_html`,
/// an export never touches the markdown file, never re-targets the tab and
/// stays out of the recent files list. `Ok(None)` means the user cancelled.
#[tauri::command]
pub async fn export_pdf(
    app: AppHandle,
    content: String,
    path: Option<String>,
) -> Result<Option<String>, String> {
    let current = path.as_deref().filter(|p| !p.is_empty()).map(Path::new);
    let stem = current
        .and_then(Path::file_stem)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_string());

    let mut dialog = app
        .dialog()
        .file()
        .add_filter("PDF", &["pdf"])
        .set_file_name(format!("{stem}.pdf"));
    if let Some(dir) = current.and_then(Path::parent) {
        dialog = dialog.set_directory(dir);
    }
    let Some(fp) = dialog.blocking_save_file() else {
        return Ok(None);
    };
    let out = fp.into_path().map_err(|e| e.to_string())?;

    render(&app, &content, &stem, &out)?;
    Ok(Some(out.display().to_string()))
}

/// Load `content` into a hidden window and print that window to `out`.
fn render(app: &AppHandle, content: &str, title: &str, out: &Path) -> Result<(), String> {
    let page = TempPage::write(content)?;

    // `on_page_load` wants `Fn + Sync`, which a bare Sender is not.
    let (loaded_tx, loaded_rx) = channel::<()>();
    let loaded_tx = Mutex::new(loaded_tx);

    let id = next_id();
    let win =
        WebviewWindowBuilder::new(app, format!("pdf-{id}"), WebviewUrl::External(page.url()?))
            // The job name most backends put on the spool, and the fallback name
            // some of them suggest when saving.
            .title(title)
            // Never shown, but a webview still lays out to *some* width, and the
            // print box is derived from the document rather than from this. Kept
            // near a portrait page so a viewport-relative unit, if a document ever
            // contains one, lands somewhere sane.
            .inner_size(816.0, 1056.0)
            .visible(false)
            .skip_taskbar(true)
            .focused(false)
            .on_page_load(move |_, payload| {
                if payload.event() == PageLoadEvent::Finished {
                    if let Ok(tx) = loaded_tx.lock() {
                        let _ = tx.send(());
                    }
                }
            })
            .build()
            .map_err(|e| format!("could not open a window to render in: {e}"))?;

    let outcome = loaded_rx
        .recv_timeout(LOAD_TIMEOUT)
        .map_err(|_| "timed out laying the document out".to_string())
        .and_then(|()| print_to_file(&win, out));

    // Whatever happened, the window goes: it is invisible, so a leaked one
    // would keep the process alive after the last real window closed.
    let _ = win.destroy();
    outcome
}

/// Wait for a platform print backend that reports completion on a channel.
fn wait(rx: Receiver<Result<(), String>>) -> Result<(), String> {
    match rx.recv_timeout(PRINT_TIMEOUT) {
        Ok(result) => result,
        Err(_) => Err("timed out writing the PDF".to_string()),
    }
}

/// Send a backend's failure, if the setup for it failed before it could
/// report one itself. A closed channel means the wait already timed out.
fn fail(tx: &Sender<Result<(), String>>, e: impl std::fmt::Display) {
    let _ = tx.send(Err(e.to_string()));
}

// ---------- Linux / BSD: WebKitGTK ----------

#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
fn print_to_file(win: &WebviewWindow, out: &Path) -> Result<(), String> {
    use std::cell::RefCell;
    use std::rc::Rc;
    use webkit2gtk::{PrintOperation, PrintOperationExt};

    let uri = Url::from_file_path(out)
        .map_err(|()| format!("not an absolute path: {}", out.display()))?
        .to_string();
    let (tx, rx) = channel::<Result<(), String>>();

    win.with_webview(move |wv| {
        let op = PrintOperation::new(&wv.inner());

        // WebKitGTK's own recipe for a fileful print: select GTK's file
        // backend by naming its printer, then say where the file goes and in
        // what format. The name is what GTK calls the backend in the print
        // dialog, so a non-English GTK may not match it — hence the error
        // path, rather than a silent no-op.
        let settings = gtk::PrintSettings::new();
        settings.set_printer("Print to File");
        settings.set("output-uri", Some(uri.as_str()));
        settings.set("output-file-format", Some("pdf"));
        op.set_print_settings(&settings);

        // Paper stays at the locale's default (A4 or Letter). The margins do
        // not: WebKit will not take them from `@page`, so they are set here.
        let (v, h) = MARGIN_MM;
        let setup = gtk::PageSetup::new();
        setup.set_top_margin(v, gtk::Unit::Mm);
        setup.set_bottom_margin(v, gtk::Unit::Mm);
        setup.set_left_margin(h, gtk::Unit::Mm);
        setup.set_right_margin(h, gtk::Unit::Mm);
        op.set_page_setup(&setup);

        // `print` returns as soon as the job is queued, so the operation has
        // to outlive this closure or the job dies with it. The handlers hold
        // the reference and drop it when the job ends — a cycle, deliberately,
        // broken at exactly the moment the job can no longer need it.
        let keep = Rc::new(RefCell::new(Some(op.clone())));

        let done = tx.clone();
        let done_keep = keep.clone();
        op.connect_finished(move |_| {
            let _ = done.send(Ok(()));
            done_keep.borrow_mut().take();
        });
        op.connect_failed(move |_, e| {
            fail(&tx, e);
            keep.borrow_mut().take();
        });

        op.print();
    })
    .map_err(|e| format!("could not reach the webview: {e}"))?;

    wait(rx)
}

// ---------- Windows: WebView2 ----------

#[cfg(windows)]
fn print_to_file(win: &WebviewWindow, out: &Path) -> Result<(), String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Environment6, ICoreWebView2_7,
    };
    use webview2_com::PrintToPdfCompletedHandler;
    use windows::core::{Interface, HSTRING, PCWSTR};

    let target = HSTRING::from(out.as_os_str());
    let (tx, rx) = channel::<Result<(), String>>();

    win.with_webview(move |wv| {
        let queued = (|| -> windows::core::Result<()> {
            let printer: ICoreWebView2_7 = unsafe { wv.controller().CoreWebView2()? }.cast()?;
            let env: ICoreWebView2Environment6 = wv.environment().cast()?;
            let settings = unsafe { env.CreatePrintSettings()? };
            unsafe {
                // Code blocks, table headers and alerts are told apart by
                // their fill, and print drops fills unless asked.
                settings.SetShouldPrintBackgrounds(true)?;
                // Zero, because this is Chromium and it does honour the
                // `@page` margin in document.css. Anything here would add.
                settings.SetMarginTop(0.0)?;
                settings.SetMarginBottom(0.0)?;
                settings.SetMarginLeft(0.0)?;
                settings.SetMarginRight(0.0)?;
            }

            let done = tx.clone();
            let handler = PrintToPdfCompletedHandler::create(Box::new(move |result, wrote| {
                let _ = done.send(match result {
                    Err(e) => Err(e.to_string()),
                    Ok(()) if !wrote => Err("the print backend wrote nothing".to_string()),
                    Ok(()) => Ok(()),
                });
                Ok(())
            }));
            unsafe { printer.PrintToPdf(PCWSTR(target.as_ptr()), &settings, &handler) }
        })();
        if let Err(e) = queued {
            fail(&tx, e);
        }
    })
    .map_err(|e| format!("could not reach the webview: {e}"))?;

    wait(rx)
}

// ---------- macOS: AppKit ----------

#[cfg(target_os = "macos")]
fn print_to_file(win: &WebviewWindow, out: &Path) -> Result<(), String> {
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{NSPrintInfo, NSPrintJobSavingURL, NSPrintSaveJob};
    use objc2_foundation::{NSCopying, NSString, NSURL};
    use objc2_web_kit::WKWebView;

    let target = out.to_path_buf();
    let (tx, rx) = channel::<Result<(), String>>();

    win.with_webview(move |wv| {
        let Some(path) = target.to_str() else {
            fail(&tx, format!("not valid UTF-8: {}", target.display()));
            return;
        };
        // `runOperation` is synchronous once both panels are off, so the
        // result is known by the time this closure returns.
        let wrote = unsafe {
            let view: &WKWebView = &*wv.inner().cast();

            let info = NSPrintInfo::new();
            // WebKit here too, so the margins are set rather than left to
            // `@page`. AppKit measures them in points.
            let (v, h) = (mm_to_points(MARGIN_MM.0), mm_to_points(MARGIN_MM.1));
            info.setTopMargin(v);
            info.setBottomMargin(v);
            info.setLeftMargin(h);
            info.setRightMargin(h);
            // "Save", not "spool": a job disposition of save plus a URL is how
            // AppKit prints to a file rather than to a printer.
            info.setJobDisposition(NSPrintSaveJob);
            let url = NSURL::fileURLWithPath(&NSString::from_str(path));
            info.dictionary().setObject_forKey(
                &url,
                ProtocolObject::<dyn NSCopying>::from_ref(NSPrintJobSavingURL),
            );

            let op = view.printOperationWithPrintInfo(&info);
            op.setShowsPrintPanel(false);
            op.setShowsProgressPanel(false);
            op.runOperation()
        };
        let _ = tx.send(if wrote {
            Ok(())
        } else {
            Err("the print backend wrote nothing".to_string())
        });
    })
    .map_err(|e| format!("could not reach the webview: {e}"))?;

    wait(rx)
}

/// AppKit works in points: 72 to the inch, 25.4 millimetres to the inch.
#[cfg(target_os = "macos")]
fn mm_to_points(mm: f64) -> f64 {
    mm / 25.4 * 72.0
}

/// The document, on disk, for the hidden window to load.
///
/// A webview navigates to a URL, and the document is far too big to be one —
/// so it goes to a file, which is deleted however this ends.
struct TempPage(PathBuf);

impl TempPage {
    fn write(content: &str) -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!(
            "mdedit-print-{}-{}.html",
            std::process::id(),
            next_id()
        ));
        std::fs::write(&path, content)
            .map_err(|e| format!("could not stage the document for printing: {e}"))?;
        Ok(Self(path))
    }

    fn url(&self) -> Result<Url, String> {
        Url::from_file_path(&self.0)
            .map_err(|()| "could not address the staged document".to_string())
    }
}

impl Drop for TempPage {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Distinguishes one export from the next, in window labels and file names.
/// A window label may not be reused while its window lives.
fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
