//! One open document: a tab's content, whatever kind of file it is.
//!
//! What a file *is* decides what it gets: markdown gets the live-preview
//! editor, other text a code editor, images are shown, binaries get the hex
//! editor, and a file past its size limit says so rather than pretending to
//! be empty.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Editor, EditorState, InputEvent};
use gpui_kit::component::{ActiveTheme, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mdedit_core::project::{self, AnyFile, Content};

use crate::actions::{Find, FindReplace};
use crate::editor::find_bar::{FindBar, FindBarEvent};
use crate::editor::style::DocStyle;
use crate::editor::{EditorEvent, MarkdownEditor};
use crate::hex_view::{HexEvent, HexView};
use crate::settings::SettingsPage;
use crate::state::AppState;
use crate::theme::{MONO_FONT, UI_FONT};

pub enum Body {
    Markdown {
        editor: Entity<MarkdownEditor>,
        saved: String,
    },
    Text {
        editor: Entity<EditorState>,
        markdown: bool,
        /// The text as last loaded or saved, which is what "modified" means.
        saved: SharedString,
    },
    Image {
        image: Option<Arc<Image>>,
        size: u64,
    },
    Hex(Entity<HexView>),
    Oversized {
        size: u64,
    },
    Settings(Entity<SettingsPage>),
}

/// Something about the tab changed: modified state, name, or content.
pub enum DocEvent {
    Changed,
    /// The user answered the changed-on-disk banner.
    Disk(DiskChoice),
    /// A `[[wiki link]]` was followed: open its note, if it has one.
    FollowWiki {
        note: String,
        path: Option<PathBuf>,
    },
    /// Files to open (dropped on the text).
    Open(Vec<PathBuf>),
    /// A message for the user.
    Notice(String),
}

/// What to do about a file that changed underneath its tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskChoice {
    /// Take the file's version.
    Reload,
    /// Three-way merge of the tab's changes and the file's.
    Merge,
    /// Keep the tab's version; the next save overwrites the file.
    Keep,
}

pub struct DocView {
    pub path: Option<PathBuf>,
    /// "Untitled 2" is number 2.
    untitled: usize,
    pub body: Body,
    dirty: bool,
    words: usize,
    chars: usize,
    focus: FocusHandle,
    find: Option<Entity<FindBar>>,
    /// The file changed on disk and the user hasn't said what to do yet.
    pub stale: bool,
    _subs: Vec<Subscription>,
}

impl EventEmitter<DocEvent> for DocView {}

impl Focusable for DocView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.body {
            Body::Markdown { editor, .. } => editor.focus_handle(cx),
            Body::Text { editor, .. } => editor.focus_handle(cx),
            Body::Hex(hex) => hex.focus_handle(cx),
            Body::Settings(page) => page.focus_handle(cx),
            _ => self.focus.clone(),
        }
    }
}

/// Syntax highlighting is chosen by extension; anything unknown is plain.
fn language_for(path: Option<&Path>, markdown: bool) -> String {
    if markdown {
        return "markdown".into();
    }
    path.and_then(|p| p.extension())
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_else(|| "text".into())
}

fn image_format(mime: &str) -> Option<ImageFormat> {
    Some(match mime {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::Webp,
        "image/bmp" => ImageFormat::Bmp,
        "image/x-icon" => ImageFormat::Ico,
        "image/svg+xml" => ImageFormat::Svg,
        _ => return None,
    })
}

impl DocView {
    /// A new, empty markdown document.
    pub fn untitled(number: usize, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut doc = Self::blank(None, cx);
        doc.untitled = number;
        doc.body = doc.markdown_body(String::new(), cx);
        doc
    }

    /// A new markdown document holding `text` (an import), not yet saved.
    pub fn untitled_with(number: usize, text: String, cx: &mut Context<Self>) -> Self {
        let mut doc = Self::blank(None, cx);
        doc.untitled = number;
        doc.body = doc.markdown_body(text, cx);
        if let Body::Markdown { saved, .. } = &mut doc.body {
            saved.clear();
        }
        doc.dirty = true;
        doc
    }

    /// The Settings tab.
    pub fn settings(cx: &mut Context<Self>) -> Self {
        let mut doc = Self::blank(None, cx);
        doc.body = Body::Settings(cx.new(SettingsPage::new));
        doc
    }

    pub fn is_settings(&self) -> bool {
        matches!(self.body, Body::Settings(_))
    }

    pub fn from_file(file: AnyFile, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut doc = Self::blank(Some(file.path.clone()), cx);
        doc.body = match file.content {
            Content::Markdown(text) => doc.markdown_body(text, cx),
            Content::Text(text) => doc.text_body(text, false, window, cx),
            Content::Image { bytes, mime } => Body::Image {
                image: image_format(mime).map(|f| Arc::new(Image::from_bytes(f, bytes))),
                size: file.size,
            },
            Content::Binary(bytes) => {
                let hex = cx.new(|cx| HexView::new(bytes, cx));
                doc._subs
                    .push(cx.subscribe(&hex, |this, hex, _: &HexEvent, cx| {
                        this.dirty = hex.read(cx).is_dirty();
                        cx.emit(DocEvent::Changed);
                        cx.notify();
                    }));
                Body::Hex(hex)
            }
            Content::Oversized => Body::Oversized { size: file.size },
        };
        doc
    }

    fn blank(path: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        DocView {
            path,
            untitled: 0,
            body: Body::Oversized { size: 0 },
            dirty: false,
            words: 0,
            chars: 0,
            focus: cx.focus_handle(),
            find: None,
            stale: false,
            _subs: Vec::new(),
        }
    }

    fn markdown_body(&mut self, text: String, cx: &mut Context<Self>) -> Body {
        let prefs = AppState::prefs(cx);
        let mode = prefs.mode;
        let style = DocStyle::new(prefs, crate::theme::palette(cx));
        let saved = text.clone();
        let base_dir = self
            .path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        let editor = cx.new(|cx| {
            let mut e = MarkdownEditor::new(text, mode, style, cx);
            e.set_base_dir(base_dir, cx);
            e
        });
        self._subs
            .push(cx.subscribe(&editor, |this, editor, ev: &EditorEvent, cx| {
                match ev {
                    EditorEvent::FollowWiki { note, path } => {
                        return cx.emit(DocEvent::FollowWiki {
                            note: note.clone(),
                            path: path.clone(),
                        });
                    }
                    EditorEvent::Open(paths) => return cx.emit(DocEvent::Open(paths.clone())),
                    EditorEvent::Notice(m) => return cx.emit(DocEvent::Notice(m.clone())),
                    _ => {}
                }
                // Escape in the text closes the search too.
                if editor.read(cx).find.is_none() {
                    this.find = None;
                }
                if let EditorEvent::Edited = ev
                    && let Body::Markdown { saved, .. } = &this.body
                {
                    let e = editor.read(cx);
                    this.dirty = e.text() != saved;
                    let (w, c) = e.counts();
                    this.words = w;
                    this.chars = c;
                }
                cx.emit(DocEvent::Changed);
                cx.notify();
            }));
        let (w, c) = editor.read(cx).counts();
        self.words = w;
        self.chars = c;
        Body::Markdown { editor, saved }
    }

    fn open_find(&mut self, replacing: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.markdown().cloned() else {
            return;
        };
        let bar = match &self.find {
            Some(bar) => bar.clone(),
            None => {
                let bar = cx.new(|cx| FindBar::new(editor.clone(), window, cx));
                self._subs.push(cx.subscribe_in(
                    &bar,
                    window,
                    |this, _, _: &FindBarEvent, window, cx| {
                        this.find = None;
                        if let Some(editor) = this.markdown().cloned() {
                            editor.update(cx, |e, cx| e.close_find(cx));
                            editor.focus_handle(cx).focus(window, cx);
                        }
                        cx.notify();
                    },
                ));
                self.find = Some(bar.clone());
                bar
            }
        };
        let e = editor.read(cx);
        let sel = e.selection().range();
        let seed = (!sel.is_empty() && !e.text()[sel.clone()].contains('\n'))
            .then(|| e.text()[sel].to_string());
        bar.update(cx, |b, cx| b.focus(seed, replacing, window, cx));
        cx.notify();
    }

    fn find(&mut self, _: &Find, window: &mut Window, cx: &mut Context<Self>) {
        self.open_find(false, window, cx);
    }

    fn find_replace(&mut self, _: &FindReplace, window: &mut Window, cx: &mut Context<Self>) {
        self.open_find(true, window, cx);
    }

    /// What was last read from or written to disk, as text: the common
    /// ancestor for a merge.
    pub fn saved_text(&self) -> Option<String> {
        match &self.body {
            Body::Markdown { saved, .. } => Some(saved.clone()),
            Body::Text { saved, .. } => Some(saved.to_string()),
            _ => None,
        }
    }

    /// Keep the tab's version over the file's: it differs, so it's modified.
    pub fn mark_dirty(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        cx.emit(DocEvent::Changed);
        cx.notify();
    }

    /// The tab's current text.
    pub fn current_text(&self, cx: &App) -> Option<String> {
        match &self.body {
            Body::Markdown { editor, .. } => Some(editor.read(cx).text().to_string()),
            Body::Text { editor, .. } => Some(editor.read(cx).value().to_string()),
            _ => None,
        }
    }

    /// What saving would write.
    pub fn disk_bytes(&self, cx: &App) -> Option<Vec<u8>> {
        match &self.body {
            Body::Hex(hex) => Some(hex.read(cx).doc.bytes().to_vec()),
            _ => self.current_text(cx).map(String::into_bytes),
        }
    }

    /// Replace the tab's content with `text` and make `disk` the new saved
    /// version (both the same for a reload; a merge keeps changes).
    pub fn set_texts(
        &mut self,
        text: String,
        disk: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match &mut self.body {
            Body::Markdown { editor, saved } => {
                *saved = disk.clone();
                editor.update(cx, |e, cx| e.set_text(text.clone(), cx));
            }
            Body::Text { editor, saved, .. } => {
                *saved = disk.clone().into();
                editor.update(cx, |e, cx| e.set_value(text.clone(), window, cx));
            }
            _ => return,
        }
        self.dirty = text != disk;
        self.stale = false;
        cx.emit(DocEvent::Changed);
        cx.notify();
    }

    /// Start over from `file` (a reload of a picture or a binary).
    pub fn reload(&mut self, file: AnyFile, window: &mut Window, cx: &mut Context<Self>) {
        match file.content {
            Content::Markdown(t) | Content::Text(t) if self.saved_text().is_some() => {
                self.set_texts(t.clone(), t, window, cx);
            }
            _ => {
                let fresh = DocView::from_file(file, window, cx);
                self.body = fresh.body;
                self._subs.extend(fresh._subs);
                self.dirty = false;
                self.stale = false;
                cx.emit(DocEvent::Changed);
                cx.notify();
            }
        }
    }

    /// The question asked of a stale tab, above its content.
    fn render_banner(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, border, fg) = (
            theme.warning.opacity(0.14),
            theme.warning.opacity(0.5),
            theme.foreground,
        );
        let can_merge = self.saved_text().is_some() && self.dirty;
        let choice = |id: &'static str,
                      label: &'static str,
                      c: DiskChoice,
                      primary: bool,
                      cx: &mut Context<Self>| {
            Button::new(id)
                .xsmall()
                .label(label)
                .map(|b| if primary { b.primary() } else { b.outline() })
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(DocEvent::Disk(c))))
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .px(px(14.))
            .py(px(7.))
            .bg(bg)
            .border_b_1()
            .border_color(border)
            .text_sm()
            .text_color(fg)
            .child(div().flex_1().child(if self.dirty {
                "This file changed on disk, and you have changes too."
            } else {
                "This file changed on disk."
            }))
            .child(choice(
                "disk-reload",
                "Reload",
                DiskChoice::Reload,
                !self.dirty,
                cx,
            ))
            .when(can_merge, |d| {
                d.child(choice("disk-merge", "Merge", DiskChoice::Merge, true, cx))
            })
            .child(choice(
                "disk-keep",
                "Keep Mine",
                DiskChoice::Keep,
                false,
                cx,
            ))
    }

    /// Typography or colours changed: re-set the editor.
    pub fn restyle(&mut self, cx: &mut Context<Self>) {
        if let Body::Markdown { editor, .. } = &self.body {
            let style = DocStyle::new(AppState::prefs(cx), crate::theme::palette(cx));
            editor.update(cx, |e, cx| {
                e.set_style(style, cx);
                // Spelling may have been turned on or off.
                e.check_spelling(cx);
            });
        }
    }

    /// Where its wiki links look for notes: the project folder.
    pub fn set_root(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        if let Some(editor) = self.markdown() {
            editor.update(cx, |e, cx| e.set_root(root, cx));
        }
    }

    pub fn markdown(&self) -> Option<&Entity<MarkdownEditor>> {
        match &self.body {
            Body::Markdown { editor, .. } => Some(editor),
            _ => None,
        }
    }

    fn text_body(
        &mut self,
        text: String,
        markdown: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Body {
        let language = language_for(self.path.as_deref(), markdown);
        let plain = markdown || language == "txt" || language == "text";
        let saved: SharedString = text.into();
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(language)
                .line_number(!plain)
                .soft_wrap(plain)
                .folding(!plain)
                .default_value(saved.clone())
        });
        self._subs
            .push(cx.subscribe(&editor, |this, editor, ev: &InputEvent, cx| {
                if let InputEvent::Change = ev {
                    this.text_changed(&editor, cx);
                }
            }));
        self.count(&saved);
        Body::Text {
            editor,
            markdown,
            saved,
        }
    }

    fn text_changed(&mut self, editor: &Entity<EditorState>, cx: &mut Context<Self>) {
        let value = editor.read(cx).value();
        if let Body::Text { saved, .. } = &self.body {
            self.dirty = value != *saved;
        }
        self.count(&value);
        cx.emit(DocEvent::Changed);
        cx.notify();
    }

    fn count(&mut self, text: &str) {
        self.words = text.split_whitespace().count();
        self.chars = text.chars().count();
    }

    pub fn name(&self) -> String {
        if self.is_settings() {
            return "Settings".into();
        }
        match &self.path {
            Some(p) => project::file_name_of(p),
            None if self.untitled > 1 => format!("Untitled {}", self.untitled),
            None => "Untitled".into(),
        }
    }

    pub fn untitled_number(&self) -> usize {
        self.untitled
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Text can be saved anywhere; a binary only back where it came from.
    pub fn can_save_as(&self) -> bool {
        matches!(self.body, Body::Text { .. } | Body::Markdown { .. })
    }

    pub fn can_save(&self) -> bool {
        matches!(
            self.body,
            Body::Text { .. } | Body::Markdown { .. } | Body::Hex(_)
        )
    }

    /// Write the document to `path`, which becomes its path.
    pub fn write_to(&mut self, path: &Path, cx: &mut Context<Self>) -> Result<(), String> {
        match &mut self.body {
            Body::Markdown { editor, saved } => {
                let text = editor.read(cx).text().to_string();
                std::fs::write(path, text.as_bytes()).map_err(|e| e.to_string())?;
                *saved = text;
                // Its tags and links may have changed.
                crate::scan::invalidate(cx);
            }
            Body::Text { editor, saved, .. } => {
                let text = editor.read(cx).value();
                std::fs::write(path, text.as_bytes()).map_err(|e| e.to_string())?;
                *saved = text;
            }
            Body::Hex(hex) => {
                let bytes = hex.read(cx).doc.bytes().to_vec();
                std::fs::write(path, &bytes).map_err(|e| e.to_string())?;
                hex.update(cx, |h, _| h.mark_saved());
            }
            _ => return Err("This tab has nothing to save.".into()),
        }
        self.path = Some(project::canonical(path));
        if let Body::Markdown { editor, .. } = &self.body {
            let dir = path.parent().map(Path::to_path_buf);
            editor.update(cx, |e, cx| e.set_base_dir(dir, cx));
        }
        self.dirty = false;
        cx.emit(DocEvent::Changed);
        cx.notify();
        Ok(())
    }

    /// The status bar's facts about this document, left to right.
    pub fn status(&self, cx: &App) -> Vec<SharedString> {
        match &self.body {
            Body::Markdown { .. } => vec![
                format!("{} words", self.words).into(),
                format!("{} characters", self.chars).into(),
            ],
            Body::Text { markdown, .. } => vec![
                format!("{} words", self.words).into(),
                format!("{} characters", self.chars).into(),
                if *markdown { "Markdown" } else { "Text" }.into(),
            ],
            Body::Image { size, .. } => vec![project::human_size(*size).into()],
            Body::Hex(hex) => {
                let hex = hex.read(cx);
                vec![
                    project::human_size(hex.doc.len() as u64).into(),
                    format!("0x{:08x}", hex.cursor()).into(),
                ]
            }
            Body::Oversized { size } => vec![project::human_size(*size).into()],
            Body::Settings(_) => Vec::new(),
        }
    }

    pub fn hex(&self) -> Option<&Entity<HexView>> {
        match &self.body {
            Body::Hex(hex) => Some(hex),
            _ => None,
        }
    }
}

impl Render for DocView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let page = div().size_full().track_focus(&self.focus);
        let stale = self.stale;
        let banner = stale.then(|| self.render_banner(cx).into_any_element());
        let content = match &self.body {
            Body::Markdown { editor, .. } => page
                .key_context("DocView")
                .on_action(cx.listener(Self::find))
                .on_action(cx.listener(Self::find_replace))
                .relative()
                .child(editor.clone())
                .when_some(self.find.clone(), |d, bar| {
                    d.child(div().absolute().top(px(12.)).right(px(28.)).child(bar))
                }),
            Body::Text {
                editor,
                markdown: true,
                ..
            } => page.flex().justify_center().child(
                div().size_full().max_w(px(860.)).px(px(24.)).child(
                    Editor::new(editor)
                        .bordered(false)
                        .size_full()
                        .font_family(MONO_FONT)
                        .text_size(px(14.)),
                ),
            ),
            Body::Text { editor, .. } => page.child(
                Editor::new(editor)
                    .bordered(false)
                    .size_full()
                    .font_family(MONO_FONT)
                    .text_size(px(13.)),
            ),
            Body::Image {
                image: Some(image), ..
            } => page
                .flex()
                .items_center()
                .justify_center()
                .p(px(24.))
                .child(
                    img(image.clone())
                        .max_w_full()
                        .max_h_full()
                        .object_fit(ObjectFit::ScaleDown),
                ),
            Body::Image { image: None, .. } => page
                .flex()
                .items_center()
                .justify_center()
                .text_color(muted)
                .child("mdedit can't display this kind of image."),
            Body::Hex(hex) => page.child(hex.clone()),
            Body::Settings(settings) => page.child(settings.clone()),
            Body::Oversized { size } => {
                page.flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .font_family(UI_FONT)
                    .child(div().text_lg().child(format!(
                        "This file is too large to open ({}).",
                        project::human_size(*size)
                    )))
                    .child(div().text_color(muted).child(
                        "mdedit opens text and binaries up to 1 MB, and images up to 16 MB.",
                    ))
            }
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .children(banner)
            .child(div().flex_1().min_h_0().child(content))
    }
}
