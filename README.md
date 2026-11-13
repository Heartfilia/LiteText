# LiteText

**轻量、原生、跨平台的文本编辑与 HEX 阅读工具。**

LiteText 使用 Rust + egui 构建，不依赖 WebView，面向日志查看、临时文本处理、JSON／HTML 整理、HTTP 请求转换和二进制内容检查等日常工作。

[下载安装](#下载安装) · [快速开始](#快速开始) · [功能说明](#功能说明) · [配置与数据存储](#配置与数据存储) · [快捷键](#快捷键) · [应用内更新](#应用内更新) · [常见问题](#常见问题) · [源码构建](#源码构建)

> 本文描述当前 `main` 分支。已发布安装包的功能以对应版本为准；推送 `main` 不会自动发布新版安装包。

## 功能概览

| 功能 | 说明 |
| --- | --- |
| 多标签与多窗口 | 拖拽排序、批量关闭、独立窗口、未命名工作区、最后窗口恢复 |
| 文本编辑 | 撤销／重做、行操作、语言相关注释、编码转换、外部文件变更保护 |
| 大文本阅读 | 可视区域分段排版、增量行索引、大段粘贴和编辑 |
| 查找与定位 | 普通／正则查找、忽略大小写、匹配计数、替换、逻辑行号跳转 |
| 阅读辅助 | 自动换行、空白字符显示、五色记号笔、语法高亮、Log 高亮 |
| 格式化 | JSON 格式化与折叠，HTML、Header、Cookie 格式化 |
| 请求与文本转换 | cURL 转七种目标代码／文本、URL 解析、MD5、Base64、URL 编解码 |
| HEX 阅读 | 字节选择、ASCII 栏、零值淡化、不可见内容和 Data Inspector |
| 个性化 | 中文／英文、全局主题与自定义配色、工作区字号、自定义快捷键 |
| 本地集成 | 文件拖入、命令行文件参数、macOS Finder 打开、从文件目录启动终端 |
| 配置与升级 | 常用偏好自动保存、GitHub 更新检查、校验后手动确认安装 |

**数据安全约定：编辑、格式化和恢复备份不会自动覆盖原始文档，只有手动保存才会写入目标文件。** HEX 目前只读，不支持字节编辑。

## 下载安装

请从本项目的 [GitHub Releases](https://github.com/Heartfilia/LiteText/releases) 获取安装包。

| 平台 | 产物 | 安装方式 |
| --- | --- | --- |
| macOS Apple Silicon（M 系列） | `LiteText-macos-arm64.dmg` | 将 App 拖入 Applications |
| Windows x64 | `LiteText-windows-x64.exe` | 单文件运行，无需安装器 |
| Linux x64 | `LiteText-linux-x64.zip` | 解压后运行，需要图形桌面 |

目前不提供 macOS Intel／x64、Windows ARM、Linux ARM、deb／rpm／AppImage 安装包。名称带 `beta`／`rc` 的版本属于预发布版本。

### 校验下载文件

每个 Release 附带 `SHA256SUMS`。下载后计算 SHA-256，与**同一版本**清单中的对应项比较：

```sh
# macOS
shasum -a 256 LiteText-macos-arm64.dmg

# Linux
sha256sum LiteText-linux-x64.zip
```

Windows PowerShell：

```powershell
Get-FileHash .\LiteText-windows-x64.exe -Algorithm SHA256
```

哈希校验用于检查完整性，**不等于独立的发布者签名**。请确认文件和清单均来自可信的本项目 Release；校验不一致时重新下载，不要继续安装。

### macOS（Apple Silicon）

1. 打开下载的 DMG。
2. 将 **LiteText.app** 拖到 **Applications（应用程序）** 文件夹。
3. 弹出磁盘映像，从「应用程序」启动 LiteText。不要长期在 DMG 内运行。

也可从终端启动并打开文件：

```sh
open "/Applications/LiteText.app"
open -a "/Applications/LiteText.app" "/path/to/example.jsonl"
```

#### 首次打开被系统拦截

当前 macOS 包使用 ad-hoc 签名，**尚未使用 Apple Developer ID 签名和公证**。可先尝试在 Finder 中右键应用 →「打开」，或查看「系统设置 → 隐私与安全性」中的允许打开提示。

仅在确认安装包来自可信 Release、并核对完整性后，可以手动移除**这一个 App** 的下载隔离标记：

```sh
xattr -dr com.apple.quarantine "/Applications/LiteText.app"
open "/Applications/LiteText.app"
```

> 此命令削弱 macOS 对该应用的首次打开检查，不会完成公证，也不能修复真正损坏的文件。不要对未知应用、整个 Applications 文件夹或系统目录执行，不需要关闭 Gatekeeper。安装在其他位置时请替换路径；权限不足时先检查位置和当前用户权限，不要盲目添加 `sudo`。

DMG 内的 `安装后打开失败查看.txt` 也包含相关说明，软件不会自动执行上述命令。

### Windows x64

1. 下载 EXE，放到固定、当前用户可写的目录。
2. 双击运行，正常启动只显示软件窗口，不附带控制台。
3. 可将文件拖入窗口，或从命令行传入文件路径：

```powershell
.\LiteText-windows-x64.exe "C:\work\example.log" "C:\work\config.json"
```

如果出现 SmartScreen 或安全软件提示，请先核对来源，不要为了运行未知文件关闭安全软件。应用内更新需要程序目录可写，不会自动请求管理员权限。

### Linux x64

在下载目录执行：

```sh
mkdir -p "$HOME/.local/opt/litetext"
unzip LiteText-linux-x64.zip -d "$HOME/.local/opt/litetext"
chmod +x "$HOME/.local/opt/litetext/litetext"
"$HOME/.local/opt/litetext/litetext"

# 打开一个或多个文件
"$HOME/.local/opt/litetext/litetext" ./example.log ./config.json
```

需要 X11／Wayland 图形会话及 OpenGL 运行库；无桌面的纯 SSH 环境不能直接显示窗口。缺少动态库时，请按发行版安装对应依赖。

## 快速开始

1. **打开或新建**：点击欢迎页「选择文件」、使用菜单、拖入文件，或按 `Cmd/Ctrl+O`；`Cmd/Ctrl+N` 新建未命名文本。
2. **编辑与阅读**：切换标签，使用顶部工具打开自动换行、空白显示、记号笔或查找替换。
3. **处理文本**：在「菜单 → 插件」格式化 JSON／HTML／Header／Cookie，或转换 cURL。操作作用于当前文本工作区，支持一次撤销。
4. **手动保存**：按 `Cmd/Ctrl+S`。未命名文本首次保存时选择位置；未保存改动由本地恢复记录保护，不会因此写回原文件。
5. **个性化**：按 `Cmd/Ctrl+,` 打开系统设置，选择语言、主题、快捷键、终端和更新代理；修改即时生效并自动保存。

## 功能说明

### 标签与窗口

- 文件支持多选打开；当前窗口已打开的同一路径只激活已有标签。
- 标签可拖拽排序，右键关闭其他／左侧／右侧标签，或打开文件所在文件夹。
- 双击标签可关闭；标签栏末尾有足够空白时，双击空白区域可新建标签。
- 未命名文本按当前窗口**最小空闲序号**命名。关闭「未命名 1」后可复用 1，所有未命名标签关闭后从 1 开始；普通文件不占用编号，编号也不代表磁盘缓存数量。
- 「菜单 → 新窗口」创建空白独立窗口，不复制其他窗口的文档内容。
- macOS 安装包声明了文本、日志、JSONL／NDJSON、常见代码和配置文件类型，可通过 Finder「打开方式 → LiteText」打开，应用运行中也能接收文件。

LiteText 不会主动修改系统默认应用。macOS 如需双击默认打开，可在 Finder「显示简介 → 打开方式」选择 LiteText 并「全部更改」；替换 App 后先启动一次新版，让系统刷新注册。

### 编辑、行号与编码

- 支持撤销／重做、整行删除，以及按当前语言规则注释／取消注释。
- 行号比正文小 **1 pt**，按对应行垂直居中，并为数字两侧保留间距。
- 点击／拖动行号可选择一行或多行；右键可「复制本行／复制内容」或「清除内容」。清除操作保留行结构，支持撤销。
- 双击正文按字符类别选词，连续中文与英文分别选择，不包含周围标点；中文不是词典分词。
- 正文及 HEX 工作区字号可在 **10～32 pt** 范围缩放，不放大菜单和设置界面。
- 状态栏显示编码、语言、光标逻辑行或选中字数；点击文件路径可复制完整地址。
- 支持 UTF-8、UTF-8 BOM、带 BOM 的 UTF-16 LE／BE；保存编码选项还包括 GBK、GB18030、Big5、Shift_JIS 和 Windows-1252。

编码转换选择的是**保存编码**，不是重新猜测／解读原始字节。无法无损解码或目标编码无法表示当前字符时，会拒绝有损覆盖；并非所有旧编码都能自动识别。

### 查找、替换与阅读辅助

- 支持普通文本／正则搜索、忽略大小写、前后匹配导航、单次替换和全部替换。
- 搜索与总数统计按操作执行，不在每次输入时持续扫描全文；正则替换支持捕获组。
- 查找面板内的「跳转行」支持按钮与回车，使用从 1 开始的逻辑行号。0、越界或溢出值不执行跳转。
- 自动换行产生的视觉行不会改变逻辑行号。
- 五色记号笔可多次标记选区、按颜色清除或全部清除；标记只用于显示，不写入正文，仅在本次运行中保留。
- 状态栏可自动识别或手动选择高亮语言；`.log` 默认使用 Log 高亮，区分时间戳、数字、字符串及常见日志级别。
- JSON 对象／数组支持折叠；折叠不删除数据，不会把隐藏内容从保存结果中移除。

大文本使用可视区域分段排版与增量索引；大段粘贴会进入大文本路径。它仍需读取文件及建立必要索引，**不等于恒定内存或无限文件大小支持**。超长单行、复杂正则、格式化和极大文本仍可能耗时；长行可能降级为普通颜色以控制高亮成本。

### 格式化与文本转换

**以下工具处理当前激活文本工作区的全部内容，而非仅处理选区。成功后可一次撤销，失败保留原文，不自动保存原文件。**

| 入口 | 功能与约定 |
| --- | --- |
| 插件 → Format JSON | JSON 格式化，保留对象键顺序、数字字面值及重复键；成功后切换 JSON 高亮 |
| 插件 → Format HTML | 两空格缩进，保留 `pre` 等空白敏感内容；不执行脚本，不另行美化内嵌 JS／CSS |
| 插件 → Format Header | 请求头／响应头转 JSON，支持 HTTP 首行与 HTTP/2 伪头；同名头合并为数组 |
| 插件 → Format Cookie | 支持 Cookie 字符串、多行 Set-Cookie、浏览器 JSON 和 Netscape cookie jar；重复名称保留为数组 |
| 插件 → curl解析 | 选择目标语言卡片，将 cURL 转换为代码或 HTTP 请求文本 |
| 空白符处理 | 清理行首／行尾空白，TAB 与空格转换；底部 TAB 菜单设置默认制表符、2 空格或 4 空格输入 |
| 文本转换 → MD5 | 对 UTF-8 字节生成小写十六进制摘要；MD5 不适合密码存储或安全签名 |
| 文本转换 → Base64 | 标准 Base64 编解码；解码必须得到有效 UTF-8 文本 |
| 文本转换 → urlencode／urldecode | UTF-8 组件编码，空格为 `%20`、加号为 `%2B`；解码保留字面 `+` |
| 文本转换 → url解析 | 单个绝对 URL 转 JSON，包含基础 URL、协议、主机、端口、路径、参数和 fragment |

补充边界：

- HTML 支持平衡花括号属性，例如 `data-cost={"renderCost":"0.4"}`；单次限 32 MiB、100000 个 `<` 标记、128 层嵌套，格式化仍同步执行。
- Header 值中的冒号保留，同名头按不区分大小写处理。Cookie 值中的 `=` 和百分号保留，Set-Cookie 属性不当作独立 Cookie。
- URL 解析中的查询参数使用表单规则：`+` 代表空格、`%2B` 代表加号，重复参数为数组；路径和 fragment 保留编码形式。它与上述 `urldecode` 的字面 `+` 规则不同。
- 无效百分号、非法 Base64、非 UTF-8 解码结果或不支持的输入会报错，不会悄悄丢弃原文。

### cURL 转换范围

支持以下七种输出，不需要安装 Node.js 或 Python 才能**执行转换**；运行生成代码则需自行配置对应环境与依赖。

| 目标 | 使用方式 |
| --- | --- |
| Python | `requests` |
| JavaScript | 浏览器 `fetch` |
| Node.js | Node.js 18+ 原生 `fetch` |
| Go | 标准库 `net/http` |
| Rust | `reqwest` 阻塞客户端 |
| PHP | cURL 扩展 |
| HTTP | HTTP/1.1 请求文本 |

原生 Rust 实现参考 [curlconverter](https://github.com/curlconverter/curlconverter) 的「命令 → 请求模型 → 目标代码」流程，**未完整嵌入上游库，不保证兼容所有 cURL 参数**。

- 接受 Bash／POSIX 引号、反斜杠续行和单个 HTTP(S) URL。
- 支持 `-X`、`-H`、`-b` Cookie 字符串、`-u user:password`、User-Agent、Referer、`--data`／`--data-raw`／`--data-binary`／`--data-urlencode`／`--json`、`-G`、`-I`、`-L`。
- 请求正文以字面 UTF-8 保留，不把 JSON 自动重构成目标语言对象。
- **转换不执行 shell、不发起网络请求、不读取 `@file` 或 stdin**。
- multipart、代理、`--compressed`、动态 shell 表达式、多个请求等不支持的输入会明确报错。不能忠实表示的重复头或 fetch GET／HEAD 正文也会拒绝。
- 浏览器 fetch 仍受 CORS、禁用请求头与 Cookie 策略限制。Cookie、Token 和认证信息可能原样出现在生成代码中，分享前请脱敏。

### HEX 与 Data Inspector

二进制文件自动进入 HEX 视图；文本也可通过 TXT／HEX 切换查看对应字节。

- 支持字节范围选择、ASCII 栏、十六进制大小写、零值淡化和不可见内容显示。
- Data Inspector 显示偏移、所选长度、字符串预览，以及从选区起点读取的整数／浮点数；多字节数值按小端序解释。
- 长值可双击查看完整内容，右键复制值；检查器宽度可调整并保存。
- **只读检查工具，不支持修改二进制字节。**

### 打开终端

先在「系统设置 → 终端」勾选「工具栏显示打开终端」（默认隐藏）。打开工作区后，终端图标位于顶部查找替换与视图切换附近。

- 关联本地文件：在文件所在目录打开终端。
- 未命名文本／已解除关联副本：在当前用户主目录打开终端。
- 可选择检测到的终端，或填写自定义**程序路径**，不是 shell 命令；支持重新检测。
- macOS 支持 Terminal、iTerm2、Otty 等已实现的终端检测；首次调用可能请求系统「自动化」权限。

## 配置与数据存储

### 升级后保留哪些设置

「系统设置」中的修改即时生效，常用偏好独立保存在 **`preferences.json`**，不放在安装包中。修改后约 0.5 秒自动保存，关闭设置、框架保存和安装更新前强制刷新；旧配置首次启动时迁移。

| 类别 | 持久化内容 |
| --- | --- |
| 语言 | 中文／英文 |
| 主题 | 跟随系统或主题预设、语法高亮开关、自定义配色 |
| 字号 | 工作区字号，10～32 pt |
| TAB | 默认制表符／2 空格／4 空格 |
| 终端 | 工具栏开关、终端选择、自定义模式及程序路径 |
| 快捷键 | 自定义绑定、主动取消的绑定 |
| 更新 | GitHub 更新代理开关及地址 |
| HEX | 大小写、零值淡化、ASCII 栏、不可见内容显示 |
| 数据检查器 | Data Inspector 宽度 |

预设包括跟随系统、Dark+、Light+、Darcula、IntelliJ Light、Monokai、Mariana、One Dark 和 Nord；自定义配色覆盖正文、背景、选区、行号和语法颜色等。

**同一系统用户保留应用数据目录时，替换 App／EXE、应用内升级或重装均可保留这些选择。** 换账户、换电脑或主动删除用户数据不在此范围；当前没有云同步。未修改设置的旧窗口不会覆盖另一个窗口刚保存的偏好。

### 数据目录

| 平台 | 默认位置 |
| --- | --- |
| macOS | `~/Library/Application Support/LiteText/` |
| Windows | `%APPDATA%\LiteText\data\` |
| Linux | `${XDG_DATA_HOME}/litetext/`（变量为绝对路径时），否则为 `~/.local/share/litetext/` |

目录内主要记录：

| 文件／目录 | 用途 |
| --- | --- |
| `preferences.json` | 上表中的常用偏好，不含文档正文 |
| `window.json` | 主窗口尺寸和位置 |
| `windows/<窗口 ID>/recovery/session.json` | 运行窗口的标签顺序、文件关联及必要的未保存内容 |
| `recovery/session.json` | 最后正常关闭窗口发布的会话，供下次启动恢复 |
| `app.ron`、`last-settings.json`、窗口配置与锁文件 | 兼容设置、最近文件路径及跨窗口协调；并非每个标签的一份历史正文 |

最近文件只记录最多 10 条路径，不是正文备份。记号笔、撤销历史、临时选区和搜索状态不作为长期偏好保存。

### 缓存、恢复与关闭规则

“活跃标签”指**仍打开的标签**，包括当前未选中的标签。

- 约每 5 秒备份当前会话；正常关闭和更新交接时再保存必要状态。异常退出仍可能丢失最近一个备份周期的输入。
- 未保存文本／临时副本记录必要内容；未修改的本地文件主要保留关联，恢复时重读磁盘。
- 关闭或确认放弃标签后更新当前恢复记录，不维护已关闭标签的正文历史。
- 多窗口提前关闭时，仅对修改过的本地关联文件询问保存／放弃／取消；纯临时工作区随该窗口关闭丢弃。
- 正常退出后只恢复**最后关闭窗口**的一份会话，不重新打开提前关闭的全部窗口；异常退出最多选择一份最新恢复会话。
- 恢复接管成功后清理旧共享副本并清空已退役窗口的可识别快照，不影响仍运行的窗口。
- 确认重新加载磁盘版本会丢弃当前未保存改动和旧撤销历史，不再生成历史归档；读取／解码失败保留当前工作区。旧版可识别的 `before-reload-<id>-<时间戳>.json` 会在恢复写入或窗口维护时清理。
- 未知／损坏记录、符号链接目标不盲目清理，设置和更新回滚包也不属于标签历史缓存。删除或替换记录**不等于磁盘安全擦除**。

恢复记录是本地明文，不是系统临时分区，也不替代重要文件的独立备份。包含敏感文档时，请按敏感数据保护整个用户数据目录。需要备份／迁移这些数据时，先关闭所有 LiteText 窗口，再复制目录；不要在运行中直接删除恢复文件。

### 外部修改保护

原文件被其他程序修改、移动、删除或暂时不可读时，LiteText 会提示处理：

- **重新加载磁盘版本**：成功读取并解码后才替换工作区；当前未保存改动会丢弃。
- **保留工作区并解除关联**：保留当前内容为临时副本，不再检查旧路径；后续保存需要选择目标。

保存前会检查目标内容；目标已变化或另存为覆盖已有文件时，不会直接静默覆盖。文件系统上的检查不能替代跨软件协作锁，重要文档仍建议独立备份。

## 快捷键

以下为默认绑定；系统设置中的快捷键页可修改文件／标签、基础编辑和查找替换绑定，点击按键区域录入，`Delete` 清空，`Esc` 取消，也可恢复默认。HEX 工具配置项不在该页展示，已有 HEX 操作仍保留。

| 操作 | macOS | Windows／Linux |
| --- | --- | --- |
| 新建／打开／保存 | `Cmd+N`／`Cmd+O`／`Cmd+S` | `Ctrl+N`／`Ctrl+O`／`Ctrl+S` |
| 关闭当前标签 | `Cmd+W` | `Ctrl+W` |
| 下一个／上一个标签 | `Ctrl+Tab`／`Ctrl+Shift+Tab` | `Ctrl+Tab`／`Ctrl+Shift+Tab` |
| 撤销／重做 | `Cmd+Z`／`Cmd+Shift+Z` | `Ctrl+Z`／`Ctrl+Shift+Z` |
| 剪切／复制／粘贴／全选 | `Cmd+X/C/V/A` | `Ctrl+X/C/V/A` |
| 删除整行 | `Cmd+Backspace` | `Ctrl+Y` |
| 注释／取消注释 | `Cmd+/` | `Ctrl+/` |
| 查找 | `Cmd+F` | `Ctrl+F` |
| 聚焦替换输入 | `Cmd+Option+F` | `Ctrl+Alt+F` |
| 下一个／上一个匹配 | `F3`／`Shift+F3` | `F3`／`Shift+F3` |
| 替换当前匹配 | `Cmd+Option+Enter` | `Ctrl+Alt+Enter` |
| 全部替换 | `Cmd+Option+Shift+Enter` | `Ctrl+Alt+Shift+Enter` |
| 统计匹配总数 | `Cmd+Option+K` | `Ctrl+Alt+K` |
| 放大／缩小工作区 | `Cmd++`／`Cmd+-` | `Ctrl++`／`Ctrl+-` |
| 系统设置 | `Cmd+,` | `Ctrl+,` |

放大同时接受 `Cmd/Ctrl+=`；部分笔记本键盘的 F3 需要配合 Fn。编辑和查找操作按当前焦点／工作区上下文生效。

## 应用内更新

更新由用户确认，**不会静默安装**：

1. 打开「系统设置 → 应用更新」。需要代理时先启用并填写地址。
2. 点击「检查更新」。
3. 发现更高兼容版本后，点击「下载更新」。
4. 下载、SHA-256 和版本校验通过后，先关闭其他 LiteText 窗口。
5. 点击「安装并重启」。软件先保存偏好与恢复记录，再由独立助手等待旧进程退出并替换程序；新版本启动未确认时尝试回退。

| 当前版本 | 更新通道 |
| --- | --- |
| 稳定版 | 仅稳定版 |
| beta／rc 等预发布版 | 预发布版和稳定版 |
| `dev` 本地构建 | 可检查两类版本，但不自动安装替换开发程序 |

代理支持 HTTP(S)／SOCKS5，例如 `http://127.0.0.1:7890`、`socks5h://127.0.0.1:1080`，不是 GitHub 镜像 URL 前缀。**仅用于 GitHub 更新检查和下载**；关闭后直连，不读取环境代理。地址明文保存，不建议包含敏感密码。

注意：

- 程序目录必须可写，不自动提权；macOS 不支持在 DMG 或 App Translocation 路径原位更新。
- 旧版仅识别 ZIP 的更新器需要手动安装新版一次，此后支持 DMG／EXE／ZIP 对应产物。
- 没有稳定版或当前平台完整产物时，会提示暂无兼容版本；网络、权限或 GitHub 限流错误仍需处理。
- 已进入安装交接的暂存目录、旧版本和诊断可能保留供回滚，不与标签缓存一起自动清理。
- 包管理器管理的安装应使用相应包管理器更新；真实升级、回滚和权限行为仍需在目标平台验收。

完整机制与验收建议见 [发布与应用内更新](docs/RELEASING.md)。

## 常见问题

### 为什么升级后 Downloads 权限又询问一次？

macOS 隐私授权与 LiteText 的偏好文件是两回事。当前 ad-hoc 签名在替换 App 或切换启动副本后，可能需要重新授权。建议固定从 `/Applications/LiteText.app` 启动，在「系统设置 → 隐私与安全性 → 文件与文件夹」检查授权。

`xattr` 只处理下载隔离标记，不授予文件夹访问权限；无需因此反复执行命令、自动重置权限或开启完全磁盘访问。

### 自动恢复是不是自动保存？

不是。自动恢复写 LiteText 自己的数据目录；只有手动保存才写原始文档。绿色恢复状态点表示当前修改已备份，不代表原文件已更新；黄色表示待备份，灰色表示无需备份的未修改／空白状态。

### 为什么未命名编号会再次使用？

编号只是当前窗口中未命名标签的显示名称，关闭后可复用，不用于统计历史文件。仍打开但未选中的标签继续保留，不会因切换标签而清理。

### 为什么终端按钮看不到？

默认隐藏。先打开系统设置并勾选工具栏终端开关，再打开一个工作区；自定义终端请填写程序路径，而不是完整命令行。

### cURL／格式化会上传内容吗？

这些转换在本地执行，不调用在线转换服务，cURL 转换也不会执行生成的请求。应用更新功能会访问 GitHub；用户主动点击仓库链接会打开浏览器。生成代码中的凭据与本地恢复文件仍需自行保护。

### 当前不包含哪些能力？

LiteText 不是完整 IDE，也不是完整 cURL 解释器；没有项目级 LSP／调试器、二进制编辑或云同步。大文本优化不保证任意大小文件无停顿，语法高亮也不是 IDE 语义分析。

## 源码构建

### 环境准备

- 安装较新的 Rust stable 工具链和 Cargo。当前本机验证使用 Rust 1.99；发布 CI 使用当时的 stable，项目未声明更低的 MSRV。
- macOS：Xcode Command Line Tools。
- Windows：MSVC Rust 工具链及 Visual Studio C++ Build Tools（含 Windows SDK）。
- Linux：C/C++ 构建工具及 X11／Wayland／OpenGL 开发依赖。Ubuntu 可参考 CI：

```sh
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libx11-dev libxi-dev libxcursor-dev libxrandr-dev libxinerama-dev libgl1-mesa-dev libwayland-dev libxkbcommon-dev
```

### 运行和检查

```sh
git clone https://github.com/Heartfilia/LiteText.git
cd LiteText

cargo run --locked

# 直接打开文件；路径含空格时加引号
cargo run --locked -- "/path/to/example.log" "/path/to/config.json"

# 优化构建与检查
cargo build --locked --release
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings

# 打包脚本测试，需要 Python 3
python3 -m unittest discover -s scripts -p 'test_*.py'
```

普通本地构建显示 `dev`。macOS／Linux 可执行文件为 `target/debug/litetext` 或 `target/release/litetext`，Windows 对应 `.exe`；**`cargo build` 本身不生成 macOS `.app`／DMG**。

当前 macOS 本机常规测试为 **215 项通过，2 项手动性能测试默认跳过**；Windows 另有平台专用图标测试。测试数量会随代码变化，目标平台结果以实际 CI 为准；手动性能测试不在普通测试中运行。

### 发布与项目结构

只有维护者手动推送 `v*` tag 才触发 [Release 工作流](.github/workflows/release.yml)。三个平台先测试、严格 Clippy 和构建，全部成功后发布 DMG／EXE／ZIP 与统一 `SHA256SUMS`。普通 `main` 推送不发布，重跑旧 tag 也不会带入新提交。版本通过 `LITETEXT_VERSION` 注入，详见 [发布文档](docs/RELEASING.md)。

```text
src/
  main.rs               应用状态、窗口、菜单与 HEX 界面
  editor.rs             普通文本编辑器、行号与行操作
  large_editor.rs       大文本索引、分段排版与增量编辑
  text_tools.rs         查找替换、跳转与显示辅助
  workspace_theme.rs    全局配色与语法高亮
  preferences.rs        常用偏好与跨窗口保存保护
  recovery.rs           会话记录、编码与原子写入
  new_window.rs         多窗口协调和最后窗口恢复
  linkage.rs            文件关联与外部修改检测
  shortcuts.rs          快捷键定义、分发与配置
  json_plugin.rs        JSON 格式化与折叠
  web_tools.rs          HTML／Header／Cookie／URL 处理
  curl_plugin.rs        离线 cURL 解析与代码生成
scripts/                发布打包、文件类型声明与测试
docs/                   实现进度、发布与验收说明
```

## 反馈与贡献

欢迎通过 [GitHub Issues](https://github.com/Heartfilia/LiteText/issues) 提交问题或建议。反馈时请附上系统及 CPU 架构、LiteText 版本、复现步骤、预期／实际结果，界面问题可附截图；文本处理问题请尽量提供脱敏的最小示例，不要上传 Token、Cookie、私人日志或恢复文件中的敏感内容。

提交改动前建议运行 Rust 测试、严格 Clippy 和相关打包测试。功能边界与待验收项见 [功能进度与实现说明](docs/PROGRESS.md)，跨平台安装与真实更新仍应做目标环境验证。

## 许可

LiteText 使用 [MIT License](LICENSE)。依赖及参考项目遵循其各自许可证。
