//! Ownership of page-image atlas entries, plus opt-in renderer diagnostics.
//! Counts first successful paint submissions, not hardware DMA/GPU timings.
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use gpui_kit::{Bounds, ImageId, Pixels, RenderImage};

#[derive(Default, Clone, Copy)]
pub struct Options {
    pub diagnostics: bool,
    pub no_dots: bool,
    pub no_page_shadows: bool,
}

impl Options {
    pub fn from_env() -> Self {
        fn enabled(name: &str) -> bool {
            std::env::var(name).is_ok_and(|v| v == "1")
        }
        Self {
            diagnostics: enabled("OFD_GPU_DIAGNOSTICS"),
            no_dots: enabled("OFD_GPU_DISABLE_DOTS"),
            no_page_shadows: enabled("OFD_GPU_DISABLE_PAGE_SHADOWS"),
        }
    }
}

#[derive(Default, Debug)]
pub struct Stats {
    pub frames: u64,
    pub page_draws: u64,
    pub images_created: u64,
    pub first_uses: u64,
    pub first_use_bytes: u64,
    pub release_calls: u64,
    pub resident_bytes: usize,
    pub peak_resident_bytes: usize,
    pub dot_quads: u64,
    pub dot_layouts: u64,
    /// Finished background raster jobs (pages, tiles, thumbnails) and how
    /// many of them requested a full-window redraw.
    pub background_jobs: u64,
    pub background_redraws: u64,
}

pub struct Resources {
    images: HashMap<ImageId, Arc<RenderImage>>,
    resident: HashMap<ImageId, usize>,
    pub options: Options,
    pub stats: Stats,
    last_log: Instant,
    last_cache_bytes: usize,
    dots: DotGrid,
}

impl Resources {
    pub fn new(options: Options) -> Self {
        Self {
            images: HashMap::new(),
            resident: HashMap::new(),
            options,
            stats: Stats::default(),
            last_log: Instant::now(),
            last_cache_bytes: 0,
            dots: DotGrid::default(),
        }
    }

    pub fn register(&mut self, image: Arc<RenderImage>) {
        if self.images.insert(image.id, image).is_none() {
            self.stats.images_created += 1;
        }
    }

    pub fn painted(&mut self, image: &RenderImage) {
        self.stats.page_draws += 1;
        if let std::collections::hash_map::Entry::Vacant(entry) = self.resident.entry(image.id) {
            let size = image.size(0);
            let bytes = size.width.0 as usize * size.height.0 as usize * 4;
            entry.insert(bytes);
            self.stats.first_uses += 1;
            self.stats.first_use_bytes += bytes as u64;
            self.stats.resident_bytes += bytes;
            self.stats.peak_resident_bytes = self
                .stats
                .peak_resident_bytes
                .max(self.stats.resident_bytes);
            self.log("first-use", self.last_cache_bytes, true);
        }
    }

    /// Run at the start of a reader frame, with the current window available.
    /// The displayed single-page image remains live even if its cache entry
    /// was replaced. No Arc::strong_count heuristics or premature releases.
    pub fn reconcile(
        &mut self,
        live: &HashSet<ImageId>,
        mut release: impl FnMut(Arc<RenderImage>) -> bool,
    ) {
        let obsolete: Vec<_> = self
            .images
            .keys()
            .filter(|id| !live.contains(id))
            .copied()
            .collect();
        for id in obsolete {
            if release(self.images[&id].clone()) {
                self.images.remove(&id);
                self.released(id);
            }
        }
    }

    fn released(&mut self, id: ImageId) {
        if let Some(bytes) = self.resident.remove(&id) {
            self.stats.release_calls += 1;
            self.stats.resident_bytes = self.stats.resident_bytes.saturating_sub(bytes);
            self.log("retire", self.last_cache_bytes, true);
        }
    }

    pub fn release_all(&mut self, mut release: impl FnMut(Arc<RenderImage>)) {
        for (id, image) in std::mem::take(&mut self.images) {
            release(image);
            self.released(id);
        }
        self.log("release", 0, true);
    }

    pub fn background_done(&mut self, redraw: bool) {
        self.stats.background_jobs += 1;
        self.stats.background_redraws += u64::from(redraw);
    }

    pub fn frame(&mut self, cache_bytes: usize) {
        self.last_cache_bytes = cache_bytes;
        self.stats.frames += 1;
        self.log("frame", cache_bytes, false);
    }

    fn log(&mut self, event: &str, cache_bytes: usize, force: bool) {
        if !self.options.diagnostics || (!force && self.last_log.elapsed() < Duration::from_secs(2))
        {
            return;
        }
        self.last_log = Instant::now();
        let s = &self.stats;
        eprintln!(
            "[gpu] {event} frames={} page_draws={} images_created={} first_uses={} first_use_mib={:.2} releases={} page_resident_mib={:.2} peak_page_resident_mib={:.2} cpu_cache_mib={:.2} dot_quads={} dot_layouts={} background_jobs={} background_redraws={}",
            s.frames,
            s.page_draws,
            s.images_created,
            s.first_uses,
            s.first_use_bytes as f64 / 1048576.0,
            s.release_calls,
            s.resident_bytes as f64 / 1048576.0,
            s.peak_resident_bytes as f64 / 1048576.0,
            cache_bytes as f64 / 1048576.0,
            s.dot_quads,
            s.dot_layouts,
            s.background_jobs,
            s.background_redraws
        );
    }

    pub fn dot_points(&mut self, width: f32, height: f32) -> &[(f32, f32)] {
        if self.dots.update(width, height) {
            self.stats.dot_layouts += 1;
        }
        &self.dots.points
    }
}

#[derive(Default)]
struct DotGrid {
    size: Option<(f32, f32)>,
    points: Vec<(f32, f32)>,
}

impl DotGrid {
    fn update(&mut self, width: f32, height: f32) -> bool {
        if self.size == Some((width, height)) {
            return false;
        }
        self.size = Some((width, height));
        self.points.clear();
        let mut y = 0.0;
        while y < height {
            let mut x = 0.0;
            while x < width {
                self.points.push((x, y));
                x += 26.0;
            }
            y += 26.0;
        }
        true
    }
}

/// Cull only dots wholly behind an opaque page, allowing for rounded corners.
pub fn dot_exposed(x: f32, y: f32, pages: &[Bounds<Pixels>]) -> bool {
    !pages.iter().any(|page| {
        let left = f32::from(page.origin.x) + 2.0;
        let top = f32::from(page.origin.y) + 2.0;
        x >= left
            && y >= top
            && x + 1.6 <= f32::from(page.origin.x + page.size.width) - 2.0
            && y + 1.6 <= f32::from(page.origin.y + page.size.height) - 2.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{point, px, size};

    fn image() -> Arc<RenderImage> {
        Arc::new(RenderImage::new([image::Frame::new(
            image::RgbaImage::new(8, 8),
        )]))
    }

    #[test]
    fn repainting_a_retained_image_does_not_count_as_another_first_use() {
        let mut resources = Resources::new(Options::default());
        let image = image();
        resources.register(image.clone());
        for _ in 0..100 {
            resources.painted(&image);
        }
        assert_eq!(resources.stats.first_uses, 1);
        assert_eq!(resources.stats.page_draws, 100);
        assert_eq!(resources.stats.resident_bytes, 256);
    }

    #[test]
    fn cache_eviction_defers_release_while_the_image_is_still_displayed() {
        let mut resources = Resources::new(Options::default());
        let old = image();
        let new = image();
        for image in [&old, &new] {
            resources.register(image.clone());
            resources.painted(image);
        }
        let mut releases = Vec::new();
        resources.reconcile(&HashSet::from([old.id, new.id]), |image| {
            releases.push(image.id);
            true
        });
        assert!(releases.is_empty());
        resources.reconcile(&HashSet::from([new.id]), |image| {
            releases.push(image.id);
            true
        });
        assert_eq!(releases, vec![old.id]);
        assert_eq!(resources.stats.resident_bytes, 256);
        resources.release_all(|image| releases.push(image.id));
        assert_eq!(resources.stats.resident_bytes, 0);
        assert_eq!(resources.stats.release_calls, 2);
    }

    #[test]
    fn repeated_document_switches_do_not_accumulate_page_resources() {
        let mut resources = Resources::new(Options::default());
        for _ in 0..100 {
            let image = image();
            resources.register(image.clone());
            resources.painted(&image);
            resources.reconcile(&HashSet::new(), |_| true);
            assert!(resources.images.is_empty());
            assert_eq!(resources.stats.resident_bytes, 0);
        }
        assert_eq!(resources.stats.release_calls, 100);
    }

    #[test]
    fn unused_prefetch_images_do_not_count_as_uploaded_and_failed_releases_retry() {
        let mut resources = Resources::new(Options::default());
        resources.register(image());
        resources.reconcile(&HashSet::new(), |_| false);
        assert_eq!(resources.images.len(), 1);
        resources.reconcile(&HashSet::new(), |_| true);
        assert!(resources.images.is_empty());
        assert_eq!(resources.stats.first_uses, 0);
    }

    #[test]
    fn cached_grid_keeps_exposed_dots_and_skips_page_interiors() {
        let mut grid = DotGrid::default();
        assert!(grid.update(1000.0, 700.0));
        let storage = grid.points.as_ptr();
        assert!(!grid.update(1000.0, 700.0));
        assert_eq!(grid.points.as_ptr(), storage);
        let page = Bounds::new(point(px(100.0), px(10.0)), size(px(800.0), px(680.0)));
        let exposed = grid
            .points
            .iter()
            .filter(|(x, y)| dot_exposed(*x, *y, &[page]))
            .count();
        assert!(exposed < grid.points.len() / 3);
        assert!(
            dot_exposed(100.0, 10.0, &[page]),
            "rounded edge must remain"
        );
        assert!(!dot_exposed(200.0, 200.0, &[page]));
    }

    #[test]
    fn evicted_page_is_removed_from_gpui_atlas() {
        use gpui_kit::{HeadlessAtlas, PlatformAtlas, RenderImageParams};
        use std::borrow::Cow;
        let atlas = HeadlessAtlas::default();
        let mut resources = Resources::new(Options::default());
        for _ in 0..20 {
            let image = image();
            resources.register(image.clone());
            let key = RenderImageParams {
                image_id: image.id,
                frame_index: 0,
            }
            .into();
            atlas
                .get_or_insert_with(key, &mut || {
                    Ok(Some((
                        image.size(0),
                        Cow::Borrowed(image.as_bytes(0).unwrap()),
                    )))
                })
                .unwrap();
            resources.painted(&image);
            // This is the same key/removal operation as Window::drop_image;
            // HeadlessAtlas uses GPUI's shared AtlasState without a GPU.
            resources.reconcile(&HashSet::new(), |image| {
                atlas.remove(
                    &RenderImageParams {
                        image_id: image.id,
                        frame_index: 0,
                    }
                    .into(),
                );
                true
            });
            let mut rebuilt = false;
            atlas
                .get_or_insert_with(
                    RenderImageParams {
                        image_id: image.id,
                        frame_index: 0,
                    }
                    .into(),
                    &mut || {
                        rebuilt = true;
                        Ok(None)
                    },
                )
                .unwrap();
            assert!(rebuilt, "a retired image must not remain in AtlasState");
        }
        assert_eq!(resources.stats.release_calls, 20);
        assert_eq!(resources.stats.resident_bytes, 0);
    }
}
