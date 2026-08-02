use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_dialog::DialogExt;

#[derive(Default)]
struct DocState {
    path: Option<PathBuf>,
    dirty: bool,
    /// File handed to us by the OS (macOS open event) before the frontend
    /// was ready to receive events.
    pending_open: Option<PathBuf>,
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

fn update_title(app: &AppHandle) {
    let state = app.state::<AppState>();
    let (name, dirty) = {
        let s = state.0.lock().unwrap();
        (
            s.path
                .as_deref()
                .map(file_name_of)
                .unwrap_or_else(|| "Untitled".to_string()),
            s.dirty,
        )
    };
    if let Some(w) = app.get_webview_window("main") {
        let marker = if dirty { "*" } else { "" };
        let _ = w.set_title(&format!("{marker}{name} - mdedit"));
    }
}

fn load_into_state(app: &AppHandle, path: PathBuf) -> Result<FileInfo, String> {
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let path = path.canonicalize().unwrap_or(path);
    {
        let state = app.state::<AppState>();
        let mut s = state.0.lock().unwrap();
        s.path = Some(path.clone());
        s.dirty = false;
    }
    update_title(app);
    Ok(FileInfo {
        name: file_name_of(&path),
        path: path.display().to_string(),
        content,
    })
}

/// File passed on the command line (or via a macOS open event that arrived
/// before the frontend was up), loaded once at startup.
#[tauri::command]
fn init_doc(app: AppHandle, state: State<AppState>) -> Option<FileInfo> {
    let pending = { state.0.lock().unwrap().pending_open.take() };
    let path = std::env::args().nth(1).map(PathBuf::from).or(pending)?;
    load_into_state(&app, path).ok()
}

/// Open a concrete path (drag & drop, OS file association).
#[tauri::command]
fn open_path(app: AppHandle, path: String) -> Result<Option<FileInfo>, String> {
    load_into_state(&app, PathBuf::from(path)).map(Some)
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

#[tauri::command]
fn new_doc(app: AppHandle, state: State<AppState>) {
    {
        let mut s = state.0.lock().unwrap();
        s.path = None;
        s.dirty = false;
    }
    update_title(&app);
}

#[tauri::command]
async fn open_doc(app: AppHandle) -> Result<Option<FileInfo>, String> {
    let picked = app
        .dialog()
        .file()
        .add_filter("Markdown", &["md", "markdown", "mdown", "mkd", "txt"])
        .add_filter("All files", &["*"])
        .blocking_pick_file();
    let Some(fp) = picked else { return Ok(None) };
    let path = fp.into_path().map_err(|e| e.to_string())?;
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let state = app.state::<AppState>();
    {
        let mut s = state.0.lock().unwrap();
        s.path = Some(path.clone());
        s.dirty = false;
    }
    update_title(&app);
    Ok(Some(FileInfo {
        name: file_name_of(&path),
        path: path.display().to_string(),
        content,
    }))
}

fn write_doc(app: &AppHandle, path: &Path, content: &str) -> Result<FileInfo, String> {
    std::fs::write(path, content).map_err(|e| e.to_string())?;
    let state = app.state::<AppState>();
    {
        let mut s = state.0.lock().unwrap();
        s.path = Some(path.to_path_buf());
        s.dirty = false;
    }
    update_title(app);
    Ok(FileInfo {
        name: file_name_of(path),
        path: path.display().to_string(),
        content: String::new(),
    })
}

/// Save to the current path, or fall through to Save As when the document
/// has never been saved. Returns None when the user cancels the dialog.
#[tauri::command]
async fn save_doc(app: AppHandle, content: String) -> Result<Option<FileInfo>, String> {
    let existing = {
        let state = app.state::<AppState>();
        let s = state.0.lock().unwrap();
        s.path.clone()
    };
    match existing {
        Some(path) => write_doc(&app, &path, &content).map(Some),
        None => save_doc_as(app, content).await,
    }
}

#[tauri::command]
async fn save_doc_as(app: AppHandle, content: String) -> Result<Option<FileInfo>, String> {
    let current_name = {
        let state = app.state::<AppState>();
        let s = state.0.lock().unwrap();
        s.path
            .as_deref()
            .map(file_name_of)
            .unwrap_or_else(|| "Untitled.md".to_string())
    };
    let picked = app
        .dialog()
        .file()
        .add_filter("Markdown", &["md", "markdown"])
        .set_file_name(&current_name)
        .blocking_save_file();
    let Some(fp) = picked else { return Ok(None) };
    let path = fp.into_path().map_err(|e| e.to_string())?;
    write_doc(&app, &path, &content).map(Some)
}

#[tauri::command]
fn set_dirty(app: AppHandle, state: State<AppState>, dirty: bool) {
    let changed = {
        let mut s = state.0.lock().unwrap();
        let changed = s.dirty != dirty;
        s.dirty = dirty;
        changed
    };
    if changed {
        update_title(&app);
    }
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

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
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

    let file_menu = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &MenuItem::with_id(app, "new", "New", true, Some("CmdOrCtrl+N"))?,
            &MenuItem::with_id(app, "open", "Open…", true, Some("CmdOrCtrl+O"))?,
            &MenuItem::with_id(app, "import_html", "Import HTML…", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "save", "Save", true, Some("CmdOrCtrl+S"))?,
            &MenuItem::with_id(app, "save_as", "Save As…", true, Some("CmdOrCtrl+Shift+S"))?,
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

    let view_menu = Submenu::with_items(
        app,
        "View",
        true,
        &[&MenuItem::with_id(
            app,
            "toggle_mode",
            "Toggle Markdown Source",
            true,
            Some("CmdOrCtrl+Shift+M"),
        )?],
    )?;

    let insert_menu = Submenu::with_items(
        app,
        "Insert",
        true,
        &[
            &MenuItem::with_id(app, "fmt_link", "Link…", true, Some("CmdOrCtrl+K"))?,
            &MenuItem::with_id(app, "fmt_image", "Image…", true, None::<&str>)?,
            &MenuItem::with_id(app, "fmt_table", "Table", true, None::<&str>)?,
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

    Menu::with_items(
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
    )
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // WebKitGTK's DMA-BUF renderer crashes on some Wayland/driver combos
    // (GDK "Error 71 (Protocol error)"). Disable it unless the user opted in.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState(Mutex::new(DocState::default())))
        .invoke_handler(tauri::generate_handler![
            init_doc, new_doc, open_doc, open_path, import_html, save_doc, save_doc_as,
            set_dirty, force_close, show_about
        ])
        .setup(|app| {
            let menu = build_menu(app.handle())?;
            app.set_menu(menu)?;
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
                    s.dirty
                };
                if dirty {
                    api.prevent_close();
                    let _ = window.emit("close-requested", ());
                }
            }
            WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) => {
                // The frontend decides what to do with it (it may need to
                // prompt about unsaved changes first).
                if let Some(p) = paths.iter().find(|p| is_markdown_file(p)) {
                    let _ = window.emit("drop-open", p.display().to_string());
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
                if let Some(path) = urls
                    .iter()
                    .filter_map(|u| u.to_file_path().ok())
                    .find(|p| is_markdown_file(p))
                {
                    let _ = _app.emit("drop-open", path.display().to_string());
                    let state = _app.state::<AppState>();
                    state.0.lock().unwrap().pending_open = Some(path);
                }
            }
        });
}
