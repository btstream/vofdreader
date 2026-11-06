use crate::*;

impl ReaderApp {
    pub(crate) fn page_texture(&self, image: Arc<RenderImage>) -> impl IntoElement + Styled {
        let gpu = self.gpu.clone();
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let visible = bounds.intersect(&window.content_mask().bounds);
                if visible.size.width <= px(0.0) || visible.size.height <= px(0.0) {
                    return;
                }
                if window
                    .paint_image(
                        bounds,
                        bounds,
                        Corners::all(px(1.0)),
                        image.clone(),
                        0,
                        false,
                    )
                    .is_ok()
                {
                    gpu.borrow_mut().painted(&image);
                }
            },
        )
        .absolute()
        .size_full()
    }

    pub(crate) fn page_occlusion(&self) -> impl IntoElement {
        let coverage = self.page_coverage.clone();
        canvas(
            move |bounds, _, _| coverage.borrow_mut().push(bounds),
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
    }

    pub(crate) fn continuous_page_card(
        &self,
        index: usize,
        image: Option<Arc<RenderImage>>,
        viewport: Size<Pixels>,
        current: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let (width, height) = self.continuous_page_size(viewport, index);
        let mut card = div()
            .relative()
            .w(px(width))
            .h(px(height))
            .rounded(px(1.0))
            .when(!self.gpu.borrow().options.no_page_shadows, |card| {
                card.shadow_lg()
            })
            .bg(rgb(p.surface));
        if let Some(image) = image {
            card = card.child(self.page_texture(image));
        } else if let Some(image) = self.thumbs.get(index).and_then(Option::as_ref) {
            card = card.child(img(image.clone()).absolute().size_full());
        } else {
            card = card.child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .items_center()
                    .gap_1()
                    .text_size(px(11.0))
                    .text_color(rgb(p.text_tertiary))
                    .child(format!("第 {} 页", index + 1))
                    .child("正在渲染…"),
            );
        }
        card = card.child(self.tile_layer(index, cx));
        if current {
            let bounds_cell = self.page_bounds.clone();
            card = card
                .on_mouse_down(MouseButton::Left, cx.listener(Self::on_page_mouse_down))
                .on_mouse_up(MouseButton::Left, cx.listener(Self::on_page_mouse_up))
                .child(
                    canvas(
                        move |bounds, _, _| bounds_cell.set(Some(bounds)),
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                );
            if !self.search_highlights.is_empty() {
                card = card.child(overlay_layer(
                    self.search_highlights.clone(),
                    rgba(SEARCH_HIGHLIGHT),
                ));
            }
            if let Some(active) = &self.search_active_highlight {
                card = card.child(overlay_layer(
                    vec![active.clone()],
                    rgba(SEARCH_ACTIVE_HIGHLIGHT),
                ));
            }
            if !self.selection_rects.is_empty() {
                card = card.child(overlay_layer(
                    self.selection_rects.clone(),
                    rgba(0x2F6FEB55),
                ));
            }
        }
        card = card.child(self.page_occlusion());
        div()
            .w_full()
            .flex_shrink_0()
            .flex()
            .justify_center()
            .child(card)
            .into_any_element()
    }

    /// Page overview opened from the floating toolbar. Thumbnail PNGs are
    /// shared with the sidebar; pages past the thumbnail budget show a
    /// lightweight placeholder rather than blocking the grid.
    pub(crate) fn page_grid(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let handle = self.grid_scroll.clone();
        let mut cards = Vec::with_capacity(self.page_count);
        for index in 0..self.page_count {
            let size = self
                .page_sizes
                .get(index)
                .copied()
                .unwrap_or(SizeMm::new(210.0, 297.0));
            let thumb_h =
                (168.0 * size.height as f32 / size.width.max(0.01) as f32).clamp(96.0, 260.0);
            let current = index == self.page;
            let mut thumb = div()
                .relative()
                .w(px(168.0))
                .h(px(thumb_h))
                .overflow_hidden()
                .rounded(px(5.0))
                .border_1()
                .border_color(rgb(if current { p.accent } else { p.border }))
                .bg(rgb(p.surface))
                .shadow_sm();
            if let Some(image) = self.thumbs.get(index).and_then(Option::as_ref) {
                thumb = thumb.child(img(image.clone()).absolute().size_full());
            } else {
                thumb = thumb.child(
                    div()
                        .size_full()
                        .flex()
                        .justify_center()
                        .items_center()
                        .text_size(px(12.0))
                        .text_color(rgb(p.text_tertiary))
                        .child(format!("{}", index + 1)),
                );
            }
            cards.push(
                div()
                    .id(("grid-page", index))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .child(thumb)
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(rgb(if current { p.accent } else { p.text_secondary }))
                            .child(format!("第 {} 页", index + 1)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.grid_open = false;
                        if this.page == index {
                            if this.view_mode == ViewMode::Continuous {
                                this.scroll_to_continuous_page(this.viewport, index);
                            }
                            cx.notify();
                        } else {
                            this.go_to_page(index, cx);
                        }
                    })),
            );
        }
        div()
            .id("page-grid-frame")
            .relative()
            .size_full()
            .child(
                div()
                    .id("page-grid")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&handle)
                    .px_6()
                    .py_5()
                    .child(
                        div()
                            .mb_4()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(12.0))
                            .text_color(rgb(p.text))
                            .child("页面总览")
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(p.text_tertiary))
                                    .child(format!("共 {} 页", self.page_count)),
                            ),
                    )
                    .child(div().flex().flex_wrap().gap_4().children(cards)),
            )
            .vertical_scrollbar(&handle)
            .into_any_element()
    }

    pub(crate) fn continuous_view(
        &mut self,
        viewport: Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_continuous_pages(viewport, cx);
        let range = self.continuous_visible_range(viewport);
        let (top, bottom, width, page_gap) = self.with_continuous_layout(viewport, |layout| {
            let (top, bottom) = layout.spacers(range.clone());
            (
                top,
                bottom,
                layout.sizes.first().map_or(0.0, |size| size.0),
                layout.gap(),
            )
        });
        let mut pages = Vec::with_capacity(range.len());
        for index in range {
            let dpi = self.continuous_dpi(viewport, index);
            // Keep the old bitmap while scrolling/zooming; refinement waits
            // until idle. Only visible rows touch the LRU or clone images.
            let image = self
                .cached_page_image(index, dpi)
                .map(|(_, image)| image)
                .or_else(|| {
                    let key = self
                        .page_cache
                        .keys()
                        .filter(|(page, _)| *page == index)
                        .max_by_key(|(_, dpi)| *dpi)
                        .copied()?;
                    let image = self.page_cache.get(&key).cloned();
                    self.touch_cached_page(key.0, key.1);
                    image
                });
            let height = self.continuous_page_size(viewport, index).1;
            let gap = if index + 1 < self.page_count {
                page_gap
            } else {
                0.0
            };
            pages.push(
                div()
                    .w(px(width + 48.0))
                    .min_w_full()
                    .h(px(height + gap))
                    .flex_shrink_0()
                    .child(self.continuous_page_card(
                        index,
                        image,
                        viewport,
                        index == self.page,
                        cx,
                    )),
            );
        }
        let handle = self.continuous_scroll.clone();
        // The scrollbar must be a sibling of the scrolling element. Adding it
        // inside an overflow container made it scroll away with the pages.
        div()
            .id("continuous-frame")
            .relative()
            .flex_1()
            .h_full()
            .w_full()
            .min_h_0()
            .child(
                div()
                    .id("continuous-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&handle)
                    .on_scroll(cx.listener(Self::on_continuous_scroll))
                    .flex()
                    .flex_col()
                    .child(div().h(px(top)).flex_shrink_0())
                    .children(pages)
                    .child(div().h(px(bottom)).flex_shrink_0()),
            )
            .vertical_scrollbar(&handle)
            .into_any_element()
    }

    pub(crate) fn tile_layer(&self, page: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let dpi = self.tile_density(page);
        let size = self
            .page_sizes
            .get(page)
            .copied()
            .unwrap_or(SizeMm::new(210.0, 297.0));
        let cache = self.tile_cache.clone();
        let gpu = self.gpu.clone();
        let entity = cx.entity().downgrade();
        let generation = self.doc_generation;
        let view_generation = self.view_generation;
        let rotation = self.rotation;
        let ring = self.cache_config.prefetch_tile_ring();
        let (rotated_w, rotated_h) = rotate::size(
            size.width as f32 * dpi.unwrap_or(1) as f32 / 25.4,
            size.height as f32 * dpi.unwrap_or(1) as f32 / 25.4,
            rotation,
        );
        let (exact_w, exact_h) = (rotated_w, rotated_h);
        canvas(
            move |bounds, window, cx| {
                let (keys, nearby) = if let Some(dpi) = dpi {
                    let width = exact_w.ceil() as u32;
                    let height = exact_h.ceil() as u32;
                    let clip = bounds.intersect(&window.content_mask().bounds);
                    let sx = exact_w / f32::from(bounds.size.width);
                    let sy = exact_h / f32::from(bounds.size.height);
                    let keys = tiles::plan(
                        page,
                        dpi,
                        (width, height),
                        [
                            f32::from(clip.origin.x - bounds.origin.x) * sx,
                            f32::from(clip.origin.y - bounds.origin.y) * sy,
                            f32::from(clip.origin.x + clip.size.width - bounds.origin.x) * sx,
                            f32::from(clip.origin.y + clip.size.height - bounds.origin.y) * sy,
                        ],
                    );
                    let nearby = tiles::neighbors(&keys, (width, height), ring);
                    (keys, nearby)
                } else {
                    (vec![], vec![])
                };
                let wanted = keys.clone();
                cx.defer(move |cx| {
                    let _ = entity.update(cx, |this, _| {
                        if this.doc_generation == generation
                            && this.view_generation == view_generation
                        {
                            for key in &wanted {
                                if this.tile_cache.contains_key(key) {
                                    this.tile_order.retain(|old| old != key);
                                    this.tile_order.push_back(*key);
                                }
                            }
                            this.tile_wanted.insert(page, wanted);
                            this.tile_prefetch.insert(page, nearby);
                        }
                    });
                    // Run after every page has contributed its prepaint plan.
                    // A speculative block on page A must not delay a visible
                    // block on page B just because A's callback ran first.
                    cx.defer(move |cx| {
                        let _ = entity.update(cx, |this, cx| {
                            if this.doc_generation == generation
                                && this.view_generation == view_generation
                            {
                                this.schedule_nearby(cx);
                            }
                        });
                    });
                });
                keys
            },
            move |bounds, keys, window, _| {
                let Some(_) = dpi else {
                    return;
                };
                let sx = f32::from(bounds.size.width) / exact_w;
                let sy = f32::from(bounds.size.height) / exact_h;
                for key in keys {
                    let Some(image) = cache.get(&key) else {
                        continue;
                    };
                    let (content, sample) =
                        key.regions(exact_w.ceil() as u32, exact_h.ceil() as u32);
                    let rect = |r: ofd_core::RasterRegion| {
                        Bounds::new(
                            point(
                                bounds.origin.x + px(r.x as f32 * sx),
                                bounds.origin.y + px(r.y as f32 * sy),
                            ),
                            gpui_kit::size(px(r.width as f32 * sx), px(r.height as f32 * sy)),
                        )
                    };
                    let clip = rect(content)
                        .intersect(&bounds)
                        .intersect(&window.content_mask().bounds);
                    window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                        let sample_bounds = rect(sample);
                        if window
                            .paint_image(
                                sample_bounds,
                                sample_bounds,
                                Corners::all(px(0.0)),
                                image.clone(),
                                0,
                                false,
                            )
                            .is_ok()
                        {
                            gpu.borrow_mut().painted(image);
                        }
                    });
                }
            },
        )
        .absolute()
        .size_full()
    }
}
