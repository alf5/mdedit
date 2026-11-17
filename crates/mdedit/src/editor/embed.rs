//! `![[Note]]` and `![[Note#Part]]`: another note (or a part of it) drawn
//! in place, by a read-only editor of its own on a card. Embeds nest two
//! deep; past that, and for a note that isn't there, a card says so.

use std::path::PathBuf;
use std::time::SystemTime;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mdedit_core::prefs::Mode;

use super::{EditorEvent, MarkdownEditor};

/// How deep embeds go: a note in a note in a note is the last drawn.
const MAX_DEPTH: u8 = 2;
/// The tallest an embed's card gets; past that it scrolls.
const MAX_HEIGHT: f32 = 420.;

/// An embedded note, kept between frames.
pub(crate) struct Embedded {
    path: PathBuf,
    stamp: Option<SystemTime>,
    pub(crate) editor: Entity<MarkdownEditor>,
    _events: Subscription,
}

/// What an embed's target turned out to be.
enum Found {
    Note(Entity<MarkdownEditor>, PathBuf),
    Missing(&'static str),
}

fn stamp(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl MarkdownEditor {
    /// Whether this is an embed's editor: read-only, compact, no caret.
    pub(crate) fn is_embedded(&self) -> bool {
        self.embedded > 0
    }

    /// The note `target` names, read (again when the file has changed) into
    /// an editor of its own.
    fn embedded_note(&mut self, target: &str, cx: &mut Context<Self>) -> Found {
        if self.embedded >= MAX_DEPTH {
            return Found::Missing("Embedded too deep to show here.");
        }
        let (note, part) = mdedit_core::wiki::split(target);
        if note.is_empty() {
            return Found::Missing("A note can't show a part of itself.");
        }
        let Some(path) = self.wiki_note(note) else {
            return Found::Missing("There's no note by that name here.");
        };
        let now = stamp(&path);
        if let Some(e) = self.embeds.get(target)
            && e.path == path
            && e.stamp == now
        {
            return Found::Note(e.editor.clone(), path);
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Found::Missing("That note couldn't be read.");
        };
        let Some(shown) = mdedit_core::wiki::section(&text, part).map(str::to_string) else {
            return Found::Missing("That note has no heading by that name.");
        };
        if let Some(e) = self.embeds.get(target)
            && e.path == path
        {
            // Changed on disk: the same editor shows the new text.
            let editor = e.editor.clone();
            editor.update(cx, |e, cx| e.load(shown, cx));
            self.embeds.get_mut(target).expect("just seen").stamp = now;
            return Found::Note(editor, path);
        }
        let style = self.style.clone();
        let depth = self.embedded + 1;
        let root = self.root.clone();
        let dir = path.parent().map(PathBuf::from);
        let allow_remote = self.allow_remote;
        let editor = cx.new(|cx| {
            let mut e = MarkdownEditor::new(shown, Mode::Rendered, style, cx);
            e.embedded = depth;
            e.allow_remote = allow_remote;
            e.set_base_dir(dir, cx);
            e.set_root(root, cx);
            e
        });
        // Links followed inside it are followed from here.
        let events = cx.subscribe(&editor, |_, _, ev: &EditorEvent, cx| match ev {
            EditorEvent::FollowWiki { .. } | EditorEvent::Notice(_) => cx.emit(ev.clone()),
            // Its height may have changed: the card is laid out again.
            EditorEvent::Moved => cx.notify(),
            _ => {}
        });
        self.embeds.insert(
            target.to_string(),
            Embedded {
                path: path.clone(),
                stamp: now,
                editor: editor.clone(),
                _events: events,
            },
        );
        Found::Note(editor, path)
    }

    /// The card an embed is drawn as.
    pub(crate) fn embed_element(
        &mut self,
        target: &str,
        start: Option<usize>,
        width: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = self.style.colors.clone();
        if let Some(el) = self.pdf_embed(target, start.unwrap_or(0), width, cx) {
            return el;
        }
        let found = self.embedded_note(target, cx);
        let (note, _) = mdedit_core::wiki::split(target);
        let title: SharedString = target.to_string().into();
        let this = cx.entity().downgrade();
        // The title shows the embed's own source (a preview has none).
        let reveal = {
            let this = this.clone();
            move |_: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                if let Some(start) = start {
                    this.update(cx, |e, cx| e.reveal_offset(start, window, cx))
                        .ok();
                }
            }
        };
        let header = |open: Option<PathBuf>| {
            let note = note.to_string();
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(12.))
                .py(px(6.))
                .border_b_1()
                .border_color(c.border.opacity(0.6))
                .text_xs()
                .text_color(c.muted)
                .child(
                    div()
                        .id("embed-title")
                        .flex_1()
                        .truncate()
                        .cursor_pointer()
                        .child(title.clone())
                        .on_mouse_down(MouseButton::Left, reveal.clone()),
                )
                .when_some(open, |d, path| {
                    let this = this.clone();
                    d.child(
                        div()
                            .id("embed-open")
                            .px(px(6.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(|s| s.bg(c.border.opacity(0.6)))
                            .child("Open ↗")
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                cx.stop_propagation();
                                this.update(cx, |_, cx| {
                                    cx.emit(EditorEvent::FollowWiki {
                                        note: note.clone(),
                                        path: Some(path.clone()),
                                    })
                                })
                                .ok();
                            }),
                    )
                })
        };
        let card = div()
            .w(width)
            .my(px(6.))
            .flex()
            .flex_col()
            .rounded(px(10.))
            .border_1()
            .border_color(c.border)
            .border_l_4()
            .bg(c.code_bg.opacity(0.5))
            .overflow_hidden();
        match found {
            Found::Note(editor, path) => {
                let h = editor.read(cx).content_height().min(px(MAX_HEIGHT));
                card.child(header(Some(path)))
                    .child(div().w_full().h(h).child(editor))
                    .into_any_element()
            }
            Found::Missing(why) => card
                .child(header(None))
                .child(
                    div()
                        .px(px(12.))
                        .py(px(8.))
                        .text_sm()
                        .text_color(c.muted)
                        .child(why),
                )
                .into_any_element(),
        }
    }
}

impl MarkdownEditor {
    /// `![[file.pdf]]` or `![[file.pdf#page=3]]`: that page, drawn, on a
    /// card with the way to open the whole file. `None` for anything else.
    fn pdf_embed(
        &mut self,
        target: &str,
        start: usize,
        width: Pixels,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (file, part) = mdedit_core::wiki::split(target);
        if !file.to_ascii_lowercase().ends_with(".pdf") {
            return None;
        }
        let page = part
            .and_then(|p| p.strip_prefix("page="))
            .and_then(|n| n.trim().parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(1);
        let c = self.style.colors.clone();
        let path = self.wiki_note(file);
        let shown = match &path {
            Some(p) => self.pdf_page(p, page, width - px(2.), 2., cx),
            None => PdfPage::Failed("There's no PDF by that name here.".into()),
        };
        let this = cx.entity().downgrade();
        let label: SharedString = match &shown {
            PdfPage::Ready { pages, .. } => format!("{file} · page {page} of {pages}"),
            _ => file.to_string(),
        }
        .into();
        let body = match shown {
            PdfPage::Ready { image, aspect, .. } => img(image)
                .w(width - px(2.))
                .h((width - px(2.)) * aspect)
                .into_any_element(),
            PdfPage::Rendering => div()
                .h(px(120.))
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(c.muted)
                .child("Drawing the page…")
                .into_any_element(),
            PdfPage::Failed(why) => div()
                .px(px(12.))
                .py(px(8.))
                .text_sm()
                .text_color(c.muted)
                .child(why)
                .into_any_element(),
        };
        Some(
            div()
                .w(width)
                .my(px(6.))
                .flex()
                .flex_col()
                .rounded(px(10.))
                .border_1()
                .border_color(c.border)
                .bg(c.code_bg.opacity(0.5))
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(12.))
                        .py(px(6.))
                        .border_b_1()
                        .border_color(c.border.opacity(0.6))
                        .text_xs()
                        .text_color(c.muted)
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .cursor_pointer()
                                .child(label)
                                .on_mouse_down(MouseButton::Left, {
                                    let this = this.clone();
                                    move |_, window, cx| {
                                        this.update(cx, |e, cx| e.reveal_offset(start, window, cx))
                                            .ok();
                                    }
                                }),
                        )
                        .when_some(path, |d, path| {
                            d.child(
                                div()
                                    .id("pdf-open")
                                    .px(px(6.))
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(c.border.opacity(0.6)))
                                    .child("Open ↗")
                                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                        cx.stop_propagation();
                                        cx.open_with_system(&path);
                                    }),
                            )
                        }),
                )
                .child(body)
                .into_any_element(),
        )
    }
}

impl MarkdownEditor {
    /// The preview of a wiki link's note, below the pointer.
    pub(crate) fn render_preview(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (target, at) = self.preview.clone()?;
        if !self.link_hover {
            return None;
        }
        let card = self.embed_element(&target, None, px(440.), cx);
        Some(
            deferred(
                anchored()
                    .position(at + point(px(-24.), px(18.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(div().occlude().shadow_lg().rounded(px(10.)).child(card)),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}

/// A PDF page drawn for an embed, or why not.
#[derive(Clone)]
pub(crate) enum PdfPage {
    Rendering,
    Ready {
        image: std::sync::Arc<Image>,
        aspect: f32,
        pages: usize,
    },
    Failed(String),
}

/// Page `page` (from 1) of the PDF `bytes`, as a PNG `width` pixels wide;
/// with how tall per wide it is and how many pages there are.
fn render_pdf_page(
    bytes: Vec<u8>,
    page: usize,
    width: f32,
) -> Result<(Vec<u8>, f32, usize), String> {
    use hayro::hayro_interpret::InterpreterSettings;
    use hayro::hayro_syntax::Pdf;
    use hayro::vello_cpu::color::palette::css::WHITE;
    use hayro::{PixmapSettings, RenderCache, RenderSettings, render};

    let pdf = Pdf::new(bytes).map_err(|e| format!("This PDF couldn't be read ({e:?})."))?;
    let pages = pdf.pages();
    let count = pages.len();
    let Some(p) = pages.get(page.saturating_sub(1)) else {
        return Err(format!(
            "This PDF has {count} pages; there's no page {page}."
        ));
    };
    let (w, h) = p.render_dimensions();
    let scale = (width / w.max(1.)).clamp(0.1, 4.);
    let pixmap = render(
        p,
        &RenderCache::new(),
        &InterpreterSettings::default(),
        &RenderSettings::default(),
        &PixmapSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: WHITE,
        },
    );
    let png = pixmap.into_png().map_err(|e| e.to_string())?;
    Ok((png, h / w.max(1.), count))
}

impl MarkdownEditor {
    /// The page a `![[file.pdf#page=N]]` shows, drawn in the background the
    /// first time it's asked for at this width.
    fn pdf_page(
        &mut self,
        path: &std::path::Path,
        page: usize,
        width: Pixels,
        window_scale: f32,
        cx: &mut Context<Self>,
    ) -> PdfPage {
        // Drawn again only when the width changes by a fair amount.
        let px_wide = ((f32::from(width) * window_scale) / 64.).ceil() as u32 * 64;
        let key = (path.to_path_buf(), page, px_wide, stamp(path));
        if let Some(found) = self.pdf_pages.get(&key) {
            return found.clone();
        }
        self.pdf_pages.insert(key.clone(), PdfPage::Rendering);
        let path = path.to_path_buf();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                    render_pdf_page(bytes, page, px_wide as f32)
                })
                .await;
            this.update(cx, |e, cx| {
                let done = match result {
                    Ok((png, aspect, pages)) => PdfPage::Ready {
                        image: std::sync::Arc::new(Image::from_bytes(ImageFormat::Png, png)),
                        aspect,
                        pages,
                    },
                    Err(why) => PdfPage::Failed(why),
                };
                e.pdf_pages.insert(key, done);
                cx.notify();
            })
            .ok();
        })
        .detach();
        PdfPage::Rendering
    }
}

#[cfg(all(test, feature = "pdf"))]
mod tests {
    #[test]
    fn a_pdf_page_draws() {
        let opts = mdedit_pdf::Options {
            style: mdedit_core::prefs::Style::GitHub,
            numbered: false,
            letter: false,
            fonts: crate::theme::FONT_FILES.to_vec(),
            base_dir: None,
            diagrams: None,
        };
        let pdf = mdedit_pdf::markdown_to_pdf("# One\n\nHello.\n", "t", &opts).unwrap();
        let (png, aspect, pages) = super::render_pdf_page(pdf.clone(), 1, 300.).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        assert_eq!(pages, 1);
        assert!(aspect > 1.2 && aspect < 1.5, "a portrait page: {aspect}");
        assert!(
            super::render_pdf_page(pdf, 2, 300.)
                .unwrap_err()
                .contains("no page 2")
        );
        assert!(super::render_pdf_page(b"nope".to_vec(), 1, 300.).is_err());
    }
}
