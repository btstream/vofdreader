# 字体渲染与核验

## 当前实现

- 资源按 `scope:id` 隔离；文档资源优先于公共资源。字形来自引用的 FontFile，不按同名字体替换内嵌字体。
- 支持 TrueType、OpenType（含 CFF）、裸 CFF；识别文件内容，不依赖扩展名。部分 `.ttf` 实际为裸 CFF，部分 `.cff` 实际为 SFNT。
- 裸 CID-keyed CFF 的稀疏 CID 按 charset 转换为 CharStrings 索引；不能直接把 CID 当作 GID。SFNT 中的 GID 不做该转换。
- 裸 CFF 应用 FontMatrix；SFNT 使用 units-per-em。TTC/OTC 按字体名称、家族及粗体/斜体属性选择成员，不再固定第 0 个 face。
- 优先使用 `CGTransform/Glyphs`，否则查询 cmap；支持部分映射、多字符合字、一字符展开及多字符/多字形簇。
- `CodeCount` / `GlyphCount` 默认值为 1；校验数量、范围、UTF-16 边界和重叠，拒绝猜测非法映射。未映射字符仍可使用内嵌 cmap。
- 字形位置与原始字符分离：`DeltaX/DeltaY` 按渲染字形流推进，合字/展开不会靠平均分配源字符来猜位置。没有显式偏移时优先使用内嵌字形 advance。
- 支持 `HScale`、`ReadDirection`、`CharDirection`（0/90/180/270），Font 级 Bold/Italic、TextObject 级 Weight/Italic，以及 Fill/Stroke/StrokeColor/LineWidth。
- 对需要的额外粗体采用轮廓描边合成，额外斜体采用倾斜变换；已具备相应样式的 SFNT face 避免重复合成。合成样式不是另一套原生字形的等价替代。
- TextCode 支持 `\XXXX` UTF-16 转义及后续节点省略 X/Y 时的坐标继承。
- 文字递归进入复合对象、注释和图案组；屏幕和 SVG 导出使用同一套路径。导出不再携带无用或错误标注格式的 `@font-face` 数据。
- 缺字只回退对应字符/字形簇，不因空格替换整段文字。原始字符仍保留在 DisplayList 供阅读器搜索和复制。

## 界面字体

界面字体与 OFD 内嵌字体相互独立：

- Linux 通过系统 `fc-match` 读取 fontconfig 的默认 `sans-serif` 家族作为 UI 主字体；再以 `sans-serif:charset=4e2d` 查询能覆盖汉字的 sans 家族，作为 GPUI 文字回退链。
- 不使用 `fontconfig` C 绑定或 `fc-match` 的 C API；仅在 Linux 启动时执行两条 `fc-match` 查询，失败时回退 GPUI 默认。
- macOS / Windows 保持 GPUI 的 `.SystemUIFont`，不查询 fontconfig。
- 中文字符使用 fontconfig 返回的回退家族；设置导航选中项使用 Medium 500，避免 600 被映射到 700 Bold 后看起来像另一套字体。

## 可复现核验

```bash
# 输出逐页实际使用的内嵌字形、回退数量及回退字符的 Unicode 编码
cargo run --release -p ofd-cli -- fonts resources/y.ofd

# 出现内嵌字形回退时以非零状态退出
./target/release/ofd-cli fonts resources/y.ofd --strict

# 全量语料字体核验
bash scripts/check-fonts.sh resources

# 全量页面渲染
./scripts/render-corpus.sh resources /tmp/opencode/corpus-font-check
```

页面字体报告在 PageRenderer 中缓存；阅读器状态栏会提示当前页的内嵌字体回退字符数量。CLI 不输出文档原文，仅输出回退字符编码。

## 本轮验证结果

- 40 份 OFD、118 页，内嵌字体引用的非空白字符 **0 回退**；文档明确使用系统字体的文字不计入内嵌回退。
- `y.ofd` 三页实际内嵌字形数量为 **1020 / 2200 / 1556**，回退均为 0。
- `zsbk.ofd` 两页为 **438 / 296**，回退均为 0。
- 数科样张中此前按系统字体显示的 PingFang/Helvetica 等字形已使用文档内嵌资源。
- 全页渲染没有新增近白页；既有近白页仍为参考 PDF 本身为空的 `pdf-match/1-1.ofd` 第 3 页。
- 测试使用自主生成的小型字体，不依赖安装的字体：覆盖 cmap/GID、原始 CFF、稀疏 CID、TTC 选面、字形簇、显式字形偏移、空格、逐字回退、样式/方向、报告缓存及导出/屏幕一致性。
- 修复了矩阵组合与 SVG 坐标约定不一致的问题；新增测试验证组合矩阵与依次变换坐标的结果一致。
- SVG 矩阵输出保留 10 位小数，避免 `1/2048` 等字体缩放被四位小数舍入；测试验证 em 缩放的序列化往返精度。

之前将部分字体判断为“损坏/空轮廓”并不准确：实际原因包括未识别裸 CFF、CID/GID 混淆，以及空格触发整段回退。本轮以实际字形提取结果重新核验，撤回该笼统判断。

## 验证边界

- 0 回退说明使用了内嵌轮廓，不等于与 WPS 逐像素一致；抗锯齿、hinting 和合成样式仍可能不同。
- 没有轮廓、没有可解释的 cmap/CGTransform，或字体数据实际损坏时，无法恢复不存在的字形；会保留回退显示及诊断，不强行绘制错误字符。
- 导出的路径不能按 SVG 文本选中；阅读器自己的原始文本索引不受影响。
- 文字裁剪区（`Clip/Area/Text`）、通用 OpenType 自动塑形（未提供字形映射的复杂文字）、彩色/位图字体不属于本轮已验证的轮廓渲染能力。
