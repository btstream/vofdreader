use super::*;

#[test]
fn reads_the_first_family_from_fontconfig_output() {
    assert_eq!(parse_family("Inter\n"), Some("Inter".into()));
    assert_eq!(
        parse_family("\"Noto Sans CJK SC\", Noto Sans CJK JP"),
        Some("Noto Sans CJK SC".into())
    );
    assert_eq!(parse_family("  \n Cantarell \n"), Some("Cantarell".into()));
    assert_eq!(parse_family(""), None);
    assert_eq!(parse_family(" \n "), None);
}

#[test]
fn resolved_ui_font_is_usable() {
    let font = ui_font();
    assert!(!font.family.is_empty());
    assert!(font.fallbacks.iter().all(|family| !family.is_empty()));
}
