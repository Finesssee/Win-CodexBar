# Win-CodexBar CI and release delivery

## Responsibilities

**Blacksmith GitHub Actions** (`.github/workflows/pr-check.yml`) remains the
primary PR/push validation path. It runs the existing format, clippy, Rust
test, frontend test, and frontend build checks on hosted Blacksmith Windows.

**CircleCI** (`.circleci/config.yml`) is release-only. The workflow is filtered
to the canonical `nesszer/Win-CodexBar` project and exact protected tags
`vX.Y.Z`; branch and pull-request pipelines cannot enter it. The CircleCI
Windows build is credential-free. Only its explicit approval-gated publisher
gets the restricted `GH_TOKEN` context.

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

Protect `main`, require the existing Blacksmith checks, and protect the `v*`
tag namespace so only authorized maintainers can create canonical `vX.Y.Z` tags.
Configure CircleCI credit/spend alerts and notifications as appropriate for
the organization. These project, context, token, ruleset, and billing changes
are intentionally manual.

## Cost, retry, and rollback

Blacksmith billing remains the recurring PR cost. CircleCI Windows credits are
incurred only for a protected release tag and its short approval/publish path;
there is no CircleCI branch or PR build. Windows executor rates depend on the
CircleCI plan, so set an organization credit alert before enabling releases.

Rerunning a failed build creates a new temporary WorkRoot and remains pinned to
the tag's full SHA. If a publish job partially uploads, rerun it after approval:
matching assets are skipped and missing assets are added. A same-name digest
mismatch fails without replacement. If a final release already exists, the
publisher refuses to touch it. Rollback is a deliberate GitHub administrator
action; replacement artifacts require a new reviewed tag/SHA, not an overwrite.

There is no upload switch on `windows-release-build.ps1`; all publication goes
through the approval-gated no-clobber publisher.
