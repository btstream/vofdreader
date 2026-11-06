use super::{FontResource, ResScope};

fn font(id: &str, file: &str) -> FontResource {
    FontResource {
        id: id.to_string(),
        scope: ResScope::Public,
        name: "HYe3gj".to_string(),
        family: "宋体".to_string(),
        file: Some(file.to_string()),
        bold: false,
        italic: false,
    }
}

#[test]
fn font_keys_include_scope() {
    assert_eq!(font("6", "font_6.ttf").key(), "Public:6");
}

#[test]
fn embedded_families_are_unique_per_file() {
    let first = font("6", "Doc_0/Res/font_6.ttf");
    let second = font("69", "Doc_0/Res/Font69.ttf");
    let same = font("5", "Doc_0/Res/font_6.ttf");
    assert_ne!(first.embedded_family(), second.embedded_family());
    assert_eq!(first.embedded_family(), same.embedded_family());
    assert!(first.embedded_family().starts_with("OFD-Embedded-"));
}
