use super::*;
#[test]
fn prefetch_defaults_can_be_disabled_or_bounded_by_future_settings() {
    let config = CacheConfig::default();
    assert_eq!(
        (config.prefetch_pages(), config.prefetch_tile_ring()),
        (2, 1)
    );
    let disabled = config.with_prefetch(0, 0);
    assert_eq!(
        (disabled.prefetch_pages(), disabled.prefetch_tile_ring()),
        (0, 0)
    );
    let bounded = config.with_prefetch(usize::MAX, u32::MAX);
    assert_eq!(
        (bounded.prefetch_pages(), bounded.prefetch_tile_ring()),
        (4, 2)
    );
}
#[test]
fn larger_cache_retains_history_without_increasing_render_density_budget() {
    let old = CacheConfig::new(128, 8, 4);
    let new = CacheConfig::default();
    assert_eq!(new.page_bytes(), 192 * MIB);
    assert_eq!(new.tile_bytes(), 64 * MIB);
    assert_eq!(new.tile_working_bytes(), old.tile_working_bytes());
    assert_eq!(new.page_limit(), 32);
    // A4 96-DPI previews (~3.4 MiB) used to hit the 8-page count limit
    // even with unused byte budget; a return trip over 24 pages now fits.
    assert!(24 <= new.page_limit());
    assert!(24 * 4 * MIB < new.page_bytes());
}
#[test]
fn user_policy_boundary_clamps_extreme_values() {
    for input in [0, 1, 128, 256, usize::MAX] {
        let config = CacheConfig::new(input, input, input);
        assert!((64 * MIB..=2048 * MIB).contains(&config.total_bytes));
        assert_eq!(
            config.page_bytes() + config.tile_bytes(),
            config.total_bytes
        );
        assert!((4..=256).contains(&config.page_limit()));
        assert!((1..=64).contains(&config.parsed_page_limit()));
        assert!(config.tile_working_bytes() <= config.tile_bytes());
    }
}
