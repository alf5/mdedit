//! Every command the app has, and the keys they're on.
//!
//! Shortcuts are written with `secondary-`, which GPUI resolves to Cmd on
//! macOS and Ctrl elsewhere. They follow v1's, except Ctrl/Cmd+Shift+L for
//! the sidebar (v1's Ctrl/Cmd+Shift+P is the command palette's now).

use std::rc::Rc;

use gpui_kit::*;

gpui_kit::actions!(
    mdedit,
    [
        NewFile,
        Open,
        OpenFolder,
        CloseFolder,
        Save,
        SaveAs,
        CloseTab,
        CloseOtherTabs,
        Quit,
        ToggleSidebar,
        SplitRight,
        JoinPanes,
        NextTab,
        PrevTab,
        ClearRecent,
        RevealInFileManager,
        About,
        ToggleHexOverwrite,
        HexUndo,
        HexRedo,
    ]
);

// The markdown editor's own commands, bound in the "MdEditor" context.
gpui_kit::actions!(
    editor,
    [
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        LineStart,
        LineEnd,
        SelectLineStart,
        SelectLineEnd,
        DocStart,
        DocEnd,
        SelectDocStart,
        SelectDocEnd,
        PageUp,
        PageDown,
        SelectPageUp,
        SelectPageDown,
        SelectAll,
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteWordRight,
        Newline,
        SoftBreak,
        Indent,
        Outdent,
        Undo,
        Redo,
        Copy,
        Cut,
        Paste,
        PastePlain,
        Bold,
        Italic,
        Strikethrough,
        InlineCode,
        InsertLink,
        Paragraph,
        BulletList,
        NumberedList,
        TaskList,
        Blockquote,
        CodeBlock,
        InsertTable,
        InsertRule,
        InsertDiagram,
        InsertMath,
        ToggleSource,
        Find,
        FindReplace,
        FindNext,
        FindPrevious,
        CloseFind,
        ReplaceOne,
        ReplaceAll,
        Cancel,
    ]
);

/// Heading 1–6.
#[derive(Clone, PartialEq, Debug, gpui_kit::Action)]
#[action(namespace = editor, no_json)]
pub struct Heading {
    pub level: u8,
}

/// Rendered, Live or Source.
#[derive(Clone, PartialEq, Debug, gpui_kit::Action)]
#[action(namespace = editor, no_json)]
pub struct SetMode {
    pub mode: mdedit_core::prefs::Mode,
}

/// Open Recent's entry `ix`.
#[derive(Clone, PartialEq, Debug, gpui_kit::Action)]
#[action(namespace = mdedit, no_json)]
pub struct OpenRecent {
    pub ix: usize,
}

/// A theme id from `themes.css`; empty follows the system.
#[derive(Clone, PartialEq, Debug, gpui_kit::Action)]
#[action(namespace = mdedit, no_json)]
pub struct SetTheme {
    pub id: SharedString,
}

fn bind(cx: &mut App, keys: &str, action: impl Action, context: Option<&str>) {
    bind_boxed(cx, keys, Box::new(action), context);
}

fn bind_boxed(cx: &mut App, keys: &str, action: Box<dyn Action>, context: Option<&str>) {
    let predicate = context
        .map(|c| Rc::new(KeyBindingContextPredicate::parse(c).expect("a valid key context")));
    match KeyBinding::load(keys, action, predicate, false, None, cx) {
        Ok(binding) => cx.bind_keys([binding]),
        Err(e) => eprintln!("mdedit: bad key binding {keys}: {e}"),
    }
}

pub fn bind_keys(cx: &mut App) {
    let ws = Some("Workspace");
    bind(cx, "secondary-n", NewFile, ws);
    bind(cx, "secondary-o", Open, ws);
    bind(cx, "secondary-shift-k", OpenFolder, ws);
    bind(cx, "secondary-s", Save, ws);
    bind(cx, "secondary-shift-s", SaveAs, ws);
    bind(cx, "secondary-w", CloseTab, ws);
    bind(cx, "secondary-q", Quit, ws);
    bind(cx, "secondary-shift-l", ToggleSidebar, ws);
    bind(cx, "secondary-\\", SplitRight, ws);
    bind(cx, "ctrl-tab", NextTab, ws);
    bind(cx, "ctrl-shift-tab", PrevTab, ws);
    bind(cx, "secondary-pagedown", NextTab, ws);
    bind(cx, "secondary-pageup", PrevTab, ws);

    let ed = Some("MdEditor");
    for (keys, action) in [
        ("left", Box::new(MoveLeft) as Box<dyn Action>),
        ("right", Box::new(MoveRight)),
        ("up", Box::new(MoveUp)),
        ("down", Box::new(MoveDown)),
        ("shift-left", Box::new(SelectLeft)),
        ("shift-right", Box::new(SelectRight)),
        ("shift-up", Box::new(SelectUp)),
        ("shift-down", Box::new(SelectDown)),
        ("home", Box::new(LineStart)),
        ("end", Box::new(LineEnd)),
        ("shift-home", Box::new(SelectLineStart)),
        ("shift-end", Box::new(SelectLineEnd)),
        ("pageup", Box::new(PageUp)),
        ("pagedown", Box::new(PageDown)),
        ("shift-pageup", Box::new(SelectPageUp)),
        ("shift-pagedown", Box::new(SelectPageDown)),
        ("backspace", Box::new(Backspace)),
        ("shift-backspace", Box::new(Backspace)),
        ("delete", Box::new(Delete)),
        ("enter", Box::new(Newline)),
        ("shift-enter", Box::new(SoftBreak)),
        ("tab", Box::new(Indent)),
        ("shift-tab", Box::new(Outdent)),
        ("escape", Box::new(Cancel)),
        ("secondary-a", Box::new(SelectAll)),
        ("secondary-z", Box::new(Undo)),
        ("secondary-shift-z", Box::new(Redo)),
        ("secondary-c", Box::new(Copy)),
        ("secondary-x", Box::new(Cut)),
        ("secondary-v", Box::new(Paste)),
        ("secondary-shift-v", Box::new(PastePlain)),
        ("secondary-b", Box::new(Bold)),
        ("secondary-i", Box::new(Italic)),
        ("secondary-shift-x", Box::new(Strikethrough)),
        ("secondary-e", Box::new(InlineCode)),
        ("secondary-k", Box::new(InsertLink)),
        ("secondary-0", Box::new(Paragraph)),
        ("secondary-shift-m", Box::new(ToggleSource)),
        ("secondary-f", Box::new(Find)),
        ("f3", Box::new(FindNext)),
        ("shift-f3", Box::new(FindPrevious)),
    ] {
        bind_boxed(cx, keys, action, ed);
    }
    // Word and document jumps: Ctrl on Linux and Windows, Alt and Cmd on a
    // Mac.
    let word = if cfg!(target_os = "macos") {
        "alt"
    } else {
        "ctrl"
    };
    bind(cx, &format!("{word}-left"), WordLeft, ed);
    bind(cx, &format!("{word}-right"), WordRight, ed);
    bind(cx, &format!("{word}-shift-left"), SelectWordLeft, ed);
    bind(cx, &format!("{word}-shift-right"), SelectWordRight, ed);
    bind(cx, &format!("{word}-backspace"), DeleteWordLeft, ed);
    bind(cx, &format!("{word}-delete"), DeleteWordRight, ed);
    if cfg!(target_os = "macos") {
        bind(cx, "cmd-left", LineStart, ed);
        bind(cx, "cmd-right", LineEnd, ed);
        bind(cx, "cmd-up", DocStart, ed);
        bind(cx, "cmd-down", DocEnd, ed);
        bind(cx, "cmd-shift-up", SelectDocStart, ed);
        bind(cx, "cmd-shift-down", SelectDocEnd, ed);
        bind(cx, "cmd-alt-f", FindReplace, ed);
    } else {
        bind(cx, "ctrl-home", DocStart, ed);
        bind(cx, "ctrl-end", DocEnd, ed);
        bind(cx, "ctrl-shift-home", SelectDocStart, ed);
        bind(cx, "ctrl-shift-end", SelectDocEnd, ed);
        bind(cx, "ctrl-y", Redo, ed);
        bind(cx, "ctrl-h", FindReplace, ed);
    }
    for level in 1..=6u8 {
        bind(cx, &format!("secondary-{level}"), Heading { level }, ed);
    }

    let hex = Some("HexView");
    bind(cx, "insert", ToggleHexOverwrite, hex);
    bind(cx, "secondary-shift-o", ToggleHexOverwrite, hex);
    bind(cx, "secondary-z", HexUndo, hex);
    bind(cx, "secondary-shift-z", HexRedo, hex);
    bind(cx, "ctrl-y", HexRedo, hex);
}
