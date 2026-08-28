# Repository Guidelines

## Project Overview

- Windows desktop tray app for AI-provider usage and limits (Win-CodexBar port of CodexBar).
- Default product surface: Tauri 2 desktop shell in `apps/desktop-tauri`, not the CLI.
- Shared domain/backend and CLI live in the `rust/` crate `codexbar`.
- Material under `docs/` that describes the upstream macOS/Swift project is historical unless the task is explicitly about upstream parity.
- When repo docs conflict, trust active sources: `apps/desktop-tauri` plus `rust/src`.

## Architecture & Data Flow

- Cargo workspace (root `Cargo.toml`): members `rust`, `apps/desktop-tauri/src-tauri`; **default-member** is the Tauri crate.
- Path dependency: `codexbar-desktop-tauri` → `codexbar = { path = "../../../rust" }`.
- Frontend: React 18 + Vite in `apps/desktop-tauri/src/`. Typed invoke bridge in `src/lib/tauri.ts`; DTOs in `src/types/bridge.ts`.
- Surfaces: the hidden `main` webview routes by window label / surface mode — TrayPanel, PopOut, Settings, FloatBar. Settings, float bar, and flyout use detached windows where needed.
- **Provider refresh**: `codexbar::core::instantiate_provider` (`rust/src/core/provider_factory.rs`) → `Provider::fetch_usage` → shell `commands/providers.rs` (semaphore + timeout) → `AppState.provider_cache` → events → React `useProviders`.
- **Settings**: `%config%/CodexBar/settings.json` via `Settings::load` / `save` and `secure_file` (DPAPI-capable on Windows). Frontend `updateSettings` patch → save → `codexbar:settings-updated` / float-bar config events.
- **Tray**: `tray_bridge` + `tray_menu`. Icon pixels from shared `codexbar::tray::{render_bar_icon_rgba, render_percent_icon_rgba}`.
- **Float bar**: `floatbar/` owns the auxiliary always-on-top window. The builder must pin `.theme(Some(tauri::Theme::Dark))` — WebView2 resolves `prefers-color-scheme` on a shared process profile; an unpinned window flips other webviews under theme `auto`.
- **Proof harness**: env `CODEXBAR_PROOF_MODE` (e.g. `settings:menu`) opens a target surface and suppresses blur-dismiss for automation / CUA capture. `CODEXBAR_SEED_USAGE_JSON=<abs-path>` seeds one synthetic bridge-shaped Codex `ProviderUsageSnapshot` into the provider cache at launch (pinned against refresh eviction; malformed files are warned about and skipped).

## Key Directories

- `apps/desktop-tauri/src/` — React UI (surfaces, hooks, i18n, bridge types)
- `apps/desktop-tauri/src-tauri/src/` — Tauri shell (`main`, tray, floatbar, shell windows, commands, proof_harness)
- `rust/src/core/` — `ProviderId`, `Provider` trait, `instantiate_provider`, fetch context
- `rust/src/providers/` — one module per provider (fetch/parse/auth)
- `rust/src/settings/` — settings model and load/save
- `rust/src/browser/` — Windows browser detection + cookie extraction
- `rust/src/tray/` — shared tray-icon renderer
- `rust/src/cli/` — CLI subcommands (`codexbar` binary)
- `scripts/` — `dev.ps1`, `local-check.ps1`, release and smoke scripts
- `docs/` — Windows port docs (`ARCHITECTURE`, `CLI`, `CONFIGURATION`, `PROVIDERS`, `BUILDING`, `COOKIES`, `WINDOWS_PROOF`, ADRs). Upstream macOS docs are read-only reference only.
- `.github/workflows/` — `pr-check.yml` (hosted gate), `interaction-guard.yml`

## Development Commands

```text
# Local CI slice (mirrors hosted PR check)
.\scripts\local-check.ps1

# Rust backend / CLI
cargo test --manifest-path rust/Cargo.toml
cargo clippy --manifest-path rust/Cargo.toml --all-targets -- -D warnings
cargo build -p codexbar
cargo run -p codexbar -- --help

# Tauri shell crate
cargo test --manifest-path apps/desktop-tauri/src-tauri/Cargo.toml
cargo clippy --manifest-path apps/desktop-tauri/src-tauri/Cargo.toml --all-targets -- -D warnings

# Frontend (cwd apps/desktop-tauri) — use pnpm, not npm
pnpm install
pnpm test
pnpm run build
pnpm run tauri:dev
pnpm run tauri:build:debug
pnpm run tauri:build

# Dev launch helpers (repo root)
.\scripts\dev.ps1
.\scripts\dev.ps1 -SkipBuild
./dev.sh
```

- Raw `cargo build --release` on the Tauri crate can still embed the **dev URL**. Prefer `pnpm run tauri:build` / `tauri:build:debug` or `scripts/dev.ps1`.
- Binaries: `codexbar.exe` (CLI), `codexbar-desktop-tauri.exe` (desktop).
- Default desktop work runs from the repo root (default-member). Use `cd rust` only for CLI/backend-only focus.
- There is no active root `Scripts/` (capital S) pipeline — use `scripts/`.
- Format before handoff when Rust changed: `cargo fmt --all`. Clippy both manifests with `-D warnings` (or explain skips).

## Code Conventions & Common Patterns

- Prefer small, typed structs/enums and focused modules; keep changes local.
- Provider-specific logic stays inside `rust/src/providers/<name>/` (or that module). Do not add cross-provider branching in shared paths.
- **New provider**: (1) `ProviderId` variant + metadata methods (`cli_name`, `display_name`, …), (2) provider module implementing `Provider`, (3) match arm in `core/provider_factory.rs::instantiate`. The factory is exhaustive — missing arms fail to compile. Never duplicate factories in the shell or CLI.
- Errors: `thiserror` (`ProviderError`) and `anyhow` where already used; keep user-facing messages friendly.
- Logging: `tracing` only. Never log secrets, cookies, tokens, or raw API keys.
- Frontend tests are co-located `*.test.ts` / `*.test.tsx`. Bridge types in `types/bridge.ts` must stay aligned with Rust command payloads (including settings tab ids).
- **Settings tab ids** (case-sensitive; backend whitelist in `surface_target.rs` must mirror frontend `SettingsTabId` / `TAB_META`): `general`, `providers`, `notifications`, `menuBar`, `menu`, `usageSpend`, `advanced`, `about`. Unknown ids fall back to General in the UI. Old ids `display` / `apiKeys` / `cookies` are not valid settings tabs.
- Cookie import UX uses **explicit browser selection** in Preferences — do not assume Chrome-only.
- Claude CLI output is user-configurable; do not treat a customizable status line as the usage source of truth.
- Keep provider data siloed: never show identity / plan / email from provider A in provider B UI.
- Secrets (manual cookies, API keys, token accounts): use existing redaction, `secure_file`, and keyring helpers.
- Do not add dependencies or tooling without confirmation.
- Do not open issues or PRs against upstream `steipete/CodexBar` unless the user explicitly asks. This repo is Win-CodexBar only.
- **GitHub write safety:** for any mutating `gh` command (comment, review, merge, close, edit, create, label, release, etc.), always pass the explicit `--repo owner/repo`; never rely on the current remote or a PR/issue number alone.
- Before any GitHub mutation, perform a read-back against that same explicit repo and verify the returned owner/repo or URL exactly matches the intended target. Abort on any mismatch.
- Treat `steipete/CodexBar` as **read-only by default**. A GitHub write to upstream requires explicit user authorization in the current turn; prior permission never carries forward.

## Important Files

- `apps/desktop-tauri/src-tauri/src/main.rs` — shell entry, command registration, setup
- `apps/desktop-tauri/src/App.tsx` — surface routing by window label
- `apps/desktop-tauri/src/lib/tauri.ts` — frontend invoke bridge
- `apps/desktop-tauri/src/types/bridge.ts` — DTOs + `SettingsTabId`
- `rust/src/core/provider_factory.rs` — sole provider factory
- `rust/src/core/provider.rs` — `ProviderId` + `Provider` trait
- `apps/desktop-tauri/src-tauri/src/commands/providers.rs` — refresh engine
- `apps/desktop-tauri/src-tauri/src/tray_bridge.rs` — tray icon and menu
- `apps/desktop-tauri/src-tauri/src/floatbar/window.rs` — float bar window builder
- `apps/desktop-tauri/src-tauri/src/surface_target.rs` — proof / settings tab whitelist
- `apps/desktop-tauri/src-tauri/tauri.conf.json` — active Tauri config
- `scripts/local-check.ps1` — local CI slice
- `.github/workflows/pr-check.yml` — hosted PR gate
- `CONTEXT.md` — CI budget glossary (Blacksmith pool / `CI_BUDGET_MODE`)

## Runtime/Tooling Preferences

- Package manager: **pnpm@10.18.1** (`packageManager` in `apps/desktop-tauri/package.json` + lockfile). Do not introduce npm or yarn lockfiles.
- Node: CI uses Node 20; no `.nvmrc` in repo — prefer Node 20 locally for parity.
- Rust: edition **2024**, stable toolchain; CI target `x86_64-pc-windows-msvc`. No committed `rust-toolchain.toml` / `rustfmt.toml` / `clippy.toml` — defaults plus CI flags (`clippy -- -D warnings`).
- Tray / DPAPI / browser-cookie behavior: validate on **Windows-native** hosts. WSL/Linux is insufficient for those paths.
- **CUA (computer-use) for UI proof** — see [Testing & QA](#testing--qa). Project: [trycua/cua](https://github.com/trycua/cua). On this machine the Windows driver is typically `%LOCALAPPDATA%\Programs\Cua\cua-driver\bin\cua-driver.exe`.


## Testing & QA

- Rust: prefer focused `#[cfg(test)]` unit tests near the changed module. Run both manifests after Rust changes.
- Frontend: Vitest 3 + jsdom + Testing Library. From `apps/desktop-tauri`: `pnpm test` (`src/**/*.{test,spec}.{ts,tsx}`).
- **Hosted PR check** (when `vars.CI_BUDGET_MODE != 'off'`): `cargo fmt --check`, clippy both crates with `-D warnings`, cargo test both crates, `pnpm --dir apps/desktop-tauri test`, `pnpm --dir apps/desktop-tauri run build` on Blacksmith Windows. Budget details: `CONTEXT.md`, ADRs under `docs/adr/`.
- **Local mirror**: `.\scripts\local-check.ps1` (default Rust + Tauri + Frontend). Does not run full installer / smoke unless you pass the matching flags.
- Parser / fetcher changes: add deterministic samples or fixtures where practical.
- No coverage thresholds are configured — do not invent any.

### UI validation with CUA ([trycua/cua](https://github.com/trycua/cua))

Unit tests and `local-check` do **not** prove tray, settings, float bar, theme, or WebView2 behavior. For UI / tray / settings / float-bar / visual changes, agents **must** retest on a real Windows desktop build using **Cua Drivers** (background computer-use: click, type, screenshot, UIA) from the open-source [trycua/cua](https://github.com/trycua/cua) project. Docs: [cua.ai/docs](https://cua.ai/docs), driver install: [install guide](https://cua.ai/docs/how-to-guides/driver/install), CLI reference: [cua-driver CLI](https://cua.ai/docs/reference/cua-driver/cli-reference).

**Install (Windows PowerShell, from upstream README):**

```powershell
irm https://cua.ai/driver/install.ps1 | iex
```

Then follow post-install instructions (permissions / accessibility as prompted).

**Typical layout after install:**

- Driver binary: `%LOCALAPPDATA%\Programs\Cua\cua-driver\bin\cua-driver.exe`
- Long-lived daemon: `cua-driver serve` (often over a named pipe). One-shot tools: `cua-driver call <tool> '<json>'` (e.g. `list_windows`, `get_window_state`, `click`, screenshots via `--screenshot-out-file`).

**Required retest loop after UI-affecting code changes:**

1. **Rebuild** a local desktop binary that includes the change (`pnpm --dir apps/desktop-tauri run tauri:build:debug` or `.\scripts\dev.ps1`). Do not validate against a stale pre-change exe.
2. **Close** any already-running CodexBar instance (single-instance plugin may hand off to the old process).
3. **Launch** the new binary. For stable automation (no blur-dismiss), set proof mode, e.g.  
   `$env:CODEXBAR_PROOF_MODE = 'settings:menu'`  
   (settings tab ids: `general`, `providers`, `notifications`, `menuBar`, `menu`, `usageSpend`, `advanced`, `about` — float bar section is on **`menu`**).
4. **Drive with CUA**: start the driver daemon if needed, then list windows / UIA tree, click the control under test, wait for UI settle, capture before/after screenshots.
5. **Assert observables** (pixels, window list, checked toggle state, theme still dark under `auto`, float bar window present, etc.) — not only “command exited 0”.
6. **Attach proof** to the PR (screenshots or short note + paths). If CUA cannot run, say why and attach equivalent manual proof (PR template).

**Do not** treat Vitest/jsdom or `cargo test` alone as sufficient for tray icon, DWM, WebView2 theme, float bar z-order, or settings chrome. **Do not** open issues/PRs against trycua/cua unless the user explicitly asks; use it as tooling.


## Commit & PR Guidelines

- Short imperative commit messages (e.g. `Fix Claude CLI parser`, `Improve cookie import errors`).
- Keep commits scoped to one change.
- In PRs / patches include:
  - Summary of behavior changes
  - Commands run (`cargo test`, `pnpm test`, `.\scripts\local-check.ps1`, etc.)
  - Screenshots / GIFs for UI changes (Windows)
  - Linked issue / reference when relevant
- Hosted PR check exists (`.github/workflows/pr-check.yml`); still run and report the local slice. Do not claim there is no CI.
- UI / tray / settings / float-bar / visual PRs: **CUA Driver proof is the default** ([trycua/cua](https://github.com/trycua/cua)) after a **fresh local rebuild** — see [UI validation with CUA](#ui-validation-with-cua-trycuacua). If CUA cannot be used, explain why and attach equivalent manual proof (PR template checkboxes).
- Before non-trivial merge: thermo-nuclear structure review when the project process requires it.


## Release & Winget Notes

- Treat Winget updates as a normal release step after GitHub release artifacts are stable.
- Winget does not track "latest" GitHub releases; every version needs its own immutable manifest folder in `microsoft/winget-pkgs`, for example `manifests/f/Finesssee/Win-CodexBar/0.23.6/`.
- For routine version bumps, copy the previous approved manifest folder and change only version-specific fields: `PackageVersion`, `InstallerUrl`, `InstallerSha256`, `DisplayName`, `DisplayVersion`, `ReleaseNotes`, and `ReleaseNotesUrl`.
- Keep stable package identity and installer behavior unchanged unless there is a real packaging reason: `PackageIdentifier`, `InstallerType`, `Scope`, `ProductCode`, `Publisher`, package URLs, and silent install behavior.
- Before opening a Winget PR, verify the release installer URL resolves and recompute the SHA-256 from the downloaded asset. On Windows, run `winget validate` when available.
- The first Winget package submission was approved in `microsoft/winget-pkgs#366653`; the v0.23.5 update was approved in `microsoft/winget-pkgs#366794`. Future updates should be faster, but still expect Microsoft validation/review.
- Agents must route GitHub mutations through `scripts/gh-safe.sh` instead of calling mutating `gh` subcommands directly. This is the sole supported mutation wrapper. It owns repo binding, performs read-back verification, blocks repo overrides, and fails closed on target mismatch.
- For object mutations (PR/issue/release), bind both the exact `owner/repo` and exact object number/tag. Use repo-only verification only for creates where the target object does not exist yet.
