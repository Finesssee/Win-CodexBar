# Win-CodexBar

[English](./README.md) | [简体中文](./README.zh-CN.md) | [繁體中文（臺灣）](./README.zh-TW.md) | [日本語](./README.ja-JP.md) | [한국어](./README.ko-KR.md) | [Español mexicano](./README.es-MX.md) | [Türkçe](./README.tr-TR.md)

[CodexBar](https://github.com/steipete/CodexBar) 的 Windows 移植版 —— 一個系統系統匣應用，讓你隨時掌握各個 AI 程式設計工具的用量額度。

> 基於 **Tauri + React** 構建，底層複用共享 **Rust** 後端。原版 CodexBar 是由 [Peter Steinberger](https://github.com/steipete) 開發的 macOS Swift 應用。

<p align="center">
  <img src="docs/images/tray-panel.png" width="280" alt="系統匣面板 — 提供者網格與 Codex 用量"/>
  &nbsp;&nbsp;
  <img src="docs/images/settings-providers.png" width="480" alt="設定 — 提供者選項卡"/>
</p>

## 功能特性

- **56 個 AI 提供者** — Codex、Claude、Cursor、Factory、Gemini、Copilot、Antigravity、z.ai、MiniMax、Kiro、Vertex AI、Augment、OpenCode、Kimi、Kimi K2、Amp、Warp、Ollama、Azure OpenAI、T3 Chat、OpenRouter、JetBrains AI、Alibaba、Alibaba Token Plan、NanoGPT、Infini、Perplexity、Abacus AI、Mistral、OpenCode Go、Kilo、AWS Bedrock、Codebuff、DeepSeek、Windsurf、Manus、小米 MiMo、Doubao、Command Code、Crof、StepFun、Venice、OpenAI、Grok、ElevenLabs、Deepgram、Groq、LLM Proxy、Chutes、LiteLLM、Poe、Devin、Zed、CrossModel、Qoder、Sakana AI
- **系統系統匣圖示** — 動態雙條進度顯示會話與周用量
- **Floating Bar** — 可選的置頂透明用量條，支援方向、透明度和點選穿透控制
- **瀏覽器 Cookie 匯入** — Chrome、Edge、Brave、Firefox（Windows DPAPI 解密）
- **逐提供者憑據管理** — API Key、Cookie 和 OAuth 均可在提供者詳情面板管理
- **憑據加固** — 應用管理的本機敏感儲存會在儲存時使用 Windows DPAPI 保護
- **Windows 釋出打包** — Inno Setup 安裝包、獨立便攜 exe、WebView2 Runtime 引導、VC++ 執行庫引導和 SHA-256 校驗檔案
- **CLI** — `codexbar usage`、`codexbar cost`、`codexbar config` 和本機迴環 `codexbar serve`，便於指令碼化、本機整合和 CI
- **WSL 支援** — CLI 開箱即用，桌面殼層透過 WSLg 執行

## v0.33.2 更新內容

- 修復系統匣面板失焦後不會自動關閉的問題，現在表現更接近標準 Windows 系統匣彈窗。
- 支援按 Escape 關閉系統匣面板，不會退出應用。
- 修復點選系統匣圖示觸發失焦關閉後又立刻重新開啟的反彈問題。

## v0.33.1 更新內容

- 當 GitHub Copilot 返回超額預算時，現在會顯示真實百分比，例如 `115% used`，而不是強行壓到 `100%`。
- 進度條仍然保持滿格顯示，避免 UI 溢位；系統匣、彈出面板、Provider 側欄和設定詳情都會保留真實超額數值。

## v0.33.0 更新內容

- 將上游 CodexBar v0.33.0 的 provider 與成本統計修復移植到 Win-CodexBar。
- 設定介面新增日語作為可選顯示語言。
- 加固帶憑據的 provider HTTP 請求：跨源重定向不會繼續沿用 provider 認證上下文。
- 更新 Claude 本機成本估算，覆蓋 Fable 5、Opus 4.6、Sonnet 4.6 與 1 小時 cache write 計價。
- 修復 Doubao Ark 成功響應裡不可靠的 `0 remaining` 請求限制頭導致誤顯示 100% 用盡的問題。

## v0.32.2 更新內容

- 將上游 CodexBar v0.32.2 的效能最佳化和系統匣 UI 微調移植到 Win-CodexBar。
- 本機 Codex token 成本掃描會先走輕量 JSONL 快速路徑，大型 session 日誌庫掃描更快、記憶體佔用更低。
- 緊湊系統匣卡片增加橫向和縱向留白，賬號與套餐行不再那麼擁擠。
- 增加當前 Codex token-count JSONL 形態的迴歸測試，覆蓋 `last_token_usage`、`total_token_usage` 和舊版 `event_msg` payload。

## v0.32.1 更新內容

- 將上游 CodexBar v0.32.1 的穩定性修復移植到 Win-CodexBar。
- 系統匣面板開啟後會短暫延後自動 provider 重新整理，讓 UI 先完成繪製並保持可點選。
- Codex 憑據讀取會複用短生命週期快取，並避免在程序內保留未使用的 Codex refresh token。
- Claude OAuth 用量讀取保持只讀，不接管 Claude Code 自己管理的憑據生命週期。

## v0.32.0 更新內容

- 將上游 CodexBar v0.32.0 的 provider 修復移植到 Win-CodexBar。
- Providers 設定頁新增搜尋，可按提供者名稱或 id 過濾大型 provider 列表，同時不破壞拖拽排序的完整順序。
- 更新 Augment CLI 解析，支援新版 `auggie account status` 輸出，並保留舊格式相容。
- 加固 Ollama Web Cookie 獲取：匯入的 Cookie 只會附加到 HTTPS `ollama.com` 請求，不會在不安全重定向中繼續攜帶。
- 改進 Antigravity model quota 選擇：image/lite/autocomplete/internal 行不會驅動主摘要條，但仍保留在詳細 model 視窗中。
- Claude 首次臨時 auth/unauthorized 重新整理失敗時會保留上一次成功用量快照；連續失敗仍會顯示真實錯誤。

## v0.31.1 更新內容

- 修復 Antigravity 在 Windows 上無法獲取用量的問題：當本機 language server 的 API 繫結到隨機監聽埠，而不是 `--extension_server_port` 附近埠時，現在也能正確發現。
- 應用會優先檢查 Antigravity language server 程序實際監聽的埠，同時保留舊的啟發式埠探測作為 fallback。

## v0.31.0 更新內容

- 將上游 CodexBar v0.31.0 的 provider 行為修復移植到 Win-CodexBar。
- AWS Bedrock 現在支援透過命名 AWS CLI profile 獲取用量，包括 AWS CLI 可解析的 SSO / assume-role profile。
- 當 Codex 用量介面返回 Spark 專屬限制時，會顯示 Codex Spark 5 小時與每週 quota。
- 隱藏 Claude 已廢棄的 Design quota，同時保留其他 Claude 用量視窗。
- 本機 Codex/Claude 圖表掃描支援取消，連續重新整理時會更快停止過期 JSONL 掃描。

## v0.30.3 更新內容

- 修復 DeepSeek 餘額顯示：僅有 CNY/RMB 餘額的賬號不再因為 USD 為 0 而顯示 Exhausted。
- 已在 Windows 上透過原生 Rust provider 測試驗證 DeepSeek CNY fallback 迴歸用例。
- 包含 v0.30.2 的 About 連結修復。

## v0.30.2 更新內容

- 修復 About 選項卡外部連結按鈕，GitHub、Website、Original Project 和頁尾專案連結現在會透過 Windows Tauri 殼層正確開啟。
- 已在真實 Windows 桌面中驗證 About 選項卡連結流程。
- 包含 v0.30.1 的 Codex 本機用量修復。

## v0.30.1 更新內容

- 修復當前 Codex session 日誌格式下的本機 token 用量解析。
- 修復本機 token 總數中 cached input tokens 被重複計入的問題。
- Codex 本機成本掃描改為複用共享 JSONL 掃描器，保持系統匣、圖表和 CLI 路徑一致。
- 非同步本機用量資料載入後會正確重新整理系統匣佈局。
- 包含 v0.30.0 的提供者更新。

## v0.30.0 更新內容

- DeepSeek 新增用量摘要：token 總量、請求數、Top model、分類明細，以及平臺 API 暴露時的當月成本。
- OpenAI Admin API 用量支援在提供者詳情面板按可選 project ID 限定範圍，預設仍為組織級用量。
- Alibaba Token Plan 更新到當前 Bailian 訂閱摘要 API，並擴充套件新的額度/重置欄位解析。
- StepFun Oasis 在存在 access/refresh 組合 token 時可重新整理過期 token。
- 系統匣和設定 UI 顯示更豐富的 Ollama pace windows 與 Antigravity per-model quota windows。

## 快速開始

```powershell
# 前置要求：Node.js + pnpm — Rust 和 MinGW 將自動安裝
git clone https://github.com/Finesssee/Win-CodexBar.git
cd Win-CodexBar
.\scripts\dev.ps1
```

指令碼會自動安裝 Rust/MinGW（如缺失）、構建 Tauri 桌面殼層並啟動應用。

```powershell
.\scripts\dev.ps1 -Release          # 最佳化構建
.\scripts\dev.ps1 -SkipBuild        # 跳過構建，直接啟動
```

## 下載

使用 Windows Package Manager 安裝：

```powershell
winget install Finesssee.Win-CodexBar
```

Winget 分發已透過 [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs/tree/master/manifests/f/Finesssee/Win-CodexBar) 稽核。GitHub Release 釋出後，新版本可能需要一點時間才會出現在 Winget 中，因為每個版本都要固定自己的安裝包 URL 和 SHA-256 雜湊。

也可以前往 [GitHub Releases](https://github.com/Finesssee/Win-CodexBar/releases) 下載最新版本。

- **安裝包**：`CodexBar-<version>-Setup.exe`
- **便攜版**：`CodexBar-<version>-portable.exe`
- **校驗和**：每個釋出版本都包含 `.sha256` 檔案，便於手動校驗

安裝包會包含桌面應用、Microsoft Evergreen WebView2 載入程式、應用圖示、開始選單快捷方式、解除安裝資訊，以及乾淨 Windows 機器可能需要的 Visual C++ 執行庫引導。便攜版 exe 是沒有安裝器整合的同一個桌面應用；release 構建會靜態連結 WebView2 loader，所以便攜版使用者只需要機器上已安裝 Microsoft Edge WebView2 Runtime。

## 快速 Windows 釋出構建

在 Windows 機器上做本機釋出構建時，使用快取版構建指令碼：

```powershell
.\scripts\windows-release-build.ps1 -Ref v0.32.2
```

指令碼會在 `C:\code\Win-CodexBar-release\source` 維護乾淨原始碼簽出，在 `C:\code\Win-CodexBar-release\cache\cargo-target` 複用 Rust 構建輸出，在 `C:\code\Win-CodexBar-release\cache\pnpm-store` 複用 pnpm 包，並複用已簽名的 WebView2/VC++ 載入程式下載。它仍會構建真實 release 二進位制、校驗 Microsoft 簽名、用 Inno Setup 打包，並在 `C:\code\Win-CodexBar-release\assets` 輸出 GitHub Release 使用的四個資產。

常用釋出引數：

```powershell
.\scripts\windows-release-build.ps1 -Ref vX.Y.Z -WarmCacheOnly
.\scripts\windows-release-build.ps1 -Ref vX.Y.Z -SmokeInstall
.\scripts\release-doctor.ps1 -Version X.Y.Z
```

構建指令碼不再上傳 GitHub Release。釋出僅透過需要人工核准的 CircleCI
Windows 草稿釋出流程完成；它會校驗 SHA-256，絕不覆蓋已有資產。

## 首次執行

1. 啟動 CodexBar — 它會駐留在系統系統匣
2. 點選系統匣圖示開啟用量面板
3. 前往 **Settings → Providers**，啟用你使用的提供者
4. 對於基於 Cookie 的提供者，點選提供者後使用 **Browser Cookies → Import**
5. 對於基於 CLI 的提供者（`codex`、`claude`、`gemini`），請確保已登入

## CLI

```bash
codexbar usage -p claude          # 單個提供者
codexbar usage -p all             # 所有已啟用的提供者
codexbar cost  -p codex           # 本機成本（JSONL 日誌）
```

## 支援的提供者

| 提供者 | 認證方式 | 跟蹤內容 |
|--------|----------|----------|
| Codex | OAuth / CLI | 會話、周用量、Credits |
| Claude | OAuth / Cookies / CLI | 會話（5h）、周用量 |
| Cursor | Cookies | 套餐、用量、賬單 |
| Factory | Cookies | 用量 |
| Gemini | gcloud OAuth | 配額 |
| Copilot | GitHub Device Flow | 用量 |
| Antigravity | Cookies / LSP | 用量 |
| z.ai | API Token | 配額 |
| MiniMax | API / Cookies | 用量、賬單彙總 |
| Kiro | Cookies / CLI | 月度 Credits、超額用量 |
| Vertex AI | gcloud OAuth | 成本 |
| Augment | Cookies | Credits |
| OpenCode | 本機配置 | 用量 |
| Kimi | Cookies | 5h 速率、周用量 |
| Kimi K2 | API Key | Credits |
| Amp | Cookies | 用量 |
| Warp | 本機配置 | 用量 |
| Ollama | Cookies | 用量 |
| OpenRouter | API Key | Credits |
| JetBrains AI | 本機配置 | 用量 |
| Alibaba | Cookies | 用量 |
| NanoGPT | API Key | Credits |
| Infini | API Key | 會話、周用量、配額 |
| Perplexity | Cookies | Credits、套餐 |
| Abacus AI | Cookies | Credits |
| Mistral | Cookies | 賬單、用量 |
| OpenCode Go | Cookies | 用量、Zen 餘額 |
| Kilo | API Key / CLI | 用量 |
| Codebuff | API Key / 本機配置 | Credits、周用量 |
| DeepSeek | API Key | 餘額 |
| Windsurf | 本機快取 | 日用量、周用量 |
| Manus | Cookies | Credits、重新整理 Credits |
| 小米 MiMo | Cookies | 餘額、Token 套餐 |
| Doubao | API Key | 請求限制 |
| Command Code | Cookies | 月度 Credits、已購 Credits |
| Crof | API Key | Credits、請求配額 |
| StepFun | Oasis Token | 5h、周用量 |
| Venice | API Key | USD / DIEM 餘額 |
| OpenAI | Admin API / API Key | 用量、請求數、餘額 |
| Grok | Cookies / auth.json | 賬單 |
| ElevenLabs | API Key | 訂閱 Credits、Voice Slots |
| Deepgram | API Key | 專案用量 |
| Groq | API Key | Enterprise Metrics |
| LLM Proxy | API Key | 配額統計 |

## 隱私

- **僅本機處理** — 不會將資料傳送到外部伺服器（提供者 API 除外）
- **不掃描磁碟** — 只讀取已知配置路徑和瀏覽器 Cookies
- **按需啟用** — 只有啟用相應提供者後才會提取 Cookies
- **受保護的憑據儲存** — 應用管理的 API Key、手動 Cookie 和權杖賬戶會寫入安全檔案層；Windows 上會優先使用當前使用者的 DPAPI
- **安全診斷** — 診斷快照只展示提供者、來源和狀態等後設資料，不展示原始 Cookie、API Key、Bearer Token 或 OAuth 值
- **已驗證更新** — 自動下載的安裝包需要 GitHub SHA-256 摘要，並會在應用前再次校驗

## 更多文件

| 主題 | 連結 |
|------|------|
| 從原始碼構建 | [docs/BUILDING.md](docs/BUILDING.md) |
| WSL 設定與認證 | [docs/WSL.md](docs/WSL.md) |
| 瀏覽器 Cookie 詳解 | [docs/COOKIES.md](docs/COOKIES.md) |

## 致謝

- **原版 CodexBar**：[steipete/CodexBar](https://github.com/steipete/CodexBar)，作者 Peter Steinberger
- **靈感來源**：[ccusage](https://github.com/ryoppippi/ccusage)，用於成本跟蹤思路

## 許可證

MIT — 與原版 CodexBar 保持一致

---

*如需原版 macOS 版本，請訪問 [steipete/CodexBar](https://github.com/steipete/CodexBar)。*
