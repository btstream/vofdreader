use crate::app::render_policy;
use crate::app::task_state::{RenderEpoch, TaskTarget};
use crate::*;

impl ReaderApp {
    /// Generations a completed raster job must still match.
    pub(crate) fn render_epoch(&self) -> RenderEpoch {
        RenderEpoch {
            document: self.active.generation,
            view: self.raster.view_generation,
        }
    }

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
            .active
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
        // newest generation may start a render. A running timer is reused, so
        // at most one timer exists at a time and it always clears the flag.
        self.raster.dpi_debounce_gen = self.raster.dpi_debounce_gen.wrapping_add(1);
        if self.raster.dpi_pending {
            return;
        }
        self.raster.dpi_pending = true;
        let generation = self.raster.dpi_debounce_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.raster.dpi_pending = false;
                if this.raster.dpi_debounce_gen != generation {
                    return;
                }
                match this.view_mode {
                    ViewMode::Single if this.target_dpi() > this.raster.image_dpi => {
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
            self.active.loading = false;
            self.ensure_continuous_pages(self.viewport, cx);
            return;
        }
        let Some(doc) = self.active.doc.clone() else {
            return;
        };
        let Some(renderer) = self.active.renderer.clone() else {
            return;
        };

        let page = self.page;
        let dpi = self.target_dpi();

        // Prefer an exact bitmap, otherwise reuse any sharper one. A larger
        // bitmap shown smaller stays crisp and avoids a redundant render.
        let has_cached_at_least = self
            .raster
            .page_cache
            .keys()
            .any(|(cached_page, cached_dpi)| *cached_page == page && *cached_dpi >= dpi);
        if has_cached_at_least && let Some((cached_dpi, image)) = self.cached_page_image(page, dpi)
        {
            self.raster.page_image = Some(image);
            self.raster.image_dpi = cached_dpi;
            self.active.loading = false;
            self.active.error = None;
            self.raster.latest_render = None;
            return;
        }

        // Single-page wheel bursts also coalesce: show cached frames now,
        // rasterize only the final uncached page after the gesture settles.
        // Neighbour prefetch is deliberately low-DPI. Display it immediately
        // on navigation, then refine instead of ignoring a useful cached frame.
        let best_cached = self
            .raster
            .page_cache
            .keys()
            .filter(|(cached_page, _)| *cached_page == page)
            .map(|(_, dpi)| *dpi)
            .max();
        if let render_policy::PreviewSource::Cached(preview_dpi) =
            render_policy::preview_density(render_policy::PreviewInput {
                best_cached_dpi: best_cached,
                image_missing: self.raster.page_image.is_none(),
            })
            && let Some((cached_dpi, image)) = self.cached_page_image(page, preview_dpi)
        {
            self.raster.page_image = Some(image);
            self.raster.image_dpi = cached_dpi;
            self.active.loading = false;
        }

        let plan = render_policy::plan_single_page(render_policy::SinglePageInput {
            target_dpi: dpi,
            has_cached_at_least,
            gesture_active: self.continuous_scrolling() || self.tile_busy(),
            other_jobs_active: !self.raster.tasks.is_empty(),
            pending_request: self
                .raster
                .latest_render
                .map(|pending| (pending.page, pending.dpi)),
            page,
        });
        match plan {
            // Handled above; listed so a new variant cannot silently start a
            // render without a matching branch.
            render_policy::SinglePagePlan::UseCached => return,
            render_policy::SinglePagePlan::Wait | render_policy::SinglePagePlan::Coalesced => {
                return;
            }
            render_policy::SinglePagePlan::Render { .. } => {}
        }

        self.raster.render_seq = self.raster.render_seq.wrapping_add(1);
        let request = render_policy::PageRequest {
            seq: self.raster.render_seq,
            page,
            dpi,
        };
        self.raster.latest_render = Some(request);
        self.active.loading = self.raster.page_image.is_none();
        let epoch = self.render_epoch();
        let ticket = self
            .raster
            .tasks
            .start(TaskTarget::page(page, dpi), 0, epoch);
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
                        if !epoch.matches(this.render_epoch()) {
                            this.raster.tasks.finish(ticket);
                            return;
                        }
                        this.raster.tasks.finish(ticket);
                        this.cache_page_image(page, dpi, image.clone(), bytes);
                        this.active.font_reports.insert(page, font_report);
                        if !request.is_latest(this.raster.latest_render) {
                            return;
                        }
                        this.raster.latest_render = None;
                        this.active.loading = false;
                        if request.replaces_visible(
                            this.page,
                            this.raster.image_dpi,
                            this.raster.page_image.is_some(),
                        ) {
                            this.raster.page_image = Some(image);
                            this.raster.image_dpi = dpi;
                            this.active.error = None;
                        }
                        // Start thumbnails only after the main page is on
                        // screen, so they do not compete for CPU with the
                        // first paint.
                        if !this.active.thumbs_started {
                            this.active.thumbs_started = true;
                            this.spawn_thumbnails(cx);
                        }
                        cx.notify();
                    });
                }
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        let usable = epoch.matches(this.render_epoch());
                        this.raster.tasks.finish(ticket);
                        if !usable {
                            return;
                        }
                        let is_latest = request.is_latest(this.raster.latest_render);
                        if !is_latest {
                            return;
                        }
                        this.raster.latest_render = None;
                        this.active.loading = false;
                        this.active.error = Some(format!("页面渲染失败：{error}"));
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
        self.raster.cached_page(page, target_dpi)
    }

    /// Bytes of the cached page bitmaps whose key matches.
    pub(crate) fn cached_page_bytes(&self, matches: impl Fn((usize, u32)) -> bool) -> usize {
        self.raster.page_bytes_matching(matches)
    }

    pub(crate) fn touch_cached_page(&mut self, page: usize, dpi: u32) {
        self.raster.touch_page(page, dpi);
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
        // Pages on screen are never evicted, even when they alone exceed the
        // budget; otherwise two large visible pages evict each other forever.
        let (on_screen, count_limit) = if self.view_mode == ViewMode::Continuous {
            (
                self.continuous_content_range(self.viewport),
                self.cache_config
                    .page_limit()
                    .max(self.continuous_visible_range(self.viewport).len()),
            )
        } else {
            (0..0, self.cache_config.page_limit())
        };
        let eviction = app::raster_state::PageEviction {
            count_limit,
            byte_limit: self.cache_config.page_bytes(),
            protected: (self.page, self.raster.image_dpi),
            on_screen,
        };
        self.raster.cache_page(page, dpi, image, bytes, eviction);
    }

    /// Renders thumbnails one at a time. Rendering them concurrently used to
    /// saturate every background worker and made the main page wait behind
    /// dozens of thumbnail jobs on multi-page documents.
    pub(crate) fn spawn_thumbnails(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.active.doc.clone() else {
            return;
        };
        let Some(renderer) = self.active.renderer.clone() else {
            return;
        };
        let generation = self.active.generation;
        // A newer batch (after clearing the cache) must win over an older one
        // that is still running: both write the same slots in different
        // orders.
        let batch = self.active.thumbnail_batch;
        let count = self.active.page_count().min(THUMB_LIMIT);
        let mut order: Vec<usize> = (0..count).collect();
        order.sort_by_key(|page| page.abs_diff(self.page));

        cx.spawn(async move |this, cx| {
            for page in order {
                // Thumbnails must not compete with wheel gestures or a
                // foreground page. At most one thumbnail is already running.
                loop {
                    let ready = this.update(cx, |this, _| {
                        if !this.thumbnail_batch_is_current(generation, batch) {
                            return None;
                        }
                        Some(render_policy::may_render_thumbnail(
                            true,
                            this.continuous_scrolling(),
                            this.raster.tasks.targets().any(|target| target.is_page()),
                            this.tile_busy(),
                            this.raster.latest_render.is_some(),
                        ))
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
                    if this.thumbnail_batch_is_current(generation, batch)
                        && page < this.active.thumbs.len()
                    {
                        this.gpu.borrow_mut().register(image.clone());
                        this.active.thumbs[page] = Some(image);
                        let visible = this.thumbnail_on_screen(page);
                        this.gpu.borrow_mut().background_done(visible);
                        if visible {
                            cx.notify();
                        }
                    }
                    this.thumbnail_batch_is_current(generation, batch)
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn request_cached_page(&mut self, page: usize, dpi: u32, cx: &mut Context<Self>) {
        let target = TaskTarget::page(page, dpi);
        let admission = render_policy::CachedPageInput {
            page,
            page_count: self.active.page_count(),
            raster_jobs: self.raster_jobs(),
            slots: self.tile_slots,
            single_page_job_active: self.raster.latest_render.is_some(),
            cached_sharp_enough: self
                .raster
                .page_cache
                .keys()
                .any(|(cached_page, cached_dpi)| *cached_page == page && *cached_dpi >= dpi),
            already_failed: self.raster.continuous_failed.contains(&(page, dpi)),
            already_in_flight: self.raster.tasks.contains(target),
        };
        if !render_policy::may_request_cached_page(admission) {
            return;
        }
        let Some(doc) = self.active.doc.clone() else {
            return;
        };
        let Some(renderer) = self.active.renderer.clone() else {
            return;
        };
        let epoch = self.render_epoch();
        let ticket = self.raster.tasks.start(target, 0, epoch);
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
                this.raster.tasks.finish(ticket);
                if !epoch.matches(this.render_epoch()) {
                    return;
                }
                if result.is_err() {
                    this.raster.continuous_failed.insert((page, dpi));
                }
                // Do not retain a completed frame outside the current visible
                // and nearby-prefetch horizon. There is one in-flight job.
                if let Ok((width, height, pixels)) = result
                    && render_policy::may_retain_cached_page(
                        this.nearby_page_range().contains(&page),
                        true,
                    )
                    && let Some(buffer) = image::RgbaImage::from_raw(width, height, pixels)
                {
                    let bytes = width as usize * height as usize * 4;
                    this.cache_page_image(
                        page,
                        dpi,
                        Arc::new(RenderImage::new([image::Frame::new(buffer)])),
                        bytes,
                    );
                    if !this.active.thumbs_started {
                        this.active.thumbs_started = true;
                        this.spawn_thumbnails(cx);
                    }
                }
                if this.view_mode == ViewMode::Single
                    && this.raster.page_image.is_none()
                    && this.raster.latest_render.is_none()
                {
                    this.request_page_render(cx);
                }
                let visible = this.page_on_screen(page);
                this.finish_background(visible, cx);
            });
        })
        .detach();
    }

    /// Whether a thumbnail batch still owns the slots it writes: same
    /// document and no newer batch has started.
    pub(crate) fn thumbnail_batch_is_current(&self, generation: u64, batch: u64) -> bool {
        self.active.generation == generation && self.active.thumbnail_batch == batch
    }

    /// Thumbnails show in the sidebar and page grid, and stand in for an
    /// on-screen page that has no bitmap yet.
    pub(crate) fn thumbnail_on_screen(&self, page: usize) -> bool {
        self.sidebar_open
            || self.grid_open
            || (self.page_on_screen(page)
                && !self
                    .raster
                    .page_cache
                    .keys()
                    .any(|(cached, _)| *cached == page)
                && (self.view_mode == ViewMode::Continuous || self.raster.page_image.is_none()))
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
        self.raster.tasks.len()
    }

    pub(crate) fn sync_gpu_resources(&mut self, window: &mut Window) {
        let mut live: HashSet<_> = self
            .raster
            .page_cache
            .values()
            .map(|image| image.id)
            .collect();
        live.extend(self.raster.tile_cache.values().map(|image| image.id));
        if let Some(image) = &self.raster.page_image {
            live.insert(image.id);
        }
        live.extend(self.active.thumbs.iter().flatten().map(|image| image.id));
        self.gpu
            .borrow_mut()
            .reconcile(&live, |image| window.drop_image(image).is_ok());
        self.gpu
            .borrow_mut()
            .frame(self.raster.page_cache_bytes + self.raster.tile_bytes);
    }
}
