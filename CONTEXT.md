# Shared CI budget glossary

This glossary is shared with the sister repo (`linear-cli`) so operators can reason about CI spend across both projects with one vocabulary.

Detailed Win-CodexBar CI topology, trigger configuration, cache policy, release flow, and operator setup belong in [`.github/CI.md`](.github/CI.md). This file intentionally stays small so shared budget language does not become a second CI manual.

## Win-CodexBar compute roles

- **CircleCI Windows** — primary hosted PR/`main` integration gate and release builder.
- **Blacksmith Windows** — manual reserve/second-opinion CI only.
- **GitHub-hosted Ubuntu** — lightweight interaction guard for untrusted authors.

The primary and reserve Windows jobs use the same canonical `scripts/local-check.ps1 -Slice ci` contract. See `.github/CI.md` for the exact trigger and runner rules.

## Blacksmith Pool

The Blacksmith allowance is shared with `linear-cli`. For Win-CodexBar it is reserve capacity rather than recurring PR compute. Do not automatically run Blacksmith beside a healthy CircleCI run.

The historical cross-repo intent split was roughly **60% Win-CodexBar / 30% linear-cli / 10% buffer**. Treat that as budget history, not a reason to schedule duplicate validation.

## CI budget mode

`CI_BUDGET_MODE` is the coarse emergency control. Supported values are `normal`, `thin`, and `off`; unset/empty means `normal`.

| Mode | CircleCI PR check | Interaction guard | Blacksmith reserve | Release |
| --- | --- | --- | --- | --- |
| `normal` | runs when in scope | runs when needed | manual only | tag-triggered CircleCI |
| `thin` | runs when in scope | runs when needed | manual only | tag-triggered CircleCI |
| `off` | skip | skip | skip | tag-triggered CircleCI |

`thin` currently has the same single hosted Windows integration job as `normal`; there is no matrix left to trim. Porting micro PRs save compute through their semantic integration-branch topology instead. See `.github/CI.md` and `docs/PORTING.md`.

Set the variable in provider settings, not source code:

- CircleCI: Project Settings -> Environment Variables -> `CI_BUDGET_MODE`
- GitHub Actions: Settings -> Secrets and variables -> Actions -> Variables

Budget mode does not disable the protected release-tag pipeline.

## Local check slice

The hosted Windows contract is:

```powershell
.\scripts\local-check.ps1 -Slice ci
```

It covers formatting, Clippy, Rust tests, frozen frontend install, frontend tests/build, and interaction-guard tests. Installer/release packaging and UI proof remain separate responsibilities.

For UI, tray, settings, and float-bar changes, CI is not sufficient: use a fresh Windows build plus CUA (or documented equivalent manual proof) as required by `AGENTS.md`.
