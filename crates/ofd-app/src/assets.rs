//! Application icons plus GPUI Component's own default icon set. Do not embed
//! the full Lucide catalog: components still need icons not used by our views.
use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

gpui_kit::assets::icon_assets!(
    ReaderIcons,
    [
        ChevronDown,
        ChevronUp,
        Download,
        Ellipsis,
        FileText,
        FolderOpen,
        Grid2x2,
        Info,
        Minus,
        Moon,
        MoveHorizontal,
        PanelLeft,
        Plus,
        Printer,
        RotateCw,
        Rows3,
        Scan,
        Search,
        Settings,
        ShieldCheck,
        ShieldOff,
        Square,
        Sun,
        X,
    ]
);

pub struct ReaderAssets;

impl AssetSource for ReaderAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(icon) = ReaderIcons.load(path)? {
            return Ok(Some(icon));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = ReaderIcons.list(path)?;
        paths.extend(gpui_kit::assets::Assets.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

#[cfg(test)]
mod tests;
