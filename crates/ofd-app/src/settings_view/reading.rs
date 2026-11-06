use crate::settings::*;
use crate::*;

impl ReaderApp {
    pub(crate) fn reading_page(&self, cx: &mut Context<Self>) -> AnyElement {
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
}
