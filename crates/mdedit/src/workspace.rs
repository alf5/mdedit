//! The window: title bar with tabs, the file sidebar, side-by-side panes of
//! tabs, and the status bar. It owns every open document and the flows
//! around them: open, save, close, quit.
//!
//! Tabs are drawn here rather than by gpui-kit's Dock, which can't ask
//! before closing a modified tab, can't close its last tab, and has no tab
//! context menu. With one pane its tabs sit in the title bar; split, each
//! pane gets its own strip.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme, Icon, IconName, Sizable as _, TitleBar, WindowExt as _, h_resizable,
    resizable_panel,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mdedit_core::prefs::{Mode, recent_parts};
use mdedit_core::{os, project, themes};

use crate::actions::*;
use crate::doc::{DiskChoice, DocEvent, DocView};
use crate::files::{FilesEvent, FilesPanel};
use crate::state::AppState;
use crate::theme;
use mdedit_core::watch::{self, Watch};

const SIDEBAR_WIDTH: f32 = 248.;
const MAX_PANES: usize = 4;

#[derive(Default)]
struct Pane {
    tabs: Vec<Entity<DocView>>,
    active: usize,
}

impl Pane {
    fn active_doc(&self) -> Option<&Entity<DocView>> {
        self.tabs.get(self.active)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum SidebarTab {
    Files,
    Outline,
}

/// A question about unsaved changes that is waiting for its answer.
enum Pending {
    Close(Entity<DocView>),
    Quit(Entity<DocView>),
}

#[derive(Clone, Copy)]
enum Answer {
    Save,
    Discard,
    Cancel,
}

/// What to do once a save has landed.
#[derive(Clone, Copy, PartialEq)]
enum Then {
    Nothing,
    Close,
    ContinueQuit,
}

/// A tab being dragged.
#[derive(Clone)]
struct DraggedTab {
    doc: Entity<DocView>,
    name: SharedString,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px(px(12.))
            .py(px(5.))
            .rounded(px(6.))
            .bg(theme.popover)
            .border_1()
            .border_color(theme.border)
            .shadow_md()
            .text_sm()
            .child(self.name.clone())
    }
}

pub struct Workspace {
    focus: FocusHandle,
    panes: Vec<Pane>,
    active: usize,
    files: Entity<FilesPanel>,
    sidebar_open: bool,
    sidebar_tab: SidebarTab,
    pending: Option<Pending>,
    /// Modified tabs the user chose not to save during this quit.
    quit_discarded: HashSet<EntityId>,
    /// Notices files changed on disk underneath their tabs.
    watch: Watch,
    _subs: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
}

impl Workspace {
    pub fn new(
        paths: Vec<PathBuf>,
        incoming: async_channel::Receiver<Vec<PathBuf>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let files = cx.new(FilesPanel::new);
        let mut subs =
            vec![
                cx.subscribe_in(&files, window, |this, _, ev: &FilesEvent, window, cx| {
                    let FilesEvent::Open(path) = ev;
                    this.open_paths(vec![path.clone()], window, cx);
                }),
            ];
        subs.push(cx.observe_window_appearance(window, |this, window, cx| {
            let id = AppState::prefs(cx).theme().to_string();
            if id.is_empty() {
                theme::apply("", window, cx);
                this.restyle(cx);
            }
        }));

        // Files handed over by later launches (see `single`).
        let ipc = cx.spawn_in(window, async move |this, cx| {
            while let Ok(paths) = incoming.recv().await {
                let alive = this.update_in(cx, |this, window, cx| {
                    window.activate_window();
                    this.open_paths(paths, window, cx);
                });
                if alive.is_err() {
                    break;
                }
            }
        });

        // Files changed underneath their tabs, reported from the watcher's
        // thread.
        let (changed_tx, changed_rx) = async_channel::unbounded::<PathBuf>();
        let watch = Watch::new(move |p| {
            let _ = changed_tx.send_blocking(p);
        });
        let changes = cx.spawn_in(window, async move |this, cx| {
            while let Ok(path) = changed_rx.recv().await {
                if this
                    .update_in(cx, |this, _, cx| this.disk_changed(&path, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        // Back in the window: look again, for filesystems that don't report
        // changes (network mounts).
        subs.push(cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                let watch = this.watch.clone();
                cx.background_spawn(async move { watch.check_all() })
                    .detach();
            }
        }));

        let ws = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            ws.update(cx, |ws, cx| ws.request_quit(window, cx))
                .unwrap_or(true)
        });

        let sidebar_open = AppState::prefs(cx).sidebar_open;
        let mut this = Workspace {
            focus: cx.focus_handle(),
            panes: vec![Pane::default()],
            active: 0,
            files,
            sidebar_open,
            // With no folder open, Files has nothing to show; the outline
            // does.
            sidebar_tab: SidebarTab::Outline,
            pending: None,
            quit_discarded: HashSet::new(),
            watch,
            _subs: subs,
            _tasks: vec![ipc, changes],
        };
        window.focus(&this.focus, cx);
        if !paths.is_empty() {
            this.open_paths(paths, window, cx);
        }
        this
    }

    // ---------- documents ----------

    fn docs(&self) -> impl Iterator<Item = &Entity<DocView>> {
        self.panes.iter().flat_map(|p| p.tabs.iter())
    }

    fn active_doc(&self) -> Option<&Entity<DocView>> {
        self.panes.get(self.active)?.active_doc()
    }

    fn locate(&self, doc: &Entity<DocView>) -> Option<(usize, usize)> {
        self.panes.iter().enumerate().find_map(|(p, pane)| {
            pane.tabs
                .iter()
                .position(|d| d.entity_id() == doc.entity_id())
                .map(|t| (p, t))
        })
    }

    fn find_path(&self, path: &Path, cx: &App) -> Option<Entity<DocView>> {
        self.docs()
            .find(|d| d.read(cx).path.as_deref() == Some(path))
            .cloned()
    }

    fn activate_doc(&mut self, doc: &Entity<DocView>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((p, t)) = self.locate(doc) {
            self.active = p;
            self.panes[p].active = t;
            doc.focus_handle(cx).focus(window, cx);
            self.sync_files(cx);
            cx.notify();
        }
    }

    fn add_doc(&mut self, doc: Entity<DocView>, window: &mut Window, cx: &mut Context<Self>) {
        self._subs.push(
            cx.subscribe_in(&doc, window, |this, doc, ev: &DocEvent, window, cx| {
                if let DocEvent::Disk(choice) = ev {
                    this.resolve_disk(doc.clone(), *choice, window, cx);
                }
                cx.notify();
            }),
        );
        let pane = &mut self.panes[self.active];
        let at = (pane.active + 1).min(pane.tabs.len());
        pane.tabs.insert(at, doc.clone());
        self.activate_doc(&doc, window, cx);
    }

    fn next_untitled(&self, cx: &App) -> usize {
        let taken: HashSet<usize> = self.docs().map(|d| d.read(cx).untitled_number()).collect();
        (1..).find(|n| !taken.contains(n)).unwrap_or(1)
    }

    fn sync_files(&self, cx: &mut Context<Self>) {
        let path = self.active_doc().and_then(|d| d.read(cx).path.clone());
        self.files.update(cx, |f, cx| f.set_active(path, cx));
    }

    pub fn open_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let mut to_read = Vec::new();
        for path in paths {
            let path = project::canonical(&path);
            if path.is_dir() {
                self.set_folder(Some(path), cx);
            } else if let Some(doc) = self.find_path(&path, cx) {
                self.activate_doc(&doc, window, cx);
            } else {
                to_read.push(path);
            }
        }
        if to_read.is_empty() {
            return;
        }
        let read = cx.background_spawn(async move {
            to_read
                .into_iter()
                .map(|p| (p.clone(), project::read_any(&p)))
                .collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let results = read.await;
            this.update_in(cx, |this, window, cx| {
                let mut opened = Vec::new();
                for (path, result) in results {
                    match result {
                        Ok(file) => {
                            // Read twice in one batch, or opened meanwhile.
                            if let Some(doc) = this.find_path(&file.path, cx) {
                                this.activate_doc(&doc, window, cx);
                                continue;
                            }
                            opened.push(file.path.clone());
                            this.stamp(&file);
                            let doc = cx.new(|cx| DocView::from_file(file, window, cx));
                            this.add_doc(doc, window, cx);
                        }
                        Err(e) => {
                            AppState::update_prefs(cx, |p| p.forget_recent(&path));
                            window.push_notification(
                                Notification::error(format!(
                                    "Couldn't open {}: {e}",
                                    path.display()
                                )),
                                cx,
                            );
                        }
                    }
                }
                AppState::update_prefs(cx, |p| p.push_recent(opened.iter().map(PathBuf::as_path)));
                this.sync_watch(cx);
            })
            .ok();
        })
        .detach();
    }

    fn set_folder(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        self.sidebar_tab = if root.is_some() {
            SidebarTab::Files
        } else {
            SidebarTab::Outline
        };
        self.files.update(cx, |f, cx| f.set_root(root, cx));
        self.sync_files(cx);
        if !self.sidebar_open {
            self.sidebar_open = true;
        }
        cx.notify();
    }

    // ---------- commands ----------

    fn new_file(&mut self, _: &NewFile, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.next_untitled(cx);
        let doc = cx.new(|cx| DocView::untitled(n, window, cx));
        self.add_doc(doc, window, cx);
    }

    fn open(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.open_paths(paths, window, cx))
                .ok();
        })
        .detach();
    }

    fn open_folder(&mut self, _: &OpenFolder, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(dir) = paths.pop() else { return };
            this.update(cx, |this, cx| this.set_folder(Some(dir), cx))
                .ok();
        })
        .detach();
    }

    fn command_palette(&mut self, _: &CommandPalette, window: &mut Window, cx: &mut Context<Self>) {
        let markdown = self
            .active_doc()
            .is_some_and(|d| d.read(cx).markdown().is_some());
        crate::palette::open_commands(markdown, window, cx);
    }

    fn quick_open(&mut self, _: &QuickOpen, window: &mut Window, cx: &mut Context<Self>) {
        let recent = AppState::prefs(cx).recent.clone();
        let root = self.files.read(cx).root().map(Path::to_path_buf);
        let files = root
            .as_deref()
            .map(|r| project::walk_files(r, 5000))
            .unwrap_or_default();
        let ws = cx.entity().downgrade();
        crate::palette::open_files(
            recent,
            root,
            files,
            move |path, window, cx| {
                ws.update(cx, |ws, cx| ws.open_paths(vec![path], window, cx))
                    .ok();
            },
            window,
            cx,
        );
    }

    /// Pick a file, convert it, and open the result as a new document.
    fn import(
        &mut self,
        convert: fn(&[u8]) -> Result<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let result = std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|b| convert(&b));
            this.update_in(cx, |this, window, cx| match result {
                Ok(md) => {
                    let n = this.next_untitled(cx);
                    let doc = cx.new(|cx| DocView::untitled_with(n, md, cx));
                    this.add_doc(doc, window, cx);
                }
                Err(e) => window.push_notification(
                    Notification::error(format!("Couldn't import {}: {e}", path.display())),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    fn import_html(&mut self, _: &ImportHtml, window: &mut Window, cx: &mut Context<Self>) {
        self.import(
            |b| {
                Ok(mdedit_core::html_import::html_to_markdown(
                    &String::from_utf8_lossy(b),
                ))
            },
            window,
            cx,
        );
    }

    fn import_word(&mut self, _: &ImportWord, window: &mut Window, cx: &mut Context<Self>) {
        self.import(mdedit_core::docx::docx_to_markdown, window, cx);
    }

    /// The active markdown document's title (its file name) and text.
    fn active_markdown(&self, cx: &App) -> Option<(String, String, Option<PathBuf>)> {
        let d = self.active_doc()?.read(cx);
        let text = d.markdown()?.read(cx).text().to_string();
        Some((d.name(), text, d.path.clone()))
    }

    /// Ask where to write an export of the active document, then write
    /// `make(title, markdown, base_dir)` there.
    fn export(
        &mut self,
        ext: &'static str,
        make: impl FnOnce(&str, &str, Option<&Path>, &App) -> Result<Vec<u8>, String> + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((name, md, path)) = self.active_markdown(cx) else {
            return;
        };
        let stem = Path::new(&name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
        let dir = path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(dirs::document_dir)
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let rx = cx.prompt_for_new_path(&dir, Some(&format!("{stem}.{ext}")));
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(out))) = rx.await else { return };
            cx.update(|window, cx| {
                let base = path.as_deref().and_then(Path::parent);
                let result = make(&name, &md, base, cx)
                    .and_then(|bytes| std::fs::write(&out, bytes).map_err(|e| e.to_string()));
                match result {
                    Ok(()) => window.push_notification(
                        Notification::success(format!("Exported {}", out.display())),
                        cx,
                    ),
                    Err(e) => window.push_notification(
                        Notification::error(format!("Couldn't export: {e}")),
                        cx,
                    ),
                }
            })
            .ok();
        })
        .detach();
    }

    fn export_html(&mut self, _: &ExportHtml, window: &mut Window, cx: &mut Context<Self>) {
        self.export(
            "html",
            |name, md, _, cx| {
                let p = AppState::prefs(cx);
                let mut class = p.style.css_class().to_string();
                if p.numbered {
                    class.push_str(" md-numbered");
                }
                if p.reading_width.is_none() {
                    class.push_str(" md-full-width");
                }
                Ok(
                    mdedit_core::export::export_html(name, md, class.trim(), p.theme())
                        .into_bytes(),
                )
            },
            window,
            cx,
        );
    }

    fn close_folder(&mut self, _: &CloseFolder, _: &mut Window, cx: &mut Context<Self>) {
        self.set_folder(None, cx);
    }

    fn open_recent(&mut self, action: &OpenRecent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = AppState::prefs(cx).recent.get(action.ix).cloned() {
            self.open_paths(vec![path], window, cx);
        }
    }

    fn clear_recent(&mut self, _: &ClearRecent, _: &mut Window, cx: &mut Context<Self>) {
        AppState::update_prefs(cx, |p| p.recent.clear());
        cx.notify();
    }

    fn save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(doc) = self.active_doc().cloned() {
            self.save_doc(doc, Then::Nothing, window, cx);
        }
    }

    fn save_as(&mut self, _: &SaveAs, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(doc) = self.active_doc().cloned() {
            self.save_doc_as(doc, Then::Nothing, window, cx);
        }
    }

    fn save_doc(
        &mut self,
        doc: Entity<DocView>,
        then: Then,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (path, can_save) = {
            let d = doc.read(cx);
            (d.path.clone(), d.can_save())
        };
        if !can_save {
            return;
        }
        match path {
            // A plain save never overwrites a file that changed underneath
            // it: it asks the changed-on-disk question instead.
            Some(path) if self.watch.changed(&path) => {
                doc.update(cx, |d, cx| {
                    d.stale = true;
                    cx.notify();
                });
                window.push_notification(
                    Notification::warning(
                        "The file changed on disk, so it wasn't overwritten. Choose what to keep.",
                    ),
                    cx,
                );
            }
            Some(path) => self.write(doc, &path, then, window, cx),
            None => self.save_doc_as(doc, then, window, cx),
        }
    }

    fn save_doc_as(
        &mut self,
        doc: Entity<DocView>,
        then: Then,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let d = doc.read(cx);
        if !d.can_save_as() {
            return;
        }
        let name = match &d.path {
            Some(_) => d.name(),
            None => format!("{}.md", d.name()),
        };
        let dir = d
            .path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| self.files.read(cx).root().map(Path::to_path_buf))
            .or_else(dirs::document_dir)
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            this.update_in(cx, |this, window, cx| {
                this.write(doc, &path, then, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn write(
        &mut self,
        doc: Entity<DocView>,
        path: &Path,
        then: Then,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match doc.update(cx, |d, cx| d.write_to(path, cx)) {
            Ok(()) => {
                if let Some(bytes) = doc.read(cx).disk_bytes(cx) {
                    self.watch.record_bytes(&project::canonical(path), &bytes);
                }
                self.sync_watch(cx);
                AppState::update_prefs(cx, |p| p.push_recent([path]));
                self.sync_files(cx);
                match then {
                    Then::Nothing => {}
                    Then::Close => self.remove_doc(&doc, window, cx),
                    Then::ContinueQuit => self.continue_quit(window, cx),
                }
            }
            Err(e) => window.push_notification(
                Notification::error(format!("Couldn't save {}: {e}", path.display())),
                cx,
            ),
        }
    }

    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(doc) = self.active_doc().cloned() {
            self.close_doc(doc, window, cx);
        }
    }

    fn close_other_tabs(
        &mut self,
        _: &CloseOtherTabs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(keep) = self.active_doc().cloned() else {
            return;
        };
        let others: Vec<_> = self.panes[self.active]
            .tabs
            .iter()
            .filter(|d| d.entity_id() != keep.entity_id() && !d.read(cx).is_dirty())
            .cloned()
            .collect();
        for doc in others {
            self.remove_doc(&doc, window, cx);
        }
        self.activate_doc(&keep, window, cx);
    }

    pub fn close_doc(&mut self, doc: Entity<DocView>, window: &mut Window, cx: &mut Context<Self>) {
        if doc.read(cx).is_dirty() {
            self.activate_doc(&doc, window, cx);
            self.ask_save(Pending::Close(doc), window, cx);
        } else {
            self.remove_doc(&doc, window, cx);
        }
    }

    fn remove_doc(&mut self, doc: &Entity<DocView>, window: &mut Window, cx: &mut Context<Self>) {
        let Some((p, t)) = self.locate(doc) else {
            return;
        };
        let pane = &mut self.panes[p];
        pane.tabs.remove(t);
        if pane.active >= t && pane.active > 0 {
            pane.active -= 1;
        }
        // An emptied side folds away; the last pane stays, empty.
        if pane.tabs.is_empty() && self.panes.len() > 1 {
            self.panes.remove(p);
            if self.active >= p && self.active > 0 {
                self.active -= 1;
            }
        }
        match self.active_doc().cloned() {
            Some(next) => self.activate_doc(&next, window, cx),
            None => {
                window.focus(&self.focus, cx);
                self.sync_files(cx);
            }
        }
        self.sync_watch(cx);
        cx.notify();
    }

    // ---------- changes on disk ----------

    /// Record what `file` held when read, so later reads can tell a real
    /// change from a touch.
    fn stamp(&self, file: &project::AnyFile) {
        use project::Content;
        match &file.content {
            Content::Markdown(t) | Content::Text(t) => self.watch.record(&file.path, t),
            Content::Image { bytes, .. } | Content::Binary(bytes) => {
                self.watch.record_bytes(&file.path, bytes)
            }
            Content::Oversized => {}
        }
    }

    /// Watch exactly the files that have tabs.
    fn sync_watch(&self, cx: &App) {
        let paths: Vec<PathBuf> = self
            .docs()
            .filter_map(|d| d.read(cx).path.clone())
            .collect();
        self.watch.set_paths(&paths);
    }

    fn disk_changed(&mut self, path: &Path, cx: &mut Context<Self>) {
        if let Some(doc) = self.find_path(path, cx) {
            doc.update(cx, |d, cx| {
                d.stale = true;
                cx.notify();
            });
            cx.notify();
        }
    }

    /// Act on the answer to the changed-on-disk banner.
    fn resolve_disk(
        &mut self,
        doc: Entity<DocView>,
        choice: DiskChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = doc.read(cx).path.clone() else {
            return;
        };
        let file = match project::read_any(&path) {
            Ok(f) => f,
            Err(e) => {
                window.push_notification(
                    Notification::error(format!("Couldn't read {}: {e}", path.display())),
                    cx,
                );
                return;
            }
        };
        self.stamp(&file);
        match choice {
            DiskChoice::Reload => doc.update(cx, |d, cx| d.reload(file, window, cx)),
            DiskChoice::Keep => doc.update(cx, |d, cx| {
                d.stale = false;
                d.mark_dirty(cx);
            }),
            DiskChoice::Merge => {
                let disk = match &file.content {
                    project::Content::Markdown(t) | project::Content::Text(t) => t.clone(),
                    _ => return doc.update(cx, |d, cx| d.reload(file, window, cx)),
                };
                let (base, ours) = {
                    let d = doc.read(cx);
                    (
                        d.saved_text().unwrap_or_default(),
                        d.current_text(cx).unwrap_or_default(),
                    )
                };
                let (merged, conflicted) = watch::merge(&base, &ours, &disk);
                doc.update(cx, |d, cx| d.set_texts(merged, disk, window, cx));
                if conflicted {
                    if let Some(editor) = doc.read(cx).markdown().cloned() {
                        editor.update(cx, |e, cx| e.set_mode(Mode::Source, cx));
                    }
                    window.push_notification(
                        Notification::warning(
                            "Both sides changed the same lines: look for the <<<<<<< markers.",
                        ),
                        cx,
                    );
                }
            }
        }
        cx.notify();
    }

    // ---------- unsaved changes ----------

    fn ask_save(&mut self, pending: Pending, window: &mut Window, cx: &mut Context<Self>) {
        let doc = match &pending {
            Pending::Close(d) | Pending::Quit(d) => d.clone(),
        };
        self.pending = Some(pending);
        let name = doc.read(cx).name();
        let ws = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let button = |id: &'static str, label: &'static str, answer: Answer| {
                let ws = ws.clone();
                Button::new(id).label(label).on_click(move |_, window, cx| {
                    // Closed first: answering may open the next question.
                    window.close_dialog(cx);
                    ws.update(cx, |ws, cx| ws.answer(answer, window, cx)).ok();
                })
            };
            let on_escape = ws.clone();
            dialog
                .title(format!("Save changes to “{name}”?"))
                .width(px(440.))
                .close_button(false)
                .child("Your changes will be lost if you don't save them.")
                .on_cancel(move |_, window, cx| {
                    on_escape
                        .update(cx, |ws, cx| ws.answer(Answer::Cancel, window, cx))
                        .ok();
                    true
                })
                .footer(
                    DialogFooter::new()
                        .child(button("dont-save", "Don't Save", Answer::Discard).ghost())
                        .child(div().flex_1())
                        .child(button("cancel", "Cancel", Answer::Cancel).outline())
                        .child(button("save", "Save", Answer::Save).primary()),
                )
        });
    }

    fn answer(&mut self, answer: Answer, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        match (pending, answer) {
            (Pending::Quit(_), Answer::Cancel) => self.quit_discarded.clear(),
            (_, Answer::Cancel) => {}
            (Pending::Close(doc), Answer::Discard) => self.remove_doc(&doc, window, cx),
            (Pending::Close(doc), Answer::Save) => self.save_doc(doc, Then::Close, window, cx),
            (Pending::Quit(doc), Answer::Discard) => {
                self.quit_discarded.insert(doc.entity_id());
                self.continue_quit(window, cx);
            }
            (Pending::Quit(doc), Answer::Save) => {
                self.save_doc(doc, Then::ContinueQuit, window, cx)
            }
        }
    }

    /// Whether the window may close right now. With modified tabs it can't:
    /// each is asked about in turn, and the app quits after the last.
    fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.pending.is_some() {
            return false;
        }
        self.quit_discarded.clear();
        let dirty = self.docs().any(|d| d.read(cx).is_dirty());
        if dirty {
            self.continue_quit(window, cx);
            false
        } else {
            cx.quit();
            true
        }
    }

    fn continue_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = self
            .docs()
            .find(|d| d.read(cx).is_dirty() && !self.quit_discarded.contains(&d.entity_id()))
            .cloned();
        match next {
            Some(doc) => {
                self.activate_doc(&doc, window, cx);
                self.ask_save(Pending::Quit(doc), window, cx);
            }
            None => cx.quit(),
        }
    }

    fn quit(&mut self, _: &Quit, window: &mut Window, cx: &mut Context<Self>) {
        self.request_quit(window, cx);
    }

    // ---------- panes ----------

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        let open = self.sidebar_open;
        AppState::update_prefs(cx, |p| p.sidebar_open = open);
        cx.notify();
    }

    /// Split the active tab off to the right; once split, move it to the
    /// neighbouring side instead.
    fn split_right(&mut self, _: &SplitRight, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.active_doc().cloned() else {
            return;
        };
        let pane = &self.panes[self.active];
        let target = if pane.tabs.len() > 1 && self.panes.len() < MAX_PANES {
            self.panes.insert(self.active + 1, Pane::default());
            self.active + 1
        } else if self.active + 1 < self.panes.len() {
            self.active + 1
        } else if self.active > 0 {
            self.active - 1
        } else {
            return;
        };
        self.move_doc(doc, target, None, window, cx);
    }

    fn join_panes(&mut self, _: &JoinPanes, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.active_doc().cloned();
        let tabs: Vec<_> = self.panes.drain(..).flat_map(|p| p.tabs).collect();
        self.panes = vec![Pane { tabs, active: 0 }];
        self.active = 0;
        if let Some(doc) = active {
            self.activate_doc(&doc, window, cx);
        }
        cx.notify();
    }

    /// Move `doc` into pane `to` at `at` (the end if None). The pane it left
    /// folds away if that emptied it.
    fn move_doc(
        &mut self,
        doc: Entity<DocView>,
        to: usize,
        at: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((from, ix)) = self.locate(&doc) else {
            return;
        };
        let mut to = to;
        let mut at = at;
        self.panes[from].tabs.remove(ix);
        if from == to
            && let Some(a) = at.as_mut()
            && *a > ix
        {
            *a -= 1;
        }
        let pane = &mut self.panes[from];
        pane.active = pane.active.min(pane.tabs.len().saturating_sub(1));
        if self.panes[from].tabs.is_empty() && self.panes.len() > 1 && from != to {
            self.panes.remove(from);
            if to > from {
                to -= 1;
            }
            at = at.map(|a| a.min(self.panes[to].tabs.len()));
        }
        let dest = &mut self.panes[to];
        let at = at.unwrap_or(dest.tabs.len()).min(dest.tabs.len());
        dest.tabs.insert(at, doc.clone());
        self.activate_doc(&doc, window, cx);
    }

    fn cycle(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let all: Vec<_> = self.docs().cloned().collect();
        let Some(cur) = self.active_doc() else { return };
        let Some(ix) = all.iter().position(|d| d.entity_id() == cur.entity_id()) else {
            return;
        };
        let next = (ix as isize + delta).rem_euclid(all.len() as isize) as usize;
        self.activate_doc(&all[next], window, cx);
    }

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle(1, window, cx);
    }

    fn prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle(-1, window, cx);
    }

    fn reveal(&mut self, _: &RevealInFileManager, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.active_doc().and_then(|d| d.read(cx).path.clone()) else {
            return;
        };
        if let Some(dir) = os::containing_folder(&path)
            && let Err(e) = os::open_in_file_manager(&dir)
        {
            window.push_notification(Notification::error(e), cx);
        }
    }

    fn set_theme(&mut self, action: &SetTheme, window: &mut Window, cx: &mut Context<Self>) {
        let id = action.id.to_string();
        AppState::update_prefs(cx, |p| p.theme = id.clone());
        theme::apply(&id, window, cx);
        self.restyle(cx);
        cx.notify();
    }

    /// Colours or typography changed: every open document re-sets itself.
    fn restyle(&mut self, cx: &mut Context<Self>) {
        let docs: Vec<_> = self.docs().cloned().collect();
        for doc in docs {
            doc.update(cx, |d, cx| d.restyle(cx));
        }
    }

    fn about(&mut self, _: &About, window: &mut Window, cx: &mut Context<Self>) {
        window.open_dialog(cx, |dialog, _, _| {
            dialog.title("mdedit").width(px(360.)).child(format!(
                "Version {}\nA Markdown editor, written in Rust.",
                env!("CARGO_PKG_VERSION")
            ))
        });
    }

    fn toggle_hex_overwrite(
        &mut self,
        _: &ToggleHexOverwrite,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(hex) = self.active_doc().and_then(|d| d.read(cx).hex().cloned()) {
            hex.update(cx, |h, cx| {
                let on = !h.overwrite;
                h.set_overwrite(on, cx)
            });
        }
    }

    // ---------- drawing ----------

    fn window_title(&self, cx: &App) -> String {
        match self.active_doc() {
            Some(doc) => {
                let d = doc.read(cx);
                let star = if d.is_dirty() { "*" } else { "" };
                format!("{star}{} - mdedit", d.name())
            }
            None => "mdedit".into(),
        }
    }

    fn render_tab(
        &self,
        pane: usize,
        ix: usize,
        doc: &Entity<DocView>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let d = doc.read(cx);
        let name: SharedString = d.name().into();
        let stale = d.stale;
        let dirty = d.is_dirty() || stale;
        let selected = self.panes[pane].active == ix;
        let focused_pane = pane == self.active;
        let group: SharedString = format!("tab-{}", doc.entity_id()).into();
        let warning = theme.warning;
        let (fg, muted, active_bg, hover_bg, accent, border) = (
            theme.tab_active_foreground,
            theme.tab_foreground,
            theme.tab_active,
            theme.accent,
            theme.primary,
            theme.border,
        );
        let doc_for_click = doc.clone();
        let doc_for_close = doc.clone();
        let doc_for_middle = doc.clone();
        let drag = DraggedTab {
            doc: doc.clone(),
            name: name.clone(),
        };
        div()
            .id(("tab", doc.entity_id()))
            .group(group.clone())
            .h(px(28.))
            .max_w(px(220.))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .pl(px(12.))
            .pr(px(6.))
            .rounded(px(7.))
            .text_sm()
            .cursor_pointer()
            .text_color(if selected { fg } else { muted })
            .when(selected, |d| {
                d.bg(active_bg)
                    .border_1()
                    .border_color(border)
                    .when(focused_pane && self.panes.len() > 1, |d| {
                        d.border_color(accent.opacity(0.6))
                    })
            })
            .when(!selected, |d| d.hover(|s| s.bg(hover_bg)))
            .child(
                div()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(name),
            )
            .child(
                div()
                    .size(px(18.))
                    .flex_none()
                    .relative()
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(!dirty, |d| d.invisible())
                            .group_hover(group.clone(), |s| s.invisible())
                            .child(div().size(px(7.)).rounded_full().bg(if stale {
                                warning
                            } else if selected {
                                fg
                            } else {
                                muted
                            })),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .when(!(selected && !dirty), |d| d.invisible())
                            .group_hover(group, |s| s.visible())
                            .child(
                                Button::new(("close-tab", doc.entity_id()))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Close)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.close_doc(doc_for_close.clone(), window, cx);
                                    })),
                            ),
                    ),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.activate_doc(&doc_for_click, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, window, cx| {
                    this.close_doc(doc_for_middle.clone(), window, cx);
                }),
            )
            .on_drag(drag, |tab, _, _, cx| cx.new(|_| tab.clone()))
            .drag_over::<DraggedTab>(move |s, _, _, _| s.border_l_2().border_color(accent))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                this.move_doc(dragged.doc.clone(), pane, Some(ix), window, cx);
            }))
            .context_menu({
                let ws = cx.entity().downgrade();
                let doc = doc.clone();
                move |menu, window, cx| {
                    // The menu acts on the tab it was opened on.
                    ws.update(cx, |ws, cx| ws.activate_doc(&doc, window, cx))
                        .ok();
                    let has_path = doc.read(cx).path.is_some();
                    let split = ws
                        .read_with(cx, |ws, _| ws.panes.len() > 1)
                        .unwrap_or(false);
                    menu.menu("Close", Box::new(CloseTab))
                        .menu("Close Others", Box::new(CloseOtherTabs))
                        .separator()
                        .menu(
                            if split {
                                "Move to Other Side"
                            } else {
                                "Split Right"
                            },
                            Box::new(SplitRight),
                        )
                        .when(split, |m| m.menu("Join Sides", Box::new(JoinPanes)))
                        .separator()
                        .menu_with_disabled(
                            "Open Containing Folder",
                            Box::new(RevealInFileManager),
                            !has_path,
                        )
                }
            })
            .into_any_element()
    }

    fn render_tab_strip(&self, pane: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs: Vec<_> = self.panes[pane].tabs.clone();
        let accent = cx.theme().primary;
        let mut items = Vec::with_capacity(tabs.len());
        for (ix, doc) in tabs.iter().enumerate() {
            items.push(self.render_tab(pane, ix, doc, cx));
        }
        div()
            .id(("tab-strip", pane))
            .flex()
            .items_center()
            .gap(px(2.))
            .min_w_0()
            .overflow_x_scroll()
            .children(items)
            // Empty space after the last tab: drop there to put a tab last.
            .child(
                div()
                    .id(("tab-strip-end", pane))
                    .h(px(28.))
                    .min_w(px(24.))
                    .flex_1()
                    .drag_over::<DraggedTab>(move |s, _, _, _| s.border_l_2().border_color(accent))
                    .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                        this.move_doc(dragged.doc.clone(), pane, None, window, cx);
                    })),
            )
            // Stop the title bar from treating a press here as a window move.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    }

    fn render_app_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = cx.entity().downgrade();
        Button::new("app-menu")
            .ghost()
            .small()
            .icon(IconName::Menu)
            .tooltip("Menu")
            .dropdown_menu(move |menu, window, cx| {
                let (has_doc, can_save_as, split, sidebar, has_markdown) = ws
                    .read_with(cx, |ws, cx| {
                        let doc = ws.active_doc().map(|d| d.read(cx));
                        (
                            doc.is_some(),
                            doc.is_some_and(|d| d.can_save_as()),
                            ws.panes.len() > 1,
                            ws.sidebar_open,
                            doc.is_some_and(|d| d.markdown().is_some()),
                        )
                    })
                    .unwrap_or_default();
                let prefs = AppState::prefs(cx);
                let recent: Vec<(usize, String)> = prefs
                    .recent
                    .iter()
                    .enumerate()
                    .map(|(ix, p)| {
                        let (name, dir) = recent_parts(p, 40);
                        (
                            ix,
                            if dir.is_empty() {
                                name
                            } else {
                                format!("{name}  —  {dir}")
                            },
                        )
                    })
                    .collect();
                let current_theme = prefs.theme().to_string();
                menu.menu("Command Palette…", Box::new(CommandPalette))
                    .menu("Quick Open…", Box::new(QuickOpen))
                    .separator()
                    .menu("New", Box::new(NewFile))
                    .menu("Open…", Box::new(Open))
                    .menu("Open Folder…", Box::new(OpenFolder))
                    .submenu("Open Recent", window, cx, move |mut m, _, _| {
                        if recent.is_empty() {
                            return m.label("No Recent Files");
                        }
                        for (ix, label) in &recent {
                            m = m.menu(label.clone(), Box::new(OpenRecent { ix: *ix }));
                        }
                        m.separator().menu("Clear Recent", Box::new(ClearRecent))
                    })
                    .separator()
                    .menu_with_disabled("Export as HTML…", Box::new(ExportHtml), !has_markdown)
                    .menu_with_disabled("Export as PDF…", Box::new(ExportPdf), !has_markdown)
                    .menu_with_disabled("Print…", Box::new(Print), !has_markdown)
                    .separator()
                    .menu("Import HTML…", Box::new(ImportHtml))
                    .menu("Import Word Document…", Box::new(ImportWord))
                    .separator()
                    .menu_with_disabled("Save", Box::new(Save), !has_doc)
                    .menu_with_disabled("Save As…", Box::new(SaveAs), !can_save_as)
                    .separator()
                    .menu_with_disabled("Close Tab", Box::new(CloseTab), !has_doc)
                    .separator()
                    .submenu("View", window, cx, move |m, _, _| {
                        m.menu_with_check("Sidebar", sidebar, Box::new(ToggleSidebar))
                            .separator()
                            .menu_with_disabled(
                                if split {
                                    "Move to Other Side"
                                } else {
                                    "Split Right"
                                },
                                Box::new(SplitRight),
                                !has_doc,
                            )
                            .menu_with_disabled("Join Sides", Box::new(JoinPanes), !split)
                            .separator()
                            .menu_with_disabled("Next Document", Box::new(NextTab), !has_doc)
                            .menu_with_disabled("Previous Document", Box::new(PrevTab), !has_doc)
                    })
                    .submenu("Theme", window, cx, move |m, window, cx| {
                        theme_menu(m, &current_theme, window, cx)
                    })
                    .separator()
                    .menu("About mdedit", Box::new(About))
                    .menu("Quit", Box::new(Quit))
            })
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let single = self.panes.len() == 1;
        let muted = cx.theme().muted_foreground;
        TitleBar::new()
            .on_close_window(|_, window, cx| window.dispatch_action(Box::new(Quit), cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .pl(px(8.))
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(self.render_app_menu(cx))
                            .child(
                                Button::new("toggle-sidebar")
                                    .ghost()
                                    .small()
                                    .icon(if self.sidebar_open {
                                        IconName::PanelLeftClose
                                    } else {
                                        IconName::PanelLeftOpen
                                    })
                                    .tooltip("Toggle Sidebar")
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(Box::new(ToggleSidebar), cx)
                                    }),
                            ),
                    )
                    .when(single, |d| d.child(self.render_tab_strip(0, cx)))
                    .when(!single, |d| {
                        d.child(div().pl(px(8.)).text_sm().text_color(muted).child("mdedit"))
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .pr(px(4.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        Button::new("command-palette")
                            .ghost()
                            .small()
                            .icon(IconName::Search)
                            .tooltip("Command Palette")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(CommandPalette), cx)
                            }),
                    )
                    .child(
                        Button::new("new-tab")
                            .ghost()
                            .small()
                            .icon(IconName::Plus)
                            .tooltip("New Document")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(NewFile), cx)
                            }),
                    ),
            )
    }

    fn render_welcome(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, hover, border) = (theme.muted_foreground, theme.accent, theme.border);
        let recent: Vec<(usize, String, String)> = AppState::prefs(cx)
            .recent
            .iter()
            .take(6)
            .enumerate()
            .map(|(ix, p)| {
                let (n, d) = recent_parts(p, 48);
                (ix, n, d)
            })
            .collect();
        let action_row = |id: &'static str,
                          icon: IconName,
                          label: &'static str,
                          keys: &'static str,
                          action: Box<dyn Action>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(10.))
                .py(px(7.))
                .rounded(px(6.))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(Icon::new(icon).small().text_color(muted))
                .child(div().flex_1().child(label))
                .child(div().text_xs().text_color(muted).child(keys))
                .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
        };
        let mod_key = if cfg!(target_os = "macos") {
            "⌘"
        } else {
            "Ctrl+"
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(420.))
                    .flex()
                    .flex_col()
                    .gap(px(18.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("mdedit"),
                            )
                            .child(
                                div()
                                    .text_color(muted)
                                    .child("A quiet place to write Markdown."),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(action_row(
                                "w-new",
                                IconName::Plus,
                                "New Document",
                                leak(format!("{mod_key}N")),
                                Box::new(NewFile),
                            ))
                            .child(action_row(
                                "w-open",
                                IconName::File,
                                "Open…",
                                leak(format!("{mod_key}O")),
                                Box::new(Open),
                            ))
                            .child(action_row(
                                "w-folder",
                                IconName::FolderOpen,
                                "Open Folder…",
                                leak(format!("{mod_key}Shift+K")),
                                Box::new(OpenFolder),
                            )),
                    )
                    .when(!recent.is_empty(), |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .pt(px(12.))
                                .border_t_1()
                                .border_color(border)
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .px(px(10.))
                                        .pb(px(4.))
                                        .child("RECENT"),
                                )
                                .children(recent.into_iter().map(|(ix, name, dir)| {
                                    div()
                                        .id(("w-recent", ix))
                                        .flex()
                                        .items_baseline()
                                        .gap(px(10.))
                                        .px(px(10.))
                                        .py(px(5.))
                                        .rounded(px(6.))
                                        .cursor_pointer()
                                        .hover(move |s| s.bg(hover))
                                        .child(div().flex_none().child(name))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(muted)
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .whitespace_nowrap()
                                                .child(dir),
                                        )
                                        .on_click(move |_, window, cx| {
                                            window.dispatch_action(Box::new(OpenRecent { ix }), cx)
                                        })
                                })),
                        )
                    }),
            )
    }

    fn render_pane(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let split = self.panes.len() > 1;
        let theme = cx.theme();
        let (border, drop_bg) = (theme.border, theme.primary.opacity(0.08));
        let content = match self.panes[ix].active_doc() {
            Some(doc) => div()
                .flex_1()
                .min_h_0()
                .child(doc.clone())
                .into_any_element(),
            None => div()
                .flex_1()
                .min_h_0()
                .child(self.render_welcome(cx))
                .into_any_element(),
        };
        div()
            .id(("pane", ix))
            .size_full()
            .flex()
            .flex_col()
            .when(split && ix > 0, |d| d.border_l_1().border_color(border))
            .on_any_mouse_down(cx.listener(move |this, _, _, cx| {
                if this.active != ix {
                    this.active = ix;
                    this.sync_files(cx);
                    cx.notify();
                }
            }))
            .when(split, |d| {
                d.child(
                    div()
                        .h(px(36.))
                        .flex()
                        .items_center()
                        .px(px(6.))
                        .bg(cx.theme().tab_bar)
                        .border_b_1()
                        .border_color(border)
                        .child(self.render_tab_strip(ix, cx)),
                )
            })
            .drag_over::<DraggedTab>(move |s, _, _, _| s.bg(drop_bg))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                this.move_doc(dragged.doc.clone(), ix, None, window, cx);
            }))
            .child(content)
            .into_any_element()
    }

    fn render_panes(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.panes.len() == 1 {
            return self.render_pane(0, cx);
        }
        let mut group = h_resizable(("panes", self.panes.len()));
        for ix in 0..self.panes.len() {
            group = group.child(resizable_panel().child(self.render_pane(ix, cx)));
        }
        group.into_any_element()
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border, bg) = (
            theme.muted_foreground,
            theme.status_bar_border,
            theme.status_bar,
        );
        let doc = self.active_doc().cloned();
        let mode = doc
            .as_ref()
            .and_then(|d| d.read(cx).markdown().cloned())
            .map(|e| (e.read(cx).mode(), e));
        let segment_bg = cx.theme().secondary;
        let (path, facts, hex_mode) = match &doc {
            Some(doc) => {
                let d = doc.read(cx);
                let path = d
                    .path
                    .as_ref()
                    .map(|p| {
                        let (name, dir) = recent_parts(p, 80);
                        if dir.is_empty() {
                            name
                        } else {
                            format!("{dir}/{name}")
                        }
                    })
                    .unwrap_or_else(|| "Not saved yet".into());
                let hex_mode = d.hex().map(|h| h.read(cx).overwrite);
                (path, d.status(cx), hex_mode)
            }
            None => (String::new(), Vec::new(), None),
        };
        div()
            .h(px(26.))
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .gap(px(16.))
            .px(px(12.))
            .bg(bg)
            .border_t_1()
            .border_color(border)
            .text_xs()
            .text_color(muted)
            .child(
                div()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(path),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .children(
                        facts
                            .into_iter()
                            .map(|f| div().whitespace_nowrap().child(f)),
                    )
                    .when_some(mode, |d, (current, editor)| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(1.))
                                .p(px(1.))
                                .rounded(px(6.))
                                .bg(segment_bg)
                                .children([Mode::Rendered, Mode::Live, Mode::Source].map(|m| {
                                    let editor = editor.clone();
                                    let label = match m {
                                        Mode::Rendered => "Rendered",
                                        Mode::Live => "Live",
                                        Mode::Source => "Source",
                                    };
                                    Button::new(label)
                                        .xsmall()
                                        .label(label)
                                        .map(|b| if m == current { b.outline() } else { b.ghost() })
                                        .on_click(move |_, _, cx| {
                                            editor.update(cx, |e, cx| e.set_mode(m, cx));
                                            AppState::update_prefs(cx, |p| p.mode = m);
                                        })
                                })),
                        )
                    })
                    .when_some(hex_mode, |d, overwrite| {
                        d.child(
                            Button::new("hex-mode")
                                .ghost()
                                .xsmall()
                                .label(if overwrite { "OVR" } else { "INS" })
                                .tooltip("Overwrite / insert (Insert)")
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(ToggleHexOverwrite), cx)
                                }),
                        )
                    }),
            )
    }
}

impl Workspace {
    fn render_sidebar_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (fg, muted, active_bg) = (theme.foreground, theme.muted_foreground, theme.background);
        let tab = |id: &'static str,
                   label: &'static str,
                   which: SidebarTab,
                   current: SidebarTab,
                   cx: &mut Context<Self>| {
            let on = which == current;
            div()
                .id(id)
                .flex_1()
                .flex()
                .justify_center()
                .py(px(3.))
                .rounded(px(5.))
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .text_color(if on { fg } else { muted })
                .when(on, |d| d.bg(active_bg).shadow_xs())
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.sidebar_tab = which;
                    cx.notify();
                }))
        };
        let current = self.sidebar_tab;
        div()
            .flex()
            .gap(px(2.))
            .mx(px(10.))
            .mt(px(10.))
            .mb(px(2.))
            .p(px(2.))
            .rounded(px(7.))
            .bg(theme.secondary)
            .child(tab("tab-files", "Files", SidebarTab::Files, current, cx))
            .child(tab(
                "tab-outline",
                "Outline",
                SidebarTab::Outline,
                current,
                cx,
            ))
    }

    /// The active document's headings; the one the caret is under is
    /// marked, and a click goes there.
    fn render_outline(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, fg, hover, active_bg) = (
            theme.muted_foreground,
            theme.foreground,
            theme.sidebar_accent,
            theme.list_active,
        );
        let editor = self
            .active_doc()
            .and_then(|d| d.read(cx).markdown().cloned());
        let Some(editor) = editor else {
            return div()
                .p(px(14.))
                .text_xs()
                .text_color(muted)
                .child("Open a Markdown document to see its outline.")
                .into_any_element();
        };
        let e = editor.read(cx);
        let syntax = e.syntax();
        let caret_line = syntax.line_at(e.selection().head);
        let headings: Vec<_> = syntax
            .headings
            .iter()
            .map(|h| {
                (
                    h.level,
                    h.text.clone(),
                    syntax.lines[h.line].range.start,
                    h.line,
                )
            })
            .collect();
        if headings.is_empty() {
            return div()
                .p(px(14.))
                .text_xs()
                .text_color(muted)
                .child("No headings yet. Start a line with # to add one.")
                .into_any_element();
        }
        let current = headings
            .iter()
            .rposition(|(_, _, _, line)| *line <= caret_line);
        let top = headings.iter().map(|h| h.0).min().unwrap_or(1);
        div()
            .id("outline")
            .size_full()
            .overflow_y_scroll()
            .py(px(6.))
            .text_sm()
            .children(
                headings
                    .into_iter()
                    .enumerate()
                    .map(|(i, (level, text, start, _))| {
                        let editor = editor.clone();
                        div()
                            .id(("heading", i))
                            .mx(px(6.))
                            .pl(px(10. + (level - top) as f32 * 14.))
                            .pr(px(8.))
                            .py(px(4.))
                            .rounded(px(5.))
                            .cursor_pointer()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(if level <= top { fg } else { muted })
                            .when(level <= top, |d| d.font_weight(FontWeight::MEDIUM))
                            .hover(|s| s.bg(hover))
                            .when(current == Some(i), |d| d.bg(active_bg).text_color(fg))
                            .child(if text.is_empty() {
                                "Untitled heading".to_string()
                            } else {
                                text
                            })
                            .on_click(move |_, window, cx| {
                                editor.update(cx, |e, cx| e.reveal_offset(start, window, cx));
                            })
                    }),
            )
            .into_any_element()
    }
}

/// The Theme submenu: Match System, then a submenu per family.
fn theme_menu(
    menu: PopupMenu,
    current: &str,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let mut menu = menu
        .menu_with_check(
            "Match System",
            current.is_empty(),
            Box::new(SetTheme { id: "".into() }),
        )
        .separator();
    for (family, items) in themes::FAMILIES {
        let current = current.to_string();
        let in_family = items.iter().any(|(id, _)| *id == current);
        menu = menu.submenu(
            if in_family {
                format!("{family} ✓")
            } else {
                family.to_string()
            },
            window,
            cx,
            move |mut m, _, _| {
                for (id, label) in *items {
                    m = m.menu_with_check(
                        *label,
                        *id == current,
                        Box::new(SetTheme { id: (*id).into() }),
                    );
                }
                m
            },
        );
    }
    menu
}

/// Shortcut labels live as long as the app; leaking six short strings once
/// keeps the welcome rows' builder simple.
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_window_title(&self.window_title(cx));
        let theme = cx.theme();
        let (bg, fg, sidebar_bg, border, drop_bg) = (
            theme.background,
            theme.foreground,
            theme.sidebar,
            theme.sidebar_border,
            theme.primary.opacity(0.06),
        );
        div()
            .key_context("Workspace")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::new_file))
            .on_action(cx.listener(Self::open))
            .on_action(cx.listener(Self::open_folder))
            .on_action(cx.listener(Self::close_folder))
            .on_action(cx.listener(Self::command_palette))
            .on_action(cx.listener(Self::import_html))
            .on_action(cx.listener(Self::import_word))
            .on_action(cx.listener(Self::export_html))
            .on_action(cx.listener(Self::quick_open))
            .on_action(cx.listener(Self::open_recent))
            .on_action(cx.listener(Self::clear_recent))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::save_as))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::close_other_tabs))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::split_right))
            .on_action(cx.listener(Self::join_panes))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::reveal))
            .on_action(cx.listener(Self::set_theme))
            .on_action(cx.listener(Self::about))
            .on_action(cx.listener(Self::toggle_hex_overwrite))
            .drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(drop_bg))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.open_paths(paths.paths().to_vec(), window, cx);
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .child(self.render_title_bar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(self.sidebar_open, |d| {
                        d.child(
                            div()
                                .w(px(SIDEBAR_WIDTH))
                                .flex_none()
                                .h_full()
                                .bg(sidebar_bg)
                                .border_r_1()
                                .border_color(border)
                                .flex()
                                .flex_col()
                                .child(self.render_sidebar_tabs(cx))
                                .child(div().flex_1().min_h_0().child(match self.sidebar_tab {
                                    SidebarTab::Files => self.files.clone().into_any_element(),
                                    SidebarTab::Outline => {
                                        self.render_outline(cx).into_any_element()
                                    }
                                })),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.render_panes(cx)),
                    ),
            )
            .child(self.render_status_bar(cx))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{
        AnyWindowHandle, AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds,
        WindowOptions, px, size,
    };

    use super::{Answer, DiskChoice, Pending, Workspace};
    use crate::actions::*;
    use crate::state::AppState;

    fn setup(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            AppState::init_ephemeral(cx);
            crate::actions::bind_keys(cx);
        });
        let (_tx, rx) = async_channel::unbounded();
        let (handle, ws) = cx.update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Point::default(),
                        size: size(px(1000.), px(700.)),
                    })),
                    ..Default::default()
                },
                cx,
                move |window, cx| cx.new(|cx| Workspace::new(vec![], rx, window, cx)),
            )
            .expect("open the test window")
        });
        cx.run_until_parked();
        (handle, ws)
    }

    fn act(cx: &mut TestAppContext, handle: AnyWindowHandle, action: impl gpui_kit::Action) {
        cx.update_window(handle, |_, window, cx| {
            window.dispatch_action(Box::new(action), cx)
        })
        .unwrap();
        cx.run_until_parked();
    }

    fn tabs(cx: &mut TestAppContext, ws: &Entity<Workspace>) -> Vec<Vec<String>> {
        ws.read_with(cx, |ws, cx| {
            ws.panes
                .iter()
                .map(|p| p.tabs.iter().map(|d| d.read(cx).name()).collect())
                .collect()
        })
    }

    fn type_into_active(
        cx: &mut TestAppContext,
        handle: AnyWindowHandle,
        _ws: &Entity<Workspace>,
        text: &str,
    ) {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.input(text, cx);
        })
        .unwrap();
        cx.run_until_parked();
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mdedit-ws-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[gpui_kit::test]
    fn new_documents_are_numbered_and_split_and_join(cx: &mut TestAppContext) {
        let (h, ws) = setup(cx);
        act(cx, h, NewFile);
        act(cx, h, NewFile);
        act(cx, h, NewFile);
        assert_eq!(
            tabs(cx, &ws),
            vec![vec!["Untitled", "Untitled 2", "Untitled 3"]]
        );

        act(cx, h, SplitRight);
        assert_eq!(
            tabs(cx, &ws),
            vec![vec!["Untitled", "Untitled 2"], vec!["Untitled 3"]]
        );
        // Split again: the lone tab moves back to the other side, which
        // folds the emptied pane away.
        act(cx, h, SplitRight);
        assert_eq!(tabs(cx, &ws).len(), 1);

        act(cx, h, SplitRight);
        act(cx, h, JoinPanes);
        assert_eq!(
            tabs(cx, &ws),
            vec![vec!["Untitled", "Untitled 2", "Untitled 3"]]
        );

        act(cx, h, CloseTab);
        assert_eq!(tabs(cx, &ws)[0].len(), 2);
        // The freed number is reused.
        act(cx, h, NewFile);
        assert!(tabs(cx, &ws)[0].contains(&"Untitled 3".to_string()));
    }

    #[gpui_kit::test]
    fn shortcuts_reach_the_workspace(cx: &mut TestAppContext) {
        let (h, ws) = setup(cx);
        cx.update_window(h, |_, window, cx| {
            window.press("ctrl-n", cx);
            window.press("ctrl-n", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(tabs(cx, &ws)[0].len(), 2);
        // From inside the editor too, where the focus now is.
        cx.update_window(h, |_, window, cx| window.press("ctrl-w", cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(tabs(cx, &ws)[0].len(), 1);
        let open = cx.update(|cx| AppState::prefs(cx).sidebar_open);
        cx.update_window(h, |_, window, cx| window.press("ctrl-shift-l", cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(ws.read_with(cx, |ws, _| ws.sidebar_open), !open);
    }

    #[gpui_kit::test]
    fn find_steps_through_matches_and_escape_closes(cx: &mut TestAppContext) {
        let (h, ws) = setup(cx);
        act(cx, h, NewFile);
        type_into_active(cx, h, &ws, "cat hat cat");
        let editor = ws.read_with(cx, |ws, cx| {
            ws.active_doc()
                .unwrap()
                .read(cx)
                .markdown()
                .cloned()
                .unwrap()
        });
        cx.update_window(h, |_, window, cx| {
            window.press("ctrl-home", cx);
            window.render_frame(cx);
            window.press("ctrl-f", cx);
            window.render_frame(cx);
            window.input("cat", cx);
            window.render_frame(cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let sel = editor.read_with(cx, |e, _| e.selection().range());
        assert_eq!(sel, 8..11, "Enter goes to the next match after the caret's");
        cx.update_window(h, |_, window, cx| {
            window.render_frame(cx);
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(
            editor.read_with(cx, |e, _| e.find.is_none()),
            "Escape closes it"
        );
    }

    #[gpui_kit::test]
    fn the_palette_runs_commands_on_the_editor(cx: &mut TestAppContext) {
        let (h, ws) = setup(cx);
        act(cx, h, NewFile);
        type_into_active(cx, h, &ws, "word");
        cx.update_window(h, |_, window, cx| {
            window.render_frame(cx);
            window.press("ctrl-a", cx);
            window.press("ctrl-shift-p", cx);
            window.render_frame(cx);
            window.render_frame(cx);
            window.input("Bold", cx);
        })
        .unwrap();
        // The palette filters in the background.
        cx.run_until_parked();
        cx.update_window(h, |_, window, cx| {
            window.render_frame(cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let text = ws.read_with(cx, |ws, cx| {
            let doc = ws.active_doc().unwrap().read(cx);
            doc.markdown().unwrap().read(cx).text().to_string()
        });
        assert_eq!(text, "**word**");
        assert_eq!(tabs(cx, &ws)[0].len(), 1);
    }

    #[gpui_kit::test]
    fn outside_changes_are_noticed_and_merged(cx: &mut TestAppContext) {
        let (h, ws) = setup(cx);
        let dir = scratch("disk");
        let path = dir.join("n.md");
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
        cx.update_window(h, |_, window, cx| {
            ws.update(cx, |ws, cx| ws.open_paths(vec![path.clone()], window, cx))
        })
        .unwrap();
        cx.run_until_parked();
        let doc = ws.read_with(cx, |ws, _| ws.active_doc().cloned().unwrap());
        let editor = doc.read_with(cx, |d, _| d.markdown().cloned().unwrap());
        // Our change at the top; someone else's at the bottom.
        editor.update(cx, |e, cx| e.set_text("ONE\ntwo\nthree\n".into(), cx));
        std::fs::write(&path, "one\ntwo\nTHREE\n").unwrap();
        let watch = ws.read_with(cx, |ws, _| ws.watch.clone());
        watch.check_all();
        cx.run_until_parked();
        assert!(doc.read_with(cx, |d, _| d.stale), "the change is noticed");

        // Saving now would lose their change: it doesn't happen.
        act(cx, h, Save);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo\nTHREE\n");

        cx.update_window(h, |_, window, cx| {
            ws.update(cx, |ws, cx| {
                ws.resolve_disk(doc.clone(), DiskChoice::Merge, window, cx)
            })
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(
            editor.read_with(cx, |e, _| e.text().to_string()),
            "ONE\ntwo\nTHREE\n"
        );
        assert!(!doc.read_with(cx, |d, _| d.stale));
        assert!(
            doc.read_with(cx, |d, _| d.is_dirty()),
            "the merge isn't saved yet"
        );
        act(cx, h, Save);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "ONE\ntwo\nTHREE\n");

        // A rewrite with the same bytes is not news.
        std::fs::write(&path, "ONE\ntwo\nTHREE\n").unwrap();
        watch.check_all();
        cx.run_until_parked();
        assert!(!doc.read_with(cx, |d, _| d.stale));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui_kit::test]
    fn opening_reads_each_kind_and_never_opens_twice(cx: &mut TestAppContext) {
        let (h, ws) = setup(cx);
        let dir = scratch("open");
        std::fs::write(dir.join("a.md"), "# A\n").unwrap();
        std::fs::write(dir.join("b.bin"), [0u8, 1, 2]).unwrap();
        let paths = vec![dir.join("a.md"), dir.join("b.bin")];
        cx.update_window(h, |_, window, cx| {
            ws.update(cx, |ws, cx| ws.open_paths(paths.clone(), window, cx))
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(tabs(cx, &ws), vec![vec!["a.md", "b.bin"]]);
        assert!(ws.read_with(cx, |ws, cx| {
            ws.active_doc().unwrap().read(cx).hex().is_some()
        }));

        cx.update_window(h, |_, window, cx| {
            ws.update(cx, |ws, cx| {
                ws.open_paths(vec![dir.join("a.md")], window, cx)
            })
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(tabs(cx, &ws), vec![vec!["a.md", "b.bin"]]);
        assert_eq!(
            ws.read_with(cx, |ws, cx| ws.active_doc().unwrap().read(cx).name()),
            "a.md"
        );
        let recent = cx.update(|cx| AppState::prefs(cx).recent.clone());
        assert_eq!(recent.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui_kit::test]
    fn closing_a_modified_tab_asks_first(cx: &mut TestAppContext) {
        let (h, ws) = setup(cx);
        act(cx, h, NewFile);
        type_into_active(cx, h, &ws, "hello");
        assert!(ws.read_with(cx, |ws, cx| ws.active_doc().unwrap().read(cx).is_dirty()));

        act(cx, h, CloseTab);
        assert!(ws.read_with(cx, |ws, _| ws.pending.is_some()));
        cx.update_window(h, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("save").is_some(),
                "the question is on screen"
            );
        })
        .unwrap();
        // Through the dialog's own buttons: Cancel keeps the tab, Don't
        // Save drops it.
        cx.update_window(h, |_, window, cx| window.click("cancel", cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(tabs(cx, &ws)[0].len(), 1);
        assert!(ws.read_with(cx, |ws, _| ws.pending.is_none()));
        act(cx, h, CloseTab);
        cx.update_window(h, |_, window, cx| {
            window.render_frame(cx);
            window.click("dont-save", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(tabs(cx, &ws)[0].is_empty());
    }

    #[gpui_kit::test]
    fn saving_writes_the_text_and_clears_modified(cx: &mut TestAppContext) {
        let (h, ws) = setup(cx);
        let dir = scratch("save");
        act(cx, h, NewFile);
        type_into_active(cx, h, &ws, "# Saved\n");
        let doc = ws.read_with(cx, |ws, _| ws.active_doc().cloned().unwrap());
        let path = dir.join("saved.md");
        cx.update_window(h, |_, window, cx| {
            ws.update(cx, |ws, cx| {
                ws.write(doc.clone(), &path, super::Then::Nothing, window, cx)
            })
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Saved\n");
        assert!(!doc.read_with(cx, |d, _| d.is_dirty()));
        assert_eq!(doc.read_with(cx, |d, _| d.name()), "saved.md");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui_kit::test]
    fn quitting_walks_the_modified_tabs(cx: &mut TestAppContext) {
        let (h, ws) = setup(cx);
        act(cx, h, NewFile);
        type_into_active(cx, h, &ws, "one");
        act(cx, h, NewFile);
        type_into_active(cx, h, &ws, "two");
        act(cx, h, NewFile); // clean, never asked about

        let may_close = cx
            .update_window(h, |_, window, cx| {
                ws.update(cx, |ws, cx| ws.request_quit(window, cx))
            })
            .unwrap();
        assert!(!may_close);
        let asked = |cx: &mut TestAppContext| {
            ws.read_with(cx, |ws, cx| match &ws.pending {
                Some(Pending::Quit(d)) => Some(d.read(cx).name()),
                _ => None,
            })
        };
        assert_eq!(asked(cx).as_deref(), Some("Untitled"));
        cx.update_window(h, |_, window, cx| {
            ws.update(cx, |ws, cx| ws.answer(Answer::Discard, window, cx))
        })
        .unwrap();
        assert_eq!(asked(cx).as_deref(), Some("Untitled 2"));
        // Cancel stops the whole quit, and forgets the earlier answer.
        cx.update_window(h, |_, window, cx| {
            ws.update(cx, |ws, cx| ws.answer(Answer::Cancel, window, cx))
        })
        .unwrap();
        assert_eq!(asked(cx), None);
        assert!(ws.read_with(cx, |ws, _| ws.quit_discarded.is_empty()));
    }
}
