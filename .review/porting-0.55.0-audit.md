# Upstream v0.55.0 port closure audit

Target: upstream `v0.54.0...v0.55.0`

This audit is for the Windows Rust/Tauri rewrite. It classifies every behavior-bearing upstream commit in the tagged range. Documentation/appcast/build-number commits are release packaging and are not transplanted. `ALREADY` means the Windows architecture already provides the user-visible behavior without copying the macOS implementation. `SKIP` requires a concrete no-counterpart reason. There are no unresolved `DEFER` items.

| Upstream | Item | Disposition | Windows evidence |
| --- | --- | --- | --- |
| `1f77a5513` #3070 | Simplified Chinese duration-specific quota labels | PORT | PR #379, five-hour generic Session quota renders `5 小时` only for Simplified Chinese quota presentation. |
| `36e2be4a7` #3096 | Bound long agent-session menu names | PORT | PR #375, constrained row name with ellipsis and full title tooltip. |
| `4933e2301` #3098 | Alibaba mainland Personal/Solo `SEC_TOKEN` | PORT | PR #370, console-shell token discovery and upper-case token shape support. |
| `cdc456ae0` #3109 | BigModel CN account balance | PORT | PR #372, CN-only best-effort balance detail without converting balance into spend. |
| `5691a02f8` #3083 | Kiro `GetUsageLimits` overage cap and charges | PORT | PR #374, read-only Kiro CLI identity, overage cap/usage/charge enrichment with CLI fallback. |
| `63bf039cc` #3082 | Distinguish same-email Claude swap accounts and aliases | ALREADY | Windows Claude multi-account uses stable token-account UUIDs plus user labels instead of email-keyed `claude-swap` fleet identities. `TokenAccountStore` and the token-account UI preserve the user label and account UUID. |
| `4394708cc` #3118 | Codex CLI 0.149 approval policy | ALREADY | Windows isolated Codex launches already use `approval_policy = "never"`; no `untrusted` value remains. |
| `5506a78a4` #3116 | Command Code `individual-pro-v1` | PORT | PR #364. |
| `961d7fd10` #3128 | Alibaba empty-Success retry | PORT | PR #368. |
| `416ef870a` #3111 | Claude iCloud email-key to slot-key migration | SKIP | Windows has no CloudKit/iCloud snapshot store or macOS claude-swap iCloud fleet records. |
| `5bc78a2c3` #3132 | Codex profile-scoped token/cost snapshot isolation | ALREADY | Windows managed Codex accounts have distinct UUIDs/home paths; `SnapshotStore` is keyed by account UUID and per-account refresh returns `(account.id, snapshot)`. |
| `1cf98b330` #3105 | Parallel spend-dashboard baseline loads | PORT | PR #382, independent Codex and Claude baselines run concurrently while each provider's 7d/30d scans remain serial. |
| `df18670d6` #3130 | Persist Codex priority trace scan cursor | SKIP | Windows does not implement upstream's `logs_2.sqlite` priority-turn trace scanner, so there is no trace cursor to persist. |
| `d2ef26296` #3112 | Business/Enterprise monthly credit on Codex account cards | ALREADY | Windows Codex API already resolves top-level/nested `individual_limit` and maps it to the account cost/credit snapshot, including Business/Enterprise plan identities. |
| `5c7109524` #3121 | macOS menu-bar layout editor drag/drop | SKIP | Windows does not expose upstream's SwiftUI token-layout editor/trash-drop surface. Provider ordering uses a separate Windows HTML5/button reorder UI. |
| `c87c35bec` #3127 | Cursor Grok Bot weekly allowance | PORT | PR #373, best-effort Sand endpoint and extra Cursor window. |
| `c2701d241` #3148 | Qwen Cloud Brave cookie import | ALREADY | Shared Windows browser detection/import supports Brave and its Windows Chromium cookie store; Qwen is not constrained to upstream's old macOS Chrome-only list. |
| `b843190cb` #3138 | OpenRouter completed UTC day | PORT | PR #366. |
| `0b715a4f6` #3159 | Preserve unknown Grok period usage | PORT | PR #367. |
| `9b034df6d` #3153 | CodexBar CLI install PATH conflict reporting | SKIP | Windows app has no upstream-style CodexBar CLI installer/path-link management surface. |
| `daa16fb84` #3155 | Single-quota icon scaling | PORT | PR #376, one meaningful quota occupies the full Windows tray meter, including secondary-only quota. |
| `6538ac345` #3106 | Spend publication/invalidation refresh behavior | PORT | PR #381, provider publications silently refresh open Usage & Spend and cache revision includes display-affecting cost/daily fields. Upstream's configured-IANA-calendar subpart has no Windows setting counterpart; Windows currently buckets its local dashboard by the host local calendar. |
| `926f5b337` #3119 | Antigravity retired Flash aliases and offline fallback | PORT | PR #377, retired Flash IDs canonicalize to current `gemini-3.7-flash`; offline CLI/app conversation artifacts and tokscale cache provide informational fallback without invented account identity. |
| `7427660af` #3113 | Cursor + Antigravity tokscale local readers | PORT | PRs #378 and #377 respectively. Cursor publishes local CSV cost/tokens; Antigravity publishes local JSONL tokens while cost remains unknown/unpriced. |
| `b83a967f8` #3149 | CHF display-currency conversion option | SKIP | Windows has no upstream display-currency conversion selector/exchange subsystem; it presents source-native currency codes instead of converting totals to a chosen display currency. |
| `d92783689` #3120 | Codex tokscale token parity | PORT | PR #371, max cached field, stale/interleaved cumulative handling, and bare usage rows. |
| `1699ad81b` #3163 | ChatGPT-hosted Codex activity detection | ALREADY | Windows process parser generically recognizes `codex ... app-server` as Codex desktop activity and the local scanner also publishes unmatched recent rollout files, so it is not tied to a macOS signed bundle path. |
| `6de934e7e` #3161 | Reuse signed-in `agy`; signed-out guidance | PORT / ALREADY | Existing Windows Antigravity probe already detects bare/`agy.exe` CLI, prefers IDE when both run, and sends tokenless CLI quota requests. PR #377 adds explicit CLI auth-required classification for signed-out/keyring-like responses plus offline fallback. |
| `146f47316` #3162 | Claude web cookie refresh in ad-hoc builds / lazy Keychain preflight | SKIP | Fix is specific to macOS Keychain persistence/preflight and ad-hoc code signing. Windows browser-cookie import uses Windows Chromium/DPAPI storage and has no Keychain cache equivalent. |
| `d8d03a122` #3136 | OpenCodex price-once hot path | PORT | PR #380, cached models.dev catalog is loaded once per aggregate and passed through row pricing while request-day pricing remains unchanged. |
| `90e6651cd` #3139 | Gemini live consumer-tier shutdown response | PORT | PR #369, recognizes shutdown and restores Antigravity migration guidance. |
| `d21639270` #3166 | Preserve Warp bonus lane in merged macOS icon | SKIP | Windows tray rendering chooses one provider snapshot for the tray icon and does not implement upstream's `.combined` merged-icon compositor. Direct Warp quota windows are already preserved. |
| `07bd85a5e` #3150 | Codex day cost vs priority trace row ownership | SKIP | Depends on upstream `logs_2.sqlite` trace-tier classification, which the Windows cost scanner does not use. |
| `6ef5c5a9a` #3141 | Stop plan-utilization canonical bucket self-merge | SKIP | Windows forecast history is in-memory and does not implement upstream's persisted legacy/opaque/unscoped-to-canonical bucket migration path. |
| `fa50cf2dc`, `27c7f334e` | macOS app version/build bumps | SKIP | Release packaging is intentionally separate from the port PR. Windows remains on its release baseline until the dedicated Win-CodexBar release/version change. |
| appcast/changelog/docs-only commits in range | Upstream release metadata | SKIP | Documentation/release metadata is evidence for this audit, not Windows runtime behavior to transplant. |

## Review PR set

`#364`, `#366`, `#367`, `#368`, `#369`, `#370`, `#371`, `#372`, `#373`, `#374`, `#375`, `#376`, `#377`, `#378`, `#379`, `#380`, `#381`, `#382`.

## Closure state

- PORT items: implemented in the review PR set above.
- ALREADY items: backed by concrete Windows architecture/source evidence above.
- SKIP items: each has a platform/no-counterpart reason above.
- DEFER: **0**.
- Upstream writes: **0**.
- Windows version bump/release packaging: intentionally not part of the behavioral port.

Final merge readiness still requires the normal per-PR CI plus the repository's aggregate validation/thermo/CUA gates where applicable; those gates do not change the PORT/SKIP classification above.
