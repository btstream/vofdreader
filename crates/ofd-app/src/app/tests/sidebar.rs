use crate::app::view::sidebar::thumbnail_scroll_target;

#[test]
fn a_page_change_reveals_the_matching_thumbnail() {
    assert_eq!(thumbnail_scroll_target(4, 10, true, Some(1)), Some(4));
    assert_eq!(thumbnail_scroll_target(0, 10, true, None), Some(0));
}

#[test]
fn the_same_page_does_not_fight_a_manual_scroll() {
    assert_eq!(thumbnail_scroll_target(4, 10, true, Some(4)), None);
}

#[test]
fn a_hidden_or_empty_list_is_left_alone() {
    assert_eq!(thumbnail_scroll_target(4, 10, false, Some(1)), None);
    assert_eq!(thumbnail_scroll_target(0, 0, true, None), None);
}

#[test]
fn pages_beyond_the_generated_limit_keep_the_last_preview_in_view() {
    // Only the first 80 thumbnails are generated; page 120 maps to the last.
    assert_eq!(thumbnail_scroll_target(120, 80, true, Some(1)), Some(79));
    assert_eq!(thumbnail_scroll_target(79, 80, true, Some(1)), Some(79));
}
