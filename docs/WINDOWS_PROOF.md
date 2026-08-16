# Windows build proof — `feature/port-phase-13-e2e`

This document describes how to produce the Windows installer from the
Phase-13 desktop-shell branch and what to manually verify on the
resulting machine. It replaces the stale macOS/Swift workflow docs for
the purposes of the Phase-13 sign-off.

**Branch:** `feature/port-phase-13-e2e`
**Default app:** Tauri shell (`apps/desktop-tauri/src-tauri`)

---

## 1. Build commands (PowerShell, from repo root)

Run in a clean PowerShell 7 session on Windows 10 or 11 (x64). `cd` into
the worktree root first.

```powershell
# Prerequisites (once per machine)
rustup target add x86_64-pc-windows-msvc
# WiX Toolset v3 is required for the .msi artefact and is installed by the
# Tauri bundler on first run (https://tauri.app/v2/reference/config/#bundle).

# Shared backend + Rust tests
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test

# Desktop Tauri shell — focused
cargo test -p codexbar-desktop-tauri
cargo clippy -p codexbar-desktop-tauri --all-targets -- -D warnings

# Proof/CUA captures: set `CODEXBAR_PROOF_MODE` (e.g. `trayPanel`) to open a
# surface with blur-dismiss suppressed; optionally set
# `CODEXBAR_SEED_USAGE_JSON=<abs-path>` to plant one synthetic bridge-shaped
# Codex ProviderUsageSnapshot into the provider cache at launch (malformed
# files are warned about and ignored) — the seeded cache is pinned against
# refresh eviction for the duration of the run.

# Frontend unit tests + bundle
cd apps\desktop-tauri
pnpm install --frozen-lockfile
pnpm run check-locale      # exits non-zero on Rust/TS locale-key drift
pnpm run test              # vitest smoke tests
pnpm run build             # prebuild runs check-locale, then tsc + vite

# Produce the Windows installer (.msi via WiX + .exe via NSIS)
pnpm run tauri:build       # add  -- --bundles msi  or  nsis  to narrow
```

Artefacts land in
`apps\desktop-tauri\src-tauri\target\release\bundle\msi\*.msi` and
`\nsis\*.exe`. Invocations that pass `--no-bundle` (our default `pnpm run
tauri:build`) skip installer packaging; drop `--no-bundle` for the real
release flow via `cargo tauri build` directly.

For the Inno Setup release artifact, prefer the cached Windows release builder:

```powershell
.\scripts\windows-release-build.ps1 -Ref v0.27.4
```

It keeps a clean managed checkout while reusing Cargo, pnpm, and signed
installer dependency caches. Use `-WarmCacheOnly` after large ports to prepare
the desktop cache, and `-SmokeInstall` to install/uninstall the generated
installer. The builder has no upload switch. Publication is performed only by
the approval-gated CircleCI draft publisher in `docs/release/ci-cd.md`, which
checks hashes and never overwrites an existing asset.
Run the standalone smoke installer test on a Windows machine before approval:

```powershell
powershell -ExecutionPolicy Bypass `
  -File .\scripts\windows-smoke-install.ps1 `
  -InstallerPath .\rust\target\installer\CodexBar-<version>-Setup.exe `
  -ExpectedVersion <version>
```

The script verifies the silent install switches
`/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`, installed files, CLI `--help` and
`--version` output, Start Menu shortcut, uninstall registry entry, optional
version match, and silent uninstall cleanup. Add `-LeaveInstalled` when the VM
should keep the installed app for manual UI validation.

---

## 2. Observable behaviours to verify on Windows

After installing the built `.msi`, launch **CodexBar** and walk through
the list below. Check each item off in §4.

1. **Tray icon visible** in the Windows notification area (may require
   "Show all icons" in the taskbar settings on first run).
2. **Left-click the tray icon** → borderless pop-out panel anchored
   near the tray. On multi-monitor setups it should appear on the
   display hosting the tray.
3. **Right-click the tray icon** → native Windows context menu with
   Pop Out / Refresh / Settings / Check for updates / Quit entries.
4. **Preferences window** opens from the tray menu or from the pop-out
   settings gear; all tabs render (General, Providers, Display, API
   keys, Cookies, Token accounts, Advanced, About).
5. **Theme toggle** — Appearance → Theme cycles `Auto ↔ Light ↔ Dark`
   and the active window repaints instantly. Auto should track the OS
   setting (Windows Settings → Personalisation → Colors).
6. **Shortcut capture** — General → Global shortcut → Record, press
   e.g. `Ctrl+Shift+K`. Chip shows the combo, `Saved` appears, and the
   shortcut toggles the tray panel globally.
7. **Provider rows drag-reorder** in Preferences → Providers: grab the
   handle, drop above/below another provider, refresh the pop-out, and
   confirm the new order is persisted across app restarts.
8. **Chart tooltip** — open a provider detail → Cost/Credits chart.
   Hovering a bar/point shows a tooltip with value + date.
9. **Reset countdown** — pop-out card shows `Resets in Xh Ym` and
   re-renders at least once during a 1-minute stare (internal tick is
   30 s).
10. **Update banner** — Advanced → Check for updates; when an update is
    offered the banner appears at the top of the pop-out and dismiss /
    download / install-and-restart buttons all respond.

---

## 3. Known platform limitations

- **Work-area rect** on Linux hosts (X11/Wayland) returns the full
  monitor size because the Tauri `monitor.work_area` helper does not
  yet exclude panels. The production target is Windows, where
  `SystemParametersInfoW(SPI_GETWORKAREA)` is correct.
- The following CLIs are **Windows-only** in this port and cannot be
  exercised on macOS/Linux: DPAPI-protected browser cookie import
  (Chrome/Edge), the single-instance lock via named mutex, and the
  MSI-based auto-update channel.
- **`fSingleSessionPerUser`** (terminal-services policy): if the
  post-install tray icon does not appear on a multi-user Windows
  server, toggle
  `HKLM\SYSTEM\CurrentControlSet\Control\Terminal Server\fSingleSessionPerUser=0`
  on the host. This is **not** set by the installer — it is a host
  policy decision, documented here only so operators can unblock
  themselves. This repository must not ship a registry tweak for it.
- E2E browser automation (Playwright / WebDriver) is intentionally out
  of scope for Phase 13; the `pnpm run test` suite covers unit-level
  behaviour only.

---

## 4. Machine-readable check-list

Tick each entry as it is verified on the Windows target.

```
[ ] build.cargo-fmt               cargo fmt --all
[ ] build.cargo-clippy            cargo clippy --all-targets -- -D warnings
[ ] build.cargo-test              cargo test
[ ] build.tauri-clippy            cargo clippy -p codexbar-desktop-tauri --all-targets -- -D warnings
[ ] build.tauri-test              cargo test -p codexbar-desktop-tauri
[ ] build.pnpm-install            cd apps/desktop-tauri && pnpm install --frozen-lockfile
[ ] build.pnpm-check-locale       pnpm run check-locale
[ ] build.pnpm-test               pnpm run test
[ ] build.pnpm-build              pnpm run build
[ ] build.tauri-bundle            pnpm run tauri:build (msi + nsis)
[ ] release.cached-build          .\scripts\windows-release-build.ps1 -Ref <tag>
[ ] release.doctor                .\scripts\release-doctor.ps1 -Version <version>

[ ] runtime.tray-icon-visible
[ ] runtime.tray-left-click-popout
[ ] runtime.tray-context-menu
[ ] runtime.preferences-opens
[ ] runtime.preferences-all-tabs
[ ] runtime.theme-light
[ ] runtime.theme-dark
[ ] runtime.theme-auto-tracks-os
[ ] runtime.shortcut-capture-records
[ ] runtime.shortcut-capture-toggles-panel
[ ] runtime.provider-drag-reorder
[ ] runtime.provider-order-persists
[ ] runtime.chart-tooltip-visible
[ ] runtime.reset-countdown-ticks
[ ] runtime.update-banner-flow
[ ] runtime.single-instance-mutex
```
