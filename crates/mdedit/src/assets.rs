//! Icons: gpui-kit's default set plus the few extra Lucide icons mdedit uses.

use std::borrow::Cow;

use gpui_kit::*;

gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        Columns2, Save, FilePlus, Image, Binary, FileCode, X, Settings
    ]
);

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(found) = ExtraIcons.load(path)? {
            return Ok(Some(found));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut all = ExtraIcons.list(path)?;
        for p in gpui_kit::assets::Assets.list(path)? {
            if !all.contains(&p) {
                all.push(p);
            }
        }
        Ok(all)
    }
}
