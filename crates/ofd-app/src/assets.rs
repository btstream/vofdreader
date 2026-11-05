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
mod tests {
    use super::{AssetSource, ReaderAssets, ReaderIcons};

    #[test]
    fn all_registered_icons_load() {
        let paths = ReaderAssets.list("icons/").unwrap();
        assert!(!paths.is_empty());
        for path in paths {
            assert!(ReaderAssets.load(&path).unwrap().is_some(), "{path}");
        }
    }

    #[test]
    fn all_view_icons_are_registered() {
        // Guard against adding IconName::Something in a view but forgetting
        // the selected asset list (which otherwise renders a blank icon).
        let source = include_str!("main.rs");
        let registered = ReaderIcons.list("icons/").unwrap();
        for tail in source.split("IconName::").skip(1) {
            let name: String = tail
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric())
                .collect();
            let path = registered.iter().find(|path| {
                let filename = path.rsplit('/').next().unwrap().trim_end_matches(".svg");
                filename.replace('-', "").eq_ignore_ascii_case(&name)
            });
            assert!(path.is_some(), "missing asset for IconName::{name}");
        }
    }
}
