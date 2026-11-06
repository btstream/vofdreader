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

mod app;
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
mod single_instance;
mod tiles;
mod ui;

pub(crate) use app::theme::apply_component_theme;
pub(crate) use ui::*;

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

    #[allow(clippy::too_many_arguments)]
    fn new(
        initial: Vec<PathBuf>,
        instance_rx: Option<std::sync::mpsc::Receiver<Vec<PathBuf>>>,
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

        if let Some(receiver) = instance_rx {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(150))
                        .await;
                    let mut paths = Vec::new();
                    let mut received = false;
                    loop {
                        match receiver.try_recv() {
                            Ok(batch) => {
                                received = true;
                                paths.extend(batch);
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => break,
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                        }
                    }
                    if !received {
                        continue;
                    }
                    let opened = this.update(cx, |this, cx| {
                        for path in paths {
                            this.open_path(path, None, cx);
                        }
                        cx.activate(true);
                        if let Some(window) = cx.windows().first().copied() {
                            let _ = window.update(cx, |_, window, _| {
                                window.activate_window();
                            });
                        }
                        cx.notify();
                    });
                    if opened.is_err() {
                        return;
                    }
                }
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
}

fn main() {
    let launch_paths: Vec<PathBuf> = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .collect();
    if single_instance::forward_to_running(&launch_paths) {
        return;
    }
    let instance_rx = single_instance::listen();

    ofd_core::preload_system_fonts();
    let settings = settings::Settings::load();
    let initial = if launch_paths.is_empty() {
        settings
            .general
            .restore_last_file
            .then(|| settings.last_file_path())
            .flatten()
            .into_iter()
            .collect()
    } else {
        launch_paths
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
                let instance_rx = instance_rx;
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
                            instance_rx,
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
