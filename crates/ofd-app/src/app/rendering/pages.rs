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
        let wanted = self.base_dpi(self.page, 1);
        self.budget_page_dpi(self.page, wanted, 96)
            .unwrap_or(wanted)
    }

    /// Resolve density against capacity, never against temporarily occupied
    /// reservations. Both admission and cache keys use this same density.
    pub(crate) fn budget_page_dpi(&self, page: usize, wanted: u32, floor: u32) -> Option<u32> {
        let size = self
            .active
            .page_sizes
            .get(page)
            .copied()
            .unwrap_or(SizeMm::new(210.0, 297.0));
        self.raster
            .affordable_page_dpi(size, wanted, floor, self.rotation)
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

    /// Applies new cache limits to the live reader. Bitmap accounting takes
    /// effect immediately; caches that are now over the limit are trimmed so a
    /// lowered setting converges without waiting for the next insertion.
    pub(crate) fn apply_cache_config(&mut self, config: cache_config::CacheConfig) {
        if config.thumbnail_bytes() != self.cache_config.thumbnail_bytes() {
            self.active.thumbnail_batch = self.active.thumbnail_batch.wrapping_add(1);
            self.active.thumbs_started = false;
            self.active.thumbs.fill(None);
        }
        self.cache_config = config;
        self.raster.page_prefetch.clear();
        if let Some(renderer) = &self.active.renderer {
            renderer.set_cache_limits(
                config.parsed_page_limit(),
                config.parsed_page_bytes(),
                config.decoded_image_bytes(),
            );
        }
        if self
            .active
            .error
            .as_ref()
            .is_some_and(|error| error.starts_with("页面位图"))
        {
            self.active.error = None;
        }
        self.raster
            .set_budget_limits(app::memory_budget::BudgetLimits::split(
                config.total_bytes(),
            ));
        if self.raster.budget.over_resident_limit() {
            // Cached sharper frames must not defeat a required budget downgrade.
            // The single-page screen remains owned until its replacement arrives.
            let obsolete: Vec<_> = self
                .raster
                .page_cache
                .keys()
                .copied()
                .filter(|&(page, dpi)| {
                    let wanted = if self.view_mode == ViewMode::Single {
                        self.base_dpi(page, 1)
                    } else {
                        self.continuous_dpi(self.viewport, page)
                    };
                    dpi > self
                        .budget_page_dpi(
                            page,
                            wanted,
                            if self.view_mode == ViewMode::Single {
                                96
                            } else {
                                48
                            },
                        )
                        .unwrap_or(0)
                })
                .collect();
            for key in obsolete {
                self.raster.remove_page(key);
            }
            if self.raster.image_dpi > self.target_dpi() {
                self.raster.image_dpi = 0;
            }
        }
        let obsolete_tiles: Vec<_> = self
            .raster
            .tile_cache
            .keys()
            .copied()
            .filter(|key| self.tile_density(key.page) != Some(key.dpi))
            .collect();
        for key in obsolete_tiles {
            self.raster.remove_tile(key);
        }
        self.shrink_caches();
    }

    /// Drops off-screen cached bitmaps until retained pixels are back within
    /// the ledger's resident limit. The visible page and the pages currently
    /// built in the continuous view are never dropped.
    pub(crate) fn shrink_caches(&mut self) -> usize {
        let on_screen = if self.view_mode == ViewMode::Continuous {
            self.continuous_content_range(self.viewport)
        } else {
            self.page..self.page.saturating_add(1)
        };
        let dropped = self.raster.shrink_to_budget(on_screen, self.page);
        // Atlas entries are released by the next frame's `sync_gpu_resources`,
        // which has the window needed to drop them correctly.
        dropped
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
            self.raster.set_page_image(Some(image));
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
            self.raster.set_page_image(Some(image));
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

        // Reserve the pixels before rasterizing. If the requested density does
        // not fit, step down before giving up: a softer frame is still a usable
        // page, while failing here would leave the screen blank.
        let size = self
            .active
            .page_sizes
            .get(page)
            .copied()
            .unwrap_or(SizeMm::new(210.0, 297.0));
        let affordable = self.budget_page_dpi(page, dpi, 96);
        let Some(dpi) = affordable else {
            self.active.loading = false;
            self.active.error = Some("页面位图超过缓存预算".to_string());
            cx.notify();
            return;
        };
        let estimate = render_policy::page_raster_estimate(size, dpi, self.rotation);
        let Ok(reservation) = self.raster.reserve(estimate) else {
            // The affordable check and the reservation can disagree only when
            // another job reserved in between; report instead of looping.
            // Temporary contention is not a page error. Every completion
            // wakes the foreground scheduler, including stale completions.
            return;
        };

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
            .start(TaskTarget::page(page, dpi), estimate, epoch);
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
                    let (image, bytes) = super::worker::page_image_from_raster(
                        raster.width,
                        raster.height,
                        raster.bgra,
                        rotation,
                    )
                    .map_err(str::to_string)?;
                    Ok::<_, String>((image, bytes, font_report))
                })
                .await;

            match result {
                Ok((image, bytes, font_report)) => {
                    let _ = this.update(cx, |this, cx| {
                        this.raster.tasks.finish(ticket);
                        let acceptance =
                            render_policy::accept_page_frame(render_policy::PageAcceptanceInput {
                                job: epoch,
                                current: this.render_epoch(),
                                latest_request: request.is_latest(this.raster.latest_render),
                                visible_page: this.page,
                                result_page: page,
                                image_missing: this.raster.page_image.is_none(),
                                result_dpi: dpi,
                                displayed_dpi: this.raster.image_dpi,
                                in_horizon: true,
                            });
                        if acceptance.replaces_screen() && this.view_mode == ViewMode::Single {
                            if let Some(old) = this.raster.pending_page.take() {
                                this.raster.cancel_reservation(old.reservation);
                            }
                            this.raster.pending_page = Some(app::raster_state::PendingPage {
                                request,
                                epoch,
                                image,
                                bytes,
                                reservation,
                            });
                            this.active.font_reports.insert(page, font_report);
                            if !this.active.thumbs_started {
                                this.active.thumbs_started = true;
                                this.spawn_thumbnails(cx);
                            }
                            cx.notify();
                            return;
                        }
                        this.raster.cancel_reservation(reservation);
                        let retained = acceptance.retention == render_policy::Retention::Keep
                            && this.cache_page_image(page, dpi, image.clone(), bytes);
                        if retained {
                            this.active.font_reports.insert(page, font_report);
                        }
                        // A superseded request still warms the cache but must
                        // not clear loading, start thumbnails or repaint; the
                        // request that replaced it owns those.
                        if !request.is_latest(this.raster.latest_render) {
                            this.wake_raster_scheduler(cx);
                            return;
                        }
                        this.raster.latest_render = None;
                        this.active.loading = false;
                        this.wake_raster_scheduler(cx);
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
                        this.raster.tasks.finish(ticket);
                        this.raster.cancel_reservation(reservation);
                        let acceptance =
                            render_policy::accept_page_frame(render_policy::PageAcceptanceInput {
                                job: epoch,
                                current: this.render_epoch(),
                                latest_request: request.is_latest(this.raster.latest_render),
                                visible_page: this.page,
                                result_page: page,
                                image_missing: this.raster.page_image.is_none(),
                                result_dpi: dpi,
                                displayed_dpi: this.raster.image_dpi,
                                in_horizon: true,
                            });
                        if !acceptance.is_current() || !request.is_latest(this.raster.latest_render)
                        {
                            this.wake_raster_scheduler(cx);
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
    ) -> bool {
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
        if !self
            .raster
            .cache_page(page, dpi, image.clone(), bytes, eviction)
        {
            return false;
        }
        self.gpu.borrow_mut().register(image.clone());
        self.raster.mark_registered(&image);
        true
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
                let limits = this.update(cx, |this, _| {
                    let size = this.active.page_sizes.get(page).copied()?;
                    let bytes = this.cache_config.thumbnail_bytes() / count.max(1);
                    let dpi = render_policy::thumbnail_dpi(
                        size,
                        count,
                        this.cache_config.thumbnail_bytes(),
                    )?;
                    Some((dpi, bytes))
                });
                let Ok(Some((dpi, byte_limit))) = limits else {
                    continue;
                };
                // Straight to a texture: no PNG encode, temp file or decode,
                // and pages built only for a thumbnail are not cached.
                let result = cx
                    .background_spawn(async move {
                        let raster = renderer.rasterize_preview(&doc, page, dpi as f32).ok()?;
                        // Thumbnails are never rotated: they follow the page's
                        // own orientation in the sidebar.
                        super::worker::image_from_raster(
                            raster.width,
                            raster.height,
                            raster.bgra,
                            0,
                            None,
                            "缩略图缓冲区尺寸无效",
                        )
                        .ok()
                        .filter(|(_, bytes)| *bytes <= byte_limit)
                        .map(|(image, _)| image)
                    })
                    .await;
                let Some(image) = result else {
                    continue;
                };
                let alive = this.update(cx, |this, cx| {
                    let acceptance =
                        render_policy::accept_thumbnail(render_policy::ThumbnailAcceptanceInput {
                            batch_current: this.thumbnail_batch_is_current(generation, batch),
                            page_in_range: page < this.active.thumbs.len(),
                        });
                    if acceptance.retention == render_policy::Retention::Keep {
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
        let Some(dpi) = self.budget_page_dpi(page, dpi, 48) else {
            return;
        };
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
            already_in_flight: self
                .raster
                .tasks
                .contains_current(target, self.render_epoch()),
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
        // Continuous and prefetch pages step down when the requested density
        // does not fit; neighbour pages can be softer without hurting the
        // reading page. A page that cannot fit at the floor is skipped.
        let size = self
            .active
            .page_sizes
            .get(page)
            .copied()
            .unwrap_or(SizeMm::new(210.0, 297.0));
        let estimate = render_policy::page_raster_estimate(size, dpi, self.rotation);
        let protected = if self.view_mode == ViewMode::Continuous {
            self.continuous_content_range(self.viewport)
        } else {
            self.page..self.page.saturating_add(1)
        };
        if !self.raster.page_working_set_fits(
            page,
            render_policy::page_raster_estimate(size, dpi, 0),
            protected,
        ) {
            return;
        }
        let Ok(reservation) = self.raster.reserve(estimate) else {
            return;
        };
        let epoch = self.render_epoch();
        let ticket = self.raster.tasks.start(target, estimate, epoch);
        let rotation = self.rotation;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let raster = renderer
                        .rasterize(&doc, page, dpi as f32)
                        .map_err(|error| error.to_string())?;
                    let (image, bytes) = super::worker::page_image_from_raster(
                        raster.width,
                        raster.height,
                        raster.bgra,
                        rotation,
                    )
                    .map_err(str::to_string)?;
                    Ok::<_, String>((image, bytes))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.raster.tasks.finish(ticket);
                this.raster.cancel_reservation(reservation);
                let acceptance =
                    render_policy::accept_page_frame(render_policy::PageAcceptanceInput {
                        job: epoch,
                        current: this.render_epoch(),
                        // Continuous jobs never own the single-page slot.
                        latest_request: false,
                        visible_page: this.page,
                        result_page: page,
                        image_missing: false,
                        result_dpi: dpi,
                        displayed_dpi: 0,
                        in_horizon: this.nearby_page_range().contains(&page)
                            && (this.raster.page_prefetch.contains(&page)
                                || if this.view_mode == ViewMode::Continuous {
                                    this.continuous_content_range(this.viewport).contains(&page)
                                } else {
                                    this.page == page
                                }),
                    });
                if !acceptance.is_current() {
                    this.wake_raster_scheduler(cx);
                    return;
                }
                if result.is_err() {
                    this.raster.continuous_failed.insert((page, dpi));
                }
                if let Ok((image, bytes)) = result
                    && acceptance.retention == render_policy::Retention::Keep
                {
                    this.cache_page_image(page, dpi, image, bytes);
                    if !this.active.thumbs_started {
                        this.active.thumbs_started = true;
                        this.spawn_thumbnails(cx);
                    }
                }
                this.finish_background(cx);
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

    /// All completions go through a window frame before more admission, so
    /// delayed registry owners can retire and stale jobs can unblock the view.
    pub(crate) fn finish_background(&mut self, cx: &mut Context<Self>) {
        self.gpu.borrow_mut().background_done(true);
        self.wake_raster_scheduler(cx);
    }

    /// Page and tile raster jobs in flight; together they share the
    /// configured render thread slots.
    pub(crate) fn raster_jobs(&self) -> usize {
        self.raster.tasks.len()
    }

    /// A freed physical slot belongs to the current view, not the job's epoch.
    pub(crate) fn wake_raster_scheduler(&mut self, cx: &mut Context<Self>) {
        // Defer actual admission to the next frame, after registry retirement.
        // In particular, loading=true must not suppress a retry after stale work.
        if self.raster.latest_render.is_none() && self.active.renderer.is_some() {
            self.active.loading = false;
        }
        cx.notify();
    }

    /// Diagnostic line for the accounted bitmap pools. Printed only when the
    /// diagnostics flag is on, and only from callers that already run per
    /// frame; it does not start any work or timer.
    pub(crate) fn budget_diagnostic(&self) -> String {
        let usage = self.raster.budget.usage();
        let limits = self.cache_config;
        format!(
            "[budget] resident={:.1}MiB reserved_in_flight={:.1}MiB page_cache={:.1}MiB tiles={:.1}MiB jobs={} limit_resident={:.1}MiB limit_in_flight={:.1}MiB",
            usage.resident_bytes as f64 / 1048576.0,
            usage.reserved_bytes as f64 / 1048576.0,
            self.raster.page_cache_bytes as f64 / 1048576.0,
            self.raster.tile_bytes as f64 / 1048576.0,
            self.raster.tasks.len(),
            limits.total_bytes() as f64 / 1048576.0,
            (limits.total_bytes() / 4) as f64 / 1048576.0,
        )
    }

    pub(crate) fn sync_gpu_resources(&mut self, window: &mut Window) {
        if let Some(pending) = self.raster.pending_page.take() {
            let current = pending.epoch.matches(self.render_epoch())
                && pending.request.is_latest(self.raster.latest_render)
                && pending.request.page == self.page
                && self.view_mode == ViewMode::Single
                // The setting may have changed while this output was running.
                // Drop it and re-plan, rather than turning a budget change into
                // a persistent admission error.
                && self.raster.can_ever_reserve(render_policy::page_raster_estimate(
                    self.active.page_sizes.get(self.page).copied().unwrap_or(SizeMm::new(210.0,297.0)),
                    pending.request.dpi, self.rotation,
                )).is_ok();
            if current {
                // The completed replacement is still charged to its reservation.
                // Retire the old displayed allocation inside this same frame,
                // before admitting or drawing the new one.
                self.raster.prepare_page_replacement(self.page);
                self.reconcile_gpu_images(window);
                let accepted = self.cache_page_image(
                    self.page,
                    pending.request.dpi,
                    pending.image.clone(),
                    pending.bytes,
                );
                if !accepted {
                    // Cache eviction may just have retired off-screen registry
                    // owners. Reconcile once, never spin while waiting for them.
                    self.reconcile_gpu_images(window);
                }
                let accepted = accepted
                    || self.cache_page_image(
                        self.page,
                        pending.request.dpi,
                        pending.image.clone(),
                        pending.bytes,
                    );
                if accepted {
                    self.raster.set_page_image(Some(pending.image));
                    self.raster.image_dpi = pending.request.dpi;
                    self.active.error = None;
                } else {
                    self.raster.image_dpi = 0;
                    self.active.error = Some("页面位图无法放入当前缓存预算".to_string());
                }
                self.raster.latest_render = None;
                self.active.loading = false;
            }
            if !current && pending.request.is_latest(self.raster.latest_render) {
                self.raster.latest_render = None;
                self.active.loading = false;
            }
            self.raster.cancel_reservation(pending.reservation);
        }
        if self.gpu.borrow().options.diagnostics {
            eprintln!("{}", self.budget_diagnostic());
        }
        self.reconcile_gpu_images(window);
        self.gpu
            .borrow_mut()
            .frame(self.raster.page_cache_bytes + self.raster.tile_bytes);
    }

    fn reconcile_gpu_images(&mut self, window: &mut Window) {
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
        let gpu = self.gpu.borrow();
        self.raster
            .reconcile_registered(|id| gpu.contains_image(id.0));
    }
}
