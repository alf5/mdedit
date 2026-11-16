//! What floats over the text: the formatting toolbar over a selection, and
//! the `/` menu for inserting a block.

use gpui_kit::component::ActiveTheme;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mdedit_core::buffer::{Change, Edit, Selection};
use mdedit_core::edit::{self, Block};
use mdedit_core::history::Kind;

use super::MarkdownEditor;
use crate::actions::*;

/// One entry of the `/` menu.
#[derive(Clone, Copy)]
pub(crate) enum Slash {
    Heading(u8),
    List(Block),
    Insert(&'static str, usize),
}

pub(crate) const SLASH_ITEMS: &[(&str, &str, Slash)] = &[
    ("Heading 1", "Big section heading", Slash::Heading(1)),
    ("Heading 2", "Medium section heading", Slash::Heading(2)),
    ("Heading 3", "Small section heading", Slash::Heading(3)),
    ("Bullet list", "A simple list", Slash::List(Block::Bullet)),
    (
        "Numbered list",
        "A list with numbers",
        Slash::List(Block::Numbered),
    ),
    ("Task list", "Things to tick off", Slash::List(Block::Task)),
    ("Quote", "A quotation", Slash::List(Block::Quote)),
    (
        "Code block",
        "Code, in a fence",
        Slash::Insert(edit::CODE_MD, 4),
    ),
    (
        "Table",
        "Rows and columns",
        Slash::Insert(edit::TABLE_MD, 2),
    ),
    (
        "Diagram",
        "A Mermaid flowchart",
        Slash::Insert(edit::MERMAID_MD, 11),
    ),
    ("Math", "A display formula", Slash::Insert(edit::MATH_MD, 3)),
    (
        "Divider",
        "A horizontal rule",
        Slash::Insert(edit::RULE_MD, 4),
    ),
];

/// Whether `query`'s characters appear in `label` in order: "h2" finds
/// "heading 2", "nl" finds "numbered list".
fn fuzzy(query: &str, label: &str) -> bool {
    let mut chars = label.chars();
    query.chars().all(|q| chars.any(|c| c == q))
}

impl MarkdownEditor {
    /// The `/` menu's entries matching what's been typed after the slash.
    pub(crate) fn slash_matches(&self) -> Vec<usize> {
        let Some(at) = self.slash else {
            return Vec::new();
        };
        let q = self.text[at + 1..self.sel.head.max(at + 1)].to_lowercase();
        SLASH_ITEMS
            .iter()
            .enumerate()
            .filter(|(_, (label, hint, _))| {
                q.is_empty() || fuzzy(&q, &label.to_lowercase()) || hint.to_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// After every edit or move: open the menu on a freshly typed `/` that
    /// starts a line, keep it while a word follows it, close it otherwise.
    pub(crate) fn update_slash(&mut self, typed: Option<&str>) {
        if !self.rich() || !self.sel.is_empty() {
            self.slash = None;
            return;
        }
        let caret = self.sel.head;
        if typed == Some("/") && caret > 0 {
            let line_start = mdedit_core::buffer::line_start(&self.text, caret);
            let (indent, _, len) = edit::line_prefix(&self.text[line_start..]);
            if self.text[line_start + indent + len..caret] == *"/" {
                self.slash = Some(caret - 1);
                self.slash_ix = 0;
                return;
            }
        }
        if let Some(at) = self.slash {
            let ok = caret > at
                && self.text.get(at..at + 1) == Some("/")
                && !self.text[at + 1..caret].contains(char::is_whitespace)
                && caret - at <= 24
                && !self.slash_matches().is_empty();
            if !ok {
                self.slash = None;
            } else {
                let n = self.slash_matches().len();
                self.slash_ix = self.slash_ix.min(n.saturating_sub(1));
            }
        }
    }

    /// Replace `/query` with the chosen block.
    pub(crate) fn apply_slash(&mut self, item: usize, cx: &mut Context<Self>) {
        let Some(at) = self.slash.take() else { return };
        let (_, _, what) = SLASH_ITEMS[item];
        let remove = Change {
            edits: vec![Edit::delete(at..self.sel.head)],
            selection: Selection::caret(at),
        };
        self.apply(remove, Kind::Other, cx);
        let change = match what {
            Slash::Heading(level) => edit::set_heading(&self.text, self.sel, level),
            Slash::List(block) => edit::toggle_block(&self.text, self.sel, block),
            Slash::Insert(block, caret) => edit::insert_block(&self.text, self.sel, block, caret),
        };
        self.apply(change, Kind::Other, cx);
        self.slash = None;
    }

    /// Where `pos` is on screen, from last frame: the top-left of its caret,
    /// and the line height there.
    pub(crate) fn screen_point(&self, pos: usize) -> Option<(Point<Pixels>, Pixels)> {
        let ix = self.row_at(pos);
        let (_, origin, l) = self.geometry.lines.iter().find(|(i, ..)| *i == ix)?;
        Some((*origin + l.point(l.display.to_display(pos)), l.line_height))
    }

    pub(crate) fn render_overlays(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.focus.is_focused(window) || self.selecting.is_some() || !self.rich() {
            return None;
        }
        if self.slash.is_some() {
            return self.render_slash(cx);
        }
        if !self.sel.is_empty() {
            return self.render_toolbar(cx);
        }
        None
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (start, _) = self.screen_point(self.sel.start())?;
        let (end, lh) = self.screen_point(self.sel.end())?;
        let theme = cx.theme();
        let (bg, border, fg, hover) = (
            theme.popover,
            theme.border,
            theme.popover_foreground,
            theme.accent,
        );
        let top = self.geometry.bounds.top();
        // Above the selection, or below it when there's no room.
        let pos = if start.y - px(44.) > top {
            point(start.x, start.y - px(40.))
        } else {
            point(start.x, end.y + lh + px(6.))
        };
        let focus = self.focus.clone();
        let button = move |id: &'static str,
                           label: &'static str,
                           tip: &'static str,
                           action: Box<dyn Action>,
                           style: fn(Div) -> Div| {
            let focus = focus.clone();
            let shortcut = action.boxed_clone();
            style(
                div()
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child(label),
            )
            .id(id)
            .tooltip(move |window, cx| {
                Tooltip::new(tip)
                    .action(&*shortcut, Some("MdEditor"))
                    .build(window, cx)
            })
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();
                window.prevent_default();
                focus.dispatch_action(&*action, window, cx);
            })
        };
        let sep = || div().w(px(1.)).h(px(16.)).mx(px(3.)).bg(border);
        Some(
            deferred(
                anchored().position(pos).child(
                    div()
                        .occlude()
                        .flex()
                        .items_center()
                        .gap(px(1.))
                        .p(px(3.))
                        .rounded(px(8.))
                        .bg(bg)
                        .border_1()
                        .border_color(border)
                        .shadow_md()
                        .text_sm()
                        .text_color(fg)
                        .child(button("tb-bold", "B", "Bold", Box::new(Bold), |d| {
                            d.font_weight(FontWeight::BOLD)
                        }))
                        .child(button("tb-italic", "I", "Italic", Box::new(Italic), |d| {
                            d.italic()
                        }))
                        .child(button(
                            "tb-strike",
                            "S",
                            "Strikethrough",
                            Box::new(Strikethrough),
                            |d| d.line_through(),
                        ))
                        .child(button(
                            "tb-code",
                            "</>",
                            "Inline Code",
                            Box::new(InlineCode),
                            |d| d.text_xs(),
                        ))
                        .child(button(
                            "tb-link",
                            "Link",
                            "Link",
                            Box::new(InsertLink),
                            |d| d,
                        ))
                        .child(sep())
                        .child(button(
                            "tb-h1",
                            "H1",
                            "Heading 1",
                            Box::new(Heading { level: 1 }),
                            |d| d.font_weight(FontWeight::SEMIBOLD),
                        ))
                        .child(button(
                            "tb-h2",
                            "H2",
                            "Heading 2",
                            Box::new(Heading { level: 2 }),
                            |d| d.font_weight(FontWeight::SEMIBOLD),
                        ))
                        .child(button(
                            "tb-h3",
                            "H3",
                            "Heading 3",
                            Box::new(Heading { level: 3 }),
                            |d| d.font_weight(FontWeight::SEMIBOLD),
                        ))
                        .child(sep())
                        .child(button(
                            "tb-quote",
                            "Quote",
                            "Quote",
                            Box::new(Blockquote),
                            |d| d,
                        ))
                        .child(button(
                            "tb-list",
                            "List",
                            "Bullet List",
                            Box::new(BulletList),
                            |d| d,
                        )),
                ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    fn render_slash(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (caret, lh) = self.screen_point(self.sel.head)?;
        let matches = self.slash_matches();
        let theme = cx.theme();
        let (bg, border, fg, muted, active) = (
            theme.popover,
            theme.border,
            theme.popover_foreground,
            theme.muted_foreground,
            theme.list_active,
        );
        let selected = self.slash_ix;
        let entity = cx.entity().downgrade();
        Some(
            deferred(
                anchored()
                    .position(point(caret.x, caret.y + lh + px(4.)))
                    .child(
                        div()
                            .occlude()
                            .w(px(260.))
                            .flex()
                            .flex_col()
                            .p(px(4.))
                            .rounded(px(8.))
                            .bg(bg)
                            .border_1()
                            .border_color(border)
                            .shadow_lg()
                            .children(matches.into_iter().enumerate().map(|(n, item)| {
                                let (label, hint, _) = SLASH_ITEMS[item];
                                let entity = entity.clone();
                                div()
                                    .id(("slash", item))
                                    .flex()
                                    .flex_col()
                                    .px(px(8.))
                                    .py(px(4.))
                                    .rounded(px(5.))
                                    .cursor_pointer()
                                    .when(n == selected, |d| d.bg(active))
                                    .child(div().text_sm().text_color(fg).child(label))
                                    .child(div().text_xs().text_color(muted).child(hint))
                                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                        cx.stop_propagation();
                                        window.prevent_default();
                                        entity.update(cx, |e, cx| e.apply_slash(item, cx)).ok();
                                    })
                            })),
                    ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}
