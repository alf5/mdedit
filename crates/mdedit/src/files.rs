//! The sidebar's file tree.
//!
//! A flat list of visible rows rather than a nested tree: expanding a folder
//! reads it and splices its children in, collapsing drains them back out. So
//! a folder with a huge `node_modules` costs nothing until someone opens it,
//! and the list stays virtualized however deep it goes. Not remembered
//! between launches, in keeping with tabs.

use std::path::{Path, PathBuf};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mdedit_core::project::{self, Entry};

use crate::actions::OpenFolder;

const ROW_HEIGHT: f32 = 26.;

struct Row {
    entry: Entry,
    depth: usize,
    open: bool,
}

pub enum FilesEvent {
    Open(PathBuf),
}

pub struct FilesPanel {
    root: Option<PathBuf>,
    rows: Vec<Row>,
    selected: Option<usize>,
    /// The active tab's file, highlighted where it appears.
    active: Option<PathBuf>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
}

impl EventEmitter<FilesEvent> for FilesPanel {}

impl Focusable for FilesPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl FilesPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        FilesPanel {
            root: None,
            rows: Vec::new(),
            selected: None,
            active: None,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
        }
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    pub fn set_root(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        self.root = root;
        self.selected = None;
        self.rows = match &self.root {
            Some(root) => Self::read(root, 0),
            None => Vec::new(),
        };
        cx.notify();
    }

    pub fn set_active(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.active != path {
            self.active = path;
            cx.notify();
        }
    }

    fn read(dir: &Path, depth: usize) -> Vec<Row> {
        project::list_dir(dir)
            .unwrap_or_default()
            .into_iter()
            .map(|entry| Row {
                entry,
                depth,
                open: false,
            })
            .collect()
    }

    fn activate(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.selected = Some(ix);
        let Some(row) = self.rows.get(ix) else { return };
        if row.entry.dir {
            self.toggle(ix, cx);
        } else {
            cx.emit(FilesEvent::Open(row.entry.path.clone()));
        }
        cx.notify();
    }

    fn toggle(&mut self, ix: usize, cx: &mut Context<Self>) {
        let row = &mut self.rows[ix];
        let depth = row.depth;
        if row.open {
            row.open = false;
            let end = self.rows[ix + 1..]
                .iter()
                .position(|r| r.depth <= depth)
                .map_or(self.rows.len(), |n| ix + 1 + n);
            self.rows.drain(ix + 1..end);
        } else {
            row.open = true;
            let children = Self::read(&row.entry.path, depth + 1);
            self.rows.splice(ix + 1..ix + 1, children);
        }
        cx.notify();
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let last = self.rows.len().saturating_sub(1);
        let sel = self.selected;
        let next = match ev.keystroke.key.as_str() {
            "down" => sel.map_or(0, |s| (s + 1).min(last)),
            "up" => sel.map_or(0, |s| s.saturating_sub(1)),
            "enter" => {
                if let Some(s) = sel {
                    self.activate(s, cx);
                }
                cx.stop_propagation();
                return;
            }
            "right" | "left" => {
                if let Some(s) = sel
                    && let Some(row) = self.rows.get(s)
                    && row.entry.dir
                    && row.open == (ev.keystroke.key == "left")
                {
                    self.toggle(s, cx);
                }
                cx.stop_propagation();
                return;
            }
            _ => return,
        };
        if !self.rows.is_empty() {
            self.selected = Some(next);
            self.scroll.scroll_to_item(next, ScrollStrategy::Nearest);
            cx.notify();
        }
        cx.stop_propagation();
    }

    fn render_rows(
        &mut self,
        range: std::ops::Range<usize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (hover, selected_bg, muted, accent) = (
            theme.sidebar_accent,
            theme.list_active,
            theme.muted_foreground,
            theme.primary,
        );
        range
            .filter_map(|ix| {
                let row = self.rows.get(ix)?;
                let icon = if row.entry.dir {
                    if row.open {
                        IconName::FolderOpen
                    } else {
                        IconName::Folder
                    }
                } else if project::is_markdown_ext(&row.entry.path) {
                    IconName::FileText
                } else {
                    IconName::File
                };
                let chevron = row.entry.dir.then_some(if row.open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                });
                let is_active = self.active.as_deref() == Some(row.entry.path.as_path());
                Some(
                    div()
                        .w_full()
                        .px(px(6.))
                        .child(
                            div()
                                .id(("file-row", ix))
                                .w_full()
                                .h(px(ROW_HEIGHT))
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .pl(px(8. + row.depth as f32 * 14.))
                                .pr(px(8.))
                                .rounded(px(5.))
                                .cursor_pointer()
                                .hover(|s| s.bg(hover))
                                .when(self.selected == Some(ix) || is_active, |d| {
                                    d.bg(selected_bg)
                                })
                                .child(div().w(px(14.)).flex().justify_center().children(
                                    chevron.map(|c| Icon::new(c).xsmall().text_color(muted)),
                                ))
                                .child(Icon::new(icon).small().text_color(if row.entry.dir {
                                    accent
                                } else {
                                    muted
                                }))
                                .child(
                                    div()
                                        .flex_1()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .child(row.entry.name.clone()),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.focus.focus(window, cx);
                                    this.activate(ix, cx);
                                })),
                        )
                        .into_any_element(),
                )
            })
            .collect()
    }

    /// No folder open: offer one.
    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(12.))
            .child(
                Button::new("open-folder")
                    .outline()
                    .small()
                    .icon(IconName::FolderOpen)
                    .label("Open Folder…")
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenFolder), cx)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("Open a folder to browse its files here."),
            )
    }
}

impl Render for FilesPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let title = self
            .root
            .as_ref()
            .map(|r| project::file_name_of(r))
            .unwrap_or_else(|| "No folder".into());
        let body = if self.root.is_some() {
            div()
                .flex_1()
                .min_h_0()
                .child(
                    uniform_list("files", self.rows.len(), cx.processor(Self::render_rows))
                        .track_scroll(&self.scroll)
                        .size_full(),
                )
                .into_any_element()
        } else {
            self.render_empty(cx).into_any_element()
        };
        div()
            .key_context("FilesPanel")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .flex()
            .flex_col()
            .text_sm()
            .child(
                div()
                    .h(px(34.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(14.))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(muted)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(title.to_uppercase()),
                    )
                    .child(
                        Button::new("choose-folder")
                            .ghost()
                            .xsmall()
                            .icon(IconName::FolderOpen)
                            .tooltip("Open Folder…")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(OpenFolder), cx)
                            }),
                    ),
            )
            .child(body)
    }
}
