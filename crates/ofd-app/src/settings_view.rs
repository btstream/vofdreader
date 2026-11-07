//! Settings modal and the bridge from a control to live reader state.
use super::*;
use crate::settings::{QualityChoice, SettingsAction};

mod about;
mod appearance;
mod files;
mod general;
mod performance;
mod reading;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SettingsTab {
    General,
    Appearance,
    #[default]
    Reading,
    Performance,
    Files,
    About,
}

impl SettingsTab {
    const ALL: [Self; 6] = [
        Self::General,
        Self::Appearance,
        Self::Reading,
        Self::Performance,
        Self::Files,
        Self::About,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::General => "通用",
            Self::Appearance => "外观",
            Self::Reading => "阅读",
            Self::Performance => "性能",
            Self::Files => "文件与缓存",
            Self::About => "关于",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::General => IconName::Settings,
            Self::Appearance => IconName::Sun,
            Self::Reading => IconName::FileText,
            Self::Performance => IconName::Scan,
            Self::Files => IconName::FolderOpen,
            Self::About => IconName::Info,
        }
    }
}

impl ReaderApp {
    pub(crate) fn settings_dialog(
        &self,
        viewport: Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.settings_open {
            return div().into_any_element();
        }
        let p = self.palette();
        let width = (f32::from(viewport.width) - 80.0).clamp(640.0, 880.0);
        let height = (f32::from(viewport.height) - 80.0).clamp(480.0, 760.0);
        let backdrop = if self.dark {
            rgba(0x05070B99)
        } else {
            rgba(0x0E111680)
        };

        div()
            .id("settings-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_center()
            .bg(backdrop)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.close_settings(cx)),
            )
            .child(
                div()
                    .id("settings-dialog")
                    .w(px(width))
                    .h(px(height))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .rounded(px(14.0))
                    .bg(rgb(p.surface))
                    .border_1()
                    .border_color(rgb(p.border_strong))
                    .shadow_lg()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .child(self.settings_header(p, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .overflow_hidden()
                            .child(self.settings_nav(p, cx))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .id("settings-content")
                                            .flex_1()
                                            .min_h_0()
                                            .overflow_y_scrollbar()
                                            .px(px(28.0))
                                            .py(px(24.0))
                                            .child(self.settings_page(cx)),
                                    )
                                    .child(self.settings_footer(p, cx)),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn settings_header(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(60.0))
            .flex_shrink_0()
            .px(px(24.0))
            .flex()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .child(
                        div()
                            .text_size(px(15.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(p.text))
                            .child("设置"),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(rgb(p.text_tertiary))
                            .child("阅读偏好、界面外观与渲染性能"),
                    ),
            )
            .child(
                div()
                    .id("settings-close")
                    .size(px(24.0))
                    .rounded(px(6.0))
                    .flex()
                    .justify_center()
                    .items_center()
                    .cursor_pointer()
                    .bg(self.control_bg())
                    .child(icon(IconName::X, 14.0, p.text_tertiary))
                    .on_click(cx.listener(|this, _, _, cx| this.close_settings(cx))),
            )
    }

    fn settings_nav(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(188.0))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .px(px(12.0))
            .py(px(12.0))
            // GPUI only clips children to a rectangle, so this fill would
            // otherwise square off the dialog's rounded bottom-left corner.
            .rounded_bl(px(13.0))
            .bg(rgb(p.sidebar))
            .border_r_1()
            .border_color(rgb(p.border))
            .children(
                SettingsTab::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(index, tab)| {
                        let selected = tab == self.settings_tab;
                        div()
                            .id(("settings-tab", index))
                            .h(px(36.0))
                            .px(px(12.0))
                            .rounded(px(8.0))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .cursor_pointer()
                            .when(selected, |item| item.bg(rgb(p.accent_soft)))
                            .child(icon(
                                tab.icon(),
                                16.0,
                                if selected { p.accent } else { p.text_secondary },
                            ))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .text_color(rgb(if selected {
                                        p.accent
                                    } else {
                                        p.text_secondary
                                    }))
                                    // 600 has no CJK face in common fallback
                                    // families and maps to 700; Medium keeps
                                    // the selected label in the same family.
                                    .when(selected, |label| label.font_weight(FontWeight::MEDIUM))
                                    .child(tab.label()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.settings_tab = tab;
                                this.settings_reset_armed = false;
                                cx.notify();
                            }))
                    }),
            )
            .child(div().flex_1())
            .child(
                div()
                    .px(px(12.0))
                    .pb(px(8.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .text_size(px(9.5))
                    .text_color(rgb(p.text_tertiary))
                    .child(format!("VofdReader {}", env!("CARGO_PKG_VERSION")))
                    .child("GPL-3.0-or-later"),
            )
    }

    fn settings_footer(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let armed = self.settings_reset_armed;
        div()
            .h(px(56.0))
            .flex_shrink_0()
            .px(px(28.0))
            .flex()
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .id("settings-reset")
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .cursor_pointer()
                    .text_size(px(11.0))
                    .text_color(rgb(if armed { p.error } else { p.text_secondary }))
                    .child(icon(
                        IconName::RotateCw,
                        13.0,
                        if armed { p.error } else { p.text_secondary },
                    ))
                    .child(if armed {
                        "再次点击确认恢复"
                    } else {
                        "恢复默认设置"
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.settings_reset_armed {
                            this.apply_settings_action(SettingsAction::ResetDefaults, window, cx);
                        } else {
                            this.settings_reset_armed = true;
                            cx.notify();
                        }
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(rgb(if armed { p.warning } else { p.text_tertiary }))
                            .child(if armed {
                                "将重置所有偏好，阅读位置会保留"
                            } else {
                                "更改会自动保存"
                            }),
                    )
                    .child(
                        Button::new("settings-done")
                            .primary()
                            .small()
                            .label("完成")
                            .on_click(cx.listener(|this, _, _, cx| this.close_settings(cx))),
                    ),
            )
    }

    fn settings_page(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.settings_tab {
            SettingsTab::General => self.general_page(cx),
            SettingsTab::Appearance => self.appearance_page(cx),
            SettingsTab::Reading => self.reading_page(cx),
            SettingsTab::Performance => self.performance_page(cx),
            SettingsTab::Files => self.files_page(cx),
            SettingsTab::About => self.about_page(cx),
        }
    }

    fn page_heading(&self, title: &str, description: &str) -> Div {
        let p = self.palette();
        div()
            .mb(px(14.0))
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .text_size(px(16.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(p.text))
                    .child(title.to_string()),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(p.text_secondary))
                    .child(description.to_string()),
            )
    }

    fn section_title(&self, title: &str) -> Div {
        let p = self.palette();
        div()
            .mt(px(12.0))
            .mb(px(2.0))
            .text_size(px(10.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(p.text_tertiary))
            .child(title.to_string())
    }

    fn setting_row(&self, label: &str, description: &str, control: impl IntoElement) -> Div {
        let p = self.palette();
        div()
            .w_full()
            .min_h(px(50.0))
            .py(px(6.0))
            .flex()
            .items_center()
            .gap(px(24.0))
            .border_b_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(rgb(p.text))
                            .child(label.to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(rgb(p.text_tertiary))
                            .child(description.to_string()),
                    ),
            )
            .child(div().flex_shrink_0().child(control))
    }

    fn setting_toggle(
        &self,
        id: &'static str,
        on: bool,
        action: SettingsAction,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = self.palette();
        div()
            .id(id)
            .w(px(36.0))
            .h(px(20.0))
            .rounded(px(10.0))
            .p(px(2.0))
            .flex()
            .items_center()
            .cursor_pointer()
            .bg(rgb(if on { p.accent } else { p.border_strong }))
            .when(on, |toggle| toggle.justify_end())
            .child(
                div()
                    .size(px(16.0))
                    .rounded(px(8.0))
                    .bg(rgb(0xFFFFFF))
                    .shadow_sm(),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.apply_settings_action(action, window, cx);
            }))
    }

    fn segmented(
        &self,
        id: &'static str,
        selected: usize,
        options: Vec<(&'static str, SettingsAction)>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = self.palette();
        div()
            .id(id)
            .flex()
            .items_center()
            .p(px(2.0))
            .rounded(px(8.0))
            .bg(self.control_bg())
            .children(
                options
                    .into_iter()
                    .enumerate()
                    .map(|(index, (label, action))| {
                        let active = index == selected;
                        div()
                            .id((id, index))
                            .h(px(26.0))
                            .px(px(12.0))
                            .rounded(px(6.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .when(active, |item| {
                                item.bg(rgb(p.surface))
                                    .border_1()
                                    .border_color(rgb(p.border))
                                    .text_color(rgb(p.accent))
                                    .font_weight(FontWeight::SEMIBOLD)
                            })
                            .when(!active, |item| item.text_color(rgb(p.text_secondary)))
                            .child(label)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.apply_settings_action(action, window, cx);
                            }))
                    }),
            )
    }

    fn stepper(
        &self,
        id: &'static str,
        value: String,
        enabled: bool,
        minus: SettingsAction,
        plus: SettingsAction,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = self.palette();
        let button = |id_suffix: usize, action: SettingsAction, glyph: IconName| {
            div()
                .id((id, id_suffix))
                .size(px(26.0))
                .rounded(px(6.0))
                .flex()
                .justify_center()
                .items_center()
                .when(enabled, |button| {
                    button
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.apply_settings_action(action, window, cx);
                        }))
                })
                .child(icon(
                    glyph,
                    13.0,
                    if enabled {
                        p.text_secondary
                    } else {
                        p.text_tertiary
                    },
                ))
        };
        div()
            .id(id)
            .w(px(108.0))
            .h(px(30.0))
            .px(px(2.0))
            .flex()
            .items_center()
            .justify_between()
            .rounded(px(7.0))
            .border_1()
            .border_color(rgb(p.border))
            .bg(self.control_bg())
            .opacity(if enabled { 1.0 } else { 0.45 })
            .child(button(0, minus, IconName::Minus))
            .child(
                div()
                    .flex_1()
                    .text_center()
                    .text_size(px(11.0))
                    .text_color(rgb(p.text))
                    .child(value),
            )
            .child(button(1, plus, IconName::Plus))
    }

    fn control_bg(&self) -> Rgba {
        if self.dark {
            rgba(0x252A33FF)
        } else {
            rgba(0xF1F3F6FF)
        }
    }

    fn settings_card(&self) -> Div {
        let p = self.palette();
        div()
            .w_full()
            .rounded(px(10.0))
            .border_1()
            .border_color(rgb(p.border))
            .bg(if self.dark {
                rgba(0x20242CFF)
            } else {
                rgba(0xF9FAFBFF)
            })
    }

    fn note(&self, text: &str) -> Div {
        let p = self.palette();
        div()
            .mt(px(10.0))
            .text_size(px(9.5))
            .text_color(rgb(p.text_tertiary))
            .child(text.to_string())
    }

    pub(crate) fn apply_settings_action(
        &mut self,
        action: SettingsAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings_reset_armed = false;
        match action {
            SettingsAction::ToggleRestoreLastFile => {
                self.settings.general.restore_last_file = !self.settings.general.restore_last_file;
            }
            SettingsAction::SetTheme(theme) => {
                self.settings.appearance.theme = theme;
                let dark = theme.resolve(matches!(window.appearance(), WindowAppearance::Dark));
                self.dark = dark;
                apply_component_theme(dark, &self.ui_font, cx);
            }
            SettingsAction::ToggleSidebarDefault => {
                self.settings.appearance.sidebar_open = !self.settings.appearance.sidebar_open;
                self.sidebar_open = self.settings.appearance.sidebar_open;
                if self.view_mode == ViewMode::Continuous && !self.grid_open {
                    self.scroll_to_continuous_page(self.viewport, self.page);
                }
            }
            SettingsAction::SetDefaultView(view) => {
                self.settings.reading.default_view = view;
            }
            SettingsAction::SetPageGap(gap) => {
                self.settings.reading.page_gap = gap;
                if self.view_mode == ViewMode::Continuous && !self.grid_open {
                    self.scroll_to_continuous_page(self.viewport, self.page);
                }
            }
            SettingsAction::ToggleRestorePosition => {
                self.settings.reading.restore_position = !self.settings.reading.restore_position;
            }
            SettingsAction::ToggleFloatingToolbar => {
                self.settings.reading.floating_toolbar = !self.settings.reading.floating_toolbar;
                if !self.settings.reading.floating_toolbar {
                    self.bar_visible = false;
                    self.bar_hovered = false;
                }
            }
            SettingsAction::ToggleDotGrid => {
                self.settings.reading.dot_grid = !self.settings.reading.dot_grid;
                self.apply_gpu_setting_flags();
            }
            SettingsAction::TogglePageShadows => {
                self.settings.reading.page_shadows = !self.settings.reading.page_shadows;
                self.apply_gpu_setting_flags();
            }
            SettingsAction::ToggleSearchAutoScroll => {
                self.settings.reading.search_auto_scroll =
                    !self.settings.reading.search_auto_scroll;
            }
            SettingsAction::SetImageSampling(sampling) => {
                self.settings.performance.image_sampling = sampling;
            }
            SettingsAction::SetQuality(quality) => {
                self.settings.performance.quality = quality;
            }
            SettingsAction::AdjustPageLimit(delta) => {
                let value = self.settings.performance.page_limit as i32 + delta;
                self.settings.performance.page_limit = value.clamp(4, 256) as usize;
                self.cache_config = crate::settings_cache_config(&self.settings);
            }
            SettingsAction::AdjustParsedLimit(delta) => {
                let value = self.settings.performance.parsed_page_limit as i32 + delta;
                self.settings.performance.parsed_page_limit = value.clamp(1, 64) as usize;
                self.cache_config = crate::settings_cache_config(&self.settings);
            }
            SettingsAction::AdjustPrefetchPages(delta) => {
                let value = self.settings.performance.prefetch_pages as i32 + delta;
                self.settings.performance.prefetch_pages = value.clamp(0, 4) as usize;
                self.cache_config = crate::settings_cache_config(&self.settings);
            }
            SettingsAction::AdjustTileRing(delta) => {
                let value = self.settings.performance.tile_ring as i32 + delta;
                self.settings.performance.tile_ring = value.clamp(0, 2) as u32;
                self.cache_config = crate::settings_cache_config(&self.settings);
            }
            SettingsAction::AdjustRenderThreads(delta) => {
                // 0 (automatic) sits below 1; manual values stop at the cores.
                let value = self.settings.performance.render_threads as i32 + delta;
                self.settings.performance.render_threads =
                    value.clamp(0, crate::tiles::max_threads() as i32) as usize;
            }
            SettingsAction::AdjustPageDpi(delta) => {
                let value = self.settings.performance.page_dpi_cap() as i32 + delta;
                self.settings.performance.max_page_dpi = value.clamp(96, 768) as u32;
                self.settings.performance.quality = QualityChoice::Custom;
            }
            SettingsAction::SetExportFormat(format) => {
                self.settings.files.export_format = format;
            }
            SettingsAction::AdjustExportDpi(delta) => {
                self.settings.files.export_dpi =
                    (self.settings.files.export_dpi + delta as f32).clamp(36.0, 600.0);
            }
            SettingsAction::AdjustRecentLimit(delta) => {
                let value = self.settings.files.recent_limit as i32 + delta;
                self.settings.files.recent_limit = value.clamp(3, 50) as usize;
                self.recent.truncate(self.settings.files.recent_limit);
                recent::save_recent(&self.recent, self.settings.files.recent_limit);
            }
            SettingsAction::ClearRecent => {
                self.recent.clear();
                recent::save_recent(&self.recent, self.settings.files.recent_limit);
            }
            SettingsAction::ClearThumbnails => self.clear_thumbnail_cache(window, cx),
            SettingsAction::ResetDefaults => {
                let positions = std::mem::take(&mut self.settings.positions);
                self.settings = crate::settings::Settings::default();
                self.settings.positions = positions;
                let dark = self
                    .settings
                    .appearance
                    .theme
                    .resolve(matches!(window.appearance(), WindowAppearance::Dark));
                self.dark = dark;
                apply_component_theme(dark, &self.ui_font, cx);
                self.sidebar_open = self.settings.appearance.sidebar_open;
                self.bar_visible = false;
                self.apply_gpu_setting_flags();
                self.cache_config = crate::settings_cache_config(&self.settings);
                let cache_mib = self.settings.performance.cache_mib;
                self.cache_slider.update(cx, |state, cx| {
                    state.set_value(cache_mib as f32, window, cx);
                });
                self.recent.truncate(self.settings.files.recent_limit);
                recent::save_recent(&self.recent, self.settings.files.recent_limit);
                if self.view_mode == ViewMode::Continuous && !self.grid_open {
                    self.scroll_to_continuous_page(self.viewport, self.page);
                }
            }
        }
        self.settings.normalize();
        // Takes effect for the next scheduled jobs; running jobs finish.
        self.tile_slots = crate::tiles::concurrency(self.settings.performance.render_threads);
        self.apply_image_sampling(cx);
        self.settings.save();
        cx.notify();
    }

    fn apply_gpu_setting_flags(&self) {
        let no_dots = std::env::var("OFD_GPU_DISABLE_DOTS").as_deref() == Ok("1")
            || !self.settings.reading.dot_grid;
        let no_page_shadows = std::env::var("OFD_GPU_DISABLE_PAGE_SHADOWS").as_deref() == Ok("1")
            || !self.settings.reading.page_shadows;
        let mut gpu = self.gpu.borrow_mut();
        gpu.options.no_dots = no_dots;
        gpu.options.no_page_shadows = no_page_shadows;
    }

    /// Drop in-memory thumbnails and regenerate them. Their atlas entries are
    /// released by the next frame's GPU reconcile.
    fn clear_thumbnail_cache(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        for thumb in &mut self.active.thumbs {
            *thumb = None;
        }
        // Invalidates batches that are still running.
        self.active.thumbnail_batch = self.active.thumbnail_batch.wrapping_add(1);
        self.active.thumbs_started = false;
        if self.active.is_open() && self.active.renderer.is_some() {
            self.active.thumbs_started = true;
            self.spawn_thumbnails(cx);
        }
        cx.notify();
    }
}
