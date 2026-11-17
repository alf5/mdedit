//! Spell checking as you write: Harper (`mdedit_core::spell`) runs in the
//! background a moment after the text stops changing; misspellings get a
//! wavy red line, grammar a blue one, and a right-click offers fixes.

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;
use mdedit_core::buffer::{Change, Edit, Selection};
use mdedit_core::history::Kind;
use mdedit_core::spell::{self, Issue};

use super::MarkdownEditor;
use crate::state::AppState;

/// How long typing pauses before the text is checked.
const PAUSE: std::time::Duration = std::time::Duration::from_millis(400);

impl MarkdownEditor {
    /// Check the text again soon, if spelling is on; clear the lines if not.
    pub(crate) fn check_spelling(&mut self, cx: &mut Context<Self>) {
        let Some(prefs) = AppState::try_prefs(cx) else {
            return;
        };
        if !prefs.spell_check || self.is_embedded() {
            self.spell_task = None;
            if !self.issues.is_empty() {
                self.issues.clear();
                cx.notify();
            }
            return;
        }
        let (grammar, dialect) = (prefs.grammar, spell::dialect(&prefs.dialect));
        let version = self.version;
        let text = self.text.clone();
        let syntax = self.syntax.clone();
        self.spell_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PAUSE).await;
            let issues = cx
                .background_executor()
                .spawn(async move {
                    let known = spell::known_words();
                    spell::check(&text, &syntax, grammar, dialect, &known)
                })
                .await;
            this.update(cx, |e, cx| {
                if e.version == version {
                    e.issues = issues;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Keep the underlines with their words through `edits`; one an edit
    /// touches goes until the next check.
    pub(crate) fn carry_issues(&mut self, edits: &[Edit]) {
        use mdedit_core::buffer::map_pos;
        self.issues.retain_mut(|i| {
            let touched = edits
                .iter()
                .any(|e| e.range.start <= i.range.end && i.range.start <= e.range.end);
            if !touched {
                i.range = map_pos(i.range.start, edits, false)..map_pos(i.range.end, edits, false);
            }
            !touched
        });
    }

    /// The issues to underline: those not ignored here.
    pub(crate) fn shown_issues(&self) -> impl Iterator<Item = &Issue> {
        self.issues
            .iter()
            .filter(|i| !self.ignored_words.contains(&self.text[i.range.clone()]))
    }

    /// The wavy lines under the visible rows' issues, and their colours.
    pub(crate) fn squiggles(
        &self,
        lines: &[super::Placed],
        content_w: Pixels,
    ) -> Vec<(Bounds<Pixels>, Hsla)> {
        let mut out = Vec::new();
        if self.issues.is_empty() {
            return out;
        }
        for (ix, origin, l) in lines {
            let src = self.row_range(*ix);
            let first = self.issues.partition_point(|i| i.range.end < src.start);
            for issue in self.issues[first..].iter() {
                if issue.range.start > src.end {
                    break;
                }
                if self.ignored_words.contains(&self.text[issue.range.clone()]) {
                    continue;
                }
                let a = l.to_display(issue.range.start.max(src.start));
                let b = l.to_display(issue.range.end.min(src.end));
                let color = if issue.spelling {
                    self.style.colors.danger
                } else {
                    self.style.colors.accent
                };
                for r in l.rects(*origin, a, b, false, content_w - l.indent) {
                    out.push((r, color));
                }
            }
        }
        out
    }

    /// A right-click on an underlined word: its fixes, and ways to stop
    /// minding it.
    pub(crate) fn spell_menu_at(
        &mut self,
        p: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((_, d, layout)) = self.hit(p) else {
            return false;
        };
        let pos = layout.to_source(d);
        let Some(issue) = self
            .shown_issues()
            .find(|i| i.range.start <= pos && pos <= i.range.end)
            .cloned()
        else {
            return false;
        };
        let word = self.text[issue.range.clone()].to_string();
        let editor = cx.entity().downgrade();
        let menu = PopupMenu::build(window, cx, move |mut menu, _, _| {
            if issue.suggestions.is_empty() {
                menu = menu.item(PopupMenuItem::new("No suggestions").disabled(true));
            }
            for s in &issue.suggestions {
                let (editor, range, s) = (editor.clone(), issue.range.clone(), s.clone());
                let label = if s.is_empty() {
                    "Remove it".to_string()
                } else {
                    s.clone()
                };
                menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                    editor
                        .update(cx, |e, cx| {
                            let at = range.start + s.len();
                            e.apply(
                                Change {
                                    edits: vec![Edit::replace(range.clone(), s.clone())],
                                    selection: Selection::caret(at),
                                },
                                Kind::Other,
                                cx,
                            );
                        })
                        .ok();
                }));
            }
            if issue.spelling {
                let (e1, e2, w1, w2) = (editor.clone(), editor.clone(), word.clone(), word.clone());
                menu = menu
                    .separator()
                    .item(
                        PopupMenuItem::new(format!("Add “{word}” to the Dictionary")).on_click(
                            move |_, _, cx| {
                                if spell::learn(&w1).is_ok() {
                                    e1.update(cx, |e, cx| {
                                        e.issues.retain(|i| e.text[i.range.clone()] != *w1);
                                        cx.notify();
                                    })
                                    .ok();
                                }
                            },
                        ),
                    )
                    .item(PopupMenuItem::new("Ignore in This Document").on_click(
                        move |_, _, cx| {
                            e2.update(cx, |e, cx| {
                                e.ignored_words.insert(w2.clone());
                                cx.notify();
                            })
                            .ok();
                        },
                    ));
            }
            menu
        });
        menu.focus_handle(cx).focus(window, cx);
        let sub = cx.subscribe_in(&menu, window, |this, _, _: &DismissEvent, window, cx| {
            this.menu = None;
            this.focus.focus(window, cx);
            cx.notify();
        });
        self.menu = Some((menu, p, sub));
        cx.notify();
        true
    }
}
