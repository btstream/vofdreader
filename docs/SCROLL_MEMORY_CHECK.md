# intro-数科.ofd 滚动内存回归

## 条件

- 文件：`resources/intro-数科.ofd`，42 页、320×240 mm，压缩体积约 7.1 MiB。
- Linux/glibc，release 构建，独立 `XDG_CONFIG_HOME`，未修改用户实际配置。
- 设置：64 MiB、高清、页面项数 12、解析页设置 6、预取 2 页、分块环 2、8 任务槽、nearest。
- 先恢复到第 36 页；准备好后等待 3 秒，回到首页，向下/向上各滚动 30 次，
  每次 1000px、间隔 200ms。停止后等待 5 秒。
- 修复前临时检测调用与修复后基准均使用 `ReaderApp::scroll_continuous`。

## 一次受控样本

| 指标 | 修复前 | 仅限制 arena 为 2 | 完整修复后 |
|---|---:|---:|---:|
| 峰值 RSS | 711.4 MiB | 339.4 MiB | 268.1 MiB |
| 停止并等待后 RSS | 711.4 MiB | 316.1 MiB | 221.0 MiB |
| 驻留输出账本 | 38.5 MiB | 38.5 MiB | 38.5 MiB |
| 缩略图输出 | 44.0 MiB | 44.0 MiB | 7.6 MiB |

表中均为显式 trim 之前的数据；正式修复与回归基准不调用 `malloc_trim`。
完整修复后的滚动阶段约 12.0 秒、67 个后台任务，停止后 5 秒内没有新增后台任务，
最终在途任务与预留均为 0。页面输出缓存保留量不变，不是通过清空画面实现下降。

修复前跟踪发现固定视口附近的 0、1 页在 168 DPI 下交替被重渲染：
可容纳预取计划没有整体保护，连续缓冲行的 LRU 访问又挤占额度。现在计划整体受保护，
不在计划内的迟到结果被丢弃，后台结果也不能突破页面缓存子额度。

这只是同机同工作负载下的样本，不是所有文档、分配器或驱动下的上限承诺。
源图解码、字体、框架与驱动仍有独立开销；RSS 不应与滑块数值直接相等。

## 复测方法

1. `cargo build --release -p ofd-app`。
2. 准备独立配置，例如 `/tmp/opencode/scroll-check/vofdreader/settings.json`，
   设置上面的性能参数、连续模式和恢复页索引 35。不要复制其他私人阅读记录。
3. 保持窗口可见，运行：

```sh
XDG_CONFIG_HOME=/tmp/opencode/scroll-check \
OFD_SCROLL_BENCH_STEPS=30 OFD_GPU_DIAGNOSTICS=1 \
target/release/ofd-app 'resources/intro-数科.ofd'
```

4. 查看 `[scroll-bench]` 的停止/稳定结果及 Linux `VmRSS`、`VmHWM`。
   基准会改变阅读位置；默认不设置该环境变量时不运行任何脚本滚动。
5. 对照 allocator 策略时，可显式设置 `MALLOC_ARENA_MAX`；程序尊重该覆盖。

系统上若已有阅读器进程，单实例转发可能将文件送入已有窗口，此时不是有效的隔离复测。
