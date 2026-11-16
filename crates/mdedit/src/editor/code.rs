//! A code block's header, as ZenNotes has it: the language as a button that
//! opens a menu of languages (the fence's info string is rewritten), and a
//! copy button.

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;
use mdedit_core::buffer::{Change, Edit};
use mdedit_core::history::Kind;
use mdedit_core::syntax::LineKind;

use super::MarkdownEditor;
use super::highlight;

/// The languages the menu offers: a name, and the word the fence takes.
pub(crate) const LANGUAGES: &[(&str, &str)] = &[
    ("Plain text", ""),
    ("Bash", "bash"),
    ("C", "c"),
    ("C++", "cpp"),
    ("C#", "csharp"),
    ("CSS", "css"),
    ("Diff", "diff"),
    ("Go", "go"),
    ("HTML", "html"),
    ("Java", "java"),
    ("JavaScript", "javascript"),
    ("JSON", "json"),
    ("Kotlin", "kotlin"),
    ("Lua", "lua"),
    ("Markdown", "markdown"),
    ("Mermaid", "mermaid"),
    ("PHP", "php"),
    ("Python", "python"),
    ("Ruby", "ruby"),
    ("Rust", "rust"),
    ("SQL", "sql"),
    ("Swift", "swift"),
    ("TOML", "toml"),
    ("TypeScript", "typescript"),
    ("XML", "xml"),
    ("YAML", "yaml"),
];

impl MarkdownEditor {
    /// The language word of the code block whose header is row `ix` (its
    /// info string's first word), and the source line of its fence.
    pub(crate) fn fence_lang(&self, ix: usize) -> Option<(usize, String)> {
        let line = self.rows.get(ix)?.lines.start;
        let l = self.syntax.lines.get(line)?;
        if l.kind != LineKind::FenceOpen {
            return None;
        }
        let word = l
            .lang
            .clone()
            .and_then(|r| self.text[r].split_whitespace().next().map(str::to_string))
            .unwrap_or_default();
        Some((line, word))
    }

    /// Give the code block whose fence is source line `line` language
    /// `word` (none, for plain text): the info string's first word is
    /// replaced, and whatever follows it kept.
    pub(crate) fn set_fence_lang(&mut self, line: usize, word: &str, cx: &mut Context<Self>) {
        let Some(l) = self.syntax.lines.get(line) else {
            return;
        };
        let edit = match l.lang.clone() {
            Some(r) => {
                let info = &self.text[r.clone()];
                let first = info.split_whitespace().next().unwrap_or("");
                let rest = info[first.len()..].trim_start();
                let text = match (word.is_empty(), rest.is_empty()) {
                    (_, true) => word.to_string(),
                    (true, false) => rest.to_string(),
                    (false, false) => format!("{word} {rest}"),
                };
                Edit { range: r, text }
            }
            None if word.is_empty() => return,
            None => {
                let lt = &self.text[l.range.clone()];
                Edit::insert(l.range.start + lt.trim_end().len(), word)
            }
        };
        let sel = self.sel;
        self.apply(
            Change {
                edits: vec![edit],
                selection: sel,
            },
            Kind::Other,
            cx,
        );
    }

    /// A press on a code block's language: the menu of languages, the
    /// block's own ticked.
    pub(crate) fn lang_menu_at(
        &mut self,
        ix: usize,
        p: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((line, current)) = self.fence_lang(ix) else {
            return;
        };
        let mut items: Vec<(String, String)> = LANGUAGES
            .iter()
            .map(|(name, word)| (name.to_string(), word.to_string()))
            .collect();
        let known = items
            .iter()
            .any(|(_, w)| highlight::same_language(w, &current));
        if !current.is_empty() && !known {
            items.insert(1, (current.clone(), current.clone()));
        }
        let editor = cx.entity().downgrade();
        let menu = PopupMenu::build(window, cx, move |mut menu, _, _| {
            menu = menu.max_h(px(360.)).scrollable(true);
            for (name, word) in &items {
                let editor = editor.clone();
                let word = word.clone();
                let ticked = highlight::same_language(&word, &current);
                menu = menu.item(PopupMenuItem::new(name.clone()).checked(ticked).on_click(
                    move |_, _, cx| {
                        editor
                            .update(cx, |e, cx| e.set_fence_lang(line, &word, cx))
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
    }
}
