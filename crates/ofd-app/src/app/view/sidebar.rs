use crate::*;

impl ReaderApp {
    pub(crate) fn thumbnail(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let selected = index == self.page;
        let size = self
            .active
            .page_sizes
            .get(index)
            .copied()
            .unwrap_or(SizeMm::new(210.0, 297.0));
        let ratio = (size.width / size.height.max(0.01)) as f32;

        let preview = if let Some(image) = self
            .active
            .thumbs
            .get(index)
            .and_then(|image| image.as_ref())
        {
            img(image.clone())
                .size_full()
                .shadow_sm()
                .rounded(px(2.0))
                .into_any_element()
        } else {
            div()
                .size_full()
                .flex()
                .justify_center()
                .items_center()
                .bg(rgb(p.surface))
                .border_1()
                .border_color(rgb(p.border))
                .rounded(px(2.0))
                .text_size(px(10.0))
                .text_color(rgb(p.text_tertiary))
                .child("…")
                .into_any_element()
        };

        div()
            .id(("thumb", index))
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.go_to_page(index, cx);
            }))
            .child(
                div()
                    .w_full()
                    .p(px(2.0))
                    .rounded(px(6.0))
                    .border_2()
                    .border_color(if selected {
                        rgb(p.accent)
                    } else {
                        rgb(p.sidebar)
                    })
                    // The page's own aspect ratio keeps single-column previews
                    // undistorted while filling the sidebar width.
                    .child(div().w_full().aspect_ratio(ratio).child(preview)),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(if selected {
                        rgb(p.accent)
                    } else {
                        rgb(p.text_tertiary)
                    })
                    .child(format!("{}", index + 1)),
            )
            .into_any_element()
    }

    pub(crate) fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette();
        let visible = self.active.page_count().min(THUMB_LIMIT);
        let searching = self.search.open;
        let tab = |label: &'static str, active: bool| {
            div()
                .h_full()
                .flex()
                .items_center()
                .border_b_2()
                .border_color(if active {
                    rgb(p.accent)
                } else {
                    rgb(p.sidebar)
                })
                .text_color(if active {
                    rgb(p.text)
                } else {
                    rgb(p.text_tertiary)
                })
                .when(active, |this| this.font_weight(FontWeight::SEMIBOLD))
                .child(label)
        };

        let thumbs = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(32.0))
                    .px_5()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(10.0))
                    .text_color(rgb(p.text_tertiary))
                    .child("页面")
                    .child(format!("{visible}")),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .px_3()
                    .pb_3()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children((0..visible).map(|index| self.thumbnail(index, cx))),
            )
            .into_any_element();

        div()
            .w(px(240.0))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(p.sidebar))
            .border_r_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .h(px(44.0))
                    .flex()
                    .items_center()
                    .gap_4()
                    .px_5()
                    .text_size(px(11.5))
                    .child(tab("缩略图", !searching))
                    .child(tab("目录", false))
                    .child(tab("书签", false))
                    .child(tab("搜索结果", searching)),
            )
            .child(if searching {
                self.sidebar_search_results(cx)
            } else {
                thumbs
            })
    }

    /// Search results replace the thumbnail list while the search panel is
    /// open, keeping the document visible next to a narrow result list.
    pub(crate) fn sidebar_search_results(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let query = self.search.query.trim();
        let query_empty = query.is_empty();
        let count = self.search.hits.len();
        let position = if count == 0 {
            String::new()
        } else {
            format!("{} / {}", self.search.index + 1, count)
        };
        let summary = if query_empty {
            "等待输入".to_string()
        } else if count == 0 {
            "未找到匹配".to_string()
        } else {
            format!("{count} 处匹配")
        };

        let list = if query_empty {
            div()
                .px_4()
                .py_3()
                .text_size(px(11.0))
                .text_color(rgb(p.text_tertiary))
                .child("输入关键词搜索文档内容")
                .into_any_element()
        } else if count == 0 {
            div()
                .px_4()
                .py_3()
                .text_size(px(11.0))
                .text_color(rgb(p.text_tertiary))
                .child("未找到匹配内容")
                .into_any_element()
        } else {
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .id("sidebar-search-results")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&self.search.results_scroll)
                        .pb_2()
                        .flex()
                        .flex_col()
                        .children(self.search.hits.iter().enumerate().map(|(index, hit)| {
                            let current = index == self.search.index;
                            let snippet = highlight_matches(&hit.snippet, query, p.search_match);
                            div()
                                .id(("sidebar-search-hit", index))
                                .w_full()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .px_4()
                                .py_3()
                                // The accent rail marks the result that Enter and the
                                // arrow keys currently point at.
                                .border_l_2()
                                .border_color(if current {
                                    rgb(p.accent)
                                } else {
                                    rgba(0x00000000)
                                })
                                .bg(if current {
                                    rgb(p.surface)
                                } else {
                                    rgb(p.sidebar)
                                })
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(p.surface)))
                                .child(
                                    div()
                                        .flex()
                                        .justify_between()
                                        .text_size(px(10.0))
                                        .child(
                                            div()
                                                .text_color(rgb(p.accent))
                                                .child(format!("第 {} 页", hit.page + 1)),
                                        )
                                        .child(
                                            div()
                                                .text_color(rgb(p.text_tertiary))
                                                .child(format!("{}", index + 1)),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(rgb(p.text_secondary))
                                        .child(snippet),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.select_search_hit(index, cx)
                                }))
                        }))
                        .child(
                            div()
                                .mx_4()
                                .mt_3()
                                .border_t_1()
                                .border_color(rgb(p.border))
                                .pt_3()
                                .pb_3()
                                .text_size(px(10.5))
                                .text_color(rgb(p.text_tertiary))
                                .text_center()
                                .child(if count >= SEARCH_LIMIT {
                                    format!("已显示前 {count} 条结果")
                                } else {
                                    "已显示全部结果".to_string()
                                }),
                        ),
                )
                .vertical_scrollbar(&self.search.results_scroll)
                .into_any_element()
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(32.0))
                    .px_5()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(10.0))
                    .text_color(rgb(p.text_tertiary))
                    .child(summary)
                    .child(position),
            )
            .child(list)
            .into_any_element()
    }
}
