//! Dashboard snapshot v1 schema (`/dashboard/v1/snapshot`) and the pure,
//! transport-independent builder.
//!
//! Upstream 0.48.0 reference: `Sources/CodexBarCLI/DashboardPayloads.swift`
//! and `DashboardSnapshotBuilder.swift` at tag `v0.48.0`. JSON field names
//! (camelCase) and defaults (`schemaVersion: 1`,
//! `staleAfterSeconds = max(180, refresh * 3)`, identity redaction
//! `redacted@domain`, sort keys `index * 10` / fallback `10000 + index`) mirror
//! the pinned upstream contract. Dates serialize as ISO-8601 (the upstream web
//! UI parses them with `Date.parse`).
//!
//! Documented divergences (Win-CodexBar architecture):
//! - `credits` is always `null`: Win-CodexBar has no separate CreditsSnapshot
//!   pipeline (balances ride the cost snapshot / extra rate windows).
//! - `status` is always `null`: provider status-page polling is a separate
//!   `--fetch-status` path here; the HTML hides the status chip (#2723 parity).
//! - `display.accentColor` defaults to upstream's own fallback `#6E6E6E`:
//!   provider descriptors here carry no brand color.
//! - `accounts.pace` uses the 7-stage local [`PaceStage`] model (identical
//!   stage names to upstream's `UsagePace.Stage`).

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::core::{ProviderFetchResult, RateWindow, UsagePace, UsageSnapshot};

/// How much account identity a snapshot exposes. Upstream 0.48.0 exposes two
/// CLI modes (`redacted` default, `full` opt-in); upstream's internal `none`
/// case is intentionally not a user-facing knob here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardIdentity {
    Redacted,
    Full,
}

impl DashboardIdentity {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "redacted" => Some(Self::Redacted),
            "full" => Some(Self::Full),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotPayload {
    pub schema_version: u32,
    pub generated_at: DateTime<Utc>,
    pub stale_after_seconds: u32,
    pub host: HostPayload,
    pub providers: Vec<SnapshotProvider>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostPayload {
    pub codex_bar_version: Option<String>,
    pub refresh_interval_seconds: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotProvider {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub source: String,
    pub status: Option<StatusPayload>,
    pub identity: Option<IdentityPayload>,
    pub windows: Vec<WindowPayload>,
    pub credits: Option<CreditsPayload>,
    pub cost: Option<CostPayload>,
    pub display: DisplayPayload,
    pub error: Option<ProviderErrorPayload>,
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accounts: Option<Vec<AccountPayload>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accounts_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusPayload {
    pub level: String,
    pub label: String,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityPayload {
    pub account_email: Option<String>,
    pub plan: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowPayload {
    pub kind: String,
    pub label: String,
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub reset_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreditsPayload {
    pub remaining: f64,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CostPayload {
    #[serde(rename = "todayUSD")]
    pub today_usd: Option<f64>,
    #[serde(rename = "last30DaysUSD")]
    pub last_30_days_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayPayload {
    pub accent_color: String,
    pub sort_key: u32,
    pub priority: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderErrorPayload {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountPayload {
    pub id: String,
    pub label: String,
    pub active: bool,
    pub identity: Option<IdentityPayload>,
    pub windows: Vec<WindowPayload>,
    pub pace: Option<ProviderPacePayload>,
    pub error: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderPacePayload {
    pub primary: Option<PacePayload>,
    pub secondary: Option<PacePayload>,
    pub tertiary: Option<PacePayload>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PacePayload {
    pub stage: String,
    pub delta_percent: f64,
    pub expected_used_percent: f64,
    pub will_last_to_reset: bool,
    pub eta_seconds: Option<f64>,
    /// Always absent upstream in CLI output; kept absent here too (schema parity).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_out_probability: Option<f64>,
    pub summary: String,
}

// ── Builder inputs ────────────────────────────────────────────────────────

/// One collected provider row: the fetch outcome plus routing metadata.
pub struct ProviderFetchEnvelope {
    pub id: String,
    pub display_name: String,
    pub session_label: String,
    pub weekly_label: String,
    pub fetch: Result<ProviderFetchResult, String>,
}

/// Local cost scan data for one provider (codex / claude only upstream).
pub struct RawCostPayload {
    pub today_usd: Option<f64>,
    pub last_30_days_usd: Option<f64>,
}

/// One collected account row for the Claude multi-account section.
pub struct AccountFetchEnvelope {
    pub id: String,
    pub label: String,
    pub active: bool,
    pub fetch: Result<ProviderFetchResult, String>,
}

/// Claude multi-account ("claude-swap" upstream) section input.
pub struct ClaudeAccountsInput {
    pub accounts: Result<Vec<AccountFetchEnvelope>, String>,
}

pub struct SnapshotInput {
    pub providers: Vec<ProviderFetchEnvelope>,
    pub costs: HashMap<String, RawCostPayload>,
    pub claude_accounts: Option<ClaudeAccountsInput>,
    pub identity: DashboardIdentity,
    pub generated_at: DateTime<Utc>,
    pub refresh_seconds: u32,
    pub version: Option<String>,
    /// Ordered provider ids from settings (`provider_order`); position * 10 is
    /// the display sort key (upstream uses config order the same way).
    pub order: Vec<String>,
    pub enabled: BTreeSet<String>,
}

/// Build the stable display-oriented snapshot (pure; no I/O).
pub fn build_snapshot(input: &SnapshotInput) -> SnapshotPayload {
    let mut sort_keys: HashMap<&str, u32> = HashMap::new();
    for (index, id) in input.order.iter().enumerate() {
        sort_keys.entry(id.as_str()).or_insert(index as u32 * 10);
    }

    let known_ids: BTreeSet<&str> = crate::core::cli_name_map().keys().copied().collect();
    let mut claude_attached = false;
    let providers = input
        .providers
        .iter()
        .enumerate()
        .map(|(index, envelope)| {
            // Provider-specific by design (upstream parity): account data
            // belongs only on the FIRST claude row.
            let claude = if !claude_attached && envelope.id == "claude" {
                claude_attached = true;
                input.claude_accounts.as_ref()
            } else {
                None
            };
            let sort_key = sort_keys
                .get(envelope.id.as_str())
                .copied()
                .unwrap_or(10_000 + index as u32);
            build_provider(envelope, &input.costs, input, &known_ids, sort_key, claude)
        })
        .collect();

    let refresh = input.refresh_seconds;
    SnapshotPayload {
        schema_version: 1,
        generated_at: input.generated_at,
        stale_after_seconds: (refresh.saturating_mul(3)).max(180),
        host: HostPayload {
            codex_bar_version: input.version.clone(),
            refresh_interval_seconds: refresh,
        },
        providers,
    }
}

fn build_provider(
    envelope: &ProviderFetchEnvelope,
    costs: &HashMap<String, RawCostPayload>,
    input: &SnapshotInput,
    known_ids: &BTreeSet<&str>,
    sort_key: u32,
    claude: Option<&ClaudeAccountsInput>,
) -> SnapshotProvider {
    let cost = costs.get(&envelope.id).and_then(|raw| {
        (raw.today_usd.is_some() || raw.last_30_days_usd.is_some()).then_some(CostPayload {
            today_usd: raw.today_usd,
            last_30_days_usd: raw.last_30_days_usd,
        })
    });

    let (source, identity, windows, updated_at, error) = match &envelope.fetch {
        Ok(result) => {
            let source = dashboard_source(&result.source_label);
            let identity = make_identity(&result.usage, input.identity);
            let windows = make_windows(
                &envelope.session_label,
                &envelope.weekly_label,
                &result.usage,
            );
            (
                source,
                identity,
                windows,
                Some(result.usage.updated_at),
                None,
            )
        }
        Err(message) => (
            "unknown".to_string(),
            None,
            Vec::new(),
            Some(input.generated_at),
            Some(ProviderErrorPayload {
                code: 1,
                message: message.clone(),
                kind: Some("provider".to_string()),
            }),
        ),
    };

    let (accounts, accounts_error) = match claude {
        Some(claude) => match &claude.accounts {
            Ok(accounts) => (
                Some(
                    accounts
                        .iter()
                        .map(|account| {
                            build_account(
                                account,
                                input.identity,
                                &envelope.session_label,
                                &envelope.weekly_label,
                                input.generated_at,
                            )
                        })
                        .collect(),
                ),
                None,
            ),
            Err(adapter_error) => (None, Some(adapter_error.clone())),
        },
        None => (None, None),
    };

    SnapshotProvider {
        id: envelope.id.clone(),
        name: envelope.display_name.clone(),
        // Upstream: known provider ids report config membership; unrecognized
        // payloads stay enabled.
        enabled: !known_ids.contains(envelope.id.as_str()) || input.enabled.contains(&envelope.id),
        source,
        status: None,
        identity,
        windows,
        credits: None,
        cost,
        display: DisplayPayload {
            accent_color: "#6E6E6E".to_string(),
            sort_key,
            priority: "normal".to_string(),
        },
        error,
        updated_at,
        accounts,
        accounts_error,
    }
}

fn build_account(
    account: &AccountFetchEnvelope,
    identity_mode: DashboardIdentity,
    session_label: &str,
    weekly_label: &str,
    generated_at: DateTime<Utc>,
) -> AccountPayload {
    let (identity, windows, pace, error, updated_at) = match &account.fetch {
        Ok(result) => (
            make_identity(&result.usage, identity_mode),
            make_windows(session_label, weekly_label, &result.usage),
            make_pace(&result.usage),
            None,
            Some(result.usage.updated_at),
        ),
        Err(message) => (
            None,
            Vec::new(),
            None,
            Some(message.clone()),
            Some(generated_at),
        ),
    };
    AccountPayload {
        id: account.id.clone(),
        label: account.label.clone(),
        active: account.active,
        identity,
        windows,
        pace,
        error,
        updated_at,
    }
}

fn dashboard_source(source: &str) -> String {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

fn make_identity(usage: &UsageSnapshot, mode: DashboardIdentity) -> Option<IdentityPayload> {
    let account_email = dashboard_email(usage.account_email.as_deref(), mode);
    let plan = usage
        .login_method
        .as_deref()
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
        .map(str::to_string);
    if account_email.is_none() && plan.is_none() {
        None
    } else {
        Some(IdentityPayload {
            account_email,
            plan,
        })
    }
}

/// Upstream redaction: `redacted@domain` (everything before the last `@`
/// replaced); bare values without `@` become just `redacted`.
fn dashboard_email(email: Option<&str>, mode: DashboardIdentity) -> Option<String> {
    let email = email?.trim();
    if email.is_empty() {
        return None;
    }
    match mode {
        DashboardIdentity::Full => Some(email.to_string()),
        DashboardIdentity::Redacted => match email.rfind('@') {
            Some(at) => Some(format!("redacted{}", &email[at..])),
            None => Some("redacted".to_string()),
        },
    }
}

fn make_windows(
    session_label: &str,
    weekly_label: &str,
    usage: &UsageSnapshot,
) -> Vec<WindowPayload> {
    let mut windows = Vec::new();
    windows.push(make_window("session", session_label, &usage.primary));
    if let Some(secondary) = &usage.secondary {
        windows.push(make_window("weekly", weekly_label, secondary));
    }
    push_model_and_tertiary_windows(&mut windows, usage);
    for extra in &usage.extra_rate_windows {
        windows.push(make_window(&extra.id, &extra.title, &extra.window));
    }
    windows
}

/// Shared tail for window mapping: model-specific row plus tertiary row.
fn push_model_and_tertiary_windows(windows: &mut Vec<WindowPayload>, usage: &UsageSnapshot) {
    if let Some(model) = &usage.model_specific {
        windows.push(make_window("model", "Opus", model));
    }
    if let Some(tertiary) = &usage.tertiary {
        windows.push(make_window("tertiary", "Tertiary", tertiary));
    }
}

fn make_window(kind: &str, label: &str, window: &RateWindow) -> WindowPayload {
    let used = window.used_percent.clamp(0.0, 100.0);
    WindowPayload {
        kind: kind.to_string(),
        label: label.to_string(),
        used_percent: used,
        remaining_percent: (100.0 - used).clamp(0.0, 100.0),
        reset_at: window.resets_at,
    }
}

fn make_pace(usage: &UsageSnapshot) -> Option<ProviderPacePayload> {
    let payload = ProviderPacePayload {
        primary: None,
        secondary: usage
            .secondary
            .as_ref()
            .and_then(|window| UsagePace::weekly(window, None, 10080))
            .map(|pace| pace_payload(&pace)),
        tertiary: None,
    };
    (payload.secondary.is_some()).then_some(payload)
}

/// Upstream `PacePayload` mapping: rounded percents, camelCase stage names.
fn pace_payload(pace: &UsagePace) -> PacePayload {
    PacePayload {
        stage: pace_stage_name(pace.stage).to_string(),
        delta_percent: pace.delta_percent.round(),
        expected_used_percent: pace.expected_used_percent.round(),
        will_last_to_reset: pace.will_last_to_reset,
        eta_seconds: pace.eta_seconds.map(|eta| eta.round()),
        run_out_probability: None,
        summary: pace.format_status(),
    }
}

/// Local stage names match upstream `UsagePace.Stage` exactly (camelCase).
fn pace_stage_name(stage: crate::core::PaceStage) -> &'static str {
    match stage {
        crate::core::PaceStage::OnTrack => "onTrack",
        crate::core::PaceStage::SlightlyAhead => "slightlyAhead",
        crate::core::PaceStage::Ahead => "ahead",
        crate::core::PaceStage::FarAhead => "farAhead",
        crate::core::PaceStage::SlightlyBehind => "slightlyBehind",
        crate::core::PaceStage::Behind => "behind",
        crate::core::PaceStage::FarBehind => "farBehind",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{CostSnapshot, RateWindow};

    fn fetch_result(used: f64, email: Option<&str>, plan: Option<&str>) -> ProviderFetchResult {
        let mut usage = UsageSnapshot::new(RateWindow::new(used));
        usage.account_email = email.map(str::to_string);
        usage.login_method = plan.map(str::to_string);
        ProviderFetchResult::new(usage, "oauth")
    }

    fn provider_envelope(fetch: Result<ProviderFetchResult, String>) -> ProviderFetchEnvelope {
        ProviderFetchEnvelope {
            id: "claude".to_string(),
            display_name: "Claude".to_string(),
            session_label: "Session".to_string(),
            weekly_label: "Weekly".to_string(),
            fetch,
        }
    }

    fn input(providers: Vec<ProviderFetchEnvelope>, identity: DashboardIdentity) -> SnapshotInput {
        SnapshotInput {
            providers,
            costs: HashMap::new(),
            claude_accounts: None,
            identity,
            generated_at: DateTime::parse_from_rfc3339("2026-08-08T01:02:03Z")
                .unwrap()
                .with_timezone(&Utc),
            refresh_seconds: 60,
            version: Some("0.48.0-test".to_string()),
            order: vec!["claude".to_string(), "codex".to_string()],
            enabled: BTreeSet::from(["claude".to_string()]),
        }
    }

    #[test]
    fn snapshot_envelope_shape() {
        let payload = build_snapshot(&input(
            vec![provider_envelope(Ok(fetch_result(
                42.0,
                Some("me@example.com"),
                None,
            )))],
            DashboardIdentity::Redacted,
        ));
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["schemaVersion"], 1);
        assert_eq!(json["generatedAt"], "2026-08-08T01:02:03Z");
        assert_eq!(json["staleAfterSeconds"], 180);
        assert_eq!(json["host"]["codexBarVersion"], "0.48.0-test");
        assert_eq!(json["host"]["refreshIntervalSeconds"], 60);
        let row = &json["providers"][0];
        assert_eq!(row["id"], "claude");
        assert_eq!(row["name"], "Claude");
        assert_eq!(row["enabled"], true);
        assert_eq!(row["source"], "oauth");
        assert!(
            row["status"].is_null(),
            "no status pipeline in v1 (parity #2723)"
        );
        assert_eq!(row["identity"]["accountEmail"], "redacted@example.com");
        assert_eq!(row["windows"][0]["kind"], "session");
        assert_eq!(row["windows"][0]["usedPercent"], 42.0);
        assert_eq!(row["windows"][0]["remainingPercent"], 58.0);
        assert!(
            row["credits"].is_null(),
            "no credits pipeline (documented divergence)"
        );
        assert!(row["cost"].is_null());
        assert!(row["error"].is_null());
        assert_eq!(row["display"]["accentColor"], "#6E6E6E");
        assert_eq!(row["display"]["sortKey"], 0);
        assert!(
            row.get("accounts").is_none(),
            "accounts absent without input"
        );
        assert!(row.get("accountsError").is_none());
    }

    #[test]
    fn identity_full_exposes_email() {
        let payload = build_snapshot(&input(
            vec![provider_envelope(Ok(fetch_result(
                1.0,
                Some("me@example.com"),
                Some("Claude Max"),
            )))],
            DashboardIdentity::Full,
        ));
        let row = &serde_json::to_value(&payload).unwrap()["providers"][0];
        assert_eq!(row["identity"]["accountEmail"], "me@example.com");
        assert_eq!(row["identity"]["plan"], "Claude Max");
    }

    #[test]
    fn redaction_handles_missing_at_and_empty() {
        assert_eq!(
            dashboard_email(Some("nobody"), DashboardIdentity::Redacted).as_deref(),
            Some("redacted")
        );
        assert_eq!(
            dashboard_email(Some("  "), DashboardIdentity::Redacted),
            None
        );
        assert_eq!(dashboard_email(None, DashboardIdentity::Full), None);
    }

    #[test]
    fn error_row_uses_provider_error_payload() {
        let payload = build_snapshot(&input(
            vec![provider_envelope(Err("network down".to_string()))],
            DashboardIdentity::Redacted,
        ));
        let row = &serde_json::to_value(&payload).unwrap()["providers"][0];
        assert_eq!(row["error"]["code"], 1);
        assert_eq!(row["error"]["message"], "network down");
        assert_eq!(row["error"]["kind"], "provider");
        assert_eq!(row["source"], "unknown");
        assert_eq!(row["updatedAt"], "2026-08-08T01:02:03Z");
        assert_eq!(row["windows"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn sort_key_falls_back_to_position() {
        let mut other = provider_envelope(Ok(fetch_result(3.0, None, None)));
        other.id = "unknownprovider".to_string();
        let payload = build_snapshot(&input(vec![other], DashboardIdentity::Redacted));
        let row = &serde_json::to_value(&payload).unwrap()["providers"][0];
        assert_eq!(row["display"]["sortKey"], 10_000);
        assert_eq!(row["enabled"], true, "unknown ids stay enabled");
    }

    #[test]
    fn window_kinds_cover_secondary_tertiary_model_extras() {
        let mut usage = UsageSnapshot::new(RateWindow::new(10.0));
        usage.secondary = Some(RateWindow::new(20.0));
        usage.model_specific = Some(RateWindow::new(30.0));
        usage.tertiary = Some(RateWindow::new(40.0));
        usage
            .extra_rate_windows
            .push(crate::core::NamedRateWindow::new(
                "reset-credits",
                "Reset credits",
                RateWindow::new(0.0),
            ));
        let payload = build_snapshot(&input(
            vec![provider_envelope(Ok(ProviderFetchResult::new(
                usage, "cli",
            )))],
            DashboardIdentity::Redacted,
        ));
        let windows = &serde_json::to_value(&payload).unwrap()["providers"][0]["windows"];
        let kinds: Vec<&str> = windows
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["kind"].as_str().unwrap())
            .collect();
        assert_eq!(
            kinds,
            ["session", "weekly", "model", "tertiary", "reset-credits"]
        );
    }

    #[test]
    fn stale_after_floor_and_scaling() {
        let mut input_fast = input(vec![], DashboardIdentity::Redacted);
        input_fast.refresh_seconds = 30;
        assert_eq!(build_snapshot(&input_fast).stale_after_seconds, 180);
        input_fast.refresh_seconds = 120;
        assert_eq!(build_snapshot(&input_fast).stale_after_seconds, 360);
    }

    #[test]
    fn cost_payload_surfaces_today_and_30d() {
        let mut costs = HashMap::new();
        costs.insert(
            "claude".to_string(),
            RawCostPayload {
                today_usd: Some(1.25),
                last_30_days_usd: Some(40.5),
            },
        );
        let mut input = input(
            vec![provider_envelope(Ok(fetch_result(5.0, None, None)))],
            DashboardIdentity::Redacted,
        );
        input.costs = costs;
        let row = &serde_json::to_value(build_snapshot(&input)).unwrap()["providers"][0];
        assert_eq!(row["cost"]["todayUSD"], 1.25);
        assert_eq!(row["cost"]["last30DaysUSD"], 40.5);
    }

    #[test]
    fn claude_accounts_attach_to_first_claude_row_only() {
        let second = ProviderFetchEnvelope {
            id: "claude".to_string(),
            display_name: "Claude".to_string(),
            session_label: "Session".to_string(),
            weekly_label: "Weekly".to_string(),
            fetch: Ok(fetch_result(9.0, None, None)),
        };
        let mut input = input(
            vec![provider_envelope(Ok(fetch_result(3.0, None, None))), second],
            DashboardIdentity::Redacted,
        );
        input.claude_accounts = Some(ClaudeAccountsInput {
            accounts: Ok(vec![AccountFetchEnvelope {
                id: "uuid-1".to_string(),
                label: "Work".to_string(),
                active: true,
                fetch: Ok(fetch_result(66.0, Some("work@corp.example"), None)),
            }]),
        });
        let json = serde_json::to_value(build_snapshot(&input)).unwrap();
        let accounts = json["providers"][0]["accounts"].as_array().unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0]["label"], "Work");
        assert_eq!(accounts[0]["active"], true);
        assert_eq!(
            accounts[0]["identity"]["accountEmail"],
            "redacted@corp.example"
        );
        assert!(json["providers"][1].get("accounts").is_none());
    }

    #[test]
    fn account_payload_serializes_camel_case_updated_at() {
        // Pinned v1: AccountPayload's snake_case `updated_at` field must cross
        // the wire as `updatedAt`. An errored account carries the deterministic
        // `generated_at` timestamp, so this golden is reproducible.
        let mut input = input(
            vec![provider_envelope(Ok(fetch_result(3.0, None, None)))],
            DashboardIdentity::Redacted,
        );
        input.claude_accounts = Some(ClaudeAccountsInput {
            accounts: Ok(vec![AccountFetchEnvelope {
                id: "uuid-1".to_string(),
                label: "Broken".to_string(),
                active: false,
                fetch: Err("cookie expired".to_string()),
            }]),
        });
        let json = serde_json::to_value(build_snapshot(&input)).unwrap();
        let account = &json["providers"][0]["accounts"][0];
        assert_eq!(account["error"], "cookie expired");
        assert_eq!(account["updatedAt"], "2026-08-08T01:02:03Z");
        assert!(
            account.get("updated_at").is_none(),
            "snake_case updated_at must not appear on the v1 wire"
        );
    }

    #[test]
    fn status_payload_serializes_camel_case_updated_at() {
        // v1 has no live status pipeline (status is null on rows), but the
        // schema struct itself must still serialize camelCase to match the
        // pinned v1 contract when a status is eventually attached.
        let status = StatusPayload {
            level: "ok".to_string(),
            label: "Healthy".to_string(),
            updated_at: Some(
                DateTime::parse_from_rfc3339("2026-08-08T01:02:03Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
        };
        let json = serde_json::to_value(&status).unwrap();
        assert!(json.get("updatedAt").is_some(), "updatedAt must be present");
        assert_eq!(json["updatedAt"], "2026-08-08T01:02:03Z");
        assert!(
            json.get("updated_at").is_none(),
            "snake_case updated_at must not appear on the v1 wire"
        );
    }

    #[test]
    fn claude_accounts_adapter_error() {
        let mut input = input(
            vec![provider_envelope(Ok(fetch_result(3.0, None, None)))],
            DashboardIdentity::Redacted,
        );
        input.claude_accounts = Some(ClaudeAccountsInput {
            accounts: Err("token store unreadable".to_string()),
        });
        let row = &serde_json::to_value(build_snapshot(&input)).unwrap()["providers"][0];
        assert_eq!(row["accountsError"], "token store unreadable");
        assert!(row.get("accounts").is_none());
    }

    #[test]
    fn account_error_and_pace_rows() {
        let mut usage = UsageSnapshot::new(RateWindow::new(10.0));
        let mut weekly = RateWindow::new(40.0);
        weekly.resets_at = Some(Utc::now() + chrono::Duration::days(3));
        weekly.window_minutes = Some(10080);
        usage.secondary = Some(weekly);
        usage.account_email = Some("a@b.c".to_string());
        let mut input = input(
            vec![provider_envelope(Ok(fetch_result(3.0, None, None)))],
            DashboardIdentity::Redacted,
        );
        input.claude_accounts = Some(ClaudeAccountsInput {
            accounts: Ok(vec![
                AccountFetchEnvelope {
                    id: "u1".to_string(),
                    label: "Main".to_string(),
                    active: true,
                    fetch: Ok(ProviderFetchResult::new(usage, "oauth")),
                },
                AccountFetchEnvelope {
                    id: "u2".to_string(),
                    label: "Broken".to_string(),
                    active: false,
                    fetch: Err("cookie expired".to_string()),
                },
            ]),
        });
        let accounts =
            serde_json::to_value(build_snapshot(&input)).unwrap()["providers"][0]["accounts"]
                .as_array()
                .unwrap()
                .clone();
        assert_eq!(accounts[0]["windows"][0]["kind"], "session");
        assert_eq!(accounts[0]["windows"][1]["kind"], "weekly");
        let pace = &accounts[0]["pace"]["secondary"];
        assert!(pace["stage"].is_string());
        assert!(pace["expectedUsedPercent"].is_number());
        assert!(pace["summary"].is_string());
        assert_eq!(accounts[1]["error"], "cookie expired");
        assert!(accounts[1]["pace"].is_null());
    }

    #[test]
    fn status_is_null_in_v1_so_chip_is_hidden() {
        // #2723 parity: no status pipeline feeds dashboard v1, so every row
        // reports status null and the shell never renders a chip.
        let payload = build_snapshot(&input(
            vec![provider_envelope(Ok(fetch_result(1.0, None, None)))],
            DashboardIdentity::Redacted,
        ));
        assert!(serde_json::to_value(&payload).unwrap()["providers"][0]["status"].is_null());
    }

    #[test]
    fn cost_snapshot_from_fetch_does_not_leak_into_credits() {
        let mut result = fetch_result(1.0, None, None);
        result.cost = Some(CostSnapshot::new(500.5, "credits", "Monthly"));
        let payload = build_snapshot(&input(
            vec![provider_envelope(Ok(result))],
            DashboardIdentity::Redacted,
        ));
        assert!(serde_json::to_value(&payload).unwrap()["providers"][0]["credits"].is_null());
    }
}
