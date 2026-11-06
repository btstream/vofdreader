use super::*;
#[test]
fn visible_tiles_on_all_pages_outrank_nearby_tiles_and_cached_return_trip_reuses_keys() {
    let a = Key {
        page: 0,
        dpi: 384,
        x: 1,
        y: 1,
    };
    let b = Key { page: 1, ..a };
    let neighbor = Key { y: 2, ..a };
    let planned = requests(
        vec![b, a],
        vec![(0, neighbor), (1, a), (2, neighbor)],
        0,
        64 * 1024 * 1024,
    );
    assert_eq!(planned, vec![a, b, neighbor]);
    let cached = std::collections::HashSet::from([a, b, neighbor]);
    // A neighbouring block that becomes visible keeps its exact cache key.
    let returned = requests(vec![neighbor], vec![(0, a)], 0, 64 * 1024 * 1024);
    assert!(returned.iter().all(|key| cached.contains(key)));
}
#[test]
fn nearby_ring_is_bounded_deduplicated_and_excludes_visible_tiles() {
    let center = Key {
        page: 2,
        dpi: 384,
        x: 1,
        y: 1,
    };
    let nearby = neighbors(&[center], (2048, 2048), 1);
    assert_eq!(nearby.len(), 8);
    assert!(!nearby.contains(&center));
    assert!(nearby.iter().all(|key| key.page == 2 && key.dpi == 384));
    let edge = Key {
        x: 0,
        y: 0,
        ..center
    };
    assert_eq!(neighbors(&[edge], (2048, 2048), 1).len(), 3);
    let visible = [center, Key { x: 2, ..center }];
    let nearby = neighbors(&visible, (2048, 2048), 1);
    assert_eq!(nearby.len(), 10);
    assert!(nearby.iter().all(|key| !visible.contains(key)));
    assert_eq!(
        nearby
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        nearby.len()
    );
    assert!(neighbors(&visible, (2048, 2048), 0).is_empty());
    assert!(neighbors(&[], (2048, 2048), 1).is_empty());
}

#[test]
fn speculative_tiles_leave_room_for_visible_working_set() {
    let budget = 64 * 1024 * 1024;
    let limit = prefetch_limit(budget, 32);
    assert!(limit * MAX_TILE_BYTES <= budget / 4);
    assert!((limit + 32) * MAX_TILE_BYTES <= budget);
    assert_eq!(prefetch_limit(budget, usize::MAX), 0);
    assert_eq!(prefetch_limit(0, 0), 0);
}
#[test]
fn boundaries_gutters_and_horizontal_clip() {
    let keys = plan(3, 384, (1100, 800), [512.0, 0.0, 1100.0, 512.0]);
    assert_eq!(keys.len(), 2);
    assert_eq!((keys[0].x, keys[1].x), (1, 2));
    let (a, sample) = keys[0].regions(1100, 800);
    let (b, _) = keys[1].regions(1100, 800);
    assert_eq!(a.x + a.width, b.x);
    assert_eq!(b.width, 76);
    assert_eq!((sample.x, sample.width), (510, 516));
    assert!(plan(0, 96, (100, 100), [101.0, 0.0, 200.0, 100.0]).is_empty());
    assert!(plan(0, 96, (100, 100), [0.0, f32::NAN, 2.0, 2.0]).is_empty());
}
#[test]
fn tile_concurrency_auto_uses_half_the_cores_within_bounds() {
    assert_eq!(resolve(0, 1), 1);
    assert_eq!(resolve(0, 2), 1);
    assert_eq!(resolve(0, 4), 2);
    assert_eq!(resolve(0, 8), 4);
    assert_eq!(resolve(0, 64), 4);
    assert!((1..=4).contains(&concurrency(0)));
}
#[test]
fn manual_thread_count_is_bounded_by_cores() {
    assert_eq!(resolve(1, 8), 1);
    assert_eq!(resolve(6, 8), 6);
    assert_eq!(resolve(12, 8), 8);
    assert_eq!(resolve(3, 1), 1);
    assert_eq!(resolve(32, 64), MAX_THREADS);
    assert!(concurrency(usize::MAX) <= max_threads());
}
#[test]
fn complete_grid_covers_each_pixel_once() {
    let keys = plan(0, 192, (1100, 800), [0.0, 0.0, 1100.0, 800.0]);
    let area: u32 = keys
        .iter()
        .map(|key| {
            let (r, _) = key.regions(1100, 800);
            r.width * r.height
        })
        .sum();
    assert_eq!(area, 1100 * 800);
    assert_eq!(hard_dpi(192, 1.0), 192);
    assert_eq!(hard_dpi(192, 1.5), 288);
    assert_eq!(hard_dpi(192, 0.5), 96);
    assert_eq!(hard_dpi(384, 1.1), 432);
    assert_eq!(hard_dpi(384, 8.0), MAX_DPI);
}
