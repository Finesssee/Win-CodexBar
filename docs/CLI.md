# CodexBar CLI (Windows)

Windows rewrite of upstream `docs/cli.md` for the **`codexbar`** binary built from `rust/`.
Upstream install paths (`/Applications`, Homebrew, Sparkle-bundled Helpers) do **not** apply.

## Install / build

```powershell
# From repo root
cargo build -p codexbar --release
# Binary: target\release\codexbar.exe  (or target\<triple>\release\ under some setups)

cargo run -p codexbar -- --help
```

Release installers may place CLI next to the desktop app; for development, run the cargo-built `codexbar.exe` or put it on `PATH` yourself. There is no “Preferences → Install CLI” symlink flow like macOS.

## Configuration

CLI and desktop app share the same Windows config directory (see [CONFIGURATION.md](./CONFIGURATION.md)):

- Settings: `%AppData%\Roaming\CodexBar\settings.json`
- Manual cookies / API keys / token accounts: sibling files under that folder

```powershell
codexbar config path
codexbar config validate
codexbar config dump
```

## Commands (current)

Top-level (from `codexbar --help`):

| Command | Purpose |
|---------|---------|
| `usage` | Print usage from enabled providers (default-style workflow; also global `-p` / `-f`) |
| `cost` | Local token cost usage (Claude + Codex session scans; no web required for those) |
| `guard` | Gate automation on remaining quota for one provider |
| `diagnose` | Export safe provider diagnostics as JSON |
| `sessions` | List or focus local / SSH agent sessions |
| `serve` | HTTP JSON for usage/cost on loopback (dashboard token optional/required by bind) |
| `autostart` | Manage Windows boot auto-start |
| `account` | Token accounts for providers |
| `config` | validate / dump / providers / enable / disable / set-api-key / path |
| `hooks` | List, enable, disable, or test external hooks |

### Usage

```powershell
codexbar usage
codexbar usage -p claude -f json --pretty
codexbar usage -p all --status
codexbar usage --source auto   # auto | web | cli | oauth
codexbar usage --brief
```

Global-style flags (also on root help): `-p/--provider`, `-f/--format`, `--json`, `--pretty`, `--status`, `--all-accounts`, `--account`, `--no-credits`, `--source`, `--web-timeout`, `--brief`.

### Cost

```powershell
codexbar cost
codexbar cost -p codex -f json --pretty
```

Claude/Codex costs come from local session logs. Antigravity exposes local **token history only** through `cost`; dollar cost remains unknown rather than becoming a false `$0`. Other providers may differ; do not assume upstream Cursor dashboard cost behavior unless implemented in this tree.

Codex local-history scans use a 60-second scanner-side debounce for ordinary disk-cache reads. This is separate from the desktop provider refresh setting. With Adaptive refresh off, **Manual** (`refresh_interval_secs = 0`) disables the recurring desktop refresh timer, but it does not forbid startup/stale-aware reads, explicit refreshes, or pending Codex catch-up scans. Low Power Mode floors recurring automatic refreshes to 30 minutes; explicit/manual work remains immediate.

### Guard

```powershell
codexbar guard -p claude --min-remaining 10 --window session
codexbar guard -p codex --json --pretty --fail-open
```

Exit codes (stable intent): `0` ok, `1` below threshold, usage errors for bad args, unavailable when quota cannot be checked (`--fail-open` turns unavailable into `0`).

### Serve

```powershell
codexbar serve --port 8080
# Non-loopback binds need a dashboard token and --allow-plain-http (cleartext bearer).
# Prefer: $env:CODEXBAR_DASHBOARD_TOKEN = '...'
```

Typical endpoints: `/health`, `/usage`, `/cost` (and dashboard snapshot routes when enabled). Loopback default keeps local use simple; treat non-loopback as a threat-model choice (token on every request over HTTP).

### Config

```powershell
codexbar config providers
codexbar config enable -p cursor
codexbar config disable -p cursor
printf '%s' $env:OPENROUTER_API_KEY | codexbar config set-api-key -p openrouter --stdin
codexbar config validate
```

`enable` / `disable` persist settings. `usage -p <id>` is a one-shot override and does not by itself toggle enabled state the same way.

### Sessions

```powershell
codexbar sessions
codexbar sessions --json --pretty
codexbar sessions --focus <session-id>
codexbar sessions --ssh-host user@host
```

### Cache / cookies

Browser cookie import for the app is documented in [COOKIES.md](./COOKIES.md). Prefer Settings → Providers → browser picker on Windows. Manual cookie paste is supported when DPAPI import fails or under WSL.

## Upstream differences (do not copy blindly)

- No Commander/Swift CLI product name `CodexBarCLI`
- No macOS Keychain cookie cache flags as primary docs
- No Homebrew Linux tarball install story as the default Windows path
- Cards / claude-swap–specific CLI behavior from upstream docs may be absent or different — trust `codexbar <cmd> --help` on this binary

## Related

- [CONFIGURATION.md](./CONFIGURATION.md)
- [PROVIDERS.md](./PROVIDERS.md)
- [BUILDING.md](./BUILDING.md)
