use super::*;

#[test]
fn resolves_relative_to_document() {
    assert_eq!(
        resolve_loc("Doc_0/Document.xml", "Pages/Page_0/Content.xml").unwrap(),
        "Doc_0/Pages/Page_0/Content.xml"
    );
    assert_eq!(
        resolve_loc("OFD.xml", "Doc_0/Document.xml").unwrap(),
        "Doc_0/Document.xml"
    );
    assert_eq!(
        resolve_loc("Doc_0/DocumentRes.xml", "Res").unwrap(),
        "Doc_0/Res"
    );
    assert_eq!(
        join_dir("Doc_0/Res", "image_0.png").unwrap(),
        "Doc_0/Res/image_0.png"
    );
}

#[test]
fn root_base_loc_resolves_to_package_root() {
    assert_eq!(join_dir("Doc_0", "/").unwrap(), "");
    assert_eq!(join_dir("", "/").unwrap(), "");
    assert_eq!(join_dir("", "/Doc_0/Res/2.gif").unwrap(), "Doc_0/Res/2.gif");
}

#[test]
fn normalizes_dots() {
    assert_eq!(normalize_loc("./a//b/./c").unwrap(), "a/b/c");
    assert_eq!(normalize_loc("a/b/../c").unwrap(), "a/c");
    assert_eq!(normalize_loc("/a/b").unwrap(), "a/b");
}

#[test]
fn rejects_escape() {
    assert!(normalize_loc("../OFD.xml").is_err());
    assert!(normalize_loc("a/../../OFD.xml").is_err());
}
