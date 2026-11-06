use crate::search_snippet;

#[test]
fn snippet_trims_to_whole_chars_and_marks_ellipsis() {
    let text = format!("{}电子发票{}", "前缀".repeat(20), "后缀".repeat(20));
    let start = text.find("电子发票").unwrap();
    let snippet = search_snippet(&text, start, start + "电子发票".len());
    assert!(snippet.starts_with('…'));
    assert!(snippet.ends_with('…'));
    assert!(snippet.contains("电子发票"));
}

#[test]
fn snippet_at_boundaries_does_not_panic() {
    assert_eq!(search_snippet("abc", 0, 1), "abc");
    assert_eq!(search_snippet("发票", 0, 6), "发票");
}

#[test]
fn snippet_handles_ascii_and_multibyte_mix() {
    let text = "invoice 发票 2026";
    let start = text.find("发票").unwrap();
    let snippet = search_snippet(text, start, start + "发票".len());
    assert_eq!(snippet, "invoice 发票 2026");
}
