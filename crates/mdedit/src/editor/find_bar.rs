//! Find and replace: a small panel floating at the document's top right.
//!
//! Matches are highlighted as you type; Enter and Shift+Enter (or F3 and
//! Shift+F3) step through them, wrapping around; Escape closes the panel
//! and returns to the text, leaving the last match selected.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, IconName, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::MarkdownEditor;
use crate::actions::{CloseFind, FindNext, FindPrevious};

pub enum FindBarEvent {
    Closed,
}

pub struct FindBar {
    editor: Entity<MarkdownEditor>,
    query: Entity<InputState>,
    replace: Entity<InputState>,
    case: bool,
    pub replacing: bool,
    _subs: Vec<Subscription>,
}

impl EventEmitter<FindBarEvent> for FindBar {}

impl FindBar {
    pub fn new(
        editor: Entity<MarkdownEditor>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Find"));
        let replace = cx.new(|cx| InputState::new(window, cx).placeholder("Replace with"));
        let subs = vec![
            cx.subscribe_in(
                &query,
                window,
                |this, _, ev: &InputEvent, window, cx| match ev {
                    InputEvent::Change => this.search(cx),
                    InputEvent::PressEnter { shift, .. } => this.step(!shift, window, cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(&replace, window, |this, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.replace_one(window, cx);
                }
            }),
            cx.observe(&editor, |_, _, cx| cx.notify()),
        ];
        FindBar {
            editor,
            query,
            replace,
            case: false,
            replacing: false,
            _subs: subs,
        }
    }

    /// Show the panel's query field, seeded with `seed` (the selection) when
    /// there is one.
    pub fn focus(
        &mut self,
        seed: Option<String>,
        replacing: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replacing = replacing;
        if let Some(seed) = seed {
            self.query.update(cx, |q, cx| q.set_value(seed, window, cx));
        }
        self.query.update(cx, |q, cx| q.focus(window, cx));
        self.search(cx);
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        let q = self.query.read(cx).value().to_string();
        let case = self.case;
        self.editor.update(cx, |e, cx| e.set_find(q, case, cx));
        cx.notify();
    }

    fn step(&mut self, forward: bool, _: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |e, cx| e.find_step(forward, cx));
    }

    fn replace_one(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let with = self.replace.read(cx).value().to_string();
        self.editor.update(cx, |e, cx| e.replace_current(&with, cx));
    }

    fn replace_all(&mut self, cx: &mut Context<Self>) {
        let with = self.replace.read(cx).value().to_string();
        self.editor.update(cx, |e, cx| e.replace_all(&with, cx));
    }

    fn close(&mut self, _: &CloseFind, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(FindBarEvent::Closed);
    }

    fn next(&mut self, _: &FindNext, w: &mut Window, cx: &mut Context<Self>) {
        self.step(true, w, cx);
    }

    fn prev(&mut self, _: &FindPrevious, w: &mut Window, cx: &mut Context<Self>) {
        self.step(false, w, cx);
    }

    fn count(&self, cx: &App) -> String {
        match &self.editor.read(cx).find {
            Some(f) if !f.query.is_empty() => match (f.current, f.matches.len()) {
                (_, 0) => "No results".into(),
                (Some(i), n) => format!("{} of {n}", i + 1),
                (None, n) => format!("{n} found"),
            },
            _ => String::new(),
        }
    }
}

impl Render for FindBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, border, muted) = (theme.popover, theme.border, theme.muted_foreground);
        let count = self.count(cx);
        let case = self.case;
        div()
            .key_context("FindBar")
            .on_action(cx.listener(Self::close))
            .on_action(cx.listener(Self::next))
            .on_action(cx.listener(Self::prev))
            .w(px(380.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(8.))
            .rounded(px(10.))
            .bg(bg)
            .border_1()
            .border_color(border)
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        div().flex_1().child(
                            Input::new(&self.query)
                                .small()
                                .suffix(div().text_xs().text_color(muted).child(count)),
                        ),
                    )
                    .child(
                        Button::new("find-case")
                            .xsmall()
                            .label("Aa")
                            .tooltip("Match case")
                            .map(|b| if case { b.outline() } else { b.ghost() })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.case = !this.case;
                                this.search(cx);
                            })),
                    )
                    .child(
                        Button::new("find-prev")
                            .xsmall()
                            .ghost()
                            .icon(IconName::ChevronUp)
                            .tooltip("Previous match (Shift+Enter)")
                            .on_click(cx.listener(|this, _, w, cx| this.step(false, w, cx))),
                    )
                    .child(
                        Button::new("find-next")
                            .xsmall()
                            .ghost()
                            .icon(IconName::ChevronDown)
                            .tooltip("Next match (Enter)")
                            .on_click(cx.listener(|this, _, w, cx| this.step(true, w, cx))),
                    )
                    .child(
                        Button::new("find-replace-toggle")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Replace)
                            .tooltip("Replace")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.replacing = !this.replacing;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("find-close")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Close (Esc)")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(FindBarEvent::Closed))),
                    ),
            )
            .when(self.replacing, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(div().flex_1().child(Input::new(&self.replace).small()))
                        .child(
                            Button::new("replace-one")
                                .xsmall()
                                .outline()
                                .label("Replace")
                                .on_click(cx.listener(|this, _, w, cx| this.replace_one(w, cx))),
                        )
                        .child(
                            Button::new("replace-all")
                                .xsmall()
                                .outline()
                                .label("All")
                                .on_click(cx.listener(|this, _, _, cx| this.replace_all(cx))),
                        ),
                )
            })
    }
}
