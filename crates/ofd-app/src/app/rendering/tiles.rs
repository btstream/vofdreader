use crate::app::render_policy;
use crate::app::task_state::{TaskTarget, TaskTicket};
use crate::*;

impl ReaderApp {
    /// Tiles carry the zoomed-in detail above the whole-page bitmap: they
    /// exist only when the wanted density exceeds the page's base density.
    pub(crate) fn tile_density(&self, page: usize) -> Option<u32> {
        if page >= self.active.page_sizes.len() {
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

    pub(crate) fn nearby_page_range(&self) -> std::ops::Range<usize> {
        let actual = if self.view_mode == ViewMode::Continuous {
            self.continuous_content_range(self.viewport)
        } else if self.active.page_count() > 0 {
            self.page..self.page + 1
        } else {
            0..0
        };
        continuous::prefetch_range(
            actual,
            self.active.page_count(),
            self.cache_config.prefetch_pages(),
        )
    }

    pub(crate) fn schedule_nearby(&mut self, cx: &mut Context<Self>) {
        // Continuous scrolling keeps prefetching so pages are ready before
        // they appear; single-page wheel bursts still coalesce.
        if !self.active.is_open()
            || self.active.renderer.is_none()
            || self.grid_open
            || (self.view_mode == ViewMode::Single && self.continuous_scrolling())
            || self.raster.dpi_pending
            || self.raster.latest_render.is_some()
            || self.active.loading
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
        pages = render_policy::order_prefetch_pages(pages, self.page, self.scroll_down);
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
                    page_raster_bytes(self.active.page_sizes[page], dpi_of(self, page))
                }
            })
            .sum();
        let candidates: Vec<_> = pages
            .iter()
            .map(|&page| {
                let dpi = dpi_of(self, page);
                // A sharper cached bitmap is what stays resident.
                let sharpest = self
                    .raster
                    .page_cache
                    .keys()
                    .filter(|(cached, cached_dpi)| *cached == page && *cached_dpi >= dpi)
                    .map(|(_, cached_dpi)| *cached_dpi)
                    .min();
                let bytes = match sharpest {
                    Some(found) => self.cached_page_bytes(|key| key == (page, found)),
                    None => page_raster_bytes(self.active.page_sizes[page], dpi),
                };
                render_policy::PrefetchCandidate { page, dpi, bytes }
            })
            .collect();
        let plan = render_policy::prefetch_plan(
            reserved,
            self.cache_config.page_limit(),
            actual.len(),
            self.cache_config.page_bytes(),
            &candidates,
        );
        for candidate in plan {
            self.request_cached_page(candidate.page, candidate.dpi, cx);
            if !continuous::render_ready(self.raster_jobs(), self.tile_slots) {
                return;
            }
        }
        self.schedule_tile(true, cx);
    }

    pub(crate) fn tile_requests(&self, include_prefetch: bool) -> Vec<tiles::Key> {
        let visible: Vec<_> = self
            .raster
            .tile_wanted
            .values()
            .flatten()
            .copied()
            .collect();
        let extra = if include_prefetch {
            self.raster
                .tile_prefetch
                .values()
                .flat_map(|keys| keys.iter().enumerate().map(|(rank, key)| (rank, *key)))
                .collect()
        } else {
            vec![]
        };
        tiles::requests(visible, extra, self.page, self.cache_config.tile_bytes())
    }

    pub(crate) fn tile_busy(&self) -> bool {
        self.raster.tasks.targets().any(|target| !target.is_page())
    }

    /// Start tile jobs in priority order until every slot is used. Tiles of
    /// one page share the renderer's prepared page, so they run in parallel.
    pub(crate) fn schedule_tile(&mut self, include_prefetch: bool, cx: &mut Context<Self>) {
        while continuous::render_ready(self.raster_jobs(), self.tile_slots) {
            if !self.start_tile(include_prefetch, cx) {
                break;
            }
        }
    }

    /// Start the most important missing tile; false when nothing started.
    pub(crate) fn start_tile(&mut self, include_prefetch: bool, cx: &mut Context<Self>) -> bool {
        // Continuous scrolling keeps rendering visible tiles; page and tile
        // jobs share the render slots (see `raster_jobs`).
        if (self.view_mode == ViewMode::Single && self.continuous_scrolling())
            || self.raster.dpi_pending
            || self.raster.latest_render.is_some()
        {
            return false;
        }
        let wanted = self.tile_requests(include_prefetch);
        let Some(key) = wanted.iter().copied().find(|key| {
            !self.raster.tile_cache.contains_key(key)
                && !self.raster.tile_failed.contains(key)
                && !self.raster.tasks.contains(TaskTarget::tile(*key))
        }) else {
            return false;
        };
        let (Some(doc), Some(renderer)) = (self.active.doc.clone(), self.active.renderer.clone())
        else {
            return false;
        };
        let size = self.active.page_sizes[key.page];
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
            .filter_map(|key| self.raster.tile_cache.get(key))
            .map(|image| {
                let size = image.size(0);
                size.width.0 as usize * size.height.0 as usize * 4
            })
            .sum();
        let in_flight = self.raster.tasks.tile_bytes();
        if protected_bytes + in_flight + bytes > self.cache_config.tile_bytes() {
            return false;
        }
        let epoch = self.render_epoch();
        let ticket: TaskTicket = self.raster.tasks.start(TaskTarget::tile(key), bytes, epoch);
        let rotation = self.rotation;
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
                let info = this.raster.tasks.finish(ticket);
                if info.is_some()
                    && epoch.matches(this.render_epoch())
                    && this.tile_requests(true).contains(&key)
                {
                    match result {
                        Ok((image, bytes)) => {
                            let protected = this.tile_requests(include_prefetch);
                            let accepted = this.raster.cache_tile(
                                key,
                                image.clone(),
                                bytes,
                                this.cache_config.tile_bytes(),
                                &protected,
                            );
                            if accepted {
                                this.gpu.borrow_mut().register(image);
                            }
                        }
                        Err(error) => {
                            this.raster.tile_failed.insert(key);
                            eprintln!("[tiles]: {key:?}: {error}");
                        }
                    }
                }
                // Next frame re-evaluates actual clipping before requesting work.
                if this.view_mode == ViewMode::Single
                    && this.raster.page_image.is_none()
                    && this.raster.latest_render.is_none()
                {
                    this.request_page_render(cx);
                }
                // Prefetched tiles stay in the cache until they scroll in.
                let visible = this
                    .raster
                    .tile_wanted
                    .values()
                    .any(|keys| keys.contains(&key));
                this.finish_background(visible, cx);
            });
        })
        .detach();
        true
    }
}
