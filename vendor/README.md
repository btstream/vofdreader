# 本地依赖补丁

## gpui-component 0.9.0

- 来源：crates.io 发布包 `gpui-component 0.9.0`。
- 上游仓库：<https://github.com/longbridge/gpui-kit>。
- 发布包记录的 Git 提交：`7591a35eced761e3613c01e71ff04c10129dae1a`，仓库路径 `crates/component`。
- 原许可证 Apache-2.0 保留在 `gpui-component/LICENSE`；该依赖不改为本项目的 GPL 许可证。
- 保留发布包源文件及清单，省略 Cargo 缓存元数据与发布包的 Cargo.lock。
- 本地功能修改仅为 `gpui-component/src/window_border.rs`，用途和 Zed 参考来源见 `docs/WINDOW_DECORATIONS.md`。

升级 GPUI Kit 时应优先检查上游是否已提供圆角窗口装饰和透明 Root 配置，避免继续维护不必要的补丁。
