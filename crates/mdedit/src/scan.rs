//! The open folder's scan (`mdedit_core::scan`), shared by every view: built
//! in the background when first asked for, and again when it's half a
//! minute old or a save may have changed it. Views redraw when one lands.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::*;
use mdedit_core::scan::Scan;

#[derive(Default)]
pub struct FolderScan {
    root: Option<PathBuf>,
    scan: Option<Arc<Scan>>,
    at: Option<Instant>,
    building: Option<PathBuf>,
}

impl Global for FolderScan {}

const STALE: Duration = Duration::from_secs(30);

/// The scan of `root` as it stands (possibly a little old, or `None` while
/// the first is built), starting a fresh one when it's due.
pub fn get(root: &Path, cx: &mut App) -> Option<Arc<Scan>> {
    let state = cx.default_global::<FolderScan>();
    let current = state.root.as_deref() == Some(root);
    let due = !current || state.at.is_none_or(|t| t.elapsed() > STALE);
    if due && state.building.as_deref() != Some(root) {
        state.building = Some(root.to_path_buf());
        let root = root.to_path_buf();
        let build = cx.background_spawn({
            let root = root.clone();
            async move { Scan::build(&root) }
        });
        cx.spawn(async move |cx| {
            let scan = build.await;
            cx.update(|cx| {
                let state = cx.default_global::<FolderScan>();
                if state.building.as_deref() == Some(&root) {
                    state.building = None;
                }
                state.root = Some(root);
                state.scan = Some(Arc::new(scan));
                state.at = Some(Instant::now());
                cx.refresh_windows();
            });
        })
        .detach();
    }
    let state = cx.default_global::<FolderScan>();
    (state.root.as_deref() == Some(root))
        .then(|| state.scan.clone())
        .flatten()
}

/// Something under the folder changed (a save): the next `get` rebuilds.
pub fn invalidate(cx: &mut App) {
    if let Some(state) = cx.try_global::<FolderScan>()
        && state.at.is_some()
    {
        cx.global_mut::<FolderScan>().at = None;
    }
}
