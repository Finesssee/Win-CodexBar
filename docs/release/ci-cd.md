# Win-CodexBar CI and release delivery

## Responsibilities

**CircleCI** (`.circleci/config.yml`) is now the hosted PR/push validation
path and the hosted release path. The `pr-check` workflow (added 2026-08) runs
the format, clippy, Rust test, frontend test, and frontend build checks on
hosted CircleCI Windows. It mirrors the GitHub workflow's trigger contract:
pull requests and pushes to `main`/`master` run the checks (delegated to
`scripts/local-check.ps1 -Slice ci`); other branch pushes and docs-only
diffs (`docs/**`, `**/*.md`, `CONTEXT.md`, `.github/CI.md`) skip via early
gates that log their reason and then call `circleci-agent step halt`, stopping
the job before any cache or toolchain spend (docs-only detection fails open
when the base revision cannot be determined). The release-tag pattern (`vX.Y.Z`) is
ignored so it never double-runs with the release workflow. The former
Blacksmith GitHub Actions gate (`.github/workflows/pr-check.yml`) is retired
for this repo — the Blacksmith pool is exhausted — and the workflow is now a
manual-dispatch-only fallback (`on: workflow_dispatch` only; no automatic
push/PR scheduling) kept for Blacksmith diagnostics.

**Fork-PR coverage note:** CircleCI does not build pull requests from forks
by default (unlike GitHub Actions). The GitHub App integration does not build
fork-PR pipelines, and there is no Advanced setting that can enable them;
external fork PRs are therefore **not covered** by the CircleCI gate. The
manual fallback for forks is needed: an external contributor's changes must
be pulled onto a same-repo branch (or the fork changes committed to a
maintainer branch) so a CircleCI branch pipeline runs the checks. When those
pipelines run, CircleCI withholds project environment variables from
untrusted builds by default (so `CI_BUDGET_MODE` arrives unset → `normal` →
checks run), which is the same withholding the `CI_BUDGET_MODE` note in
`CONTEXT.md` relies on. Tradeoff: bounded OSS credit spend, but external
contributors get no direct hosted Windows validation of their own PRs.

The `release` workflow remains filtered to the canonical
`nesszer/Win-CodexBar` project and exact protected tags `vX.Y.Z`; branch and
pull-request pipelines cannot enter it. The CircleCI Windows release build is
credential-free. Only its explicit approval-gated publisher gets the
restricted `GH_TOKEN` context.

## CircleCI release flow

1. A maintainer creates a protected canonical tag such as `v0.48.0` on `main`.
   CircleCI's workflow filter and `scripts/release-preflight.ps1` both reject
   branches, PRs, non-semver tags, non-canonical remotes, tag/SHA mismatch,
   and commits whose tag is not reachable from protected `origin/main`.
2. The build job provisions/asserts Node 24.x via the `OpenJS.NodeJS.LTS`
   winget package, pnpm 11.24.0, the Windows MSVC Rust target, Git, and Inno
   Setup 6. It validates every committed project version against the tag.
3. The build invokes `scripts/release-doctor.ps1 -SkipGitHub`, then
   `scripts/windows-release-build.ps1 -Ref <full-SHA> -SmokeInstall` with a
   fresh temporary `WorkRoot`. It never receives `GH_TOKEN` and never uploads.
4. The job emits exactly these six publishable assets:
   `CodexBar-X.Y.Z-Setup.exe`, its `.sha256` sidecar,
   `CodexBar-X.Y.Z-portable.exe`, its `.sha256` sidecar,
   `CodexBarCLI-vX.Y.Z-windows-x64.zip`, and its `.sha256` sidecar. It also
   emits `release-manifest.json` (tag, commit, version, sizes, and hashes)
   and logs,
   then persists/stores the bundle as CircleCI workspace/artifacts.
5. A human must approve the `release-approval` job after reviewing the
   manifest and artifact logs.
6. The `release-publish` job receives `GH_TOKEN` only from the restricted
   `github-release-publisher` context. `scripts/publish-github-release.ps1`
   creates a draft release if absent, or uses an existing draft. It compares
   every same-name asset by SHA-256, skips exact matches, fails on mismatch,
   and uploads only missing assets. It never clobbers and never changes a
   draft to a final release.
7. A maintainer publishes the draft manually in GitHub after any final release
   notes/review. Winget follows only after the immutable installer URL and
   digest are stable.

## Signing status

The `CodexBarCLI-vX.Y.Z-windows-x64.zip` asset is currently **unsigned**. The
SignPath workflow's upload glob (`CodexBar-*.exe`) does not include it.
Revisit when SignPath wiring for the CLI zip lands.

## Local checks

Run the focused, dependency-free helper tests and prerequisite assertions:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\release-pipeline.tests.ps1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\install-release-prerequisites.ps1 -AssertOnly
```

The build/preflight commands used by CircleCI can be exercised with a full SHA:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\release-preflight.ps1 -Tag vX.Y.Z -Sha <full-40-char-sha>
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\circleci-release-build.ps1 -Tag vX.Y.Z -Sha <full-40-char-sha>
```

The smoke test covers install, expected version validation, and uninstall of
the generated Inno Setup installer on the real Windows runner. It does not
cover all tray, WebView2, provider, or CUA/UI behavior.

## Setup that requires administrators

The repository cannot create external settings. Configure the CircleCI project
for `nesszer/Win-CodexBar`, enable `.circleci/config.yml`, and create a
project-restricted context named `github-release-publisher`. Store `GH_TOKEN`
there only, using a fine-grained GitHub token scoped to this repository with
Contents read/write for release APIs. Do not grant Workflows permission.
Add `CI_BUDGET_MODE` as a CircleCI project environment variable (Project
Settings → Environment Variables) so the `pr-check` budget guard can read it;
unset/empty is treated as `normal`.

Protect `main`, require the hosted CircleCI `pr-check` for branch/PR
validation, and protect the `v*`
tag namespace so only authorized maintainers can create canonical `vX.Y.Z` tags.
Configure CircleCI credit/spend alerts and notifications as appropriate for
the organization. These project, context, token, ruleset, and billing changes
are intentionally manual.

## Cost, retry, and rollback

The hosted PR check now runs on CircleCI Windows: its thin-slice Windows
credits recur on PRs and `main`/`master` pushes (other branch pushes and
docs-only diffs skip before spending), alongside the protected release tag
path. The repository is public, so the PR check spends the Free Plan
open-source allowance (open-source builds are not subject to the Free Plan's
30,000-credit personal block). Fork PRs remain uncovered — the GitHub App does
not build fork-PR pipelines and a manual same-repo branch fallback is needed
(see the fork-PR coverage note above). Blacksmith is retired for
this repo and no longer bills recurring PR cost. Windows executor rates
depend on the CircleCI plan, so set an organization credit alert before
enabling the PR check or releases.

Rerunning a failed build creates a new temporary WorkRoot and remains pinned to
the tag's full SHA. If a publish job partially uploads, rerun it after approval:
matching assets are skipped and missing assets are added. A same-name digest
mismatch fails without replacement. If a final release already exists, the
publisher refuses to touch it. Rollback is a deliberate GitHub administrator
action; replacement artifacts require a new reviewed tag/SHA, not an overwrite.

There is no upload switch on `windows-release-build.ps1`; all publication goes
through the approval-gated no-clobber publisher.
