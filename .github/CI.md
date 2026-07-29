# CI — Win-CodexBar

Win-CodexBar runs one hosted workflow: the **PR check** on Blacksmith Windows.
Release stays **local**. See `CONTEXT.md` for the shared CI budget glossary
and `docs/adr/0001` and `docs/adr/0002` for the decisions.

## Workflows

### PR check — `.github/workflows/pr-check.yml`

Runs on `pull_request`, on `push` to `main`/`master`, and on
`workflow_dispatch`. Runner: `blacksmith-4vcpu-windows-2025`
(Windows Server 2025; VS Build Tools available per Blacksmith docs).

Exact commands run, in order:

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm --dir apps/desktop-tauri test
pnpm --dir apps/desktop-tauri run build
```

This is the **local-check slice** from `scripts/local-check.ps1` only. It does
**not** run `tauri:build` release, installer packaging, smoke install, or
upload. Those stay on the local release path.

`concurrency.cancel-in-progress` is on, keyed by ref, so superseded pushes
cancel the in-flight run.

### Interaction guard — `.github/workflows/interaction-guard.yml`

Unchanged except for the budget gate (below). Runner stays
`blacksmith-2vcpu-ubuntu-2404`. Permissions are unchanged
(`contents: read`, `issues: write`, `pull-requests: write`).

## CI budget mode

Both workflows carry the gate `if: vars.CI_BUDGET_MODE != 'off'`, so they run
when the variable is unset (`normal`), `normal`, or `thin`, and **skip only
when `off`**.

Set `CI_BUDGET_MODE` in **Settings → Secrets and variables → Actions →
Variables**. Do not hard-code it in a workflow.

| Mode   | PR check | Interaction guard | Release |
|--------|----------|--------------------|---------|
| normal | runs     | runs               | local   |
| thin   | runs     | runs               | local   |
| off    | skip     | skip               | local   |

## Intent share (60/30/10)

The Blacksmith Pool minutes intent is divided: roughly **60% Win-CodexBar**,
**30% linear-cli**, **10% buffer**. This is an intent allocation of the
pool's minutes, not a measure of time spent in `normal`/`thin`/`off` modes.
Win-CodexBar's single Blacksmith Windows PR check is the only recurring
Windows job. If you add a second recurring job, reassess the 60% share before
merging.

## Blacksmith billing — Windows bills 2x

On the Blacksmith **free tier**, **Windows minutes bill at 2x**: one Windows
minute consumes two free-tier minutes. `blacksmith-4vcpu-windows-2025` is
Windows Server 2025. Because the PR check is a thin slice (fmt + clippy +
test) and never runs release builds, this is the only recurring Windows
cost. Keep it that way.

## $0 spend alert

Treat any non-zero monthly Blacksmith bill as an alert. If spend appears:

1. Set `CI_BUDGET_MODE=off` to stop both workflows immediately.
2. Investigate which run(s) consumed minutes (Actions tab → billed runs).
3. Trim or fix, then restore `CI_BUDGET_MODE=normal` (or `thin`).

The default posture is **$0 spend**. The PR check should stay within free-tier
minutes; if it does not, prefer reducing the check slice before paying.

## Release

Release is **local only**. There is no hosted release workflow. Follow
`docs/BUILDING.md` and `docs/release/ci-cd.md`: tag, run release-doctor, run
`windows-release-build.ps1 -SmokeInstall`, then upload assets to GitHub
Releases manually.
