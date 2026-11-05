# Linux 桌面集成

本目录提供：

- `vofdreader.desktop`：应用菜单入口及 OFD 文件打开能力声明。
- `vofdreader-mime.xml`：注册 `.ofd` 扩展名，主类型为 `application/ofd`，兼容别名 `application/vnd.ofd`。
- `mimeapps.list`：默认打开关系示例，**不要直接覆盖已有配置文件**。

图标复用仓库中的 `assets/vofdreader.svg`（应用）和 `assets/application-ofd.svg`（文件类型）。启动命令仍为 `ofd-app`，一次接收一个本地文件，因此使用 `%f`。

## 用户级安装

在仓库根目录运行以下命令，无需 root。需要 `shared-mime-info`、`desktop-file-utils` 和 `xdg-utils` 提供的工具。

```sh
cargo build --release -p ofd-app

data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
install -Dm755 target/release/ofd-app "$HOME/.local/bin/ofd-app"
install -Dm644 packaging/linux/vofdreader.desktop "$data_home/applications/vofdreader.desktop"
install -Dm644 packaging/linux/vofdreader-mime.xml "$data_home/mime/packages/vofdreader.xml"
install -Dm644 assets/vofdreader.svg "$data_home/icons/hicolor/scalable/apps/vofdreader.svg"
install -Dm644 assets/application-ofd.svg "$data_home/icons/hicolor/scalable/mimetypes/vofdreader-ofd.svg"

update-mime-database "$data_home/mime"
update-desktop-database "$data_home/applications"
touch "$data_home/icons/hicolor"
```

确保桌面会话的 `PATH` 包含 `~/.local/bin`；否则将安装后的 desktop 文件中的 `Exec` 和 `TryExec` 改为可执行文件的绝对路径（desktop 文件不展开 `~` 或 `$HOME`）。图标未立即刷新时，可注销后重新登录。

### 文件类型图标名

MIME 定义中的图标名是 `vofdreader-ofd`，没有使用默认推导出的 `application-ofd`。原因是 KDE 在查找 `application-*` 图标时，如果当前主题（例如 Colloid）里没有这个名字，会先使用该主题自带的 `application-x-generic`，不会再继续到 hicolor 里查找。这样一来，安装到 hicolor 的 `application-ofd.svg` 永远不会显示。改用不带 `application-` 前缀的名字（WPS 的 `wps-office-ofd` 也是这种做法）后，可以正常回退到 hicolor 并找到图标。

已经打开的 Dolphin 需要重开后才会显示新图标；桌面可执行 `systemctl --user restart plasma-plasmashell`。

## 设置默认打开方式（可选）

安装后应用会出现在“打开方式”列表中；下面的命令才会将其设为默认程序：

```sh
xdg-mime default vofdreader.desktop application/ofd
xdg-mime default vofdreader.desktop application/vnd.ofd
```

也可把 `mimeapps.list` 中的条目合并到 `${XDG_CONFIG_HOME:-$HOME/.config}/mimeapps.list` 的对应分组。保留其他类型和应用的已有条目；不需要设置默认时，只合并 `[Added Associations]`。

## 验证

```sh
desktop-file-validate packaging/linux/vofdreader.desktop
xdg-mime query filetype resources/y.ofd
xdg-mime query default application/ofd
xdg-open resources/y.ofd
```

预期文件类型为 `application/ofd`，默认程序为 `vofdreader.desktop`。桌面环境已有的 MIME 定义、用户配置及图标主题可能影响识别和显示。
