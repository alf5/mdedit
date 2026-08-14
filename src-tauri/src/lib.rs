mod docx;
mod watch;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent, Wry};
use tauri_plugin_dialog::DialogExt;

const RECENT_MAX: usize = 10;

/// The frontend owns the tab list, so almost nothing about "the document"
/// lives here any more. What remains is what the close guard needs to answer
/// *synchronously* — it runs inside the window event handler and cannot await
/// the webview — plus files the OS handed us before the UI existed.
#[derive(Default)]
struct DocState {
    /// True when any tab has unsaved changes. Pushed by the frontend.
    any_dirty: bool,
    /// Set once the frontend has drained `pending_open`; before that, OS open
    /// events must be stashed rather than emitted into the void.
    ready: bool,
    /// Files handed to us by the OS (macOS open event) before the frontend
    /// was ready to receive events.
    pending_open: Vec<PathBuf>,
    /// Canonical paths, most-recent-first, capped at `RECENT_MAX`.
    recent: Vec<String>,
    /// Live handle to File ▸ Open Recent, repopulated in place at runtime.
    recent_menu: Option<Submenu<Wry>>,
    /// Menu entries that mean nothing with no document open.
    doc_items: Vec<MenuItem<Wry>>,
    doc_menus: Vec<Submenu<Wry>>,
}

fn is_markdown_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "md" | "markdown" | "mdown" | "mkd" | "txt"
            )
        })
        .unwrap_or(false)
}

struct AppState(Mutex<DocState>);

#[derive(Serialize, Clone)]
struct FileInfo {
    path: String,
    name: String,
    content: String,
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_string())
}

/// Read a file for the frontend. The path is canonicalized so the frontend
/// can recognise "this file is already open in a tab" by string comparison —
/// without it, the same file reached by two routes would open twice, and the
/// two tabs would race to write it.
///
/// Every read is also a sync point: this is the moment the editor's copy and
/// the file agree, so it is the moment to stamp for change detection.
fn read_file(app: &AppHandle, path: PathBuf) -> Result<FileInfo, String> {
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let path = path.canonicalize().unwrap_or(path);
    watch::record(app, &path, &content);
    Ok(FileInfo {
        name: file_name_of(&path),
        path: path.display().to_string(),
        content,
    })
}

/// Files named on the command line (or handed over by macOS before the
/// frontend was up), loaded once at startup. Unreadable paths are skipped,
/// as they were when this returned a single optional document.
#[tauri::command]
fn init_doc(app: AppHandle, state: State<AppState>) -> Vec<FileInfo> {
    let pending = {
        let mut s = state.0.lock().unwrap();
        s.ready = true;
        std::mem::take(&mut s.pending_open)
    };
    let loaded: Vec<FileInfo> = std::env::args()
        .skip(1)
        // macOS passes -psn_… on some launches.
        .filter(|a| !a.starts_with('-'))
        .map(PathBuf::from)
        .chain(pending)
        .filter_map(|p| read_file(&app, p).ok())
        .collect();
    let paths: Vec<PathBuf> = loaded.iter().map(|f| PathBuf::from(&f.path)).collect();
    push_recent(&app, &paths);
    loaded
}

/// Open a concrete path (drag & drop, OS file association, recent files).
#[tauri::command]
fn open_path(app: AppHandle, path: String) -> Result<Option<FileInfo>, String> {
    match read_file(&app, PathBuf::from(&path)) {
        Ok(info) => {
            push_recent(&app, &[PathBuf::from(&info.path)]);
            Ok(Some(info))
        }
        Err(e) => {
            // Covers permission changes that a bare `exists()` cannot see.
            forget_recent(&app, &path);
            Err(e)
        }
    }
}

/// Pick an HTML file and return its raw contents; the frontend converts it
/// to markdown (the webview's parser does the heavy lifting). Doesn't touch
/// document state — the import becomes a new unsaved document.
#[tauri::command]
async fn import_html(app: AppHandle) -> Result<Option<String>, String> {
    let picked = app
        .dialog()
        .file()
        .add_filter("HTML", &["html", "htm", "xhtml"])
        .add_filter("All files", &["*"])
        .blocking_pick_file();
    let Some(fp) = picked else { return Ok(None) };
    let path = fp.into_path().map_err(|e| e.to_string())?;
    std::fs::read_to_string(&path).map(Some).map_err(|e| e.to_string())
}

/// Pick a .docx and return it converted to markdown. Like import_html,
/// the result becomes a new unsaved document.
#[tauri::command]
async fn import_docx(app: AppHandle) -> Result<Option<String>, String> {
    let picked = app
        .dialog()
        .file()
        .add_filter("Word document", &["docx"])
        .blocking_pick_file();
    let Some(fp) = picked else { return Ok(None) };
    let path = fp.into_path().map_err(|e| e.to_string())?;
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    docx::docx_to_markdown(&bytes).map(Some)
}

/// The dialog is multi-select: a selection of five files becomes five tabs.
/// An empty Vec means the user cancelled.
#[tauri::command]
async fn open_doc(app: AppHandle) -> Result<Vec<FileInfo>, String> {
    let picked = app
        .dialog()
        .file()
        .add_filter("Markdown", &["md", "markdown", "mdown", "mkd", "txt"])
        .add_filter("All files", &["*"])
        .blocking_pick_files();
    let Some(files) = picked else {
        return Ok(Vec::new());
    };
    let loaded: Vec<FileInfo> = files
        .into_iter()
        .map(|fp| {
            fp.into_path()
                .map_err(|e| e.to_string())
                .and_then(|p| read_file(&app, p))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let paths: Vec<PathBuf> = loaded.iter().map(|f| PathBuf::from(&f.path)).collect();
    push_recent(&app, &paths);
    Ok(loaded)
}

fn write_doc(app: &AppHandle, path: &Path, content: &str) -> Result<FileInfo, String> {
    std::fs::write(path, content).map_err(|e| e.to_string())?;
    // Canonicalize *after* the write: a Save As target need not have existed.
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    // Before `push_recent`, which touches the menu and can take a while: until
    // this lands, the watcher would read our own write as an external change.
    watch::record(app, &path, content);
    // Covers Save As creating a new path, and move-to-front on a plain Save.
    push_recent(app, std::slice::from_ref(&path));
    Ok(FileInfo {
        name: file_name_of(&path),
        path: path.display().to_string(),
        content: String::new(),
    })
}

/// The outcome of a save. `file` is None when the user cancelled the dialog.
#[derive(Serialize)]
struct SaveResult {
    file: Option<FileInfo>,
    /// The file changed on disk since the editor last read or wrote it, and
    /// **nothing was written**. The frontend prompts instead.
    conflict: bool,
}

impl SaveResult {
    fn saved(file: Option<FileInfo>) -> Self {
        Self {
            file,
            conflict: false,
        }
    }
}

/// Save to `path`, or fall through to Save As when the tab has never been
/// saved.
///
/// This is the quiet path — no dialog, no confirmation — so it is the one that
/// can silently destroy someone else's work. It refuses to write over a file
/// that changed underneath us; the frontend then offers reload / merge / keep,
/// and whichever the user picks re-stamps the file so the next save goes
/// through. Save As is deliberately not guarded: its dialog already asks
/// before replacing an existing file.
#[tauri::command]
async fn save_doc(
    app: AppHandle,
    content: String,
    path: Option<String>,
) -> Result<SaveResult, String> {
    match path.as_deref().filter(|p| !p.is_empty()) {
        Some(p) if watch::changed(&app, Path::new(p)) => Ok(SaveResult {
            file: None,
            conflict: true,
        }),
        Some(p) => write_doc(&app, Path::new(p), &content).map(|f| SaveResult::saved(Some(f))),
        None => save_doc_as(app, content, None).await,
    }
}

/// Re-read a file that changed on disk, discarding whatever the tab holds.
#[tauri::command]
fn reload_doc(app: AppHandle, path: String) -> Result<FileInfo, String> {
    read_file(&app, PathBuf::from(path))
}

/// Accept the file's current contents as the new baseline without touching the
/// editor: "keep mine". The tab stays as it is, but its unsaved state is now
/// measured against what is on disk *now*, so the next save is not blocked by
/// a change the user has already seen and decided about.
#[tauri::command]
fn ack_disk_change(app: AppHandle, path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    watch::record(&app, &path, &content);
    Ok(())
}

#[derive(Serialize)]
struct MergeResult {
    /// The merged document, with conflict markers if it came to that.
    text: String,
    /// The file's contents, which become the tab's new merge baseline.
    disk: String,
    conflicted: bool,
}

/// Three-way merge the editor's text with the file's, against `base` — the
/// content both last agreed on. Merging accepts the file's current state as
/// the new baseline, so this re-stamps it exactly as a reload would.
#[tauri::command]
fn merge_doc(app: AppHandle, path: String, base: String, mine: String) -> Result<MergeResult, String> {
    let path = PathBuf::from(path);
    let disk = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let (text, conflicted) = watch::merge(&base, &mine, &disk);
    watch::record(&app, &path, &disk);
    Ok(MergeResult {
        text,
        disk,
        conflicted,
    })
}

/// The set of files to watch — every tab that has one, replacing the previous
/// set. Pushed by the frontend, which owns the tab list.
#[tauri::command]
fn watch_paths(app: AppHandle, paths: Vec<String>) {
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    watch::set_paths(&app, &paths);
}

/// `path` is the tab's current path, used only for the dialog's default
/// filename and starting directory. Shares `SaveResult` with `save_doc` so the
/// frontend reads one shape whichever it called; `conflict` is never set here,
/// because the dialog has already asked about replacing the chosen file.
#[tauri::command]
async fn save_doc_as(
    app: AppHandle,
    content: String,
    path: Option<String>,
) -> Result<SaveResult, String> {
    let current = path.as_deref().filter(|p| !p.is_empty()).map(Path::new);
    let name = current
        .map(file_name_of)
        .unwrap_or_else(|| "Untitled.md".to_string());
    let mut dialog = app
        .dialog()
        .file()
        .add_filter("Markdown", &["md", "markdown"])
        .set_file_name(&name);
    if let Some(dir) = current.and_then(Path::parent) {
        dialog = dialog.set_directory(dir);
    }
    let Some(fp) = dialog.blocking_save_file() else {
        return Ok(SaveResult::saved(None));
    };
    let path = fp.into_path().map_err(|e| e.to_string())?;
    write_doc(&app, &path, &content).map(|f| SaveResult::saved(Some(f)))
}

/// Write the frontend's standalone HTML rendering of the document wherever the
/// user points. `path` is the tab's markdown path, used only to seed the
/// dialog: an export never touches the markdown file and never re-targets the
/// tab, so — unlike `write_doc` — this deliberately stays out of the recent
/// files list, which is a list of documents to *open*.
#[tauri::command]
async fn export_html(
    app: AppHandle,
    content: String,
    path: Option<String>,
) -> Result<Option<String>, String> {
    let current = path.as_deref().filter(|p| !p.is_empty()).map(Path::new);
    let name = current
        .and_then(Path::file_stem)
        .map(|s| format!("{}.html", s.to_string_lossy()))
        .unwrap_or_else(|| "Untitled.html".to_string());
    let mut dialog = app
        .dialog()
        .file()
        .add_filter("HTML", &["html", "htm"])
        .set_file_name(&name);
    if let Some(dir) = current.and_then(Path::parent) {
        dialog = dialog.set_directory(dir);
    }
    let Some(fp) = dialog.blocking_save_file() else {
        return Ok(None);
    };
    let path = fp.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, content).map_err(|e| e.to_string())?;
    Ok(Some(path.display().to_string()))
}

/// The frontend owns the tab list, so it drives the title: `name`/`dirty`
/// describe the active tab, `any_dirty` covers every tab and gates the
/// close-confirmation prompt. An empty `name` means no document is open.
#[tauri::command]
fn set_title(app: AppHandle, state: State<AppState>, name: String, dirty: bool, any_dirty: bool) {
    state.0.lock().unwrap().any_dirty = any_dirty;
    if let Some(w) = app.get_webview_window("main") {
        let title = if name.is_empty() {
            "mdedit".to_string()
        } else {
            format!("{}{name} - mdedit", if dirty { "*" } else { "" })
        };
        let _ = w.set_title(&title);
    }
    set_doc_menus_enabled(&app, !name.is_empty());
}

/// Close for real, bypassing the unsaved-changes check.
#[tauri::command]
fn force_close(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.destroy();
    }
}

#[tauri::command]
fn show_about(app: AppHandle) {
    app.dialog()
        .message(format!(
            "mdedit {}\n\nA WYSIWYG Markdown editor with GitHub-flavored\nMarkdown support, built with Tauri and Leptos.",
            env!("CARGO_PKG_VERSION")
        ))
        .title("About mdedit")
        .show(|_| {});
}

// ---------- recent files ----------

/// Wrapped in a struct rather than a bare array so later settings can join it
/// without a format break.
#[derive(Serialize, Deserialize, Default)]
struct Prefs {
    #[serde(default)]
    recent: Vec<String>,
}

fn prefs_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("recent.json"))
}

/// Never fails loudly: a missing, unreadable or corrupt file just means
/// "no recent documents".
fn load_recent(app: &AppHandle) -> Vec<String> {
    let Some(p) = prefs_path(app) else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(p) else {
        return Vec::new();
    };
    serde_json::from_str::<Prefs>(&text)
        .map(|p| p.recent)
        .unwrap_or_default()
}

/// Best effort — losing the recent list is never worth an error dialog, and a
/// read-only or missing config dir must not break anything.
fn save_recent(app: &AppHandle) {
    let recent = {
        let state = app.state::<AppState>();
        let s = state.0.lock().unwrap();
        s.recent.clone()
    };
    let Some(p) = prefs_path(app) else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(&Prefs { recent }) {
        let _ = std::fs::write(p, text);
    }
}

/// Keep the tail: the last path components identify a file far better than
/// the root does.
fn elide_start(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    format!("…{}", chars[chars.len() - max + 1..].iter().collect::<String>())
}

/// A native menu offers exactly one text run per item — there is no dimmed
/// parent directory to be had — so the directory rides along with the
/// basename. Five `README.md` entries have to stay distinguishable.
fn recent_label(path: &str) -> String {
    let p = Path::new(path);
    let name = file_name_of(p);
    let mut dir = p
        .parent()
        .map(|d| d.display().to_string())
        .unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = home.to_string_lossy().into_owned();
        if !home.is_empty() && dir.starts_with(&home) {
            dir = format!("~{}", &dir[home.len()..]);
        }
    }
    let dir = elide_start(&dir, 40);
    let label = if dir.is_empty() {
        name
    } else {
        format!("{name}   —   {dir}")
    };
    // muda uses `&` as the mnemonic marker on Windows and Linux; `&&` renders
    // a literal one. Without this, "Q&A.md" shows up as "QA.md".
    label.replace('&', "&&")
}

/// (Re)populate Open Recent.
///
/// Must NOT be called with the `AppState` lock held: these calls block on the
/// main thread, which may itself be waiting for that lock inside
/// `on_menu_event`.
fn fill_recent_menu(app: &AppHandle, menu: &Submenu<Wry>, recent: &[String]) {
    if let Ok(items) = menu.items() {
        // Back to front: every removal shifts the indices after it.
        for i in (0..items.len()).rev() {
            let _ = menu.remove_at(i);
        }
    }
    if recent.is_empty() {
        // An empty submenu is a dead arrow on GTK; a disabled placeholder
        // reads as intentional.
        if let Ok(none) =
            MenuItem::with_id(app, "recent_none", "No Recent Files", false, None::<&str>)
        {
            let _ = menu.append(&none);
        }
        return;
    }
    for (i, path) in recent.iter().enumerate() {
        if let Ok(item) = MenuItem::with_id(
            app,
            format!("recent:{i}"),
            recent_label(path),
            true,
            None::<&str>,
        ) {
            let _ = menu.append(&item);
        }
    }
    if let (Ok(sep), Ok(clear)) = (
        PredefinedMenuItem::separator(app),
        MenuItem::with_id(app, "recent_clear", "Clear Recent", true, None::<&str>),
    ) {
        let _ = menu.append(&sep);
        let _ = menu.append(&clear);
    }
}

fn refresh_recent_menu(app: &AppHandle) {
    let (menu, recent) = {
        let state = app.state::<AppState>();
        let s = state.0.lock().unwrap();
        (s.recent_menu.clone(), s.recent.clone())
    };
    if let Some(menu) = menu {
        fill_recent_menu(app, &menu, &recent);
    }
}

/// Move-to-front, dedup by canonical path, cap at `RECENT_MAX`. Batched:
/// opening ten files at startup would otherwise mean ten menu rebuilds and
/// ten config writes.
fn push_recent(app: &AppHandle, paths: &[PathBuf]) {
    if paths.is_empty() {
        return;
    }
    {
        let state = app.state::<AppState>();
        let mut s = state.0.lock().unwrap();
        // In order, so the *last* path of a batch ends up most-recent — which
        // is also the tab the frontend leaves focused after opening them.
        for path in paths.iter() {
            let key = path
                .canonicalize()
                .unwrap_or_else(|_| path.clone())
                .display()
                .to_string();
            s.recent.retain(|p| p != &key);
            s.recent.insert(0, key);
        }
        s.recent.truncate(RECENT_MAX);
    }
    save_recent(app);
    refresh_recent_menu(app);
}

fn forget_recent(app: &AppHandle, path: &str) {
    {
        let state = app.state::<AppState>();
        let mut s = state.0.lock().unwrap();
        s.recent.retain(|p| p != path);
    }
    save_recent(app);
    refresh_recent_menu(app);
}

/// Open a recent entry by index. Existence is checked here rather than at
/// load time: stat-ing ten paths at startup stalls exactly the setups where
/// it hurts (NFS, sshfs, an unplugged drive) and permanently forgets files
/// that are only temporarily unreachable.
fn open_recent(app: &AppHandle, idx: usize) {
    let path = {
        let state = app.state::<AppState>();
        let s = state.0.lock().unwrap();
        s.recent.get(idx).cloned()
    };
    let Some(path) = path else { return };
    if !Path::new(&path).exists() {
        forget_recent(app, &path);
        // Non-blocking: this runs on the main thread, so a blocking dialog
        // would freeze the event loop.
        app.dialog()
            .message(format!("{path}\n\nThe file no longer exists."))
            .title("Could not open file")
            .show(|_| {});
        return;
    }
    let _ = app.emit("drop-open", vec![path]);
}

/// Save / Save As / Close Tab, and the Insert and Format menus, mean nothing
/// with no document open. A disabled item does not fire its accelerator, so
/// this makes Ctrl+S inert structurally rather than by convention.
fn set_doc_menus_enabled(app: &AppHandle, enabled: bool) {
    let (items, menus) = {
        let state = app.state::<AppState>();
        let s = state.0.lock().unwrap();
        (s.doc_items.clone(), s.doc_menus.clone())
    };
    for it in &items {
        let _ = it.set_enabled(enabled);
    }
    for m in &menus {
        let _ = m.set_enabled(enabled);
    }
}

/// The built menu plus the handles that get mutated at runtime.
struct MenuHandles {
    menu: Menu<Wry>,
    recent: Submenu<Wry>,
    doc_items: Vec<MenuItem<Wry>>,
    doc_menus: Vec<Submenu<Wry>>,
}

fn build_menu(app: &AppHandle, recent: &[String]) -> tauri::Result<MenuHandles> {
    #[cfg(target_os = "macos")]
    let app_menu = Submenu::with_items(
        app,
        "mdedit",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("About mdedit"), None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;

    let recent_menu = Submenu::with_id_and_items(app, "open_recent", "Open Recent", true, &[])?;
    fill_recent_menu(app, &recent_menu, recent);

    let save = MenuItem::with_id(app, "save", "Save", true, Some("CmdOrCtrl+S"))?;
    let save_as = MenuItem::with_id(app, "save_as", "Save As…", true, Some("CmdOrCtrl+Shift+S"))?;
    let close_tab = MenuItem::with_id(app, "close_tab", "Close Tab", true, Some("CmdOrCtrl+W"))?;
    let export = MenuItem::with_id(
        app,
        "export_html",
        "Export as HTML…",
        true,
        Some("CmdOrCtrl+Shift+E"),
    )?;

    let file_menu = Submenu::with_id_and_items(
        app,
        "file",
        "File",
        true,
        &[
            &MenuItem::with_id(app, "new", "New", true, Some("CmdOrCtrl+N"))?,
            &MenuItem::with_id(app, "open", "Open…", true, Some("CmdOrCtrl+O"))?,
            &recent_menu,
            &MenuItem::with_id(app, "import_html", "Import HTML…", true, None::<&str>)?,
            &MenuItem::with_id(app, "import_docx", "Import Word Document…", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &save,
            &save_as,
            &export,
            &PredefinedMenuItem::separator(app)?,
            &close_tab,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "quit", "Quit", true, Some("CmdOrCtrl+Q"))?,
        ],
    )?;

    let edit_menu = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &MenuItem::with_id(app, "undo", "Undo", true, Some("CmdOrCtrl+Z"))?,
            &MenuItem::with_id(app, "redo", "Redo", true, Some("CmdOrCtrl+Shift+Z"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &MenuItem::with_id(app, "select_all", "Select All", true, Some("CmdOrCtrl+A"))?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "find", "Find…", true, Some("CmdOrCtrl+F"))?,
            &MenuItem::with_id(
                app,
                "replace",
                "Replace…",
                true,
                Some(if cfg!(target_os = "macos") {
                    "Cmd+Alt+F"
                } else {
                    "Ctrl+H"
                }),
            )?,
        ],
    )?;

    // Ctrl+Tab / Ctrl+Shift+Tab also cycle documents, but only via the
    // in-page keydown handler: as a *native* accelerator it is unreliable on
    // WebKitGTK, where focus traversal eats it first.
    let view_menu = Submenu::with_items(
        app,
        "View",
        true,
        &[
            &MenuItem::with_id(
                app,
                "toggle_mode",
                "Toggle Markdown Source",
                true,
                Some("CmdOrCtrl+Shift+M"),
            )?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(
                app,
                "next_tab",
                "Next Document",
                true,
                Some("CmdOrCtrl+PageDown"),
            )?,
            &MenuItem::with_id(
                app,
                "prev_tab",
                "Previous Document",
                true,
                Some("CmdOrCtrl+PageUp"),
            )?,
        ],
    )?;

    let insert_menu = Submenu::with_items(
        app,
        "Insert",
        true,
        &[
            &MenuItem::with_id(app, "fmt_link", "Link…", true, Some("CmdOrCtrl+K"))?,
            &MenuItem::with_id(app, "fmt_image", "Image…", true, None::<&str>)?,
            &MenuItem::with_id(app, "fmt_table", "Table", true, None::<&str>)?,
            &MenuItem::with_id(app, "fmt_mermaid", "Mermaid Diagram", true, None::<&str>)?,
            &MenuItem::with_id(app, "fmt_hr", "Horizontal Rule", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "tbl_row_above", "Row Above", true, None::<&str>)?,
            &MenuItem::with_id(app, "tbl_row_below", "Row Below", true, None::<&str>)?,
            &MenuItem::with_id(app, "tbl_col_left", "Column Left", true, None::<&str>)?,
            &MenuItem::with_id(app, "tbl_col_right", "Column Right", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "tbl_del_row", "Delete Row", true, None::<&str>)?,
            &MenuItem::with_id(app, "tbl_del_col", "Delete Column", true, None::<&str>)?,
            &MenuItem::with_id(app, "tbl_del_table", "Delete Table", true, None::<&str>)?,
        ],
    )?;

    let format_menu = Submenu::with_items(
        app,
        "Format",
        true,
        &[
            &MenuItem::with_id(app, "fmt_bold", "Bold", true, Some("CmdOrCtrl+B"))?,
            &MenuItem::with_id(app, "fmt_italic", "Italic", true, Some("CmdOrCtrl+I"))?,
            &MenuItem::with_id(app, "fmt_strike", "Strikethrough", true, Some("CmdOrCtrl+Shift+X"))?,
            &MenuItem::with_id(app, "fmt_code", "Inline Code", true, Some("CmdOrCtrl+E"))?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "fmt_h1", "Heading 1", true, Some("CmdOrCtrl+1"))?,
            &MenuItem::with_id(app, "fmt_h2", "Heading 2", true, Some("CmdOrCtrl+2"))?,
            &MenuItem::with_id(app, "fmt_h3", "Heading 3", true, Some("CmdOrCtrl+3"))?,
            &MenuItem::with_id(app, "fmt_p", "Paragraph", true, Some("CmdOrCtrl+0"))?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "fmt_ul", "Bullet List", true, None::<&str>)?,
            &MenuItem::with_id(app, "fmt_ol", "Numbered List", true, None::<&str>)?,
            &MenuItem::with_id(app, "fmt_task", "Task List", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "fmt_quote", "Blockquote", true, None::<&str>)?,
            &MenuItem::with_id(app, "fmt_codeblock", "Code Block", true, None::<&str>)?,
        ],
    )?;

    let help_menu = Submenu::with_items(
        app,
        "Help",
        true,
        &[&MenuItem::with_id(app, "about", "About mdedit", true, None::<&str>)?],
    )?;

    let menu = Menu::with_items(
        app,
        &[
            #[cfg(target_os = "macos")]
            &app_menu,
            &file_menu,
            &edit_menu,
            &view_menu,
            &insert_menu,
            &format_menu,
            &help_menu,
        ],
    )?;

    Ok(MenuHandles {
        menu,
        recent: recent_menu,
        doc_items: vec![save, save_as, export, close_tab],
        doc_menus: vec![insert_menu, format_menu],
    })
}

/// A second launch forwarded its argv to us. Open the files as tabs and bring
/// the window forward.
#[cfg(desktop)]
fn on_second_instance(app: &AppHandle, args: Vec<String>, cwd: String) {
    // argv[0] is the executable, and relative paths are relative to the
    // *second* process's cwd, not ours — joining against ours would resolve
    // to the wrong file or to nothing.
    let cwd = PathBuf::from(cwd);
    let paths: Vec<PathBuf> = args
        .iter()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .map(|a| {
            let p = PathBuf::from(a);
            if p.is_absolute() {
                p
            } else {
                cwd.join(p)
            }
        })
        .filter(|p| is_markdown_file(p))
        .collect();

    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        // Best effort on Linux: Wayland and most compositors enforce
        // focus-stealing prevention, so this may only raise an urgency hint.
        let _ = w.set_focus();
    }

    if !paths.is_empty() {
        // No push_recent here: the frontend answers "drop-open" with one
        // `open_path` per file, and that already records them.
        let strs: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
        let _ = app.emit("drop-open", strs);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // WebKitGTK's DMA-BUF renderer crashes on some Wayland/driver combos
    // (GDK "Error 71 (Protocol error)"). Disable it unless the user opted in.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    let mut builder = tauri::Builder::default();

    // Must be registered before every other plugin: it aborts the second
    // process before anything else has started up. On macOS the Finder route
    // never spawns a second process (Launch Services sends an Apple event,
    // handled by RunEvent::Opened below), so this only covers CLI launches
    // there.
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            on_second_instance(app, args, cwd);
        }));
    }

    builder
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState(Mutex::new(DocState::default())))
        .manage(watch::Watch::default())
        .invoke_handler(tauri::generate_handler![
            init_doc, open_doc, open_path, import_html, import_docx, export_html, save_doc,
            save_doc_as, set_title, force_close, show_about, watch_paths, reload_doc, merge_doc,
            ack_disk_change
        ])
        .setup(|app| {
            let handle = app.handle();
            // Loaded before the menu is built so Open Recent is born
            // populated and nothing has to be mutated during setup.
            let recent = load_recent(handle);
            let h = build_menu(handle, &recent)?;
            app.set_menu(h.menu)?;
            let state = app.state::<AppState>();
            let mut s = state.0.lock().unwrap();
            s.recent = recent;
            s.recent_menu = Some(h.recent);
            s.doc_items = h.doc_items;
            s.doc_menus = h.doc_menus;
            Ok(())
        })
        .on_menu_event(|app, event| {
            let id = event.id().0.as_str();
            match id {
                "quit" => {
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.close();
                    }
                }
                "about" => show_about(app.clone()),
                // Handled here rather than forwarded: the index→path table
                // lives in AppState, and "drop-open" already means exactly
                // "open these paths as tabs".
                s if s.starts_with("recent:") => {
                    if let Ok(i) = s["recent:".len()..].parse::<usize>() {
                        open_recent(app, i);
                    }
                }
                "recent_clear" => {
                    {
                        let state = app.state::<AppState>();
                        state.0.lock().unwrap().recent.clear();
                    }
                    save_recent(app);
                    refresh_recent_menu(app);
                }
                "recent_none" => {}
                _ => {
                    let _ = app.emit("menu", id.to_string());
                }
            }
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                let dirty = {
                    let state = window.app_handle().state::<AppState>();
                    let s = state.0.lock().unwrap();
                    s.any_dirty
                };
                if dirty {
                    api.prevent_close();
                    let _ = window.emit("close-requested", ());
                }
            }
            // The safety net under the filesystem watcher: inotify and its
            // equivalents miss changes on network and fuse mounts, and a
            // directory that couldn't be armed sends nothing at all. Coming
            // back to the window is exactly when someone has just finished
            // editing the file somewhere else.
            WindowEvent::Focused(true) => {
                let app = window.app_handle().clone();
                // Stat-ing every open file could block on a stalled mount, and
                // this runs on the UI thread.
                std::thread::spawn(move || watch::check_all(&app));
            }
            WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) => {
                // Every markdown file in the drop becomes a tab. The frontend
                // decides what to do with them.
                let md: Vec<String> = paths
                    .iter()
                    .filter(|p| is_markdown_file(p))
                    .map(|p| p.display().to_string())
                    .collect();
                if !md.is_empty() {
                    let _ = window.emit("drop-open", md);
                }
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, _event| {
            // Files opened via Finder / "Open With" arrive as Apple events,
            // not argv. If the frontend is already up it handles the emitted
            // event; the stashed path covers launch-time opens (drained by
            // init_doc).
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = &_event {
                let paths: Vec<PathBuf> = urls
                    .iter()
                    .filter_map(|u| u.to_file_path().ok())
                    .filter(|p| is_markdown_file(p))
                    .collect();
                if !paths.is_empty() {
                    // Either the frontend is up and handles the event, or it
                    // isn't and `init_doc` will drain the stash — never both,
                    // which is what the `ready` flag is for.
                    let state = _app.state::<AppState>();
                    let ready = {
                        let mut s = state.0.lock().unwrap();
                        if !s.ready {
                            s.pending_open.extend(paths.iter().cloned());
                        }
                        s.ready
                    };
                    if ready {
                        let strs: Vec<String> =
                            paths.iter().map(|p| p.display().to_string()).collect();
                        let _ = _app.emit("drop-open", strs);
                    }
                }
            }
        });
}
