use crate::*;

impl ReaderApp {
    pub(crate) fn max_page_dpi(&self) -> u32 {
        self.settings.performance.page_dpi_cap()
    }

    /// Whole-page DPI for the single-page view: the quality preset at the
    /// current zoom (see `base_dpi`). Screen resolution is not considered.
    pub(crate) fn target_dpi(&self) -> u32 {
        self.base_dpi(self.page, 1)
    }

    /// Quality preset DPI at 100% zoom (质量档位).
    pub(crate) fn preset_dpi(&self) -> u32 {
        self.max_page_dpi().max(96)
    }

    /// Density the page content should have at the current zoom.
    pub(crate) fn wanted_dpi(&self) -> u32 {
        tiles::hard_dpi(self.preset_dpi(), self.zoom)
    }

    /// Whole-page bitmap density: the wanted density, but never above the
    /// preset (zoomed-in detail comes from tiles) and within the GPU texture
    /// size and the page cache shared by `visible` pages. Visible pages and
    /// prefetched neighbours use the same value, so a page that scrolls into
    /// view is already at its final quality.
    pub(crate) fn base_dpi(&self, page: usize, visible: usize) -> u32 {
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
    pub(crate) fn schedule_dpi_render(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn request_page_render(&mut self, cx: &mut Context<Self>) {
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
    pub(crate) fn cached_page_image(
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
    pub(crate) fn cached_page_bytes(&self, matches: impl Fn((usize, u32)) -> bool) -> usize {
        self.page_cache
            .iter()
            .filter(|(key, _)| matches(**key))
            .map(|(_, image)| {
                let size = image.size(0);
                size.width.to_bytes(4) as usize * size.height.0 as usize
            })
            .sum()
    }

    pub(crate) fn touch_cached_page(&mut self, page: usize, dpi: u32) {
        let key = (page, dpi);
        self.page_cache_order.retain(|existing| *existing != key);
        self.page_cache_order.push_back(key);
    }

    /// Keeps decoded pages in memory within a byte budget. The displayed page
    /// is never evicted by the request that produced it.
    pub(crate) fn cache_page_image(
        &mut self,
        page: usize,
        dpi: u32,
        image: Arc<RenderImage>,
        bytes: usize,
    ) {
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
    pub(crate) fn spawn_thumbnails(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn request_cached_page(&mut self, page: usize, dpi: u32, cx: &mut Context<Self>) {
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
    pub(crate) fn thumbnail_on_screen(&self, page: usize) -> bool {
        self.sidebar_open
            || self.grid_open
            || (self.page_on_screen(page)
                && !self.page_cache.keys().any(|(cached, _)| *cached == page)
                && (self.view_mode == ViewMode::Continuous || self.page_image.is_none()))
    }

    /// Whether a bitmap of `page` is (or is about to be) drawn: the
    /// continuous rows actually built, or the single page being shown.
    pub(crate) fn page_on_screen(&self, page: usize) -> bool {
        match self.view_mode {
            ViewMode::Continuous => self.continuous_visible_range(self.viewport).contains(&page),
            ViewMode::Single => page == self.page,
        }
    }

    /// A background raster job finished. Every notify repaints the whole
    /// window, so only results that are on screen request one; otherwise the
    /// next job is scheduled directly instead of through a frame.
    pub(crate) fn finish_background(&mut self, visible: bool, cx: &mut Context<Self>) {
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

    /// Page and tile raster jobs in flight; together they share the
    /// configured render thread slots.
    pub(crate) fn raster_jobs(&self) -> usize {
        self.continuous_pending.len() + self.tile_jobs.len()
    }

    pub(crate) fn sync_gpu_resources(&mut self, window: &mut Window) {
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
}
