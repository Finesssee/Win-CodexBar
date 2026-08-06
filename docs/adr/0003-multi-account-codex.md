# ADR 0003: Multi-account Codex coexistence with ambient provider

Date: 2026-08-06
Status: Accepted

## Context

CodexBar today surfaces exactly one `codex` provider snapshot. That snapshot is
produced by the ambient single-account Codex provider in
`rust/src/providers/codex/`: it reads the identity that happens to be active in
`~/.codex` (`auth.json`), calls `fetch_usage`, and publishes the result through
the standard provider pipeline to the tray, flyout, and settings surfaces.

A stacked 5-PR series ports the MIT-licensed Windows core of
[`ademisler/codexcontrol`](https://github.com/ademisler/codexcontrol) (Windows
Python modules) into Rust as `rust/src/codex_accounts/`:

- **PR 1/5 — domain core**: account model (`CodexAccount`,
  `CodexAccountSource::Ambient | Managed`), ambient vs app-managed
  `CODEX_HOME` discovery (`file_locations`), per-account quota snapshots,
  account stores, login runner, and Codex Desktop (MSIX) session switching.
- **PR 2/5 — shell**: Tauri commands exposing discovery/login/switch/remove,
  plus managed-account refresh lanes that fetch and cache a snapshot per
  managed account in parallel.
- **PR 3/5 — frontend bridge**: TS types, `tauri.ts` wrappers, i18n keys.
- **PR 4/5 — settings panel**: `CodexAccountsSection` in provider settings.
- **PR 5/5 — tray menu**: `CodexAccountsMenu` flyout.

The new module therefore manages *many* accounts while the existing provider
pipeline still models *one* `codex` provider. Both must work while the port
lands in reviewable slices.

## Decision

**COEXISTENCE-NOW.** The ambient single-account provider
(`rust/src/providers/codex/`) remains the source of truth for the single
`codex` provider snapshot published through the existing pipeline today.
`rust/src/codex_accounts/` is added as a *parallel* managed-account domain:

- PR 2/5 ships managed-account lanes that fetch/cache one snapshot per managed
  account without disturbing the ambient provider's snapshot.
- PR 4/5 and PR 5/5 add surfaces (settings panel, tray flyout) that read the
  managed-account domain; they present the ambient slot through the same model
  (`CodexAccountSource::Ambient`) without changing what the single `codex`
  provider row shows.
- No existing provider, surface, or persistence path is rewired in this series;
  the ambient provider keeps publishing exactly as before.

**Replacement intent** (tracked as part of this ADR decision, not executed in
this series): the ambient provider path retires in a follow-up major release
once `codex_accounts` reaches feature parity against it — i.e. the managed
lanes match ambient `fetch_usage` behavior *and* local cost scans are migrated
onto the managed-home model. Retirement is a separate change with its own
review; this series only prepares the ground.

## Consequences

- During the transition both worlds publish snapshots: the ambient provider
  emits the single `codex` provider snapshot, while the codex_accounts lanes
  emit per-managed-account snapshots. They do not overwrite each other.
- **Identity precedence**: the ambient slot's snapshot *is* the `codex`
  provider snapshot (authority for credit bars, notifications, ordering) until
  the ambient path retires. Managed lanes never claim the provider slot; the
  tray/settings account picker is informational for them.
- **Signed/tracked lifetime hook**: the ambient path becomes eligible for
  retirement when (a) codex_accounts lane coverage matches ambient
  `fetch_usage` behavior (same usage windows, credits, and reset semantics per
  account) and (b) local cost scans are migrated to read managed homes. Both
  conditions are checked off under this ADR before the retirement PR merges.
- Review cost stays bounded: each PR is a small, independently testable slice;
  rollback of any surface PR leaves the domain intact.
- Transitional duplication is accepted: two code paths compute "current Codex
  usage" until retirement. The `NOTICE` file carries the upstream
  ademisler/codexcontrol MIT attribution.
