# 主窗口圆角与阴影

## 参考的 Zed 实现

固定上游提交：`d36a70661471ec64dce230798121d554930d5c60`。

- [workspace::client_side_decorations](https://github.com/zed-industries/zed/blob/d36a70661471ec64dce230798121d554930d5c60/crates/workspace/src/workspace.rs#L12230)：实际主窗口装饰，不是 `window_shadow` 示例。
- [主题装饰常量与逐角处理](https://github.com/zed-industries/zed/blob/d36a70661471ec64dce230798121d554930d5c60/crates/theme/src/theme.rs#L52)。

Zed 使用 10px 圆角、10px 阴影留白、1px 边框；居中黑色 40% 阴影，blur 为留白的一半。本应用将外圆角改为 **5px**，其余采用相同参数。保留组件现有的边缘/角落缩放命中区和光标逻辑。

## 为什么之前看起来仍是直角

GPUI Kit 的 `open_window` 会创建 `gpui_base::Root`。组件层 `WindowState` 插件又为 Root 设置不透明背景，并自动包裹 `WindowBorder`。应用内部再画圆角与阴影，实际形成了两套装饰，外层仍是白色直角。

GPUI 0.3.11 的 `ContentMask` 只支持矩形，因此 `.rounded(...).overflow_hidden()` 不能裁掉子元素的直角背景。

现在只由组件的 `WindowBorder` 绘制外框；Root 实例设置透明背景，不改变组件主题的背景色。应用内容、标题栏、状态栏分别绘制 4px 内圆角，与 1px 边框外侧的 5px 圆角同心。以后若给贴近四角的子元素增加背景，也必须遵循内圆角。

## 本地依赖补丁

`vendor/gpui-component` 来自 crates.io 的 `gpui-component 0.9.0`（Apache-2.0），通过 workspace 的 `[patch.crates-io]` 使用。功能修改仅在 `src/window_border.rs`：

- 圆角从 0 改为 5px，四个角根据相邻边的平铺状态分别设置；同步平台 corner radii。
- Linux 阴影留白改为 10px，使用 Zed 的居中单层阴影。
- 浮动窗口绘制阴影；平铺时不绘制阴影，贴边取消留白和边框。
- 全屏不绘制客户端装饰，清除平台 inset 和 corner radii。
- 保留原来的缩放命中区实现，避免将内容区域误识别为缩放边缘。

平台 inset 在最大化/平铺期间仍保持固定值，仅取消对应的视觉留白，以遵循组件和 Wayland 后端的尺寸计算约定。

## 平台范围

按用户后续调整，**Linux 暂不禁用 X11**：GPUI Kit 0.9.0 固定启用 Wayland 和 X11，本次不修改后端依赖或加入 Wayland-only 启动限制。Windows/macOS 的原生外框仍受平台实现控制，不宣称已实机验证。

## 验证

- `cargo fmt --all -- --check`、工作区 Clippy（`-D warnings`）、工作区测试通过。
- 新增 3 项应用单测：浮动窗口四角、单边平铺、最大化/全屏/服务端装饰状态的内圆角。
- Linux Wayland 实机启动空白窗口并检查 RGBA 截图：窗口外侧透明，阴影区域为半透明黑色，不再有外围白色矩形。
- Wayland 实机验证最大化：内容贴边、四角直角、无阴影和 inset，缩放比例随视口更新；验证全屏：不绘制客户端装饰，标题栏仍可访问。
- 关闭/还原、分屏快速平铺和鼠标拖动缩放尚未实机验证（KWin 的 Wayland 脚本接口不允许修改窗口状态，也无法注入指针事件）；这些路径复用组件原有缩放命中区，只随阴影留白从 20px 调整为 10px。
- Windows/macOS 未实机验证。
