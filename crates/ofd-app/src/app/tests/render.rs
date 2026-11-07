use crate::app::render_policy::frame_is_current;

#[test]
fn stale_or_wrong_page_frames_are_dropped() {
    assert!(!frame_is_current(false, true, 216, 168, true));
    assert!(!frame_is_current(true, false, 216, 168, true));
}

#[test]
fn missing_image_accepts_the_latest_frame_even_below_stale_dpi() {
    assert!(frame_is_current(true, true, 104, 384, false));
}

#[test]
fn sharper_current_frame_replaces_preview() {
    assert!(frame_is_current(true, true, 216, 168, true));
    assert!(frame_is_current(true, true, 168, 168, true));
}

#[test]
fn older_density_never_replaces_a_sharper_image() {
    assert!(!frame_is_current(true, true, 144, 168, true));
}
