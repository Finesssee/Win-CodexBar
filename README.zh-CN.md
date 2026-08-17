# Win-CodexBar

[English](./README.md) | [简体中文](./README.zh-CN.md) | [繁體中文（臺灣）](./README.zh-TW.md) | [日本語](./README.ja-JP.md) | [한국어](./README.ko-KR.md) | [Español mexicano](./README.es-MX.md) | [Türkçe](./README.tr-TR.md)

[CodexBar](https://github.com/steipete/CodexBar) 的 Windows 移植版 —— 一个系统托盘应用，让你随时掌握各个 AI 编程工具的用量额度。

> 基于 **Tauri + React** 构建，底层复用共享 **Rust** 后端。原版 CodexBar 是由 [Peter Steinberger](https://github.com/steipete) 开发的 macOS Swift 应用。

<p align="center">
  <img src="docs/images/tray-panel.png" width="280" alt="托盘面板 — 服务商网格与 Codex 用量"/>
  &nbsp;&nbsp;
  <img src="docs/images/settings-providers.png" width="480" alt="设置 — 服务商选项卡"/>
</p>

## 功能特性

- **56 个 AI 服务商** — Codex、Claude、Cursor、Factory、Gemini、Copilot、Antigravity、z.ai、MiniMax、Kiro、Vertex AI、Augment、OpenCode、Kimi、Kimi K2、Amp、Warp、Ollama、Azure OpenAI、T3 Chat、OpenRouter、JetBrains AI、Alibaba、Alibaba Token Plan、NanoGPT、Infini、Perplexity、Abacus AI、Mistral、OpenCode Go、Kilo、AWS Bedrock、Codebuff、DeepSeek、Windsurf、Manus、小米 MiMo、Doubao、Command Code、Crof、StepFun、Venice、OpenAI、Grok、ElevenLabs、Deepgram、Groq、LLM Proxy、Chutes、LiteLLM、Poe、Devin、Zed、CrossModel、Qoder、Sakana AI
- **系统托盘图标** — 动态双条进度显示会话与周用量
- **Floating Bar** — 可选的置顶透明用量条，支持方向、透明度和点击穿透控制
- **浏览器 Cookie 导入** — Chrome、Edge、Brave、Firefox（Windows DPAPI 解密）
- **逐服务商凭据管理** — API Key、Cookie 和 OAuth 均可在服务商详情面板管理
- **凭据加固** — 应用管理的本地敏感存储会在保存时使用 Windows DPAPI 保护
- **Windows 发布打包** — Inno Setup 安装包、独立便携 exe、WebView2 Runtime 引导、VC++ 运行库引导和 SHA-256 校验文件
- **CLI** — `codexbar usage`、`codexbar cost`、`codexbar config` 和本机回环 `codexbar serve`，便于脚本化、本地集成和 CI
- **WSL 支持** — CLI 开箱即用，桌面壳层通过 WSLg 运行

## v0.33.2 更新内容

- 修复托盘面板失焦后不会自动关闭的问题，现在表现更接近标准 Windows 托盘弹窗。
- 支持按 Escape 关闭托盘面板，不会退出应用。
- 修复点击托盘图标触发失焦关闭后又立刻重新打开的反弹问题。

## v0.33.1 更新内容

- 当 GitHub Copilot 返回超额预算时，现在会显示真实百分比，例如 `115% used`，而不是强行压到 `100%`。
- 进度条仍然保持满格显示，避免 UI 溢出；托盘、弹出面板、Provider 侧栏和设置详情都会保留真实超额数值。

## v0.33.0 更新内容

- 将上游 CodexBar v0.33.0 的 provider 与成本统计修复移植到 Win-CodexBar。
- 设置界面新增日语作为可选显示语言。
- 加固带凭据的 provider HTTP 请求：跨源重定向不会继续沿用 provider 认证上下文。
- 更新 Claude 本地成本估算，覆盖 Fable 5、Opus 4.6、Sonnet 4.6 与 1 小时 cache write 计价。
- 修复 Doubao Ark 成功响应里不可靠的 `0 remaining` 请求限制头导致误显示 100% 用尽的问题。

## v0.32.2 更新内容

- 将上游 CodexBar v0.32.2 的性能优化和托盘 UI 微调移植到 Win-CodexBar。
- 本地 Codex token 成本扫描会先走轻量 JSONL 快速路径，大型 session 日志库扫描更快、内存占用更低。
- 紧凑托盘卡片增加横向和纵向留白，账号与套餐行不再那么拥挤。
- 增加当前 Codex token-count JSONL 形态的回归测试，覆盖 `last_token_usage`、`total_token_usage` 和旧版 `event_msg` payload。

## v0.32.1 更新内容

- 将上游 CodexBar v0.32.1 的稳定性修复移植到 Win-CodexBar。
- 托盘面板打开后会短暂延后自动 provider 刷新，让 UI 先完成绘制并保持可点击。
- Codex 凭据读取会复用短生命周期缓存，并避免在进程内保留未使用的 Codex refresh token。
- Claude OAuth 用量读取保持只读，不接管 Claude Code 自己管理的凭据生命周期。

## v0.32.0 更新内容

- 将上游 CodexBar v0.32.0 的 provider 修复移植到 Win-CodexBar。
- Providers 设置页新增搜索，可按服务商名称或 id 过滤大型 provider 列表，同时不破坏拖拽排序的完整顺序。
- 更新 Augment CLI 解析，支持新版 `auggie account status` 输出，并保留旧格式兼容。
- 加固 Ollama Web Cookie 获取：导入的 Cookie 只会附加到 HTTPS `ollama.com` 请求，不会在不安全重定向中继续携带。
- 改进 Antigravity model quota 选择：image/lite/autocomplete/internal 行不会驱动主摘要条，但仍保留在详细 model 窗口中。
- Claude 首次临时 auth/unauthorized 刷新失败时会保留上一次成功用量快照；连续失败仍会显示真实错误。

## v0.31.1 更新内容

- 修复 Antigravity 在 Windows 上无法获取用量的问题：当本地 language server 的 API 绑定到随机监听端口，而不是 `--extension_server_port` 附近端口时，现在也能正确发现。
- 应用会优先检查 Antigravity language server 进程实际监听的端口，同时保留旧的启发式端口探测作为 fallback。

## v0.31.0 更新内容

- 将上游 CodexBar v0.31.0 的 provider 行为修复移植到 Win-CodexBar。
- AWS Bedrock 现在支持通过命名 AWS CLI profile 获取用量，包括 AWS CLI 可解析的 SSO / assume-role profile。
- 当 Codex 用量接口返回 Spark 专属限制时，会显示 Codex Spark 5 小时与每周 quota。
- 隐藏 Claude 已废弃的 Design quota，同时保留其他 Claude 用量窗口。
- 本地 Codex/Claude 图表扫描支持取消，连续刷新时会更快停止过期 JSONL 扫描。

## v0.30.3 更新内容

- 修复 DeepSeek 余额显示：仅有 CNY/RMB 余额的账号不再因为 USD 为 0 而显示 Exhausted。
- 已在 Windows 上通过原生 Rust provider 测试验证 DeepSeek CNY fallback 回归用例。
- 包含 v0.30.2 的 About 链接修复。

## v0.30.2 更新内容

- 修复 About 选项卡外部链接按钮，GitHub、Website、Original Project 和页脚项目链接现在会通过 Windows Tauri 壳层正确打开。
- 已在真实 Windows 桌面中验证 About 选项卡链接流程。
- 包含 v0.30.1 的 Codex 本地用量修复。

## v0.30.1 更新内容

- 修复当前 Codex session 日志格式下的本地 token 用量解析。
- 修复本地 token 总数中 cached input tokens 被重复计入的问题。
- Codex 本地成本扫描改为复用共享 JSONL 扫描器，保持托盘、图表和 CLI 路径一致。
- 异步本地用量数据加载后会正确刷新托盘布局。
- 包含 v0.30.0 的服务商更新。

## v0.30.0 更新内容

- DeepSeek 新增用量摘要：token 总量、请求数、Top model、分类明细，以及平台 API 暴露时的当月成本。
- OpenAI Admin API 用量支持在服务商详情面板按可选 project ID 限定范围，默认仍为组织级用量。
- Alibaba Token Plan 更新到当前 Bailian 订阅摘要 API，并扩展新的额度/重置字段解析。
- StepFun Oasis 在存在 access/refresh 组合 token 时可刷新过期 token。
- 托盘和设置 UI 显示更丰富的 Ollama pace windows 与 Antigravity per-model quota windows。

## 快速开始

```powershell
# 前置要求：Node.js + pnpm — Rust 和 MinGW 将自动安装
git clone https://github.com/Finesssee/Win-CodexBar.git
cd Win-CodexBar
.\scripts\dev.ps1
```

脚本会自动安装 Rust/MinGW（如缺失）、构建 Tauri 桌面壳层并启动应用。

```powershell
.\scripts\dev.ps1 -Release          # 优化构建
.\scripts\dev.ps1 -SkipBuild        # 跳过构建，直接启动
```

## 下载

使用 Windows Package Manager 安装：

```powershell
winget install Finesssee.Win-CodexBar
```

Winget 分发已通过 [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs/tree/master/manifests/f/Finesssee/Win-CodexBar) 审核。GitHub Release 发布后，新版本可能需要一点时间才会出现在 Winget 中，因为每个版本都要固定自己的安装包 URL 和 SHA-256 哈希。

也可以前往 [GitHub Releases](https://github.com/Finesssee/Win-CodexBar/releases) 下载最新版本。

- **安装包**：`CodexBar-<version>-Setup.exe`
- **便携版**：`CodexBar-<version>-portable.exe`
- **校验和**：每个发布版本都包含 `.sha256` 文件，便于手动校验

安装包会包含桌面应用、Microsoft Evergreen WebView2 引导程序、应用图标、开始菜单快捷方式、卸载信息，以及干净 Windows 机器可能需要的 Visual C++ 运行库引导。便携版 exe 是没有安装器集成的同一个桌面应用；release 构建会静态链接 WebView2 loader，所以便携版用户只需要机器上已安装 Microsoft Edge WebView2 Runtime。

## 快速 Windows 发布构建

在 Windows 机器上做本地发布构建时，使用缓存版构建脚本：

```powershell
.\scripts\windows-release-build.ps1 -Ref v0.32.2
```

脚本会在 `C:\code\Win-CodexBar-release\source` 维护干净源码签出，在 `C:\code\Win-CodexBar-release\cache\cargo-target` 复用 Rust 构建输出，在 `C:\code\Win-CodexBar-release\cache\pnpm-store` 复用 pnpm 包，并复用已签名的 WebView2/VC++ 引导程序下载。它仍会构建真实 release 二进制、校验 Microsoft 签名、用 Inno Setup 打包，并在 `C:\code\Win-CodexBar-release\assets` 输出 GitHub Release 使用的四个资产。

常用发布参数：

```powershell
.\scripts\windows-release-build.ps1 -Ref vX.Y.Z -WarmCacheOnly
.\scripts\windows-release-build.ps1 -Ref vX.Y.Z -SmokeInstall
.\scripts\release-doctor.ps1 -Version X.Y.Z
```

构建脚本不再上传 GitHub Release。发布仅通过需要人工批准的 CircleCI
Windows 草稿发布流程完成；它会校验 SHA-256，绝不会覆盖已有资产。

## 首次运行

1. 启动 CodexBar — 它会驻留在系统托盘
2. 点击托盘图标打开用量面板
3. 前往 **Settings → Providers**，启用你使用的服务商
4. 对于基于 Cookie 的服务商，点击服务商后使用 **Browser Cookies → Import**
5. 对于基于 CLI 的服务商（`codex`、`claude`、`gemini`），请确保已登录

## CLI

```bash
codexbar usage -p claude          # 单个服务商
codexbar usage -p all             # 所有已启用的服务商
codexbar cost  -p codex           # 本地成本（JSONL 日志）
```

## 支持的服务商

| 服务商 | 认证方式 | 跟踪内容 |
|--------|----------|----------|
| Codex | OAuth / CLI | 会话、周用量、Credits |
| Claude | OAuth / Cookies / CLI | 会话（5h）、周用量 |
| Cursor | Cookies | 套餐、用量、账单 |
| Factory | Cookies | 用量 |
| Gemini | gcloud OAuth | 配额 |
| Copilot | GitHub Device Flow | 用量 |
| Antigravity | Cookies / LSP | 用量 |
| z.ai | API Token | 配额 |
| MiniMax | API / Cookies | 用量、账单汇总 |
| Kiro | Cookies / CLI | 月度 Credits、超额用量 |
| Vertex AI | gcloud OAuth | 成本 |
| Augment | Cookies | Credits |
| OpenCode | 本地配置 | 用量 |
| Kimi | Cookies | 5h 速率、周用量 |
| Kimi K2 | API Key | Credits |
| Amp | Cookies | 用量 |
| Warp | 本地配置 | 用量 |
| Ollama | Cookies | 用量 |
| OpenRouter | API Key | Credits |
| JetBrains AI | 本地配置 | 用量 |
| Alibaba | Cookies | 用量 |
| NanoGPT | API Key | Credits |
| Infini | API Key | 会话、周用量、配额 |
| Perplexity | Cookies | Credits、套餐 |
| Abacus AI | Cookies | Credits |
| Mistral | Cookies | 账单、用量 |
| OpenCode Go | Cookies | 用量、Zen 余额 |
| Kilo | API Key / CLI | 用量 |
| Codebuff | API Key / 本地配置 | Credits、周用量 |
| DeepSeek | API Key | 余额 |
| Windsurf | 本地缓存 | 日用量、周用量 |
| Manus | Cookies | Credits、刷新 Credits |
| 小米 MiMo | Cookies | 余额、Token 套餐 |
| Doubao | API Key | 请求限制 |
| Command Code | Cookies | 月度 Credits、已购 Credits |
| Crof | API Key | Credits、请求配额 |
| StepFun | Oasis Token | 5h、周用量 |
| Venice | API Key | USD / DIEM 余额 |
| OpenAI | Admin API / API Key | 用量、请求数、余额 |
| Grok | Cookies / auth.json | 账单 |
| ElevenLabs | API Key | 订阅 Credits、Voice Slots |
| Deepgram | API Key | 项目用量 |
| Groq | API Key | Enterprise Metrics |
| LLM Proxy | API Key | 配额统计 |

## 隐私

- **仅本地处理** — 不会将数据发送到外部服务器（服务商 API 除外）
- **不扫描磁盘** — 只读取已知配置路径和浏览器 Cookies
- **按需启用** — 只有启用相应服务商后才会提取 Cookies
- **受保护的凭据存储** — 应用管理的 API Key、手动 Cookie 和令牌账户会写入安全文件层；Windows 上会优先使用当前用户的 DPAPI
- **安全诊断** — 诊断快照只展示服务商、来源和状态等元数据，不展示原始 Cookie、API Key、Bearer Token 或 OAuth 值
- **已验证更新** — 自动下载的安装包需要 GitHub SHA-256 摘要，并会在应用前再次校验

## 更多文档

| 主题 | 链接 |
|------|------|
| 从源码构建 | [docs/BUILDING.md](docs/BUILDING.md) |
| WSL 设置与认证 | [docs/WSL.md](docs/WSL.md) |
| 浏览器 Cookie 详解 | [docs/COOKIES.md](docs/COOKIES.md) |

## 致谢

- **原版 CodexBar**：[steipete/CodexBar](https://github.com/steipete/CodexBar)，作者 Peter Steinberger
- **灵感来源**：[ccusage](https://github.com/ryoppippi/ccusage)，用于成本跟踪思路

## 许可证

MIT — 与原版 CodexBar 保持一致

---

*如需原版 macOS 版本，请访问 [steipete/CodexBar](https://github.com/steipete/CodexBar)。*
