# LiteText

轻量、原生、跨平台的文本编辑与 HEX 阅读工具，使用 Rust + egui 构建，无需 WebView。

- 多标签文本编辑、大文本分段排版、语法高亮、JSON 格式化与折叠。
- 查找替换、正则搜索、范围标记、空白字符处理和可配置快捷键。
- 只读 HEX 视图、字节选择与 Data Inspector。
- 全局主题、中文／英文界面、从当前文件目录打开终端。
- 未保存内容恢复、外部文件变更保护、GitHub 应用内更新。

## 下载

从本项目的 [GitHub Releases](https://github.com/Heartfilia/LiteText/releases) 下载：

| 平台 | 安装包 | 说明 |
| --- | --- | --- |
| macOS Apple Silicon（M 系列） | `LiteText-macos-arm64.dmg` | 推荐首次安装使用；也提供 ZIP 更新包 |
| Windows x64 | `LiteText-windows-x64.zip` | 解压后运行，无需安装器 |
| Linux x64 | `LiteText-linux-x64.zip` | 解压后运行，需要图形桌面环境 |

**目前不提供 macOS Intel／x64 安装包。** 带 `beta`／`rc` 的版本是测试版，请在 Releases 页面手动选择；应用内检查目前只获取最新稳定版。

每个 Release 附有 `SHA256SUMS`。可计算下载文件的 SHA-256，与同版本清单中的对应项比较：

```sh
# macOS 示例，在下载文件所在目录执行
shasum -a 256 LiteText-macos-arm64.dmg
```

哈希一致只能检查文件完整性，不等同于发布者签名；请确认下载来自本项目仓库。

## 安装与首次启动

### macOS（Apple Silicon）

1. 下载并打开 `LiteText-macos-arm64.dmg`。
2. 将 **LiteText.app** 拖到 **Applications（应用程序）** 文件夹。
3. 弹出磁盘映像，从「应用程序」打开 LiteText，不要长期在 DMG 内运行。

安装到 `/Applications` 后，也可以在终端启动：

```sh
open "/Applications/LiteText.app"
```

#### 提示无法验证开发者或应用已损坏

当前 macOS 包仅使用 ad-hoc 签名，**尚未使用 Apple Developer ID 签名和公证**，首次打开可能被 Gatekeeper 拦截。可先尝试在 Finder 中右键应用 →「打开」，或在「系统设置 → 隐私与安全性」中查看是否允许打开。

**仅当你已确认安装包来自本项目的可信 Release，并核对文件完整性后**，可以手动移除这一个应用的下载隔离属性，再启动：

```sh
xattr -dr com.apple.quarantine "/Applications/LiteText.app"
open "/Applications/LiteText.app"
```

这里的应用名是 **LiteText.app**，不是 LiteRedis.app。若安装在其他位置，请替换为实际路径（例如 `"$HOME/Applications/LiteText.app"`）。

> 此命令会递归移除该应用的隔离标记，削弱 macOS 对它的首次打开检查，并非修复损坏文件或完成公证。不要对未知应用、整个 Applications 文件夹或系统范围执行，也无需关闭系统 Gatekeeper。若文件校验失败，请重新下载，不要用该命令绕过。

如果提示没有该隔离属性，可以直接尝试打开；如果提示权限不足，请先核对安装位置和当前用户权限，不要盲目添加 `sudo`。应用不会自动执行上述命令。

### Windows x64

1. 下载 `LiteText-windows-x64.zip`。
2. 解压到固定且当前用户可写的目录，不要直接在压缩包预览中运行。
3. 双击解压后的 `litetext.exe`。

若出现 SmartScreen／安全软件提示，请先确认来源；不要为运行未知文件关闭安全软件。应用内更新需要对程序所在目录有写权限，不会自动请求管理员权限。

### Linux x64

在下载文件所在目录解压，并启动：

```sh
mkdir -p "$HOME/.local/opt/litetext"
unzip LiteText-linux-x64.zip -d "$HOME/.local/opt/litetext"
chmod +x "$HOME/.local/opt/litetext/litetext"
"$HOME/.local/opt/litetext/litetext"
```

需要可用的 X11／Wayland 图形会话及 OpenGL 运行库；纯 SSH 无桌面服务器不能直接显示窗口。若缺少动态库，请按发行版安装错误提示对应的运行依赖。当前未提供 deb／rpm／AppImage。

## 基本使用

### 打开、编辑与保存

- 点击欢迎页或右上角菜单中的「打开文件」，也可将文件拖入窗口；支持一次打开多个文件。
- 点击新建图标创建未命名文本，使用标签切换文件；标签可拖动排序，右键可关闭其他标签或打开所在文件夹。
- **手动保存才会写原文件**。新建文本首次保存时选择位置。
- 应用定期备份未保存内容，重新启动时尝试恢复工作区；恢复记录不是原文件的自动保存，也不能替代重要文件的独立备份。
- 检测到原文件在外部变更时会提示处理，避免直接覆盖。
- 点击右下角文件路径可复制完整地址，上方会显示约 1 秒的「已复制文件地址」提示。

### 编辑工具

- 右上角工具支持查找替换及文本显示选项；搜索按操作执行，可使用正则表达式。
- 「菜单 → 插件 → Format JSON」格式化当前文本，支持撤销；JSON 对象／数组可折叠。
- 「菜单 → 空白符处理」可清理行首／行尾空白、转换 TAB 与空格；状态栏可选择 TAB 输入模式。
- 状态栏语言选项支持自动识别或手动选择语法高亮；行注释使用当前语言规则。
- TXT／二进制切换按钮可查看 HEX，HEX 目前用于阅读与选择，不支持字节编辑。

### 打开终端

打开工作区后，顶部查找替换与 TXT／二进制切换之间显示终端图标：

- 已保存文件：从文件所在目录打开终端。
- 未命名文本或解除文件关联的副本：从当前用户主目录打开。
- 「系统设置 → 默认终端」可选择检测到的终端或自定义程序路径；填写程序路径，不是 shell 命令。
- macOS 的「系统默认」使用 Terminal；选择 iTerm2 也可打开。首次调用可能需要系统「自动化」权限，拒绝后可在「系统设置 → 隐私与安全性 → 自动化」中检查授权。

### 设置与快捷键

「系统设置」支持界面语言、默认终端、全局主题、快捷键和更新代理。设置即时生效，无需点击「完成」；可用右上角关闭按钮或 Esc 返回。

| 操作 | macOS | Windows／Linux |
| --- | --- | --- |
| 新建 | Cmd+N | Ctrl+N |
| 打开 | Cmd+O | Ctrl+O |
| 保存 | Cmd+S | Ctrl+S |
| 关闭当前标签 | Cmd+W | Ctrl+W |
| 查找 | Cmd+F | Ctrl+F |
| 撤销 | Cmd+Z | Ctrl+Z |
| 注释／取消注释 | Cmd+/ | Ctrl+/ |
| 系统设置 | Cmd+, | Ctrl+, |

以上为默认绑定，完整列表及修改入口在「系统设置 → 快捷键」。

## 应用内更新

1. 打开「系统设置」，点击「检查更新」。
2. 如访问 GitHub 需要代理，启用专用代理并填写例如 `http://127.0.0.1:7890` 或 `socks5h://127.0.0.1:1080`。
3. 发现更高的稳定版本后，点击「下载更新」。
4. 下载、SHA-256 和版本校验通过后，点击「安装并重启」。

代理**只用于 GitHub 更新检查和下载**，不是 GitHub 镜像 URL 前缀；关闭时直连，不使用环境代理。代理随设置保存但不加密，避免保存敏感凭据。

重启前会备份工作区；更新助手等待旧进程退出后替换程序，新版本未确认启动时尝试回退。请保持安装目录可写；macOS 不支持在 DMG 或 App Translocation 路径中原位更新。开发构建显示 `dev`，只能检查版本，不自动覆盖本地开发程序。

更完整的更新、安全边界与发布说明见 [docs/RELEASING.md](docs/RELEASING.md)。

## 从源码运行

安装 Rust stable 和平台构建工具后，在项目目录运行：

```sh
cargo run --locked
```

构建和检查：

```sh
cargo build --locked --release
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

macOS 需要 Xcode Command Line Tools；Windows 推荐 MSVC Rust 工具链和 Visual Studio C++ Build Tools；Ubuntu 的构建依赖可参考 [Release 工作流](.github/workflows/release.yml)。普通本地构建版本为 `dev`；发布版本由维护者手动推送 `v*` tag 后在 GitHub Actions 中注入，并自动生成 Release 产物。

## 项目文档与许可

- [功能进度与实现说明](docs/PROGRESS.md)
- [发布、更新与验收说明](docs/RELEASING.md)
- [MIT License](LICENSE)
