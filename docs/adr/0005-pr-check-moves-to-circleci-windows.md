# ADR 0005: Hosted PR check moves to CircleCI Windows

Date: 2026-08-31
Status: Accepted; supersedes the hosted-runner decision of ADR 0001 and the
Blacksmith gate description in ADR 0002

## Context

The shared Blacksmith free-tier minute pool is exhausted, so the Blacksmith
GitHub Actions runners behind ADR 0001's PR check
(`.github/workflows/pr-check.yml`) can no longer be relied on for scheduled
PR validation. CircleCI is already this repo's hosted Windows platform for
releases (ADR 0004), with a working `circleci/windows@5.0` configuration, and
the repository is public, so its PR builds draw on CircleCI's Free Plan
open-source allowance rather than the personal credit block.

## Decision

Move the hosted PR/push gate to CircleCI as a new `pr-check` job in
`.circleci/config.yml` (workflow `pr-check`):

- `win/default` executor, `size: medium`. The check steps are not re-declared
  in the config: one fused step provisions the toolchain, then delegates the
  whole check to `scripts/local-check.ps1 -Slice ci`, a new opt-in slice that
  mirrors `.github/workflows/pr-check.yml` step for step (`cargo fmt --all
  --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo
  test --workspace`, `pnpm install --frozen-lockfile`, `pnpm test`, `pnpm run
  build`, plus the interaction-guard script tests). The script's default
  (no-parameter) local behavior is unchanged.
- Rust stable is installed by downloading `rustup-init.exe` directly from
  `https://win.rustup.rs/x86_64` (the hosted image has no winget) and then
  installing the stable toolchain with the `rustfmt`/`clippy` components and
  the `x86_64-pc-windows-msvc` target; Node 24.18.0 is installed from the
  official x64 MSI (`msiexec /qn /norestart` with a dedicated per-version
  `INSTALLDIR`, because the image's preinstalled Node is a different major and
  a same-product MSI upgrade silently no-ops); pnpm 11.24.0 is activated by
  corepack from the exact `packageManager` pin in
  `apps/desktop-tauri/package.json`.
- Trigger: the workflow filter stays wide (`branches: only: /.*/`) because
  CircleCI delivers same-repo PR builds as branch pipelines; parity with the
  GitHub workflow (PRs plus pushes to `main`/`master`, `paths-ignore` for
  `docs/**`, `**/*.md`, `CONTEXT.md`, `.github/CI.md`) is enforced inside the
  job by early gates. Each true skip condition logs its reason and then calls
  `circleci-agent step halt`, which stops the entire job — before the cache
  restore steps and the fused provision/run step — so a skip spends no cache
  or toolchain minutes (the throw on a non-zero `circleci-agent` exit code
  prevents a silent no-op). Docs-only detection diffs the base commit
  against the head with a two-dot tree diff (robust when the base is fetched
  at depth 1): on PRs the base SHA comes primarily from the compile-time
  GitHub App pipeline value `pipeline.event.github.pull_request.base.sha`
  (populated on pull request events); if that is empty while the pipeline is
  still PR-associated, the base SHA is resolved from the public GitHub pulls
  API using the documented `CIRCLE_PROJECT_USERNAME`/`CIRCLE_PROJECT_REPONAME`
  variables and fetched with `--depth=1`; on `main`/`master` pushes
  `HEAD^` is used when available. Any resolution, fetch, or diff failure
  fails open: it exits only the gate step and therefore continues the job
  into the checks; the gate never silently skips on an unknown base.
- Budget gating stays honest and coarse: the job reads `CI_BUDGET_MODE` as a
  CircleCI project environment variable (unset/empty = `normal`) and halts
  the job via `circleci-agent step halt` when it equals `off`.

`.github/workflows/pr-check.yml` is retained as a manual-dispatch-only
fallback: its `on:` block now holds `workflow_dispatch` only (push and
`pull_request` triggers removed), so it no longer schedules automatically and
can be run by hand for Blacksmith diagnostics with the job body intact. The
interaction guard (`.github/workflows/interaction-guard.yml`) remains a
GitHub Actions workflow and is unaffected.

## Consequences

- Hosted PR feedback continues on the real Windows target with no Blacksmith
  dependency.
- `CI_BUDGET_MODE` must now be set in two places to control both surfaces:
  CircleCI project environment variables (read by the `pr-check` guard) and
  GitHub Actions repository variables (read by `vars.CI_BUDGET_MODE`). The
  glossary in `CONTEXT.md` documents both.
- CircleCI Windows credits become the recurring PR cost, spent against the
  open-source allowance; organization credit alerts should cover the PR check
  as well as releases.
- ADRs 0001 and 0002 remain as immutable history describing the Blacksmith
  era; this ADR supersedes only where the hosted PR check runs.
