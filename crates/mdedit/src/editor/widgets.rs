//! Blocks drawn as something other than text: mermaid diagrams, display
//! formulas and images. Each is drawn in place of its source until the
//! caret goes into it; then the source shows, a diagram or formula keeping
//! a live preview under it. Tables are grids, edited in place (`grid.rs`,
//! `table.rs`).

use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use gpui_kit::*;
use mdedit_core::mermaid::{self, Colors, DrawOptions, fontdb};
use mdedit_core::syntax::WidgetKind;
use mdedit_core::{math, os};

use super::MarkdownEditor;
use crate::theme::{FONT_FILES, MONO_FONT, UI_FONT};

/// A drawn diagram or formula, or why there isn't one.
pub(crate) type Picture = Result<(Arc<Image>, f32, f32), String>;

/// The diagram renderer's fonts: the app's own, so labels are measured and
/// outlined in the face the rest of the UI uses.
pub(crate) fn fonts() -> Arc<fontdb::Database> {
    static DB: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        for f in FONT_FILES {
            db.load_font_data(f.to_vec());
        }
        db.set_sans_serif_family(UI_FONT);
        db.set_monospace_family(MONO_FONT);
        Arc::new(db)
    })
    .clone()
}

fn hash_of(parts: &[&dyn Fn(&mut std::collections::hash_map::DefaultHasher)]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in parts {
        p(&mut h);
    }
    h.finish()
}

/// A palette's colours as a diagram's.
pub(crate) fn diagram_colors(p: &mdedit_core::themes::Palette) -> Colors {
    Colors {
        bg: p.bg,
        fg: p.text,
        muted: p.text_muted,
        line: p.text,
        surface: p.bg_soft,
        surface_alt: p.diagram_surface_alt,
        border: p.diagram_border,
        accent: p.accent,
        critical: p.danger,
        series: [
            p.accent,
            p.alert_tip,
            p.alert_warning,
            p.alert_important,
            p.alert_caution,
            p.alert_note,
            p.text_muted,
            p.diagram_border,
        ],
    }
}

impl MarkdownEditor {
    /// Where the document's relative images are looked up.
    pub fn set_base_dir(&mut self, dir: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.base_dir != dir {
            self.base_dir = dir;
            cx.notify();
        }
    }

    /// The widget (index into `syntax.widgets`) whose lines hold `pos`.
    pub(crate) fn widget_at(&self, pos: usize) -> Option<usize> {
        let line = self.syntax.line_at(pos);
        self.syntax
            .widgets
            .iter()
            .position(|w| w.lines.contains(&line))
    }

    fn picture(&mut self, body: &str, diagram: bool) -> Picture {
        let c = &self.style.colors;
        let key = hash_of(&[
            &|h| body.hash(h),
            &|h| diagram.hash(h),
            &|h| format!("{:?}{:?}", c.text, c.code_bg).hash(h),
            &|h| self.style.size.to_bits().hash(h),
        ]);
        if let Some(p) = self.pictures.get(&key) {
            return p.clone();
        }
        let p = if diagram {
            self.draw_diagram(body)
        } else {
            self.draw_math(body)
        };
        if self.pictures.len() > 200 {
            self.pictures.clear();
        }
        self.pictures.insert(key, p.clone());
        p
    }

    fn draw_diagram(&self, body: &str) -> Picture {
        let opts = DrawOptions {
            colors: diagram_colors(&self.style.palette),
            font_family: UI_FONT.into(),
            font_family_mono: MONO_FONT.into(),
            fontdb: fonts(),
        };
        let d = mermaid::draw(body, &opts)?;
        Ok((
            Arc::new(Image::from_bytes(ImageFormat::Svg, d.svg.into_bytes())),
            d.width as f32,
            d.height as f32,
        ))
    }

    fn draw_math(&self, body: &str) -> Picture {
        let p = &self.style.palette;
        // Points to pixels: CSS's 96 / 72.
        let size_pt = self.style.size * 0.75 * 1.15;
        let m = math::draw(
            body,
            true,
            size_pt as f64,
            [p.text[0], p.text[1], p.text[2]],
        )?;
        Ok((
            Arc::new(Image::from_bytes(ImageFormat::Svg, m.svg.into_bytes())),
            m.width_pt as f32 * 4. / 3.,
            m.height_pt as f32 * 4. / 3.,
        ))
    }

    /// The element widget `w` is drawn as, `width` wide at most.
    pub(crate) fn widget_element(
        &mut self,
        w: usize,
        width: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kind = self.syntax.widgets[w].kind.clone();
        let c = self.style.colors.clone();
        match kind {
            // Tables are grids (`grid.rs`); this is one that couldn't be read.
            WidgetKind::Table => div()
                .w(width)
                .py(px(6.))
                .text_color(c.muted)
                .child("This table couldn't be read.")
                .into_any_element(),
            WidgetKind::Diagram { body } | WidgetKind::Math { body } => {
                let diagram = matches!(self.syntax.widgets[w].kind, WidgetKind::Diagram { .. });
                let src = self.text[body.clone()].to_string();
                let start = body.start;
                let editor = cx.entity().downgrade();
                let open = move |_: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                    editor
                        .update(cx, |e, cx| e.reveal_offset(start, window, cx))
                        .ok();
                };
                match self.picture(&src, diagram) {
                    Ok((image, w_px, h_px)) => {
                        let scale = (f32::from(width) / w_px).min(1.0);
                        div()
                            .w(width)
                            .flex()
                            .justify_center()
                            .py(px(10.))
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, open)
                            .child(img(image).w(px(w_px * scale)).h(px(h_px * scale)))
                            .into_any_element()
                    }
                    Err(e) => div()
                        .w(width)
                        .my(px(6.))
                        .px(px(12.))
                        .py(px(8.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(c.danger)
                        .text_color(c.danger)
                        .text_sm()
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, open)
                        .child(if diagram {
                            format!("Diagram: {e}")
                        } else {
                            format!("Formula: {e}")
                        })
                        .into_any_element(),
                }
            }
            WidgetKind::Image { url, alt } => {
                let remote_waiting;
                let source: Option<ImageSource> =
                    match os::resolve_image(&url, self.base_dir.as_deref()) {
                        os::ImageRef::Local(path) => {
                            remote_waiting = false;
                            Some(path.into())
                        }
                        os::ImageRef::Remote(u) if self.allow_remote => {
                            remote_waiting = false;
                            Some(SharedUri::from(u).into())
                        }
                        os::ImageRef::Remote(_) => {
                            remote_waiting = true;
                            None
                        }
                        os::ImageRef::Refused => {
                            remote_waiting = false;
                            None
                        }
                    };
                let line = self.syntax.widgets[w].lines.start;
                let start = self.syntax.lines[line].range.start;
                let editor = cx.entity().downgrade();
                let placeholder = {
                    let c = c.clone();
                    let alt = if alt.is_empty() {
                        url.clone()
                    } else {
                        alt.clone()
                    };
                    move || {
                        div()
                            .px(px(10.))
                            .py(px(6.))
                            .rounded(px(5.))
                            .bg(c.code_bg)
                            .text_color(c.muted)
                            .text_sm()
                            .child(if remote_waiting {
                                format!("🖼 {alt} — click to load images from the web")
                            } else {
                                format!("🖼 {alt}")
                            })
                            .into_any_element()
                    }
                };
                div()
                    .w(width)
                    .py(px(8.))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        editor
                            .update(cx, |e, cx| {
                                if remote_waiting {
                                    // Asked for: this document's web images load.
                                    e.allow_remote = true;
                                    cx.notify();
                                } else {
                                    e.reveal_offset(start, window, cx);
                                }
                            })
                            .ok();
                    })
                    .child(match source {
                        Some(s) => img(s)
                            .max_w(width)
                            .max_h(px(560.))
                            .object_fit(ObjectFit::ScaleDown)
                            .with_fallback(placeholder)
                            .into_any_element(),
                        None => placeholder(),
                    })
                    .into_any_element()
            }
        }
    }
}
