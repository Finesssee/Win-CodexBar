# Upstream port procedure (Windows)

How **Win-CodexBar** tracks and ports releases from upstream
[`steipete/CodexBar`](https://github.com/steipete/CodexBar). This is a Windows
rewrite (Rust + Tauri), not a Swift fork — treat upstream as a **behavior and
wire-shape source**, not code to cherry-pick.

## Sync baseline

| Item | Value |
|------|--------|
| Upstream repo | `steipete/CodexBar` |
| Last landed baseline | **v0.54.0** (`1181138ac`) |
| Current port branch | **v0.54.0** |
| PR naming | One PR per upstream release: `Port upstream CodexBar X.Y.Z` |

Version bumps of *this* repo are a **separate later step**. A port PR lands
behavior; it does not have to ship a release tag.

## Zero upstream contact

- Do **not** open issues or PRs against `steipete/CodexBar`.
- Do **not** ping upstream maintainers about port status.
- Consume public release notes, compare API, and tagged sources only.

---

## Procedure

### 1. Detect the delta

1. Read the upstream GitHub release notes for `v<new>`.
2. Diff tags with the compare API (pin both ends — never `main`):

```powershell
# Example: 0.46.0 → 0.47.0
$prev = 'v0.46.0'
$new  = 'v0.47.0'
Invoke-RestMethod "https://api.github.com/repos/steipete/CodexBar/compare/$prev...$new" |
  Select-Object -ExpandProperty files |
  Select-Object status, filename |
  Format-Table -AutoSize
```

3. **Pin every source read to the release tag**, e.g.

```text
https://raw.githubusercontent.com/steipete/CodexBar/v0.47.0/path/to/File.swift
```

Never use `main` / default-branch raw URLs for port work — they drift under you.

### 2. Classify every release-note item

For each bullet / merged PR in the release, assign exactly one class:

| Class | Meaning |
|-------|---------|
| **PORT** | Has a local counterpart (provider, settings path, CLI, UI surface, fixture). |
| **SKIP** | macOS-exclusive or no Windows analog. Common skips: Keychain, iCloud/CloudKit, AppKit/SwiftUI menu chrome, `libproc`, `0600` POSIX file modes (our `secure_file` + DPAPI already covers owner-only intent). |
| **DECIDE-by-audit** | Unclear. Needs evidence before coding (see below). |

**DECIDE-by-audit evidence** (collect before touching code):

- Upstream file paths + symbols at the **tagged** revision
- Local counterpart path (or explicit “none”)
- Wire shape / fixture sample if network or file format is involved
- UI surface impact (tray / settings tab / float bar / none)
- Proposed class: PORT, SKIP, or **DEFER-with-evidence**

When the upstream reference is ambiguous, prefer **DEFER-with-evidence** over a
guess-port. Example from the 0.47.0 pass: Claude cold-boot items
(`#2493` / `#2494`) were deferred — not portable as written, not silently
half-implemented.

### 3. Split into workstreams

Group PORT items into independent commits / mini-PRs on the port branch:

- One provider or one vertical feature per commit when practical
- Shared infrastructure (factory, icons, settings schema) lands with the first
  consumer that needs it, or as its own scoped commit if several consumers share
  it
- Keep SKIP / DEFER notes in the final PR body — do not open empty stub modules
  “for later”

### 4. Port fixtures from exact wire shapes

- Copy field names, enums, and JSON shapes from upstream’s tagged sources or
  captured responses.
- **Never invent** upstream field names to make a test green.
- Prefer checked-in fixtures under the provider’s test tree; keep redaction of
  secrets.

### 5. Per-provider recipe

New or materially changed provider — touch the full registration path:

1. Provider module — `rust/src/providers/<name>/` (parse, auth, `fetch_usage`)
2. `ProviderId` — `rust/src/core/provider.rs` (`cli_name`, display, cookie domain, …)
3. Factory arm — `rust/src/core/provider_factory.rs` (exhaustive match)
4. Token accounts / multi-account plumbing if the provider uses it
5. Frontend catalog — `providerIcons` / `providerCatalog` (and any settings
   detail UI)
6. `provider_settings` / settings schema as needed
7. Locale keys in the **existing catalog style** (machine-translated values OK)

Worked example on this branch:

- Notion AI — commit `4774d64a` (`Port upstream 0.47.0: Notion AI provider`)
- xAI — commit `c05d48df` (`Port upstream 0.47.0: XAI provider`)

Also see [PROVIDERS.md](./PROVIDERS.md).

### 6. CUA proof rule (UI-affecting ports)

Unit tests and `local-check` do **not** prove tray, settings chrome, float bar,
theme, or WebView2 behavior.

If the port changes any of those surfaces:

1. Fresh local rebuild of the desktop binary
2. Proof mode as needed (`CODEXBAR_PROOF_MODE`, e.g. `settings:providers`)
3. Drive with **CUA Driver** ([trycua/cua](https://github.com/trycua/cua));
   attach screenshots / notes to the PR
4. If CUA cannot run, say why and attach equivalent manual proof

Details: root [AGENTS.md](../AGENTS.md) (Testing & QA) and the PR template.

### 7. Gate before PR

```powershell
# Repo gate
.\scripts\local-check.ps1

# Focused provider / parser tests (example)
cargo test -p codexbar notion
cargo test -p codexbar xai

# CLI smoke (adjust -p ids)
cargo run -p codexbar -- usage -p notion -v
cargo run -p codexbar -- config providers
```

UI-affecting work: CUA (or documented manual) proof on top of the above.

### 8. PR body and follow-ups

PR title: `Port upstream CodexBar X.Y.Z`.

Body must list:

- **Ported** — item + short note / commit
- **Skipped** — item + reason (macOS-only, no counterpart, …)
- **Deferred** — item + evidence pointer (issue-style notes OK inside the PR)

Do **not** mix the Win-CodexBar version bump / changelog release cut into the
port PR unless that is an explicit separate decision. Port first; release later.

---

## Conventions

| Rule | Detail |
|------|--------|
| No upstream interaction | No issues/PRs/comments on `steipete/CodexBar` |
| Commit scope | One workstream per commit; message prefix `Port upstream X.Y.Z: …` |
| Source pin | Always `vX.Y.Z` tag URLs / compare range — never `main` |
| Locales | Add keys in existing catalog style; machine translation allowed |
| Ambiguity | **DEFER-with-evidence** > guess-port |
| Secrets | DPAPI / `secure_file` for owner-only data; do not reimplement Keychain |
| POSIX mode bits | SKIP — owner-only intent already covered on Windows |
| Stubs | No empty “coming soon” provider shells for SKIP items |

---

## Appendix — Worked example: upstream 0.47.0

Baseline: upstream **v0.46.0** @ `e53abbeb`. Branch: `port/upstream-0.47.0`.

### Ported

| Item | Notes |
|------|--------|
| Notion AI provider | Full recipe; commit `4774d64a` |
| XAI provider | Full recipe; commit `c05d48df` |
| Hooks watch | `codexbar hooks watch` (#2536); commit `6495db74` |
| Low Power Mode | Settings + refresh throttling (#2518); commit `e42cb140` |
| Cursor optional on-demand usage | #2338; commit `19400339` |
| Command Code persist browser sessions | #2564; commit `f079b603` |
| OpenCode Go idle WAL read | #2544; commit `6510f30b` |
| Real-calendar monthly pace | #2552; commit `acb9c44f` |
| z.ai / Kimi / Grok window durations | #2431; commit `7cbcc775` |

### Skipped

| Item | Reason |
|------|--------|
| iCloud sync | CloudKit / macOS account surface — no counterpart |
| Keychain work | macOS Keychain; Windows uses DPAPI + `secure_file` |
| `libproc` usage | macOS process APIs |
| Menu SwiftUI churn | AppKit/SwiftUI menu bar — different shell (Tauri tray) |
| z.ai charts | No local chart counterpart for that change set |
| CurrencyExchange | No local counterpart |
| W5 | No local counterpart |
| W7 | No local counterpart |

### Deferred (not portable as written)

| Item | Reason |
|------|--------|
| Claude #2493 / #2494 (cold-boot) | DEFER-with-evidence — upstream behavior tied to macOS lifecycle / paths that do not map cleanly; do not guess-port |

---

## Appendix — Worked example: upstream 0.49.0–0.53.0

Baseline: upstream **v0.46.0** @ `e53abbeb` → **v0.53.0**. PRs #337–#346.

### Ported

| Upstream | PR | Items |
|----------|-----|-------|
| 0.49.0 | #337 | Provider parity, settings schema updates |
| 0.50.0 | #338 | Provider usage parsing fixes |
| 0.51.0 | #339 | Billing/usage wire-shape updates |
| 0.52.0 | #341, #344, #346 | TOON provider, gRPC-web/proto billing, UsageSourceSection changes |
| 0.53.0 | #341, #344, #346 | Locale parity, billing internals, deferred items |

Bug fixes landed alongside the ports: #333, #334, #335, #336, #342, #343.

### Skipped

| Item | Reason |
|------|--------|
| iCloud sync | CloudKit / macOS — no Windows counterpart |
| Keychain work | macOS Keychain; Windows uses DPAPI + `secure_file` |
| `libproc` usage | macOS process APIs |
| Menu SwiftUI churn | AppKit/SwiftUI menu bar — different shell (Tauri tray) |

### Deferred (not portable as written)

| Item | Reason |
|------|--------|
| Full upstream 0.53 locale parity (all 23 languages) | Only `en-US` ported; full 23-language catalog is a follow-up |
| TOON v4.1 spec conformance depth | Basic provider ported; full spec compliance deferred pending upstream stabilization |
| Grok gRPC-web+proto billing internals | Wire shape captured; deep proto decoding deferred |
| Codex fork-accounting timezone audit | Upstream timezone handling needs evidence before porting |
| UsageSourceSection backend-driven options | `GrokUsageSourceSection` renamed but still hardcodes options — backend-driven enumeration deferred |

---

## Related

- [ARCHITECTURE.md](./ARCHITECTURE.md) — module map and data flow
- [PROVIDERS.md](./PROVIDERS.md) — factory and add-provider checklist
- [CONFIGURATION.md](./CONFIGURATION.md) — settings stores / DPAPI paths
- [BUILDING.md](./BUILDING.md) — build and test entry points
- Root [AGENTS.md](../AGENTS.md) — CUA proof and agent rules
