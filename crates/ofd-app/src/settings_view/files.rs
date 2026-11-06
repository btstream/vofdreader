use crate::settings::*;
use crate::*;

impl ReaderApp {
    pub(crate) fn files_page(&self, cx: &mut Context<Self>) -> AnyElement {
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
}
