use crate::*;

impl ReaderApp {
    pub(crate) fn continuous_page_scale(&self, viewport: Size<Pixels>, index: usize) -> f32 {
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

    pub(crate) fn continuous_page_size(&self, viewport: Size<Pixels>, index: usize) -> (f32, f32) {
        self.with_continuous_layout(viewport, |layout| {
            layout.sizes.get(index).copied().unwrap_or((0.0, 0.0))
        })
    }

    pub(crate) fn with_continuous_layout<T>(
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

    pub(crate) fn continuous_top(&self, viewport: Size<Pixels>, index: usize) -> f32 {
        self.with_continuous_layout(viewport, |layout| layout.top(index))
    }

    pub(crate) fn scroll_to_continuous_page(&self, viewport: Size<Pixels>, page: usize) {
        let top = self.continuous_top(viewport, page);
        self.continuous_scroll.set_offset(point(px(0.0), px(-top)));
    }

    pub(crate) fn scroll_to_continuous_page_offset(
        &self,
        viewport: Size<Pixels>,
        page: usize,
        offset: f32,
    ) {
        let top = self.continuous_top(viewport, page);
        let height = self.continuous_page_size(viewport, page).1;
        let y = top + height * offset.clamp(0.0, 1.0);
        self.continuous_scroll.set_offset(point(px(0.0), px(-y)));
    }

    /// Puts a search match near the upper third of the continuous view.
    pub(crate) fn scroll_to_search_match(
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

    pub(crate) fn continuous_page_at(&self, viewport: Size<Pixels>, scroll: f32) -> usize {
        self.with_continuous_layout(viewport, |layout| layout.page_at(scroll))
    }

    pub(crate) fn continuous_visible_range(
        &self,
        viewport: Size<Pixels>,
    ) -> std::ops::Range<usize> {
        if self.page_count == 0 {
            return 0..0;
        }
        let offset = -f32::from(self.continuous_scroll.offset().y);
        let height = (f32::from(viewport.height) - 124.0).max(240.0);
        self.with_continuous_layout(viewport, |layout| layout.visible(offset, height))
    }

    pub(crate) fn continuous_content_range(
        &self,
        viewport: Size<Pixels>,
    ) -> std::ops::Range<usize> {
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
    pub(crate) fn continuous_page_slots(&self, viewport: Size<Pixels>, page: usize) -> usize {
        let height = (f32::from(viewport.height) - 124.0).max(240.0);
        let page_height = self.continuous_page_size(viewport, page).1.max(1.0);
        ((height / page_height).ceil() as usize + 1).clamp(1, self.page_count.max(1))
    }

    pub(crate) fn continuous_dpi(&self, viewport: Size<Pixels>, page: usize) -> u32 {
        // Same density on and off screen: prefetched neighbours must not be
        // a blurrier stand-in that is refined after scrolling stops.
        self.base_dpi(page, self.continuous_page_slots(viewport, page))
    }

    pub(crate) fn request_continuous_page(
        &mut self,
        viewport: Size<Pixels>,
        page: usize,
        cx: &mut Context<Self>,
    ) {
        let dpi = self.continuous_dpi(viewport, page);
        self.request_cached_page(page, dpi, cx);
    }

    pub(crate) fn ensure_continuous_pages(
        &mut self,
        viewport: Size<Pixels>,
        cx: &mut Context<Self>,
    ) {
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

    pub(crate) fn continuous_scrolling(&self) -> bool {
        self.continuous_last_scroll
            .is_some_and(|last| last.elapsed() < CONTINUOUS_IDLE)
    }

    pub(crate) fn note_continuous_scroll(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn scroll_continuous(&mut self, delta: f32, cx: &mut Context<Self>) {
        self.note_continuous_scroll(cx);
        let offset = self.continuous_scroll.offset();
        self.continuous_scroll
            .set_offset(point(offset.x, offset.y - px(delta)));
        self.sync_continuous_page(self.viewport, cx);
        cx.notify();
    }

    pub(crate) fn sync_continuous_page(&mut self, viewport: Size<Pixels>, cx: &mut Context<Self>) {
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

    pub(crate) fn on_continuous_scroll(
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
    pub(crate) fn on_single_scroll(
        &mut self,
        event: &ScrollEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
}
