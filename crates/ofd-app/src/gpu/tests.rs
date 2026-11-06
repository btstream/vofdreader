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
