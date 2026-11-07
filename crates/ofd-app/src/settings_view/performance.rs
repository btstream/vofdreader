use crate::settings::*;
use crate::*;

impl ReaderApp {
    pub(crate) fn performance_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let settings = &self.settings;
        let performance = &settings.performance;
        let quality_index = match performance.quality {
            QualityChoice::Auto => 0,
            QualityChoice::Standard => 1,
            QualityChoice::High => 2,
            QualityChoice::Custom => 3,
        };
        let custom = performance.quality == QualityChoice::Custom;
        let page_cap = performance.page_dpi_cap();
        let cache_label = if performance.cache_mib >= 1024 {
            format!("{:.1} GiB", performance.cache_mib as f32 / 1024.0)
        } else {
            format!("{} MiB", performance.cache_mib)
        };

        let advanced_cell = |label: &str,
                             description: &str,
                             value: u32,
                             minus: SettingsAction,
                             plus: SettingsAction,
                             id: &'static str,
                             cx: &mut Context<Self>| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(p.text))
                                .child(label.to_string()),
                        )
                        .child(
                            div()
                                .text_size(px(9.5))
                                .text_color(rgb(p.text_tertiary))
                                .child(description.to_string()),
                        ),
                )
                .child(self.stepper(id, value.to_string(), custom, minus, plus, cx))
        };

        div()
            .flex()
            .flex_col()
            .child(self.page_heading(
                "性能",
                "调整渲染质量、缓存与预取策略；页面 DPI 仍受缓存预算约束，解析树缓存在下次打开文档时生效。",
            ))
            .child(
                self.settings_card()
                    .p(px(12.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(p.text))
                                    .child("渲染缓存上限"),
                            )
                            .child(
                                div()
                                    .text_size(px(17.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(p.accent))
                                    .child(cache_label),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(rgb(p.text_tertiary))
                            .child("整页位图与高清分块共享的保留预算；不足时自动降低预览清晰度"),
                    )
                    .child(
                        div()
                            .pt(px(4.0))
                            .child(Slider::new(&self.cache_slider).horizontal()),
                    )
                    .child(
                        // Ticks follow the slider's linear 64–2048 MiB scale.
                        // Equal spacing put "512" at 33% while its real
                        // position is about 22.6%.
                        div()
                            .relative()
                            .h(px(14.0))
                            .text_size(px(9.5))
                            .text_color(rgb(p.text_tertiary))
                            .child(div().absolute().left_0().child("64"))
                            .child(
                                div()
                                    .absolute()
                                    .left(relative(crate::settings::cache_fraction(512)))
                                    .ml(px(-12.0))
                                    .w(px(24.0))
                                    .text_center()
                                    .child("512"),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left(relative(crate::settings::cache_fraction(1024)))
                                    .ml(px(-17.0))
                                    .w(px(34.0))
                                    .text_center()
                                    .child("1 GiB"),
                            )
                            .child(div().absolute().right_0().child("2 GiB")),
                    )
                    .child(
                        div()
                            .text_size(px(9.5))
                            .text_color(rgb(p.text_tertiary))
                            .child(
                                "缓存越大，往返滚动越少重复渲染；预算不含字体、页面树和 GPU 纹理。",
                            ),
                    ),
            )
            .child(self.section_title("渲染质量"))
            .child(self.setting_row(
                "质量档位",
                "页面按档位 DPI 渲染，与屏幕分辨率无关；放大时按比例提高",
                self.segmented(
                    "performance-quality",
                    quality_index,
                    vec![
                        ("自动", SettingsAction::SetQuality(QualityChoice::Auto)),
                        ("标准", SettingsAction::SetQuality(QualityChoice::Standard)),
                        ("高清", SettingsAction::SetQuality(QualityChoice::High)),
                        ("自定义", SettingsAction::SetQuality(QualityChoice::Custom)),
                    ],
                    cx,
                ),
            ))
            .child(self.setting_row(
                "图片采样",
                "页面中的图片缩放时的插值方式；双线性在大图页面上比双三次快 2–3 倍",
                self.segmented(
                    "performance-image-sampling",
                    match performance.image_sampling {
                        ImageSampling::Nearest => 0,
                        ImageSampling::Bilinear => 1,
                        ImageSampling::Bicubic => 2,
                    },
                    vec![
                        (
                            "原始像素",
                            SettingsAction::SetImageSampling(ImageSampling::Nearest),
                        ),
                        (
                            "双线性",
                            SettingsAction::SetImageSampling(ImageSampling::Bilinear),
                        ),
                        (
                            "双三次",
                            SettingsAction::SetImageSampling(ImageSampling::Bicubic),
                        ),
                    ],
                    cx,
                ),
            ))
            .child(
                self.settings_card()
                    .mt(px(4.0))
                    .p(px(12.0))
                    .flex()
                    .items_center()
                    .gap(px(18.0))
                    .child(advanced_cell(
                        "页面 DPI",
                        "100% 缩放时的渲染密度，滚动与预取都按此渲染",
                        page_cap,
                        SettingsAction::AdjustPageDpi(-24),
                        SettingsAction::AdjustPageDpi(24),
                        "performance-page-dpi",
                        cx,
                    )),
            )
            .child(self.section_title("缓存细分"))
            .child(self.setting_row(
                "整页缓存上限",
                "最多保留的整页位图数量",
                self.stepper(
                    "performance-page-limit",
                    format!("{} 张", performance.page_limit),
                    true,
                    SettingsAction::AdjustPageLimit(-1),
                    SettingsAction::AdjustPageLimit(1),
                    cx,
                ),
            ))
            .child(self.setting_row(
                "解析页缓存",
                "保留已解析页面树的页数",
                self.stepper(
                    "performance-parsed-limit",
                    format!("{} 页", performance.parsed_page_limit),
                    true,
                    SettingsAction::AdjustParsedLimit(-1),
                    SettingsAction::AdjustParsedLimit(1),
                    cx,
                ),
            ))
            .child(self.section_title("并行渲染"))
            .child(self.setting_row(
                "渲染线程数",
                "同时渲染的高清分块数；自动为 CPU 核数的一半（1–4）",
                self.stepper(
                    "performance-render-threads",
                    match performance.render_threads {
                        0 => format!(
                            "自动（{}）",
                            crate::tiles::concurrency(0)
                        ),
                        threads => format!("{threads} 线程"),
                    },
                    true,
                    SettingsAction::AdjustRenderThreads(-1),
                    SettingsAction::AdjustRenderThreads(1),
                    cx,
                ),
            ))
            .child(self.section_title("预取"))
            .child(
                self.settings_card()
                    .p(px(12.0))
                    .flex()
                    .items_center()
                    .gap(px(18.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(3.0))
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .text_color(rgb(p.text))
                                            .child("预取前后页面"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.5))
                                            .text_color(rgb(p.text_tertiary))
                                            .child("空闲时预渲染相邻页面"),
                                    ),
                            )
                            .child(self.stepper(
                                "performance-prefetch-pages",
                                format!("{} 页", performance.prefetch_pages),
                                true,
                                SettingsAction::AdjustPrefetchPages(-1),
                                SettingsAction::AdjustPrefetchPages(1),
                                cx,
                            )),
                    )
                    .child(div().w(px(1.0)).h(px(38.0)).bg(rgb(p.border)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(3.0))
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .text_color(rgb(p.text))
                                            .child("邻块预取范围"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.5))
                                            .text_color(rgb(p.text_tertiary))
                                            .child("可见分块外围预取圈数"),
                                    ),
                            )
                            .child(self.stepper(
                                "performance-tile-ring",
                                format!("{} 圈", performance.tile_ring),
                                true,
                                SettingsAction::AdjustTileRing(-1),
                                SettingsAction::AdjustTileRing(1),
                                cx,
                            )),
                    ),
            )
            .into_any_element()
    }
}
