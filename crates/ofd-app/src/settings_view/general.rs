use crate::settings::*;
use crate::*;

impl ReaderApp {
    pub(crate) fn general_page(&self, cx: &mut Context<Self>) -> AnyElement {
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
}
