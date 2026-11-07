use crate::*;

impl ReaderApp {
    pub(crate) fn info_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.info_dialog_open || !self.active.is_open() {
            return div().into_any_element();
        }
        let p = self.palette();
        let doc = self.active.doc.as_ref().expect("checked above");
        let info = &doc.ofd.info;
        let signed = doc.has_signature_material();
        let size = self
            .active
            .page_sizes
            .get(self.page)
            .map(|size| format!("{:.0} × {:.0} mm", size.width, size.height))
            .unwrap_or_else(|| "—".to_string());
        let path = self
            .active
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "—".to_string());
        let rows = [
            ("标题", info.title.clone().unwrap_or_else(|| "—".into())),
            ("文件", self.active.file_name.clone()),
            (
                "格式",
                format!("OFD {} / {}", doc.ofd.version, doc.ofd.doc_type),
            ),
            ("文档 ID", info.doc_id.clone().unwrap_or_else(|| "—".into())),
            (
                "页面",
                format!("{} 页 · 当前 {}", self.active.page_count(), size),
            ),
            ("大小", human_size(self.active.file_size)),
            ("作者", info.author.clone().unwrap_or_else(|| "—".into())),
            ("创建者", info.creator.clone().unwrap_or_else(|| "—".into())),
            (
                "创建日期",
                info.creation_date
                    .clone()
                    .filter(|date| !date.is_empty())
                    .unwrap_or_else(|| "—".into()),
            ),
            (
                "修改日期",
                info.mod_date
                    .clone()
                    .filter(|date| !date.is_empty())
                    .unwrap_or_else(|| "—".into()),
            ),
            (
                "签名",
                if signed {
                    "含签名（未验证）".to_string()
                } else {
                    "无签名".to_string()
                },
            ),
        ];

        div()
            .id("info-dialog-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_center()
            .bg(rgba(0x0E111680))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.info_dialog_open = false;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id("info-dialog")
                    .w(px(470.0))
                    .max_w(px(560.0))
                    .max_h(px(560.0))
                    .overflow_y_scrollbar()
                    .rounded(px(12.0))
                    .bg(rgb(p.surface))
                    .border_1()
                    .border_color(rgb(p.border))
                    .shadow_lg()
                    .text_size(px(11.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .child(
                        div()
                            .px_5()
                            .pt_4()
                            .pb_3()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(p.text))
                                    .child("文档信息"),
                            )
                            .child(
                                div()
                                    .id("info-dialog-close")
                                    .size(px(22.0))
                                    .rounded(px(6.0))
                                    .flex()
                                    .justify_center()
                                    .items_center()
                                    .cursor_pointer()
                                    .child(icon(IconName::X, 13.0, p.text_tertiary))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.info_dialog_open = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(div().mx_5().h(px(1.0)).bg(rgb(p.border)))
                    .child(
                        div()
                            .px_5()
                            .py_3()
                            .flex()
                            .flex_col()
                            .gap_2p5()
                            .children(rows.iter().map(|(label, value)| info_row(label, value, p))),
                    )
                    .child(
                        div()
                            .mx_5()
                            .mb_5()
                            .mt_1()
                            .pt_3()
                            .border_t_1()
                            .border_color(rgb(p.border))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(rgb(p.text_tertiary))
                                    .child("文件路径"),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .text_size(px(10.5))
                                    .text_color(rgb(p.text_secondary))
                                    .child(path),
                            ),
                    ),
            )
            .into_any_element()
    }
}
