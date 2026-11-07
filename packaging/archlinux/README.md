# Arch Linux 打包

`PKGBUILD` 构建 `vofdreader-git`，从 GitHub `main` 获取最新源码，版本号自动生成为 `0.3.0.r<提交数>.g<短哈希>`。目前声明支持 `x86_64`，其他架构未经打包验证。

## 构建与安装

先确保本仓库新增的 `packaging/linux/` 文件和两个 SVG 图标已提交并推送到远端；PKGBUILD 默认读取远端源码，**不读取工作目录中尚未推送的修改**。

```sh
sudo pacman -S --needed base-devel git
cd packaging/archlinux
makepkg -si
```

以普通用户运行 `makepkg`，不要使用 `sudo makepkg`。首次构建需要网络下载 Rust 依赖，可能耗费较多时间和磁盘空间。默认执行工作区测试，可用 `makepkg --nocheck -si` 跳过。

PKGBUILD 设置 `options=('!lto')`。Arch 默认的 `lto` 选项会给 `CFLAGS` 追加 `-flto=auto`，把 `wayland-backend` 的 C shim 编译成 GCC LTO 对象；rustc 在 x86_64 Linux 上默认使用 rust-lld 链接（Rust 1.90 起），而 lld 无法读取 GCC LTO 字节码，因此会报 `wl_log_trampoline_to_rust_client` 未定义。（GNU ld、mold 可以通过 GCC 插件读取这类对象，但不能依赖构建环境恰好用了它们。）项目自身的 `[profile.release]` 已启用 thin LTO，因此这里只关闭 makepkg 注入到 C 编译的 LTO。

若需验证尚未推送的本地源码，可在 **PKGBUILD 的副本** 中将 `source` 改为本地 Git 仓库的 `git+file:///绝对路径`；本地修改也必须先提交才能被 Git 源码机制获取。

## CI 自动打包

`.github/workflows/ci.yml` 中的 `archlinux` 任务在 `archlinux:base-devel` 容器内用本 PKGBUILD 构建软件包。它只在推送到 `main` 或手动触发（workflow_dispatch）时运行，并且要等 `test` 任务通过。任务会把 `source` 改为当前检出的提交，所以构建的就是本次 CI 所测的代码；工作区测试已在 `test` 任务中执行，这里使用 `--nocheck`。生成的 `.pkg.tar.zst` 作为构件 `vofdreader-archlinux-x86_64` 上传，可在 Actions 运行页面下载，用 `sudo pacman -U` 安装。

`main` 分支打包成功后，`nightly` 任务把软件包发布到 GitHub 的滚动预发布版本 **Nightly**（tag `nightly`）：tag 移到本次构建的提交，附件替换为新的 `.pkg.tar.zst` 与 `SHA256SUMS`（先上传新文件再删除旧文件，下载链接不会中断；调试符号包不发布）。其他分支的手动触发只生成 Actions 构件，不发布。该任务需要 `contents: write` 权限，已在 workflow 中为它单独声明。

```sh
# 文件名随版本变化，请在 Releases 页面的 Nightly 中查看
curl -LO https://github.com/btstream/vofdreader/releases/download/nightly/SHA256SUMS
curl -LO https://github.com/btstream/vofdreader/releases/download/nightly/<包文件名>
sha256sum -c --ignore-missing SHA256SUMS
sudo pacman -U <包文件名>
```

## 安装内容

- `/usr/bin/ofd-app`：桌面阅读器。
- `/usr/bin/ofd-cli`：命令行工具。
- 应用菜单入口、OFD MIME 定义、应用和文件类型图标。

保留 Wayland 和 X11 支持；需要可用的 Vulkan 实现，安装与显卡匹配的驱动。中文字体建议安装 `noto-fonts-cjk`，也可使用已有的中文字体。

桌面、MIME 和图标缓存由 Arch 的系统 pacman hooks 更新，无需额外 `.install` 脚本。包不安装 `mimeapps.list`，不覆盖用户的默认关联。如需设为默认阅读器：

```sh
xdg-mime default vofdreader.desktop application/ofd
xdg-mime default vofdreader.desktop application/vnd.ofd
```

以上可选命令需要 `xdg-utils`。

此 PKGBUILD 尚未完成 Arch 干净 chroot 构建验证；发布到 AUR 前建议使用 `devtools` 进行验证，并用 `makepkg --printsrcinfo > .SRCINFO` 生成元数据。
