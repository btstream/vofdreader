# 渲染调度架构

## 目标边界

```text
视图与交互
    ↓ 视口、页码、缩放、可见区域
调度策略（纯逻辑，可无窗口测试）
    ↓ 任务计划
任务簿记（任务身份、代际、槽位）
    ↓ 任务票
GPUI 执行适配
    ↓ 后台光栅化、图片转换
结果接纳（完成、缓存、显示）
    ↓
资源生命周期（登记、对账、释放）
```

## 当前分层现状

| 层 | 位置 | 状态 |
|---|---|---|
| 结果接纳规则 | `app/render_policy.rs` | 已抽出，可测试 |
| 预取排序与预算 | `app/render_policy.rs` | 已抽出，可测试 |
| 缓存数据与淘汰 | `app/raster_state.rs` | 已抽出，依赖 `RenderImage` |
| 任务身份与结算 | 分散在 `raster_state` + 执行闭包 | **未统一** |
| 任务准入 | `rendering/{pages,tiles,continuous}.rs` | **与执行混在一起** |
| GPUI 执行适配 | 各 `cx.spawn` 闭包 | **含业务判断** |
| 资源登记与释放 | `gpu.rs` | 已集中，调用点分散 |

`gpu::Resources::register()` 只是图片所有权登记，不是 GPU 上传。
`painted()` 统计绘制提交，不是硬件时序。
`background_done()` 只记录诊断计数。

## 阶段 0 核验结论（2026-11-07）

对现有任务簿记逐项核验，确认以下问题**真实存在**。

### 问题 1：分块任务按 Key 无条件删除（已修复）

`rendering/tiles.rs` 完成回调第一行：

```rust
this.raster.tile_jobs.remove(&key);
```

位于文档代际和视图代际校验**之前**。若同 Key 的新任务已在旧任务返回前启动，
旧任务返回会删除新任务的槽位记录，导致：

- `tile_busy()` 误报空闲，可能超出并发上限
- 在途字节统计偏小，预算判断失真

### 问题 2：连续页任务跨代际删除 pending（已修复）

`rendering/pages.rs` 的 `request_cached_page` 回调：

```rust
if this.active.generation != generation {
    return;                     // 文档已切换：不删除，槽位滞留到回调结束
}
this.raster.continuous_pending.remove(&(page, dpi));
if this.raster.view_generation != view_generation {
    cx.notify();
    return;
}
```

文档代际不同时提前返回，`(page, dpi)` 会一直留在 `continuous_pending`，
阻塞同页后续请求（`render_ready` 计算的是 pending 数量）。

视图代际不同时先删除再返回，但同键的**新视图**任务可能已经启动，
删除会误伤新任务。

### 问题 3：DPI 防抖计时器（复核后撤销）

最初判断旧计时器会错误清除 `dpi_pending`，导致防抖失效。

复核结论：**该问题不存在。**

- `schedule_dpi_render` 在 `dpi_pending` 为真时直接返回，同一时刻最多一个计时器。
- 该计时器必然观察到自己的代际，即“最新代际”。
- 因此 `dpi_pending = false` 与代际判断的顺序不影响正确性。

保持现有顺序不变。该次修改已回退。

### 问题 4：缩略图重新生成没有批次身份（已修复）

`spawn_thumbnails` 只检查文档代际。用户在设置中“清理缩略图缓存”后：

1. `thumbs` 被清空、`thumbs_started = true`、启动新批次
2. 旧批次仍在运行，其回调只校验文档代际
3. 旧批次继续写入 `thumbs`，与新批次交错

新批次从当前页向外排序，旧批次从旧页向外排序，同一页可能被写入两次，
旧图可能覆盖新图。功能上图片相同，但会产生重复工作与额外登记。

### 问题 5：被拒绝的分块仍登记图片（已修复）

```rust
this.gpu.borrow_mut().register(image.clone());
this.raster.cache_tile(...);   // 可能因预算拒绝
```

预算拒绝时 `cache_tile` 返回 `false`，但图片已登记。当前依赖下一帧
`reconcile` 清理，属于短暂驻留。不是泄漏，但增加了峰值内存与统计噪声。

### 已确认无问题的部分

- 文档切换后同步写入的旧结果被 `active.generation` 正确拒绝
- 缓存淘汰保护当前页与刚插入项
- `reconcile` 释放失败会保留条目以便重试
- 图片释放通过 `App::drop_image` 可跨窗口完成，不强制要求 `Window`

## 阶段划分

| 阶段 | 内容 | 状态 |
|---|---|---|
| 0 | 行为基线与风险核验 | 完成 |
| 1 | 统一任务身份与结算 | 完成 |
| 2 | 抽离任务准入与请求规划 | 完成 |
| 3 | 收敛 GPUI 异步执行适配 | 完成 |
| 4 | 统一结果接纳 | 完成 |
| 5 | 整理资源登记与释放边界 | 完成 |
| 6 | 回归、文档与提交 | 完成 |

## 基线

`resources/pdf-match/1.ofd` 第 0 页，96 DPI PNG：

```
sha256 97d7b23796b622917fc03e623ea8466476f9c96b901c85e1bef3dc880f490674
```

`resources/y.ofd` 第 0 页：

```
PNG (96 DPI) efa44f8d43097b7ec235c48805ef9cc83745293b807abc8c620945270c420f40
SVG          2497d434e0d7e1f16dfb3dbfa174e72546daf62e99fe560f212f5a1c4df1ed2f
```

阶段 1 之后必须与上述值一致。

## 阶段 1 完成情况

新增 `app/task_state.rs`：

- `RenderEpoch`：文档代际 + 视图代际
- `TaskId` / `TaskTarget` / `TaskTicket` / `TaskInfo`
- `InFlightTasks`：按任务编号结算，按目标去重

`RasterState` 用 `tasks: InFlightTasks` 取代了：

```text
tile_jobs: HashMap<Key, usize>
continuous_pending: HashSet<(page, dpi)>
```

由此修复：

| 问题 | 修复方式 |
|---|---|
| 分块按 Key 无条件删除 | 完成时先 `finish(ticket)`，再判断代际 |
| 连续页跨代际遗留 pending | 无论代际是否匹配都先结算自己的票 |
| 被拒绝的分块仍登记图片 | `cache_tile` 返回是否接纳，仅接纳后 `register` |
| 缩略图旧批次覆盖新批次 | `ActiveDocument::thumbnail_batch` |

`raster_jobs()`、`tile_busy()`、加载指示器和缩略图等待条件全部改为查询
`InFlightTasks`，不再读取分散的集合。

### 新增测试

`task_state.rs` 覆盖：

- 旧票结算不影响同目标的新任务
- 按目标去重与按类型统计字节
- 重复结算只释放一次
- 代际比较同时覆盖文档与视图

### 回归结果

- `cargo fmt`、Clippy `-D warnings`、全工作区测试通过
- `resources/pdf-match/1.ofd` PNG 与基线一致
- `resources/y.ofd` PNG/SVG 与基线一致

## 阶段 2 完成情况

`render_policy` 改为目录模块：

```text
render_policy/
├── mod.rs       请求标识、画面替换、预取排序与预算
└── admission.rs 任务准入
```

`admission.rs` 全部是纯函数，不引用 GPUI：

| 函数 | 取代的内联判断 |
|---|---|
| `plan_single_page` | 单页：命中缓存、等待手势或任务、合并同请求、开始渲染 |
| `may_request_cached_page` | 连续页/预取的六项提前返回 |
| `may_schedule_prefetch` | 预取总闸门（文档、渲染器、总览、滚动、防抖、加载） |
| `may_start_tile` | 分块是否可启动 |
| `may_keep_tile` | 分块结果是否仍在预算内 |
| `may_render_thumbnail` | 缩略图是否让路给前台 |
| `preview_density` | 是否用缓存低清图临时占位 |
| `may_retain_cached_page` | 结果是否仍在可见/预取范围内 |
| `result_is_usable` | 代际是否仍匹配 |

渲染模块现在是"取值 → 调策略 → 执行或返回"，不再各自维护准入条件。
新增 11 个准入测试，覆盖每个提前返回分支。

### 行为一致性

单页路径的判断顺序保持与原实现相同：先精确/更清晰缓存，再低清占位，
再判断手势与在途任务，最后合并相同请求。策略函数只承载判断，不改变顺序。

### 回归结果

- `cargo fmt`、Clippy `-D warnings`、全工作区测试通过（app 98 项）
- `resources/pdf-match/1.ofd` PNG 与基线一致
- `resources/y.ofd` PNG/SVG 与基线一致

## 阶段 3 完成情况

新增 `app/rendering/worker.rs`：

- `image_from_raster`：校验缓冲区、旋转、包装成 `RenderImage`
- `page_image_from_raster`：整页版本，尺寸由渲染器决定

四个闭包（单页、连续页、分块、缩略图）不再各自复制旋转与校验代码。
分块保留预期的旋转后尺寸校验，与原有"尺寸不一致"错误一致。

新增 3 个测试：旋转后像素总数不变、期望尺寸不符被拒绝、缓冲区长度不符被拒绝。

## 阶段 4 完成情况

新增 `render_policy/acceptance.rs`，把完成结果拆成三个独立判断：

```text
Currency   文档/视图代际是否仍匹配
Retention  是否值得进入缓存
Display    是否可以替换屏幕
```

对应 `accept_page_frame` / `accept_tile` / `accept_thumbnail`。

调用点变化：

| 位置 | 原先 | 现在 |
|---|---|---|
| 单页成功 | 代际 + latest + DPI 三重 if | `acceptance.replaces_screen()` |
| 单页失败 | 单独判断代际 | 同一 `acceptance.is_current()` |
| 连续页 | 只判断代际 | 同一接纳结构，`latest_request: false` |
| 分块成功 | `finish` + 代际 + wanted + 预算 | `accept_tile` 汇总判断 |
| 缩略图 | 批次判断内联 | `accept_thumbnail` |

新增 12 个接纳测试，覆盖过期、被取代、离屏、超出预取范围、
空白页接受低清、低清不覆盖高清、分块预算和旧缩略图批次。

原先分散在 `mod.rs` 与 `admission.rs` 的 `replaces_visible`、
`frame_is_current`、`may_retain_cached_page`、`result_is_usable`
及其测试已由接纳结构取代并删除。

### 回归结果

- `cargo fmt`、Clippy `-D warnings`、全工作区测试通过（app 104 项）
- `resources/pdf-match/1.ofd` PNG 与基线一致
- `resources/y.ofd` PNG/SVG 与基线一致

## 阶段 5 完成情况

### 文档关闭时的窗口上下文

`clear_active_document` 原先调用 `cx.drop_image(image, None)`。GPUI 在
窗口更新期间会暂时把该窗口移出 `App.windows`，只传 `None` 时当前窗口的
atlas 条目要等下一次对账才释放。

现在 `clear_active_document` 接收 `Option<&mut Window>`，打开/切换标签时
传入当前窗口；`close_tab` 也传入窗口。退出路径（`on_release`）没有窗口，
仍然用 `None` 覆盖其余窗口，这是正确用法而非遗漏。

### 登记与接纳

分块结果改为"缓存接纳后再登记"，配不上预算的图片不再进入
`gpu::Resources`，因此不需要等下一帧对账清理。

### 语义澄清

- `register` 只登记所有权，不上传。
- `painted` 统计首次绘制提交。
- `background_done` 只记诊断计数。
- `reconcile` 与 `release_all` 分别用于持续对账和整体清理。

注释已按上述语义修正，不再暗示 `register` 会触发 GPU 工作。

## 阶段 6 完成情况

- `cargo fmt --all -- --check` 通过
- `cargo clippy --workspace --all-targets -- -D warnings` 通过
- `cargo test --workspace` 通过（app 104 项）
- 三个渲染产物与阶段 0 基线逐字节一致

### 行为基线

| 样本 | 产物 | sha256 |
|---|---|---|
| `pdf-match/1.ofd` p0 | PNG 96 DPI | `97d7b237…f490674` |
| `y.ofd` p0 | PNG 96 DPI | `efa44f8d…c420f40` |
| `y.ofd` p0 | SVG | `2497d434…df1ed2f` |

### 未覆盖的验收

图形界面手工验收（快速翻页、连续滚动、高倍缩放、旋转、切换采样、
渲染中切换标签、缩略图重建、打开失败、不同渲染线程数）需要实机窗口，
本轮没有执行，不能以单元测试替代。

## 位图内存预算

见 [`MEMORY_BUDGET.md`](MEMORY_BUDGET.md)。要点：

- 预算控制整页/分块位图与当前画面，不控制系统总内存。
- 任务启动前预留像素；失效任务保留预留到真正返回，待替换单页结果保留到窗口帧接纳或丢弃。
- 位图按 `ImageId` 去重计费。
- 预算不足时不启动任务，而不是渲染后静默丢弃。
- 页面按 24 DPI 档降级；只有最低档也放不下时才显示错误。
- 调小预算会有界淘汰预取分块与离屏页面；不符合新密度计划的缓存会被重新规划。
- 驻留计费覆盖资源登记表的延迟持有，缓存拒绝的结果不得登记或显示。
- 单页替换在窗口帧中先释放旧登记资源，再接纳新画面；额度比例不是实际内存峰值保证。
- 滚动预取将整个可容纳计划纳入淘汰保护；缓冲行复用图片但不触碰 LRU，避免相邻页反复渲染。
- 缩略图、解码原图 LRU 与解析页估算保留额度随设置缩放；解析页计数也受低预算约束。
- Linux/glibc 默认限制 malloc arena 为 2，尊重启动环境覆盖，不做周期性 trim。
- 显式的 `OFD_SCROLL_BENCH_STEPS` 工作负载用于同文件滚动内存回归，详见 `SCROLL_MEMORY_CHECK.md`。

## 后续工作

本轮完成了计划中的阶段 0–6。仍可继续的低优先级项：

- `render/renderer.rs` 仍承担字体注册、页面缓存与后端分派
- `settings_view.rs`、`app/view/chrome.rs` 体量偏大
- 若出现第二个前端，再评估拆分 `ofd-reader-engine` crate
