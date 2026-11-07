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

use app::reading::{ReadingSnapshot, ViewMode};
use app::search_state::SearchSnapshot;
pub(crate) use app::theme::{Palette, apply_component_theme};
pub(crate) use ui::*;

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

#[derive(Clone)]
struct DocumentTab {
    path: PathBuf,
    file_name: String,
    reading: ReadingSnapshot,
    search: SearchSnapshot,
}

struct ReaderApp {
    /// Document identity, metadata and per-document raster products.
    active: app::active_document::ActiveDocument,
    pending_initial: Vec<PathBuf>,
    tabs: Vec<DocumentTab>,
    active_tab: Option<usize>,
    pending_restore_scroll: Option<f32>,
    sidebar_open: bool,
    page: usize,
    zoom: f32,
    page_image: Option<Arc<RenderImage>>,
    view_mode: ViewMode,
    continuous_scroll: ScrollHandle,
    continuous_pending: HashSet<(usize, u32)>,
    continuous_failed: HashSet<(usize, u32)>,
    continuous_layout: RefCell<continuous::LayoutCache>,
    continuous_last_scroll: Option<Instant>,
    /// Last continuous scroll offset and direction; prefetch goes ahead.
    last_scroll_y: f32,
    scroll_down: bool,
    continuous_settle_pending: bool,
    page_cache: HashMap<(usize, u32), Arc<RenderImage>>,
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
    latest_render: Option<app::render_policy::PageRequest>,
    dpi_debounce_gen: u64,
    search: app::search_state::SearchState,
    page_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    selection: app::selection_state::SelectionState,
    recent: Vec<PathBuf>,
    notice: Option<(String, bool)>,
    notice_generation: u64,
    exporting: bool,
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
                        this.active.is_open()
                            && !this.active.loading
                            && this.latest_render.is_none()
                            && this.continuous_pending.is_empty()
                            && !this.tile_busy()
                            && this.gpu.borrow().stats.page_draws > 0
                            && this
                                .active
                                .thumbs
                                .iter()
                                .take(THUMB_LIMIT)
                                .all(Option::is_some)
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
            active: app::active_document::ActiveDocument::empty(),
            pending_initial: initial,
            tabs: Vec::new(),
            active_tab: None,
            pending_restore_scroll: None,
            sidebar_open: settings.appearance.sidebar_open,
            page: 0,
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
            continuous_last_scroll: None,
            last_scroll_y: 0.0,
            scroll_down: true,
            continuous_settle_pending: false,
            page_cache: HashMap::new(),
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
            search: app::search_state::SearchState::new(),
            page_bounds: Rc::new(Cell::new(None)),
            selection: app::selection_state::SelectionState::new(),
            recent: recent::load_recent(settings.files.recent_limit),
            notice: None,
            notice_generation: 0,
            exporting: false,
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
