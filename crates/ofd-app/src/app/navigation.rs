use crate::*;

impl ReaderApp {
    pub(crate) fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        self.zoom = zoom.clamp(0.5, 4.0);
        if self.view_mode == ViewMode::Continuous {
            self.scroll_to_continuous_page(self.viewport, self.page);
        }
        // A render pass decides whether the bitmap needs more density; zooming
        // out keeps the sharper image and costs nothing.
        cx.notify();
    }

    pub(crate) fn on_toggle_view_mode(
        &mut self,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

    pub(crate) fn go_to_page(&mut self, page: usize, cx: &mut Context<Self>) {
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

    pub(crate) fn next_page(&mut self, cx: &mut Context<Self>) {
        self.go_to_page(self.page + 1, cx);
    }

    pub(crate) fn prev_page(&mut self, cx: &mut Context<Self>) {
        self.go_to_page(self.page.saturating_sub(1), cx);
    }

    pub(crate) fn zoom_percent(&self, viewport: Size<Pixels>) -> f32 {
        match self.view_mode {
            ViewMode::Single => self.fit_scale(viewport) * self.zoom * 100.0,
            ViewMode::Continuous => self.continuous_page_scale(viewport, self.page) * 100.0,
        }
    }

    pub(crate) fn fit_scale(&self, viewport: Size<Pixels>) -> f32 {
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

    pub(crate) fn display_size(&self, viewport: Size<Pixels>) -> (Pixels, Pixels) {
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

    pub(crate) fn on_zoom_out(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(self.zoom - 0.25, cx);
    }

    pub(crate) fn on_zoom_in(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(self.zoom + 0.25, cx);
    }

    /// Zoom value that shows one whole page. Single-page mode's fit scale is
    /// already page-based; continuous mode is width-based at 100%, so it must
    /// zoom out until the page height fits the viewport.
    pub(crate) fn fit_page_zoom(&self, viewport: Size<Pixels>) -> f32 {
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

    pub(crate) fn on_fit(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let zoom = self.fit_page_zoom(window.viewport_size());
        self.set_zoom(zoom, cx);
    }

    pub(crate) fn on_fit_width(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

    pub(crate) fn on_rotate(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.rotate_view(cx);
    }

    pub(crate) fn rotate_view(&mut self, cx: &mut Context<Self>) {
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
    pub(crate) fn apply_image_sampling(&mut self, cx: &mut Context<Self>) {
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
    pub(crate) fn invalidate_rasters(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn on_toggle_grid(
        &mut self,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_grid(cx);
    }

    pub(crate) fn toggle_grid(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn on_toggle_sidebar(
        &mut self,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_open = !self.sidebar_open;
        if self.view_mode == ViewMode::Continuous {
            self.scroll_to_continuous_page(self.viewport, self.page);
        }
        cx.notify();
    }
}
