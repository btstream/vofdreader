use crate::app::active_document::ActiveDocument;
use crate::app::reading::{ReadingSnapshot, ViewMode};
use crate::app::search_state::SearchState;
use crate::app::selection_state::SelectionState;

#[test]
fn close_clears_identity_and_bumps_the_generation() {
    let mut active = ActiveDocument::empty();
    active.path = Some("/tmp/doc.ofd".into());
    active.file_name = "doc.ofd".to_string();
    active.file_size = 42;
    active.page_sizes = vec![ofd_core::SizeMm::new(210.0, 297.0)];
    let before = active.generation;

    active.close();

    assert!(active.path.is_none());
    assert!(active.file_name.is_empty());
    assert_eq!(active.file_size, 0);
    assert!(active.page_sizes.is_empty());
    assert!(!active.is_open());
    assert_eq!(active.page_count(), 0);
    assert_eq!(active.generation, before + 1);
}

#[test]
fn failed_open_keeps_the_attempted_path_but_drops_page_state() {
    let mut active = ActiveDocument::empty();
    active.path = Some("/tmp/broken.ofd".into());
    active.file_name = "broken.ofd".to_string();
    active.file_size = 99;
    active.page_sizes = vec![ofd_core::SizeMm::new(210.0, 297.0); 3];
    active.loading = true;
    let before = active.generation;

    active.fail("无法打开".to_string());

    assert_eq!(
        active.path.as_deref(),
        Some(std::path::Path::new("/tmp/broken.ofd"))
    );
    assert_eq!(active.file_name, "broken.ofd");
    assert_eq!(active.file_size, 0);
    assert!(active.page_sizes.is_empty());
    assert!(!active.loading);
    assert_eq!(active.error.as_deref(), Some("无法打开"));
    assert_eq!(active.generation, before + 1);
}

#[test]
fn every_document_transition_invalidates_in_flight_results() {
    let mut active = ActiveDocument::empty();
    let first = active.next_generation();
    let second = active.next_generation();
    assert!(second > first);

    active.close();
    let closed = active.generation;
    active.fail("error".to_string());
    assert!(active.generation > closed);
}

#[test]
fn search_snapshot_round_trips_and_close_drops_the_query() {
    let mut search = SearchState::new();
    search.open = true;
    search.query = "发票".to_string();
    search.index = 2;

    let snapshot = search.snapshot();
    let mut restored = SearchState::new();
    restored.apply_snapshot(&snapshot);
    assert!(!restored.open || restored.query == "发票");
    assert_eq!(restored.query, "发票");
    assert_eq!(restored.index, 2);

    restored.clear_results();
    assert_eq!(restored.query, "发票");
    assert_eq!(restored.index, 0);

    restored.close();
    assert!(!restored.open);
    assert!(restored.query.is_empty());
}

#[test]
fn selection_clear_resets_every_range_and_derived_rectangle() {
    let mut selection = SelectionState::new();
    selection.anchor = Some(1);
    selection.focus = Some(4);
    selection.rects.push(crate::search_highlight::Highlight {
        x: 0.0,
        y: 0.0,
        width: 0.1,
        height: 0.1,
    });
    selection.text = "文字".to_string();
    selection.dragging = true;
    selection.moved = true;

    selection.clear();

    assert!(selection.is_empty());
    assert!(selection.rects.is_empty());
    assert!(selection.text.is_empty());
    assert!(!selection.dragging);
    assert!(!selection.moved);
}

#[test]
fn reading_snapshot_defaults_match_a_fresh_reader() {
    let reading = ReadingSnapshot::default();
    assert_eq!(reading.page, 0);
    assert_eq!(reading.zoom, 1.0);
    assert_eq!(reading.rotation, 0);
    assert_eq!(reading.view_mode, ViewMode::Continuous);
    assert_eq!(reading.page_offset, 0.0);
}
