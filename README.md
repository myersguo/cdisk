# CDisk

Rust + Tauri + React 构建的本地 macOS 磁盘维护工具。支持“扫描、理解、确认、清理、记录”的完整流程。

[隐私说明](PRIVACY.md) · [安全策略](SECURITY.md) · [依赖审计](docs/dependency-audit.md) · [MIT License](LICENSE)

## 安装

当前预构建版本支持 Apple Silicon Mac，使用 ad-hoc 签名且尚未完成 Apple
公证。首次启动时请在 Finder 中右键 CDisk 并选择“打开”，确认来源后启动。

使用安装脚本（会校验发布包 SHA-256）：

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/myersguo/cdisk/main/scripts/install.sh | sh
```

或使用 Homebrew：

```bash
brew install --cask myersguo/tap/cdisk
```

- **多语言**：支持 English、简体中文、繁體中文、日本語、한국어、Español、Français；首次启动跟随 macOS 语言，手动选择后保存在本机。
- **AGM 设计语言**：浅色桌面工具界面、紧凑侧栏、低对比边界与清晰选中态；列表与详情仍可独立滚动。
- **日常清理**：单路径缓存/日志候选、搜索和分类、路径占用与应用运行保护。
- **项目瘦身**：可配置开发根目录，检查 Git ignore/跟踪内容、嵌套仓库和敏感文件，最近 7 天有活动的产物不建议选择。
- **安装包**：检查 Downloads/Desktop 中的常见安装包，保留已挂载镜像。
- **磁盘分析**：自定义目录或 Data 卷，逐层下钻、返回、按大小排序和 Finder 定位。只读，不直接删除。
- **保护名单**：持久化保护路径；保存会使待确认计划失效，但保留已有扫描结果。
- **操作记录**：逐项保存成功、跳过、失败与中断状态，以及磁盘可用空间前后变化。

日常清理、项目瘦身、安装包和磁盘分析可同时扫描，每页独立保存进度、结果、筛选和选择；同一页同时运行一个扫描。暂停保留当前位置，继续后接着扫描；暂停后可以选择已经完整扫描的候选并预览清理，此时会结束剩余扫描并保留当前结果。取消也能唤醒暂停的任务。暂停/取消在目录项边界生效，不强行中断内核文件系统调用。

候选随扫描陆续显示。暂停后，**已经完整扫描且通过检查的候选可以立即选择并预览清理**；进入预览时会结束剩余扫描并保留当前结果。取消后，已完成候选同样仍可清理；未完整扫描的项不可清理，可“重新检查此项”。磁盘分析结果依然只读。取消不等于清理，扫描本身不会删除文件。

所有候选初始不勾选；清理前展示确切路径，重新验证后永久删除。不会清理系统根目录、凭证、应用数据库、Codex 会话或 VM 运行时，不申请 sudo。清理和配置写入仍串行处理。

大量候选的预览校验使用最多 4 路有界并行，并复用一次进程快照；界面显示已完成数量、当前路径和取消按钮。预览只建立短期确认计划，不删除文件。真正执行时仍逐项重新做完整目录、Git、挂载、身份和占用检查；扫描后发生变化的项目会被跳过。

## 为什么项目不能勾选？

- **手动保护**：详情显示具体规则，点击“移除手动保护并检查”；设置页也可以逐条移除。移除父路径规则同时解除其子项的手动保护。
- **镜像已挂载**：先在 Finder 推出该安装镜像，再点“重新检查此项”。这不是手动保护，不能靠解除保护强删。
- **路径被占用**：详情会列出占用该路径的进程名和 PID；关闭对应任务后重新检查。
- **应用运行中**：仅应用自有缓存/日志按精确应用身份保护；完整退出该应用后重新检查。
- **系统/数据保护**：包含凭证、Git 跟踪内容等，不提供强制解锁。
- **状态未知 / 未完整扫描**：重新检查后才判断是否可以清理，不能跳过安全校验。

修改整份扫描设置前需结束或取消所有任务（暂停任务尚未结束）；保存后已有扫描结果、筛选和选择会保留，新加入保护名单的候选会即时锁定，移除保护的候选需重新检查。单项解除手动保护不会打断其他扫描。其他页面已有结果仍会在实际清理前按最新设置再次校验。

Go、npm、pnpm、Bun、uv、pip 和 Homebrew 等开发缓存只检查候选路径是否实际被打开，不会再因任意 Node、IDE 或 `/opt/homebrew` 下的进程正在运行而整类锁定。Chrome、飞书、微信、Codex 和 JetBrains 等应用自有缓存/日志还会检查对应 `.app` 是否在运行。扫描和单项重新检查会给出当前状态；预览执行快速资格检查；永久删除前再执行完整目录、Git、挂载、身份和占用检查。探针超时或输出异常会显示“状态未知”并拒绝清理。

对标公开工作流及当前差距见 [Mole 对标说明](docs/mole-benchmark.md)。未复制 Mole 源码；尚不提供卸载器、系统优化、实时监控和 Finder Trash 恢复。

## 开发

```bash
pnpm install
pnpm run tauri dev
```

## 验证

```bash
pnpm run verify
```

界面回归测试需要启动 `pnpm dev` 并提供 Playwright（可使用本机已有安装，不必新增生产依赖）：

```bash
PLAYWRIGHT_MODULE=/path/to/playwright node tests/i18n.cjs
PLAYWRIGHT_MODULE=/path/to/playwright node tests/ui.cjs
PLAYWRIGHT_MODULE=/path/to/playwright node tests/tasks.cjs
```

`pnpm dev` 的浏览器界面是明确标记的演示模式，不执行磁盘操作；原生应用使用真实 Rust 后端。

## 隐私与安全

- 打包应用不包含遥测、分析、云同步、自动更新或网络上传功能。
- 扫描结果只保存在内存；设置和最近 100 次清理记录保存在用户本机的 Application Support 目录。
- 清理历史包含完整本地路径，属于敏感本机数据。详见 [隐私说明](PRIVACY.md)。
- 删除只接受后端扫描快照中的候选 ID，并在永久删除前重新验证路径身份、Git、挂载、敏感文件和占用状态。
- 安全问题请通过 GitHub Private Vulnerability Reporting 报告，不要在公开 issue 中附带本机路径或私有文件。详见 [安全策略](SECURITY.md)。

## 打包

```bash
pnpm run tauri build --debug --bundles app
```

本地包位于 `src-tauri/target/debug/bundle/macos/CDisk.app`。调试配置关闭 debug symbols 与增量缓存以控制磁盘占用；不要在每次验证之间 `cargo clean`。发行包仍需要 Developer ID 签名和公证。

公开发布前按 [Release checklist](docs/release-checklist.md) 检查源码、依赖、
签名、公证、校验和及隐私材料。`release/` 和 debug bundle 仅用于本机验证，
不应直接上传到 GitHub Release。

本机交付副本：`release/CDisk.app`。本地配置/记录：`~/Library/Application Support/com.myersguo.cdisk/{settings,history}.json`，不上传网络。

无法读取的目录会显示部分结果；需要时由用户在 macOS“隐私与安全性 → 完全磁盘访问权限”中授权 CDisk。权限未知不会被当成 0 字节或可删除。
