# CI — Win-CodexBar

Win-CodexBar has two deliberately separate hosted CI responsibilities:

- **Blacksmith GitHub Actions** remains the primary PR/push validation path.
- **CircleCI** is a release-only Windows path. It can start only for a
  canonical protected semver tag (`vX.Y.Z`), never for a branch or PR.

The CircleCI pipeline does not replace or weaken the Blacksmith checks. Its
build job has no GitHub write credential; only the post-approval publisher
receives the restricted `GH_TOKEN` context.

## Blacksmith GitHub Actions

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
not run release packaging, installer smoke, or publication.

`concurrency.cancel-in-progress` is on, keyed by ref, so superseded pushes
cancel the in-flight run.

### Interaction guard — `.github/workflows/interaction-guard.yml`

The interaction guard remains on `blacksmith-2vcpu-ubuntu-2404` with its
existing `contents: read`, `issues: write`, and `pull-requests: write`
permissions. It is unrelated to release publication.

## GitHub Actions budget mode

Both GitHub workflows carry `if: vars.CI_BUDGET_MODE != 'off'`, so they run
when the variable is unset (`normal`), `normal`, or `thin`, and skip only when
it is `off`. This gate does not disable CircleCI releases.

Set `CI_BUDGET_MODE` in **Settings → Secrets and variables → Actions →
Variables**. Do not hard-code it in a workflow.

| Mode   | PR check | Interaction guard | Circle release |
|--------|----------|-------------------|----------------|
| normal | runs     | runs              | tag-triggered  |
| thin   | runs     | runs              | tag-triggered  |
| off    | skip     | skip              | tag-triggered  |

The Blacksmith Pool minutes intent remains roughly **60% Win-CodexBar**,
**30% linear-cli**, and **10% buffer**. CircleCI credits are separate and must
be budgeted in CircleCI.

## CircleCI release pipeline

Configuration: `.circleci/config.yml`, project **`nesszer/Win-CodexBar`**.
Both jobs use CircleCI's hosted Windows executor (`circleci/windows@5.0`).

1. `release-build` runs only when the workflow tag filter matches exactly
   `^v[0-9]+\.[0-9]+\.[0-9]+$`; branch filters explicitly ignore every branch.
   The preflight rejects PR/branch environment markers as a second boundary.
2. The credential-free build validates the canonical remote, full tag SHA,
   tag-to-SHA identity, protected `main` ancestry, and every project version
   file. It provisions/asserts Node 24.x via the `OpenJS.NodeJS.LTS` winget
   package, pnpm 11.24.0, the Rust MSVC target, Git, and Inno Setup 6.
3. It uses a new temporary `WorkRoot`, runs `release-doctor.ps1`, then runs
   `windows-release-build.ps1` with the immutable SHA and `-SmokeInstall`.
   It never uploads. Four assets, `release-manifest.json`, and build logs are
   persisted to the workspace and stored as CircleCI artifacts.
4. `release-approval` is a required manual CircleCI approval job.
5. `release-publish` attaches the workspace and is the only job with context
   `github-release-publisher`. Its `GH_TOKEN` is used by
   `scripts/publish-github-release.ps1` to create or update a **draft** release.
   Exact SHA-256 matches are skipped, mismatches fail, and missing assets are
   uploaded without clobbering. The script never publishes/finalizes a release.

### CircleCI project and context setup

These steps require repository/CircleCI administrator access and are not
automated by this repository:

1. Add the canonical GitHub project `nesszer/Win-CodexBar` to the CircleCI
   organization and enable `.circleci/config.yml`.
2. Create a restricted context named `github-release-publisher`, scoped only to
   this project (and the release job if organization policy supports expression
   restrictions). Add `GH_TOKEN` as a secret; never add it as a project
   variable or to the build job.
3. Use a fine-grained GitHub token for only this repository with **Contents:
   read and write** (release asset API access). Do not grant **Workflows**
   permission; no workflow file is changed by the publisher.
4. Protect the `v*` tag namespace with a GitHub ruleset/tag protection policy
   that permits only authorized release maintainers to create canonical
   `vX.Y.Z` tags. Protect `main` and require the normal Blacksmith checks.
5. Configure CircleCI notifications and a spending/credit alert appropriate to
   the organization. Do not approve a release until the build artifacts and
   manifest have been reviewed.

The GitHub token is intentionally not available to checkout, preflight,
prerequisite provisioning, release-doctor, build, smoke, or artifact steps.
CircleCI project setup and GitHub tag/context/ruleset changes are the current
manual setup scope.

## Cost, retry, and rollback behavior

Blacksmith Windows minutes remain the recurring PR cost and are still billed
according to the existing Blacksmith plan (Windows has historically billed at
2x on its free tier). CircleCI release builds add Windows executor credits only
for protected semver tags, plus the short approval/publish job. Do not use
CircleCI for branch validation or ad-hoc release testing.

Reruns are safe: the build is tied to the immutable SHA from the tag and
produces a fresh temporary WorkRoot. If publication stops after some uploads,
rerun the publisher after approval; matching assets are skipped and a same-name
different-hash asset fails rather than being replaced. A final/non-draft release
is never modified by the publisher. Rollback or deletion of a draft/final
release is a deliberate GitHub administrator action, followed by a new tag/SHA
release if replacement is required.

The current smoke scope is `scripts/windows-smoke-install.ps1`: install the
generated Inno Setup installer, verify the expected application/version, then
uninstall it. It does not prove every tray/UI/provider path; those remain
separate Windows/CUA validation work.

## Local release checks

Run the same pure checks and parser-level validation locally:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\release-pipeline.tests.ps1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\install-release-prerequisites.ps1 -AssertOnly
```

The hosted release flow is:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\release-preflight.ps1 -Tag vX.Y.Z -Sha <full-40-char-sha>
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\circleci-release-build.ps1 -Tag vX.Y.Z -Sha <full-40-char-sha>
```

Do not pass an upload switch to `windows-release-build.ps1`; publication is
owned exclusively by the approval-gated, hash-safe publisher.
