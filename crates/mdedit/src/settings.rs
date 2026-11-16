//! Settings, as a tab: the theme gallery, the document's typography, and
//! how editing behaves. Every change applies at once and is saved.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mdedit_core::prefs::{Mode, Prefs, Style};
use mdedit_core::themes::{self, Palette};

use crate::actions::{PrefsChanged, SetTheme};
use crate::editor::style::hsla;
use crate::state::AppState;
use crate::theme::{MONO_FONT, SERIF_FONT, UI_FONT};

pub struct SettingsPage {
    focus: FocusHandle,
}

impl Focusable for SettingsPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Change a setting, then let the workspace re-style everything.
fn change(window: &mut Window, cx: &mut App, f: impl FnOnce(&mut Prefs)) {
    AppState::update_prefs(cx, f);
    window.dispatch_action(Box::new(PrefsChanged), cx);
}

impl SettingsPage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Settings change from here, the menus and the palette alike.
        cx.observe_global::<AppState>(|_, cx| cx.notify()).detach();
        SettingsPage {
            focus: cx.focus_handle(),
        }
    }

    fn section(title: &'static str, hint: &'static str, cx: &App) -> Div {
        let muted = cx.theme().muted_foreground;
        div().flex().flex_col().gap(px(12.)).pb(px(28.)).child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(div().text_sm().text_color(muted).child(hint)),
        )
    }

    fn row(label: &'static str, control: impl IntoElement) -> Div {
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(16.))
            .py(px(4.))
            .child(div().child(label))
            .child(control)
    }

    fn swatch(
        id: SharedString,
        label: String,
        p: Palette,
        selected: bool,
        cx: &App,
    ) -> impl IntoElement {
        let accent = cx.theme().primary;
        let muted = cx.theme().muted_foreground;
        let action_id = id.clone();
        div()
            .id(SharedString::from(format!("theme-{id}")))
            .w(px(132.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .cursor_pointer()
            .child(
                div()
                    .h(px(70.))
                    .rounded(px(8.))
                    .bg(hsla(p.bg))
                    .border_2()
                    .border_color(if selected { accent } else { hsla(p.border) })
                    .p(px(10.))
                    .flex()
                    .flex_col()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_color(hsla(p.text))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Aa"),
                            )
                            .child(div().size(px(10.)).rounded_full().bg(hsla(p.accent))),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(4.))
                            .child(
                                div()
                                    .h(px(4.))
                                    .w(px(40.))
                                    .rounded(px(2.))
                                    .bg(hsla(p.text_muted)),
                            )
                            .child(
                                div()
                                    .h(px(4.))
                                    .w(px(22.))
                                    .rounded(px(2.))
                                    .bg(hsla(p.code_bg)),
                            ),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(if selected {
                        cx.theme().foreground
                    } else {
                        muted
                    })
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(label),
            )
            .on_click(move |_, window, cx| {
                window.dispatch_action(
                    Box::new(SetTheme {
                        id: action_id.clone(),
                    }),
                    cx,
                )
            })
            .test_support()
    }

    fn style_card(style: Style, selected: bool, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        let (border, accent, bg) = (theme.border, theme.primary, theme.background);
        let (font, size): (&str, f32) = match style {
            Style::Manuscript => (SERIF_FONT, 16.),
            Style::Typewriter => (MONO_FONT, 13.),
            Style::Compact => (UI_FONT, 13.),
            _ => (UI_FONT, 14.),
        };
        div()
            .id(SharedString::from(format!("style-{}", style.label())))
            .flex_1()
            .min_w(px(136.))
            .p(px(12.))
            .rounded(px(8.))
            .bg(bg)
            .border_2()
            .border_color(if selected { accent } else { border })
            .cursor_pointer()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(style.label()),
            )
            .child(
                div()
                    .font_family(font)
                    .text_size(px(size))
                    .line_height(relative(1.4))
                    .when(style == Style::Ogma, |d| {
                        d.text_color(cx.theme().muted_foreground)
                    })
                    .child("The quick brown fox jumps over the lazy dog."),
            )
            .on_click(move |_, window, cx| change(window, cx, |p| p.style = style))
            .test_support()
    }
}

impl Render for SettingsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prefs = AppState::prefs(cx).clone();
        let current = prefs.theme().to_string();
        let muted = cx.theme().muted_foreground;

        let mut gallery = div().flex().flex_wrap().gap(px(14.));
        gallery = gallery.child(Self::swatch(
            "".into(),
            "Match System".into(),
            themes::system_palette(cx.theme().mode.is_dark()),
            current.is_empty(),
            cx,
        ));
        for (family, items) in themes::FAMILIES {
            for (id, label) in *items {
                if let Some(p) = themes::palette(id) {
                    gallery = gallery.child(Self::swatch(
                        (*id).into(),
                        format!("{family} · {label}"),
                        p,
                        current == *id,
                        cx,
                    ));
                }
            }
        }

        let size = prefs.font_size;
        let mode = prefs.mode;
        let column = prefs.reading_width.is_some();
        div()
            .id("settings")
            .track_focus(&self.focus)
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .max_w(px(920.))
                    .mx_auto()
                    .px(px(32.))
                    .py(px(28.))
                    .flex()
                    .flex_col()
                    .child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).pb(px(24.)).child("Settings"))
                    .child(
                        Self::section("Theme", "The colours of the window and of every document.", cx)
                            .child(gallery),
                    )
                    .child(
                        Self::section(
                            "Typography",
                            "How documents are set. HTML and PDF exports follow these too.",
                            cx,
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(px(12.))
                                .children(Style::ALL.map(|s| Self::style_card(s, prefs.style == s, cx))),
                        )
                        .child(Self::row(
                            "Text size",
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(
                                    Button::new("size-down")
                                        .small()
                                        .outline()
                                        .label("−")
                                        .on_click(move |_, w, cx| {
                                            change(w, cx, |p| p.font_size = (p.font_size - 1.).max(11.))
                                        }),
                                )
                                .child(div().w(px(48.)).flex().justify_center().child(format!("{size:.0} px")))
                                .child(
                                    Button::new("size-up")
                                        .small()
                                        .outline()
                                        .label("+")
                                        .on_click(move |_, w, cx| {
                                            change(w, cx, |p| p.font_size = (p.font_size + 1.).min(26.))
                                        }),
                                ),
                        ))
                        .child(Self::row(
                            "Width",
                            div()
                                .flex()
                                .gap(px(4.))
                                .child(
                                    Button::new("width-column")
                                        .small()
                                        .label("Reading column")
                                        .map(|b| if column { b.primary() } else { b.outline() })
                                        .on_click(|_, w, cx| change(w, cx, |p| p.reading_width = Some(760.))),
                                )
                                .child(
                                    Button::new("width-full")
                                        .small()
                                        .label("Full width")
                                        .map(|b| if column { b.outline() } else { b.primary() })
                                        .on_click(|_, w, cx| change(w, cx, |p| p.reading_width = None)),
                                ),
                        ))
                        .child(Self::row(
                            "Number headings like an outline (1., 1.1., …)",
                            Switch::new("numbered")
                                .checked(prefs.numbered)
                                .on_click(|on, w, cx| {
                                    let on = *on;
                                    change(w, cx, |p| p.numbered = on)
                                }),
                        )),
                    )
                    .child(
                        Self::section("Editing", "How markdown is shown while you write.", cx)
                            .child(Self::row(
                                "New documents open in",
                                div().flex().gap(px(4.)).children(
                                    [(Mode::Rendered, "Rendered"), (Mode::Live, "Live"), (Mode::Source, "Source")].map(
                                        |(m, label)| {
                                            Button::new(label)
                                                .small()
                                                .label(label)
                                                .map(|b| if m == mode { b.primary() } else { b.outline() })
                                                .on_click(move |_, w, cx| change(w, cx, |p| p.mode = m))
                                        },
                                    ),
                                ),
                            ))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(muted)
                                    .child("Rendered hides the markdown; Live shows it on the line you're on; Source shows it all."),
                            )
                            .child(Self::row(
                                "Load images from the web without asking",
                                Switch::new("remote-images")
                                    .checked(prefs.remote_images)
                                    .on_click(|on, w, cx| {
                                        let on = *on;
                                        change(w, cx, |p| p.remote_images = on)
                                    }),
                            )),
                    )
                    .child(
                        Self::section("About", "", cx).child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .child(format!("mdedit {} · MIT licence", env!("CARGO_PKG_VERSION")))
                                .child("Fonts: Inter, Source Serif 4 and JetBrains Mono, under the SIL Open Font Licence.")
                                .child("Diagrams by bezel-mermaid (a port of Pele), formulas by latex-rust, PDFs by Typst."),
                        ),
                    ),
            )
    }
}
