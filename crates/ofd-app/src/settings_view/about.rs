use crate::*;

impl ReaderApp {
    pub(crate) fn about_page(&self, _cx: &mut Context<Self>) -> AnyElement {
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
}
