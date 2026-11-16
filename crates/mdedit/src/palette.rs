//! The command palette (Ctrl/Cmd+Shift+P): every command, searchable, with
//! its shortcut. And Quick Open (Ctrl/Cmd+P): recent files and the open
//! folder's files, by name or path.
//!
//! Both are dialogs, which gpui-kit draws beside the window's content
//! rather than inside it. So a command isn't dispatched from the dialog:
//! the dialog closes, focus goes back where it was, and the command is
//! dispatched from there, where the editor and the workspace can hear it.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_kit::*;
use mdedit_core::prefs::Mode;
use mdedit_core::themes;

use crate::actions::*;

type Groups = Rc<Vec<(&'static str, Vec<(SharedString, Box<dyn Action>)>)>>;

fn entry(label: impl Into<SharedString>, action: impl Action) -> (SharedString, Box<dyn Action>) {
    (label.into(), Box::new(action))
}

/// Every command, grouped. `markdown` adds the formatting and insert
/// commands, which only make sense on a markdown tab.
fn commands(markdown: bool) -> Groups {
    let mut groups = vec![(
        "File",
        vec![
            entry("New Document", NewFile),
            entry("Open…", Open),
            entry("Open Folder…", OpenFolder),
            entry("Quick Open…", QuickOpen),
            entry("Import HTML…", ImportHtml),
            entry("Import Word Document…", ImportWord),
            entry("Save", Save),
            entry("Save As…", SaveAs),
            entry("Close Tab", CloseTab),
            entry("Close Other Tabs", CloseOtherTabs),
            entry("Open Containing Folder", RevealInFileManager),
            entry("Clear Recent Files", ClearRecent),
            entry("Quit", Quit),
        ],
    )];
    if markdown {
        groups.push((
            "Format",
            vec![
                entry("Bold", Bold),
                entry("Italic", Italic),
                entry("Strikethrough", Strikethrough),
                entry("Inline Code", InlineCode),
                entry("Link", InsertLink),
                entry("Heading 1", Heading { level: 1 }),
                entry("Heading 2", Heading { level: 2 }),
                entry("Heading 3", Heading { level: 3 }),
                entry("Heading 4", Heading { level: 4 }),
                entry("Paragraph", Paragraph),
                entry("Bullet List", BulletList),
                entry("Numbered List", NumberedList),
                entry("Task List", TaskList),
                entry("Quote", Blockquote),
            ],
        ));
        groups.push((
            "Insert",
            vec![
                entry("Code Block", CodeBlock),
                entry("Table", InsertTable),
                entry("Mermaid Diagram", InsertDiagram),
                entry("Math Block", InsertMath),
                entry("Horizontal Rule", InsertRule),
            ],
        ));
        groups.push((
            "Mode",
            vec![
                entry(
                    "Rendered",
                    SetMode {
                        mode: Mode::Rendered,
                    },
                ),
                entry("Live: Markdown at the Caret", SetMode { mode: Mode::Live }),
                entry("Source", SetMode { mode: Mode::Source }),
                entry("Toggle Markdown Source", ToggleSource),
                entry("Find", Find),
                entry("Find and Replace", FindReplace),
            ],
        ));
    }
    groups.push((
        "View",
        vec![
            entry("Toggle Sidebar", ToggleSidebar),
            entry("Split Right / Move to Other Side", SplitRight),
            entry("Join Sides", JoinPanes),
            entry("Next Document", NextTab),
            entry("Previous Document", PrevTab),
            entry("About mdedit", About),
        ],
    ));
    let mut theme = vec![entry("Theme: Match System", SetTheme { id: "".into() })];
    for (family, items) in themes::FAMILIES {
        for (id, label) in *items {
            theme.push(entry(
                format!("Theme: {family} {label}"),
                SetTheme { id: (*id).into() },
            ));
        }
    }
    groups.push(("Theme", theme));
    Rc::new(groups)
}

/// Open the command palette over `window`.
pub fn open_commands(markdown: bool, window: &mut Window, cx: &mut App) {
    let groups = commands(markdown);
    let back_to = window.focused(cx);
    let state = cx.new(|cx| CommandState::new(window, cx));
    // Focus the search field once it exists, on the dialog's first build.
    let first = std::cell::Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if first.replace(false) {
            state.update(cx, |s, cx| s.focus(window, cx));
        }
        let mut command = Command::new(&state)
            .placeholder("Type a command")
            .bordered(false)
            .max_h(px(420.));
        for (label, items) in groups.iter() {
            command = command.group(
                CommandGroup::new().label(*label).items(
                    items
                        .iter()
                        .map(|(l, a)| CommandItem::new().label(l.clone()).action(a.boxed_clone())),
                ),
            );
        }
        let groups = groups.clone();
        let back_to = back_to.clone();
        dialog.close_button(false).width(px(560.)).child(
            command
                .on_confirm(move |ix, window, cx| {
                    window.close_dialog(cx);
                    let Some((_, items)) = groups.get(ix.section) else {
                        return;
                    };
                    let Some((_, action)) = items.get(ix.row) else {
                        return;
                    };
                    match &back_to {
                        Some(f) => {
                            f.focus(window, cx);
                            f.dispatch_action(&**action, window, cx);
                        }
                        None => window.dispatch_action(action.boxed_clone(), cx),
                    }
                })
                .on_cancel(|window, cx| window.close_dialog(cx)),
        )
    });
}

/// Open Quick Open: `recent` first, then `files` (relative to `root`).
pub fn open_files(
    recent: Vec<PathBuf>,
    root: Option<PathBuf>,
    files: Vec<PathBuf>,
    on_pick: impl Fn(PathBuf, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let label = move |p: &Path| -> SharedString {
        match root.as_deref().and_then(|r| p.strip_prefix(r).ok()) {
            Some(rel) => rel.display().to_string().into(),
            None => {
                let (name, dir) = mdedit_core::prefs::recent_parts(p, 48);
                if dir.is_empty() {
                    name.into()
                } else {
                    format!("{name}  —  {dir}").into()
                }
            }
        }
    };
    let sections: Rc<Vec<(&'static str, Vec<(SharedString, PathBuf)>)>> = Rc::new(
        [("Recent", recent), ("Folder", files)]
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(name, v)| (name, v.into_iter().map(|p| (label(&p), p)).collect()))
            .collect(),
    );
    let on_pick = Rc::new(on_pick);
    let state = cx.new(|cx| CommandState::new(window, cx));
    let first = std::cell::Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if first.replace(false) {
            state.update(cx, |s, cx| s.focus(window, cx));
        }
        let mut command = Command::new(&state)
            .placeholder("Open a file by name")
            .bordered(false)
            .max_h(px(420.));
        for (name, items) in sections.iter() {
            command = command.group(CommandGroup::new().label(*name).items(items.iter().map(
                |(l, p)| {
                    CommandItem::new()
                        .label(l.clone())
                        .keywords([p.display().to_string()])
                },
            )));
        }
        let sections = sections.clone();
        let on_pick = on_pick.clone();
        dialog.close_button(false).width(px(560.)).child(
            command
                .on_confirm(move |ix, window, cx| {
                    window.close_dialog(cx);
                    if let Some((_, path)) =
                        sections.get(ix.section).and_then(|(_, v)| v.get(ix.row))
                    {
                        on_pick(path.clone(), window, cx);
                    }
                })
                .on_cancel(|window, cx| window.close_dialog(cx)),
        )
    });
}
