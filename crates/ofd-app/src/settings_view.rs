//! Settings modal and the bridge from a control to live reader state.
use super::*;
use crate::settings::{
    GapChoice, ImageSampling, QualityChoice, SettingsAction, ThemeChoice, ViewChoice,
};

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

    fn general_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let settings = &self.settings;
        let path = crate::settings::Settings::path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "不可用".to_string());
        div()
            .flex()
            .flex_col()
            .child(self.page_heading("通用", "启动行为与本地配置。"))
            .child(self.section_title("启动"))
            .child(self.setting_row(
                "恢复上次打开的文件",
                "启动应用时重新打开上次关闭的文档",
                self.setting_toggle(
                    "general-restore-file",
                    settings.general.restore_last_file,
                    SettingsAction::ToggleRestoreLastFile,
                    cx,
                ),
            ))
            .child(self.section_title("界面"))
            .child(
                self.setting_row(
                    "界面语言",
                    "当前版本仅提供简体中文",
                    div()
                        .px(px(10.0))
                        .py(px(5.0))
                        .rounded(px(6.0))
                        .bg(self.control_bg())
                        .text_size(px(11.0))
                        .text_color(rgb(p.text_secondary))
                        .child("简体中文"),
                ),
            )
            .child(self.note(&format!("配置文件：{path}")))
            .into_any_element()
    }

    fn appearance_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let settings = &self.settings;
        let theme_index = match settings.appearance.theme {
            ThemeChoice::System => 0,
            ThemeChoice::Light => 1,
            ThemeChoice::Dark => 2,
        };
        div()
            .flex()
            .flex_col()
            .child(self.page_heading("外观", "主题和侧栏的默认状态。"))
            .child(self.section_title("主题"))
            .child(self.setting_row(
                "界面主题",
                "跟随系统，或固定使用亮色 / 暗色",
                self.segmented(
                    "appearance-theme",
                    theme_index,
                    vec![
                        ("跟随系统", SettingsAction::SetTheme(ThemeChoice::System)),
                        ("亮色", SettingsAction::SetTheme(ThemeChoice::Light)),
                        ("暗色", SettingsAction::SetTheme(ThemeChoice::Dark)),
                    ],
                    cx,
                ),
            ))
            .child(self.section_title("侧栏"))
            .child(self.setting_row(
                "默认展开侧栏",
                "打开文档时显示缩略图导航",
                self.setting_toggle(
                    "appearance-sidebar",
                    settings.appearance.sidebar_open,
                    SettingsAction::ToggleSidebarDefault,
                    cx,
                ),
            ))
            .child(self.note("主题切换会立即应用到窗口、按钮和弹出菜单。"))
            .into_any_element()
    }

    fn reading_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let settings = &self.settings;
        let view_index = match settings.reading.default_view {
            ViewChoice::Single => 0,
            ViewChoice::Continuous => 1,
        };
        let gap_index = match settings.reading.page_gap {
            GapChoice::Compact => 0,
            GapChoice::Standard => 1,
            GapChoice::Wide => 2,
        };
        div()
            .flex()
            .flex_col()
            .child(self.page_heading("阅读", "控制页面显示、滚动行为和文字选择。"))
            .child(self.section_title("显示"))
            .child(self.setting_row(
                "默认视图",
                "打开文档时使用的阅读布局",
                self.segmented(
                    "reading-view",
                    view_index,
                    vec![
                        ("单页", SettingsAction::SetDefaultView(ViewChoice::Single)),
                        (
                            "连续",
                            SettingsAction::SetDefaultView(ViewChoice::Continuous),
                        ),
                    ],
                    cx,
                ),
            ))
            .child(self.setting_row(
                "页面间距",
                "连续视图相邻页面之间的留白",
                self.segmented(
                    "reading-gap",
                    gap_index,
                    vec![
                        ("紧凑", SettingsAction::SetPageGap(GapChoice::Compact)),
                        ("标准", SettingsAction::SetPageGap(GapChoice::Standard)),
                        ("宽松", SettingsAction::SetPageGap(GapChoice::Wide)),
                    ],
                    cx,
                ),
            ))
            .child(self.setting_row(
                "启动时恢复上次阅读位置",
                "重新打开同一文件时跳转到上次页码",
                self.setting_toggle(
                    "reading-position",
                    settings.reading.restore_position,
                    SettingsAction::ToggleRestorePosition,
                    cx,
                ),
            ))
            .child(self.setting_row(
                "显示浮动工具栏",
                "鼠标靠近阅读区底部时显示缩放和视图控件",
                self.setting_toggle(
                    "reading-toolbar",
                    settings.reading.floating_toolbar,
                    SettingsAction::ToggleFloatingToolbar,
                    cx,
                ),
            ))
            .child(self.section_title("页面外观"))
            .child(self.setting_row(
                "点阵背景",
                "在页面周围的画布上显示细点纹理",
                self.setting_toggle(
                    "reading-dots",
                    settings.reading.dot_grid,
                    SettingsAction::ToggleDotGrid,
                    cx,
                ),
            ))
            .child(self.setting_row(
                "页面阴影",
                "为纸张添加轻微投影，关闭可减少绘制开销",
                self.setting_toggle(
                    "reading-shadows",
                    settings.reading.page_shadows,
                    SettingsAction::TogglePageShadows,
                    cx,
                ),
            ))
            .child(self.section_title("文字与搜索"))
            .child(self.setting_row(
                "搜索命中后自动滚动",
                "切换到包含命中内容的页面并保持在视口中央",
                self.setting_toggle(
                    "reading-search-scroll",
                    settings.reading.search_auto_scroll,
                    SettingsAction::ToggleSearchAutoScroll,
                    cx,
                ),
            ))
            .into_any_element()
    }

    fn performance_page(&self, cx: &mut Context<Self>) -> AnyElement {
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
                            .child("整页位图与高清分块共享的保留预算"),
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

    fn files_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let settings = &self.settings;
        let format_index = match settings.files.export_format {
            ExportFormat::Png => 0,
            ExportFormat::Svg => 1,
        };
        let (thumb_count, thumb_bytes) =
            self.thumbs
                .iter()
                .flatten()
                .fold((0, 0u64), |(count, bytes), image| {
                    let size = image.size(0);
                    (
                        count + 1,
                        bytes + size.width.0 as u64 * size.height.0 as u64 * 4,
                    )
                });
        let thumb_summary = if thumb_count == 0 {
            "仅保存在内存中，当前为空".to_string()
        } else {
            format!(
                "仅保存在内存中：{thumb_count} 张，约 {}",
                crate::human_size(thumb_bytes)
            )
        };
        div()
            .flex()
            .flex_col()
            .child(self.page_heading("文件与缓存", "导出默认值和本地缓存管理。"))
            .child(self.section_title("导出"))
            .child(self.setting_row(
                "默认导出格式",
                "保存对话框会使用该扩展名，仍可手动改为另一种格式",
                self.segmented(
                    "files-format",
                    format_index,
                    vec![
                        ("PNG", SettingsAction::SetExportFormat(ExportFormat::Png)),
                        ("SVG", SettingsAction::SetExportFormat(ExportFormat::Svg)),
                    ],
                    cx,
                ),
            ))
            .child(self.setting_row(
                "PNG 导出 DPI",
                "只影响导出图片，不影响屏幕渲染",
                self.stepper(
                    "files-export-dpi",
                    format!("{:.0}", settings.files.export_dpi),
                    true,
                    SettingsAction::AdjustExportDpi(-24),
                    SettingsAction::AdjustExportDpi(24),
                    cx,
                ),
            ))
            .child(self.section_title("最近文件"))
            .child(self.setting_row(
                "最近文件数量",
                "显示在空状态和最近打开列表中的条数",
                self.stepper(
                    "files-recent-limit",
                    format!("{} 条", settings.files.recent_limit),
                    true,
                    SettingsAction::AdjustRecentLimit(-1),
                    SettingsAction::AdjustRecentLimit(1),
                    cx,
                ),
            ))
            .child(
                div()
                    .py(px(12.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(rgb(p.text_tertiary))
                            .child(format!("当前记录 {} 个文件", self.recent.len())),
                    )
                    .child(
                        Button::new("files-clear-recent")
                            .ghost()
                            .small()
                            .label("清除最近文件")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.apply_settings_action(SettingsAction::ClearRecent, window, cx);
                            })),
                    ),
            )
            .child(self.section_title("缓存"))
            .child(
                div()
                    .py(px(12.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(16.0))
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
                                    .child("缩略图缓存"),
                            )
                            .child(
                                div()
                                    .text_size(px(9.5))
                                    .text_color(rgb(p.text_tertiary))
                                    .child(thumb_summary),
                            ),
                    )
                    .child(
                        Button::new("files-clear-thumbs")
                            .ghost()
                            .small()
                            .label("清理缓存")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.apply_settings_action(
                                    SettingsAction::ClearThumbnails,
                                    window,
                                    cx,
                                );
                            })),
                    ),
            )
            .into_any_element()
    }

    fn about_page(&self, _cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let rows = [
            ("版本", env!("CARGO_PKG_VERSION").to_string()),
            ("许可证", "GPL-3.0-or-later".to_string()),
            ("标准", "GB/T 33190-2016 OFD".to_string()),
            (
                "渲染管线",
                match ofd_core::RasterBackend::from_env() {
                    ofd_core::RasterBackend::Direct => "DisplayList → tiny-skia → BGRA",
                    ofd_core::RasterBackend::Svg => "DisplayList → SVG → usvg/resvg → BGRA",
                }
                .to_string(),
            ),
            (
                "配置文件",
                crate::settings::Settings::path()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "不可用".to_string()),
            ),
        ];
        div()
            .flex()
            .flex_col()
            .child(self.page_heading("关于", "VofdReader 是一款轻量的 OFD 阅读器。"))
            .child(
                self.settings_card()
                    .p(px(18.0))
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .children(rows.into_iter().map(|(label, value)| {
                        div()
                            .w_full()
                            .flex()
                            .items_start()
                            .gap(px(20.0))
                            .child(
                                div()
                                    .w(px(76.0))
                                    .flex_shrink_0()
                                    .text_size(px(11.0))
                                    .text_color(rgb(p.text_tertiary))
                                    .child(label),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(11.0))
                                    .text_color(rgb(p.text_secondary))
                                    .child(value),
                            )
                    })),
            )
            .child(self.note("签名印章仅展示外观，不执行 SM2/SM3 验签。"))
            .into_any_element()
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
        for thumb in &mut self.thumbs {
            *thumb = None;
        }
        self.thumbs_started = false;
        if self.doc.is_some() && self.renderer.is_some() {
            self.thumbs_started = true;
            self.spawn_thumbnails(cx);
        }
        cx.notify();
    }
}
