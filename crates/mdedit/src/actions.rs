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
    let predicate = context
        .map(|c| Rc::new(KeyBindingContextPredicate::parse(c).expect("a valid key context")));
    match KeyBinding::load(keys, Box::new(action), predicate, false, None, cx) {
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

    let hex = Some("HexView");
    bind(cx, "insert", ToggleHexOverwrite, hex);
    bind(cx, "secondary-shift-o", ToggleHexOverwrite, hex);
    bind(cx, "secondary-z", HexUndo, hex);
    bind(cx, "secondary-shift-z", HexRedo, hex);
    bind(cx, "ctrl-y", HexRedo, hex);
}
