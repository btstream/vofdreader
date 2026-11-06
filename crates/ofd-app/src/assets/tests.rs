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
    let source = include_str!("../main.rs");
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
