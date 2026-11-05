//! Persisted reader preferences. This module owns validation and on-disk
//! format only; applying a change to a live reader lives in `settings_view`.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::export::ExportFormat;

const SETTINGS_FILE: &str = "settings.json";
const CONFIG_DIR: &str = "vofdreader";

/// Shared by the cache slider state, its ticks and stored-value validation.
pub const CACHE_MIN_MIB: usize = 64;
pub const CACHE_MAX_MIB: usize = 2048;

pub fn cache_fraction(mib: usize) -> f32 {
    let value = mib.clamp(CACHE_MIN_MIB, CACHE_MAX_MIB) as f32;
    (value - CACHE_MIN_MIB as f32) / (CACHE_MAX_MIB - CACHE_MIN_MIB) as f32
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub general: GeneralSettings,
    pub appearance: AppearanceSettings,
    pub reading: ReadingSettings,
    pub performance: PerformanceSettings,
    pub files: FileSettings,
    /// Last page per canonical document path. Written on document switch and
    /// application exit, not on every scroll frame.
    pub positions: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GeneralSettings {
    pub restore_last_file: bool,
    pub last_file: Option<String>,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            restore_last_file: true,
            last_file: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AppearanceSettings {
    pub theme: ThemeChoice,
    pub sidebar_open: bool,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::System,
            sidebar_open: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ReadingSettings {
    pub default_view: ViewChoice,
    pub page_gap: GapChoice,
    pub restore_position: bool,
    pub floating_toolbar: bool,
    pub dot_grid: bool,
    pub page_shadows: bool,
    pub search_auto_scroll: bool,
}

impl Default for ReadingSettings {
    fn default() -> Self {
        Self {
            default_view: ViewChoice::Continuous,
            page_gap: GapChoice::Standard,
            restore_position: true,
            floating_toolbar: true,
            dot_grid: true,
            page_shadows: true,
            search_auto_scroll: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PerformanceSettings {
    pub cache_mib: usize,
    pub page_limit: usize,
    pub parsed_page_limit: usize,
    pub prefetch_pages: usize,
    pub tile_ring: u32,
    pub quality: QualityChoice,
    pub max_page_dpi: u32,
    pub max_tile_dpi: u32,
    /// Parallel render jobs; 0 means automatic (see `tiles::concurrency`).
    pub render_threads: usize,
}

impl Default for PerformanceSettings {
    fn default() -> Self {
        Self {
            cache_mib: 256,
            page_limit: 32,
            parsed_page_limit: 12,
            prefetch_pages: 2,
            tile_ring: 1,
            quality: QualityChoice::Auto,
            max_page_dpi: 384,
            max_tile_dpi: 768,
            render_threads: 0,
        }
    }
}

impl PerformanceSettings {
    /// Effective caps, not the stored custom values. Presets stay independent
    /// of the advanced values so switching back to Custom restores them.
    pub fn page_dpi_cap(&self) -> u32 {
        match self.quality {
            QualityChoice::Auto => 384,
            QualityChoice::Standard => 192,
            QualityChoice::High => 288,
            QualityChoice::Custom => self.max_page_dpi,
        }
    }

    pub fn tile_dpi_cap(&self) -> u32 {
        match self.quality {
            QualityChoice::Auto => 768,
            QualityChoice::Standard => 384,
            QualityChoice::High => 576,
            QualityChoice::Custom => self.max_tile_dpi,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FileSettings {
    pub export_format: ExportFormat,
    pub export_dpi: f32,
    pub recent_limit: usize,
}

impl Default for FileSettings {
    fn default() -> Self {
        Self {
            export_format: ExportFormat::Png,
            export_dpi: 144.0,
            recent_limit: 10,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub fn resolve(self, system_dark: bool) -> bool {
        match self {
            Self::System => system_dark,
            Self::Light => false,
            Self::Dark => true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewChoice {
    Single,
    #[default]
    Continuous,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapChoice {
    Compact,
    #[default]
    Standard,
    Wide,
}

impl GapChoice {
    pub fn pixels(self) -> f32 {
        match self {
            Self::Compact => 10.0,
            Self::Standard => 20.0,
            Self::Wide => 32.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityChoice {
    #[default]
    Auto,
    Standard,
    High,
    Custom,
}

/// A user action from the settings window. Kept copyable so click handlers do
/// not need to clone model data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsAction {
    ToggleRestoreLastFile,
    SetTheme(ThemeChoice),
    ToggleSidebarDefault,
    SetDefaultView(ViewChoice),
    SetPageGap(GapChoice),
    ToggleRestorePosition,
    ToggleFloatingToolbar,
    ToggleDotGrid,
    TogglePageShadows,
    ToggleSearchAutoScroll,
    SetQuality(QualityChoice),
    AdjustPageLimit(i32),
    AdjustParsedLimit(i32),
    AdjustPrefetchPages(i32),
    AdjustTileRing(i32),
    AdjustRenderThreads(i32),
    AdjustPageDpi(i32),
    AdjustTileDpi(i32),
    SetExportFormat(ExportFormat),
    AdjustExportDpi(i32),
    AdjustRecentLimit(i32),
    ClearRecent,
    ClearThumbnails,
    ResetDefaults,
}

impl Settings {
    pub fn load() -> Self {
        Self::path()
            .map(|path| Self::load_from(&path))
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(path) = Self::path() {
            self.save_to(&path);
        }
    }

    pub fn path() -> Option<PathBuf> {
        crate::recent::config_dir().map(|dir| dir.join(CONFIG_DIR).join(SETTINGS_FILE))
    }

    pub(crate) fn load_from(path: &Path) -> Self {
        let Ok(contents) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let mut settings: Self = serde_json::from_str(&contents).unwrap_or_default();
        settings.normalize();
        settings
    }

    pub(crate) fn save_to(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let Ok(contents) = serde_json::to_vec_pretty(self) else {
            return;
        };
        let _ = std::fs::write(path, contents);
    }

    pub fn normalize(&mut self) {
        self.files.recent_limit = self.files.recent_limit.clamp(3, 50);
        self.files.export_dpi = self.files.export_dpi.clamp(36.0, 600.0);
        self.performance.cache_mib = self
            .performance
            .cache_mib
            .clamp(CACHE_MIN_MIB, CACHE_MAX_MIB);
        self.performance.page_limit = self.performance.page_limit.clamp(4, 256);
        self.performance.parsed_page_limit = self.performance.parsed_page_limit.clamp(1, 64);
        self.performance.prefetch_pages = self.performance.prefetch_pages.min(4);
        self.performance.tile_ring = self.performance.tile_ring.min(2);
        self.performance.render_threads = self
            .performance
            .render_threads
            .min(crate::tiles::MAX_THREADS);
        self.performance.max_page_dpi = quantize(self.performance.max_page_dpi, 96, 768);
        self.performance.max_tile_dpi = quantize(self.performance.max_tile_dpi, 96, 1536);
    }

    /// Drop a stale last-file reference without touching other preferences.
    pub fn last_file_path(&self) -> Option<PathBuf> {
        self.general
            .last_file
            .as_ref()
            .map(PathBuf::from)
            .filter(|path| path.exists())
    }

    pub fn position(&self, path: &Path) -> Option<usize> {
        self.positions
            .get(&path.to_string_lossy().to_string())
            .copied()
    }

    pub fn remember_position(&mut self, path: &Path, page: usize) {
        self.positions
            .insert(path.to_string_lossy().to_string(), page);
    }
}

fn quantize(value: u32, min: u32, max: u32) -> u32 {
    let value = value.clamp(min, max);
    (value.saturating_add(12) / 24 * 24).clamp(min, max)
}

#[cfg(test)]
mod tests {
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
    fn presets_override_custom_caps_independently() {
        let mut performance = PerformanceSettings {
            quality: QualityChoice::Custom,
            max_page_dpi: 432,
            max_tile_dpi: 984,
            ..PerformanceSettings::default()
        };
        assert_eq!(
            (performance.page_dpi_cap(), performance.tile_dpi_cap()),
            (432, 984)
        );
        performance.quality = QualityChoice::Standard;
        assert_eq!(
            (performance.page_dpi_cap(), performance.tile_dpi_cap()),
            (192, 384)
        );
        performance.quality = QualityChoice::High;
        assert_eq!(
            (performance.page_dpi_cap(), performance.tile_dpi_cap()),
            (288, 576)
        );
        performance.quality = QualityChoice::Auto;
        assert_eq!(
            (performance.page_dpi_cap(), performance.tile_dpi_cap()),
            (384, 768)
        );
    }
}
