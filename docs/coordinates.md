# 坐标与排版校准记录

> 状态：**首版已验证**（2026-11-03；真实发票样本渲染结果与版面结构一致）。
> 样本：`fixtures/private/sample-invoice.ofd`（210×140 mm 电子发票；仅本地校准，不入库）。

本文记录真实样本中观察到的坐标规律。M2 的首版渲染器已按这些规律实现，并在 210×140 mm 发票上验证；仍待确认的条目已单独标注。

---

## 1. 页面

```xml
<ofd:Area><ofd:PhysicalBox>0 0 210 140</ofd:PhysicalBox></ofd:Area>
```

- 单位：mm；原点左上角，Y 向下；
- 页面尺寸取 `PhysicalBox` 的第 3、4 个数（宽、高）；
- 发票不是 A4，UI 必须动态取尺寸。

## 2. PathObject

```xml
<ofd:PathObject Boundary="65.5 17.8 73 0.4" LineWidth="0.25">
  <ofd:AbbreviatedData>M 0 0.2 L 73 0.2</ofd:AbbreviatedData>
</ofd:PathObject>
```

已确认：路径坐标以 **Boundary 左上角为局部原点**。

- 局部 `(0, 0.2) → 页面 (65.5, 18.0)`；
- 局部 `(73, 0.2) → 页面 (138.5, 18.0)`；
- 若把 `Boundary.x/y` 再加一次 CTM 平移，会重复偏移，因此 **CTM 为恒等时只加一次 Boundary 原点**；
- 已验证：模板边框、表格线、正文对齐。

## 3. TextObject

```xml
<ofd:TextObject Boundary="20 33.8 84 10.8" Font="6952" Size="3.175">
  <ofd:TextCode X="0" Y="2.7273" DeltaX="3.175 3.175 ...">中国科学院计算技术研究所</ofd:TextCode>
</ofd:TextObject>
```

已确认：

- `Size` 单位 mm，3.175 mm ≈ 9 pt；
- 基线起点 = `Boundary.x + X`，`Boundary.y + Y`（此处 20, 36.5273）；
- `DeltaX` 为每个字符的推进量（mm），逐字累加；数字常用 1.5875（半角），汉字 3.175；
- `CGTransform` 的 `Glyphs` 用字形 ID 指定字体中的字形。本样本全部 26 个文字对象都带该映射；首版用「Unicode + 内嵌字体 cmap」渲染，结果正确，GID-only 子集仍需后续支持；
- 已验证：标题、购买方/销售方信息、金额表格、旋转水印均落在正确位置。

待确认：

- `TextCode.Y` 是否始终为基线，而非字形顶部；
- `g` 前缀的真实语义（本样本未出现）；
- 多 `TextCode` 时的换行/换列规则。

## 4. ImageObject

```xml
<ofd:ImageObject Boundary="6.5 6 20 20" CTM="20 0 0 20 0 0" ResourceID="6920"/>
```

资源图片为 300×300 PNG。已确认：

- 图像内容先被 CTM 缩放（20×20），再放到 `Boundary` 原点 (6.5, 6)；
- 即 CTM 把单位正方形映射到 20×20 mm，**不能再乘 Boundary 的宽高**；
- 若按 `Boundary` 宽高再缩放一次，会得到 400×400 mm 的错误结果；
- 已验证：二维码位于左上角，印章位于页面上方居中，尺寸正确。

## 5. 旋转文字水印（注释外观）

```xml
<ofd:Appearance Boundary="206.4583 29.5417 3.3084 22.225">
  <ofd:TextObject Boundary="0 0 3.3084 22.225"
                  CTM="0 1 -1 0 3.3084 0" Size="3.175">
    <ofd:TextCode X="0" Y="2.8638" DeltaX="3.175 ...">下载次数：1</ofd:TextCode>
  </ofd:TextObject>
</ofd:Appearance>
```

已确认：

- `CTM="0 1 -1 0 3.3084 0"` 是顺时针 90° 旋转 + 平移；
- 先应用 CTM，再叠加 Appearance 的 `Boundary` 原点（206.4583, 29.5417）；
- 若先加 Boundary 再旋转，水印会落到错误位置；
- `Alpha="127"` 需要乘以图元透明度。

首版结果：旋转水印出现在页面右侧，文字方向与位置正确；`NoRotate` 尚未实现。

## 6. 注释与资源

- 印章是 `Watermark` 注释里的 `ImageObject`（`ResourceID` 指向 DocumentRes 的 MultiMedia）；
- `ResourceID` 可能只有 ID、没有类型位：按 PageRes → DocumentRes → PublicRes 依次查找；
- `DrawParam` 可 `Relative` 继承另一个 DrawParam 的描边/填充属性；
- 颜色 `Value="246 237 225"` + `ColorSpace` 引用，需解析为 sRGB。

## 7. 首版踩坑记录

1. **矩阵组合顺序**：首版把 `translate(Boundary)` 与 `CTM` 组合时写错了 e/f 分量，导致所有带 `CTM` 的模板文字整体左移；正确关系是
   `page = translate(Boundary.x, Boundary.y) × CTM × local`，实现为 `T.compose(CTM)`。
2. **图片单位正方形**：`ImageObject` 有有效 `CTM` 时按 1×1 局部尺寸绘制，由 CTM 缩放到目标尺寸；若再乘一次 `Boundary.w/h`，图片会放大数百倍。
3. **印章白底**：发票监制章是带白底的 PNG，靠注释 `BlendMode="Darken"` 去掉白底；只渲染图片会盖住标题。SVG 中用 `mix-blend-mode:darken`，resvg 支持。
4. **注释坐标**：`Appearance` 的 `Boundary` 提供外层平移；内部对象仍按自身 `Boundary + CTM` 变换，二者不能混为一次。
5. **文字逐字定位**：`DeltaX` 按“前一字符到当前字符”的推进量使用（列表长度通常为字符数 − 1）；用单字符 `<text>` 输出可绕开字体 shaping 的差异。

## 8. 校验方法（M2）

1. 用 `DisplayList → SVG` 输出同一页；
2. 用参考阅读器（ofdrw / WPS / 福昕）导出 PNG；
3. 叠加对比，优先检查：页面边框、表格线、文字基线、印章位置、旋转水印；
4. 将确认后的规则回写到本文；首版已用 144 DPI PNG 做人工对照，后续补像素差基准。
