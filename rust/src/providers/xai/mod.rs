//! xAI developer-platform provider (Management API billing).
//!
//! Intentionally separate from the Grok consumer provider:
//! - `GET https://management-api.x.ai/v1/billing/teams/{team_id}/prepaid/balance`
//! - `POST https://management-api.x.ai/v1/billing/teams/{team_id}/usage`
//!
//! Ported from steipete/CodexBar v0.47.0 `XAIBillingFetcher` /
//! `XAIProviderDescriptor` / `XAIUsageSnapshot`.

use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::{DateTime, Datelike, Duration, NaiveDate, Timelike, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::{
    CostDailyPoint, CostSnapshot, FetchContext, Provider, ProviderError, ProviderFetchResult,
    ProviderId, ProviderMetadata, RateWindow, SourceMode, UsageSnapshot,
};

const BASE_URL: &str = "https://management-api.x.ai";
const CREDENTIAL_TARGET: &str = "codexbar-xai";
const ENV_API_KEYS: &[&str] = &["XAI_MANAGEMENT_API_KEY"];
const ENV_TEAM_ID: &str = "XAI_TEAM_ID";
const HISTORY_DAYS: i64 = 30;
const REQUEST_TIMEOUT_SECS: u64 = 15;

#[derive(Debug, Deserialize)]
struct BalanceEnvelope {
    total: BalanceAmount,
}

#[derive(Debug, Deserialize)]
struct BalanceAmount {
    /// Inverted ledger cents as a string (a $10 top-up is "-1000").
    val: String,
}

#[derive(Debug, Deserialize)]
struct UsageEnvelope {
    #[serde(default, rename = "timeSeries")]
    time_series: Vec<UsageSeries>,
    #[serde(default, rename = "limitReached")]
    limit_reached: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct UsageSeries {
    #[serde(default, rename = "dataPoints")]
    data_points: Vec<UsageDataPoint>,
}

#[derive(Debug, Deserialize)]
struct UsageDataPoint {
    timestamp: String,
    #[serde(default)]
    values: Vec<f64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageRequestEnvelope {
    analytics_request: AnalyticsRequest,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AnalyticsRequest {
    time_range: TimeRange,
    time_unit: &'static str,
    values: [AnalyticsValue; 1],
    group_by: [String; 0],
    filters: [String; 0],
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TimeRange {
    start_time: String,
    end_time: String,
    timezone: &'static str,
}

#[derive(Debug, Serialize)]
struct AnalyticsValue {
    name: &'static str,
    aggregation: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
struct DailyBucket {
    day: String,
    cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq)]
struct XaiUsageSnapshot {
    balance_usd: f64,
    daily: Vec<DailyBucket>,
    history_days: i64,
    limit_reached: bool,
    updated_at: DateTime<Utc>,
}

impl XaiUsageSnapshot {
    fn window_cost_usd(&self) -> f64 {
        self.daily.iter().map(|b| b.cost_usd).sum()
    }

    fn history_window_period_label(&self) -> String {
        let base = if self.history_days == 1 {
            "Today".to_string()
        } else {
            format!("Last {} days", self.history_days)
        };
        if self.limit_reached {
            format!("{base} (partial)")
        } else {
            base
        }
    }
    fn to_usage_snapshot(&self) -> UsageSnapshot {
        let balance_text = format_balance_line(self.balance_usd);
        let mut detail = balance_text.clone();
        if !self.daily.is_empty() {
            detail = format!(
                "{detail} · {}: ${:.2}",
                self.history_window_period_label(),
                self.window_cost_usd()
            );
        }

        // Prepaid money is not a quota meter.
        UsageSnapshot::new(RateWindow::informational(detail)).with_login_method("Management API")
    }

    fn to_cost_snapshot(&self) -> CostSnapshot {
        // Menu card prefers `balance` when limit is absent (shows period title + balance).
        // `used` carries the best-effort 30-day window spend for any secondary UI.
        let mut cost = CostSnapshot::new(self.window_cost_usd().max(0.0), "USD", "Prepaid credits")
            .with_daily(
                self.daily
                    .iter()
                    .map(|bucket| CostDailyPoint {
                        day: bucket.day.clone(),
                        amount: bucket.cost_usd.max(0.0),
                    })
                    .collect(),
            );
        // `with_balance` clamps negatives to 0; deficit is still shown in the
        // primary reset description above.
        if self.balance_usd.is_finite() && self.balance_usd >= 0.0 {
            cost = cost.with_balance(self.balance_usd);
        } else if self.balance_usd.is_finite() {
            cost = cost.with_balance(0.0);
        }
        cost
    }
}

fn format_balance_line(balance_usd: f64) -> String {
    if balance_usd < 0.0 {
        format!("Deficit: ${:.2}", -balance_usd)
    } else {
        format!("Balance: ${:.2}", balance_usd)
    }
}

pub struct XaiProvider {
    metadata: ProviderMetadata,
    client: Client,
}

impl XaiProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Xai,
                display_name: "xAI",
                session_label: "Spend",
                weekly_label: "Spend",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://console.x.ai"),
                status_page_url: Some("https://status.x.ai"),
            },
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    fn resolve_api_key(api_key: Option<&str>) -> Result<String, ProviderError> {
        let raw = crate::providers::resolve_api_key(api_key, CREDENTIAL_TARGET, ENV_API_KEYS)?;
        clean_value(&raw).ok_or_else(|| {
            ProviderError::NotInstalled(
                "Missing xAI Management API key. Add one in Settings or set XAI_MANAGEMENT_API_KEY. \
                 Inference API keys are not accepted by the Management API."
                    .to_string(),
            )
        })
    }

    fn resolve_team_id(workspace_id: Option<&str>) -> Result<String, ProviderError> {
        if let Some(id) = workspace_id.and_then(clean_value) {
            validate_team_id(&id)?;
            return Ok(id);
        }
        if let Ok(env) = std::env::var(ENV_TEAM_ID)
            && let Some(id) = clean_value(&env)
        {
            validate_team_id(&id)?;
            return Ok(id);
        }
        Err(ProviderError::NotInstalled(
            "Missing xAI team ID. Add it in Settings or set XAI_TEAM_ID \
             (shown in the xAI Console URL and team settings)."
                .to_string(),
        ))
    }

    async fn fetch_usage_api(
        &self,
        ctx: &FetchContext,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let api_key = Self::resolve_api_key(ctx.api_key.as_deref())?;
        let team_id = Self::resolve_team_id(ctx.workspace_id.as_deref())?;
        let now = Utc::now();
        let snapshot = self.fetch_usage_snapshot(&api_key, &team_id, now).await?;
        Ok(
            ProviderFetchResult::new(snapshot.to_usage_snapshot(), "api")
                .with_cost(snapshot.to_cost_snapshot()),
        )
    }

    async fn fetch_usage_snapshot(
        &self,
        api_key: &str,
        team_id: &str,
        now: DateTime<Utc>,
    ) -> Result<XaiUsageSnapshot, ProviderError> {
        let balance_usd = self.fetch_balance_usd(api_key, team_id).await?;

        // History is best-effort enrichment: the balance is independently useful,
        // so only credential problems escalate from the usage call.
        let (daily, limit_reached) = match self.fetch_daily_usage(api_key, team_id, now).await {
            Ok(pair) => pair,
            Err(ProviderError::AuthRequired) => return Err(ProviderError::AuthRequired),
            Err(err) if is_auth_like(&err) => return Err(err),
            Err(_) => (Vec::new(), false),
        };

        Ok(XaiUsageSnapshot {
            balance_usd,
            daily,
            history_days: HISTORY_DAYS,
            limit_reached,
            updated_at: now,
        })
    }

    async fn fetch_balance_usd(&self, api_key: &str, team_id: &str) -> Result<f64, ProviderError> {
        let url = team_url(team_id, &["prepaid", "balance"])?;
        let response = self
            .client
            .get(url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Accept", "application/json")
            .send()
            .await?;
        map_status_error(response.status())?;
        let envelope: BalanceEnvelope = response
            .json()
            .await
            .map_err(|e| ProviderError::Parse(format!("Could not parse xAI billing data: {e}")))?;
        balance_usd_from_ledger_cents(&envelope.total.val)
    }

    async fn fetch_daily_usage(
        &self,
        api_key: &str,
        team_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(Vec<DailyBucket>, bool), ProviderError> {
        let url = team_url(team_id, &["usage"])?;
        let body = usage_request_body(now);
        let response = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;
        map_status_error(response.status())?;

        let envelope: UsageEnvelope = response
            .json()
            .await
            .map_err(|e| ProviderError::Parse(format!("Could not parse xAI usage history: {e}")))?;

        aggregate_daily_usage(&envelope)
    }
}

fn aggregate_daily_usage(
    envelope: &UsageEnvelope,
) -> Result<(Vec<DailyBucket>, bool), ProviderError> {
    let mut totals: BTreeMap<String, f64> = BTreeMap::new();
    for series in &envelope.time_series {
        for point in &series.data_points {
            let day = utc_day_from_timestamp(&point.timestamp)?;
            let value = point.values.first().copied().unwrap_or(0.0);
            *totals.entry(day).or_default() += value;
        }
    }
    let daily = totals
        .into_iter()
        .map(|(day, cost_usd)| DailyBucket { day, cost_usd })
        .collect();
    Ok((daily, envelope.limit_reached.unwrap_or(false)))
}

impl Default for XaiProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for XaiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Xai
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto | SourceMode::OAuth => self.fetch_usage_api(ctx).await,
            SourceMode::Web | SourceMode::Cli => {
                Err(ProviderError::UnsupportedSource(ctx.source_mode))
            }
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        // Upstream source modes: auto + api (OAuth slot = API key path here).
        vec![SourceMode::Auto, SourceMode::OAuth]
    }
}

fn clean_value(raw: &str) -> Option<String> {
    let mut value = raw.trim().to_string();
    if (value.starts_with('"') && value.ends_with('"'))
        || (value.starts_with('\'') && value.ends_with('\''))
    {
        value = value[1..value.len() - 1].trim().to_string();
    }
    if let Some(stripped) = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
    {
        value = stripped.trim().to_string();
    }
    (!value.is_empty()).then_some(value)
}

fn validate_team_id(team_id: &str) -> Result<(), ProviderError> {
    if team_id.contains('/') || team_id == "." || team_id == ".." {
        return Err(ProviderError::Other(
            "The xAI team ID must be a single identifier without path separators.".to_string(),
        ));
    }
    Ok(())
}

/// The ledger records credit as negative cents (a $10 top-up is "-1000"),
/// so remaining balance is the negated cent value in dollars.
fn balance_usd_from_ledger_cents(raw: &str) -> Result<f64, ProviderError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(ProviderError::Parse(format!(
            "balance total.val is not a cent amount: {raw}"
        )));
    }
    // Match upstream: optional leading minus, digits, optional fractional part.
    let valid = value
        .bytes()
        .enumerate()
        .all(|(i, b)| b.is_ascii_digit() || b == b'.' || (i == 0 && b == b'-'))
        && value.chars().filter(|c| *c == '.').count() <= 1
        && value != "-"
        && value != "."
        && value != "-.";
    if !valid {
        return Err(ProviderError::Parse(format!(
            "balance total.val is not a cent amount: {raw}"
        )));
    }
    let cents: f64 = value.parse().map_err(|_| {
        ProviderError::Parse(format!("balance total.val is not a cent amount: {raw}"))
    })?;
    if !cents.is_finite() {
        return Err(ProviderError::Parse(format!(
            "balance total.val is not a cent amount: {raw}"
        )));
    }
    Ok(-cents / 100.0)
}

fn team_url(team_id: &str, suffix: &[&str]) -> Result<reqwest::Url, ProviderError> {
    let mut url = reqwest::Url::parse(BASE_URL)
        .map_err(|e| ProviderError::Other(format!("Invalid xAI base URL: {e}")))?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| ProviderError::Other("Invalid xAI base URL path".into()))?;
        segments.push("v1");
        segments.push("billing");
        segments.push("teams");
        segments.push(team_id);
        for part in suffix {
            segments.push(part);
        }
    }
    Ok(url)
}

fn usage_request_body(now: DateTime<Utc>) -> UsageRequestEnvelope {
    let window_start = (now.date_naive() - Duration::days(HISTORY_DAYS - 1))
        .and_hms_opt(0, 0, 0)
        .map(|naive| DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc))
        .unwrap_or(now);
    UsageRequestEnvelope {
        analytics_request: AnalyticsRequest {
            time_range: TimeRange {
                start_time: format_request_timestamp(window_start),
                end_time: format_request_timestamp(now),
                timezone: "Etc/GMT",
            },
            time_unit: "TIME_UNIT_DAY",
            values: [AnalyticsValue {
                name: "usd",
                aggregation: "AGGREGATION_SUM",
            }],
            group_by: [],
            filters: [],
        },
    }
}

fn format_request_timestamp(dt: DateTime<Utc>) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        dt.year(),
        dt.month(),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second()
    )
}

fn utc_day_from_timestamp(timestamp: &str) -> Result<String, ProviderError> {
    let parsed = DateTime::parse_from_rfc3339(timestamp)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            // Fractional seconds without offset, or plain date.
            DateTime::parse_from_str(timestamp, "%Y-%m-%dT%H:%M:%S%.fZ")
                .map(|dt| dt.with_timezone(&Utc))
        })
        .or_else(|_| {
            NaiveDate::parse_from_str(timestamp, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|naive| DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc))
                .ok_or_else(|| {
                    ProviderError::Parse(format!("usage timestamp is not ISO 8601: {timestamp}"))
                })
        })
        .map_err(|_| {
            ProviderError::Parse(format!("usage timestamp is not ISO 8601: {timestamp}"))
        })?;
    Ok(parsed.format("%Y-%m-%d").to_string())
}

fn map_status_error(status: reqwest::StatusCode) -> Result<(), ProviderError> {
    if status.is_success() {
        return Ok(());
    }
    match status.as_u16() {
        401 | 403 => Err(ProviderError::Other(
            "xAI rejected the Management API key. Create one in the xAI Console under \
             Settings > Management Keys; inference API keys are not accepted."
                .to_string(),
        )),
        404 => Err(ProviderError::Other(
            "xAI returned 404 for this team. Check the team ID, and that the Management key \
             belongs to the same team."
                .to_string(),
        )),
        429 => Err(ProviderError::Other(
            "xAI Management API rate limit exceeded. Usage will refresh on the next cycle."
                .to_string(),
        )),
        code => Err(ProviderError::Other(format!(
            "xAI Management API returned HTTP {code}."
        ))),
    }
}

fn is_auth_like(err: &ProviderError) -> bool {
    match err {
        ProviderError::AuthRequired => true,
        ProviderError::Other(msg) => {
            msg.contains("rejected the Management API key") || msg.contains("HTTP 401")
        }
        _ => false,
    }
}

/// Parse fixture JSON without network (unit tests).
fn parse_snapshot_for_testing(
    balance_json: &str,
    usage_json: Option<&str>,
    now: DateTime<Utc>,
) -> Result<XaiUsageSnapshot, ProviderError> {
    let envelope: BalanceEnvelope = serde_json::from_str(balance_json)
        .map_err(|e| ProviderError::Parse(format!("Could not parse xAI billing data: {e}")))?;
    let balance_usd = balance_usd_from_ledger_cents(&envelope.total.val)?;

    let (daily, limit_reached) = if let Some(usage_json) = usage_json {
        match serde_json::from_str::<UsageEnvelope>(usage_json) {
            Ok(envelope) => aggregate_daily_usage(&envelope)?,
            Err(_) => (Vec::new(), false),
        }
    } else {
        (Vec::new(), false)
    };

    Ok(XaiUsageSnapshot {
        balance_usd,
        daily,
        history_days: HISTORY_DAYS,
        limit_reached,
        updated_at: now,
    })
}

/// Encode the usage request body for assertion in tests.
fn usage_request_json_for_testing(now: DateTime<Utc>) -> Value {
    serde_json::to_value(usage_request_body(now)).expect("usage request serializes")
}

#[cfg(test)]
mod tests {
    use super::*;

    const BALANCE_FIXTURE: &str = r#"{
      "changes": [
        {
          "teamId": "team-1234",
          "changeOrigin": "PURCHASE",
          "topupStatus": "SUCCEEDED",
          "amount": { "val": "-1000" },
          "invoiceId": "fixture-invoice-id",
          "invoiceNumber": "000-000-000-001",
          "createTime": "2026-12-24T15:28:02.308840Z",
          "paymentProcessor": { "kind": "STRIPE" }
        }
      ],
      "total": { "val": "-1000" }
    }"#;

    const USAGE_FIXTURE: &str = r#"{
      "timeSeries": [
        {
          "group": ["Chat grok-4-fixture"],
          "groupLabels": ["Chat grok-4-fixture"],
          "dataPoints": [
            { "timestamp": "2027-01-13T00:00:00Z", "values": [0.75973725] },
            { "timestamp": "2027-01-14T00:00:00Z", "values": [0.5] },
            { "timestamp": "2027-01-15T00:00:00Z", "values": [0] }
          ]
        },
        {
          "group": ["Live search"],
          "groupLabels": ["Live search"],
          "dataPoints": [
            { "timestamp": "2027-01-13T00:00:00Z", "values": [0.5] },
            { "timestamp": "2027-01-14T00:00:00Z", "values": [0] },
            { "timestamp": "2027-01-15T00:00:00Z", "values": [0] }
          ]
        }
      ],
      "limitReached": false
    }"#;

    fn fixture_now() -> DateTime<Utc> {
        // 2027-01-15 08:00:00 UTC
        DateTime::from_timestamp(1_800_000_000, 0).unwrap()
    }

    #[test]
    fn cleans_whitespace_and_quotes() {
        assert_eq!(
            clean_value("  'fixture-management-key'  ").as_deref(),
            Some("fixture-management-key")
        );
        assert_eq!(clean_value("   "), None);
        assert_eq!(clean_value(" \"team-1234\" ").as_deref(), Some("team-1234"));
        assert_eq!(
            clean_value("Bearer xai-mgmt-key").as_deref(),
            Some("xai-mgmt-key")
        );
    }

    #[test]
    fn balance_ledger_mapping() {
        assert!((balance_usd_from_ledger_cents("-1000").unwrap() - 10.0).abs() < 1e-9);
        assert!((balance_usd_from_ledger_cents("2500").unwrap() - (-25.0)).abs() < 1e-9);
        assert!((balance_usd_from_ledger_cents("0").unwrap()).abs() < 1e-9);
        assert!((balance_usd_from_ledger_cents("-333").unwrap() - 3.33).abs() < 1e-9);
        for bad in ["", "n/a", "12abc", " "] {
            assert!(
                balance_usd_from_ledger_cents(bad).is_err(),
                "expected err for {bad:?}"
            );
        }
    }

    #[test]
    fn team_id_path_separators_rejected() {
        assert!(validate_team_id("team-1234").is_ok());
        assert!(validate_team_id("team/../other").is_err());
        assert!(validate_team_id(".").is_err());
        assert!(validate_team_id("..").is_err());
    }

    #[test]
    fn usage_request_window_matches_upstream() {
        let body = usage_request_json_for_testing(fixture_now());
        let analytics = &body["analyticsRequest"];
        let time_range = &analytics["timeRange"];
        assert_eq!(time_range["startTime"], "2026-12-17 00:00:00");
        assert_eq!(time_range["endTime"], "2027-01-15 08:00:00");
        assert_eq!(time_range["timezone"], "Etc/GMT");
        assert_eq!(analytics["timeUnit"], "TIME_UNIT_DAY");
        assert_eq!(analytics["values"][0]["name"], "usd");
        assert_eq!(analytics["values"][0]["aggregation"], "AGGREGATION_SUM");
        assert_eq!(analytics["groupBy"], Value::Array(vec![]));
        assert_eq!(analytics["filters"], Value::Array(vec![]));
    }

    #[test]
    fn parses_balance_and_sums_daily_usage() {
        let snapshot =
            parse_snapshot_for_testing(BALANCE_FIXTURE, Some(USAGE_FIXTURE), fixture_now())
                .unwrap();
        assert!((snapshot.balance_usd - 10.0).abs() < 1e-9);
        assert!(!snapshot.limit_reached);
        assert_eq!(snapshot.history_days, 30);
        assert_eq!(
            snapshot
                .daily
                .iter()
                .map(|b| b.day.as_str())
                .collect::<Vec<_>>(),
            vec!["2027-01-13", "2027-01-14", "2027-01-15"]
        );
        assert!((snapshot.daily[0].cost_usd - 1.25973725).abs() < 1e-9);
        assert!((snapshot.daily[1].cost_usd - 0.5).abs() < 1e-9);
        assert!((snapshot.daily[2].cost_usd).abs() < 1e-9);

        let usage = snapshot.to_usage_snapshot();
        assert_eq!(usage.login_method.as_deref(), Some("Management API"));
        assert!(
            usage
                .primary
                .reset_description
                .as_deref()
                .unwrap_or("")
                .contains("Balance: $10.00")
        );
        assert!(usage.primary.is_informational);

        let cost = snapshot.to_cost_snapshot();
        assert_eq!(cost.balance, Some(10.0));
        assert_eq!(cost.period, "Prepaid credits");
        assert!((cost.used - 1.75973725).abs() < 1e-9);
        assert!(cost.limit.is_none());
    }

    #[test]
    fn malformed_balance_is_parse_error_not_zero() {
        for body in [
            "{}",
            r#"{"total":{}}"#,
            r#"{"total":{"val":""}}"#,
            r#"{"total":{"val":"n/a"}}"#,
            r#"{"error":"forbidden"}"#,
        ] {
            let err =
                parse_snapshot_for_testing(body, Some(USAGE_FIXTURE), fixture_now()).unwrap_err();
            match err {
                ProviderError::Parse(_) => {}
                other => panic!("expected parse error for {body}, got {other:?}"),
            }
        }
    }

    #[test]
    fn malformed_usage_degrades_to_balance_only() {
        let snapshot = parse_snapshot_for_testing(
            BALANCE_FIXTURE,
            Some(r#"{"object":"list"}"#),
            fixture_now(),
        )
        .unwrap();
        assert!((snapshot.balance_usd - 10.0).abs() < 1e-9);
        assert!(snapshot.daily.is_empty());
    }

    #[test]
    fn limit_reached_labels_history_partial() {
        let body = USAGE_FIXTURE.replace(r#""limitReached": false"#, r#""limitReached": true"#);
        let snapshot =
            parse_snapshot_for_testing(BALANCE_FIXTURE, Some(&body), fixture_now()).unwrap();
        assert!(snapshot.limit_reached);
        assert_eq!(
            snapshot.history_window_period_label(),
            "Last 30 days (partial)"
        );
        let usage = snapshot.to_usage_snapshot();
        assert!(
            usage
                .primary
                .reset_description
                .as_deref()
                .unwrap_or("")
                .contains("Last 30 days (partial)")
        );
    }

    #[test]
    fn team_url_encodes_path_component() {
        let url = team_url("team one", &["prepaid", "balance"]).unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("management-api.x.ai"));
        assert!(url.as_str().contains("team%20one"));
        assert!(!url.as_str().contains("team one"));
        assert!(url.path().ends_with("/prepaid/balance"));
    }

    #[test]
    fn metadata_matches_upstream_descriptor() {
        let provider = XaiProvider::new();
        assert_eq!(provider.id(), ProviderId::Xai);
        assert_eq!(provider.metadata().display_name, "xAI");
        assert_eq!(provider.metadata().session_label, "Spend");
        assert_eq!(
            provider.metadata().dashboard_url,
            Some("https://console.x.ai")
        );
        assert_eq!(
            provider.metadata().status_page_url,
            Some("https://status.x.ai")
        );
        assert!(!provider.metadata().supports_credits);
        assert!(!provider.metadata().default_enabled);
        assert_eq!(
            provider.available_sources(),
            vec![SourceMode::Auto, SourceMode::OAuth]
        );
    }

    #[test]
    fn xai_is_distinct_from_grok() {
        assert_ne!(ProviderId::Xai, ProviderId::Grok);
        assert_eq!(ProviderId::Xai.cli_name(), "xai");
        assert_eq!(ProviderId::Grok.cli_name(), "grok");
        assert_eq!(ProviderId::from_cli_name("xai"), Some(ProviderId::Xai));
        assert_eq!(ProviderId::from_cli_name("x.ai"), Some(ProviderId::Xai));
        assert_eq!(ProviderId::from_cli_name("grok"), Some(ProviderId::Grok));
        assert_eq!(
            ProviderId::from_cli_name("supergrok"),
            Some(ProviderId::Grok)
        );
        // XAI has no cookie domain; Grok still scrapes grok.com sessions.
        assert_eq!(ProviderId::Xai.cookie_domain(), None);
        assert_eq!(ProviderId::Grok.cookie_domain(), Some("grok.com"));
    }

    #[test]
    fn missing_credentials_are_actionable() {
        let err = XaiProvider::resolve_api_key(Some("   ")).unwrap_err();
        match err {
            ProviderError::NotInstalled(msg) => {
                assert!(msg.contains("XAI_MANAGEMENT_API_KEY"));
            }
            other => panic!("expected NotInstalled, got {other:?}"),
        }
        let err = XaiProvider::resolve_team_id(Some("  ")).unwrap_err();
        match err {
            ProviderError::NotInstalled(msg) => {
                assert!(msg.contains("XAI_TEAM_ID"));
            }
            other => panic!("expected NotInstalled, got {other:?}"),
        }
    }
}
