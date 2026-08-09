# Code signing policy

Free code signing of Win-CodexBar releases via SignPath.io, certificate by SignPath Foundation.

> **Status: pending onboarding — not yet implemented.** All current release artifacts (installer and portable build) are **unsigned**; authenticity relies on the SHA-256 `.sha256` sidecar files published alongside each release. The sections below describe the target process once SignPath onboarding and pipeline wiring are complete.

## Project identity

- **Project name:** Win-CodexBar
- **Homepage:** https://github.com/nesszer/Win-CodexBar
- **Source code:** https://github.com/nesszer/Win-CodexBar
- **Releases:** https://github.com/nesszer/Win-CodexBar/releases
- **License:** MIT

## Roles

| Role |
|------|
| Author: Finesssee |
| Reviewer: Finesssee |
| Approver: Finesssee (@Finesssee) |

## Build system

- CI runs on GitHub Actions (`.github/workflows/pr-check.yml`).
- The Windows release pipeline is driven by `scripts/windows-release-build.ps1`, which builds the Tauri release binary plus the console CLI and packages them with Inno Setup into the installer (`CodexBar-<version>-Setup.exe`) and portable build, writing SHA-256 sidecar files for every artifact.
- Release artifacts are published to [GitHub Releases](https://github.com/nesszer/Win-CodexBar/releases).
- **Not yet wired:** release signing will be submitted to SignPath from this pipeline once SignPath onboarding completes; each release-signing request is approved manually by the approver listed above before signed binaries are published.

## Privacy

See [docs/PRIVACY.md](PRIVACY.md) for the project's privacy policy.

## Notes

*The notes below apply once signing is active:*

- Certificates are issued in the SignPath Foundation's name; signed binaries show "SignPath Foundation" as the publisher.
- Every release-signing request requires manual approval per release; no unattended signing is performed.
