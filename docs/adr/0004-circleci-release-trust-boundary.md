# ADR 0004: CircleCI release-only pipeline and dual-CI trust boundary

Date: 2026-08-14
Status: Accepted; supersedes the release-local portion of ADR 0001

## Context

ADR 0001 kept all release packaging and GitHub publication on an operator's
Windows server while Blacksmith GitHub Actions handled PR checks. We still need
Blacksmith to remain the primary PR/push CI, but a repeatable hosted Windows
release build is safer than an ad-hoc workstation. Release credentials must not
be exposed to build tooling or ordinary CI paths.

## Decision

Add `.circleci/config.yml` for the canonical `nesszer/Win-CodexBar` project.
CircleCI is a release-only pipeline with exact `vX.Y.Z` tag filters and an
explicit branch-ignore filter. `scripts/release-preflight.ps1` is a second
boundary: it validates the canonical remote, full immutable tag SHA, protected
`main` ancestry, and all project version files.

The `release-build` job uses hosted Windows without a GitHub write credential.
It provisions/asserts pinned-enough prerequisites, uses a fixed WorkRoot
(`~/cb/release`) with persistent caches (Cargo registry, Cargo target, and
pnpm store), runs release-doctor, and invokes `windows-release-build.ps1` with
the immutable SHA and `-SmokeInstall`. The WorkRoot's `cache/` subdirectory
survives between runs while `source/` and `assets/` are cleaned fresh. It emits
six expected assets — `CodexBar-<version>-Setup.exe` and its `.sha256`
sidecar, `CodexBar-<version>-portable.exe` and its `.sha256` sidecar,
`CodexBarCLI-v<version>-windows-x64.zip` and its `.sha256` sidecar — plus a
manifest and logs into a persisted workspace. The builder has no publication
path.

A required CircleCI approval separates build from `release-publish`. Only that
job receives the project-restricted `github-release-publisher` context with a
fine-grained `GH_TOKEN` scoped to repository Contents read/write. No Workflows
permission is needed. `publish-github-release.ps1` creates or uses a draft
release, compares existing assets by SHA-256, uploads only missing assets, fails
on a digest mismatch, and never clobbers or finalizes a release.

## Trust boundaries

- **Blacksmith Actions:** primary PR/push validation; unchanged permissions and
  runner responsibilities.
- **CircleCI build:** untrusted/reproducible packaging boundary; no GitHub write
  secret, no release API calls, immutable source, fixed WorkRoot with
  persistent build caches (see "Compiled-output caching" below).
- **Human approval:** reviews persisted manifest/logs before publication.
- **CircleCI publisher context:** sole release write capability; draft-only,
  hash-safe, idempotent publisher.
- **GitHub administrators:** protect `main` and the `v*` tag namespace and
  manually finalize or roll back releases.

## Consequences

- Release builds consume CircleCI hosted Windows credits only for protected
  semver tags; branch and PR builds remain on Blacksmith.
- A partial upload can be retried safely: exact assets are skipped and a
  different digest is a hard failure rather than an overwrite.
- Final release publication remains a deliberate GitHub action.
- CircleCI project/context creation, token storage, tag rulesets, and billing
  alerts remain manual administrator setup.

## Compiled-output caching

The initial design avoided persistent compiled-output caches for OSS fork
safety: a random GUID-based WorkRoot was created and deleted per run, ensuring
no state carried between builds. This is now **superseded** — the pipeline
enables Cargo registry, Cargo target, and pnpm store caching via CircleCI
`save_cache`/`restore_cache` and a fixed WorkRoot (`~/cb/release`).

### What is cached

1. **Cargo registry** (`~/.cargo/registry`) — downloaded crate sources, keyed
   on `Cargo.lock` checksum.
2. **Cargo target** (`~/cb/release/cache/cargo-target` +
   `cargo-target-cli`) — compiled build artifacts, keyed on `Cargo.lock` +
   both `Cargo.toml` checksums, with a partial-match fallback key.
3. **pnpm store** (`~/cb/release/cache/pnpm-store`) — content-addressable
   node_modules store, cached alongside the cargo target under the same key.

### Why it is safe

- **Tag-only pipeline:** the workflow filters accept only `vX.Y.Z` semver tags
  and ignore all branches. A fork cannot trigger the pipeline because pushes
  to a fork do not create tags in the canonical repo.
- **Protected tags:** the `v*` tag namespace is governed by a GitHub ruleset
  that restricts tag creation to administrators.
- **Preflight validation:** `release-preflight.ps1` validates the canonical
  remote (`git origin`), immutable tag-to-SHA identity, and `main` ancestry
  before any build step runs.
- **Cache-key sensitivity:** the cache key includes `Cargo.lock` and both
  `Cargo.toml` checksums, so a different dependency set or version bump
  produces a cache miss rather than a stale-artifact build.
- **No poisoning surface:** cache poisoning from a fork is not possible
  because forks cannot trigger the tag-only pipeline or create protected tags.
