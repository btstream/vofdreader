use crate::*;

/// Bottom-center reveal zone, relative to the actual reading canvas rather
/// than the window (sidebar/status bar must not activate the floating bar).
pub(crate) fn near_floating_bar(bounds: Bounds<Pixels>, pointer: Point<Pixels>) -> bool {
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
pub(crate) fn page_raster_bytes(size: SizeMm, dpi: u32) -> usize {
    let side = |mm: f64| (mm * f64::from(dpi) / 25.4).ceil() as usize + 1;
    side(size.width) * side(size.height) * 4
}

pub(crate) fn pointer_moved(last: Point<Pixels>, current: Point<Pixels>) -> bool {
    let dx = f32::from(current.x - last.x);
    let dy = f32::from(current.y - last.y);
    dx * dx + dy * dy >= 9.0
}

/// Maps a mouse-wheel event to a multiplicative zoom step. Positive deltas
/// (wheel up) zoom in; one notch is about ten percent and a fast flick is
/// capped so a single event cannot jump across the whole zoom range.
pub(crate) fn wheel_zoom_factor(delta: ScrollDelta) -> f32 {
    let lines = match delta {
        ScrollDelta::Lines(delta) => delta.y,
        // Precise touchpad deltas are pixels; 40 px approximates one wheel line.
        ScrollDelta::Pixels(delta) => f32::from(delta.y) / 40.0,
    };
    (1.0 + lines * 0.1).clamp(0.75, 1.25)
}

pub(crate) fn content_corners(decorations: Decorations, fullscreen: bool) -> Corners<Pixels> {
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

pub(crate) fn overlay_layer(
    highlights: Vec<search_highlight::Highlight>,
    color: Rgba,
) -> impl IntoElement {
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

pub(crate) fn dot_grid(
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

pub(crate) fn icon(name: IconName, size: f32, color: u32) -> impl IntoElement {
    div().text_size(px(size)).text_color(rgb(color)).child(name)
}

/// Short context around a match, trimmed to whole characters.
/// Highlights every occurrence of `query` inside a search snippet.
pub(crate) fn highlight_matches(text: &str, query: &str, color: u32) -> StyledText {
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

pub(crate) fn search_snippet(text: &str, start: usize, end: usize) -> String {
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

pub(crate) fn settings_cache_config(settings: &settings::Settings) -> cache_config::CacheConfig {
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

pub(crate) fn info_row(label: &str, value: &str, p: Palette) -> Div {
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

pub(crate) fn human_size(bytes: u64) -> String {
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
