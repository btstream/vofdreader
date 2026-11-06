use crate::*;

impl ReaderApp {
    pub(crate) fn search_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let Some(input) = &self.search_input else {
            return div().into_any_element();
        };
        let query_empty = self.search_query.trim().is_empty();
        let count = self.search_hits.len();

        let bar = div()
            .h(px(42.0))
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .bg(rgb(p.surface))
            .border_1()
            .border_color(rgb(p.border_strong))
            .rounded(px(10.0))
            .shadow_lg()
            .child(icon(IconName::Search, 16.0, p.accent))
            .child(
                div()
                    .flex_1()
                    // Arrow keys move through the sidebar results instead of
                    // the single-line input's caret.
                    .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        match event.keystroke.key.as_str() {
                            "up" => {
                                this.search_step(-1, cx);
                                cx.stop_propagation();
                            }
                            "down" => {
                                this.search_step(1, cx);
                                cx.stop_propagation();
                            }
                            _ => {}
                        }
                    }))
                    .child(
                        Input::new(input)
                            .appearance(false)
                            .focus_bordered(false)
                            .text_size(px(12.5)),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(p.text_tertiary))
                    .child(if query_empty {
                        String::new()
                    } else {
                        format!("{count} 处")
                    }),
            )
            .child(
                div()
                    .id("search-prev")
                    .size(px(24.0))
                    .rounded(px(6.0))
                    .flex()
                    .justify_center()
                    .items_center()
                    .cursor_pointer()
                    .child(icon(IconName::ChevronUp, 14.0, p.text_secondary))
                    .on_click(cx.listener(|this, _, _, cx| this.search_step(-1, cx))),
            )
            .child(
                div()
                    .id("search-next")
                    .size(px(24.0))
                    .rounded(px(6.0))
                    .flex()
                    .justify_center()
                    .items_center()
                    .cursor_pointer()
                    .child(icon(IconName::ChevronDown, 14.0, p.text_secondary))
                    .on_click(cx.listener(|this, _, _, cx| this.search_step(1, cx))),
            )
            .child(
                div()
                    .id("search-close")
                    .size(px(24.0))
                    .rounded(px(6.0))
                    .flex()
                    .justify_center()
                    .items_center()
                    .cursor_pointer()
                    .child(icon(IconName::X, 13.0, p.text_tertiary))
                    .on_click(cx.listener(|this, _, window, cx| this.close_search(window, cx))),
            );

        div()
            .absolute()
            .top(px(12.0))
            .left(px(0.0))
            .right(px(0.0))
            .flex()
            .justify_center()
            .child(div().w(px(520.0)).flex().flex_col().gap_2().child(bar))
            .into_any_element()
    }
}
