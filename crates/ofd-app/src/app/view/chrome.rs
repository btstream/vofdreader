use crate::*;

impl ReaderApp {
    pub(crate) fn titlebar(&self, corners: Corners<Pixels>) -> impl IntoElement {
        let p = self.palette();
        div()
            .h(px(44.0))
            .flex_shrink_0()
            .rounded_tl(corners.top_left)
            .rounded_tr(corners.top_right)
            .flex()
            .items_center()
            .bg(rgb(p.titlebar))
            .border_b_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .h_full()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .px_4()
                    .on_mouse_down(MouseButton::Left, |_, window, _| {
                        window.start_window_move();
                    })
                    .child(
                        div()
                            .size(px(20.0))
                            .rounded(px(5.0))
                            .bg(rgb(p.accent))
                            .flex()
                            .justify_center()
                            .items_center()
                            .text_size(px(12.0))
                            .text_color(rgb(0xFFFFFF))
                            .child(IconName::FileText),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(p.text))
                            .child("VofdReader"),
                    )
                    .child(div().w(px(1.0)).h(px(14.0)).bg(rgb(p.border)))
                    .child(
                        div()
                            .max_w(px(520.0))
                            .overflow_hidden()
                            .text_size(px(12.5))
                            .text_color(if self.active.file_name.is_empty() {
                                rgb(p.text_tertiary)
                            } else {
                                rgb(p.text)
                            })
                            .child(if self.active.file_name.is_empty() {
                                "未打开文件".to_string()
                            } else {
                                self.active.file_name.clone()
                            }),
                    )
                    .child(if self.active.page_count() > 0 {
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(p.text_tertiary))
                            .child(format!("· {} 页", self.active.page_count()))
                    } else {
                        div()
                    }),
            )
            .child(
                div()
                    .h_full()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .id("window-min")
                            .w(px(36.0))
                            .h_full()
                            .flex()
                            .justify_center()
                            .items_center()
                            .cursor_pointer()
                            .text_size(px(12.0))
                            .text_color(rgb(p.text_secondary))
                            .hover(|style| {
                                style
                                    .bg(rgb(p.window_control_hover))
                                    .text_color(rgb(p.text))
                            })
                            .active(|style| {
                                style
                                    .bg(rgb(p.window_control_active))
                                    .text_color(rgb(p.text))
                            })
                            .child(div().text_size(px(14.0)).child(IconName::Minus))
                            .on_click(|_, window, _| window.minimize_window()),
                    )
                    .child(
                        div()
                            .id("window-max")
                            .w(px(36.0))
                            .h_full()
                            .flex()
                            .justify_center()
                            .items_center()
                            .cursor_pointer()
                            .text_size(px(12.0))
                            .text_color(rgb(p.text_secondary))
                            .hover(|style| {
                                style
                                    .bg(rgb(p.window_control_hover))
                                    .text_color(rgb(p.text))
                            })
                            .active(|style| {
                                style
                                    .bg(rgb(p.window_control_active))
                                    .text_color(rgb(p.text))
                            })
                            .child(div().text_size(px(12.0)).child(IconName::Square))
                            .on_click(|_, window, _| window.zoom_window()),
                    )
                    .child(
                        div()
                            .id("window-close")
                            .w(px(36.0))
                            .h_full()
                            .flex()
                            .justify_center()
                            .items_center()
                            .cursor_pointer()
                            .text_size(px(12.0))
                            .text_color(rgb(p.text_secondary))
                            // Keep the close button's hover fill inside the
                            // window's rounded top-right corner.
                            .rounded_tr(corners.top_right)
                            .hover(|style| {
                                style
                                    .bg(rgb(p.window_close_hover))
                                    .text_color(rgb(0xFFFFFF))
                            })
                            .active(|style| {
                                style
                                    .bg(rgb(p.window_close_active))
                                    .text_color(rgb(0xFFFFFF))
                            })
                            .child(div().text_size(px(13.0)).child(IconName::X))
                            .on_click(|_, window, cx| {
                                window.remove_window();
                                cx.quit();
                            }),
                    ),
            )
    }

    pub(crate) fn tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette();
        let tabs = self.tabs.iter().enumerate().map(|(index, tab)| {
            let active = self.active_tab == Some(index);
            div()
                .id(("doc-tab", index))
                .relative()
                .h_full()
                .w(px(240.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .border_r_1()
                .border_color(rgb(p.border))
                .bg(if active { rgb(p.surface) } else { rgb(p.bg) })
                .text_size(px(11.5))
                .text_color(if active {
                    rgb(p.text)
                } else {
                    rgb(p.text_secondary)
                })
                .cursor_pointer()
                .when(!active, |this| this.hover(|style| style.bg(rgb(p.surface))))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.activate_tab(index, Some(window), cx);
                }))
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |this, _, window, cx| {
                        this.close_tab(index, window, cx);
                    }),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(px(2.0))
                        .bg(if active {
                            rgb(p.accent)
                        } else {
                            rgba(0x00000000)
                        }),
                )
                .child(div().w(px(16.0)).flex_shrink_0())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_center()
                        .text_ellipsis()
                        .child(tab.file_name.clone()),
                )
                .child(
                    div()
                        .id(("doc-tab-close", index))
                        .flex_shrink_0()
                        .size(px(16.0))
                        .rounded(px(4.0))
                        .flex()
                        .justify_center()
                        .items_center()
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(p.window_control_hover)))
                        .child(icon(IconName::X, 10.0, p.text_tertiary))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_tab(index, window, cx);
                        })),
                )
        });

        div()
            .h(px(34.0))
            .flex_shrink_0()
            .flex()
            .items_stretch()
            .bg(rgb(p.bg))
            .border_b_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .overflow_x_scrollbar()
                    .children(tabs),
            )
    }

    pub(crate) fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette();
        div()
            .h(px(52.0))
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .bg(rgb(p.toolbar))
            .border_b_1()
            .border_color(rgb(p.border))
            .child(
                Button::new("open")
                    .primary()
                    .small()
                    .icon(IconName::FolderOpen)
                    .label("打开")
                    .on_click(cx.listener(Self::on_open)),
            )
            .child(div().w(px(1.0)).h(px(20.0)).bg(rgb(p.border)).mx_1())
            .child(
                div()
                    .id("sidebar-toggle")
                    .w(px(32.0))
                    .h(px(32.0))
                    .rounded(px(7.0))
                    .flex()
                    .justify_center()
                    .items_center()
                    .cursor_pointer()
                    .bg(if self.sidebar_open {
                        rgb(p.accent_soft)
                    } else {
                        rgb(p.surface)
                    })
                    .child(icon(
                        IconName::PanelLeft,
                        16.0,
                        if self.sidebar_open {
                            p.accent
                        } else {
                            p.text_secondary
                        },
                    ))
                    .on_click(cx.listener(Self::on_toggle_sidebar)),
            )
            .child(
                div()
                    .id("search-toggle")
                    .w(px(32.0))
                    .h(px(32.0))
                    .rounded(px(7.0))
                    .flex()
                    .justify_center()
                    .items_center()
                    .cursor_pointer()
                    .child(icon(IconName::Search, 16.0, p.text_secondary))
                    .on_click(cx.listener(Self::on_toggle_search)),
            )
            .child(div().flex_1())
            .child(
                Button::new("print")
                    .ghost()
                    .small()
                    .compact()
                    .text_color(rgb(p.text_secondary))
                    .icon(IconName::Printer)
                    .label("打印")
                    .tooltip("打印（即将支持）")
                    .on_click(cx.listener(Self::on_noop)),
            )
            .child(
                Button::new("export")
                    .ghost()
                    .small()
                    .compact()
                    .text_color(rgb(p.text_secondary))
                    .icon(IconName::Download)
                    .label(if self.exporting {
                        "导出中"
                    } else {
                        "导出"
                    })
                    .tooltip("导出当前页为 PNG 或 SVG")
                    .on_click(cx.listener(Self::on_export)),
            )
            .child(div().w(px(1.0)).h(px(20.0)).bg(rgb(p.border)).mx_1())
            .child(
                Button::new("theme")
                    .ghost()
                    .small()
                    .compact()
                    .text_color(rgb(p.text_secondary))
                    .icon(if self.dark {
                        IconName::Moon
                    } else {
                        IconName::Sun
                    })
                    .tooltip(if self.dark {
                        "切换到亮色主题"
                    } else {
                        "切换到暗色主题"
                    })
                    .on_click(cx.listener(Self::on_toggle_theme)),
            )
            .child(
                Button::new("settings")
                    .ghost()
                    .small()
                    .compact()
                    .text_color(if self.settings_open {
                        rgb(p.accent)
                    } else {
                        rgb(p.text_secondary)
                    })
                    .icon(IconName::Settings)
                    .tooltip("设置")
                    .on_click(cx.listener(Self::on_open_settings)),
            )
            .child({
                let info_entity = cx.entity().downgrade();
                let has_doc = self.active.is_open();
                Button::new("more")
                    .ghost()
                    .small()
                    .compact()
                    .text_color(rgb(p.text_secondary))
                    .icon(IconName::Ellipsis)
                    .tooltip("更多")
                    .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                        let info_entity = info_entity.clone();
                        menu.item(
                            PopupMenuItem::new("文档信息")
                                .icon(IconName::Info)
                                .disabled(!has_doc)
                                .on_click(move |_, _, cx| {
                                    let _ = info_entity.update(cx, |this, cx| {
                                        this.info_dialog_open = true;
                                        cx.notify();
                                    });
                                }),
                        )
                    })
            })
    }

    pub(crate) fn status_bar(
        &self,
        viewport: Size<Pixels>,
        corners: Corners<Pixels>,
    ) -> impl IntoElement {
        let p = self.palette();
        let actual = self.zoom_percent(viewport);
        let size = self
            .active
            .page_sizes
            .get(self.page)
            .map(|size| format!("{:.1} × {:.1} mm", size.width, size.height))
            .unwrap_or_else(|| "—".to_string());

        let signed = self
            .active
            .doc
            .as_ref()
            .map(|doc| doc.has_signature_material())
            .unwrap_or(false);

        div()
            .relative()
            .h(px(28.0))
            .flex_shrink_0()
            .rounded_bl(corners.bottom_left)
            .rounded_br(corners.bottom_right)
            .flex()
            .items_center()
            .justify_between()
            .px_4()
            .bg(rgb(p.titlebar))
            .border_t_1()
            .border_color(rgb(p.border))
            .text_size(px(11.0))
            .text_color(rgb(p.text_secondary))
            .child(if self.active.page_count() == 0 {
                "未打开文档".to_string()
            } else {
                format!(
                    "第 {} 页，共 {} 页",
                    self.page + 1,
                    self.active.page_count()
                )
            })
            .child(size)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .children(
                        self.active
                            .font_reports
                            .get(&self.page)
                            .filter(|report| report.fallback_chars > 0)
                            .map(|report| {
                                div().text_color(rgb(p.warning)).child(format!(
                                    "{} 字已回退（{} 种内嵌字体）",
                                    report.fallback_chars,
                                    report.replaced_fonts.len()
                                ))
                            }),
                    )
                    .child(format!("{actual:.0}%"))
                    .child("OFD 1.0")
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_color(if signed {
                                rgb(p.warning)
                            } else {
                                rgb(p.text_tertiary)
                            })
                            .child(icon(
                                if signed {
                                    IconName::ShieldCheck
                                } else {
                                    IconName::ShieldOff
                                },
                                13.0,
                                if signed { p.warning } else { p.text_tertiary },
                            ))
                            .child(if signed {
                                "含签名（未验证）"
                            } else {
                                "无签名"
                            }),
                    ),
            )
            .children(self.notice.as_ref().map(|(text, error)| {
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .justify_center()
                    .items_center()
                    .text_color(if *error {
                        rgb(p.error)
                    } else {
                        rgb(p.text_secondary)
                    })
                    .child(text.clone())
            }))
    }

    pub(crate) fn floating_bar(
        &self,
        cx: &mut Context<Self>,
        viewport: Size<Pixels>,
    ) -> impl IntoElement {
        let p = self.palette();
        let actual = self.zoom_percent(viewport);
        let page_fit = self.zoom <= self.fit_page_zoom(viewport) + 0.001;

        div()
            .id("floating-bar")
            .absolute()
            .on_mouse_move(cx.listener(Self::on_pointer_move))
            .bottom(px(16.0))
            .left(px(0.0))
            .right(px(0.0))
            .flex()
            .justify_center()
            .child(
                div()
                    .id("floating-bar-controls")
                    .on_hover(cx.listener(|this, hovered, _, _| {
                        this.bar_hovered = *hovered;
                        this.bar_last_activity = Instant::now();
                    }))
                    .h(px(36.0))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .bg(rgb(p.surface))
                    .border_1()
                    .border_color(rgb(p.border_strong))
                    .rounded(px(18.0))
                    .shadow_lg()
                    .child(
                        div()
                            .id("bar-zoom-out")
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(7.0))
                            .flex()
                            .justify_center()
                            .items_center()
                            .cursor_pointer()
                            .child(icon(IconName::Minus, 15.0, 0x4B5563))
                            .on_click(cx.listener(Self::on_zoom_out)),
                    )
                    .child(
                        div()
                            .min_w(px(42.0))
                            .text_size(px(11.5))
                            .text_color(rgb(p.text))
                            .text_center()
                            .child(format!("{actual:.0}%")),
                    )
                    .child(
                        div()
                            .id("bar-zoom-in")
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(7.0))
                            .flex()
                            .justify_center()
                            .items_center()
                            .cursor_pointer()
                            .child(icon(IconName::Plus, 15.0, 0x4B5563))
                            .on_click(cx.listener(Self::on_zoom_in)),
                    )
                    .child(div().w(px(1.0)).h(px(20.0)).bg(rgb(p.border)).mx_1())
                    .child(
                        div()
                            .id("bar-fit-width")
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(7.0))
                            .flex()
                            .justify_center()
                            .items_center()
                            .cursor_pointer()
                            .child(icon(IconName::MoveHorizontal, 15.0, 0x4B5563))
                            .on_click(cx.listener(Self::on_fit_width)),
                    )
                    .child(
                        div()
                            .id("bar-fit-page")
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(7.0))
                            .flex()
                            .justify_center()
                            .items_center()
                            .cursor_pointer()
                            .bg(if page_fit {
                                rgb(p.accent_soft)
                            } else {
                                rgb(p.surface)
                            })
                            .child(icon(
                                IconName::Scan,
                                15.0,
                                if page_fit { p.accent } else { p.text_secondary },
                            ))
                            .on_click(cx.listener(Self::on_fit)),
                    )
                    .child(
                        div()
                            .id("bar-view-mode")
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(7.0))
                            .flex()
                            .justify_center()
                            .items_center()
                            .cursor_pointer()
                            .bg(if self.view_mode == ViewMode::Continuous {
                                rgb(p.accent_soft)
                            } else {
                                rgb(p.surface)
                            })
                            .child(icon(
                                if self.view_mode == ViewMode::Continuous {
                                    IconName::Rows3
                                } else {
                                    IconName::FileText
                                },
                                15.0,
                                if self.view_mode == ViewMode::Continuous {
                                    p.accent
                                } else {
                                    0x4B5563
                                },
                            ))
                            .on_click(cx.listener(Self::on_toggle_view_mode)),
                    )
                    .child(div().w(px(1.0)).h(px(20.0)).bg(rgb(p.border)).mx_1())
                    .child(
                        div()
                            .id("bar-rotate")
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(7.0))
                            .flex()
                            .justify_center()
                            .items_center()
                            .cursor_pointer()
                            .child(icon(
                                IconName::RotateCw,
                                15.0,
                                if self.rotation == 0 {
                                    0x4B5563
                                } else {
                                    p.accent
                                },
                            ))
                            .on_click(cx.listener(Self::on_rotate)),
                    )
                    .child(
                        div()
                            .id("bar-view")
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(7.0))
                            .flex()
                            .justify_center()
                            .items_center()
                            .cursor_pointer()
                            .bg(if self.grid_open {
                                rgb(p.accent_soft)
                            } else {
                                rgb(p.surface)
                            })
                            .child(icon(
                                IconName::Grid2x2,
                                15.0,
                                if self.grid_open { p.accent } else { 0x4B5563 },
                            ))
                            .on_click(cx.listener(Self::on_toggle_grid)),
                    ),
            )
    }
}
