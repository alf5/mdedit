//! App-wide state: the settings, shared by every view and saved on change.

use gpui_kit::*;
use mdedit_core::prefs::Prefs;

pub struct AppState {
    pub prefs: Prefs,
    /// Off in tests, so they never write the real settings file.
    persist: bool,
}

impl Global for AppState {}

impl AppState {
    pub fn init(cx: &mut App) {
        cx.set_global(AppState {
            prefs: Prefs::load(),
            persist: true,
        });
    }

    /// Settings that live only in memory.
    #[cfg(test)]
    pub fn init_ephemeral(cx: &mut App) {
        cx.set_global(AppState {
            prefs: Prefs::default(),
            persist: false,
        });
    }

    pub fn prefs(cx: &App) -> &Prefs {
        &cx.global::<AppState>().prefs
    }

    /// The settings, where there may be none yet (some tests).
    pub fn try_prefs(cx: &App) -> Option<&Prefs> {
        cx.try_global::<AppState>().map(|s| &s.prefs)
    }

    /// Change the settings and write them out. Saving is cheap and rare
    /// (a theme pick, an open), so it happens on every change rather than
    /// on exit, which a crash would skip.
    pub fn update_prefs(cx: &mut App, f: impl FnOnce(&mut Prefs)) {
        let state = cx.global_mut::<AppState>();
        f(&mut state.prefs);
        if state.persist {
            state.prefs.save();
        }
    }
}
