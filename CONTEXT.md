# Shared CI Budget glossary

This glossary is shared with the sister repo (`linear-cli`) so operators can
reason about CI spend across both projects with one vocabulary.

## Blacksmith Pool

The shared Blacksmith free-tier minute pool that both Win-CodexBar and the
sister repo (`linear-cli`) draw from. All `blacksmith-*` runners bill against
this one pool. The intent-share (60%) and $0 spend alert are defined against
the combined draw on this pool, not per repo.

**Retired for Win-CodexBar (2026-08):** the shared pool is exhausted, so this
repo's jobs no longer draw on it; the hosted PR check now runs on CircleCI
Windows (see "Hosted PR check (CircleCI)" below). The definitions here remain
for the sister repo and as pool history.

## CI budget mode

A repository variable `CI_BUDGET_MODE` controls how much CI runs per change.
It is intentionally coarse: `normal`, `thin`, or `off`.

| Mode   | PR check (CircleCI) | Interaction guard | Release                          |
|--------|----------|--------------------|----------------------------------|
| normal | runs     | runs               | local (Win-CodexBar only)        |
| thin   | runs     | runs               | local (Win-CodexBar only)        |
| off    | **skip** | **skip**           | local (Win-CodexBar only)        |

> Note: Win-CodexBar's Release is **always local** (no release workflow on
> Blacksmith). The sister repo `linear-cli` has a Blacksmith dispatch-only
> release workflow, so its Release column differs by design. Budget mode does
> not gate Win-CodexBar's local release — it is operator-driven regardless of
> mode.

- `normal` — default. Unset is treated as `normal`.
- `thin` — Win-CodexBar's PR check survives `thin`: it is the single hosted
  Windows job (now CircleCI; Blacksmith is retired for this repo), so it still
  runs. The
  sister repo (`linear-cli`) skips its PR check entirely under `thin`
  (`if: mode != 'off' && mode != 'thin'`); its default PR check is already a
  single Linux-only job, so there is no matrix to trim — `thin` simply drops
  the whole job.
- `off` — emergency stop. Both the PR check and the interaction guard skip.
  Use this when the bill approaches the `$0 spend` alert threshold or when a
  runaway workflow is burning minutes.

Set it in both CI surfaces, not in code:

- **CircleCI** (the hosted `pr-check` job): CircleCI Project Settings →
  Environment Variables → `CI_BUDGET_MODE`. The job's budget guard reads it as
  `$env:CI_BUDGET_MODE`; unset/empty is treated as `normal`. CircleCI's
  GitHub App integration does not build fork-PR pipelines, so fork-PR
  changes need the manual same-repo branch fallback (see "Hosted PR check
  (CircleCI)"); `CI_BUDGET_MODE` applies whenever a pipeline actually runs.
- **GitHub Actions** (`interaction-guard.yml` unchanged; `pr-check.yml` now a
  manual-dispatch-only fallback with no automatic push/PR scheduling):
  Settings → Secrets and variables → Actions → Variables; the workflows read
  it as `vars.CI_BUDGET_MODE`.

## Hosted PR check (CircleCI)

As of 2026-08, Win-CodexBar's hosted PR/push gate is the `pr-check` job in
`.circleci/config.yml` (workflow `pr-check`): a single CircleCI Windows job on
the `circleci/windows@5.0` executor (`win/default`, `size: medium`). One
fused step provisions the toolchain and delegates the checks to
`scripts/local-check.ps1 -Slice ci` (the GitHub-workflow mirror; see "Local
check slice" below). Early gates reproduce the GitHub workflow's trigger
contract and log why they skip, then call `circleci-agent step halt` to stop
the job before any cache or toolchain spend: budget `off`, non-PR branch
pushes (PRs and `main`/`master` pushes run), and docs-only diffs
(`docs/**`, `**/*.md`, `CONTEXT.md`, `.github/CI.md`; fails open if the
base revision cannot be determined). The workflow ignores release tags
(`/^v[0-9]+\.[0-9]+\.[0-9]+$/`), so it never double-runs with the tag-gated
`release` workflow. The repository stays public, so the job spends CircleCI's
Free Plan open-source allowance: open-source builds are not subject to the
Free Plan's 30,000-credit personal-usage block and keep running even when a
personal credit balance is exhausted
(https://circleci.com/docs/guides/plans-pricing/credits). CircleCI's OSS
program also documents a monthly allowance for macOS/Windows OSS builds, so
budget the thin slice's Windows credits on the CircleCI plan, not the retired
Blacksmith intent share.

The required branch/PR status check for `main` is `ci/circleci: pr-check`
(the CircleCI `pr-check` job), not a Blacksmith check.

## Intent share (60/30/10)

The Blacksmith Pool minutes intent was divided roughly **60% Win-CodexBar**,
**30% linear-cli**, and **10% buffer**. That allocation is historical for this
repo: Win-CodexBar's hosted PR check is now a single CircleCI Windows job (the
Blacksmith runner is retired for this repo), so this repo no longer draws on
the pool; release builds stay local.

## Blacksmith billing note

**Retired for this repo (2026-08):** the shared Blacksmith pool is exhausted,
so Win-CodexBar's hosted PR check no longer draws on it; it runs on CircleCI
Windows instead (see "Hosted PR check (CircleCI)"). The notes below are kept
as pool history.

On the Blacksmith free tier, **Windows minutes bill at 2x** (one Windows
minute consumes two free-tier minutes). `blacksmith-4vcpu-windows-2025` is a
Windows Server 2025 runner with VS Build Tools available, so Rust + Tauri
builds work without extra setup. The PR check is a thin slice (fmt + clippy +
test) and never runs `tauri:build` release, installer packaging, smoke
install, or upload.

## Local check slice

The hosted PR check runs `scripts/local-check.ps1 -Slice ci`, which mirrors
`.github/workflows/pr-check.yml` step for step: workspace-wide
`cargo fmt --check` and `cargo clippy -D warnings`, workspace `cargo test`,
and the frontend `pnpm install --frozen-lockfile`, `pnpm test`, and `tsc
--noEmit` (via `pnpm run build`), plus the interaction-guard script tests.
The script's default (no-parameter) slice is unchanged for developers. The
PR check deliberately excludes `tauri:build` release, the installer, smoke
install, and release upload — those stay on the local Windows release path.
