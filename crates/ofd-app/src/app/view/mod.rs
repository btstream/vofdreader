pub(crate) mod chrome;
pub(crate) mod dialogs;
pub(crate) mod pages;
pub(crate) mod search;
pub(crate) mod sidebar;
use crate::*;

impl Render for ReaderApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette();
        if let Some(value) = self.search_pending_input.take() {
            cx.defer_in(window, move |this, window, cx| {
                this.set_search_input_value(window, &value, cx);
                cx.notify();
            });
        }
        let root_font = Font {
            family: self.ui_font.family.clone(),
            fallbacks: (!self.ui_font.fallbacks.is_empty())
                .then(|| FontFallbacks::from_fonts(self.ui_font.fallbacks.clone())),
            ..Font::default()
        };
        self.sync_gpu_resources(window);
        self.page_coverage.borrow_mut().clear();
        self.tile_wanted.clear();
        self.tile_prefetch.clear();
        if !self.focused {
            window.focus(&self.focus_handle, cx);
            self.focused = true;
        }
        if self.appearance_subscription.is_none() {
            let reader = cx.entity().downgrade();
            self.appearance_subscription =
                Some(window.observe_window_appearance(move |window, cx| {
                    let dark = matches!(window.appearance(), WindowAppearance::Dark);
                    let _ = reader.update(cx, |this, cx| {
                        if this.settings.appearance.theme == settings::ThemeChoice::System
                            && this.dark != dark
                        {
                            this.dark = dark;
                            apply_component_theme(dark, &this.ui_font, cx);
                            cx.notify();
                        }
                    });
                }));
        }

        self.viewport = window.viewport_size();
        self.window_scale = window.scale_factor();
        if self.view_mode == ViewMode::Single {
            self.pending_restore_page = None;
        }
        if self.doc.is_some()
            && self.view_mode == ViewMode::Continuous
            && !self.grid_open
            && self.pending_restore_page.is_none()
        {
            self.sync_continuous_page(self.viewport, cx);
        }
        if self.doc.is_some()
            && !self.grid_open
            && self.view_mode == ViewMode::Single
            && !self.loading
            && !self.dpi_pending
            && self.target_dpi() > self.image_dpi
        {
            self.schedule_dpi_render(cx);
        }
        // Defensive recovery: the document is open but no frame is displayed.
        // This can happen after switching documents while a stale image_dpi is
        // still around; never show the "open a file" state in that case.
        if self.doc.is_some()
            && !self.grid_open
            && self.view_mode == ViewMode::Single
            && self.page_image.is_none()
            && !self.loading
            && self.error.is_none()
            && self.renderer.is_some()
        {
            self.request_page_render(cx);
        }
        let page = if self.grid_open && self.doc.is_some() {
            self.page_grid(cx)
        } else if self.view_mode == ViewMode::Continuous {
            self.continuous_view(self.viewport, cx)
        } else {
            let (page_w, page_h) = self.display_size(self.viewport);
            if let Some(image) = &self.page_image {
                let bounds_cell = self.page_bounds.clone();
                let mut stack = div()
                    .relative()
                    .w(page_w)
                    .h(page_h)
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_page_mouse_down))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_page_mouse_up))
                    .child(
                        self.page_texture(image.clone())
                            .when(!self.gpu.borrow().options.no_page_shadows, |image| {
                                image.shadow_lg()
                            })
                            .rounded(px(1.0)),
                    )
                    .child(self.page_occlusion())
                    .child(self.tile_layer(self.page, cx))
                    .child(
                        canvas(
                            move |bounds, _, _| {
                                bounds_cell.set(Some(bounds));
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    );
                if !self.search_highlights.is_empty() {
                    stack = stack.child(overlay_layer(
                        self.search_highlights.clone(),
                        rgba(SEARCH_HIGHLIGHT),
                    ));
                }
                if let Some(active) = &self.search_active_highlight {
                    stack = stack.child(overlay_layer(
                        vec![active.clone()],
                        rgba(SEARCH_ACTIVE_HIGHLIGHT),
                    ));
                }
                if !self.selection_rects.is_empty() {
                    stack = stack.child(overlay_layer(
                        self.selection_rects.clone(),
                        rgba(0x2F6FEB55),
                    ));
                }
                stack.into_any_element()
            } else if let Some(error) = &self.error {
                div()
                    .max_w(px(560.0))
                    .p_6()
                    .rounded_lg()
                    .bg(rgb(p.error_soft))
                    .border_1()
                    .border_color(rgb(p.error_border))
                    .text_size(px(13.0))
                    .text_color(rgb(p.error))
                    .children(error.lines().map(|line| div().child(line.to_string())))
                    .into_any_element()
            } else if self.loading || self.doc.is_some() {
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.0))
                    .text_color(rgb(p.text_secondary))
                    .child("正在渲染页面…")
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_3()
                    .text_color(rgb(p.text_secondary))
                    .child(
                        div()
                            .text_size(px(20.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(p.text))
                            .child("打开一个 OFD 文件"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .child("支持电子发票、公文、合同等版式文档，也可直接拖入窗口"),
                    )
                    .child(
                        Button::new("empty-open")
                            .primary()
                            .label("选择文件")
                            .on_click(cx.listener(Self::on_open)),
                    )
                    .children(if self.recent.is_empty() {
                        None
                    } else {
                        Some(
                            div()
                                .mt_2()
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(p.text_tertiary))
                                        .child("最近打开"),
                                )
                                .children(self.recent.iter().take(6).enumerate().map(
                                    |(index, path)| {
                                        let name = path
                                            .file_name()
                                            .map(|name| name.to_string_lossy().into_owned())
                                            .unwrap_or_else(|| path.display().to_string());
                                        let parent = path
                                            .parent()
                                            .map(|parent| parent.display().to_string())
                                            .unwrap_or_default();
                                        let path = path.clone();
                                        div()
                                            .id(("recent", index))
                                            .w(px(420.0))
                                            .px_3()
                                            .py_1()
                                            .rounded(px(6.0))
                                            .flex()
                                            .flex_col()
                                            .items_center()
                                            .cursor_pointer()
                                            .text_size(px(11.5))
                                            .text_color(rgb(p.text_secondary))
                                            .child(name)
                                            .child(
                                                div()
                                                    .max_w(px(400.0))
                                                    .overflow_hidden()
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(p.text_tertiary))
                                                    .child(parent),
                                            )
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.open_path(path.clone(), Some(window), cx);
                                            }))
                                    },
                                )),
                        )
                    })
                    .into_any_element()
            }
        };

        let reader_bounds = self.reader_bounds.clone();
        let mut canvas = div()
            .id("page-canvas")
            .flex_1()
            .min_h_0()
            .relative()
            .flex()
            .justify_center()
            .items_center()
            .bg(rgb(p.canvas))
            .overflow_hidden()
            .on_scroll(cx.listener(Self::on_single_scroll))
            .child(dot_grid(
                p.dot,
                self.gpu.clone(),
                self.page_coverage.clone(),
            ))
            .child(page)
            .child(
                canvas(
                    move |bounds, _, _| reader_bounds.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );

        if self.doc.is_some() && self.settings.reading.floating_toolbar && self.bar_visible {
            canvas = canvas.child(self.floating_bar(cx, window.viewport_size()));
        }
        if self.search_open {
            canvas = canvas.child(self.search_panel(cx));
        }
        // Only on-screen pages: background prefetch neither shows the badge
        // nor needs a repaint to hide it again.
        if self.loading
            || (self.view_mode == ViewMode::Continuous
                && self
                    .continuous_pending
                    .iter()
                    .any(|(page, _)| self.page_on_screen(*page)))
        {
            canvas = canvas.child(
                div()
                    .absolute()
                    .top(px(12.0))
                    .right(px(12.0))
                    .px_3()
                    .py_1()
                    .rounded(px(14.0))
                    .bg(rgb(p.surface))
                    .border_1()
                    .border_color(rgb(p.border))
                    .shadow_sm()
                    .text_size(px(10.5))
                    .text_color(rgb(p.text_tertiary))
                    .child("渲染中…"),
            );
        }

        let mut reading = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .flex()
            .flex_col();
        if self.tabs.len() > 1 {
            reading = reading.child(self.tab_bar(cx));
        }
        let reading = reading.child(canvas);

        let body = if self.doc.is_some() && self.sidebar_open {
            div()
                .flex_1()
                .flex()
                .overflow_hidden()
                .child(self.sidebar(cx))
                .child(reading)
        } else {
            div().flex_1().flex().overflow_hidden().child(reading)
        };

        // GPUI Kit owns the only outer frame. Its border is 1px, so content
        // corners use a concentric 4px radius inside the requested 5px frame.
        let corners = content_corners(window.window_decorations(), window.is_fullscreen());

        div()
            .id("root")
            .relative()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .flex()
            .flex_col()
            .font(root_font)
            .bg(rgb(p.bg))
            .text_color(rgb(p.text))
            .on_mouse_move(cx.listener(Self::on_pointer_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_page_mouse_up))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                let paths = paths.paths();
                let ofd = paths.iter().find(|path| {
                    path.extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("ofd"))
                });
                if let Some(path) = ofd.or_else(|| paths.first()) {
                    this.open_path(path.clone(), Some(window), cx);
                }
            }))
            .rounded_tl(corners.top_left)
            .rounded_tr(corners.top_right)
            .rounded_bl(corners.bottom_left)
            .rounded_br(corners.bottom_right)
            .child(self.titlebar(corners))
            .child(self.toolbar(cx))
            .child(body)
            .child(self.status_bar(window.viewport_size(), corners))
            .child(self.info_dialog(cx))
            .child(self.settings_dialog(window.viewport_size(), cx))
    }
}
