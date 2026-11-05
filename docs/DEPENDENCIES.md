# 依赖与特性精简

## 当前配置

| 位置 | 调整 | 保留能力 |
| --- | --- | --- |
| `ofd-core` | 移除直接 `usvg` 依赖，改用 `resvg::usvg` 重导出 | 与 resvg 配套的 SVG 解析器，版本不变 |
| `resvg 0.49` | 禁用默认特性，仅启用 `text`、`system-fonts`、`memmap-fonts`、`raster-images` | 文本、系统及内嵌字体、字体映射、图片；不启用 SVGZ，因为 SVG 由应用在内存中生成 |
| `base64 0.23` | 显式启用 `std`，不主动启用默认 SIMD 特性 | 内嵌字体和图片的 Base64 编码；注意 usvg 的 writer 仍会合并启用其默认特性，不能宣称全局关闭 SIMD |
| `ofd-cli` / `clap` | 禁用默认特性，启用 `derive`、`std`、`help`、`usage`、`error-context` | 命令行解析、帮助、用法、版本号、错误上下文；舍弃彩色输出和模糊拼写建议 |
| `ofd-app` / `gpui-kit` | 显式只启用 `component`、`assets` | 当前界面与图标；这两项原本就是默认项，不宣称因此减少包数 |
| `ofd-app` / `image` | 维持 `default-features = false` | 仅使用像素缓冲区和 Frame API；GPUI 自己的解码器特性仍由上游启用 |
| `ofd-app` / `serde` + `serde_json` | 新增，仅 derive 与 JSON 读写 | 设置持久化；不引入网络、YAML/TOML 解析或额外运行时 |
| 图标资源 | `ReaderIcons` + 组件 `Assets` 代替 `AllAssets` | 24 个应用图标与组件默认图标合并去重，共 115 个 SVG；原全量目录为 1830 个 SVG |

`zip` 原本已关闭默认特性、只启用 Deflate（zlib-rs），继续保留。XML 解析的位置信息用于错误诊断，不为微小收益移除。`tempfile` 只用于 core 测试，保持其默认随机命名方式。

## 不扩大补丁范围

- Linux 保留 Wayland / X11，Windows/macOS 后端不变。
- GPUI 及组件存在不支持按应用功能关闭的依赖，例如富文本、图片格式、网络接口和编辑器基础设施。仅设置应用的 `default-features = false` 不能关闭上游已显式启用的特性；本轮不为此 vendor 更多框架代码。
- `usvg 0.49` 的 `text` 会启用 `writer`，不能独立删除 writer。
- 框架与 core 的不同 resvg/usvg 版本暂不强行合并，以免通过降级渲染器改变字体排版和图像输出。
- 未修改开发模式的 `opt-level` 或 release 编译参数。

## 检查结果

- 全平台依赖解析图从 **824** 个包减至 **815** 个包（不等于单个平台实际编译包数）。
- 移除的包为 `anstream`、`anstyle-parse`、`anstyle-query`、`anstyle-wincon`、`colorchoice`、`is_terminal_polyfill`、`once_cell_polyfill`、`strsim`、`utf8parse`。
- 图标测试覆盖选用图标加载及视图 `IconName` 与选用资源的对应关系，防止精简后出现空白图标。
- 工作区 fmt、Clippy、测试通过；CLI 主帮助及 render 子命令帮助检查通过。

可用 `cargo tree -e features -p ofd-core` 和 `cargo tree -e features -i image` 复核 Cargo 的特性合并结果。
