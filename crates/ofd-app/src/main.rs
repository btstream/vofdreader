use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use export::{ExportFormat, suggested_name_for};
use gpui_kit::FontFallbacks;
use gpui_kit::assets::IconName;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use ofd_core::{OfdDocument, PageRenderer, PageTextLayout, SizeMm};

mod assets;
mod cache_config;
mod continuous;
mod export;
mod font_policy;
mod gpu;
mod recent;
mod rotate;
mod search_highlight;
mod settings;
mod settings_view;
mod tiles;

/// Colors for one appearance mode. Kept separate from the component theme so
/// the reader keeps its own neutral palette while buttons and dialogs follow it.
#[derive(Clone, Copy)]
struct Palette {
    bg: u32,
    titlebar: u32,
    toolbar: u32,
    surface: u32,
    canvas: u32,
    sidebar: u32,
    border: u32,
    border_strong: u32,
    text: u32,
    text_secondary: u32,
    text_tertiary: u32,
    accent: u32,
    accent_soft: u32,
    primary_hover: u32,
    primary_active: u32,
    window_control_hover: u32,
    window_control_active: u32,
    window_close_hover: u32,
    window_close_active: u32,
    search_match: u32,
    warning: u32,
    error: u32,
    error_border: u32,
    error_soft: u32,
    dot: u32,
}

impl Palette {
    const LIGHT: Self = Self {
        bg: 0xF7F8FA,
        titlebar: 0xFBFBFC,
        toolbar: 0xFFFFFF,
        surface: 0xFFFFFF,
        canvas: 0xE9EBEF,
        sidebar: 0xF6F7F9,
        border: 0xE5E7EB,
        border_strong: 0xE3E6EB,
        text: 0x1B1E25,
        text_secondary: 0x5B6472,
        text_tertiary: 0x98A1AF,
        accent: 0x2F6FEB,
        accent_soft: 0xEDF2FE,
        primary_hover: 0x2560D4,
        primary_active: 0x1F4FB5,
        window_control_hover: 0xEDEFF2,
        window_control_active: 0xE1E4E9,
        window_close_hover: 0xE5484D,
        window_close_active: 0xC93A3F,
        search_match: 0xB45309,
        warning: 0xB45309,
        error: 0xB42318,
        error_border: 0xFCA5A5,
        error_soft: 0xFEF2F2,
        dot: 0xD5DAE2,
    };

    const DARK: Self = Self {
        bg: 0x17191E,
        titlebar: 0x17191E,
        toolbar: 0x1A1D23,
        surface: 0x1C1F26,
        canvas: 0x0F1217,
        sidebar: 0x17191E,
        border: 0x262A31,
        border_strong: 0x363C47,
        text: 0xE7EAF0,
        text_secondary: 0xA7B0BF,
        text_tertiary: 0x6F7A8B,
        accent: 0x3B76F0,
        accent_soft: 0x202A3D,
        primary_hover: 0x4B85F5,
        primary_active: 0x2F63D8,
        window_control_hover: 0x23262D,
        window_control_active: 0x2C3038,
        window_close_hover: 0xDC454B,
        window_close_active: 0xB83439,
        search_match: 0xFFD666,
        warning: 0xE0A64A,
        error: 0xFF7A7A,
        error_border: 0x7F3B3B,
        error_soft: 0x2A1B1D,
        dot: 0x3A424F,
    };

    fn for_mode(dark: bool) -> Self {
        if dark { Self::DARK } else { Self::LIGHT }
    }
}

/// Desktop entry name (packaging/linux/vofdreader.desktop) and the window
/// app_id / WM_CLASS that lets the desktop find the application icon.
const APP_ID: &str = "vofdreader";
const THUMB_LIMIT: usize = 80;
const SEARCH_LIMIT: usize = 120;
const THUMB_DPI: f32 = 48.0;
/// Hides the floating zoom bar after this much pointer inactivity.
const BAR_HIDE_DELAY: Duration = Duration::from_millis(2500);
const CONTINUOUS_IDLE: Duration = Duration::from_millis(160);
/// Search matches on the page, and the one currently selected in the sidebar.
const SEARCH_HIGHLIGHT: u32 = 0xFFD54A80;
const SEARCH_ACTIVE_HIGHLIGHT: u32 = 0xFF8A3D99;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Single,
    Continuous,
}

#[derive(Clone)]
struct SearchHit {
    page: usize,
    /// Ordinal of this match within the page's `page_search_rects` list.
    match_index: usize,
    snippet: String,
}

/// Lightweight state kept for inactive tabs. Rendering resources are rebuilt
/// from this snapshot when the tab becomes active again.
#[derive(Clone)]
struct DocumentTab {
    path: PathBuf,
    file_name: String,
    page: usize,
    zoom: f32,
    rotation: u8,
    view_mode: ViewMode,
    search_open: bool,
    search_query: String,
    search_index: usize,
    /// Vertical position inside the current page, as a 0..1 fraction.
    page_offset: f32,
}

struct ReaderApp {
    doc: Option<Arc<OfdDocument>>,
    renderer: Option<Arc<PageRenderer>>,
    file_path: Option<PathBuf>,
    pending_initial: Vec<PathBuf>,
    tabs: Vec<DocumentTab>,
    active_tab: Option<usize>,
    pending_restore_scroll: Option<f32>,
    file_name: String,
    file_size: u64,
    sidebar_open: bool,
    page: usize,
    page_count: usize,
    page_sizes: Vec<SizeMm>,
    zoom: f32,
    page_image: Option<Arc<RenderImage>>,
    view_mode: ViewMode,
    continuous_scroll: ScrollHandle,
    continuous_pending: HashSet<(usize, u32)>,
    continuous_failed: HashSet<(usize, u32)>,
    continuous_layout: RefCell<continuous::LayoutCache>,
    page_sizes_revision: u64,
    continuous_last_scroll: Option<Instant>,
    /// Last continuous scroll offset and direction; prefetch goes ahead.
    last_scroll_y: f32,
    scroll_down: bool,
    continuous_settle_pending: bool,
    page_cache: HashMap<(usize, u32), Arc<RenderImage>>,
    font_reports: HashMap<usize, Arc<ofd_core::FontRenderReport>>,
    page_cache_order: VecDeque<(usize, u32)>,
    page_cache_bytes: usize,
    cache_config: cache_config::CacheConfig,
    gpu: Rc<RefCell<gpu::Resources>>,
    tile_cache: HashMap<tiles::Key, Arc<RenderImage>>,
    tile_order: VecDeque<tiles::Key>,
    tile_bytes: usize,
    /// In-flight tile jobs and their pixel bytes; at most `tile_slots`.
    tile_jobs: HashMap<tiles::Key, usize>,
    tile_slots: usize,
    tile_wanted: HashMap<usize, Vec<tiles::Key>>,
    tile_prefetch: HashMap<usize, Vec<tiles::Key>>,
    tile_failed: HashSet<tiles::Key>,
    page_coverage: Rc<RefCell<Vec<Bounds<Pixels>>>>,
    render_seq: u64,
    latest_render: Option<(u64, usize, u32)>,
    dpi_debounce_gen: u64,
    /// In-memory thumbnails (BGRA), registered with the GPU resources so a
    /// document switch releases their atlas entries.
    thumbs: Vec<Option<Arc<RenderImage>>>,
    thumbs_started: bool,
    search_open: bool,
    search_input: Option<Entity<InputState>>,
    search_query: String,
    search_hits: Vec<SearchHit>,
    search_index: usize,
    search_results_scroll: ScrollHandle,
    search_highlights: Vec<search_highlight::Highlight>,
    search_active_highlight: Option<search_highlight::Highlight>,
    search_suppress_change: bool,
    search_pending_index: Option<(String, usize)>,
    search_pending_input: Option<String>,
    page_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    text_layout: Option<PageTextLayout>,
    selection_anchor: Option<usize>,
    selection_focus: Option<usize>,
    selection_rects: Vec<search_highlight::Highlight>,
    selected_text: String,
    selecting: bool,
    selection_moved: bool,
    page_text_cache: Option<Vec<Vec<String>>>,
    recent: Vec<PathBuf>,
    error: Option<String>,
    notice: Option<(String, bool)>,
    notice_generation: u64,
    exporting: bool,
    loading: bool,
    doc_generation: u64,
    focus_handle: FocusHandle,
    focused: bool,
    viewport: Size<Pixels>,
    window_scale: f32,
    ui_font: font_policy::UiFont,
    image_dpi: u32,
    /// Clockwise quarter turns applied to the view, bitmaps and overlays.
    rotation: u8,
    /// Invalidates in-flight raster jobs when view orientation changes.
    view_generation: u64,
    grid_open: bool,
    grid_scroll: ScrollHandle,
    dpi_pending: bool,
    dark: bool,
    info_dialog_open: bool,
    settings: settings::Settings,
    settings_open: bool,
    settings_tab: settings_view::SettingsTab,
    settings_reset_armed: bool,
    cache_slider: Entity<SliderState>,
    _cache_slider_subscription: Subscription,
    pending_restore_page: Option<usize>,
    appearance_subscription: Option<Subscription>,
    bar_visible: bool,
    bar_hovered: bool,
    bar_last_activity: Instant,
    bar_watchdog: bool,
    bar_pointer: Option<Point<Pixels>>,
    reader_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    wheel_accum: f32,
    wheel_last: Instant,
}

impl ReaderApp {
    fn palette(&self) -> Palette {
        Palette::for_mode(self.dark)
    }

    fn save_position(&mut self) {
        if let Some(path) = &self.file_path {
            self.settings.general.last_file = Some(path.to_string_lossy().into_owned());
            self.settings.remember_position(path, self.page);
        }
    }

    /// Stores the active reader state in its tab before the document is released.
    fn snapshot_active_tab(&mut self) {
        let Some(index) = self.active_tab else {
            return;
        };
        if index >= self.tabs.len() {
            return;
        }
        let page_offset = self.current_page_offset();
        let tab = &mut self.tabs[index];
        tab.search_open = self.search_open;
        tab.search_query = self.search_query.clone();
        tab.search_index = self.search_index;
        if self.doc.is_none() {
            return;
        }
        tab.page = self.page;
        tab.zoom = self.zoom;
        tab.rotation = self.rotation;
        tab.view_mode = self.view_mode;
        tab.page_offset = page_offset;
        tab.file_name = self.file_name.clone();
        if let Some(path) = &self.file_path {
            tab.path = path.clone();
        }
    }

    /// Fraction of the current page that is scrolled above the viewport.
    fn current_page_offset(&self) -> f32 {
        if self.view_mode != ViewMode::Continuous || self.page_count == 0 {
            return 0.0;
        }
        let viewport = self.viewport;
        let (top, height) = self.with_continuous_layout(viewport, |layout| {
            (
                layout.top(self.page),
                layout.sizes.get(self.page).map_or(0.0, |size| size.1),
            )
        });
        if height <= 0.0 {
            return 0.0;
        }
        let scroll = -f32::from(self.continuous_scroll.offset().y);
        ((scroll - top) / height).clamp(0.0, 1.0)
    }

    /// Releases every rendering resource owned by the active tab, then clears
    /// the document state. Inactive tabs keep only their `DocumentTab` snapshot.
    fn clear_active_document(&mut self, cx: &mut Context<Self>) {
        self.doc_generation = self.doc_generation.wrapping_add(1);
        let gpu = self.gpu.clone();
        gpu.borrow_mut()
            .release_all(|image| cx.drop_image(image, None));
        self.doc = None;
        self.renderer = None;
        self.page_image = None;
        self.image_dpi = 0;
        self.page_cache.clear();
        self.page_cache_order.clear();
        self.page_cache_bytes = 0;
        self.clear_tiles();
        self.tile_order.clear();
        self.tile_bytes = 0;
        self.font_reports.clear();
        self.continuous_pending.clear();
        self.continuous_failed.clear();
        self.continuous_last_scroll = None;
        self.continuous_settle_pending = false;
        self.continuous_scroll.set_offset(point(px(0.0), px(0.0)));
        self.continuous_layout = RefCell::new(continuous::LayoutCache::default());
        self.page_sizes_revision = self.page_sizes_revision.wrapping_add(1);
        self.render_seq = self.render_seq.wrapping_add(1);
        self.latest_render = None;
        self.thumbs.clear();
        self.thumbs_started = false;
        self.page_text_cache = None;
        self.text_layout = None;
        self.page_bounds.set(None);
        self.clear_selection();
        self.search_highlights.clear();
        self.search_active_highlight = None;
        self.search_hits.clear();
        self.search_index = 0;
        self.search_open = false;
        self.search_query.clear();
        self.search_pending_index = None;
        self.search_pending_input = None;
        self.pending_restore_page = None;
        self.pending_restore_scroll = None;
        self.page_count = 0;
        self.page_sizes.clear();
        self.loading = false;
        self.error = None;
        self.notice = None;
        self.exporting = false;
        self.grid_open = false;
        self.info_dialog_open = false;
        self.bar_visible = false;
        self.bar_hovered = false;
    }

    fn activate_tab(&mut self, index: usize, window: Option<&mut Window>, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        if self.active_tab == Some(index) && self.doc.is_some() {
            return;
        }
        if self.active_tab != Some(index) {
            self.snapshot_active_tab();
        }
        self.load_tab(index, window, cx);
    }

    fn max_page_dpi(&self) -> u32 {
        self.settings.performance.page_dpi_cap()
    }

    fn new(
        initial: Vec<PathBuf>,
        viewport: Size<Pixels>,
        window_scale: f32,
        ui_font: font_policy::UiFont,
        settings: settings::Settings,
        dark: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let cache_config = settings_cache_config(&settings);
        let cache_slider = cx.new(|_| {
            SliderState::new()
                .min(settings::CACHE_MIN_MIB as f32)
                .max(settings::CACHE_MAX_MIB as f32)
                .step(64.0)
                .default_value(settings.performance.cache_mib as f32)
        });
        let cache_slider_subscription = cx.subscribe(&cache_slider, |this, _, event, cx| {
            let (value, persist) = match event {
                SliderEvent::Change(value) => (value.end(), false),
                SliderEvent::Release(value) => (value.end(), true),
            };
            this.settings.performance.cache_mib =
                (value.round() as usize).clamp(settings::CACHE_MIN_MIB, settings::CACHE_MAX_MIB);
            this.cache_config = settings_cache_config(&this.settings);
            if persist {
                this.settings.save();
            }
            cx.notify();
        });

        cx.on_release(|this, cx| {
            this.save_position();
            this.settings.save();
            this.gpu
                .borrow_mut()
                .release_all(|image| cx.drop_image(image, None));
        })
        .detach();

        // Opt-in, fixed redraw workload for shadow A/B measurement. No input
        // injection, scrolling or extra timers in ordinary reader sessions.
        if let Some(frames) = std::env::var("OFD_GPU_BENCH_FRAMES")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|n| *n > 0 && *n <= 10000)
        {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                let mut warmed = false;
                for _ in 0..100 {
                    let ready = this.update(cx, |this, _| {
                        this.doc.is_some()
                            && !this.loading
                            && this.latest_render.is_none()
                            && this.continuous_pending.is_empty()
                            && !this.tile_busy()
                            && this.gpu.borrow().stats.page_draws > 0
                            && this.thumbs.iter().take(THUMB_LIMIT).all(Option::is_some)
                    });
                    if matches!(ready, Ok(true)) {
                        warmed = true;
                        break;
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(100))
                        .await;
                }
                if !warmed {
                    eprintln!("[gpu-bench] invalid: page not painted/ready; keep window visible");
                    return;
                }
                let baseline = this
                    .update(cx, |this, _| {
                        let s = &this.gpu.borrow().stats;
                        (s.frames, s.page_draws, s.first_uses)
                    })
                    .unwrap_or_default();
                let timestamp = || {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs_f64()
                };
                let start = Instant::now();
                eprintln!(
                    "[gpu-bench] begin unix_s={:.6} requested_frames={frames}",
                    timestamp()
                );
                for _ in 0..frames {
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        return;
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(16))
                        .await;
                }
                eprintln!(
                    "[gpu-bench] end unix_s={:.6} elapsed_s={:.6}",
                    timestamp(),
                    start.elapsed().as_secs_f64()
                );
                let _ = this.update(cx, |this, _| {
                    let s = &this.gpu.borrow().stats;
                    eprintln!(
                        "[gpu-bench] totals frames={} page_draws={} first_uses={}",
                        s.frames - baseline.0,
                        s.page_draws - baseline.1,
                        s.first_uses - baseline.2
                    );
                    if s.frames - baseline.0 < frames as u64 * 9 / 10 || s.first_uses != baseline.2
                    {
                        eprintln!(
                            "[gpu-bench] invalid: redraw count or texture working set changed"
                        );
                    }
                });
            })
            .detach();
        }

        let mut gpu_options = gpu::Options::from_env();
        gpu_options.no_dots |= !settings.reading.dot_grid;
        gpu_options.no_page_shadows |= !settings.reading.page_shadows;

        Self {
            doc: None,
            renderer: None,
            file_path: None,
            pending_initial: initial,
            tabs: Vec::new(),
            active_tab: None,
            pending_restore_scroll: None,
            file_name: String::new(),
            file_size: 0,
            sidebar_open: settings.appearance.sidebar_open,
            page: 0,
            page_count: 0,
            page_sizes: Vec::new(),
            zoom: 1.0,
            page_image: None,
            view_mode: match settings.reading.default_view {
                settings::ViewChoice::Single => ViewMode::Single,
                settings::ViewChoice::Continuous => ViewMode::Continuous,
            },
            continuous_scroll: ScrollHandle::new(),
            continuous_pending: HashSet::new(),
            continuous_failed: HashSet::new(),
            continuous_layout: RefCell::new(continuous::LayoutCache::default()),
            page_sizes_revision: 0,
            continuous_last_scroll: None,
            last_scroll_y: 0.0,
            scroll_down: true,
            continuous_settle_pending: false,
            page_cache: HashMap::new(),
            font_reports: HashMap::new(),
            page_cache_order: VecDeque::new(),
            page_cache_bytes: 0,
            cache_config,
            gpu: Rc::new(RefCell::new(gpu::Resources::new(gpu_options))),
            tile_cache: HashMap::new(),
            tile_order: VecDeque::new(),
            tile_bytes: 0,
            tile_jobs: HashMap::new(),
            tile_slots: tiles::concurrency(settings.performance.render_threads),
            tile_wanted: HashMap::new(),
            tile_prefetch: HashMap::new(),
            tile_failed: HashSet::new(),
            page_coverage: Rc::new(RefCell::new(Vec::new())),
            render_seq: 0,
            latest_render: None,
            dpi_debounce_gen: 0,
            thumbs: Vec::new(),
            thumbs_started: false,
            search_open: false,
            search_input: None,
            search_query: String::new(),
            search_hits: Vec::new(),
            search_index: 0,
            search_results_scroll: ScrollHandle::new(),
            search_highlights: Vec::new(),
            search_active_highlight: None,
            search_suppress_change: false,
            search_pending_index: None,
            search_pending_input: None,
            page_bounds: Rc::new(Cell::new(None)),
            text_layout: None,
            selection_anchor: None,
            selection_focus: None,
            selection_rects: Vec::new(),
            selected_text: String::new(),
            selecting: false,
            selection_moved: false,
            page_text_cache: None,
            recent: recent::load_recent(settings.files.recent_limit),
            error: None,
            notice: None,
            notice_generation: 0,
            exporting: false,
            loading: false,
            doc_generation: 0,
            focus_handle: cx.focus_handle(),
            focused: false,
            viewport,
            window_scale,
            ui_font,
            image_dpi: 0,
            rotation: 0,
            view_generation: 0,
            grid_open: false,
            grid_scroll: ScrollHandle::new(),
            dpi_pending: false,
            dark,
            info_dialog_open: false,
            settings,
            settings_open: false,
            settings_tab: settings_view::SettingsTab::Reading,
            settings_reset_armed: false,
            cache_slider,
            _cache_slider_subscription: cache_slider_subscription,
            pending_restore_page: None,
            appearance_subscription: None,
            bar_visible: false,
            bar_hovered: false,
            bar_last_activity: Instant::now(),
            bar_watchdog: false,
            bar_pointer: None,
            reader_bounds: Rc::new(Cell::new(None)),
            wheel_accum: 0.0,
            wheel_last: Instant::now(),
        }
    }

    fn open_path(&mut self, path: PathBuf, window: Option<&mut Window>, cx: &mut Context<Self>) {
        self.save_position();
        self.settings.save();
        // Recent files and the export directory should not depend on the
        // directory the application happened to start in.
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.activate_tab(index, window, cx);
            return;
        }
        self.snapshot_active_tab();
        let page = if self.settings.reading.restore_position {
            self.settings.position(&path).unwrap_or(0)
        } else {
            0
        };
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        self.tabs.push(DocumentTab {
            path,
            file_name,
            page,
            zoom: 1.0,
            rotation: 0,
            view_mode: match self.settings.reading.default_view {
                settings::ViewChoice::Single => ViewMode::Single,
                settings::ViewChoice::Continuous => ViewMode::Continuous,
            },
            search_open: false,
            search_query: String::new(),
            search_index: 0,
            page_offset: 0.0,
        });
        let index = self.tabs.len() - 1;
        self.load_tab(index, window, cx);
    }

    /// Releases the previous document, restores the tab's reading state and
    /// opens the file again through the ordinary single-document path.
    fn load_tab(&mut self, index: usize, window: Option<&mut Window>, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(index).cloned() else {
            return;
        };
        self.clear_active_document(cx);
        self.active_tab = Some(index);
        self.file_path = Some(tab.path.clone());
        self.file_name = tab.file_name;
        self.zoom = tab.zoom.max(0.1);
        self.rotation = tab.rotation % 4;
        self.view_mode = tab.view_mode;
        self.search_open = tab.search_open;
        self.search_query = tab.search_query.clone();
        self.search_index = tab.search_index;
        self.pending_restore_scroll = (tab.page_offset > 0.0).then_some(tab.page_offset);
        self.load_document(tab.path, tab.page, window, cx);
    }

    fn load_document(
        &mut self,
        path: PathBuf,
        target_page: usize,
        mut window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        match OfdDocument::open(&path) {
            Ok(doc) => {
                let doc = Arc::new(doc);
                self.file_path = Some(path.clone());
                self.file_size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                let recent_limit = self.settings.files.recent_limit;
                recent::push_recent(&mut self.recent, &path, recent_limit);
                recent::save_recent(&self.recent, recent_limit);
                self.page_count = doc.page_count();
                self.page = target_page.min(self.page_count.saturating_sub(1));
                self.pending_restore_page = (self.page_count > 0
                    && (self.page > 0 || self.pending_restore_scroll.is_some()))
                .then_some(self.page);
                let fallback = doc.meta.page_area.unwrap_or(SizeMm::new(210.0, 297.0));
                // Do not scan every page on the UI thread. The visible page is
                // resolved synchronously; the rest are filled in the background.
                self.page_sizes = vec![fallback; self.page_count];
                self.page_sizes_revision = self.page_sizes_revision.wrapping_add(1);
                if let Ok(Some(size)) = doc.page_size_only(self.page)
                    && self.page < self.page_sizes.len()
                {
                    self.page_sizes[self.page] = size;
                }
                self.renderer = None;
                self.doc = Some(doc.clone());
                self.doc_generation = self.doc_generation.wrapping_add(1);
                self.page_image = None;
                self.image_dpi = 0;
                self.page_cache.clear();
                self.clear_tiles();
                self.font_reports.clear();
                self.continuous_pending.clear();
                self.continuous_failed.clear();
                self.continuous_last_scroll = None;
                self.continuous_settle_pending = false;
                self.continuous_scroll.set_offset(point(px(0.0), px(0.0)));
                self.page_cache_order.clear();
                self.page_cache_bytes = 0;
                self.render_seq = self.render_seq.wrapping_add(1);
                self.latest_render = None;
                self.thumbs = vec![None; self.page_count];
                self.thumbs_started = false;
                self.page_text_cache = None;
                self.search_highlights.clear();
                self.search_active_highlight = None;
                self.clear_selection();
                self.grid_open = false;
                self.info_dialog_open = false;
                self.error = None;
                self.notice = None;
                self.loading = true;
                self.wheel_accum = 0.0;
                self.settings.general.last_file = Some(path.to_string_lossy().into_owned());
                self.settings.save();
                self.restore_tab_search(window.as_deref_mut(), cx);
                if let Some(window) = window {
                    if self.search_open {
                        if let Some(input) = &self.search_input {
                            input.update(cx, |state, cx| state.focus(window, cx));
                        }
                    } else {
                        window.focus(&self.focus_handle, cx);
                    }
                }

                // Font database construction is expensive; keep the UI thread
                // free so the window chrome and empty state paint immediately.
                let generation = self.doc_generation;
                let renderer_doc = doc.clone();
                let parsed_page_limit = self.cache_config.parsed_page_limit();
                let image_filter = self.settings.performance.image_sampling.filter();
                cx.spawn(async move |this, cx| {
                    let renderer = cx
                        .background_spawn(async move {
                            PageRenderer::with_tree_cache_limit(&renderer_doc, parsed_page_limit)
                                .with_image_filter(image_filter)
                        })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if this.doc_generation != generation {
                            return;
                        }
                        this.renderer = Some(Arc::new(renderer));
                        this.request_page_render(cx);
                        cx.notify();
                    });
                })
                .detach();

                let sizes_doc = doc.clone();
                let generation = self.doc_generation;
                cx.spawn(async move |this, cx| {
                    let sizes = cx
                        .background_spawn(async move {
                            (0..sizes_doc.page_count())
                                .map(|index| sizes_doc.page_size_only(index).ok().flatten())
                                .collect::<Vec<_>>()
                        })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if this.doc_generation != generation {
                            return;
                        }
                        let mut changed = false;
                        for (index, size) in sizes.into_iter().enumerate() {
                            if let Some(size) = size
                                && index < this.page_sizes.len()
                                && this.page_sizes[index] != size
                            {
                                this.page_sizes[index] = size;
                                changed = true;
                            }
                        }
                        if changed {
                            this.page_sizes_revision = this.page_sizes_revision.wrapping_add(1);
                            if this.view_mode == ViewMode::Continuous {
                                this.ensure_continuous_pages(this.viewport, cx);
                            }
                            cx.notify();
                        }
                        if let Some(page) = this.pending_restore_page.take()
                            && this.view_mode == ViewMode::Continuous
                        {
                            let offset = this.pending_restore_scroll.take().unwrap_or(0.0);
                            this.scroll_to_continuous_page_offset(this.viewport, page, offset);
                            this.sync_continuous_page(this.viewport, cx);
                            cx.notify();
                        }
                    });
                })
                .detach();
                self.bar_visible = false;
                self.bar_hovered = false;
            }
            Err(error) => {
                self.bar_visible = false;
                self.bar_hovered = false;
                self.continuous_last_scroll = None;
                self.continuous_settle_pending = false;
                self.error = Some(format!(
                    "无法打开 {}：\n{error}",
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ));
                self.doc = None;
                self.renderer = None;
                self.page_image = None;
                self.image_dpi = 0;
                self.page_cache.clear();
                self.clear_tiles();
                self.font_reports.clear();
                self.continuous_pending.clear();
                self.continuous_failed.clear();
                self.continuous_scroll.set_offset(point(px(0.0), px(0.0)));
                self.page_cache_order.clear();
                self.page_cache_bytes = 0;
                self.render_seq = self.render_seq.wrapping_add(1);
                self.latest_render = None;
                self.thumbs.clear();
                self.thumbs_started = false;
                self.page_text_cache = None;
                self.search_highlights.clear();
                self.search_active_highlight = None;
                self.clear_selection();
                self.page_count = 0;
                self.page_sizes.clear();
                self.file_size = 0;
                self.loading = false;
            }
        }
    }

    fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        let was_active = self.active_tab == Some(index);
        if was_active {
            self.tabs.remove(index);
            self.active_tab = None;
            if self.tabs.is_empty() {
                self.clear_active_document(cx);
                self.file_path = None;
                self.file_name.clear();
                self.file_size = 0;
                self.page = 0;
                self.pending_restore_page = None;
                self.pending_restore_scroll = None;
                self.settings.general.last_file = None;
                self.settings.save();
                self.set_search_input_value(window, "", cx);
                window.focus(&self.focus_handle, cx);
                cx.notify();
                return;
            }
            let next = index.min(self.tabs.len() - 1);
            self.activate_tab(next, Some(window), cx);
        } else {
            self.tabs.remove(index);
            if let Some(active) = self.active_tab
                && index < active
            {
                self.active_tab = Some(active - 1);
            }
            let active = self.active_tab.and_then(|active| self.tabs.get(active));
            self.settings.general.last_file =
                active.map(|tab| tab.path.to_string_lossy().into_owned());
            self.settings.save();
            cx.notify();
        }
    }

    /// Whole-page DPI for the single-page view: the quality preset at the
    /// current zoom (see `base_dpi`). Screen resolution is not considered.
    fn target_dpi(&self) -> u32 {
        self.base_dpi(self.page, 1)
    }

    /// Quality preset DPI at 100% zoom (质量档位).
    fn preset_dpi(&self) -> u32 {
        self.max_page_dpi().max(96)
    }

    /// Density the page content should have at the current zoom.
    fn wanted_dpi(&self) -> u32 {
        tiles::hard_dpi(self.preset_dpi(), self.zoom)
    }

    /// Whole-page bitmap density: the wanted density, but never above the
    /// preset (zoomed-in detail comes from tiles) and within the GPU texture
    /// size and the page cache shared by `visible` pages. Visible pages and
    /// prefetched neighbours use the same value, so a page that scrolls into
    /// view is already at its final quality.
    fn base_dpi(&self, page: usize, visible: usize) -> u32 {
        let size = self
            .page_sizes
            .get(page)
            .copied()
            .unwrap_or(SizeMm::new(210.0, 297.0));
        continuous::page_dpi(
            self.wanted_dpi(),
            self.preset_dpi(),
            size,
            visible,
            self.cache_config.page_bytes(),
        )
    }

    /// Upgrades the bitmap only after the zoom/resize has settled, and only
    /// when the new density is higher. Avoids a render per resize frame and
    /// never re-rasterizes a page that is merely being shown smaller.
    fn schedule_dpi_render(&mut self, cx: &mut Context<Self>) {
        // Every change bumps the generation; only the timer that observes the
        // newest generation may start a render. A running timer is reused.
        self.dpi_debounce_gen = self.dpi_debounce_gen.wrapping_add(1);
        if self.dpi_pending {
            return;
        }
        self.dpi_pending = true;
        let generation = self.dpi_debounce_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.dpi_pending = false;
                if this.dpi_debounce_gen != generation {
                    return;
                }
                match this.view_mode {
                    ViewMode::Single if this.target_dpi() > this.image_dpi => {
                        this.request_page_render(cx);
                    }
                    ViewMode::Continuous => this.ensure_continuous_pages(this.viewport, cx),
                    ViewMode::Single => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn request_page_render(&mut self, cx: &mut Context<Self>) {
        if self.view_mode == ViewMode::Continuous {
            self.loading = false;
            self.ensure_continuous_pages(self.viewport, cx);
            return;
        }
        let Some(doc) = self.doc.clone() else {
            return;
        };
        let Some(renderer) = self.renderer.clone() else {
            return;
        };

        let page = self.page;
        let dpi = self.target_dpi();

        // Prefer an exact bitmap, otherwise reuse any sharper one. A larger
        // bitmap shown smaller stays crisp and avoids a redundant render.
        if let Some((cached_dpi, image)) = self.cached_page_image(page, dpi) {
            self.page_image = Some(image);
            self.image_dpi = cached_dpi;
            self.loading = false;
            self.error = None;
            self.latest_render = None;
            return;
        }

        // Single-page wheel bursts also coalesce: show cached frames now,
        // rasterize only the final uncached page after the gesture settles.
        // Neighbour prefetch is deliberately low-DPI. Display it immediately
        // on navigation, then refine instead of ignoring a useful cached frame.
        if self.page_image.is_none()
            && let Some(preview_dpi) = self
                .page_cache
                .keys()
                .filter(|(cached_page, _)| *cached_page == page)
                .map(|(_, dpi)| *dpi)
                .max()
            && let Some((cached_dpi, image)) = self.cached_page_image(page, preview_dpi)
        {
            self.page_image = Some(image);
            self.image_dpi = cached_dpi;
            self.loading = false;
        }
        if self.continuous_scrolling() || self.tile_busy() || !self.continuous_pending.is_empty() {
            return;
        }

        // Coalesce an identical in-flight request (e.g. several frames asking
        // for the same density while a render is running).
        if self
            .latest_render
            .is_some_and(|(_, pending_page, pending_dpi)| {
                pending_page == page && pending_dpi == dpi
            })
        {
            return;
        }

        self.render_seq = self.render_seq.wrapping_add(1);
        let seq = self.render_seq;
        self.latest_render = Some((seq, page, dpi));
        self.loading = self.page_image.is_none();
        let generation = self.doc_generation;
        let view_generation = self.view_generation;
        let rotation = self.rotation;

        cx.spawn(async move |this, cx| {
            // Rasterize straight to BGRA: no temporary PNG, no encode/decode.
            let result = cx
                .background_spawn(async move {
                    let raster = renderer
                        .rasterize(&doc, page, dpi as f32)
                        .map_err(|error| error.to_string())?;
                    let font_report = renderer
                        .font_report(&doc, page)
                        .map_err(|error| error.to_string())?;
                    let bytes = raster.width as usize * raster.height as usize * 4;
                    let (width, height, pixels) =
                        rotate::owned(raster.width, raster.height, raster.bgra, rotation)
                            .ok_or_else(|| "页面图像缓冲区尺寸无效".to_string())?;
                    let buffer = image::RgbaImage::from_raw(width, height, pixels)
                        .ok_or_else(|| "页面图像缓冲区尺寸无效".to_string())?;
                    Ok::<_, String>((
                        Arc::new(RenderImage::new([image::Frame::new(buffer)])),
                        bytes,
                        font_report,
                    ))
                })
                .await;

            match result {
                Ok((image, bytes, font_report)) => {
                    let _ = this.update(cx, |this, cx| {
                        if this.doc_generation != generation
                            || this.view_generation != view_generation
                        {
                            return;
                        }
                        this.cache_page_image(page, dpi, image.clone(), bytes);
                        this.font_reports.insert(page, font_report);
                        let is_latest = this
                            .latest_render
                            .is_some_and(|(latest_seq, _, _)| latest_seq == seq);
                        if !is_latest {
                            return;
                        }
                        this.latest_render = None;
                        this.loading = false;
                        if frame_is_current(
                            true,
                            this.page == page,
                            dpi,
                            this.image_dpi,
                            this.page_image.is_some(),
                        ) {
                            this.page_image = Some(image);
                            this.image_dpi = dpi;
                            this.error = None;
                        }
                        // Start thumbnails only after the main page is on
                        // screen, so they do not compete for CPU with the
                        // first paint.
                        if !this.thumbs_started {
                            this.thumbs_started = true;
                            this.spawn_thumbnails(cx);
                        }
                        cx.notify();
                    });
                }
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        if this.doc_generation != generation
                            || this.view_generation != view_generation
                        {
                            return;
                        }
                        let is_latest = this
                            .latest_render
                            .is_some_and(|(latest_seq, _, _)| latest_seq == seq);
                        if !is_latest {
                            return;
                        }
                        this.latest_render = None;
                        this.loading = false;
                        this.error = Some(format!("页面渲染失败：{error}"));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// Exact bitmap if present, otherwise the smallest cached bitmap that is
    /// still at least as sharp as `target_dpi`.
    fn cached_page_image(
        &mut self,
        page: usize,
        target_dpi: u32,
    ) -> Option<(u32, Arc<RenderImage>)> {
        if let Some(image) = self.page_cache.get(&(page, target_dpi)).cloned() {
            self.touch_cached_page(page, target_dpi);
            return Some((target_dpi, image));
        }
        let sharper = self
            .page_cache
            .iter()
            .filter(|((cached_page, dpi), _)| *cached_page == page && *dpi > target_dpi)
            .map(|((_, dpi), _)| *dpi)
            .min()?;
        let image = self.page_cache.get(&(page, sharper)).cloned()?;
        self.touch_cached_page(page, sharper);
        Some((sharper, image))
    }

    /// Bytes of the cached page bitmaps whose key matches.
    fn cached_page_bytes(&self, matches: impl Fn((usize, u32)) -> bool) -> usize {
        self.page_cache
            .iter()
            .filter(|(key, _)| matches(**key))
            .map(|(_, image)| {
                let size = image.size(0);
                size.width.to_bytes(4) as usize * size.height.0 as usize
            })
            .sum()
    }

    fn touch_cached_page(&mut self, page: usize, dpi: u32) {
        let key = (page, dpi);
        self.page_cache_order.retain(|existing| *existing != key);
        self.page_cache_order.push_back(key);
    }

    /// Keeps decoded pages in memory within a byte budget. The displayed page
    /// is never evicted by the request that produced it.
    fn cache_page_image(&mut self, page: usize, dpi: u32, image: Arc<RenderImage>, bytes: usize) {
        self.gpu.borrow_mut().register(image.clone());
        let key = (page, dpi);
        // A sharper bitmap subsumes older densities; retaining both can make
        // a visible working set exceed its budget during zoom refinement.
        let obsolete: Vec<_> = self
            .page_cache
            .keys()
            .filter(|(cached_page, cached_dpi)| *cached_page == page && *cached_dpi < dpi)
            .copied()
            .collect();
        for old_key in obsolete {
            if let Some(old) = self.page_cache.remove(&old_key) {
                let size = old.size(0);
                self.page_cache_bytes = self
                    .page_cache_bytes
                    .saturating_sub(size.width.to_bytes(4) as usize * size.height.0 as usize);
            }
            self.page_cache_order.retain(|key| *key != old_key);
        }
        if let Some(old) = self.page_cache.insert(key, image) {
            let size = old.size(0);
            self.page_cache_bytes = self
                .page_cache_bytes
                .saturating_sub(size.width.to_bytes(4) as usize * size.height.0 as usize);
        }
        self.page_cache_bytes = self.page_cache_bytes.saturating_add(bytes);
        self.touch_cached_page(page, dpi);

        let protected = (self.page, self.image_dpi);
        // Pages on screen are never evicted, even when they alone exceed the
        // budget; otherwise two large visible pages evict each other forever.
        let on_screen = if self.view_mode == ViewMode::Continuous {
            self.continuous_content_range(self.viewport)
        } else {
            0..0
        };
        let count_limit = if self.view_mode == ViewMode::Continuous {
            self.cache_config
                .page_limit()
                .max(self.continuous_visible_range(self.viewport).len())
        } else {
            self.cache_config.page_limit()
        };
        while self.page_cache.len() > 1
            && (self.page_cache.len() > count_limit
                || self.page_cache_bytes > self.cache_config.page_bytes())
        {
            let Some(oldest) = self.page_cache_order.pop_front() else {
                break;
            };
            let keep = |entry: &(usize, u32)| {
                *entry == key || *entry == protected || on_screen.contains(&entry.0)
            };
            if keep(&oldest) {
                self.page_cache_order.push_back(oldest);
                // Only protected entries are left; stop instead of spinning.
                if self.page_cache_order.iter().all(keep) {
                    break;
                }
                continue;
            }
            if let Some(old) = self.page_cache.remove(&oldest) {
                let size = old.size(0);
                self.page_cache_bytes = self
                    .page_cache_bytes
                    .saturating_sub(size.width.to_bytes(4) as usize * size.height.0 as usize);
            }
        }
    }

    /// Renders thumbnails one at a time. Rendering them concurrently used to
    /// saturate every background worker and made the main page wait behind
    /// dozens of thumbnail jobs on multi-page documents.
    fn spawn_thumbnails(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.clone() else {
            return;
        };
        let Some(renderer) = self.renderer.clone() else {
            return;
        };
        let generation = self.doc_generation;
        let count = self.page_count.min(THUMB_LIMIT);
        let mut order: Vec<usize> = (0..count).collect();
        order.sort_by_key(|page| page.abs_diff(self.page));

        cx.spawn(async move |this, cx| {
            for page in order {
                // Thumbnails must not compete with wheel gestures or a
                // foreground page. At most one thumbnail is already running.
                loop {
                    let ready = this.update(cx, |this, _| {
                        if this.doc_generation != generation {
                            return None;
                        }
                        Some(
                            !this.continuous_scrolling()
                                && this.continuous_pending.is_empty()
                                && !this.tile_busy()
                                && this.latest_render.is_none(),
                        )
                    });
                    match ready {
                        Ok(Some(true)) => break,
                        Ok(Some(false)) => cx.background_executor().timer(CONTINUOUS_IDLE).await,
                        _ => return,
                    }
                }
                let doc = doc.clone();
                let renderer = renderer.clone();
                // Straight to a texture: no PNG encode, temp file or decode,
                // and pages built only for a thumbnail are not cached.
                let result = cx
                    .background_spawn(async move {
                        let raster = renderer.rasterize_preview(&doc, page, THUMB_DPI).ok()?;
                        let buffer =
                            image::RgbaImage::from_raw(raster.width, raster.height, raster.bgra)?;
                        Some(Arc::new(RenderImage::new([image::Frame::new(buffer)])))
                    })
                    .await;
                let Some(image) = result else {
                    continue;
                };
                let alive = this.update(cx, |this, cx| {
                    if this.doc_generation == generation && page < this.thumbs.len() {
                        this.gpu.borrow_mut().register(image.clone());
                        this.thumbs[page] = Some(image);
                        let visible = this.thumbnail_on_screen(page);
                        this.gpu.borrow_mut().background_done(visible);
                        if visible {
                            cx.notify();
                        }
                    }
                    this.doc_generation == generation
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        self.zoom = zoom.clamp(0.5, 4.0);
        if self.view_mode == ViewMode::Continuous {
            self.scroll_to_continuous_page(self.viewport, self.page);
        }
        // A render pass decides whether the bitmap needs more density; zooming
        // out keeps the sharper image and costs nothing.
        cx.notify();
    }

    fn continuous_page_scale(&self, viewport: Size<Pixels>, index: usize) -> f32 {
        let size = self
            .page_sizes
            .get(index)
            .copied()
            .unwrap_or(SizeMm::new(210.0, 297.0));
        let (width, _) = rotate::size(size.width as f32, size.height as f32, self.rotation);
        let base_w = (width * 96.0 / 25.4).max(1.0);
        let sidebar = if self.sidebar_open { 240.0 } else { 0.0 };
        let available_w = (f32::from(viewport.width) - sidebar - 72.0).max(240.0);
        (available_w / base_w) * self.zoom
    }

    fn continuous_page_size(&self, viewport: Size<Pixels>, index: usize) -> (f32, f32) {
        self.with_continuous_layout(viewport, |layout| {
            layout.sizes.get(index).copied().unwrap_or((0.0, 0.0))
        })
    }

    fn with_continuous_layout<T>(
        &self,
        viewport: Size<Pixels>,
        f: impl FnOnce(&continuous::Layout) -> T,
    ) -> T {
        let sidebar = if self.sidebar_open { 240.0 } else { 0.0 };
        let key = continuous::LayoutKey {
            width: (f32::from(viewport.width) - sidebar - 72.0).max(240.0),
            zoom: self.zoom,
            revision: self.page_sizes_revision,
            rotation: self.rotation,
            gap: self.settings.reading.page_gap.pixels(),
        };
        f(self
            .continuous_layout
            .borrow_mut()
            .get(key, &self.page_sizes))
    }

    fn continuous_top(&self, viewport: Size<Pixels>, index: usize) -> f32 {
        self.with_continuous_layout(viewport, |layout| layout.top(index))
    }

    fn scroll_to_continuous_page(&self, viewport: Size<Pixels>, page: usize) {
        let top = self.continuous_top(viewport, page);
        self.continuous_scroll.set_offset(point(px(0.0), px(-top)));
    }

    fn scroll_to_continuous_page_offset(&self, viewport: Size<Pixels>, page: usize, offset: f32) {
        let top = self.continuous_top(viewport, page);
        let height = self.continuous_page_size(viewport, page).1;
        let y = top + height * offset.clamp(0.0, 1.0);
        self.continuous_scroll.set_offset(point(px(0.0), px(-y)));
    }

    /// Puts a search match near the upper third of the continuous view.
    fn scroll_to_search_match(
        &mut self,
        viewport: Size<Pixels>,
        page: usize,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(highlight) = self.search_highlights.get(index) else {
            return;
        };
        let (page_top, page_height, content_height) =
            self.with_continuous_layout(viewport, |layout| {
                (
                    layout.top(page),
                    layout.sizes.get(page).map_or(0.0, |size| size.1),
                    layout.height,
                )
            });
        if page_height <= 0.0 {
            return;
        }
        let visible_height = (f32::from(viewport.height) - 124.0).max(240.0);
        let match_y = page_top + (highlight.y + highlight.height * 0.5) * page_height;
        let offset = (match_y - visible_height * 0.35)
            .clamp(0.0, (content_height - visible_height).max(0.0));
        self.continuous_scroll
            .set_offset(point(px(0.0), px(-offset)));
        self.ensure_continuous_pages(viewport, cx);
    }

    fn continuous_page_at(&self, viewport: Size<Pixels>, scroll: f32) -> usize {
        self.with_continuous_layout(viewport, |layout| layout.page_at(scroll))
    }

    fn continuous_visible_range(&self, viewport: Size<Pixels>) -> std::ops::Range<usize> {
        if self.page_count == 0 {
            return 0..0;
        }
        let offset = -f32::from(self.continuous_scroll.offset().y);
        let height = (f32::from(viewport.height) - 124.0).max(240.0);
        self.with_continuous_layout(viewport, |layout| layout.visible(offset, height))
    }

    fn continuous_content_range(&self, viewport: Size<Pixels>) -> std::ops::Range<usize> {
        if self.page_count == 0 {
            return 0..0;
        }
        let offset = -f32::from(self.continuous_scroll.offset().y);
        let height = (f32::from(viewport.height) - 124.0).max(240.0);
        self.with_continuous_layout(viewport, |layout| {
            layout.page_at(offset)..(layout.page_at(offset + height) + 1).min(self.page_count)
        })
    }

    /// How many pages of this size can be on screen at once. Unlike the
    /// current visible range it does not change while scrolling, so a tight
    /// cache budget does not make the density (and cache key) flip.
    fn continuous_page_slots(&self, viewport: Size<Pixels>, page: usize) -> usize {
        let height = (f32::from(viewport.height) - 124.0).max(240.0);
        let page_height = self.continuous_page_size(viewport, page).1.max(1.0);
        ((height / page_height).ceil() as usize + 1).clamp(1, self.page_count.max(1))
    }

    fn continuous_dpi(&self, viewport: Size<Pixels>, page: usize) -> u32 {
        // Same density on and off screen: prefetched neighbours must not be
        // a blurrier stand-in that is refined after scrolling stops.
        self.base_dpi(page, self.continuous_page_slots(viewport, page))
    }

    fn request_continuous_page(
        &mut self,
        viewport: Size<Pixels>,
        page: usize,
        cx: &mut Context<Self>,
    ) {
        let dpi = self.continuous_dpi(viewport, page);
        self.request_cached_page(page, dpi, cx);
    }

    fn request_cached_page(&mut self, page: usize, dpi: u32, cx: &mut Context<Self>) {
        if page >= self.page_count
            || !continuous::render_ready(self.raster_jobs(), self.tile_slots)
            || self.latest_render.is_some()
        {
            return;
        }
        if self
            .page_cache
            .keys()
            .any(|(cached_page, cached_dpi)| *cached_page == page && *cached_dpi >= dpi)
        {
            return;
        }
        let Some(doc) = self.doc.clone() else {
            return;
        };
        let Some(renderer) = self.renderer.clone() else {
            return;
        };
        if self.continuous_failed.contains(&(page, dpi))
            || !self.continuous_pending.insert((page, dpi))
        {
            return;
        }
        let generation = self.doc_generation;
        let view_generation = self.view_generation;
        let rotation = self.rotation;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let raster = renderer
                        .rasterize(&doc, page, dpi as f32)
                        .map_err(|error| error.to_string())?;
                    rotate::owned(raster.width, raster.height, raster.bgra, rotation)
                        .ok_or_else(|| "页面图像缓冲区尺寸无效".to_string())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.doc_generation != generation {
                    return;
                }
                this.continuous_pending.remove(&(page, dpi));
                if this.view_generation != view_generation {
                    cx.notify();
                    return;
                }
                if result.is_err() {
                    this.continuous_failed.insert((page, dpi));
                }
                // Do not retain a completed frame outside the current visible
                // and nearby-prefetch horizon. There is one in-flight job.
                if let Ok((width, height, pixels)) = result
                    && this.nearby_page_range().contains(&page)
                    && let Some(buffer) = image::RgbaImage::from_raw(width, height, pixels)
                {
                    let bytes = width as usize * height as usize * 4;
                    this.cache_page_image(
                        page,
                        dpi,
                        Arc::new(RenderImage::new([image::Frame::new(buffer)])),
                        bytes,
                    );
                    if !this.thumbs_started {
                        this.thumbs_started = true;
                        this.spawn_thumbnails(cx);
                    }
                }
                if this.view_mode == ViewMode::Single
                    && this.page_image.is_none()
                    && this.latest_render.is_none()
                {
                    this.request_page_render(cx);
                }
                let visible = this.page_on_screen(page);
                this.finish_background(visible, cx);
            });
        })
        .detach();
    }

    /// Thumbnails show in the sidebar and page grid, and stand in for an
    /// on-screen page that has no bitmap yet.
    fn thumbnail_on_screen(&self, page: usize) -> bool {
        self.sidebar_open
            || self.grid_open
            || (self.page_on_screen(page)
                && !self.page_cache.keys().any(|(cached, _)| *cached == page)
                && (self.view_mode == ViewMode::Continuous || self.page_image.is_none()))
    }

    /// Whether a bitmap of `page` is (or is about to be) drawn: the
    /// continuous rows actually built, or the single page being shown.
    fn page_on_screen(&self, page: usize) -> bool {
        match self.view_mode {
            ViewMode::Continuous => self.continuous_visible_range(self.viewport).contains(&page),
            ViewMode::Single => page == self.page,
        }
    }

    /// A background raster job finished. Every notify repaints the whole
    /// window, so only results that are on screen request one; otherwise the
    /// next job is scheduled directly instead of through a frame.
    fn finish_background(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.gpu.borrow_mut().background_done(visible);
        if visible {
            cx.notify();
            return;
        }
        if self.view_mode == ViewMode::Continuous {
            self.ensure_continuous_pages(self.viewport, cx);
        }
        self.schedule_nearby(cx);
    }

    fn ensure_continuous_pages(&mut self, viewport: Size<Pixels>, cx: &mut Context<Self>) {
        if self.grid_open || !continuous::render_ready(self.raster_jobs(), self.tile_slots) {
            return;
        }
        // Only visible previews here. Speculative pages wait until tile plans
        // from all prepaint callbacks are known, so they cannot jump the queue.
        let mut pages: Vec<_> = self.continuous_content_range(viewport).collect();
        let actual = self.continuous_content_range(viewport);
        pages.sort_by_key(|page| {
            (
                !actual.contains(page),
                page.abs_diff(self.page),
                *page < self.page,
            )
        });
        for page in pages {
            self.request_continuous_page(viewport, page, cx);
            if !continuous::render_ready(self.raster_jobs(), self.tile_slots) {
                break;
            }
        }
    }

    /// Page and tile raster jobs in flight; together they share the
    /// configured render thread slots.
    fn raster_jobs(&self) -> usize {
        self.continuous_pending.len() + self.tile_jobs.len()
    }

    fn continuous_scrolling(&self) -> bool {
        self.continuous_last_scroll
            .is_some_and(|last| last.elapsed() < CONTINUOUS_IDLE)
    }

    fn note_continuous_scroll(&mut self, cx: &mut Context<Self>) {
        self.continuous_last_scroll = Some(Instant::now());
        if self.continuous_settle_pending {
            return;
        }
        self.continuous_settle_pending = true;
        let generation = self.doc_generation;
        cx.spawn(async move |this, cx| {
            let mut wait = CONTINUOUS_IDLE;
            loop {
                cx.background_executor().timer(wait).await;
                let next = this.update(cx, |this, cx| {
                    if this.doc_generation != generation {
                        return None;
                    }
                    if let Some(last) = this.continuous_last_scroll {
                        let elapsed = last.elapsed();
                        if elapsed < CONTINUOUS_IDLE {
                            return Some(CONTINUOUS_IDLE - elapsed);
                        }
                    }
                    this.continuous_settle_pending = false;
                    if this.view_mode == ViewMode::Continuous {
                        this.refresh_search_highlights();
                        this.ensure_continuous_pages(this.viewport, cx);
                        cx.notify();
                    } else if this.doc.is_some() {
                        this.request_page_render(cx);
                        cx.notify();
                    }
                    None
                });
                match next {
                    Ok(Some(remaining)) => wait = remaining,
                    _ => break,
                }
            }
        })
        .detach();
    }

    fn scroll_continuous(&mut self, delta: f32, cx: &mut Context<Self>) {
        self.note_continuous_scroll(cx);
        let offset = self.continuous_scroll.offset();
        self.continuous_scroll
            .set_offset(point(offset.x, offset.y - px(delta)));
        self.sync_continuous_page(self.viewport, cx);
        cx.notify();
    }

    fn sync_continuous_page(&mut self, viewport: Size<Pixels>, cx: &mut Context<Self>) {
        let scroll = -f32::from(self.continuous_scroll.offset().y);
        if scroll != self.last_scroll_y {
            self.scroll_down = scroll > self.last_scroll_y;
            self.last_scroll_y = scroll;
        }
        let page = self.continuous_page_at(viewport, scroll);
        if page != self.page {
            self.page = page;
            self.text_layout = None;
            self.clear_selection();
            if !self.continuous_scrolling() {
                self.refresh_search_highlights();
            } else {
                self.search_highlights.clear();
                self.search_active_highlight = None;
            }
            cx.notify();
        }
    }

    fn on_continuous_scroll(
        &mut self,
        event: &ScrollEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if (event.modifiers.control || event.modifiers.platform) && self.page_count > 0 {
            self.note_continuous_scroll(cx);
            self.set_zoom(self.zoom * wheel_zoom_factor(event.delta), cx);
            return;
        }
        self.note_continuous_scroll(cx);
        self.sync_continuous_page(self.viewport, cx);
        cx.notify();
    }

    /// Single-page mode has no scrollable content, so map plain mouse-wheel
    /// notches to page changes and Ctrl/Cmd+wheel to zoom. Continuous mode
    /// receives the event from its own scroll container and never reaches this
    /// fallback.
    fn on_single_scroll(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.view_mode != ViewMode::Single || self.page_count == 0 || self.grid_open {
            return;
        }
        if event.modifiers.control || event.modifiers.platform {
            self.note_continuous_scroll(cx);
            self.set_zoom(self.zoom * wheel_zoom_factor(event.delta), cx);
            return;
        }
        let lines = match event.delta {
            ScrollDelta::Lines(delta) => delta.y,
            ScrollDelta::Pixels(delta) => f32::from(delta.y) / 40.0,
        };
        if lines.abs() > f32::EPSILON {
            self.note_continuous_scroll(cx);
        }
        self.wheel_accum += lines;
        let now = Instant::now();
        if now.duration_since(self.wheel_last) < Duration::from_millis(120)
            || self.wheel_accum.abs() < 0.8
        {
            return;
        }
        self.wheel_last = now;
        self.wheel_accum = 0.0;
        if lines > 0.0 {
            self.prev_page(cx);
        } else {
            self.next_page(cx);
        }
    }

    fn on_toggle_view_mode(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.view_mode = match self.view_mode {
            ViewMode::Single => ViewMode::Continuous,
            ViewMode::Continuous => ViewMode::Single,
        };
        self.page_image = None;
        self.image_dpi = 0;
        self.text_layout = None;
        self.clear_selection();
        match self.view_mode {
            ViewMode::Continuous => {
                self.scroll_to_continuous_page(self.viewport, self.page);
                self.ensure_continuous_pages(self.viewport, cx);
            }
            ViewMode::Single => self.request_page_render(cx),
        }
        cx.notify();
    }

    fn go_to_page(&mut self, page: usize, cx: &mut Context<Self>) {
        if page >= self.page_count || page == self.page {
            return;
        }
        self.page = page;
        self.page_image = None;
        self.image_dpi = 0;
        self.text_layout = None;
        self.clear_selection();
        self.refresh_search_highlights();
        match self.view_mode {
            ViewMode::Single => self.request_page_render(cx),
            ViewMode::Continuous => {
                self.scroll_to_continuous_page(self.viewport, page);
                self.ensure_continuous_pages(self.viewport, cx);
            }
        }
        cx.notify();
    }

    fn next_page(&mut self, cx: &mut Context<Self>) {
        self.go_to_page(self.page + 1, cx);
    }

    fn prev_page(&mut self, cx: &mut Context<Self>) {
        self.go_to_page(self.page.saturating_sub(1), cx);
    }

    fn zoom_percent(&self, viewport: Size<Pixels>) -> f32 {
        match self.view_mode {
            ViewMode::Single => self.fit_scale(viewport) * self.zoom * 100.0,
            ViewMode::Continuous => self.continuous_page_scale(viewport, self.page) * 100.0,
        }
    }

    fn fit_scale(&self, viewport: Size<Pixels>) -> f32 {
        let size = self
            .page_sizes
            .get(self.page)
            .copied()
            .unwrap_or(SizeMm::new(210.0, 297.0));
        let (width, height) = rotate::size(size.width as f32, size.height as f32, self.rotation);
        let base_w = width * 96.0 / 25.4;
        let base_h = height * 96.0 / 25.4;

        let sidebar = if self.sidebar_open { 240.0 } else { 0.0 };
        let available_w = (f32::from(viewport.width) - sidebar - 96.0).max(240.0);
        let available_h = (f32::from(viewport.height) - 160.0).max(240.0);
        (available_w / base_w).min(available_h / base_h).min(1.0)
    }

    fn display_size(&self, viewport: Size<Pixels>) -> (Pixels, Pixels) {
        let size = self
            .page_sizes
            .get(self.page)
            .copied()
            .unwrap_or(SizeMm::new(210.0, 297.0));
        let (width, height) = rotate::size(size.width as f32, size.height as f32, self.rotation);
        let base_w = width * 96.0 / 25.4;
        let base_h = height * 96.0 / 25.4;
        let scale = (self.fit_scale(viewport) * self.zoom).max(0.1);
        (px(base_w * scale), px(base_h * scale))
    }

    fn on_open(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.prompt_open(cx);
    }

    fn prompt_open(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("打开 OFD 文件".into()),
        });

        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                let _ = this.update(cx, |this, cx| {
                    for path in paths {
                        if path
                            .extension()
                            .is_some_and(|extension| extension.eq_ignore_ascii_case("ofd"))
                        {
                            this.open_path(path, None, cx);
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn on_zoom_out(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(self.zoom - 0.25, cx);
    }

    fn on_zoom_in(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(self.zoom + 0.25, cx);
    }

    /// Zoom value that shows one whole page. Single-page mode's fit scale is
    /// already page-based; continuous mode is width-based at 100%, so it must
    /// zoom out until the page height fits the viewport.
    fn fit_page_zoom(&self, viewport: Size<Pixels>) -> f32 {
        if self.view_mode == ViewMode::Single {
            return 1.0;
        }
        let size = self
            .page_sizes
            .get(self.page)
            .copied()
            .unwrap_or(SizeMm::new(210.0, 297.0));
        let (width, height) = rotate::size(size.width as f32, size.height as f32, self.rotation);
        let sidebar = if self.sidebar_open { 240.0 } else { 0.0 };
        let available_w = (f32::from(viewport.width) - sidebar - 72.0).max(240.0);
        let available_h = (f32::from(viewport.height) - 160.0).max(240.0);
        (available_h / (available_w * height / width.max(1.0))).clamp(0.5, 4.0)
    }

    fn on_fit(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let zoom = self.fit_page_zoom(window.viewport_size());
        self.set_zoom(zoom, cx);
    }

    fn on_fit_width(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.view_mode == ViewMode::Continuous {
            self.set_zoom(1.0, cx);
            return;
        }
        let viewport = window.viewport_size();
        let size = self
            .page_sizes
            .get(self.page)
            .copied()
            .unwrap_or(SizeMm::new(210.0, 297.0));
        let (width, _) = rotate::size(size.width as f32, size.height as f32, self.rotation);
        let base_w = width * 96.0 / 25.4;
        let sidebar = if self.sidebar_open { 240.0 } else { 0.0 };
        let available_w = (f32::from(viewport.width) - sidebar - 96.0).max(240.0);
        let width_fit = (available_w / base_w).min(1.0);
        let page_fit = self.fit_scale(viewport).max(0.01);
        self.set_zoom((width_fit / page_fit).clamp(0.5, 4.0), cx);
    }

    fn on_rotate(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.rotate_view(cx);
    }

    fn rotate_view(&mut self, cx: &mut Context<Self>) {
        self.rotation = (self.rotation + 1) % 4;
        self.clear_selection();
        self.refresh_search_highlights();
        // Cached rasters and in-flight jobs belong to the previous orientation;
        // the old bitmap has the wrong shape, so nothing stays on screen.
        self.page_image = None;
        if self.doc.is_some() && self.view_mode == ViewMode::Continuous {
            self.scroll_to_continuous_page(self.viewport, self.page);
        }
        self.invalidate_rasters(cx);
    }

    /// Applies the image sampling setting; bitmaps drawn with the old filter
    /// are dropped and re-rendered. A no-op when it did not change.
    fn apply_image_sampling(&mut self, cx: &mut Context<Self>) {
        let filter = self.settings.performance.image_sampling.filter();
        let Some(renderer) = &self.renderer else {
            return;
        };
        if renderer.image_filter() == filter {
            return;
        }
        renderer.set_image_filter(filter);
        self.invalidate_rasters(cx);
    }

    /// Drops every cached page bitmap and tile, discards in-flight results
    /// and re-requests what is on screen. A single page's current bitmap
    /// stays visible until its replacement arrives (`image_dpi = 0` forces
    /// the re-render).
    fn invalidate_rasters(&mut self, cx: &mut Context<Self>) {
        self.view_generation = self.view_generation.wrapping_add(1);
        self.render_seq = self.render_seq.wrapping_add(1);
        self.latest_render = None;
        self.image_dpi = 0;
        self.page_cache.clear();
        self.page_cache_order.clear();
        self.page_cache_bytes = 0;
        self.clear_tiles();
        self.continuous_pending.clear();
        self.continuous_failed.clear();
        self.loading =
            self.view_mode == ViewMode::Single && self.doc.is_some() && self.page_image.is_none();
        if self.doc.is_some() {
            if self.view_mode == ViewMode::Continuous {
                self.ensure_continuous_pages(self.viewport, cx);
            } else if !self.grid_open {
                self.request_page_render(cx);
            }
        }
        cx.notify();
    }

    fn on_toggle_grid(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_grid(cx);
    }

    fn toggle_grid(&mut self, cx: &mut Context<Self>) {
        self.grid_open = !self.grid_open;
        if !self.grid_open && self.doc.is_some() {
            match self.view_mode {
                ViewMode::Continuous => {
                    self.scroll_to_continuous_page(self.viewport, self.page);
                    self.ensure_continuous_pages(self.viewport, cx);
                }
                ViewMode::Single if self.page_image.is_none() => self.request_page_render(cx),
                ViewMode::Single => {}
            }
        }
        if self.grid_open && !self.thumbs_started && self.renderer.is_some() {
            self.thumbs_started = true;
            self.spawn_thumbnails(cx);
        }
        cx.notify();
    }

    fn on_toggle_sidebar(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        if self.view_mode == ViewMode::Continuous {
            self.scroll_to_continuous_page(self.viewport, self.page);
        }
        cx.notify();
    }

    /// Pointer moves caused by hover/hitbox changes are synthetically dispatched
    /// by GPUI; only a real position change is activity, otherwise hiding the
    /// bar would immediately show it again. Tiny per-frame jitter is ignored so
    /// a stationary pointer can time out.
    fn on_pointer_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting && event.pressed_button == Some(MouseButton::Left) {
            self.update_selection_drag(event.position, cx);
        }
        if self
            .bar_pointer
            .is_some_and(|last| !pointer_moved(last, event.position))
        {
            return;
        }
        self.bar_pointer = Some(event.position);
        if self.doc.is_some()
            && self.settings.reading.floating_toolbar
            && self
                .reader_bounds
                .get()
                .is_some_and(|bounds| near_floating_bar(bounds, event.position))
        {
            self.note_bar_activity(cx);
        }
    }

    /// Only proximity to the toolbar reveals it; unrelated pointer movement
    /// must not keep resetting its hide timer.
    fn note_bar_activity(&mut self, cx: &mut Context<Self>) {
        self.bar_last_activity = Instant::now();
        if !self.bar_visible {
            self.bar_visible = true;
            cx.notify();
        }
        if self.bar_watchdog || self.doc.is_none() {
            return;
        }

        self.bar_watchdog = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
                let keep_running = this.update(cx, |this, cx| {
                    if this.doc.is_none() || !this.bar_visible {
                        this.bar_visible = false;
                        this.bar_hovered = false;
                        this.bar_watchdog = false;
                        return false;
                    }
                    if !this.bar_hovered && this.bar_last_activity.elapsed() >= BAR_HIDE_DELAY {
                        this.bar_visible = false;
                        this.bar_watchdog = false;
                        cx.notify();
                        return false;
                    }
                    true
                });
                if !matches!(keep_running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn on_toggle_search(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_search(window, cx);
    }

    fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_open {
            self.close_search(window, cx);
            return;
        }
        self.ensure_search_input(window, cx);
        self.search_open = true;
        // Results live in the sidebar, so make sure it is visible when a
        // search starts.
        if self.doc.is_some() {
            self.sidebar_open = true;
        }
        if let Some(input) = &self.search_input {
            input.update(cx, |state, cx| state.focus(window, cx));
        }
        self.run_search(cx);
    }

    fn ensure_search_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(input) = &self.search_input {
            return input.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("搜索文档内容"));
        cx.subscribe(&input, |this, _, event, cx| match event {
            InputEvent::Change => {
                if !this.search_suppress_change {
                    this.run_search(cx);
                }
            }
            InputEvent::PressEnter { shift, .. } => {
                this.search_step(if *shift { -1 } else { 1 }, cx);
            }
            _ => {}
        })
        .detach();
        self.search_input = Some(input.clone());
        input
    }

    fn set_search_input_value(&mut self, window: &mut Window, value: &str, cx: &mut Context<Self>) {
        if value.is_empty() && self.search_input.is_none() {
            return;
        }
        let input = self.ensure_search_input(window, cx);
        self.search_suppress_change = true;
        input.update(cx, |state, cx| state.set_value(value, window, cx));
        self.search_suppress_change = false;
    }

    /// Restores the tab's search query and selected hit after the document has
    /// been reopened. The input widget is only synced when a window is around.
    fn restore_tab_search(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        self.search_pending_index = None;
        self.search_pending_input = None;
        let saved_index = self.search_index;
        if !self.search_open {
            if let Some(window) = window {
                self.set_search_input_value(window, "", cx);
            } else {
                self.search_pending_input = Some(String::new());
            }
            return;
        }
        let query = self.search_query.clone();
        if let Some(window) = window {
            self.set_search_input_value(window, &query, cx);
        } else {
            self.search_pending_input = Some(query.clone());
        }
        self.apply_search_query(query.clone(), cx);
        if !self.search_hits.is_empty() {
            self.search_index = saved_index.min(self.search_hits.len() - 1);
            self.sync_search_active_highlight();
            self.search_pending_index = Some((query, self.search_index));
        }
    }

    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_open = false;
        self.search_query.clear();
        self.search_hits.clear();
        self.search_index = 0;
        self.search_highlights.clear();
        self.search_active_highlight = None;
        self.search_pending_index = None;
        self.search_pending_input = None;
        if let Some(input) = &self.search_input {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    /// Case-sensitive substring search over the extracted text of every page.
    /// One hit is produced per occurrence so Enter walks through them in order.
    fn run_search(&mut self, cx: &mut Context<Self>) {
        let query = self
            .search_input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default();
        self.apply_search_query(query, cx);
    }

    fn apply_search_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.search_query = query;
        self.search_hits.clear();
        self.search_index = 0;
        self.search_results_scroll
            .set_offset(point(px(0.0), px(0.0)));

        let needle = self.search_query.trim();
        if needle.is_empty() {
            self.search_pending_index = None;
            self.search_highlights.clear();
            self.search_active_highlight = None;
            cx.notify();
            return;
        }
        let Some(doc) = &self.doc else {
            self.search_pending_index = None;
            self.search_highlights.clear();
            self.search_active_highlight = None;
            cx.notify();
            return;
        };
        if self.page_text_cache.is_none() {
            self.page_text_cache = Some(
                (0..self.page_count)
                    .map(|page| doc.page_display_text(page).unwrap_or_default())
                    .collect(),
            );
        }
        let pages = self.page_text_cache.as_ref().expect("just built");

        'pages: for (page, texts) in pages.iter().enumerate() {
            let mut page_match = 0;
            for text in texts {
                let text = text.replace(['\n', '\r'], " ");
                let mut from = 0;
                while let Some(offset) = text[from..].find(needle) {
                    let start = from + offset;
                    let end = start + needle.len();
                    self.search_hits.push(SearchHit {
                        page,
                        match_index: page_match,
                        snippet: search_snippet(&text, start, end),
                    });
                    page_match += 1;
                    if self.search_hits.len() >= SEARCH_LIMIT {
                        break 'pages;
                    }
                    from = end;
                }
            }
        }
        if let Some((pending_query, pending_index)) = self.search_pending_index.take()
            && pending_query == self.search_query
        {
            self.search_index = pending_index.min(self.search_hits.len().saturating_sub(1));
        }
        self.refresh_search_highlights();
        cx.notify();
    }

    /// Recomputes on-page highlight rectangles for the current page and query.
    fn refresh_search_highlights(&mut self) {
        self.search_highlights.clear();
        self.search_active_highlight = None;
        if !self.search_open {
            return;
        }
        let query = self.search_query.trim();
        if query.is_empty() {
            return;
        }
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let size = self
            .page_sizes
            .get(self.page)
            .copied()
            .unwrap_or(SizeMm::new(210.0, 297.0));
        if let Ok(rects) = doc.page_search_rects(self.page, query) {
            self.search_highlights =
                rotate::highlights(search_highlight::normalize(size, &rects), self.rotation);
            self.sync_search_active_highlight();
        }
    }

    /// Points the distinct on-page colour at the result selected in the list.
    fn sync_search_active_highlight(&mut self) {
        self.search_active_highlight = self
            .search_hits
            .get(self.search_index)
            .filter(|hit| hit.page == self.page)
            .and_then(|hit| self.search_highlights.get(hit.match_index).cloned());
    }

    /// Reveals the selected hit: switches page when needed and centres the
    /// match in the continuous view.
    fn reveal_search_hit(&mut self, index: usize, cx: &mut Context<Self>) {
        if !self.settings.reading.search_auto_scroll {
            cx.notify();
            return;
        }
        let Some(hit) = self.search_hits.get(index) else {
            return;
        };
        let (page, match_index) = (hit.page, hit.match_index);
        if page != self.page {
            self.go_to_page(page, cx);
        }
        if self.view_mode == ViewMode::Continuous {
            // Scrolling clears the rects until it settles; selecting a result
            // must restore them even if the pointer arrives mid-scroll.
            if self.search_highlights.is_empty() {
                self.refresh_search_highlights();
            }
            let viewport = self.viewport;
            self.scroll_to_search_match(viewport, page, match_index, cx);
        }
        cx.notify();
    }

    fn search_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.search_hits.is_empty() {
            return;
        }
        let len = self.search_hits.len() as isize;
        self.search_index = (self.search_index as isize + delta).rem_euclid(len) as usize;
        self.search_results_scroll.scroll_to_item(self.search_index);
        self.sync_search_active_highlight();
        self.reveal_search_hit(self.search_index, cx);
    }

    fn select_search_hit(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.search_hits.len() {
            return;
        }
        self.search_index = index;
        self.search_results_scroll.scroll_to_item(index);
        self.sync_search_active_highlight();
        self.reveal_search_hit(index, cx);
    }

    fn ensure_text_layout(&mut self) {
        if self.text_layout.is_some() {
            return;
        }
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        self.text_layout = doc.page_text_layout(self.page).ok();
    }

    fn clear_selection(&mut self) {
        self.selection_anchor = None;
        self.selection_focus = None;
        self.selection_rects.clear();
        self.selected_text.clear();
        self.selecting = false;
        self.selection_moved = false;
    }

    /// Converts a window position into page millimetres. `clamp` is used while
    /// dragging so the selection still follows the pointer outside the page.
    fn page_point(&self, position: Point<Pixels>, clamp: bool) -> Option<(f64, f64)> {
        let bounds = self.page_bounds.get()?;
        let size = self.page_sizes.get(self.page).copied()?;
        let width = f32::from(bounds.size.width);
        let height = f32::from(bounds.size.height);
        if width <= 0.0 || height <= 0.0 {
            return None;
        }
        let mut x = (f32::from(position.x) - f32::from(bounds.origin.x)) / width;
        let mut y = (f32::from(position.y) - f32::from(bounds.origin.y)) / height;
        if !clamp && !(0.0..=1.0).contains(&x) {
            return None;
        }
        if !clamp && !(0.0..=1.0).contains(&y) {
            return None;
        }
        x = x.clamp(0.0, 1.0);
        y = y.clamp(0.0, 1.0);
        let (x, y) = rotate::inverse_point(x as f64, y as f64, self.rotation);
        Some((x * size.width, y * size.height))
    }

    fn update_selection(&mut self) {
        self.selection_rects.clear();
        self.selected_text.clear();
        let (Some(layout), Some(anchor), Some(focus)) = (
            &self.text_layout,
            self.selection_anchor,
            self.selection_focus,
        ) else {
            return;
        };
        let start = anchor.min(focus);
        let end = anchor.max(focus) + 1;
        let size = self
            .page_sizes
            .get(self.page)
            .copied()
            .unwrap_or(SizeMm::new(210.0, 297.0));
        self.selected_text = layout.text_range(start, end);
        self.selection_rects = rotate::highlights(
            search_highlight::normalize(size, &layout.rects(start, end)),
            self.rotation,
        );
    }

    fn on_page_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left {
            return;
        }
        self.ensure_text_layout();
        self.selecting = false;
        self.selection_moved = false;
        let Some((x, y)) = self.page_point(event.position, false) else {
            self.clear_selection();
            cx.notify();
            return;
        };
        let index = self
            .text_layout
            .as_ref()
            .and_then(|layout| layout.index_at(x, y, 8.0));
        if let Some(index) = index {
            self.selection_anchor = Some(index);
            self.selection_focus = Some(index);
            self.selecting = true;
            self.update_selection();
        } else {
            self.clear_selection();
        }
        cx.notify();
    }

    fn update_selection_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        let Some((x, y)) = self.page_point(position, true) else {
            return;
        };
        let Some(index) = self
            .text_layout
            .as_ref()
            .and_then(|layout| layout.index_at(x, y, f64::INFINITY))
        else {
            return;
        };
        if self.selection_focus != Some(index) {
            self.selection_focus = Some(index);
            self.selection_moved = true;
            self.update_selection();
            cx.notify();
        }
    }

    fn on_page_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        self.selecting = false;
        if !self.selection_moved {
            self.clear_selection();
        }
        cx.notify();
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        if self.selected_text.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(self.selected_text.clone()));
        let count = self.selected_text.chars().count();
        self.set_notice(format!("已复制 {count} 个字符"), false, cx);
        cx.notify();
    }

    fn on_toggle_theme(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.dark = !self.dark;
        self.settings.appearance.theme = if self.dark {
            settings::ThemeChoice::Dark
        } else {
            settings::ThemeChoice::Light
        };
        self.settings.save();
        apply_component_theme(self.dark, &self.ui_font, cx);
        cx.notify();
    }

    /// Shows a transient status-bar message and clears it after a few seconds.
    fn set_notice(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        self.notice_generation = self.notice_generation.wrapping_add(1);
        let generation = self.notice_generation;
        self.notice = Some((text.into(), error));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(4)).await;
            let _ = this.update(cx, |this, cx| {
                if this.notice_generation == generation {
                    this.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn on_export(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.exporting {
            return;
        }
        let Some(doc) = self.doc.clone() else {
            self.set_notice("请先打开 OFD 文档", true, cx);
            cx.notify();
            return;
        };
        let Some(renderer) = self.renderer.clone() else {
            self.set_notice("文档渲染器尚未就绪", true, cx);
            cx.notify();
            return;
        };

        let page = self.page;
        let directory = self
            .file_path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        let stem = self
            .file_path
            .as_deref()
            .and_then(Path::file_stem)
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "ofd".to_string());

        let default_format = self.settings.files.export_format;
        let export_dpi = self.settings.files.export_dpi;
        let receiver = cx.prompt_for_new_path(
            &directory,
            Some(&suggested_name_for(&stem, page, default_format)),
        );
        self.exporting = true;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let target = match receiver.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => {
                    let _ = this.update(cx, |this, cx| {
                        this.exporting = false;
                        cx.notify();
                    });
                    return;
                }
                Ok(Err(error)) => {
                    let message = format!("无法打开保存对话框：{error}");
                    let _ = this.update(cx, |this, cx| {
                        this.exporting = false;
                        this.set_notice(message, true, cx);
                        cx.notify();
                    });
                    return;
                }
                Err(_) => {
                    let _ = this.update(cx, |this, cx| {
                        this.exporting = false;
                        cx.notify();
                    });
                    return;
                }
            };

            let format = ExportFormat::for_path(&target);
            let result = cx
                .background_spawn(async move {
                    if format == ExportFormat::Svg {
                        let svg = doc.page_svg(page).map_err(|error| error.to_string())?;
                        std::fs::write(&target, svg).map_err(|error| error.to_string())?;
                    } else {
                        let rendered = renderer
                            .render_png(&doc, page, export_dpi)
                            .map_err(|error| error.to_string())?;
                        std::fs::write(&target, rendered.png).map_err(|error| error.to_string())?;
                    }
                    Ok::<PathBuf, String>(target)
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.exporting = false;
                match result {
                    Ok(path) => {
                        let name = path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string());
                        this.set_notice(format!("已导出 {name}"), false, cx);
                    }
                    Err(error) => this.set_notice(format!("导出失败：{error}"), true, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn on_open_settings(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.open_settings(cx);
    }

    fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.info_dialog_open = false;
        self.settings_open = true;
        self.settings_reset_armed = false;
        cx.notify();
    }

    fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_open = false;
        self.settings_reset_armed = false;
        self.settings.save();
        cx.notify();
    }

    fn on_noop(&mut self, _: &ClickEvent, _: &mut Window, _: &mut Context<Self>) {}

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let modifiers = &event.keystroke.modifiers;
        if self.settings_open {
            if key == "escape" {
                self.close_settings(cx);
            }
            return;
        }
        if self.info_dialog_open {
            if key == "escape" {
                self.info_dialog_open = false;
                cx.notify();
            }
            return;
        }
        if key == "," && (modifiers.control || modifiers.platform) {
            self.open_settings(cx);
            return;
        }
        if key == "f" && (modifiers.control || modifiers.platform) {
            self.toggle_search(window, cx);
            return;
        }
        if key == "o" && (modifiers.control || modifiers.platform) {
            self.prompt_open(cx);
            return;
        }
        if key == "w" && (modifiers.control || modifiers.platform) {
            if let Some(index) = self.active_tab {
                self.close_tab(index, window, cx);
            }
            return;
        }
        if key == "tab" && (modifiers.control || modifiers.platform) {
            let len = self.tabs.len();
            if len > 1 {
                let current = self.active_tab.unwrap_or(0);
                let next = if modifiers.shift {
                    (current + len - 1) % len
                } else {
                    (current + 1) % len
                };
                self.activate_tab(next, Some(window), cx);
            }
            cx.stop_propagation();
            return;
        }
        if self.search_open {
            match key {
                "escape" => self.close_search(window, cx),
                "up" => self.search_step(-1, cx),
                "down" => self.search_step(1, cx),
                _ => {}
            }
            return;
        }
        if self.grid_open {
            if key == "escape" {
                self.grid_open = false;
                cx.notify();
            } else if key == "down" || key == "j" {
                self.go_to_page((self.page + 1).min(self.page_count.saturating_sub(1)), cx);
            } else if key == "up" || key == "k" {
                self.go_to_page(self.page.saturating_sub(1), cx);
            }
            return;
        }
        if key == "c" && (modifiers.control || modifiers.platform) {
            self.copy_selection(cx);
            return;
        }
        if !modifiers.control && !modifiers.platform && !modifiers.alt && self.doc.is_some() {
            match key {
                "r" => {
                    self.rotate_view(cx);
                    return;
                }
                "g" => {
                    self.toggle_grid(cx);
                    return;
                }
                _ => {}
            }
        }
        if key == "escape" && self.selection_anchor.is_some() {
            self.clear_selection();
            cx.notify();
            return;
        }
        if self.view_mode == ViewMode::Continuous {
            let viewport = window.viewport_size();
            let page_step = (f32::from(viewport.height) - 180.0).max(160.0);
            match key {
                "down" | "j" => self.scroll_continuous(120.0, cx),
                "up" | "k" => self.scroll_continuous(-120.0, cx),
                "pagedown" | "space" if !modifiers.shift => self.scroll_continuous(page_step, cx),
                "pageup" | "space" => self.scroll_continuous(-page_step, cx),
                "left" => self.prev_page(cx),
                "right" => self.next_page(cx),
                "home" => {
                    self.continuous_scroll.set_offset(point(px(0.0), px(0.0)));
                    self.go_to_page(0, cx);
                }
                "end" => {
                    self.continuous_scroll.scroll_to_bottom();
                    self.go_to_page(self.page_count.saturating_sub(1), cx);
                }
                "=" | "+" => self.set_zoom(self.zoom + 0.25, cx),
                "-" | "_" => self.set_zoom(self.zoom - 0.25, cx),
                "0" => self.set_zoom(1.0, cx),
                _ => return,
            }
            return;
        }
        match key {
            "pagedown" | "right" | "down" | "j" | "space" if !modifiers.shift => self.next_page(cx),
            "pageup" | "left" | "up" | "k" | "space" => self.prev_page(cx),
            "home" => self.go_to_page(0, cx),
            "end" => self.go_to_page(self.page_count.saturating_sub(1), cx),
            "=" | "+" => self.set_zoom(self.zoom + 0.25, cx),
            "-" | "_" => self.set_zoom(self.zoom - 0.25, cx),
            "0" => self.set_zoom(1.0, cx),
            _ => return,
        }
        cx.notify();
    }

    fn titlebar(&self, corners: Corners<Pixels>) -> impl IntoElement {
        let p = self.palette();
        div()
            .h(px(44.0))
            .flex_shrink_0()
            .rounded_tl(corners.top_left)
            .rounded_tr(corners.top_right)
            .flex()
            .items_center()
            .bg(rgb(p.titlebar))
            .border_b_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .h_full()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .px_4()
                    .on_mouse_down(MouseButton::Left, |_, window, _| {
                        window.start_window_move();
                    })
                    .child(
                        div()
                            .size(px(20.0))
                            .rounded(px(5.0))
                            .bg(rgb(p.accent))
                            .flex()
                            .justify_center()
                            .items_center()
                            .text_size(px(12.0))
                            .text_color(rgb(0xFFFFFF))
                            .child(IconName::FileText),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(p.text))
                            .child("VofdReader"),
                    )
                    .child(div().w(px(1.0)).h(px(14.0)).bg(rgb(p.border)))
                    .child(
                        div()
                            .max_w(px(520.0))
                            .overflow_hidden()
                            .text_size(px(12.5))
                            .text_color(if self.file_name.is_empty() {
                                rgb(p.text_tertiary)
                            } else {
                                rgb(p.text)
                            })
                            .child(if self.file_name.is_empty() {
                                "未打开文件".to_string()
                            } else {
                                self.file_name.clone()
                            }),
                    )
                    .child(if self.page_count > 0 {
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(p.text_tertiary))
                            .child(format!("· {} 页", self.page_count))
                    } else {
                        div()
                    }),
            )
            .child(
                div()
                    .h_full()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .id("window-min")
                            .w(px(36.0))
                            .h_full()
                            .flex()
                            .justify_center()
                            .items_center()
                            .cursor_pointer()
                            .text_size(px(12.0))
                            .text_color(rgb(p.text_secondary))
                            .hover(|style| {
                                style
                                    .bg(rgb(p.window_control_hover))
                                    .text_color(rgb(p.text))
                            })
                            .active(|style| {
                                style
                                    .bg(rgb(p.window_control_active))
                                    .text_color(rgb(p.text))
                            })
                            .child(div().text_size(px(14.0)).child(IconName::Minus))
                            .on_click(|_, window, _| window.minimize_window()),
                    )
                    .child(
                        div()
                            .id("window-max")
                            .w(px(36.0))
                            .h_full()
                            .flex()
                            .justify_center()
                            .items_center()
                            .cursor_pointer()
                            .text_size(px(12.0))
                            .text_color(rgb(p.text_secondary))
                            .hover(|style| {
                                style
                                    .bg(rgb(p.window_control_hover))
                                    .text_color(rgb(p.text))
                            })
                            .active(|style| {
                                style
                                    .bg(rgb(p.window_control_active))
                                    .text_color(rgb(p.text))
                            })
                            .child(div().text_size(px(12.0)).child(IconName::Square))
                            .on_click(|_, window, _| window.zoom_window()),
                    )
                    .child(
                        div()
                            .id("window-close")
                            .w(px(36.0))
                            .h_full()
                            .flex()
                            .justify_center()
                            .items_center()
                            .cursor_pointer()
                            .text_size(px(12.0))
                            .text_color(rgb(p.text_secondary))
                            // Keep the close button's hover fill inside the
                            // window's rounded top-right corner.
                            .rounded_tr(corners.top_right)
                            .hover(|style| {
                                style
                                    .bg(rgb(p.window_close_hover))
                                    .text_color(rgb(0xFFFFFF))
                            })
                            .active(|style| {
                                style
                                    .bg(rgb(p.window_close_active))
                                    .text_color(rgb(0xFFFFFF))
                            })
                            .child(div().text_size(px(13.0)).child(IconName::X))
                            .on_click(|_, window, cx| {
                                window.remove_window();
                                cx.quit();
                            }),
                    ),
            )
    }

    fn tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette();
        let tabs = self.tabs.iter().enumerate().map(|(index, tab)| {
            let active = self.active_tab == Some(index);
            div()
                .id(("doc-tab", index))
                .relative()
                .h_full()
                .w(px(240.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .border_r_1()
                .border_color(rgb(p.border))
                .bg(if active { rgb(p.surface) } else { rgb(p.bg) })
                .text_size(px(11.5))
                .text_color(if active {
                    rgb(p.text)
                } else {
                    rgb(p.text_secondary)
                })
                .cursor_pointer()
                .when(!active, |this| this.hover(|style| style.bg(rgb(p.surface))))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.activate_tab(index, Some(window), cx);
                }))
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |this, _, window, cx| {
                        this.close_tab(index, window, cx);
                    }),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(px(2.0))
                        .bg(if active {
                            rgb(p.accent)
                        } else {
                            rgba(0x00000000)
                        }),
                )
                .child(div().w(px(16.0)).flex_shrink_0())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_center()
                        .text_ellipsis()
                        .child(tab.file_name.clone()),
                )
                .child(
                    div()
                        .id(("doc-tab-close", index))
                        .flex_shrink_0()
                        .size(px(16.0))
                        .rounded(px(4.0))
                        .flex()
                        .justify_center()
                        .items_center()
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(p.window_control_hover)))
                        .child(icon(IconName::X, 10.0, p.text_tertiary))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_tab(index, window, cx);
                        })),
                )
        });

        div()
            .h(px(34.0))
            .flex_shrink_0()
            .flex()
            .items_stretch()
            .bg(rgb(p.bg))
            .border_b_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .overflow_x_scrollbar()
                    .children(tabs),
            )
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette();
        div()
            .h(px(52.0))
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .bg(rgb(p.toolbar))
            .border_b_1()
            .border_color(rgb(p.border))
            .child(
                Button::new("open")
                    .primary()
                    .small()
                    .icon(IconName::FolderOpen)
                    .label("打开")
                    .on_click(cx.listener(Self::on_open)),
            )
            .child(div().w(px(1.0)).h(px(20.0)).bg(rgb(p.border)).mx_1())
            .child(
                div()
                    .id("sidebar-toggle")
                    .w(px(32.0))
                    .h(px(32.0))
                    .rounded(px(7.0))
                    .flex()
                    .justify_center()
                    .items_center()
                    .cursor_pointer()
                    .bg(if self.sidebar_open {
                        rgb(p.accent_soft)
                    } else {
                        rgb(p.surface)
                    })
                    .child(icon(
                        IconName::PanelLeft,
                        16.0,
                        if self.sidebar_open {
                            p.accent
                        } else {
                            p.text_secondary
                        },
                    ))
                    .on_click(cx.listener(Self::on_toggle_sidebar)),
            )
            .child(
                div()
                    .id("search-toggle")
                    .w(px(32.0))
                    .h(px(32.0))
                    .rounded(px(7.0))
                    .flex()
                    .justify_center()
                    .items_center()
                    .cursor_pointer()
                    .child(icon(IconName::Search, 16.0, p.text_secondary))
                    .on_click(cx.listener(Self::on_toggle_search)),
            )
            .child(div().flex_1())
            .child(
                Button::new("print")
                    .ghost()
                    .small()
                    .compact()
                    .text_color(rgb(p.text_secondary))
                    .icon(IconName::Printer)
                    .label("打印")
                    .tooltip("打印（即将支持）")
                    .on_click(cx.listener(Self::on_noop)),
            )
            .child(
                Button::new("export")
                    .ghost()
                    .small()
                    .compact()
                    .text_color(rgb(p.text_secondary))
                    .icon(IconName::Download)
                    .label(if self.exporting {
                        "导出中"
                    } else {
                        "导出"
                    })
                    .tooltip("导出当前页为 PNG 或 SVG")
                    .on_click(cx.listener(Self::on_export)),
            )
            .child(div().w(px(1.0)).h(px(20.0)).bg(rgb(p.border)).mx_1())
            .child(
                Button::new("theme")
                    .ghost()
                    .small()
                    .compact()
                    .text_color(rgb(p.text_secondary))
                    .icon(if self.dark {
                        IconName::Moon
                    } else {
                        IconName::Sun
                    })
                    .tooltip(if self.dark {
                        "切换到亮色主题"
                    } else {
                        "切换到暗色主题"
                    })
                    .on_click(cx.listener(Self::on_toggle_theme)),
            )
            .child(
                Button::new("settings")
                    .ghost()
                    .small()
                    .compact()
                    .text_color(if self.settings_open {
                        rgb(p.accent)
                    } else {
                        rgb(p.text_secondary)
                    })
                    .icon(IconName::Settings)
                    .tooltip("设置")
                    .on_click(cx.listener(Self::on_open_settings)),
            )
            .child({
                let info_entity = cx.entity().downgrade();
                let has_doc = self.doc.is_some();
                Button::new("more")
                    .ghost()
                    .small()
                    .compact()
                    .text_color(rgb(p.text_secondary))
                    .icon(IconName::Ellipsis)
                    .tooltip("更多")
                    .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                        let info_entity = info_entity.clone();
                        menu.item(
                            PopupMenuItem::new("文档信息")
                                .icon(IconName::Info)
                                .disabled(!has_doc)
                                .on_click(move |_, _, cx| {
                                    let _ = info_entity.update(cx, |this, cx| {
                                        this.info_dialog_open = true;
                                        cx.notify();
                                    });
                                }),
                        )
                    })
            })
    }

    fn thumbnail(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let selected = index == self.page;
        let size = self
            .page_sizes
            .get(index)
            .copied()
            .unwrap_or(SizeMm::new(210.0, 297.0));
        let ratio = (size.width / size.height.max(0.01)) as f32;

        let preview = if let Some(image) = self.thumbs.get(index).and_then(|image| image.as_ref()) {
            img(image.clone())
                .size_full()
                .shadow_sm()
                .rounded(px(2.0))
                .into_any_element()
        } else {
            div()
                .size_full()
                .flex()
                .justify_center()
                .items_center()
                .bg(rgb(p.surface))
                .border_1()
                .border_color(rgb(p.border))
                .rounded(px(2.0))
                .text_size(px(10.0))
                .text_color(rgb(p.text_tertiary))
                .child("…")
                .into_any_element()
        };

        div()
            .id(("thumb", index))
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.go_to_page(index, cx);
            }))
            .child(
                div()
                    .w_full()
                    .p(px(2.0))
                    .rounded(px(6.0))
                    .border_2()
                    .border_color(if selected {
                        rgb(p.accent)
                    } else {
                        rgb(p.sidebar)
                    })
                    // The page's own aspect ratio keeps single-column previews
                    // undistorted while filling the sidebar width.
                    .child(div().w_full().aspect_ratio(ratio).child(preview)),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(if selected {
                        rgb(p.accent)
                    } else {
                        rgb(p.text_tertiary)
                    })
                    .child(format!("{}", index + 1)),
            )
            .into_any_element()
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette();
        let visible = self.page_count.min(THUMB_LIMIT);
        let searching = self.search_open;
        let tab = |label: &'static str, active: bool| {
            div()
                .h_full()
                .flex()
                .items_center()
                .border_b_2()
                .border_color(if active {
                    rgb(p.accent)
                } else {
                    rgb(p.sidebar)
                })
                .text_color(if active {
                    rgb(p.text)
                } else {
                    rgb(p.text_tertiary)
                })
                .when(active, |this| this.font_weight(FontWeight::SEMIBOLD))
                .child(label)
        };

        let thumbs = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(32.0))
                    .px_5()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(10.0))
                    .text_color(rgb(p.text_tertiary))
                    .child("页面")
                    .child(format!("{visible}")),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .px_3()
                    .pb_3()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children((0..visible).map(|index| self.thumbnail(index, cx))),
            )
            .into_any_element();

        div()
            .w(px(240.0))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(p.sidebar))
            .border_r_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .h(px(44.0))
                    .flex()
                    .items_center()
                    .gap_4()
                    .px_5()
                    .text_size(px(11.5))
                    .child(tab("缩略图", !searching))
                    .child(tab("目录", false))
                    .child(tab("书签", false))
                    .child(tab("搜索结果", searching)),
            )
            .child(if searching {
                self.sidebar_search_results(cx)
            } else {
                thumbs
            })
    }

    /// Search results replace the thumbnail list while the search panel is
    /// open, keeping the document visible next to a narrow result list.
    fn sidebar_search_results(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let query = self.search_query.trim();
        let query_empty = query.is_empty();
        let count = self.search_hits.len();
        let position = if count == 0 {
            String::new()
        } else {
            format!("{} / {}", self.search_index + 1, count)
        };
        let summary = if query_empty {
            "等待输入".to_string()
        } else if count == 0 {
            "未找到匹配".to_string()
        } else {
            format!("{count} 处匹配")
        };

        let list = if query_empty {
            div()
                .px_4()
                .py_3()
                .text_size(px(11.0))
                .text_color(rgb(p.text_tertiary))
                .child("输入关键词搜索文档内容")
                .into_any_element()
        } else if count == 0 {
            div()
                .px_4()
                .py_3()
                .text_size(px(11.0))
                .text_color(rgb(p.text_tertiary))
                .child("未找到匹配内容")
                .into_any_element()
        } else {
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .id("sidebar-search-results")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&self.search_results_scroll)
                        .pb_2()
                        .flex()
                        .flex_col()
                        .children(self.search_hits.iter().enumerate().map(|(index, hit)| {
                            let current = index == self.search_index;
                            let snippet = highlight_matches(&hit.snippet, query, p.search_match);
                            div()
                                .id(("sidebar-search-hit", index))
                                .w_full()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .px_4()
                                .py_3()
                                // The accent rail marks the result that Enter and the
                                // arrow keys currently point at.
                                .border_l_2()
                                .border_color(if current {
                                    rgb(p.accent)
                                } else {
                                    rgba(0x00000000)
                                })
                                .bg(if current {
                                    rgb(p.surface)
                                } else {
                                    rgb(p.sidebar)
                                })
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(p.surface)))
                                .child(
                                    div()
                                        .flex()
                                        .justify_between()
                                        .text_size(px(10.0))
                                        .child(
                                            div()
                                                .text_color(rgb(p.accent))
                                                .child(format!("第 {} 页", hit.page + 1)),
                                        )
                                        .child(
                                            div()
                                                .text_color(rgb(p.text_tertiary))
                                                .child(format!("{}", index + 1)),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(rgb(p.text_secondary))
                                        .child(snippet),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.select_search_hit(index, cx)
                                }))
                        }))
                        .child(
                            div()
                                .mx_4()
                                .mt_3()
                                .border_t_1()
                                .border_color(rgb(p.border))
                                .pt_3()
                                .pb_3()
                                .text_size(px(10.5))
                                .text_color(rgb(p.text_tertiary))
                                .text_center()
                                .child(if count >= SEARCH_LIMIT {
                                    format!("已显示前 {count} 条结果")
                                } else {
                                    "已显示全部结果".to_string()
                                }),
                        ),
                )
                .vertical_scrollbar(&self.search_results_scroll)
                .into_any_element()
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(32.0))
                    .px_5()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(10.0))
                    .text_color(rgb(p.text_tertiary))
                    .child(summary)
                    .child(position),
            )
            .child(list)
            .into_any_element()
    }

    fn info_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.info_dialog_open || self.doc.is_none() {
            return div().into_any_element();
        }
        let p = self.palette();
        let doc = self.doc.as_ref().expect("checked above");
        let info = &doc.ofd.info;
        let signed = doc.has_signature_material();
        let size = self
            .page_sizes
            .get(self.page)
            .map(|size| format!("{:.0} × {:.0} mm", size.width, size.height))
            .unwrap_or_else(|| "—".to_string());
        let path = self
            .file_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "—".to_string());
        let rows = [
            ("标题", info.title.clone().unwrap_or_else(|| "—".into())),
            ("文件", self.file_name.clone()),
            (
                "格式",
                format!("OFD {} / {}", doc.ofd.version, doc.ofd.doc_type),
            ),
            ("文档 ID", info.doc_id.clone().unwrap_or_else(|| "—".into())),
            ("页面", format!("{} 页 · 当前 {}", self.page_count, size)),
            ("大小", human_size(self.file_size)),
            ("作者", info.author.clone().unwrap_or_else(|| "—".into())),
            ("创建者", info.creator.clone().unwrap_or_else(|| "—".into())),
            (
                "创建日期",
                info.creation_date
                    .clone()
                    .filter(|date| !date.is_empty())
                    .unwrap_or_else(|| "—".into()),
            ),
            (
                "修改日期",
                info.mod_date
                    .clone()
                    .filter(|date| !date.is_empty())
                    .unwrap_or_else(|| "—".into()),
            ),
            (
                "签名",
                if signed {
                    "含签名（未验证）".to_string()
                } else {
                    "无签名".to_string()
                },
            ),
        ];

        div()
            .id("info-dialog-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_center()
            .bg(rgba(0x0E111680))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.info_dialog_open = false;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id("info-dialog")
                    .w(px(470.0))
                    .max_w(px(560.0))
                    .max_h(px(560.0))
                    .overflow_y_scrollbar()
                    .rounded(px(12.0))
                    .bg(rgb(p.surface))
                    .border_1()
                    .border_color(rgb(p.border))
                    .shadow_lg()
                    .text_size(px(11.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .child(
                        div()
                            .px_5()
                            .pt_4()
                            .pb_3()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(p.text))
                                    .child("文档信息"),
                            )
                            .child(
                                div()
                                    .id("info-dialog-close")
                                    .size(px(22.0))
                                    .rounded(px(6.0))
                                    .flex()
                                    .justify_center()
                                    .items_center()
                                    .cursor_pointer()
                                    .child(icon(IconName::X, 13.0, p.text_tertiary))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.info_dialog_open = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(div().mx_5().h(px(1.0)).bg(rgb(p.border)))
                    .child(
                        div()
                            .px_5()
                            .py_3()
                            .flex()
                            .flex_col()
                            .gap_2p5()
                            .children(rows.iter().map(|(label, value)| info_row(label, value, p))),
                    )
                    .child(
                        div()
                            .mx_5()
                            .mb_5()
                            .mt_1()
                            .pt_3()
                            .border_t_1()
                            .border_color(rgb(p.border))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(rgb(p.text_tertiary))
                                    .child("文件路径"),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .text_size(px(10.5))
                                    .text_color(rgb(p.text_secondary))
                                    .child(path),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn floating_bar(&self, cx: &mut Context<Self>, viewport: Size<Pixels>) -> impl IntoElement {
        let p = self.palette();
        let actual = self.zoom_percent(viewport);
        let page_fit = self.zoom <= self.fit_page_zoom(viewport) + 0.001;

        div()
            .id("floating-bar")
            .absolute()
            .on_mouse_move(cx.listener(Self::on_pointer_move))
            .bottom(px(16.0))
            .left(px(0.0))
            .right(px(0.0))
            .flex()
            .justify_center()
            .child(
                div()
                    .id("floating-bar-controls")
                    .on_hover(cx.listener(|this, hovered, _, _| {
                        this.bar_hovered = *hovered;
                        this.bar_last_activity = Instant::now();
                    }))
                    .h(px(36.0))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .bg(rgb(p.surface))
                    .border_1()
                    .border_color(rgb(p.border_strong))
                    .rounded(px(18.0))
                    .shadow_lg()
                    .child(
                        div()
                            .id("bar-zoom-out")
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(7.0))
                            .flex()
                            .justify_center()
                            .items_center()
                            .cursor_pointer()
                            .child(icon(IconName::Minus, 15.0, 0x4B5563))
                            .on_click(cx.listener(Self::on_zoom_out)),
                    )
                    .child(
                        div()
                            .min_w(px(42.0))
                            .text_size(px(11.5))
                            .text_color(rgb(p.text))
                            .text_center()
                            .child(format!("{actual:.0}%")),
                    )
                    .child(
                        div()
                            .id("bar-zoom-in")
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(7.0))
                            .flex()
                            .justify_center()
                            .items_center()
                            .cursor_pointer()
                            .child(icon(IconName::Plus, 15.0, 0x4B5563))
                            .on_click(cx.listener(Self::on_zoom_in)),
                    )
                    .child(div().w(px(1.0)).h(px(20.0)).bg(rgb(p.border)).mx_1())
                    .child(
                        div()
                            .id("bar-fit-width")
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(7.0))
                            .flex()
                            .justify_center()
                            .items_center()
                            .cursor_pointer()
                            .child(icon(IconName::MoveHorizontal, 15.0, 0x4B5563))
                            .on_click(cx.listener(Self::on_fit_width)),
                    )
                    .child(
                        div()
                            .id("bar-fit-page")
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(7.0))
                            .flex()
                            .justify_center()
                            .items_center()
                            .cursor_pointer()
                            .bg(if page_fit {
                                rgb(p.accent_soft)
                            } else {
                                rgb(p.surface)
                            })
                            .child(icon(
                                IconName::Scan,
                                15.0,
                                if page_fit { p.accent } else { p.text_secondary },
                            ))
                            .on_click(cx.listener(Self::on_fit)),
                    )
                    .child(
                        div()
                            .id("bar-view-mode")
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(7.0))
                            .flex()
                            .justify_center()
                            .items_center()
                            .cursor_pointer()
                            .bg(if self.view_mode == ViewMode::Continuous {
                                rgb(p.accent_soft)
                            } else {
                                rgb(p.surface)
                            })
                            .child(icon(
                                if self.view_mode == ViewMode::Continuous {
                                    IconName::Rows3
                                } else {
                                    IconName::FileText
                                },
                                15.0,
                                if self.view_mode == ViewMode::Continuous {
                                    p.accent
                                } else {
                                    0x4B5563
                                },
                            ))
                            .on_click(cx.listener(Self::on_toggle_view_mode)),
                    )
                    .child(div().w(px(1.0)).h(px(20.0)).bg(rgb(p.border)).mx_1())
                    .child(
                        div()
                            .id("bar-rotate")
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(7.0))
                            .flex()
                            .justify_center()
                            .items_center()
                            .cursor_pointer()
                            .child(icon(
                                IconName::RotateCw,
                                15.0,
                                if self.rotation == 0 {
                                    0x4B5563
                                } else {
                                    p.accent
                                },
                            ))
                            .on_click(cx.listener(Self::on_rotate)),
                    )
                    .child(
                        div()
                            .id("bar-view")
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(7.0))
                            .flex()
                            .justify_center()
                            .items_center()
                            .cursor_pointer()
                            .bg(if self.grid_open {
                                rgb(p.accent_soft)
                            } else {
                                rgb(p.surface)
                            })
                            .child(icon(
                                IconName::Grid2x2,
                                15.0,
                                if self.grid_open { p.accent } else { 0x4B5563 },
                            ))
                            .on_click(cx.listener(Self::on_toggle_grid)),
                    ),
            )
    }

    fn search_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let Some(input) = &self.search_input else {
            return div().into_any_element();
        };
        let query_empty = self.search_query.trim().is_empty();
        let count = self.search_hits.len();

        let bar = div()
            .h(px(42.0))
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .bg(rgb(p.surface))
            .border_1()
            .border_color(rgb(p.border_strong))
            .rounded(px(10.0))
            .shadow_lg()
            .child(icon(IconName::Search, 16.0, p.accent))
            .child(
                div()
                    .flex_1()
                    // Arrow keys move through the sidebar results instead of
                    // the single-line input's caret.
                    .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        match event.keystroke.key.as_str() {
                            "up" => {
                                this.search_step(-1, cx);
                                cx.stop_propagation();
                            }
                            "down" => {
                                this.search_step(1, cx);
                                cx.stop_propagation();
                            }
                            _ => {}
                        }
                    }))
                    .child(
                        Input::new(input)
                            .appearance(false)
                            .focus_bordered(false)
                            .text_size(px(12.5)),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(p.text_tertiary))
                    .child(if query_empty {
                        String::new()
                    } else {
                        format!("{count} 处")
                    }),
            )
            .child(
                div()
                    .id("search-prev")
                    .size(px(24.0))
                    .rounded(px(6.0))
                    .flex()
                    .justify_center()
                    .items_center()
                    .cursor_pointer()
                    .child(icon(IconName::ChevronUp, 14.0, p.text_secondary))
                    .on_click(cx.listener(|this, _, _, cx| this.search_step(-1, cx))),
            )
            .child(
                div()
                    .id("search-next")
                    .size(px(24.0))
                    .rounded(px(6.0))
                    .flex()
                    .justify_center()
                    .items_center()
                    .cursor_pointer()
                    .child(icon(IconName::ChevronDown, 14.0, p.text_secondary))
                    .on_click(cx.listener(|this, _, _, cx| this.search_step(1, cx))),
            )
            .child(
                div()
                    .id("search-close")
                    .size(px(24.0))
                    .rounded(px(6.0))
                    .flex()
                    .justify_center()
                    .items_center()
                    .cursor_pointer()
                    .child(icon(IconName::X, 13.0, p.text_tertiary))
                    .on_click(cx.listener(|this, _, window, cx| this.close_search(window, cx))),
            );

        div()
            .absolute()
            .top(px(12.0))
            .left(px(0.0))
            .right(px(0.0))
            .flex()
            .justify_center()
            .child(div().w(px(520.0)).flex().flex_col().gap_2().child(bar))
            .into_any_element()
    }

    fn clear_tiles(&mut self) {
        self.tile_cache.clear();
        self.tile_order.clear();
        self.tile_wanted.clear();
        self.tile_prefetch.clear();
        self.tile_failed.clear();
        self.tile_bytes = 0;
        // An old document's tasks keep their slots until they return.
    }

    /// Tiles carry the zoomed-in detail above the whole-page bitmap: they
    /// exist only when the wanted density exceeds the page's base density.
    fn tile_density(&self, page: usize) -> Option<u32> {
        if page >= self.page_sizes.len() {
            return None;
        }
        let (scale, visible) = if self.view_mode == ViewMode::Continuous {
            (
                self.continuous_page_scale(self.viewport, page),
                self.continuous_page_slots(self.viewport, page),
            )
        } else {
            (self.fit_scale(self.viewport) * self.zoom, 1)
        };
        let base = self.base_dpi(page, visible);
        // Keep the visible tile working set (viewport area in page pixels at
        // the tile density) inside its reservation.
        let area =
            f32::from(self.viewport.width).max(1.0) * f32::from(self.viewport.height).max(1.0);
        let fit = (self.cache_config.tile_working_bytes() as f32 / (area * 8.0)).sqrt();
        let budget = ((96.0 * scale * fit / 24.0).floor() * 24.0).max(96.0) as u32;
        let dpi = self.wanted_dpi().min(budget);
        (dpi > base).then_some(dpi)
    }

    fn nearby_page_range(&self) -> std::ops::Range<usize> {
        let actual = if self.view_mode == ViewMode::Continuous {
            self.continuous_content_range(self.viewport)
        } else if self.page_count > 0 {
            self.page..self.page + 1
        } else {
            0..0
        };
        continuous::prefetch_range(actual, self.page_count, self.cache_config.prefetch_pages())
    }

    fn schedule_nearby(&mut self, cx: &mut Context<Self>) {
        // Continuous scrolling keeps prefetching so pages are ready before
        // they appear; single-page wheel bursts still coalesce.
        if self.doc.is_none()
            || self.renderer.is_none()
            || self.grid_open
            || (self.view_mode == ViewMode::Single && self.continuous_scrolling())
            || self.dpi_pending
            || self.latest_render.is_some()
            || self.loading
        {
            return;
        }
        // Fill every free slot with visible tiles first; nearby pages and
        // speculative tiles wait until no tile is running.
        self.schedule_tile(false, cx);
        if self.tile_busy() {
            return;
        }
        let actual = if self.view_mode == ViewMode::Continuous {
            self.continuous_content_range(self.viewport)
        } else {
            self.page..self.page + 1
        };
        let mut pages: Vec<_> = self
            .nearby_page_range()
            .filter(|page| !actual.contains(page))
            .collect();
        // Pages ahead in the scroll direction first; a page behind counts as
        // twice as far, it was usually just on screen and is still cached.
        let down = self.scroll_down;
        pages.sort_by_key(|page| {
            let ahead = (*page > self.page) == down;
            (page.abs_diff(self.page) * if ahead { 1 } else { 2 }, !ahead)
        });
        // Neighbours use the visible pages' density; when memory is short,
        // fewer pages are prefetched instead of blurrier ones. Only the space
        // left after the on-screen pages counts: prefetching more than the
        // cache holds evicts one neighbour per finished neighbour, forever.
        let dpi_of = |this: &Self, page: usize| {
            if this.view_mode == ViewMode::Continuous {
                this.continuous_dpi(this.viewport, page)
            } else {
                this.base_dpi(page, 1)
            }
        };
        // On-screen pages keep every cached density (they are never evicted).
        let reserved: usize = actual
            .clone()
            .map(|page| {
                let cached = self.cached_page_bytes(|(cached, _)| cached == page);
                if cached > 0 {
                    cached
                } else {
                    page_raster_bytes(self.page_sizes[page], dpi_of(self, page))
                }
            })
            .sum();
        let room = self.cache_config.page_bytes().saturating_sub(reserved);
        let slots = self.cache_config.page_limit().saturating_sub(actual.len());
        let sizes: Vec<_> = pages
            .iter()
            .map(|&page| {
                let dpi = dpi_of(self, page);
                // A sharper cached bitmap is what stays resident.
                let sharpest = self
                    .page_cache
                    .keys()
                    .filter(|(cached, cached_dpi)| *cached == page && *cached_dpi >= dpi)
                    .map(|(_, cached_dpi)| *cached_dpi)
                    .min();
                let bytes = match sharpest {
                    Some(found) => self.cached_page_bytes(|key| key == (page, found)),
                    None => page_raster_bytes(self.page_sizes[page], dpi),
                };
                (page, dpi, bytes)
            })
            .collect();
        let fit = continuous::prefetch_fit(sizes.iter().map(|(_, _, bytes)| *bytes), room, slots);
        for &(page, dpi, _) in &sizes[..fit] {
            self.request_cached_page(page, dpi, cx);
            if !continuous::render_ready(self.raster_jobs(), self.tile_slots) {
                return;
            }
        }
        self.schedule_tile(true, cx);
    }

    fn tile_requests(&self, include_prefetch: bool) -> Vec<tiles::Key> {
        let visible: Vec<_> = self.tile_wanted.values().flatten().copied().collect();
        let extra = if include_prefetch {
            self.tile_prefetch
                .values()
                .flat_map(|keys| keys.iter().enumerate().map(|(rank, key)| (rank, *key)))
                .collect()
        } else {
            vec![]
        };
        tiles::requests(visible, extra, self.page, self.cache_config.tile_bytes())
    }

    fn tile_busy(&self) -> bool {
        !self.tile_jobs.is_empty()
    }

    /// Start tile jobs in priority order until every slot is used. Tiles of
    /// one page share the renderer's prepared page, so they run in parallel.
    fn schedule_tile(&mut self, include_prefetch: bool, cx: &mut Context<Self>) {
        while continuous::render_ready(self.raster_jobs(), self.tile_slots) {
            if !self.start_tile(include_prefetch, cx) {
                break;
            }
        }
    }

    /// Start the most important missing tile; false when nothing started.
    fn start_tile(&mut self, include_prefetch: bool, cx: &mut Context<Self>) -> bool {
        // Continuous scrolling keeps rendering visible tiles; page and tile
        // jobs share the render slots (see `raster_jobs`).
        if (self.view_mode == ViewMode::Single && self.continuous_scrolling())
            || self.dpi_pending
            || self.latest_render.is_some()
        {
            return false;
        }
        let wanted = self.tile_requests(include_prefetch);
        let Some(key) = wanted.iter().copied().find(|key| {
            !self.tile_cache.contains_key(key)
                && !self.tile_failed.contains(key)
                && !self.tile_jobs.contains_key(key)
        }) else {
            return false;
        };
        let (Some(doc), Some(renderer)) = (self.doc.clone(), self.renderer.clone()) else {
            return false;
        };
        let size = self.page_sizes[key.page];
        let (width, height) = rotate::size(
            (size.width * key.dpi as f64 / 25.4).ceil() as f32,
            (size.height * key.dpi as f64 / 25.4).ceil() as f32,
            self.rotation,
        );
        let (width, height) = (width as u32, height as u32);
        // `key` lives in rotated view space; rasterize the matching source
        // region and rotate the pixels back into that same space.
        let (_, sample) = key.regions(width, height);
        let region = rotate::source_region(width, height, self.rotation, sample);
        let bytes = sample.width as usize * sample.height as usize * 4;
        let protected_bytes: usize = wanted
            .iter()
            .filter_map(|key| self.tile_cache.get(key))
            .map(|image| {
                let size = image.size(0);
                size.width.0 as usize * size.height.0 as usize * 4
            })
            .sum();
        let in_flight: usize = self.tile_jobs.values().sum();
        if protected_bytes + in_flight + bytes > self.cache_config.tile_bytes() {
            return false;
        }
        let generation = self.doc_generation;
        let view_generation = self.view_generation;
        let rotation = self.rotation;
        self.tile_jobs.insert(key, bytes);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let raster = renderer
                        .rasterize_region(&doc, key.page, key.dpi as f32, region)
                        .map_err(|e| e.to_string())?;
                    let (width, height, pixels) =
                        rotate::owned(raster.width, raster.height, raster.bgra, rotation)
                            .ok_or("无效分块缓冲区")?;
                    if (width, height) != (sample.width, sample.height) {
                        return Err("分块旋转尺寸不一致".to_string());
                    }
                    let bytes = width as usize * height as usize * 4;
                    let buffer = image::RgbaImage::from_raw(width, height, pixels)
                        .ok_or("无效分块缓冲区")?;
                    Ok::<_, String>((
                        Arc::new(RenderImage::new([image::Frame::new(buffer)])),
                        bytes,
                    ))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.tile_jobs.remove(&key);
                if generation == this.doc_generation
                    && view_generation == this.view_generation
                    && this.tile_requests(true).contains(&key)
                {
                    match result {
                        Ok((image, bytes)) => {
                            let protected = this.tile_requests(include_prefetch);
                            while this.tile_bytes + bytes > this.cache_config.tile_bytes() {
                                let Some(position) = this
                                    .tile_order
                                    .iter()
                                    .position(|old| !protected.contains(old))
                                else {
                                    break;
                                };
                                let old = this.tile_order.remove(position).unwrap();
                                if let Some(image) = this.tile_cache.remove(&old) {
                                    let size = image.size(0);
                                    this.tile_bytes -=
                                        size.width.0 as usize * size.height.0 as usize * 4;
                                }
                            }
                            if this.tile_bytes + bytes <= this.cache_config.tile_bytes() {
                                this.gpu.borrow_mut().register(image.clone());
                                this.tile_cache.insert(key, image);
                                this.tile_order.push_back(key);
                                this.tile_bytes += bytes;
                            }
                        }
                        Err(error) => {
                            this.tile_failed.insert(key);
                            eprintln!("[tiles] {key:?}: {error}");
                        }
                    }
                }
                // Next frame re-evaluates actual clipping before requesting work.
                if this.view_mode == ViewMode::Single
                    && this.page_image.is_none()
                    && this.latest_render.is_none()
                {
                    this.request_page_render(cx);
                }
                // Prefetched tiles stay in the cache until they scroll in.
                let visible = this.tile_wanted.values().any(|keys| keys.contains(&key));
                this.finish_background(visible, cx);
            });
        })
        .detach();
        true
    }

    fn tile_layer(&self, page: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let dpi = self.tile_density(page);
        let size = self
            .page_sizes
            .get(page)
            .copied()
            .unwrap_or(SizeMm::new(210.0, 297.0));
        let cache = self.tile_cache.clone();
        let gpu = self.gpu.clone();
        let entity = cx.entity().downgrade();
        let generation = self.doc_generation;
        let view_generation = self.view_generation;
        let rotation = self.rotation;
        let ring = self.cache_config.prefetch_tile_ring();
        let (rotated_w, rotated_h) = rotate::size(
            size.width as f32 * dpi.unwrap_or(1) as f32 / 25.4,
            size.height as f32 * dpi.unwrap_or(1) as f32 / 25.4,
            rotation,
        );
        let (exact_w, exact_h) = (rotated_w, rotated_h);
        canvas(
            move |bounds, window, cx| {
                let (keys, nearby) = if let Some(dpi) = dpi {
                    let width = exact_w.ceil() as u32;
                    let height = exact_h.ceil() as u32;
                    let clip = bounds.intersect(&window.content_mask().bounds);
                    let sx = exact_w / f32::from(bounds.size.width);
                    let sy = exact_h / f32::from(bounds.size.height);
                    let keys = tiles::plan(
                        page,
                        dpi,
                        (width, height),
                        [
                            f32::from(clip.origin.x - bounds.origin.x) * sx,
                            f32::from(clip.origin.y - bounds.origin.y) * sy,
                            f32::from(clip.origin.x + clip.size.width - bounds.origin.x) * sx,
                            f32::from(clip.origin.y + clip.size.height - bounds.origin.y) * sy,
                        ],
                    );
                    let nearby = tiles::neighbors(&keys, (width, height), ring);
                    (keys, nearby)
                } else {
                    (vec![], vec![])
                };
                let wanted = keys.clone();
                cx.defer(move |cx| {
                    let _ = entity.update(cx, |this, _| {
                        if this.doc_generation == generation
                            && this.view_generation == view_generation
                        {
                            for key in &wanted {
                                if this.tile_cache.contains_key(key) {
                                    this.tile_order.retain(|old| old != key);
                                    this.tile_order.push_back(*key);
                                }
                            }
                            this.tile_wanted.insert(page, wanted);
                            this.tile_prefetch.insert(page, nearby);
                        }
                    });
                    // Run after every page has contributed its prepaint plan.
                    // A speculative block on page A must not delay a visible
                    // block on page B just because A's callback ran first.
                    cx.defer(move |cx| {
                        let _ = entity.update(cx, |this, cx| {
                            if this.doc_generation == generation
                                && this.view_generation == view_generation
                            {
                                this.schedule_nearby(cx);
                            }
                        });
                    });
                });
                keys
            },
            move |bounds, keys, window, _| {
                let Some(_) = dpi else {
                    return;
                };
                let sx = f32::from(bounds.size.width) / exact_w;
                let sy = f32::from(bounds.size.height) / exact_h;
                for key in keys {
                    let Some(image) = cache.get(&key) else {
                        continue;
                    };
                    let (content, sample) =
                        key.regions(exact_w.ceil() as u32, exact_h.ceil() as u32);
                    let rect = |r: ofd_core::RasterRegion| {
                        Bounds::new(
                            point(
                                bounds.origin.x + px(r.x as f32 * sx),
                                bounds.origin.y + px(r.y as f32 * sy),
                            ),
                            gpui_kit::size(px(r.width as f32 * sx), px(r.height as f32 * sy)),
                        )
                    };
                    let clip = rect(content)
                        .intersect(&bounds)
                        .intersect(&window.content_mask().bounds);
                    window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                        let sample_bounds = rect(sample);
                        if window
                            .paint_image(
                                sample_bounds,
                                sample_bounds,
                                Corners::all(px(0.0)),
                                image.clone(),
                                0,
                                false,
                            )
                            .is_ok()
                        {
                            gpu.borrow_mut().painted(image);
                        }
                    });
                }
            },
        )
        .absolute()
        .size_full()
    }

    fn sync_gpu_resources(&mut self, window: &mut Window) {
        let mut live: HashSet<_> = self.page_cache.values().map(|image| image.id).collect();
        live.extend(self.tile_cache.values().map(|image| image.id));
        if let Some(image) = &self.page_image {
            live.insert(image.id);
        }
        live.extend(self.thumbs.iter().flatten().map(|image| image.id));
        self.gpu
            .borrow_mut()
            .reconcile(&live, |image| window.drop_image(image).is_ok());
        self.gpu
            .borrow_mut()
            .frame(self.page_cache_bytes + self.tile_bytes);
    }

    fn page_texture(&self, image: Arc<RenderImage>) -> impl IntoElement + Styled {
        let gpu = self.gpu.clone();
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let visible = bounds.intersect(&window.content_mask().bounds);
                if visible.size.width <= px(0.0) || visible.size.height <= px(0.0) {
                    return;
                }
                if window
                    .paint_image(
                        bounds,
                        bounds,
                        Corners::all(px(1.0)),
                        image.clone(),
                        0,
                        false,
                    )
                    .is_ok()
                {
                    gpu.borrow_mut().painted(&image);
                }
            },
        )
        .absolute()
        .size_full()
    }

    fn page_occlusion(&self) -> impl IntoElement {
        let coverage = self.page_coverage.clone();
        canvas(
            move |bounds, _, _| coverage.borrow_mut().push(bounds),
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
    }

    fn continuous_page_card(
        &self,
        index: usize,
        image: Option<Arc<RenderImage>>,
        viewport: Size<Pixels>,
        current: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let (width, height) = self.continuous_page_size(viewport, index);
        let mut card = div()
            .relative()
            .w(px(width))
            .h(px(height))
            .rounded(px(1.0))
            .when(!self.gpu.borrow().options.no_page_shadows, |card| {
                card.shadow_lg()
            })
            .bg(rgb(p.surface));
        if let Some(image) = image {
            card = card.child(self.page_texture(image));
        } else if let Some(image) = self.thumbs.get(index).and_then(Option::as_ref) {
            card = card.child(img(image.clone()).absolute().size_full());
        } else {
            card = card.child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .items_center()
                    .gap_1()
                    .text_size(px(11.0))
                    .text_color(rgb(p.text_tertiary))
                    .child(format!("第 {} 页", index + 1))
                    .child("正在渲染…"),
            );
        }
        card = card.child(self.tile_layer(index, cx));
        if current {
            let bounds_cell = self.page_bounds.clone();
            card = card
                .on_mouse_down(MouseButton::Left, cx.listener(Self::on_page_mouse_down))
                .on_mouse_up(MouseButton::Left, cx.listener(Self::on_page_mouse_up))
                .child(
                    canvas(
                        move |bounds, _, _| bounds_cell.set(Some(bounds)),
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                );
            if !self.search_highlights.is_empty() {
                card = card.child(overlay_layer(
                    self.search_highlights.clone(),
                    rgba(SEARCH_HIGHLIGHT),
                ));
            }
            if let Some(active) = &self.search_active_highlight {
                card = card.child(overlay_layer(
                    vec![active.clone()],
                    rgba(SEARCH_ACTIVE_HIGHLIGHT),
                ));
            }
            if !self.selection_rects.is_empty() {
                card = card.child(overlay_layer(
                    self.selection_rects.clone(),
                    rgba(0x2F6FEB55),
                ));
            }
        }
        card = card.child(self.page_occlusion());
        div()
            .w_full()
            .flex_shrink_0()
            .flex()
            .justify_center()
            .child(card)
            .into_any_element()
    }

    /// Page overview opened from the floating toolbar. Thumbnail PNGs are
    /// shared with the sidebar; pages past the thumbnail budget show a
    /// lightweight placeholder rather than blocking the grid.
    fn page_grid(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let handle = self.grid_scroll.clone();
        let mut cards = Vec::with_capacity(self.page_count);
        for index in 0..self.page_count {
            let size = self
                .page_sizes
                .get(index)
                .copied()
                .unwrap_or(SizeMm::new(210.0, 297.0));
            let thumb_h =
                (168.0 * size.height as f32 / size.width.max(0.01) as f32).clamp(96.0, 260.0);
            let current = index == self.page;
            let mut thumb = div()
                .relative()
                .w(px(168.0))
                .h(px(thumb_h))
                .overflow_hidden()
                .rounded(px(5.0))
                .border_1()
                .border_color(rgb(if current { p.accent } else { p.border }))
                .bg(rgb(p.surface))
                .shadow_sm();
            if let Some(image) = self.thumbs.get(index).and_then(Option::as_ref) {
                thumb = thumb.child(img(image.clone()).absolute().size_full());
            } else {
                thumb = thumb.child(
                    div()
                        .size_full()
                        .flex()
                        .justify_center()
                        .items_center()
                        .text_size(px(12.0))
                        .text_color(rgb(p.text_tertiary))
                        .child(format!("{}", index + 1)),
                );
            }
            cards.push(
                div()
                    .id(("grid-page", index))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .child(thumb)
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(rgb(if current { p.accent } else { p.text_secondary }))
                            .child(format!("第 {} 页", index + 1)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.grid_open = false;
                        if this.page == index {
                            if this.view_mode == ViewMode::Continuous {
                                this.scroll_to_continuous_page(this.viewport, index);
                            }
                            cx.notify();
                        } else {
                            this.go_to_page(index, cx);
                        }
                    })),
            );
        }
        div()
            .id("page-grid-frame")
            .relative()
            .size_full()
            .child(
                div()
                    .id("page-grid")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&handle)
                    .px_6()
                    .py_5()
                    .child(
                        div()
                            .mb_4()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(12.0))
                            .text_color(rgb(p.text))
                            .child("页面总览")
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(p.text_tertiary))
                                    .child(format!("共 {} 页", self.page_count)),
                            ),
                    )
                    .child(div().flex().flex_wrap().gap_4().children(cards)),
            )
            .vertical_scrollbar(&handle)
            .into_any_element()
    }

    fn continuous_view(&mut self, viewport: Size<Pixels>, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_continuous_pages(viewport, cx);
        let range = self.continuous_visible_range(viewport);
        let (top, bottom, width, page_gap) = self.with_continuous_layout(viewport, |layout| {
            let (top, bottom) = layout.spacers(range.clone());
            (
                top,
                bottom,
                layout.sizes.first().map_or(0.0, |size| size.0),
                layout.gap(),
            )
        });
        let mut pages = Vec::with_capacity(range.len());
        for index in range {
            let dpi = self.continuous_dpi(viewport, index);
            // Keep the old bitmap while scrolling/zooming; refinement waits
            // until idle. Only visible rows touch the LRU or clone images.
            let image = self
                .cached_page_image(index, dpi)
                .map(|(_, image)| image)
                .or_else(|| {
                    let key = self
                        .page_cache
                        .keys()
                        .filter(|(page, _)| *page == index)
                        .max_by_key(|(_, dpi)| *dpi)
                        .copied()?;
                    let image = self.page_cache.get(&key).cloned();
                    self.touch_cached_page(key.0, key.1);
                    image
                });
            let height = self.continuous_page_size(viewport, index).1;
            let gap = if index + 1 < self.page_count {
                page_gap
            } else {
                0.0
            };
            pages.push(
                div()
                    .w(px(width + 48.0))
                    .min_w_full()
                    .h(px(height + gap))
                    .flex_shrink_0()
                    .child(self.continuous_page_card(
                        index,
                        image,
                        viewport,
                        index == self.page,
                        cx,
                    )),
            );
        }
        let handle = self.continuous_scroll.clone();
        // The scrollbar must be a sibling of the scrolling element. Adding it
        // inside an overflow container made it scroll away with the pages.
        div()
            .id("continuous-frame")
            .relative()
            .flex_1()
            .h_full()
            .w_full()
            .min_h_0()
            .child(
                div()
                    .id("continuous-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&handle)
                    .on_scroll(cx.listener(Self::on_continuous_scroll))
                    .flex()
                    .flex_col()
                    .child(div().h(px(top)).flex_shrink_0())
                    .children(pages)
                    .child(div().h(px(bottom)).flex_shrink_0()),
            )
            .vertical_scrollbar(&handle)
            .into_any_element()
    }

    fn status_bar(&self, viewport: Size<Pixels>, corners: Corners<Pixels>) -> impl IntoElement {
        let p = self.palette();
        let actual = self.zoom_percent(viewport);
        let size = self
            .page_sizes
            .get(self.page)
            .map(|size| format!("{:.1} × {:.1} mm", size.width, size.height))
            .unwrap_or_else(|| "—".to_string());

        let signed = self
            .doc
            .as_ref()
            .map(|doc| doc.has_signature_material())
            .unwrap_or(false);

        div()
            .relative()
            .h(px(28.0))
            .flex_shrink_0()
            .rounded_bl(corners.bottom_left)
            .rounded_br(corners.bottom_right)
            .flex()
            .items_center()
            .justify_between()
            .px_4()
            .bg(rgb(p.titlebar))
            .border_t_1()
            .border_color(rgb(p.border))
            .text_size(px(11.0))
            .text_color(rgb(p.text_secondary))
            .child(if self.page_count == 0 {
                "未打开文档".to_string()
            } else {
                format!("第 {} 页，共 {} 页", self.page + 1, self.page_count)
            })
            .child(size)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .children(
                        self.font_reports
                            .get(&self.page)
                            .filter(|report| report.fallback_chars > 0)
                            .map(|report| {
                                div().text_color(rgb(p.warning)).child(format!(
                                    "{} 字已回退（{} 种内嵌字体）",
                                    report.fallback_chars,
                                    report.replaced_fonts.len()
                                ))
                            }),
                    )
                    .child(format!("{actual:.0}%"))
                    .child("OFD 1.0")
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_color(if signed {
                                rgb(p.warning)
                            } else {
                                rgb(p.text_tertiary)
                            })
                            .child(icon(
                                if signed {
                                    IconName::ShieldCheck
                                } else {
                                    IconName::ShieldOff
                                },
                                13.0,
                                if signed { p.warning } else { p.text_tertiary },
                            ))
                            .child(if signed {
                                "含签名（未验证）"
                            } else {
                                "无签名"
                            }),
                    ),
            )
            .children(self.notice.as_ref().map(|(text, error)| {
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .justify_center()
                    .items_center()
                    .text_color(if *error {
                        rgb(p.error)
                    } else {
                        rgb(p.text_secondary)
                    })
                    .child(text.clone())
            }))
    }
}

impl Render for ReaderApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette();
        if let Some(value) = self.search_pending_input.take() {
            cx.defer_in(window, move |this, window, cx| {
                this.set_search_input_value(window, &value, cx);
                cx.notify();
            });
        }
        let root_font = Font {
            family: self.ui_font.family.clone(),
            fallbacks: (!self.ui_font.fallbacks.is_empty())
                .then(|| FontFallbacks::from_fonts(self.ui_font.fallbacks.clone())),
            ..Font::default()
        };
        self.sync_gpu_resources(window);
        self.page_coverage.borrow_mut().clear();
        self.tile_wanted.clear();
        self.tile_prefetch.clear();
        if !self.focused {
            window.focus(&self.focus_handle, cx);
            self.focused = true;
        }
        if self.appearance_subscription.is_none() {
            let reader = cx.entity().downgrade();
            self.appearance_subscription =
                Some(window.observe_window_appearance(move |window, cx| {
                    let dark = matches!(window.appearance(), WindowAppearance::Dark);
                    let _ = reader.update(cx, |this, cx| {
                        if this.settings.appearance.theme == settings::ThemeChoice::System
                            && this.dark != dark
                        {
                            this.dark = dark;
                            apply_component_theme(dark, &this.ui_font, cx);
                            cx.notify();
                        }
                    });
                }));
        }

        self.viewport = window.viewport_size();
        self.window_scale = window.scale_factor();
        if self.view_mode == ViewMode::Single {
            self.pending_restore_page = None;
        }
        if self.doc.is_some()
            && self.view_mode == ViewMode::Continuous
            && !self.grid_open
            && self.pending_restore_page.is_none()
        {
            self.sync_continuous_page(self.viewport, cx);
        }
        if self.doc.is_some()
            && !self.grid_open
            && self.view_mode == ViewMode::Single
            && !self.loading
            && !self.dpi_pending
            && self.target_dpi() > self.image_dpi
        {
            self.schedule_dpi_render(cx);
        }
        // Defensive recovery: the document is open but no frame is displayed.
        // This can happen after switching documents while a stale image_dpi is
        // still around; never show the "open a file" state in that case.
        if self.doc.is_some()
            && !self.grid_open
            && self.view_mode == ViewMode::Single
            && self.page_image.is_none()
            && !self.loading
            && self.error.is_none()
            && self.renderer.is_some()
        {
            self.request_page_render(cx);
        }
        let page = if self.grid_open && self.doc.is_some() {
            self.page_grid(cx)
        } else if self.view_mode == ViewMode::Continuous {
            self.continuous_view(self.viewport, cx)
        } else {
            let (page_w, page_h) = self.display_size(self.viewport);
            if let Some(image) = &self.page_image {
                let bounds_cell = self.page_bounds.clone();
                let mut stack = div()
                    .relative()
                    .w(page_w)
                    .h(page_h)
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_page_mouse_down))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_page_mouse_up))
                    .child(
                        self.page_texture(image.clone())
                            .when(!self.gpu.borrow().options.no_page_shadows, |image| {
                                image.shadow_lg()
                            })
                            .rounded(px(1.0)),
                    )
                    .child(self.page_occlusion())
                    .child(self.tile_layer(self.page, cx))
                    .child(
                        canvas(
                            move |bounds, _, _| {
                                bounds_cell.set(Some(bounds));
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    );
                if !self.search_highlights.is_empty() {
                    stack = stack.child(overlay_layer(
                        self.search_highlights.clone(),
                        rgba(SEARCH_HIGHLIGHT),
                    ));
                }
                if let Some(active) = &self.search_active_highlight {
                    stack = stack.child(overlay_layer(
                        vec![active.clone()],
                        rgba(SEARCH_ACTIVE_HIGHLIGHT),
                    ));
                }
                if !self.selection_rects.is_empty() {
                    stack = stack.child(overlay_layer(
                        self.selection_rects.clone(),
                        rgba(0x2F6FEB55),
                    ));
                }
                stack.into_any_element()
            } else if let Some(error) = &self.error {
                div()
                    .max_w(px(560.0))
                    .p_6()
                    .rounded_lg()
                    .bg(rgb(p.error_soft))
                    .border_1()
                    .border_color(rgb(p.error_border))
                    .text_size(px(13.0))
                    .text_color(rgb(p.error))
                    .children(error.lines().map(|line| div().child(line.to_string())))
                    .into_any_element()
            } else if self.loading || self.doc.is_some() {
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.0))
                    .text_color(rgb(p.text_secondary))
                    .child("正在渲染页面…")
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_3()
                    .text_color(rgb(p.text_secondary))
                    .child(
                        div()
                            .text_size(px(20.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(p.text))
                            .child("打开一个 OFD 文件"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .child("支持电子发票、公文、合同等版式文档，也可直接拖入窗口"),
                    )
                    .child(
                        Button::new("empty-open")
                            .primary()
                            .label("选择文件")
                            .on_click(cx.listener(Self::on_open)),
                    )
                    .children(if self.recent.is_empty() {
                        None
                    } else {
                        Some(
                            div()
                                .mt_2()
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(p.text_tertiary))
                                        .child("最近打开"),
                                )
                                .children(self.recent.iter().take(6).enumerate().map(
                                    |(index, path)| {
                                        let name = path
                                            .file_name()
                                            .map(|name| name.to_string_lossy().into_owned())
                                            .unwrap_or_else(|| path.display().to_string());
                                        let parent = path
                                            .parent()
                                            .map(|parent| parent.display().to_string())
                                            .unwrap_or_default();
                                        let path = path.clone();
                                        div()
                                            .id(("recent", index))
                                            .w(px(420.0))
                                            .px_3()
                                            .py_1()
                                            .rounded(px(6.0))
                                            .flex()
                                            .flex_col()
                                            .items_center()
                                            .cursor_pointer()
                                            .text_size(px(11.5))
                                            .text_color(rgb(p.text_secondary))
                                            .child(name)
                                            .child(
                                                div()
                                                    .max_w(px(400.0))
                                                    .overflow_hidden()
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(p.text_tertiary))
                                                    .child(parent),
                                            )
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.open_path(path.clone(), Some(window), cx);
                                            }))
                                    },
                                )),
                        )
                    })
                    .into_any_element()
            }
        };

        let reader_bounds = self.reader_bounds.clone();
        let mut canvas = div()
            .id("page-canvas")
            .flex_1()
            .min_h_0()
            .relative()
            .flex()
            .justify_center()
            .items_center()
            .bg(rgb(p.canvas))
            .overflow_hidden()
            .on_scroll(cx.listener(Self::on_single_scroll))
            .child(dot_grid(
                p.dot,
                self.gpu.clone(),
                self.page_coverage.clone(),
            ))
            .child(page)
            .child(
                canvas(
                    move |bounds, _, _| reader_bounds.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );

        if self.doc.is_some() && self.settings.reading.floating_toolbar && self.bar_visible {
            canvas = canvas.child(self.floating_bar(cx, window.viewport_size()));
        }
        if self.search_open {
            canvas = canvas.child(self.search_panel(cx));
        }
        // Only on-screen pages: background prefetch neither shows the badge
        // nor needs a repaint to hide it again.
        if self.loading
            || (self.view_mode == ViewMode::Continuous
                && self
                    .continuous_pending
                    .iter()
                    .any(|(page, _)| self.page_on_screen(*page)))
        {
            canvas = canvas.child(
                div()
                    .absolute()
                    .top(px(12.0))
                    .right(px(12.0))
                    .px_3()
                    .py_1()
                    .rounded(px(14.0))
                    .bg(rgb(p.surface))
                    .border_1()
                    .border_color(rgb(p.border))
                    .shadow_sm()
                    .text_size(px(10.5))
                    .text_color(rgb(p.text_tertiary))
                    .child("渲染中…"),
            );
        }

        let mut reading = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .flex()
            .flex_col();
        if self.tabs.len() > 1 {
            reading = reading.child(self.tab_bar(cx));
        }
        let reading = reading.child(canvas);

        let body = if self.doc.is_some() && self.sidebar_open {
            div()
                .flex_1()
                .flex()
                .overflow_hidden()
                .child(self.sidebar(cx))
                .child(reading)
        } else {
            div().flex_1().flex().overflow_hidden().child(reading)
        };

        // GPUI Kit owns the only outer frame. Its border is 1px, so content
        // corners use a concentric 4px radius inside the requested 5px frame.
        let corners = content_corners(window.window_decorations(), window.is_fullscreen());

        div()
            .id("root")
            .relative()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .flex()
            .flex_col()
            .font(root_font)
            .bg(rgb(p.bg))
            .text_color(rgb(p.text))
            .on_mouse_move(cx.listener(Self::on_pointer_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_page_mouse_up))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                let paths = paths.paths();
                let ofd = paths.iter().find(|path| {
                    path.extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("ofd"))
                });
                if let Some(path) = ofd.or_else(|| paths.first()) {
                    this.open_path(path.clone(), Some(window), cx);
                }
            }))
            .rounded_tl(corners.top_left)
            .rounded_tr(corners.top_right)
            .rounded_bl(corners.bottom_left)
            .rounded_br(corners.bottom_right)
            .child(self.titlebar(corners))
            .child(self.toolbar(cx))
            .child(body)
            .child(self.status_bar(window.viewport_size(), corners))
            .child(self.info_dialog(cx))
            .child(self.settings_dialog(window.viewport_size(), cx))
    }
}

/// Re-applies the reader palette to GPUI Kit after a theme mode switch.
/// `Theme::change` loads the registered theme and would otherwise overwrite the
/// accent, so the custom colors are set in a second update.
fn apply_component_theme(dark: bool, ui_font: &font_policy::UiFont, cx: &mut App) {
    let p = Palette::for_mode(dark);
    Theme::change(
        if dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        },
        None,
        cx,
    );
    Theme::update(cx, |theme| {
        theme.colors.primary = rgb(p.accent).into();
        theme.colors.primary_foreground = rgb(0xFFFFFF).into();
        theme.colors.button_primary = rgb(p.accent).into();
        theme.colors.button_primary_hover = rgb(p.primary_hover).into();
        theme.colors.button_primary_active = rgb(p.primary_active).into();
        theme.colors.button_primary_foreground = rgb(0xFFFFFF).into();
        theme.colors.border = rgb(p.border).into();
        theme.colors.foreground = rgb(p.text).into();
        theme.colors.background = rgb(p.bg).into();
        theme.font_family = ui_font.family.clone();
        theme.radius = px(8.0);
    });
}

/// Movement of at least 3 logical pixels counts as pointer activity. GPUI can
/// deliver per-frame moves with sub-pixel jitter while the pointer is still.
/// A completed frame may only replace the displayed page when it belongs to
/// the latest request, targets the visible page, and is not a downgrade.
fn frame_is_current(
    latest: bool,
    page_matches: bool,
    dpi: u32,
    image_dpi: u32,
    image_present: bool,
) -> bool {
    latest && page_matches && (!image_present || dpi >= image_dpi)
}

/// Bottom-center reveal zone, relative to the actual reading canvas rather
/// than the window (sidebar/status bar must not activate the floating bar).
fn near_floating_bar(bounds: Bounds<Pixels>, pointer: Point<Pixels>) -> bool {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let x = f32::from(pointer.x - bounds.origin.x);
    let y = f32::from(pointer.y - bounds.origin.y);
    let half_width = (width * 0.5).min(200.0);
    width > 0.0
        && height > 0.0
        && x >= width * 0.5 - half_width
        && x <= width * 0.5 + half_width
        && y >= (height - 88.0).max(0.0)
        && y <= height
}

/// RGBA bytes of a whole-page bitmap at `dpi`, rounded up by one pixel per
/// side: an underestimate would let prefetch overfill the cache by a few
/// bytes and evict a neighbour on every insert.
fn page_raster_bytes(size: SizeMm, dpi: u32) -> usize {
    let side = |mm: f64| (mm * f64::from(dpi) / 25.4).ceil() as usize + 1;
    side(size.width) * side(size.height) * 4
}

fn pointer_moved(last: Point<Pixels>, current: Point<Pixels>) -> bool {
    let dx = f32::from(current.x - last.x);
    let dy = f32::from(current.y - last.y);
    dx * dx + dy * dy >= 9.0
}

/// Maps a mouse-wheel event to a multiplicative zoom step. Positive deltas
/// (wheel up) zoom in; one notch is about ten percent and a fast flick is
/// capped so a single event cannot jump across the whole zoom range.
fn wheel_zoom_factor(delta: ScrollDelta) -> f32 {
    let lines = match delta {
        ScrollDelta::Lines(delta) => delta.y,
        // Precise touchpad deltas are pixels; 40 px approximates one wheel line.
        ScrollDelta::Pixels(delta) => f32::from(delta.y) / 40.0,
    };
    (1.0 + lines * 0.1).clamp(0.75, 1.25)
}

fn content_corners(decorations: Decorations, fullscreen: bool) -> Corners<Pixels> {
    let Decorations::Client { tiling } = decorations else {
        return Corners::default();
    };
    let radius = |free| {
        if free && !fullscreen {
            px(4.0)
        } else {
            px(0.0)
        }
    };
    Corners {
        top_left: radius(!tiling.top && !tiling.left),
        top_right: radius(!tiling.top && !tiling.right),
        bottom_left: radius(!tiling.bottom && !tiling.left),
        bottom_right: radius(!tiling.bottom && !tiling.right),
    }
}

fn overlay_layer(highlights: Vec<search_highlight::Highlight>, color: Rgba) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            for highlight in &highlights {
                let x = bounds.origin.x + px(f32::from(bounds.size.width) * highlight.x);
                let y = bounds.origin.y + px(f32::from(bounds.size.height) * highlight.y);
                let width = px(f32::from(bounds.size.width) * highlight.width);
                let height = px(f32::from(bounds.size.height) * highlight.height);
                window.paint_quad(quad(
                    Bounds::new(point(x, y), size(width, height)),
                    px(2.0),
                    color,
                    px(0.0),
                    gpui::transparent_black(),
                    Default::default(),
                ));
            }
        },
    )
    .absolute()
    .size_full()
}

fn dot_grid(
    color: u32,
    gpu: Rc<RefCell<gpu::Resources>>,
    coverage: Rc<RefCell<Vec<Bounds<Pixels>>>>,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let mut gpu = gpu.borrow_mut();
            if gpu.options.no_dots {
                return;
            }
            let width = f32::from(bounds.size.width);
            let height = f32::from(bounds.size.height);
            let origin_x = f32::from(bounds.origin.x);
            let origin_y = f32::from(bounds.origin.y);
            let pages = coverage.borrow();
            let clip = bounds.intersect(&window.content_mask().bounds);
            let mut painted = 0;
            for &(x, y) in gpu.dot_points(width, height) {
                let (x, y) = (origin_x + x, origin_y + y);
                let dot = Bounds::new(point(px(x), px(y)), size(px(1.6), px(1.6)));
                let visible = dot.intersect(&clip);
                if visible.size.width <= px(0.0)
                    || visible.size.height <= px(0.0)
                    || !gpu::dot_exposed(x, y, &pages)
                {
                    continue;
                }
                window.paint_quad(quad(
                    dot,
                    px(0.8),
                    rgb(color),
                    px(0.0),
                    gpui::transparent_black(),
                    Default::default(),
                ));
                painted += 1;
            }
            gpu.stats.dot_quads += painted;
        },
    )
    .absolute()
    .size_full()
}

fn icon(name: IconName, size: f32, color: u32) -> impl IntoElement {
    div().text_size(px(size)).text_color(rgb(color)).child(name)
}

/// Short context around a match, trimmed to whole characters.
/// Highlights every occurrence of `query` inside a search snippet.
fn highlight_matches(text: &str, query: &str, color: u32) -> StyledText {
    let highlight = HighlightStyle {
        color: Some(Hsla::from(rgb(color))),
        font_weight: Some(FontWeight::SEMIBOLD),
        ..Default::default()
    };
    StyledText::new(text.to_string()).with_highlights(
        text.match_indices(query)
            .map(move |(start, matched)| (start..start + matched.len(), highlight)),
    )
}

fn search_snippet(text: &str, start: usize, end: usize) -> String {
    let mut begin = start;
    for _ in 0..16 {
        match text[..begin].char_indices().next_back() {
            Some((index, _)) => begin = index,
            None => break,
        }
    }
    let mut finish = end;
    for _ in 0..32 {
        match text[finish..].char_indices().next() {
            Some((_, ch)) => finish += ch.len_utf8(),
            None => break,
        }
    }
    let mut snippet = String::new();
    if begin > 0 {
        snippet.push('…');
    }
    snippet.push_str(text[begin..finish].trim());
    if finish < text.len() {
        snippet.push('…');
    }
    snippet
}

fn settings_cache_config(settings: &settings::Settings) -> cache_config::CacheConfig {
    cache_config::CacheConfig::new(
        settings.performance.cache_mib,
        settings.performance.page_limit,
        settings.performance.parsed_page_limit,
    )
    .with_prefetch(
        settings.performance.prefetch_pages,
        settings.performance.tile_ring,
    )
}

fn info_row(label: &str, value: &str, p: Palette) -> Div {
    div()
        .w_full()
        .flex()
        .items_start()
        .gap_4()
        .child(
            div()
                .w(px(72.0))
                .flex_shrink_0()
                .text_color(rgb(p.text_tertiary))
                .child(label.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_right()
                .text_color(rgb(p.text_secondary))
                .child(value.to_string()),
        )
}

fn human_size(bytes: u64) -> String {
    if bytes == 0 {
        return "—".to_string();
    }
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn main() {
    ofd_core::preload_system_fonts();
    let settings = settings::Settings::load();
    let initial: Vec<PathBuf> = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .collect();
    let initial = if initial.is_empty() {
        settings
            .general
            .restore_last_file
            .then(|| settings.last_file_path())
            .flatten()
            .into_iter()
            .collect()
    } else {
        initial
    };

    gpui_kit::application()
        .with_assets(assets::ReaderAssets)
        .run(move |cx| {
            gpui_kit::init(cx);

            let bounds = Bounds::centered(None, size(px(1280.0), px(860.0)), cx);
            let options = WindowOptions {
                focus: true,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_decorations: Some(WindowDecorations::Client),
                window_background: WindowBackgroundAppearance::Transparent,
                titlebar: Some(TitlebarOptions {
                    title: Some("VofdReader".into()),
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
                // Wayland compositors map the window to its desktop entry by
                // app_id; without it KDE shows a generic "W" taskbar icon.
                app_id: Some(APP_ID.into()),
                ..Default::default()
            };

            let ui_font = font_policy::ui_font();
            cx.open_window(options, {
                let initial = initial.clone();
                let settings = settings.clone();
                let ui_font = ui_font.clone();
                move |window, cx| {
                    window.set_window_title("VofdReader");
                    // X11 reads WM_CLASS instead, which is only set here.
                    window.set_app_id(APP_ID);
                    let dark = settings
                        .appearance
                        .theme
                        .resolve(matches!(window.appearance(), WindowAppearance::Dark));
                    apply_component_theme(dark, &ui_font, cx);
                    let viewport = window.viewport_size();
                    let window_scale = window.scale_factor();
                    let settings = settings.clone();
                    let reader = cx.new(|cx| {
                        ReaderApp::new(
                            initial.clone(),
                            viewport,
                            window_scale,
                            ui_font.clone(),
                            settings.clone(),
                            dark,
                            cx,
                        )
                    });
                    // Open after the first frame so the window appears at once.
                    window.defer(cx, {
                        let reader = reader.clone();
                        move |window, cx| {
                            reader.update(cx, |this, cx| {
                                let paths = std::mem::take(&mut this.pending_initial);
                                for path in paths {
                                    this.open_path(path, Some(&mut *window), cx);
                                }
                            });
                        }
                    });
                    // Override WindowState's opaque Root background, not the
                    // component theme (dialogs and popovers must stay opaque).
                    cx.new(|cx| {
                        gpui_kit::base::Root::new(reader, window, cx).bg(transparent_black())
                    })
                }
            })
            .expect("无法创建 VofdReader 窗口");

            cx.activate(true);
        });
}

#[cfg(test)]
mod window_tests;

#[cfg(test)]
mod bar_tests;

#[cfg(test)]
mod render_tests;

#[cfg(test)]
mod search_tests;
