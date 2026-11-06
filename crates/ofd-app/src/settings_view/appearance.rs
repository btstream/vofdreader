use crate::settings::*;
use crate::*;

impl ReaderApp {
    pub(crate) fn appearance_page(&self, cx: &mut Context<Self>) -> AnyElement {
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
}
