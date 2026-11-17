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

/// What the caret popup is offering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PopKind {
    /// Blocks, after a `/` starting a line.
    Slash,
    /// Notes, after `[[` (files of any kind after `![[`).
    Note { embed: bool },
    /// Headings, after `[[#` (this document's) or `[[Note#`.
    Heading(String),
    /// Tags, after a `#` starting a word.
    Tag,
}

/// One entry of the caret popup.
#[derive(Clone, Debug)]
pub(crate) struct PopItem {
    pub label: String,
    pub hint: String,
    pick: Pick,
}

#[derive(Clone, Debug)]
enum Pick {
    Slash(usize),
    /// Replace the query with this; for links, close them with `]]`.
    Text(String),
}

/// The most entries the popup lists.
const MAX_ITEMS: usize = 8;

/// How well `label` answers `query` (both lower-case): a start beats a
/// word start beats anywhere beats letters in order; `None` is no match.
fn rank(query: &str, label: &str) -> Option<u8> {
    if query.is_empty() || label.starts_with(query) {
        Some(0)
    } else if label
        .match_indices(query)
        .any(|(i, _)| label[..i].ends_with([' ', '-', '_', '/']))
    {
        Some(1)
    } else if label.contains(query) {
        Some(2)
    } else {
        fuzzy(query, label).then_some(3)
    }
}

/// The best of `labels` (with hints) for `query`, best first.
fn ranked(query: &str, labels: impl IntoIterator<Item = (String, String)>) -> Vec<PopItem> {
    let q = query.to_lowercase();
    let mut hits: Vec<(u8, PopItem)> = labels
        .into_iter()
        .filter_map(|(label, hint)| {
            let r = rank(&q, &label.to_lowercase())?;
            Some((
                r,
                PopItem {
                    pick: Pick::Text(label.clone()),
                    label,
                    hint,
                },
            ))
        })
        .collect();
    hits.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.label.len().cmp(&b.1.label.len()))
            .then_with(|| a.1.label.cmp(&b.1.label))
    });
    hits.dedup_by(|a, b| a.1.label == b.1.label);
    hits.into_iter().take(MAX_ITEMS).map(|(_, i)| i).collect()
}

impl MarkdownEditor {
    /// The `/` menu's entries matching what's been typed after the slash.
    fn slash_matches(&self, at: usize) -> Vec<PopItem> {
        let q = self.text[at + 1..self.sel.head.max(at + 1)].to_lowercase();
        SLASH_ITEMS
            .iter()
            .enumerate()
            .filter(|(_, (label, hint, _))| {
                q.is_empty() || fuzzy(&q, &label.to_lowercase()) || hint.to_lowercase().contains(&q)
            })
            .map(|(i, (label, hint, _))| PopItem {
                label: (*label).into(),
                hint: (*hint).into(),
                pick: Pick::Slash(i),
            })
            .collect()
    }

    /// The popup's entries for what's typed between its anchor and the
    /// caret.
    fn pop_matches(&mut self, at: usize, cx: &mut Context<Self>) -> Vec<PopItem> {
        let query = self.text[at..self.sel.head.max(at)].to_string();
        match self.pop.clone() {
            PopKind::Slash => self.slash_matches(at),
            PopKind::Note { embed } => {
                let Some(index) = self.wiki_files() else {
                    return Vec::new();
                };
                let root = index.root().to_path_buf();
                let names = index.files().filter_map(|p| {
                    let name = p.file_name()?.to_string_lossy().to_string();
                    let md = name.ends_with(".md") || name.ends_with(".markdown");
                    let label = if md {
                        p.file_stem()?.to_string_lossy().to_string()
                    } else if embed {
                        name
                    } else {
                        return None;
                    };
                    let hint = p
                        .parent()
                        .and_then(|d| d.strip_prefix(&root).ok())
                        .map(|d| d.to_string_lossy().to_string())
                        .unwrap_or_default();
                    Some((label, hint))
                });
                ranked(&query, names.collect::<Vec<_>>())
            }
            PopKind::Heading(note) => {
                let headings: Vec<String> = if note.is_empty() {
                    self.syntax
                        .headings
                        .iter()
                        .map(|h| h.text.clone())
                        .collect()
                } else {
                    self.wiki_note(&note)
                        .and_then(|p| std::fs::read_to_string(p).ok())
                        .map(|t| {
                            mdedit_core::syntax::parse(&t)
                                .headings
                                .into_iter()
                                .map(|h| h.text)
                                .collect()
                        })
                        .unwrap_or_default()
                };
                ranked(&query, headings.into_iter().map(|h| (h, String::new())))
            }
            PopKind::Tag => {
                let mut tags: Vec<(String, String)> = self
                    .syntax
                    .tags(&self.text)
                    .into_iter()
                    .filter(|t| t.len() > query.len() || *t != query)
                    .map(|t| (t.to_string(), String::new()))
                    .collect();
                if let Some(root) = self.root.clone()
                    && let Some(scan) = crate::scan::get(&root, cx)
                {
                    tags.extend(scan.tags().into_iter().map(|(t, n)| {
                        (
                            t,
                            if n == 1 {
                                "1 note".into()
                            } else {
                                format!("{n} notes")
                            },
                        )
                    }));
                }
                // The tag being typed isn't a suggestion for itself.
                tags.retain(|(t, _)| *t != query);
                ranked(&query, tags)
            }
        }
    }

    /// After every edit or move: open the popup on a freshly typed `/` at a
    /// line's start, `[[`, `[[…#` or a word-starting `#`; keep it while the
    /// query fits; close it otherwise.
    pub(crate) fn update_slash(&mut self, typed: Option<&str>, cx: &mut Context<Self>) {
        let in_code = matches!(
            self.syntax.lines[self.syntax.line_at(self.sel.head)].kind,
            mdedit_core::syntax::LineKind::Code | mdedit_core::syntax::LineKind::FenceOpen
        );
        if !self.rich() || !self.sel.is_empty() || in_code {
            self.slash = None;
            return;
        }
        let caret = self.sel.head;
        let before = &self.text[..caret];
        let line_start = mdedit_core::buffer::line_start(&self.text, caret);
        let in_grid = self.grid_widget(caret).is_some();
        let open = |kind: PopKind, at: usize| Some((kind, at));
        let opened = match typed {
            // Blocks can't go in a table cell.
            Some("/") if !in_grid => {
                let (indent, _, len) = edit::line_prefix(&self.text[line_start..]);
                (self.text[line_start + indent + len..caret] == *"/")
                    .then(|| open(PopKind::Slash, caret - 1))
                    .flatten()
            }
            Some("[") if before.ends_with("[[") => open(
                PopKind::Note {
                    embed: before[..caret - 2].ends_with('!'),
                },
                caret,
            ),
            Some("#") => {
                // Inside an open `[[`: that note's headings.
                let link = before[line_start..caret - 1]
                    .rfind("[[")
                    .map(|i| line_start + i + 2)
                    .filter(|&i| !self.text[i..caret - 1].contains([']', '|', '#']));
                match link {
                    Some(i) => open(PopKind::Heading(self.text[i..caret - 1].to_string()), caret),
                    None => {
                        let word_start = before[..caret - 1]
                            .chars()
                            .next_back()
                            .is_none_or(char::is_whitespace);
                        word_start.then(|| open(PopKind::Tag, caret)).flatten()
                    }
                }
            }
            _ => None,
        };
        if let Some((kind, at)) = opened {
            self.pop = kind;
            self.slash = Some(at);
            self.slash_ix = 0;
        }
        let Some(at) = self.slash else { return };
        let query = self.text.get(at.min(caret)..caret).unwrap_or("");
        let fits = caret >= at
            && match &self.pop {
                PopKind::Slash => {
                    caret > at
                        && self.text.get(at..at + 1) == Some("/")
                        && !query[1..].contains(char::is_whitespace)
                        && caret - at <= 24
                }
                PopKind::Note { .. } | PopKind::Heading(_) => {
                    self.text[..at].ends_with(['[', '#'])
                        && !query.contains(['\n', ']', '|', '#'])
                        && query.len() <= 80
                }
                PopKind::Tag => {
                    self.text[..at].ends_with('#')
                        && query
                            .chars()
                            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/'))
                        && query.len() <= 40
                }
            };
        let items = if fits {
            self.pop_matches(at, cx)
        } else {
            Vec::new()
        };
        if items.is_empty() {
            self.slash = None;
            self.pop_items.clear();
        } else {
            self.slash_ix = self.slash_ix.min(items.len() - 1);
            self.pop_items = items;
        }
    }

    /// The popup's entry `n` chosen: a block for `/query`, or the name
    /// typed out (and a link closed with `]]`).
    pub(crate) fn apply_pop(&mut self, n: usize, cx: &mut Context<Self>) {
        let Some(at) = self.slash.take() else { return };
        let Some(item) = self.pop_items.get(n).cloned() else {
            return;
        };
        self.pop_items.clear();
        match item.pick {
            Pick::Slash(i) => {
                let (_, _, what) = SLASH_ITEMS[i];
                let remove = Change {
                    edits: vec![Edit::delete(at..self.sel.head)],
                    selection: Selection::caret(at),
                };
                self.apply(remove, Kind::Other, cx);
                let change = match what {
                    Slash::Heading(level) => edit::set_heading(&self.text, self.sel, level),
                    Slash::List(block) => edit::toggle_block(&self.text, self.sel, block),
                    Slash::Insert(block, caret) => {
                        edit::insert_block(&self.text, self.sel, block, caret)
                    }
                };
                self.apply(change, Kind::Other, cx);
            }
            Pick::Text(name) => {
                let caret = self.sel.head;
                let (insert, end) = match self.pop {
                    PopKind::Tag => (name, caret),
                    _ if self.text[caret..].starts_with("]]") => (name, caret + 2),
                    _ => (format!("{name}]]"), caret),
                };
                let after = at + insert.len() + (end - caret);
                self.apply(
                    Change {
                        edits: vec![Edit::replace(at..caret, insert)],
                        selection: Selection::caret(after),
                    },
                    Kind::Other,
                    cx,
                );
            }
        }
        self.slash = None;
    }

    /// Where `pos` is on screen, from last frame: the top-left of its caret,
    /// and the line height there.
    pub(crate) fn screen_point(&self, pos: usize) -> Option<(Point<Pixels>, Pixels)> {
        let ix = self.row_at(pos);
        let (_, origin, l) = self.geometry.lines.iter().find(|(i, ..)| *i == ix)?;
        Some((*origin + l.point(l.to_display(pos)), l.line_height))
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
        let matches = self.pop_items.clone();
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
                                let PopItem { label, hint, .. } = item;
                                let entity = entity.clone();
                                div()
                                    .id(("slash", n))
                                    .flex()
                                    .flex_col()
                                    .px(px(8.))
                                    .py(px(4.))
                                    .rounded(px(5.))
                                    .cursor_pointer()
                                    .when(n == selected, |d| d.bg(active))
                                    .child(div().text_sm().text_color(fg).truncate().child(label))
                                    .when(!hint.is_empty(), |d| {
                                        d.child(
                                            div()
                                                .text_xs()
                                                .text_color(muted)
                                                .truncate()
                                                .child(hint),
                                        )
                                    })
                                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                        cx.stop_propagation();
                                        window.prevent_default();
                                        entity.update(cx, |e, cx| e.apply_pop(n, cx)).ok();
                                    })
                            })),
                    ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}
