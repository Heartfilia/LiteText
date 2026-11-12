# 发布与应用内更新

## 版本与 CI

- `cargo run`、普通 `cargo build --release` 均显示 **dev**，不读取 Cargo 包版本或任意本地 Git tag。
- 发布通过 `LITETEXT_VERSION` 注入 tag（例如 `v1.2.3`），构建时验证 SemVer。
- 版本 tag 由维护者手动创建和推送，助手不代建、不移动既有 tag。推送 `v*` tag 触发 `.github/workflows/release.yml`；普通 main 推送不发布。每个平台先测试、Clippy，再构建；全部成功才发布 GitHub Release。
- 产物：macOS arm64（Apple Silicon）的 `.dmg`（首次安装）与 `.zip`（自动更新）、Windows x64 ZIP、Linux x64 ZIP，以及统一 `SHA256SUMS`。不再编译或上传 macOS x64（Intel）。
- `-rc.1` 等带连字符的 tag 发布为 prerelease；应用内目前仅查询 GitHub `/releases/latest` 的稳定版本。
- 配置目标仓库：**Heartfilia/LiteText**。发布前必须把 Cargo.lock、源码、assets、scripts 和 workflow 一并提交。失败任务的重跑仍使用原 tag 的旧提交，不会自动带入 main 上的修复；请在包含修复的提交上手动创建新版本 tag。

示例（自行确认代码和版本后执行）：

```sh
git tag v0.1.1
git push origin v0.1.1
```

## 用户更新流程

1. 首次安装 Release：macOS 将 DMG 中的 LiteText.app 拖至可写的应用目录；Windows/Linux 解压到用户有写权限的固定目录。
2. 系统设置中选择直连或专用 HTTP(S)/SOCKS5 代理，点击「检查更新」。两分钟内相同代理配置重复检查使用内存缓存；配置变动后重新检查。
3. 点击「下载更新」，后台下载对应 OS/CPU 的 ZIP 和 SHA256SUMS，检查大小与哈希，安全解压，并运行版本查询核对 tag。
4. 点击「安装并重启」：先写工作区恢复记录和配置，再启动独立更新助手，正常关闭应用。
5. 助手等待应用释放安装锁，在同一文件系统中重命名旧版、放入新版并启动；新版恢复会话、完成第一帧后写入启动确认。45 秒未确认时停止该新进程并尝试恢复旧版。原始文档始终不由更新器写入。

开发构建仅能检查版本，不自动覆盖 `target/` 中的程序。只读安装目录会报错，不自动提权。macOS 磁盘映像和 App Translocation 位置拒绝原位更新。

代理只用于本模块的 GitHub API、校验文件与下载请求。关闭代理时明确不读取 `HTTP_PROXY` 等环境变量。填写 `http://127.0.0.1:7890` 或 `socks5h://127.0.0.1:1080`，**不是** GitHub 镜像 URL 前缀。代理地址随设置持久化，不做加密；建议使用本机无凭据代理，不在共享机器的配置里保存密码。

## 安全边界与待验证项

- 只接受该仓库 HTTPS release 下载 URL，TLS 使用正常证书验证；压缩包拒绝路径穿越、符号链接、非预期根目录、重复文件，限制下载与解压大小。
- SHA256SUMS 用于完整性检查，**不是独立的发布者签名**；GitHub 账户、Release 和配置代理的信任仍是前提。不要安装不可信来源的更新。
- macOS 当前仅 ad-hoc 签名，**没有 Apple Developer ID 签名或 notarization**。不去除 quarantine、不绕过 Gatekeeper。面向公开分发前建议配置 Developer ID、公证与签名验证。
- 安装后的旧版本保留在安装目录内 `.litetext-stage-*` 的 `previous`；临时更新目录为 `litetext-update-*`，保留助手和诊断。未进入安装的下载/解压失败与取消会自动清理；安装交接后的目录故意保留供回退，不自动递归清理历史安装。
- 安装结果写入应用数据目录 `update-result.txt`，应用读取后显示在设置页。新进程无法启动时，助手临时目录还保留 `install-error.txt`。
- 不支持包管理器托管的安装升级（例如 Homebrew/apt）；应使用相应包管理器。Windows/Linux 首版是解压型应用，非 MSI/deb/rpm 安装器。
- macOS 本机编译与模拟事务已测；真实 GitHub tag 构建、跨平台首次安装、权限/Gatekeeper、真实版本升级和回退仍需 Release 验收。尚未替换用户当前应用或触发真实安装。

## 发布验收建议

在测试目录安装一个已成功发布的稳定版本，编辑未保存文档，再发布更高版本，验证直连和代理检查、下载与重启后恢复、原文件 hash 不变、版本号一致。另模拟哈希不匹配、断网、只读目录、启动失败，确认不替换或可以回退。应分别在 macOS arm64、Windows x64、Linux x64 实机执行。
