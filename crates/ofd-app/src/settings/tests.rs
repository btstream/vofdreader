use super::*;

#[test]
fn round_trips_json_and_keeps_positions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let mut settings = Settings::default();
    settings.appearance.theme = ThemeChoice::Dark;
    settings.reading.page_gap = GapChoice::Wide;
    settings.performance.quality = QualityChoice::Custom;
    settings.performance.max_page_dpi = 432;
    settings.remember_position(Path::new("/tmp/a.ofd"), 7);
    settings.save_to(&path);
    let loaded = Settings::load_from(&path);
    assert_eq!(loaded, settings);
    assert_eq!(loaded.position(Path::new("/tmp/a.ofd")), Some(7));
}

#[test]
fn new_documents_open_without_the_sidebar_by_default() {
    let settings = Settings::default();
    assert!(!settings.appearance.sidebar_open);
}

#[test]
fn missing_or_corrupt_file_falls_back_to_defaults() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        Settings::load_from(&dir.path().join("missing.json")),
        Settings::default()
    );
    let path = dir.path().join("broken.json");
    std::fs::write(&path, "{not json").unwrap();
    assert_eq!(Settings::load_from(&path), Settings::default());
}

#[test]
fn last_file_is_only_used_while_it_still_exists() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.ofd");
    std::fs::write(&file, b"x").unwrap();
    let mut settings = Settings::default();
    settings.general.last_file = Some(file.to_string_lossy().into_owned());
    assert_eq!(settings.last_file_path(), Some(file.clone()));
    std::fs::remove_file(file).unwrap();
    assert_eq!(settings.last_file_path(), None);
}

#[test]
fn out_of_range_values_are_clamped_and_dpi_is_quantized() {
    let mut settings = Settings::default();
    settings.performance.cache_mib = 1;
    settings.performance.page_limit = 9999;
    settings.performance.max_page_dpi = 1000;
    settings.performance.max_tile_dpi = 10;
    settings.performance.render_threads = 999;
    settings.files.recent_limit = 0;
    settings.normalize();
    assert_eq!(settings.performance.render_threads, 16);
    assert_eq!(settings.performance.cache_mib, 64);
    assert_eq!(settings.performance.page_limit, 256);
    assert_eq!(settings.performance.max_page_dpi, 768);
    assert_eq!(settings.performance.max_tile_dpi, 96);
    assert_eq!(settings.files.recent_limit, 3);
}

#[test]
fn cache_ticks_use_the_real_linear_scale() {
    assert_eq!(cache_fraction(CACHE_MIN_MIB), 0.0);
    assert_eq!(cache_fraction(CACHE_MAX_MIB), 1.0);
    assert!((cache_fraction(512) - 0.2258).abs() < 0.001);
    assert!((cache_fraction(1024) - 0.4839).abs() < 0.001);
    // Values outside the slider still map to its ends, never past them.
    assert_eq!(cache_fraction(1), 0.0);
    assert_eq!(cache_fraction(9999), 1.0);
}

#[test]
fn image_sampling_defaults_to_bicubic_and_round_trips() {
    // Settings files written before the option existed.
    let old: PerformanceSettings = serde_json::from_str(r#"{"cache_mib": 512}"#).unwrap();
    assert_eq!(old.image_sampling, ImageSampling::Bicubic);
    for sampling in [
        ImageSampling::Nearest,
        ImageSampling::Bilinear,
        ImageSampling::Bicubic,
    ] {
        let performance = PerformanceSettings {
            image_sampling: sampling,
            ..PerformanceSettings::default()
        };
        let json = serde_json::to_string(&performance).unwrap();
        let back: PerformanceSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.image_sampling, sampling);
    }
    assert_eq!(
        ImageSampling::Bilinear.filter(),
        ofd_core::ImageFilter::Bilinear
    );
}

#[test]
fn presets_override_custom_caps_independently() {
    let mut performance = PerformanceSettings {
        quality: QualityChoice::Custom,
        max_page_dpi: 432,
        max_tile_dpi: 984,
        ..PerformanceSettings::default()
    };
    assert_eq!(performance.page_dpi_cap(), 432);
    performance.quality = QualityChoice::Standard;
    assert_eq!(performance.page_dpi_cap(), 192);
    performance.quality = QualityChoice::High;
    assert_eq!(performance.page_dpi_cap(), 288);
    performance.quality = QualityChoice::Auto;
    assert_eq!(performance.page_dpi_cap(), 384);
}
