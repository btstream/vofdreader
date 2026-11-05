//! Repeatable Release-mode timings for each rendering stage.
//!
//! cargo run --release -p ofd-core --example bench -- FILE [PAGE] [DPI]

use std::time::Instant;

use ofd_core::{OfdDocument, PageRenderer};

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: bench FILE [PAGE] [DPI]");
    let page: usize = args.next().map_or(0, |value| value.parse().unwrap());
    let dpi: f32 = args.next().map_or(168.0, |value| value.parse().unwrap());
    let query = args.next();

    let t = Instant::now();
    let doc = OfdDocument::open(&path).unwrap();
    println!("open                       {:8.2} ms", ms(t));

    let t = Instant::now();
    let size = doc.page_size(page).unwrap();
    println!("page_size (cold)           {:8.2} ms  {size:?}", ms(t));
    let t = Instant::now();
    let _ = doc.page_size(page).unwrap();
    println!("page_size (cold again)     {:8.2} ms", ms(t));
    let t = Instant::now();
    let count = doc.page_count();
    let resolved = (0..count)
        .filter(|index| doc.page_size_only(*index).ok().flatten().is_some())
        .count();
    println!(
        "page_size_only x {count:<4}      {:8.2} ms  {resolved} resolved",
        ms(t)
    );

    let t = Instant::now();
    let list = doc.display_list(page).unwrap();
    println!(
        "display_list               {:8.2} ms  {} top-level items",
        ms(t),
        list.items.len()
    );

    let t = Instant::now();
    let svg = doc.page_svg(page).unwrap();
    println!(
        "page_svg (export)          {:8.2} ms  {:.1} KiB",
        ms(t),
        svg.len() as f64 / 1024.0
    );
    let t = Instant::now();
    let svg = doc.page_svg_for_render(page).unwrap();
    println!(
        "page_svg (render)          {:8.2} ms  {:.1} KiB",
        ms(t),
        svg.len() as f64 / 1024.0
    );

    let t = Instant::now();
    let renderer = PageRenderer::new(&doc);
    println!("PageRenderer::new          {:8.2} ms", ms(t));

    for (label, dpi) in [("factored", 168.0_f32), ("2x-ish", 384.0)] {
        let t = Instant::now();
        let raster = renderer.rasterize(&doc, page, dpi).unwrap();
        println!(
            "rasterize {label:<10}      {:8.2} ms  {}x{}",
            ms(t),
            raster.width,
            raster.height
        );
        let t = Instant::now();
        let raster2 = renderer.rasterize(&doc, page, dpi).unwrap();
        println!(
            "rasterize {label:<10} warm {:8.2} ms  {}x{}",
            ms(t),
            raster2.width,
            raster2.height
        );
    }

    // Tile path: a 3x2 "viewport" of 512px tiles and the whole-page grid, as
    // the desktop app requests them (2px gutter, serial, shared parsed tree).
    for tile_dpi in [384.0_f32, 768.0] {
        let width = (list.size.width * f64::from(tile_dpi) / 25.4).ceil() as u32;
        let height = (list.size.height * f64::from(tile_dpi) / 25.4).ceil() as u32;
        let columns = width.div_ceil(512);
        let rows = height.div_ceil(512);
        let region = |x: u32, y: u32| {
            let x0 = (x * 512).saturating_sub(2);
            let y0 = (y * 512).saturating_sub(2);
            ofd_core::RasterRegion {
                x: x0,
                y: y0,
                width: ((x + 1) * 512 + 2).min(width) - x0,
                height: ((y + 1) * 512 + 2).min(height) - y0,
            }
        };
        let mut times = Vec::new();
        let t = Instant::now();
        for y in 0..rows {
            for x in 0..columns {
                let tile = Instant::now();
                renderer
                    .rasterize_region(&doc, page, tile_dpi, region(x, y))
                    .unwrap();
                times.push(tile.elapsed().as_secs_f64() * 1000.0);
            }
        }
        let total = ms(t);
        times.sort_by(f64::total_cmp);
        let viewport: f64 = (0..rows.min(2))
            .flat_map(|y| (0..columns.min(3)).map(move |x| (x, y)))
            .map(|(x, y)| {
                let t = Instant::now();
                renderer
                    .rasterize_region(&doc, page, tile_dpi, region(x, y))
                    .unwrap();
                ms(t)
            })
            .sum();
        // Same grid on 4 workers sharing the renderer, like the app's slots.
        let keys: Vec<_> = (0..rows)
            .flat_map(|y| (0..columns).map(move |x| (x, y)))
            .collect();
        let next = std::sync::atomic::AtomicUsize::new(0);
        let t = Instant::now();
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    while let Some(&(x, y)) =
                        keys.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
                    {
                        renderer
                            .rasterize_region(&doc, page, tile_dpi, region(x, y))
                            .unwrap();
                    }
                });
            }
        });
        let parallel = ms(t);
        println!("tiles {tile_dpi:>4} dpi grid on 4 threads {parallel:8.2} ms");
        println!(
            "tiles {tile_dpi:>4} dpi grid {columns}x{rows}  {total:8.2} ms  median {:.2} max {:.2} ms/tile; 3x2 viewport {viewport:.2} ms",
            times[times.len() / 2],
            times[times.len() - 1],
        );
    }

    if let Some(query) = query {
        let t = Instant::now();
        let rects = doc.page_search_rects(page, &query).unwrap();
        println!(
            "search {query:?}              {:8.2} ms  {} rects",
            ms(t),
            rects.len()
        );
        for rect in rects.iter().take(8) {
            println!(
                "  x={:.2} y={:.2} w={:.2} h={:.2} mm",
                rect.x, rect.y, rect.width, rect.height
            );
        }
    }

    let t = Instant::now();
    let png = renderer.render_png(&doc, page, dpi).unwrap();
    println!(
        "render_png                 {:8.2} ms  {:.1} KiB",
        ms(t),
        png.png.len() as f64 / 1024.0
    );
}
