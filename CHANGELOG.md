# Changelog

## [Windows] 0.55.0 - 2026-08-25

Windows port of upstream CodexBar **0.54.0 → 0.55.0**, plus the Windows Grok OAuth routing fix from issue #362.

### Added
- Command Code: individual-pro-v1 Pro tier with the current monthly credit grant.
- Cursor: Grok Bot usage window and tokscale-compatible local spend import.
- Antigravity: tokscale-compatible local spend, offline conversation fallback, agy authentication guidance, and retired Flash model aliases.
- Kiro: overage limits and charges from GetUsageLimits.
- z.ai: China account balance enrichment.
- Alibaba Token Plan: Personal/Solo SEC_TOKEN dashboard extraction.
- Codex: tokscale-compatible cached-token, bare-usage, stale-snapshot, and reasoning accounting parity.

### Changed
- Usage & Spend refreshes silently when provider data changes and loads independent provider baselines in parallel.
- OpenCodex bulk pricing reuses one shared models.dev pricing snapshot.
- OpenRouter management spend uses completed UTC days for the rolling 30-day window.
- Simplified Chinese session quota labels use explicit whole-hour durations and map weekly-duration windows to the weekly label.
- Agent session names truncate safely in the desktop UI.
- Single-meaningful-quota tray icons use the full meter while multi-quota icons keep distinct lanes.

### Fixed
- Grok preserves unknown-period usage instead of displaying it as zero.
- Grok Auto/OAuth routing works when browser cookies are disabled (#362).
- Alibaba retries successful-but-empty usage responses and sends navigation headers on the dashboard request that renders SEC_TOKEN.
- Gemini shutdown guidance preserves paid/workspace exclusions.

---

## [Windows] 0.54.0 - 2026-08-22

Windows port of upstream CodexBar **0.53.0 → 0.54.0**.

### Added
- Providers: Codex PAT (Personal Access Token) usage source with user-agent normalization and fallback narrowing (#353).
- Providers: Grok local session token scanning — reads local session files for Grok usage data (#353).
- Providers: OpenCode Go usage API — authenticated usage API with rolling/weekly/monthly windows, auto-fallback from local to API (#353).
- Providers: OpenRouter Activity spend — 30-day spend tracking via management API key, with daily breakdown, dedup, and conflict detection (#353).
- Providers: Antigravity idle-family filtering — idle window IDs filtered to family-level granularity for the dashboard (#353).
- Cost: Historical GPT-5.6 Terra/Luna pricing — date-gated rates: requests before 2026-07-30 use pre-cut pricing, after uses current rates (#353).
- Cost: OpenCodex spend routing — typed `RouteTarget` enum routes entries to the correct subscription (Codex, OpenCode Go, Kimi, DeepSeek) by model prefix first, then provider label (#353).
- Cost: xAI daily spend breakdown added to xAI spend snapshot (#353).
- Cost: Management API token — optional secondary management credential for providers that expose one (OpenRouter Activity) (#353).
- Settings: `show_pace` toggle for pace visualizations and forecast text in provider menu cards (default: on) (#353).
- Settings: OpenRouter management API key configuration in Settings → Providers (#353).

### Fixed
- Tray: Auto-fit ResizeObserver feedback loop fix — panel size adjusts correctly on content changes (#352).

### Changed
- Upstream 0.53.0 → 0.54.0 provider, cost, and CLI behavior ported onto the Windows tray/desktop shell (#353).

---

## [Windows] 0.53.0 - 2026-08-19

Windows port of upstream CodexBar **0.48.0 → 0.53.0**.

### Added
- Providers: Fireworks provider with 30-day rated billing spend (#337).
- Providers: Amp monthly renewals + 8PM NY free reset, LongCat token-packs summary, OpenCode pay-as-you-go billing fallback (#338).
- Providers: Cursor app session preference — reads Cursor's read-only `state.vscdb` for automatic session detection before browser cookies (#339).
- Providers: Kiro re-authenticate via `run_kiro_login()` wired into `trigger_provider_login` (#341).
- Providers: Grok source picker — Auto / Grok CLI / SuperGrok OAuth / Browser cookies (#346).
- Charts: Tokens/Cost chart switch with a **Tokens** tab in Settings, defaulting active for Codex; "Refreshing" marker for incomplete backfill (#339).
- Charts: Centered x-axis date labels on bar/point centers (#341).
- Cost: Explicit cost provenance and coverage — `SpendContract` with `CostProvenance` (ListPriceEstimate/VendorMetered/Mixed/Unknown) and `CostCoverageCounts`; partial totals show coverage, nothing unpriced masquerades as a bill (#346).
- Cost: Token mix, hourly activity heatmap, and conversation totals in UsageSpendTab (#346).
- Cost: All-time range (365 days) — `selectedDays: 0` maps to 365-day clamp (#346).
- Cost: Exact-match custom pricing overlays from `custom-pricing.json` — custom rates authoritative over models.dev/built-in (#346).
- Cost: Opt-in read-only OpenCodex import — reads `usage.jsonl`, deduplicates by `request_id`, aggregates into `ImportedSpendSource` with SQLite cache (#346).
- Cost: Per-project spend and Projects panel in UsageSpendTab (#344, #346).
- Cost: OpenCode Go local estimate source labeling (#344).
- Cost: CLI `codexbar usage --format toon` — TOON v4.1 JSON payload emission (#346).
- Cost: CLI `codexbar cost --json` aligned with spend provenance, token mix, coverage, and opt-in OpenCodex (#346).
- Low Power Mode Off/On/Automatic — `LowPowerModePreference` enum; Automatic reads Windows Battery Saver via `SystemStatusFlag` (#346).
- i18n: Turkish (tr-TR) locale support (#342).
- Settings: Light theme toggle fix (#343).
- Sessions: Cost CLI session grouping (#344).
- Serve: `--identity` now optional; follows `hide_personal_info` setting when absent (#341).
- z.ai: `CREDIT_LIMIT` coding plans now parse like `TOKENS_LIMIT` — 5-hour credit window drives primary % and reset (#337).
- Kimi: Official lane names + duplicate suppression — membership pool renamed to "Total usage", duplicate `Code 7-day` row hidden on positive evidence (#337).
- OpenRouter: Key-quota fast join on a 1-second deadline (was 3s) with explicit degraded-join logging (#337).
- Proof: Seed provider usage snapshot for deterministic proof runs (#336).
- FloatBar: Synced with menu bar metric preference (#335).
- Tray: `OverviewSpendSummary` component showing 30-day total spend across all providers in the tray panel header (#346).

### Fixed
- Claude: Prefer `limits[]` session over stale `five_hour` after 5h rollover (#334).
- Claude: Stop rescaling utilization below 1% to 100% (#274).
- Antigravity: Detect `agy` CLI in Antigravity provider on Windows (#333).
- Qwen Cloud: Promote weekly window to primary (#284).
- DeepSeek: Show peak and off-peak pricing (#280).
- Ollama: Cookie stripping — `strip_curl_cookie_wrapper`, `curl ` prefix + `Cookie:` label stripping in `normalize_cookie_header` (#341).
- Ollama: Cookie-expiry recovery — invalidate + browser re-import + retry (#338).
- Claude: Scope session-equivalent forecast history by account (#276).
- Menu-card: Detail wrapping for long metric reset/pace details — two-line clamp instead of ellipsizing (#337).
- Provider status: Last-success cache — transport failures never replace the last known status (#339).
- Codex: Custom-backend guidance — missing `auth.json` + custom backend reports rate limits unavailable instead of telling user to `codex login` (#339).
- Cursor: Rename locale keys — `weekly_label: "Auto"→"Cursor"`, `plan_period_label` → "Cursor and Third Party" (#341).
- OpenCode Go: 5h/weekly pace via `pace_json()` in CLI JSON + text output (#341).

### Changed
- Upstream 0.49.0–0.53.0 provider, cost, and CLI behavior ported onto the Windows tray/desktop shell (#337, #338, #339, #341, #344, #346).
- UsageSpendTab rebuilt with `SpendContractOverview` (provenance/coverage/conversations/token-mix metric cards), `ContractModelsPanel`, `ProjectsPanel`, and OpenCodex import toggle (#346).
- GeneralTab: Low-power mode Select dropdown replacing Toggle; legacy `low_power_mode` boolean kept for settings.json migration (#346).
- CI: GitHub Actions release workflow added as fallback to CircleCI, reusing existing release scripts (preflight, build, manifest, publish).
- CI: Cargo registry, target, and pnpm store caching added to release pipeline (#329).

---

## [Windows] 0.48.0 - 2026-08-09

Windows port of upstream CodexBar **0.47.0 → 0.48.0**.

### Added
- Providers: CodeBuddy CN credit usage (#269).
- Codex: multi-account management — Codex accounts panel in provider settings and tray menu flyout for switching accounts (#255–#260).
- Serve: modular usage dashboard (coordinator/snapshot structure with provider icons) from upstream 0.48.0 (#271).
- Sessions: unified Pi-family (pi + OMP) agent sessions with live-process correlation, PID-only rows, and a unified AgentSessions view; `sessions --json-v2` with legacy `--json` fallback negotiated over SSH (#271).
- Upstream 0.48.0: Copilot AI credits counter, CommandCode rolling windows + GOAT plan, OpenRouter key meter, ZoomMate browser cookie scope, OpenCode Go per-model daily cost breakdown (#271).
- Regional: Kimi Desktop monthly membership enrichment, GLM Coding Plan 5-hour/weekly windows, z.ai Global vs BigModel CN region routing with cross-region endpoint override rejection (#271).

### Fixed
- Codex weekly window detection when the 5-hour session window is missing (#268).
- MiniMax CN Token Plan web fetch via token-plan endpoints (#254, #262).
- Tray: auto-fit measure scaled by active zoom + proof anchor clamp (#265, #266).
- Tray: Codex accounts card overflow and weekly-only tray bar (#264).
- Tray: fractional-DPI flyout resize oscillation via two-state cycle detection (#261, #272).
- Upstream 0.48.0: bounded serve request heads, bounded Codex JSONL cost cache (oversize persistence refused), Claude OAuth terminal/transient refresh classification, Codex reset backfill (#271).

### Changed
- Upstream 0.48.0 provider and CLI behavior ported onto the Windows tray/desktop shell (#271).
- Repo automation: contributor interaction guard relaxed to a 14-day account-age floor and 15 PRs per 7-day window (#263).
- Docs: SignPath code-signing policy and privacy statement published; code signing is pending SignPath onboarding — release artifacts remain unsigned with SHA-256 sidecars (#270).

---

## [Windows] 0.47.0 - 2026-08-04

Windows port of upstream CodexBar **0.46.0 → 0.47.0**.

### Added
- Providers: Notion AI and xAI usage surfaces from upstream 0.47.0.
- CLI: hooks watch mode for live hook event streaming.
- Low Power Mode to reduce background refresh/work on battery or demand.
- Cost pace: real-calendar monthly pacing (not rolling 30-day only).
- Settings: per-notification custom sounds (#251).
- Russian localization (#248).

### Fixed
- MiniMax coding-plan web-session auth/status refresh (#250, #246).
- OpenCode Go rolling 5-hour usage falsely showed 100% "Exhausted" when the real value was 1% (#250, #247).
- FloatBar DPI-aware sizing on scaled displays (#245).
- Provider fixes: Cursor, CommandCode, OpenCode Go WAL handling, and ZAI/Kimi/Grok duration windows.

### Changed
- Upstream 0.47.0 provider and CLI behavior ported onto the Windows tray/desktop shell.

---

## [Windows] 0.46.0 - 2026-07-30

Windows port of upstream CodexBar **0.45.2 → 0.46.0**. macOS-only shell items (WidgetKit, Sparkle, AppKit menu layout, Homebrew, Safari cookie APIs) remain deferred.

### Added
- Providers: Qwen Cloud (Individual Token Plans with 5-hour and weekly rolling windows) and ZoomMate (credit status with host failover, cURL/cookie auth, bearer mint).
- Alibaba Token Plan: Personal/Solo variants for mainland (Bailian) and international (Model Studio) accounts via `alibaba_token_plan_region` setting.
- Claude: prepaid credit balance in cost surfaces from cached/manual web sessions.
- Claude: setting to hide the Daily Routines row (`claude_daily_routines_usage_visible`, default on).
- Menu: fractional session-quota estimates on the weekly row ("Estimated: N session quotas left") when session+weekly history qualifies; optional `weekly_progress_work_days`.
- Codex: local Workspaces indexing foundation — per-project/session/model usage attribution with a local sidecar index; new `codexbar workspaces` CLI and desktop snapshot bridge.
- CLI: `config dump` redacts stored credentials by default; `--show-secrets` restores raw output.

### Changed
- Codex local cost scans use the disk cache: unchanged files skipped by mtime/size, grown logs resumed mid-file, 256 KiB line bound — repeat scans are incremental.
- Automatic tray metric surfaces the highest-used (exhausted) window across providers, preserving per-provider overrides.
- CLI alias `qwen` now resolves to Qwen Cloud (use `alibaba` for the Coding Plan).

### Fixed
- Claude: model-scoped weekly rows above Daily Routines; automatic metric prefers account Weekly over exhausted model carve-outs; learned full-session estimate stays visible while the session window is idle.
- Amp: subscription plans (e.g. Megawatt) parse into Other/Orb percentage windows instead of a misleading cookie error.
- Grok: explicit cookie refresh with validated session caching for background reuse.
- Chutes: quota counts render as detail text instead of being misread as reset schedules.
- LLMProxy: skip already-elapsed reset times when picking the next reset.
- Ollama: reuse validated browser sessions across refreshes.

---

## [Windows] 0.45.3 - 2026-07-29

### Added
- Settings: global HTTP proxy for provider traffic (#235, #236).
- Codex tray: reset-credit **next expiry** on the Reset credits row (count + “Next expires …”; honors Relative reset time) (#241).

### Fixed
- Tray icon missing from the taskbar after upgrade (#237, #238).
- Float bar window flipping Settings Auto theme to light (pin WebView2 dark on float bar) (#240).

### Changed
- Hosted PR check on Blacksmith Windows (fmt, clippy, tests, frontend build); release packaging remains local.
- Dead-code and helper cleanup (shared SigV4/parse helpers, drop unused CLI args/bridge bindings/icons registry, simplify build scripts).

---

## [Windows] 0.45.2 - 2026-07-21

Windows port of upstream CodexBar **0.43.0 → 0.45.2** (providers, CLI, cost, and settings surfaces).
macOS-only shell polish (menu-bar layout editor, widgets, full hooks UI chrome) remains deferred.

### Added
- Providers: ZenMux, ClinePass, LongCat, Neuralwatt, DeepInfra, and ai& (30-day spend from logs).
- Doubao: `arkcli usage plan` path with Coding / Agent / team plan windows.
- OpenRouter: multi-key token accounts and CLI `usage --account`.
- OpenCode Go: local-first Auto from `opencode.db` (web-first when scoped).
- CLI: `codexbar guard` quota automation gates; `codexbar hooks list|enable|disable|test`.
- CLI serve: optional `--host`, `--dashboard-token` / `CODEXBAR_DASHBOARD_TOKEN`, and `--allow-plain-http` for LAN binds.
- Settings: Usage & Spend tab (7d/30d) with sanitized PNG share-card export.
- Settings: Adaptive refresh cadence (activity/power-aware) with Windows coding-agent process detection.
- External hooks: opt-in `hooks.json` runner (no shell) with threshold-crossing dispatch.
- Cost: OMP (`~/.omp`) and pi-compatible (`~/.pi`) agent session logs without double-counting.
- Cursor: dashboard usage-events token-cost (per-model API-rate windows + metered totals).

### Changed
- Soft-remove Kimi K2 and CrossModel (deprecated labels; hidden unless already enabled; CLI still resolves).
- Prefer active OpenRouter token-account keys over stored single API keys.

### Fixed
- Claude: prefer `weekly_all` over misleading `seven_day.utilization`; last-good CLI usage on parse fail; probe session-id reuse (#210, #216).
- OpenCode: do not rescale sub-1% computed usage to 100% (#211, #215 / upstream #2331).
- Cursor: map Cli to web cookie path; clamp plan usage at 100% (#212, #217 / upstream #2255).
- Cost scanner: process incomplete final JSONL lines for Claude transcripts.

---

## [Windows] 0.43.0 - 2026-07-17

### Added
- Add the sub2api provider (group-key gateway usage, subscription, and wallet).
- Add Factory/Droid API-key Auto with web-cookie fallback, dotenv key resolution, and token-rate-limits billing.
- Reuse fresh Kimi Code CLI OAuth credentials read-only when no API key is configured.
- Treat Claude web null five-hour session as an informational placeholder (no phantom 0% quota).
- Promote the tray icon out of the Windows 11 hidden-icons overflow when enabled.
- Toggle the tray flyout from the global shortcut.

### Changed
- Scope quota threshold and session-depleted notifications by account identity.
- Improve cost-scan freshness: unattributed model pricing, 256 KiB JSONL line bounds, and Ultra lineage watermark containment.
- Port remaining upstream 0.43.0 residuals (Claude passive-probe env, nested Factory usage shapes).
- Migrate CI workflows to Blacksmith runners.
- Complete zh-CN / zh-TW / ja-JP localization coverage and wire remaining frontend strings into the locale system.

### Fixed
- Prevent stale provider publish after enablement changes (refresh generation ownership).
- Honor Factory cookie-source off (API-only; no browser scrape).
- Avoid re-arming still-hot notifications from volatile plan/login identity strings.
- Skip informational primaries in Session and Automatic tray metrics.
- Fix FloatBar recovery after minimize, monitor disconnect, and taskbar z-order races.
- Fix weekly toast spam, OpenCode usage parse, Antigravity 2.0 port detection, Z.ai quota parsing, and predicted exhaustion display.

---

## [Windows] 0.42.0 - 2026-07-12

### Added
- Add the Wayfinder provider.
- Add opt-in local and SSH Agent Sessions for Codex and Claude.
- Add predictive pace warnings and provider/window-specific usage thresholds.
- Add GPT-5.6 Sol, Terra, and Luna pricing and model aliases.
- Show Claude model-scoped weekly quotas from OAuth, web, and CLI sources.

### Changed
- Reorganize Settings and restore Providers as a dedicated tab.
- Keep every Settings tab at the same window size.
- Improve token-cost pricing coverage, freshness, and refresh coalescing.

### Fixed
- Keep refresh intervals anchored and provider cards synchronized with completed work.
- Avoid quota refreshes for visual-only settings and provider reorder changes.
- Improve Antigravity detection, Gemini paid-tier labels, Ollama authentication, and other provider parsing.
- Eliminate the duplicate FloatBar cost control and keep local-cost display opt-in.

---

## [Windows] 0.41.3 - 2026-07-11

### Added
- Add local cost summaries to FloatBar.
- Add a PowerToys Command Palette status pipe.

### Fixed
- Keep FloatBar topmost without repeatedly stealing focus.
- Preserve the OpenCode Go workspace override during authentication.
- Open Cursor's usage dashboard at the correct URL.
- Align the popup correctly with a side-mounted Windows taskbar (#159).
- Correct Codex daily usage reporting (#153, thanks @0reki).

---

## [Windows] 0.41.2 - 2026-07-08

### Added
- Add Antigravity `agy` CLI alias setup guidance.
- Add Traditional Chinese (Taiwan) localization.

### Changed
- Add repository interaction guardrails and switch the tray panel to a masonry card layout.

---

## [Windows] 0.41.1 - 2026-07-08

### Fixed
- Localize the tray panel and native tray menu for Japanese, Chinese (Simplified), Korean, and Spanish.
- Add missing `es-MX` UI key translations.
- Stop baking the UI language into cached provider snapshots so locale changes remain independent of provider data.
- Localize the native tray menu proof harness.

---

## [Windows] 0.41.0 - 2026-07-07

### Changed
- Port scoped upstream CodexBar 0.41.0 Rust/provider updates into the Windows/Tauri app.

### Fixed
- Fix the Windows tray/background launch auto-popup regression from #129 and fix Kimi auth cookie fallback.

---

## [Windows] 0.38.3 - 2026-07-06

### Fixed
- Fix NanoGPT usage parsing when the API omits the monthly usage block.

---

## [Windows] 0.38.2 - 2026-07-05

### Fixed
- Fix tray flyout flicker/hide when opened from the Windows tray overflow.

---

## [Windows] 0.38.1 - 2026-07-04

### Changed
- Bump the Windows/Tauri release version to 0.38.1 after merging the tray flyout, vertical-taskbar placement, Claude OAuth refresh, Claude cost, README language, and repo cleanup fixes.

### Fixed
- Port upstream 0.38.1 parser hardening for OpenAI API non-finite cost values, OpenCode reset timestamps, and z.ai BigModel CN quota responses without optional messages.

---

## [Windows] 0.38.0 - 2026-07-03

### Added
- Port upstream v0.38.0 provider support for CrossModel, Qoder, and Sakana AI into the Windows/Tauri app.
- Add Tauri provider icons, provider catalog metadata, manual cookie support for Qoder/Sakana, and saved API-key settings for CrossModel.

### Fixed
- Accept current Command Code `commandcode_prod` manual cookie headers.

---

## [Windows] 0.37.6 - 2026-07-02

### Added
- Add window mode with a taskbar-visible PopOut window, custom title bar controls, maximize/restore behavior, and display scaling.
- Add Mexican Spanish (`es-MX`) locale support with a centralized backend language catalog.

### Fixed
- Ship the installed console CLI as `codexbar-cli.exe` and verify it is a real console-subsystem binary with redirected stdout.
- Keep browser cookie imports scoped to exact provider domains and validate imported cookie header length before saving.
- Validate provider workspace/base URL extras in Rust before persistence so saved credentials cannot be retargeted to unsafe endpoints.
- Allow clearing the global shortcut setting without attempting to register an empty shortcut.
- Mask Unicode API keys without slicing through UTF-8 boundaries.
- Resolve automatic theme mode from the current OS light/dark preference.
- Fix the cost scanner token-count regression test so it does not age out of the 30-day scan window.

### Changed
- Harden CI and release workflows with pinned GitHub Action SHAs, read-only build permissions, and separate release publishing jobs.
- Package `codexbar.exe` as the desktop app, `codexbar-cli.exe` as the console CLI, and `codexbar-desktop.exe` as a compatibility alias.

---
## [Windows] 0.37.5 - 2026-06-27

### Fixed
- Fix Windows desktop startup paths that could leave CodexBar running with only the tiny internal Tauri shell window visible.
- Reopen the tray panel for normal or blank-argument desktop launches unless **Start Minimized** is enabled.
- Recover startup tray reveals that remain hidden or stuck at a tiny shell-window size.

---

## [Windows] 0.37.4 - 2026-06-24

### Changed
- Remove stale release scripts, unused fetch planning code, the fake Synthetic provider, and dead settings toggles.

---

## [Windows] 0.33.2 - 2026-06-12

### Fixed
- Hide the tray panel when it loses focus, matching normal tray-popover behavior.
- Allow Escape to dismiss the tray panel without quitting the app.
- Prevent the tray icon click that caused a blur-dismiss from immediately reopening the panel.

---

## [Windows] 0.33.1 - 2026-06-11

### Fixed
- Show GitHub Copilot over-budget quota values when GitHub reports negative remaining quota, such as displaying `115% used` instead of clamping to `100%`.
- Keep Copilot progress bars visually capped at full width while preserving the true overage percentage in tray, pop-out, provider sidebar, and settings details.

---

## [Windows] 0.33.0 - 2026-06-11

### Added
- Add Japanese as a selectable interface language in the Tauri Settings UI.

### Changed
- Port upstream CodexBar 0.33.0 provider and cost-accounting fixes into the Windows/Tauri Rust backend.
- Route provider HTTP clients through a shared same-origin redirect policy so credentialed requests do not follow cross-origin redirects with provider auth context.
- Update Claude local cost pricing for Fable 5, Opus 4.6, Sonnet 4.6, and 1-hour cache writes.

### Fixed
- Avoid showing Doubao API keys as falsely exhausted when Ark returns successful zero-remaining request-limit headers that are not reliable quota state.
- Preserve existing Copilot unlimited-chat and Antigravity untracked-quota behavior from the upstream 0.33.0 cycle.

---

## [Windows] 0.32.9 - 2026-06-11

### Fixed
- Apply Display settings changes to the native tray immediately after saving, including tray metric mode, highest-usage selection, percent icon mode, provider metric preferences, and enabled-provider changes.
- Make the **Show provider icons** setting affect the tray and pop-out provider switcher grids instead of only being stored.
- Make **Show percent in tray** render a real numeric tray icon.
- Make **Tray icon mode** affect native tray status rows by switching between a single summary row and per-provider rows.
- Shorten native tray tooltip reset text to relative countdowns such as `resets in 2h 05m` and bound long tooltip lines so Windows does not trim provider status text mid-line.

---

## [Windows] 0.32.8 - 2026-06-09

### Changed
- Install `codexbar.exe` as the tray app and `codexbar-cli.exe` as the console CLI so Start Menu shortcuts launch the desktop UI while terminal diagnostics print real output.
- Build the console CLI during every Windows release packaging run.

### Fixed
- Run provider auto-refresh from the Tauri backend even when the tray panel is closed and the floating bar is disabled.
- Keep `refresh_interval_secs = 0` as manual-only and prevent overlapping background refreshes.
- Extend Windows smoke install validation to prove installed CLI `--version` and `--help` output.

---

## [Windows] 0.32.7 - 2026-06-08

### Added
- Expand Alibaba Coding Plan support with selectable Singapore, US, Germany, Hong Kong, and China Mainland regions.

### Changed
- Route Alibaba Coding Plan cookies, dashboard links, gateway requests, and SEC_TOKEN caching through a canonical region model so region behavior stays consistent.

### Fixed
- Fix Cursor usage percentages by trusting Cursor's `totalPercentUsed`, `autoPercentUsed`, and `apiPercentUsed` fields as 0-100 percentages instead of recalculating or multiplying them.
- Keep Cursor bonus-credit breakdown totals from distorting fallback percentage calculations.

---

## [Windows] 0.32.6 - 2026-06-05

### Changed
- Reveal the Windows tray panel only after the frontend completes its first layout pass, avoiding the blank backing-frame flash on tray startup.
- Lazy-load heavier secondary surfaces so first tray activation does not compete with Settings, Pop Out, or Floating Bar module startup.
- Limit concurrent provider refreshes and emit provider-updated events after releasing the provider-cache lock to reduce refresh contention.
- Keep tray and pop-out provider ordering aligned with the configured provider catalog order.

### Fixed
- Restore the full bootstrap bridge contract for surface modes, commands, and events instead of exposing test-only descriptors.
- Keep dense tray overview layout visible while provider data is still loading, using stable placeholders instead of waiting indefinitely for the first providers to fetch.
- Handle surface-state mutex errors at the Tauri command boundary instead of panicking.
- Fix NanoGPT monthly-only usage parsing.

---

## [Windows] 0.32.5 - 2026-06-02

### Fixed
- Treat GitHub Copilot Business token-based billing zero-entitlement quota rows as unavailable instead of showing misleading `0% used` usage.
- Keep percent-only Copilot quota snapshots and fully consumed positive-entitlement quotas working while dropping only explicit zero-entitlement placeholders.
- Prioritize OpenAI Web login and Cloudflare blocking states over public-route detection so blocked dashboard responses do not get misclassified.

---

## [Windows] 0.32.4 - 2026-06-02

### Fixed
- Fix OpenRouter credits fetching by routing requests to the canonical `/api/v1/credits` endpoint instead of the broken `/api/v1/auth/credits` path.
- Align OpenRouter key introspection with the upstream `/api/v1/key` endpoint and add regression coverage for both endpoint URLs.

---

## [Windows] 0.32.3 - 2026-06-01

### Fixed
- Detect modern Chrome/Edge `v20` App-Bound encrypted cookies during browser import so Codex/ChatGPT imports no longer misreport protected signed-in sessions as missing cookies.
- Replace the outdated Chromium cookie-import guidance with a clearer manual-cookie or Firefox fallback when Windows browser encryption blocks direct import.

---

## [Windows] 0.31.1 - 2026-05-30

### Fixed
- Fix Antigravity usage on Windows when the local language server binds its API to a random listening port instead of a port near `--extension_server_port`.
- Prefer the Antigravity language-server process's actual listening ports before falling back to heuristic API port probes.

---

## [Windows] 0.31.0 - 2026-05-29

### Added
- Support AWS Bedrock usage through named AWS CLI profiles, including SSO, assume-role, and credential-process profiles that `aws configure export-credentials` can resolve.
- Show Codex Spark 5-hour and weekly quota lanes from ChatGPT/Codex `additional_rate_limits` payloads.

### Changed
- Port upstream CodexBar 0.31.0 provider behavior into the Windows/Tauri Rust backend while keeping macOS-only AppKit menu and Homebrew changes out of the Windows shell.
- Make local Codex/Claude chart scans cancellation-aware so repeated chart refreshes stop obsolete JSONL scans sooner.
- Document Bedrock profile credentials in the provider settings help text.

### Fixed
- Hide Claude's obsolete Design quota lane while preserving the remaining OAuth apps and Daily Routines usage lanes.

---

## [Windows] 0.30.4 - 2026-05-29

### Added
- Add `codexbar diagnose`, a generic safe provider diagnostic export that reports provider/source/config/fetch health without exposing cookies, tokens, account emails, or raw secrets.

### Changed
- Port upstream CodexBar 0.30.1 provider diagnostics behavior to the Rust CLI while omitting macOS-only AppKit status-item handling that has no Windows Tauri equivalent.
- Add trailing breathing room to Providers settings sidebar rows so row controls do not crowd the scrollbar.

### Fixed
- Treat Claude OAuth usage HTTP 429s as rate limits, preserve cached credentials, and back off repeated background retries.
- Reopen the tray panel from Windows shortcut/tray activation when the app is hidden, keep Claude usage on the current OAuth API path when no manual cookie is configured, and avoid flashing console windows during Windows CLI path probes.

---

## [Windows] 0.29.0 - 2026-05-24

### Added
- Port upstream CodexBar 0.29 Alibaba Token Plan support to the Rust/Tauri provider registry, protected token-account storage, provider icon registry, and CLI aliases.
- Show OpenCode and OpenCode Go renewal dates as a separate **Renews** usage window when their usage payloads expose `renewAt` / `renew_at`.
- Split local Codex cost output into standard vs fast/priority buckets when local session logs expose fast model naming.

### Fixed
- Preserve the upstream 0.29 quote-handling intent through the existing Rust secret cleanup paths while keeping provider tokens out of frontend state and logs.

---

## [Windows] 0.28.0 - 2026-05-24

### Added
- Port upstream CodexBar 0.28 Azure OpenAI and T3 Chat provider support to the Windows/Tauri provider registry, Settings UI metadata, protected credential storage, and CLI aliases.
- Add Ollama API-key support while keeping browser-cookie usage available as the Web source.

### Fixed
- Harden OpenAI dashboard account scraping and MiniMax billing aggregation for additional upstream response shapes.

---

## [Windows] 0.27.4 - 2026-05-23

### Added
- Add Floating Bar light-background mode for better contrast on bright desktops.
- Show localized provider reset timing in Floating Bar pill tooltips.

### Fixed
- Apply the Floating Bar opacity slider through the rendered bar so visual opacity updates reliably.
- Keep the Floating Bar release build compiling on the repo's active Tauri 2.10 API surface.

---

## [Windows] 0.27.3 - 2026-05-21

### Fixed
- Prefer Claude browser-session usage over Claude OAuth in Auto mode so the app follows the same settings-page endpoint as `claude.ai/settings/usage`.
- Use Claude's `lastActiveOrg` cookie or account memberships before falling back to the organizations list, which keeps multi-org accounts aligned with the active Claude web session.
- Parse Claude `seven_day_oauth_apps` and embedded `extra_usage` payloads from the web usage response.

---

## [Windows] 0.27.2 - 2026-05-20

### Added
- Add GitHub device-code sign-in for Copilot in the Tauri provider settings, storing the OAuth token as a protected Copilot token account.
- Reuse `gh auth token` as a Copilot auth fallback so existing GitHub CLI logins can power Copilot usage without pasting a token.

### Fixed
- Parse Copilot plan usage across paid `premium_interactions` / `chat` snapshots and free-plan `monthly_quotas` / `limited_user_quotas` responses.
- Show Copilot as OAuth-backed in the Providers UI while keeping the legacy manual token path as an optional fallback.

---

## [Windows] 0.27.1 - 2026-05-19

### Added
- Complete the upstream CodexBar 0.27 provider port for Windows/Tauri by adding Grok billing support, Claude Admin API usage, OpenAI Admin API usage with legacy credit-balance fallback, MiniMax billing summaries, OpenCode Go Zen balance display, and Kiro overage usage/cost parsing.
- Add Grok across the Rust provider registry, credential migration, token-account support decisions, Settings provider catalog, Tauri provider unions, provider icon registry, and chart colors.
- Add `codexbar serve` for loopback `/health`, `/usage`, and `/cost` JSON, with loopback Host-header validation.
- Add the upstream-compatible `--all-accounts` CLI flag surface.

### Fixed
- Correct the Windows 0.27 line so the release reflects the full portable upstream provider/CLI changes instead of only the API-key quota-provider subset.
- Update README provider counts and v0.27 notes to describe the full Windows/Tauri port.

---

## [Windows] 0.27.0 - 2026-05-19

### Added
- Port upstream CodexBar 0.27 API quota providers for ElevenLabs subscription credits, Deepgram project usage, GroqCloud Enterprise Prometheus metrics, and LLM Proxy quota-stats.
- Add the v0.27 provider wiring across the Rust registry, Settings API-key catalog, Tauri provider icons, chart colors, dashboard/status links, and frontend provider unions.
- Add `codexbar config providers`, `codexbar config enable`, `codexbar config disable`, and `codexbar config set-api-key` for scriptable provider setup.

### Fixed
- Keep API-key provider refreshes on the Windows/Tauri API source path and add deterministic parser coverage for the new providers.

---

## [Windows] 0.26.3 - 2026-05-17

### Fixed
- Fix DeepSeek refresh in the desktop app when an API key is configured by keeping API-key providers on the automatic API source path instead of falling back to unsupported CLI mode.
- Stop writing raw CLI argument values to startup diagnostics and use per-process launch log files to avoid leaking tokens or mixing concurrent launch logs.

---

## [Windows] 0.26.2 - 2026-05-16

### Added
- Add an optional always-on-top Floating Bar that shows remaining provider capacity in a compact transparent strip.
- Add Floating Bar display settings for enablement, horizontal/vertical orientation, opacity, and click-through overlay mode.
- Add native tray menu support for toggling the Floating Bar while keeping the tray check state in sync with Settings changes.

### Fixed
- Make the Settings tab's Quit button close only the Settings window so the tray service keeps running.
- Keep tray panel, pop-out, and native tray menu Quit actions on the app-level exit path.
- Keep Floating Bar updates live when provider enablement, refresh cadence, or usage thresholds change.
- Avoid Windows Tauri deadlocks by opening the Floating Bar from async command paths.

---

## [Windows] 0.26.1 - 2026-05-15

### Fixed
- Preserve Moonshot / Kimi API compatibility across international and China-region API keys by trying both endpoints when `MOONSHOT_API_REGION` is unset.
- Keep explicit `MOONSHOT_API_REGION=international` and `MOONSHOT_API_REGION=china` region pinning for users who want a single endpoint.

---

## [Windows] 0.26.0 - 2026-05-15

### Added
- Port upstream CodexBar 0.26 AWS Bedrock monthly spend tracking into the Windows/Tauri Rust backend using AWS Cost Explorer and SigV4 request signing.
- Add Bedrock provider metadata, CLI aliases, Settings provider catalog support, frontend provider types, source hints, and provider icon registry entry.
- Add OpenRouter daily, weekly, and monthly API-key spend windows from the `/api/v1/auth/key` endpoint.
- Add Moonshot / Kimi API balance parsing for international and China API regions, voucher balance, cash balance, and deficit state.

### Changed
- Stop shipping `WebView2Loader.dll` in Windows installer and portable packages because MSVC release builds statically link the WebView2 loader.
- Restore portable release packaging to a standalone `CodexBar-<version>-portable.exe` asset instead of a zip bundle.
- Update app, CLI, package, Tauri, and release metadata to 0.26.0 for the Windows artifact release.

### Notes
- Upstream 0.26 also includes several macOS/Swift-only menu, Sparkle, localization, and native settings changes. Those code paths do not exist in Win-CodexBar's Tauri/Rust shell.

---

## [Windows] 0.25.1 - 2026-05-11

### Changed
- Align the Windows/Tauri release with upstream CodexBar 0.25.1 after reviewing the upstream patch set.
- Bump app, CLI, package, Tauri, and release metadata to 0.25.1 for the follow-up Windows artifact release.

### Notes
- Upstream 0.25.1 fixes macOS SwiftPM localization bundle lookup, macOS Keychain cache prompt churn, Pi session cost cache migration, Swift concurrency annotations, and standalone Swift CLI archive version fallback. Those code paths do not exist in the Windows/Tauri port, so no runtime Rust/Tauri logic change was required beyond the release alignment.

---

## [Windows] 0.25.0 - 2026-05-11

### Added
- Port upstream CodexBar 0.25 provider support for Manus, Xiaomi MiMo, Doubao, Command Code, Crof, StepFun, Venice, and OpenAI API balance into the Windows/Tauri app.
- Add v0.25 providers to the Rust provider registry, Settings provider list, credential/API-key catalog, CLI aliases, cookie/token-account handling, and provider icon registry.
- Add credit, request, refresh-credit, token-plan, purchased-credit, DIEM/USD balance, and OpenAI API credit-grants usage snapshots.

### Changed
- Update the provider catalog, CLI metadata, frontend provider unions, and release docs for 40 supported providers.

---

## [Windows] 0.24.0 - 2026-05-10

### Added
- Port upstream CodexBar 0.24 provider support for Codebuff, DeepSeek, and Windsurf into the Windows/Tauri app.
- Add Codebuff and DeepSeek API-key setup to Preferences, including provider icons, chart colors, CLI aliases, and release metadata.
- Add Windsurf local cached-plan usage reading from the Windows application data path.

### Changed
- Update the provider catalog, CLI help text, credential metadata, frontend provider unions, and release docs for 32 supported providers.

---

## [Windows] 0.23.11 - 2026-05-10

### Fixed
- Handle Claude Web usage payloads that include overlapping design or routines alias fields without failing with a duplicate-field parse error.
- Keep Claude Web parse diagnostics useful without exposing raw response bodies in user-facing errors or logs.

---

## [Windows] 0.23.10 - 2026-05-06

### Fixed
- Route active Claude OAuth token accounts through OAuth mode and pass the selected token directly into the Claude OAuth fetcher.
- Keep Claude `sessionKey` token accounts on the web/cookie path instead of confusing them with OAuth tokens.
- Report OAuth, Web, and CLI failures together in Claude Auto mode so a final CLI parse error no longer hides earlier token or cookie failures.

---

## [Windows] 0.23.7 - 2026-05-03

### Fixed
- Parse Claude CLI's exhausted `You've hit your limit · resets ...` short form as full session usage instead of reporting `Claude CLI did not return usage data`.
- Make Claude CLI usage parsing more tolerant of compact labels, decimal percentages, and remaining/available wording.
- Keep weekly reset lines from being promoted into the session reset when the session section has no reset.

### Security
- Re-enable the Tauri content security policy and disable global Tauri injection.
- Narrow the default Tauri capability permissions to the event, window, and global shortcut APIs the frontend actually uses.
- Harden external URL opening by validating web URLs and avoiding `cmd /c start` on Windows.

---

## [Windows] 0.23.5 - 2026-04-29

### Added
- Add safe diagnostics and credential storage status reporting without exposing secret values.
- Add a Windows installer smoke-test script for silent install, installed-file, registry, shortcut, and uninstall validation.

### Changed
- Reuse fresh provider refresh results during startup and panel opening to reduce avoidable provider fetches.

### Fixed
- Redact secret-like values from provider refresh errors before they cross the Tauri bridge.
- Re-verify downloaded installer SHA-256 hashes immediately before applying an update.
- Harden desktop command inputs for provider IDs, credential values, cookie source values, region values, token accounts, and filesystem paths.

---

## [Windows] 0.23.4 - 2026-04-29

### Security
- Default browser-cookie usage to manual mode so provider refreshes no longer read and decrypt browser cookie stores unless the user explicitly selects Automatic or imports cookies.
- Respect manual/off cookie-source settings when building provider fetch contexts, reducing behavior-based antivirus triggers around DPAPI browser-cookie access.
- Save local secret-bearing files through a secure-file wrapper; Windows writes are protected with DPAPI while existing plaintext files remain readable for migration.
- Redact raw provider response bodies and browser cookie-store paths from routine diagnostic logs.

---

## [Windows] 0.23.3 - 2026-04-29

### Fixed
- Ship `WebView2Loader.dll` beside `codexbar.exe` in the Windows installer so clean installs can launch the Tauri shell.
- Replace the standalone portable executable release asset with `CodexBar-<version>-portable.zip`, which includes both `codexbar.exe` and `WebView2Loader.dll`.
- Add release workflow checks that fail the build when the WebView2 runtime sidecar is missing.

### Superseded
- Later Windows MSVC builds statically link the WebView2 loader, so release packaging no longer needs to ship `WebView2Loader.dll` beside `codexbar.exe`.

---

## [Windows] 0.23.2 - 2026-04-28

### Fixed
- Accept a raw `__Secure-session` value for Ollama Cloud manual cookies instead of requiring a full `Cookie` header.
- Normalize Ollama token-account entries the same way, so saved accounts can use either raw `__Secure-session` values or full cookie headers.
- Clarify the Ollama cookie placeholder in the desktop settings UI.

---

## [Windows] 0.23.1 — 2026-04-26

### Fixed
- Add the provider Settings picker for the tray/menu bar metric so the Windows frontend can choose session, weekly, model-specific, tertiary, average, or Cursor extra-usage display modes.
- Make the tray icon respect per-provider metric preferences, including Cursor on-demand budget and legacy credits settings.

---

## [Windows] 0.23.0 — 2026-04-26

### Upstream 0.23 Parity
- Add Mistral usage support with monthly spend parsing from the Mistral Admin billing API, browser-cookie/manual-cookie auth, token-account storage, and provider branding.
- Add Claude Designs and Daily Routines usage windows when Claude OAuth/Web quota payloads include those limits.
- Add GPT-5.5 and GPT-5.5 Pro pricing for local Codex cost scanning.
- Prefer Cursor on-demand budget data for the extra/monthly cost metric when Cursor returns it.

### Windows Release
- Bump the Tauri desktop and shared Rust crate to `0.23.0`.
- Keep macOS-only upstream 0.23 work out of the Windows port: WidgetKit metadata, Sparkle appcast, AppKit menu sizing, and full-screen confetti are not applicable here.

---

## [Windows] 0.22.1 — 2026-04-24

### Fixed
- Stabilize the tray panel height measurement so provider refreshes and provider selection no longer visibly jump or re-anchor the popup.
- Close the tray panel when opening Settings or About so those windows can take focus cleanly.
- Keep the Windows DWM helper clean under `cargo clippy --all-targets -- -D warnings`.

---

## [Windows] 0.22.0 — 2026-04-23

### New Providers
- Perplexity: cookie-based credits tracking (recurring/bonus/purchased), Pro/Max plan detection
- Abacus AI: cookie-based compute points + billing tier fetch
- OpenCode Go: cookie-based workspace usage (rolling/weekly/monthly windows)
- Kilo: API-key tRPC batch (env/keyring/auth.json), credit blocks + Kilo Pass

### Provider Updates (upstream 0.18–0.22 parity)
- Claude: broader CLI lookup (Volta, fnm, npm-global), status page URL fix
- Codex: Pro Lite/Go/Quorum/K12 plan types, dashboard URL, weekly-only rate limits
- Cursor: defensive JSON parsing with text fallback
- Synthetic: 3-slot quota (5-hour, weekly, search limits)
- Antigravity: extension_server_csrf_token extraction and fallback probing
- z.ai: dual TOKENS_LIMIT (weekly + 5-hour session), TIME_LIMIT, plan name
- Ollama: validate session cookie names
- OpenCode: expanded percent/reset key variants, absolute resetAt support
- Alibaba: region-aware endpoints (international/China), multi-domain cookies
- Copilot: verification_uri_complete for pre-filled device login URL
- Gemini: OAuth credential discovery from CLI paths (Homebrew/npm/Nix/Bun/Volta)

### Pricing & Models
- Fix stale GPT-5.4/5.4-mini/5.4-nano pricing
- Add 10 new Codex models (gpt-5-mini, gpt-5-nano, gpt-5-pro, gpt-5.1-codex, etc.)
- Add Claude Opus 4.7 and Claude Sonnet 4.6 pricing
- Add displayLabel field to CodexPricing (for Research Preview tags)

### UI
- Add keyboard shortcuts: Ctrl+R (Refresh), Ctrl+, (Settings), Ctrl+Q (Quit)
- Show shortcut hints in footer menu items
- Update PopOutPanel shortcuts from macOS ⌘ to Windows Ctrl+
- Fix settings window resize (preserve WS_THICKFRAME in DWM caption hack)
- Fix async race conditions on provider switching (stale response guards)
- Fix error visibility in API key section
- Fix GDI brush leak in DWM dark caption

### Repo Cleanup
- Remove legacy egui shell (22,539 lines of dead code)
- Rewrite README with extra-docs split (WSL, Building, Cookies)
- Fresh Windows screenshots
- Fix CI target paths for workspace layout
- Release workflow now builds Tauri app as codexbar.exe
- Add frontend CI job, Rust/npm caching, Dependabot

---

## [Windows] 1.0.2 — 2026-01-24

### UI Redesign
- Redesign main UI with 4-column grid layout for provider tabs
- Replace amber progress bars with blue color scheme
- Add section headers with chevron indicators
- Increase font sizes across all tiers for better readability
- Disable window state persistence to prevent size corruption

### Settings Page
- Complete redesign with "precision calm" aesthetic
- Underline-style tab navigation
- Settings cards with grouped settings and dividers
- Left accent bars on API key cards for status indication
- Reusable helper components for consistent styling

### New Provider
- Add JetBrains AI provider support with usage tracking
- Support aliases: jetbrains, jetbrains-ai, intellij
- Add JetBrains icon and brand color to theme

### Housekeeping
- Remove development screenshots from repository

---

## 0.18.0 — Unreleased
### Providers
- Claude: harden Windows CLI detection, prefer `.cmd` wrappers on PATH, and surface clearer startup errors for Git Bash / PowerShell wrapper failures.
- OpenCode: add web usage provider with workspace override + Chrome-first cookie import (#188). Thanks @anthnykr!
- Providers: cache browser cookies on disk (per provider) and show cached source/time in settings.
- Vertex AI: add provider with quota-based usage from gcloud ADC. Thanks @bahag-chaurasiak!
- Vertex AI: token costs are shown via the Claude provider (same local logs).
- Vertex AI: harden quota usage parsing for edge-case responses.
- Kiro: add CLI-based usage provider via kiro-cli. Thanks @neror!
- Kiro: clean up provider wiring and show plan name in the menu.
- Augment: add provider with browser-cookie usage tracking.
- Cursor: support legacy request-based plans and show individual on-demand usage (#125) — thanks @vltansky
- Cursor: avoid Intel crash when opening login and harden WebKit teardown. Thanks @meghanto!
- Cursor: load stored session cookies before reads to make relaunches deterministic.
- Codex/Claude/Cursor/Factory/MiniMax: cookie sources now include Manual (paste a Cookie header) in addition to Automatic.
- Codex/Claude/Cursor/Factory/MiniMax: skip cookie imports from browsers without usable cookie stores (profile/cookie DB) to avoid unnecessary Keychain prompts.
- Claude: fix OAuth “Extra usage” spend/limit units when the API returns minor currency units (#97).
- Usage formatting: fix currency parsing/formatting on non-US locales (e.g., pt-BR). Thanks @mneves75!
- Antigravity: compile Windows probe regexes once instead of rebuilding them on each scan.

### Preferences & UI
- Windows: open the main window automatically when tray startup is unavailable, and support `CODEXBAR_START_VISIBLE` for proof/automation flows.
- Preferences: move “Access OpenAI via web” into Providers → Codex.
- Preferences: add usage source pickers for Codex + Claude with auto fallback.
- Preferences: add cookie source pickers with contextual helper text for the selected mode.
- Preferences: add debug switch to disable Keychain access and hide cookie-based web options.
- Preferences: add per-provider menu bar metric picker (#185) — thanks @HaukeSchnau
- Preferences: tighten provider rows (inline pickers, compact layout, inline refresh + auto-source status).
- Preferences: remove the “experimental” label from Antigravity.
- Menu bar: fix combined loading indicator flicker during loading animation (incl. debug replay).
- Menu bar: prevent blink updates from clobbering the loading animation.

### Menu
- Menu: add a toggle to show reset times as absolute clock values (instead of countdowns).
- Menu: show an “Open Terminal” action when Claude OAuth fails.
- Menu: add “Hide personal information” toggle and redact emails in menu UI (#137). Thanks @t3dotgg!
- Menu: reduce provider-switch flicker and avoid redundant menu card sizing for faster opens (#132). Thanks @ibehnam!

### CLI
- CLI: respect the reset time display setting.

### Dev & Tests
- Windows: switch eframe from `glow` to `wgpu` to avoid legacy OpenGL renderer issues in the VM.
- Dev: ignore VM proof screenshots and throwaway launcher scripts in git.
- Browser detection: remove an unused `find_browser_with_cookies` stub.
- Dev: move Chromium profile discovery into SweetCookieKit (adds Helium net.imput.helium). Thanks @hhushhas!
- Dev: bump SweetCookieKit to 0.2.0.
- Dev: migrate stored Keychain items to reduce rebuild prompts.
- Tests: expand Kiro CLI coverage.
- Tests: stabilize Claude PTY integration cleanup and reset CLI sessions after probes.
- Tests: kill leaked codex app-server after tests.
- Tests: add regression coverage for merged loading icon layout stability.
- Build: stabilize Swift test runtime.

## 0.17.0 — 2025-12-31
- New providers: MiniMax.
- Keychain: show a preflight explanation before macOS prompts for OAuth tokens or cookie decryption.
- Providers: defer z.ai + Copilot Keychain reads until the user interacts with the token field.
- Menu bar: avoid status item menu reattachment and layout flips during refresh to reduce icon flicker.
- Dev: align SweetCookieKit local-storage tests with Swift Testing.
- Charts: align hover selection bands with visible bars in credits + usage breakdown history.
- About: fix website link in the About panel. Thanks @felipeorlando!

## 0.16.1 — 2025-12-29
- Menu: reduce layout thrash when opening menus and sizing charts. Thanks @ibehnam!
- Packaging: default release notarization builds universal (arm64 + x86_64) zip.
- OpenAI web: reduce idle CPU by suspending cached WebViews when not scraping. Thanks @douglascamata!
- Icons: switch provider brand icons to SVGs for sharper rendering. Thanks @vandamd!

## 0.16.0 — 2025-12-29
- Menu bar: optional “percent mode” (provider brand icons + percentage labels) via Advanced toggle.
- CLI: add `codexbar cost` to print local cost usage (text/JSON) for Codex + Claude.
- Cost: align local cost scanner with ccusage; stabilize parsing/decoding and handle large JSONL lines.
- Claude: skip pricing for unknown models (tokens still tracked) to avoid hard-coded legacy prices.
- Performance: reduce menu bar CPU usage by caching morph icons, skipping redundant status-item updates, and caching provider enablement/order during animations.
- Menu: improve provider switcher hover contrast in light mode.
- Icons: refresh Droid + Claude brand assets to better match menu sizing.
- CI: avoid interactive login-shell probes to reduce noisy “CLI missing” errors.

## 0.15.3 — 2025-12-28
- Codex: default to OAuth usage API (ChatGPT backend) with CLI-only override in Debug.
- Codex: map OAuth credits balance directly, avoiding web fallback for credits.
- Preferences: add optional “Access OpenAI via web” toggle and show blended source labels when web extras are active.
- Copilot: replace blocking auth wait dialog with a non-modal sheet to avoid stuck login.

## 0.15.2 — 2025-12-28
- Copilot: fix device-flow waiting modal to close reliably after auth (and avoid stuck waits).
- Packaging: include the KeyboardShortcuts resource bundle to prevent Settings → Keyboard shortcut crashes in packaged builds.

## 0.15.1 — 2025-12-28
- Preferences: fix provider API key fields reusing the wrong input when switching rows.
- Preferences: avoid Advanced tab crash when opening settings.

## 0.15.0 — 2025-12-28
- New providers: Droid (Factory), Cursor, z.ai, Copilot.
- macOS: CodexBar now supports Intel Macs (x86_64 builds + Sonoma fallbacks). Thanks @epoyraz!
- Droid (Factory): new provider with Standard + Premium usage via browser cookies, plus dashboard + status links. Thanks @shashank-factory!
- Menu: allow multi-line error messages in the provider subtitle (up to 4 lines).
- Menu: fix subtitle sizing for multi-line error states.
- Menu: avoid clipping on multi-line error subtitles.
- Menu: widen the menu card when 7+ providers are enabled.
- Providers: Codex, Claude Code, Cursor, Gemini, Antigravity, z.ai.
- Gemini: switch plan detection to loadCodeAssist tier lookup (Paid/Workspace/Free/Legacy). Thanks @381181295!
- Codex: OpenAI web dashboard is now the primary source for usage + credits; CLI fallback only when no matching cookies exist.
- Claude: prefer OAuth when credentials exist; fall back to web cookies or CLI (thanks @ibehnam).
- CLI: replace `--web`/`--claude-source` with `--source` (auto/web/cli/oauth); auto falls back only when cookies are missing.
- Homebrew: cask now installs the `codexbar` CLI symlink. Thanks @dalisoft!
- Cursor: add new usage provider with browser cookie auth (cursor.com + cursor.sh), on-demand bar support, and dashboard access.
- Cursor: keep stored sessions on transient failures; clear only on invalid auth.
- z.ai: new provider support with Tokens + MCP usage bars and MCP details submenu; API token now lives in Preferences (stored in Keychain); usage bars respect the show-used toggle. Thanks @uwe-schwarz for the initial work!
- Copilot: new GitHub Copilot provider with device flow login plus Premium + Chat usage bars (including CLI support). Thanks @roshan-c!
- Preferences: fix Advanced Display checkboxes and move the Quit button to the bottom of General.
- Preferences: hide “Augment Claude via web” unless Claude usage source is CLI; rename the cost toggle to “Show cost summary”.
- Preferences: add an Advanced toggle to show/hide optional Codex Credits + Claude Extra usage sections (on by default).
- Widgets: add a new “CodexBar Switcher” widget that lets you switch providers and remember the selection.
- Menu: provider switcher now uses crisp brand icons with equal-width segments and a per-provider usage indicator.
- Menu: tighten provider switcher sizing and increase spacing between label and weekly indicator bar.
- Menu: provider switcher no longer forces a wider menu when many providers are enabled; segments clamp to the menu width.
- Menu: provider switcher now aligns to the same horizontal padding grid as the menu cards when space allows.
- Dev: `compile_and_run.sh` now force-kills old instances to avoid launching duplicates.
- Dev: `compile_and_run.sh` now waits for slow launches (polling for the process).
- Dev: `compile_and_run.sh` now launches a single app instance (no more extra windows).
- CI: build/test Linux `CodexBarCLI` (x86_64 + aarch64) and publish release assets as `CodexBarCLI-<tag>-linux-<arch>.tar.gz` (+ `.sha256`).
- CLI: add alias fallback for Codex/Claude detection when PATH lookups fail.
- Providers: support Arc browser cookies for Factory/Droid (and other Chromium-based cookie imports).
- Providers: support ChatGPT Atlas browser data for Chromium cookie imports.
- Providers: accept Auth.js secure session cookies for Factory/Droid login detection.
- Providers: accept Factory auth session cookies (session/access-token) for Droid.
- Droid: surface Factory API errors instead of masking them as missing sessions.
- Droid: retry auth without access-token cookies when Factory flags a stale token.
- Droid: try all detected browser profiles before giving up.
- Droid: fall back to auth.factory.ai endpoints when cookies live on the auth host.
- Droid: use WorkOS refresh tokens from browser local storage when cookies fail.
- Droid: read WorkOS refresh tokens from Safari local storage.
- Droid: try stored/WorkOS tokens before Chrome cookies to reduce Chrome Safe Storage prompts.
- Menu: provider switcher bars now track primary quotas (Plan/Tokens/Pro), with Premium shown for Droid.
- Menu: avoid duplicate summary blocks when a provider has no action rows.
- OpenAI web: ignore cookie sets without session tokens to avoid false-positive dashboard fetches.
- Providers: hide z.ai in the menu until an API key is set.
- Menu: refresh runs automatically when opening the menu with a short retry (refresh row removed).
- Menu: hide the Status Page row when a provider has no status URL.
- Menu: align switcher bar with the “show usage as used” toggle.
- Antigravity: fix lsof port filtering by ANDing listen + pid conditions. Thanks @shaw-baobao!
- Claude: default to Claude Code OAuth usage API (credentials from Keychain or `~/.claude/.credentials.json`), with Debug selector + `--claude-source` CLI override (OAuth/Web/CLI).
- OpenAI web: allow importing any signed-in browser session when Codex email is unknown (first-run friendly).
- Core: Linux CLI builds now compile (mac-only WebKit/logging gated; FoundationNetworking imports where needed).
- Core: fix CI flake for Claude trust prompts by making PTY writes fully reliable.
- Core: Cursor provider is macOS-only (Linux CLI builds stub it).
- Core: make `RateWindow` equatable (used by OpenAI dashboard snapshots and tests).
- Tests: cover alias fallback resolution for Codex/Claude and add Linux platform gating coverage (run in CI).
- Tests: cover hiding Codex Credits + Claude Extra usage via the Advanced toggle.
- Docs: expand CLI docs for Linux install + flags.

## 0.14.0 — 2025-12-25
- New providers: Antigravity.
- Antigravity: new local provider for the Antigravity language server (Claude + Gemini quotas) with an experimental toggle; improved plan display + debug output; clearer not-running/port errors; hide account switch.
- Status: poll Google Workspace incidents for Gemini + Antigravity; Status Page opens the Workspace status page.
- Settings: add Providers tab; move ccusage + status toggles to General; keep display controls in Advanced.
- Menu/UI: widen the menu for four providers; cards/charts adapt to menu width; tighten provider switcher/toggle spacing; keep menus refreshed while open.
- Gemini: hide the dashboard action when unsupported.
- Claude: fix Extra usage spend/limit units (cents); improve CLI probe stability; surface web session info in Debug.
- OpenAI web: fix dashboard ghost overlay on desktop (WebKit keepalive window).
- Debug: add a debug-lldb build mode for troubleshooting.

## 0.13.0 — 2025-12-24
- Claude: add optional web-first usage via Safari/Chrome cookies (no CLI fallback) including “Extra usage” budget bar.
- Claude: web identity now uses `/api/account` for email + plan (via rate_limit_tier).
- Settings: standardize “Augment … via web” copy for Codex + Claude web cookie features.
- Debug: Claude dump now shows web strategy, cookie discovery, HTTP status codes, and parsed summary.
- Dev: add Claude web probe CLI to enumerate endpoints/fields using browser cookies.
- Tests: add unit coverage for Claude web API usage, overage, and account parsing.
- Menu: custom menu items now use the native selection highlight color (plus matching selection text/track colors).
- Charts: boost hover highlight contrast for credits/usage history bands.
- Menu: reorder Codex blocks to show credits before cost.
- Menu: split Claude “Extra usage” (no submenu) from “Cost” (history submenu) and trim redundant extra-usage subtext.

## 0.12.0 — 2025-12-23
- Widgets: add WidgetKit extension backed by a shared app‑group usage snapshot.
- New local cost usage tracking (Codex + Claude) via a lightweight scanner — inspired by ccusage (MIT). Computes cost from local JSONL logs without Node CLIs. Thanks @ryoppippi!
- Cost summary now includes last‑30‑days tokens; weekly pace indicators (with runout copy) hide when usage is fully depleted. Thanks @Remedy92!
- Claude: PTY probes now stop after idle, auto‑clean on restart, and run under a watchdog to avoid runaway CLI processes.
- Menu polish: group history under card sections, simplify history labels, and refresh menus live while open.
- Performance: faster usage log scanning + cost parsing; cache menu icons and speed up OpenAI dashboard parsing.
- Sparkle: auto-download updates when auto-check is enabled, and only show the restart menu entry once an update is ready.
- Widgets: experimental WidgetKit extension (may require restarting the widget gallery/Dock to appear).
- Credits: show credits as a progress bar and add a credits history chart when OpenAI web data is available.
- Credits: move “Buy Credits…” into its own menu item and improve auto-start checkout flow.

## 0.11.2 — 2025-12-21
- ccusage-codex cost fetch is faster and more reliable by limiting the session scan window.
- Fix ccusage cost fetch hanging for large Codex histories by draining subprocess output while commands run.
- Fix merged-icon loading animation when another provider is fetching (only the selected provider animates).
- CLI PATH capture now uses an interactive login shell and merges with the app PATH, fixing missing Node/Codex/Claude/Gemini resolution for NVM-style installs.

## 0.11.1 — 2025-12-21
- Gemini OAuth token refresh now supports Bun/npm installations. Thanks @ben-vargas!

## 0.11.0 — 2025-12-21
- New optional cost display in the menu (session + last 30 days), powered by ccusage. Thanks @Xuanwo!
- Fix loading-state card spacing to avoid double separators.

## 0.10.0 — 2025-12-20
- Gemini provider support (usage, plan detection, login flow). Thanks @381181295!
- Unified menu bar icon mode with a provider switcher and Merge Icons toggle (default on when multiple providers are enabled). Thanks @ibehnam!
- Fix regression from 0.9.1 where CLI detection failed for some installs by restoring interactive login-shell PATH loading.

## 0.9.1 — 2025-12-19
- CLI resolution now uses the login shell PATH directly (no more heuristic path scanning), so Codex/Claude match your shell config reliably.

## 0.9.0 — 2025-12-19
- New optional OpenAI web access: reuses your signed-in Safari/Chrome session to show **Code review remaining**, **Usage breakdown**, and **Credits usage history** in the menu (no credentials stored).
- Credits still come from the Codex CLI; OpenAI web access is only used for the dashboard extras above.
- OpenAI web sessions auto-sync to the Codex CLI email, support multiple accounts, and reset/re-import cookies on account switches to avoid stale cross-account data.
- Fix Chrome cookie import (macOS 10): signed-in Chrome sessions are detected reliably (thanks @tobihagemann!).
- Usage breakdown submenu: compact chart with hover details for day/service totals.
- New “Show usage as used” toggle to invert progress bars (default remains “% left”, now in Advanced).
- Session (5-hour) reset now shows a relative countdown (“Resets in 3h 31m”) in the menu card for Codex and Claude.
- Claude: fix reset parsing so “Resets …” can’t be mis-attributed to the wrong window (session vs weekly).

## 0.8.1 — 2025-12-17
- Claude trust prompts (“Do you trust the files in this folder?”) are now auto-accepted during probes to prevent stuck refreshes. Thanks @tobihagemann!

## 0.8.0 — 2025-12-17
- CodexBar is now available via Homebrew: `brew install --cask steipete/tap/codexbar` (updates via `brew upgrade --cask steipete/tap/codexbar`).
- Added session quota notifications for the sliding 5-hour window (Codex + Claude): notifies when it hits 0% and when it’s available again, based only on observed refresh data (including startup when already depleted). Thanks @GKannanDev!

## 0.7.3 — 2025-12-17
- Claude Enterprise accounts whose Claude Code `/usage` panel only shows “Current session” no longer fail parsing; weekly usage is treated as unavailable (fixes #19).

## 0.7.2 — 2025-12-13
- Claude “Open Dashboard” now routes subscription accounts (Max/Pro/Ultra/Team) to the usage page instead of the API console billing page. Thanks @auroraflux!
- Codex/Claude binary resolution now detects mise/rtx installs (shims and newest installed tool version), fixing missing CLI detection for mise users. Thanks @philipp-spiess!
- Claude usage/status probes now auto-accept the first-run “Ready to code here?” permission prompt (when launched from Finder), preventing timeouts and parse errors. Thanks @alexissan!
- General preferences now surface full Codex/Claude fetch errors with one-click copy and expandable details, reducing first-run confusion when a CLI is missing.
- Polished the menu bar “critter” icons: Claude is now a crisper, blockier pixel crab, and Codex has punchier eyes with reduced blurring in SwiftUI/menu rendering.

## 0.7.1 — 2025-12-09
- Menu bar icons now render on a true 18 pt/2× backing with pixel-aligned bars and overlays for noticeably crisper edges.
- PTY runner now preserves the caller’s environment (HOME/TERM/bun installs) while enriching PATH, preventing Codex/Claude
  probes from failing when CLIs are installed via bun/nvm or need their auth/config paths.
- Added regression tests to lock in the enriched environment behavior.
- Fixed a first-launch crash on macOS 26 caused by the 1×1 keepalive window triggering endless constraint updates; the hidden
  window now uses a safe size and no longer spams SwiftUI state warnings.
- Menu action rows now ship with SF Symbol icons (refresh, dashboard, status, settings, about, quit, copy error) for clearer at-a-glance affordances.
- When the Codex CLI is missing, menu and CLI now surface an actionable install hint (`npm i -g @openai/codex` / bun) instead of a generic PATH error.
- Node manager (nvm/fnm) resolution corrected so codex/claude binaries — and their `node` — are found reliably even when installed via fnm aliases or nvm defaults. Thanks @aliceisjustplaying for surfacing the gaps.
- Login menu now shows phase-specific subtitles and disables interaction while running: “Requesting login…” while starting the CLI, then “Waiting in browser…” once the auth URL is printed; success still triggers the macOS notification.
- Login state is tracked per provider so Codex and Claude icons/menus no longer share the same in-flight status when switching accounts.
- Claude login PTY runner detects the auth URL without clearing buffers, keeps the session alive until confirmation, and exposes a Sendable phase callback used by the menu.
- Claude CLI detection now includes Claude Code’s self-updating paths (`~/.claude/local/claude`, `~/.claude/bin/claude`) so PTY probes work even when only the bundled installer is used.

## 0.7.0 — 2025-12-07
- ✨ New rich menu card with inline progress bars and reset times for each provider, giving the menu a beautiful, at-a-glance dashboard feel (credit: Anton Sotkov @antons).

## 0.6.1 — 2025-12-07
- Claude CLI probes stop passing `--dangerously-skip-permissions`, aligning with the default permission prompt and avoiding hidden first-run failures.

## 0.6.0 — 2025-12-04
- New bundled CLI (`codexbar`) with single `usage` command, `--format text|json`, `--status`, and fast `-h/-V`.
- CLI output now shows consistent headers (`Codex 0.x.y (codex-cli)`, `Claude Code <ver> (claude)`) and JSON includes `source` + `status`.
- Advanced prefs install button symlinks `codexbar` into /usr/local/bin and /opt/homebrew/bin; docs refreshed.

## 0.5.7 — 2025-11-26
- Status Page and Usage Dashboard menu actions now honor the icon you click; Codex menus no longer open the Claude status site.

## 0.5.6 — 2025-11-25
- New playful “Surprise me” option adds occasional blinks/tilts/wiggles to the menu bar icons (one random effect at a time) plus a Debug “Blink now” trigger.
- Preferences now include an Advanced tab (refresh cadence, Surprise me toggle, Debug visibility); window height trimmed ~20% for a tighter fit.
- Motion timing eased and lengthened so blinks/wiggles feel smoother and less twitchy.

## 0.5.5 — 2025-11-25
- Claude usage scrape now recognizes the new “Current week (Sonnet only)” bar while keeping the legacy Opus label as a fallback.
- Menu and docs now label the Claude tertiary limit as Sonnet to match the latest CLI wording.
- PATH seeding now uses a deterministic binary locator plus a one-shot login-shell capture at startup (no globbed nvm paths); the Debug tab shows the resolved Codex binary and effective PATH layers.

## 0.5.4 — 2025-11-24
- Status blurb under “Status Page” no longer prefixes the text with “Status:”, keeping the incident description concise.
- PTY runner now registers cleanup before launch so both ends of the TTY and the process group are torn down even when `Process.run()` throws (no leaked fds when spawn fails).

## 0.5.3 — 2025-11-22
- Added a per-provider “Status Page” menu item beneath Usage that opens the provider’s live status page (OpenAI or Claude).
- Status API now refreshes alongside usage; incident states show a dot/! overlay on the status icon plus a status blurb under the menu item.
- General preferences now include a default-on “Check provider status” toggle above refresh cadence.

## 0.5.2 — 2025-11-22
- Release packaging now includes uploading the dSYM archive alongside the app zip to aid crash symbolication (policy documented in the shared mac release guide).
- Claude PTY fallback removed: Claude probes now rely solely on `script` stdout parsing, and the generic TTY runner is trimmed to Codex `/status` handling.
- Fixed a busy-loop on the codex RPC stderr pipe (handler now detaches on EOF), eliminating the long-running high-CPU spin reported in issue #9.

## 0.5.1 — 2025-11-22
- Debug pane now exposes the Claude parse dump toggle, keeping the captured raw scrape in memory for inspection.
- Claude About/debug views embed the current git hash so builds can be identified precisely.
- Minor runtime robustness tweaks in the PTY runner and usage fetcher.

## 0.5.0 — 2025-11-22
- Codex usage/credits now use the codex app-server RPC by default (with PTY `/status` fallback when RPC is unavailable), reducing flakiness and speeding refreshes.
- Codex CLI launches seed PATH with Homebrew/bun/npm/nvm/fnm defaults to avoid ENOENT in hardened/release builds; TTY probes reuse the same PATH.
- Claude CLI probe now runs `/usage` and `/status` in parallel (no simulated typing), captures reset strings, and uses a resilient parser (label-first with ordered fallback) while keeping org/email separate by provider.
- TTY runner now always tears down the spawned process group (even on early Claude login prompts) to avoid leaking CLI processes.
- Default refresh cadence is now 5 minutes, and a 15-minute option was added to the settings picker.
- Claude probes/version detection now start with `--allowed-tools ""` (tool access disabled) while keeping interactive PTY mode working.
- Codex probes and version detection now launch the CLI with `-s read-only -a untrusted` to keep PTY runs sandboxed.
- Codex warm-up screens (“data not available yet”) are handled gracefully: cached credits stay visible and the menu skips the scary parse error.
- Codex reset times are shown for both RPC and TTY fallback, and plan labels are capitalized while emails stay verbatim.

## 0.4.3 — 2025-11-21
- Fix status item creation timing on macOS 15 by deferring NSStatusItem setup to after launch; adds a regression test for the path.
- Menu bar icon with unknown usage now draws empty tracks (instead of a full bar when decorations are shown) by treating nil values as 0%.

## 0.4.2 — 2025-11-21
- Sparkle updates re-enabled in release builds (disabled only for the debug bundle ID).

## 0.4.1 — 2025-11-21
- Both Codex and Claude probes now run off the main thread (background PTY), avoiding menu/UI stalls during `/status` or `/usage` fetches.
- Codex credits stay available even when `/status` times out: cached values are kept and errors are surfaced separately.
- Claude/Codex provider autodetect runs on first launch (defaults to Codex if neither is installed) with a debug reset button.
- Sparkle updates re-enabled in release builds (disabled only for debug bundle ID).
- Claude probe now issues the `/usage` slash command directly to land on the Usage tab reliably and avoid palette misfires.

## 0.4.0 — 2025-11-21
- Claude Code support: dedicated Claude menu/icon plus dual-wired menus when both providers are enabled; shows email/org/plan and Sonnet usage with clickable errors.
- New Preferences window: General/About tabs with provider toggles, refresh cadence, start-at-login, and always-on Quit.
- Codex credits without web login: we now read `codex /status` in a PTY, auto-skip the update prompt, and parse session/weekly/credits; cached credits stay visible on transient timeouts.
- Resilience: longer PTY timeouts, cached-credit fallback, one-line menu errors, and clearer parse/update messages.

## 0.3.0 — 2025-11-18
- Credits support: reads Codex CLI `/status` via PTY (no browser login), shows remaining credits inline, and moves history to a submenu.
- Sign-in window with cookie reuse and a logout/clear-cookies action; waits out workspace picker and auto-navigates to usage page.
- Menu: credits line bolded; login prompt hides once credits load; debug toggle always visible (HTML dump).
- Icon: when weekly is empty, top bar becomes a thick credits bar (capped at 1k); otherwise bars stay 5h/weekly.

## 0.2.2 — 2025-11-17
- Menu bar icon stays static when no account/usage is present; loading animation only runs while fetching (12 fps) to keep idle CPU low.
- Usage refresh first tails the newest session log (512 KB window) before scanning everything, reducing IO on large Codex logs.
- Packaging/signing hardened: strip extended attributes, delete AppleDouble (`._*`) files, and re-sign Sparkle + app bundle to satisfy Gatekeeper.

## 0.2.1 — 2025-11-17
- Patch bump for refactor/relative-time changes; packaging scripts set to 0.2.1 (5).
- Streamlined Codex usage parsing: modern rate-limit handling, flexible reset time parsing, and account rate-limit updates (thanks @jazzyalex and https://jazzyalex.github.io/agent-sessions/).

## 0.2.0 — 2025-11-16
- CADisplayLink-based loading animations (macOS 15 displayLink API) with randomized patterns (Knight Rider, Cylon, outside-in, race, pulse) and debug replay cycling through all.
- Debug replay toggle (`defaults write com.steipete.codexbar debugMenuEnabled -bool YES`) to view every pattern.
- Usage Dashboard link in menu; menu layout tweaked.
- Updated time now shows relative formatting when fresher than 24h; refactored sources into smaller files for maintainability.
- Version bumped to 0.2.0 (4).

## 0.1.2 — 2025-11-16
- Animated loading icon (dual bars sweep until usage arrives); always uses rendered template icon.
- Sparkle embedding/signing fixed with deep+timestamp; notarization pipeline solid.
- Icon conversion scripted via ictool with docs.
- Menu: settings submenu, no GitHub item; About link clickable.

## 0.1.1 — 2025-11-16
- Launch-at-login toggle (SMAppService) and saved preference applied at startup.
- Sparkle auto-update wiring (SUFeedURL to GitHub, SUPublicEDKey set); Settings submenu with auto-update toggle + Check for Updates.
- Menu cleanup: settings grouped, GitHub menu removed, About link clickable.
- Usage parser scans newest session logs until it finds `token_count` events.
- Icon pipeline fixed: regenerated `.icns` via ictool with proper transparency (docs in docs/icon.md).
- Added lint/format configs, Swift Testing, strict concurrency, and usage parser tests.
- Notarized release build "CodexBar-0.1.0.zip" remains current artifact; app version 0.1.1.

## 0.1.0 — 2025-11-16
- Initial CodexBar release: macOS 15+ menu bar app, no Dock icon.
- Reads latest Codex CLI `token_count` events from session logs (5h + weekly usage, reset times); no extra login or browser scraping.
- Shows account email/plan decoded locally from `auth.json`.
- Horizontal dual-bar icon (top = 5h, bottom = weekly); dims on errors.
- Configurable refresh cadence, manual refresh, and About links.
- Async off-main log parsing for responsiveness; strict-concurrency build flags enabled.
- Packaging + signing/notarization scripts (arm64); build scripts convert `.icon` bundle to `.icns`.
