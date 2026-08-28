//! Doubao / Volcengine Ark provider implementation.
//!
//! Probes Ark chat-completions with a one-token request and reads rate-limit headers.
//! Also supports signed Coding Plan API credentials and `arkcli usage plan` (0.45).

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use crate::core::{
    FetchContext, NamedRateWindow, Provider, ProviderError, ProviderFetchResult, ProviderId,
    ProviderMetadata, RateWindow, SourceMode, UsageSnapshot, hex, hmac_sha256, sha256_hex,
};

const DOUBAO_API_URL: &str = "https://ark.cn-beijing.volces.com/api/coding/v3/chat/completions";
const DOUBAO_CODING_PLAN_URL: &str =
    "https://open.volcengineapi.com/?Action=GetCodingPlanUsage&Version=2024-01-01";
const DOUBAO_CREDENTIAL_TARGET: &str = "codexbar-doubao";
const PROBE_MODELS: &[&str] = &[
    "doubao-seed-2.0-code",
    "doubao-1.5-pro-32k",
    "doubao-lite-32k",
];

pub struct DoubaoProvider {
    metadata: ProviderMetadata,
    client: Client,
}

impl DoubaoProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Doubao,
                display_name: "Doubao",
                session_label: "Requests",
                weekly_label: "Usage",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some(
                    "https://console.volcengine.com/ark/region:ark+cn-beijing/usage",
                ),
                status_page_url: None,
            },
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    fn api_key(api_key: Option<&str>) -> Result<String, ProviderError> {
        resolve_api_key(
            api_key,
            DOUBAO_CREDENTIAL_TARGET,
            &["ARK_API_KEY", "DOUBAO_API_KEY", "VOLCENGINE_API_KEY"],
        )
    }

    fn coding_plan_credentials(api_key: Option<&str>) -> Option<DoubaoCodingPlanCredentials> {
        api_key
            .and_then(DoubaoCodingPlanCredentials::parse)
            .or_else(DoubaoCodingPlanCredentials::from_env)
    }

    async fn fetch_api(&self, api_key: &str) -> Result<UsageSnapshot, ProviderError> {
        let mut last_error = None;
        for model in PROBE_MODELS {
            match self.probe(api_key, model).await {
                Ok(result) => {
                    return Ok(self
                        .confirm_ambiguous_zero_remaining(api_key, model, result)
                        .await);
                }
                Err(error @ ProviderError::AuthRequired) => return Err(error),
                Err(error) => {
                    last_error = Some(error);
                }
            }
        }
        Err(last_error
            .unwrap_or_else(|| ProviderError::Other("All Doubao probe models failed".into())))
    }

    async fn fetch_coding_plan(
        &self,
        credentials: &DoubaoCodingPlanCredentials,
    ) -> Result<UsageSnapshot, ProviderError> {
        let body = Vec::new();
        let signed = sign_volcengine_request(credentials, &body, Utc::now())?;
        let response = self
            .client
            .post(DOUBAO_CODING_PLAN_URL)
            .header("Accept", "application/json")
            .header("Content-Type", signed.content_type)
            .header("Host", signed.host)
            .header("X-Date", signed.timestamp)
            .header("X-Content-Sha256", signed.payload_hash)
            .header("Authorization", signed.authorization)
            .body(body)
            .send()
            .await?;

        let status = response.status();
        let bytes = response.bytes().await?;
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ProviderError::AuthRequired);
        }
        if !status.is_success() {
            return Err(ProviderError::Other(format!(
                "Doubao Coding Plan API returned {status}: {}",
                sanitized_body(&String::from_utf8_lossy(&bytes))
            )));
        }

        Ok(coding_plan_snapshot(decode_coding_plan_usage(&bytes)?))
    }

    async fn confirm_ambiguous_zero_remaining(
        &self,
        api_key: &str,
        model: &str,
        initial: DoubaoProbeResult,
    ) -> UsageSnapshot {
        if !initial.has_ambiguous_zero_remaining() {
            return initial.snapshot;
        }

        match self.probe(api_key, model).await {
            Ok(confirmation) if confirmation.status == reqwest::StatusCode::TOO_MANY_REQUESTS => {
                initial.snapshot
            }
            Ok(confirmation) if confirmation.has_ambiguous_zero_remaining() => snapshot_from_parts(
                confirmation.remaining,
                confirmation.limit,
                confirmation.resets_at,
                confirmation.total_tokens,
                false,
            ),
            Ok(confirmation) => confirmation.snapshot,
            Err(error) => {
                tracing::warn!(
                    "Doubao zero-remaining confirmation failed; preserving initial exhausted state: {error}"
                );
                initial.snapshot
            }
        }
    }

    async fn probe(&self, api_key: &str, model: &str) -> Result<DoubaoProbeResult, ProviderError> {
        let response = self
            .client
            .post(DOUBAO_API_URL)
            .bearer_auth(api_key)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(&json!({
                "model": model,
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "hi"}],
            }))
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ProviderError::AuthRequired);
        }

        let status = response.status();
        if status != reqwest::StatusCode::OK && status != reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ProviderError::Other(format!(
                "Doubao probe model {model} returned status {status}"
            )));
        }

        let headers = response.headers().clone();
        let body: serde_json::Value = response.json().await.unwrap_or_else(|_| json!({}));
        Ok(probe_result_from_response(status, &headers, &body))
    }
}

#[derive(Debug)]
struct DoubaoProbeResult {
    snapshot: UsageSnapshot,
    status: reqwest::StatusCode,
    remaining: Option<i64>,
    limit: Option<i64>,
    resets_at: Option<DateTime<Utc>>,
    total_tokens: Option<i64>,
    request_limits_reliable: bool,
}

impl DoubaoProbeResult {
    fn has_ambiguous_zero_remaining(&self) -> bool {
        self.status == reqwest::StatusCode::OK
            && self.request_limits_reliable
            && self.limit.is_some_and(|limit| limit > 0)
            && self.remaining == Some(0)
    }
}

fn probe_result_from_response(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    body: &serde_json::Value,
) -> DoubaoProbeResult {
    let remaining = int_header(headers, "x-ratelimit-remaining-requests");
    let limit = int_header(headers, "x-ratelimit-limit-requests");
    let resets_at = string_header(headers, "x-ratelimit-reset-requests").and_then(parse_reset_time);
    let total_tokens = body
        .get("usage")
        .and_then(|usage| usage.get("total_tokens"))
        .and_then(|value| value.as_i64());
    let request_limits_reliable = if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        limit.is_some()
    } else {
        limit.is_some() && remaining.is_some()
    };

    let snapshot = snapshot_from_parts(
        remaining,
        limit,
        resets_at,
        total_tokens,
        request_limits_reliable,
    );

    DoubaoProbeResult {
        snapshot,
        status,
        remaining,
        limit,
        resets_at,
        total_tokens,
        request_limits_reliable,
    }
}

fn snapshot_from_parts(
    remaining: Option<i64>,
    limit: Option<i64>,
    resets_at: Option<DateTime<Utc>>,
    total_tokens: Option<i64>,
    request_limits_reliable: bool,
) -> UsageSnapshot {
    let effective_remaining = remaining.unwrap_or(0);

    let (used_percent, detail) = if let (Some(remaining), Some(limit)) = (remaining, limit) {
        if request_limits_reliable && limit > 0 {
            let used = (limit - remaining).max(0);
            let percent = used as f64 / limit as f64 * 100.0;
            (percent, format!("{used}/{limit} requests"))
        } else {
            (0.0, "Active - check dashboard for details".to_string())
        }
    } else if let Some(limit) = limit.filter(|limit| request_limits_reliable && *limit > 0) {
        let used = (limit - effective_remaining).max(0);
        let percent = if limit > 0 {
            used as f64 / limit as f64 * 100.0
        } else {
            0.0
        };
        (percent, format!("{used}/{limit} requests"))
    } else if let Some(total_tokens) = total_tokens {
        (0.0, format!("Active - {total_tokens} tokens observed"))
    } else {
        (0.0, "Active - check dashboard for details".to_string())
    };

    let mut window = RateWindow::with_details(used_percent, None, resets_at, Some(detail));
    if window.used_percent.is_nan() {
        window.used_percent = 0.0;
    }
    UsageSnapshot::new(window)
}

fn int_header(headers: &reqwest::header::HeaderMap, name: &str) -> Option<i64> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok())
}

fn string_header(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string())
}

fn parse_reset_time(value: String) -> Option<DateTime<Utc>> {
    let trimmed = value.trim();
    if let Ok(ts) = trimmed.parse::<i64>() {
        return Utc.timestamp_opt(ts, 0).single();
    }
    DateTime::parse_from_rfc3339(trimmed)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

#[derive(Debug)]
struct DoubaoCodingPlanCredentials {
    access_key_id: String,
    secret_access_key: String,
    region: String,
}

impl DoubaoCodingPlanCredentials {
    fn from_env() -> Option<Self> {
        let access_key_id = cleaned_env("VOLCENGINE_ACCESS_KEY_ID")
            .or_else(|| cleaned_env("DOUBAO_ACCESS_KEY_ID"))?;
        let secret_access_key = cleaned_env("VOLCENGINE_SECRET_ACCESS_KEY")
            .or_else(|| cleaned_env("DOUBAO_SECRET_ACCESS_KEY"))?;
        let region = cleaned_env("VOLCENGINE_REGION")
            .or_else(|| cleaned_env("DOUBAO_REGION"))
            .unwrap_or_else(|| "cn-beijing".to_string());
        Some(Self {
            access_key_id,
            secret_access_key,
            region,
        })
    }

    fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.starts_with('{') {
            let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
            let access_key_id = string_key(
                &value,
                &["accessKeyID", "accessKeyId", "access_key_id", "ak"],
            )?;
            let secret_access_key = string_key(
                &value,
                &[
                    "secretAccessKey",
                    "secret_access_key",
                    "secretKey",
                    "secret_key",
                    "sk",
                ],
            )?;
            let region =
                string_key(&value, &["region"]).unwrap_or_else(|| "cn-beijing".to_string());
            return Some(Self {
                access_key_id,
                secret_access_key,
                region,
            });
        }

        let parts = trimmed.split('|').map(str::trim).collect::<Vec<_>>();
        if parts.len() >= 2 && !parts[0].is_empty() && !parts[1].is_empty() {
            return Some(Self {
                access_key_id: parts[0].to_string(),
                secret_access_key: parts[1].to_string(),
                region: parts
                    .get(2)
                    .copied()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("cn-beijing")
                    .to_string(),
            });
        }
        None
    }
}

fn cleaned_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn string_key(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    })
}

#[derive(Debug, Deserialize)]
struct CodingPlanUsageResponse {
    #[serde(rename = "Result")]
    result: CodingPlanResult,
}

#[derive(Debug, Deserialize)]
struct CodingPlanResult {
    #[serde(rename = "Status")]
    status: Option<String>,
    #[serde(rename = "UpdateTimestamp")]
    update_timestamp: Option<f64>,
    #[serde(rename = "QuotaUsage", default)]
    quota_usage: Vec<CodingPlanQuota>,
}

#[derive(Debug, Deserialize)]
struct CodingPlanQuota {
    #[serde(rename = "Level")]
    level: String,
    #[serde(rename = "Percent")]
    percent: f64,
    #[serde(rename = "ResetTimestamp")]
    reset_timestamp: Option<f64>,
}

fn decode_coding_plan_usage(bytes: &[u8]) -> Result<CodingPlanResult, ProviderError> {
    let response: CodingPlanUsageResponse = serde_json::from_slice(bytes)
        .map_err(|e| ProviderError::Parse(format!("Failed to parse Doubao Coding Plan: {e}")))?;
    Ok(response.result)
}

fn coding_plan_snapshot(usage: CodingPlanResult) -> UsageSnapshot {
    let primary = coding_plan_window(
        &usage,
        &["session", "5-hour", "five_hour", "5h"],
        Some(5 * 60),
    )
    .unwrap_or_else(|| RateWindow::new(0.0));
    let mut snapshot = UsageSnapshot::new(primary);
    if let Some(weekly) = coding_plan_window(&usage, &["weekly", "week"], Some(7 * 24 * 60)) {
        snapshot = snapshot.with_secondary(weekly);
    }
    if let Some(monthly) = coding_plan_window(&usage, &["monthly", "month"], Some(30 * 24 * 60)) {
        snapshot = snapshot.with_tertiary(monthly);
    }
    // Agent / team plan buckets from arkcli (level prefixes).
    for (prefix, id_prefix) in [
        ("agent_", "doubao-agent"),
        ("coding_team_", "doubao-coding-team"),
        ("agent_team_", "doubao-agent-team"),
    ] {
        if let Some(w) = coding_plan_window(
            &usage,
            &[
                &format!("{prefix}session"),
                &format!("{prefix}5-hour"),
                &format!("{prefix}five_hour"),
                &format!("{prefix}5h"),
            ],
            Some(5 * 60),
        ) {
            snapshot.extra_rate_windows.push(NamedRateWindow::new(
                format!("{id_prefix}-session"),
                "5-hour",
                w,
            ));
        }
        if let Some(w) = coding_plan_window(
            &usage,
            &[&format!("{prefix}weekly"), &format!("{prefix}week")],
            Some(7 * 24 * 60),
        ) {
            snapshot.extra_rate_windows.push(NamedRateWindow::new(
                format!("{id_prefix}-weekly"),
                "Weekly",
                w,
            ));
        }
        if let Some(w) = coding_plan_window(
            &usage,
            &[&format!("{prefix}monthly"), &format!("{prefix}month")],
            Some(30 * 24 * 60),
        ) {
            snapshot.extra_rate_windows.push(NamedRateWindow::new(
                format!("{id_prefix}-monthly"),
                "Monthly",
                w,
            ));
        }
    }
    if let Some(status) = usage.status.filter(|s| !s.trim().is_empty()) {
        snapshot = snapshot.with_login_method(status);
    }
    if let Some(update) = usage.update_timestamp.and_then(datetime_from_epoch) {
        snapshot.updated_at = update;
    }
    snapshot
}

fn coding_plan_window(
    usage: &CodingPlanResult,
    levels: &[&str],
    minutes: Option<u32>,
) -> Option<RateWindow> {
    let quota = usage.quota_usage.iter().find(|quota| {
        let level = quota.level.to_ascii_lowercase();
        levels.iter().any(|candidate| *candidate == level)
    })?;
    let resets_at = quota.reset_timestamp.and_then(datetime_from_epoch);
    // Monthly windows: expand the 30-day sentinel to the real calendar cycle.
    let window_minutes = match minutes {
        Some(m) if m == 30 * 24 * 60 => RateWindow::monthly_window_minutes(resets_at).or(Some(m)),
        other => other,
    };
    Some(RateWindow::with_details(
        quota.percent,
        window_minutes,
        resets_at,
        None,
    ))
}

// --- arkcli usage plan (upstream 0.45 #2221) ---

#[derive(Debug, Deserialize)]
struct ArkcliUsageResponse {
    #[serde(default)]
    viewer: Option<ArkcliViewer>,
    #[serde(default)]
    items: Vec<ArkcliUsageItem>,
}

#[derive(Debug, Deserialize)]
struct ArkcliViewer {
    #[serde(default, rename = "auth_method")]
    auth_method: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ArkcliUsageItem {
    product: String,
    #[serde(default)]
    subscribed: Option<bool>,
    #[serde(default)]
    periods: Option<Vec<ArkcliPeriod>>,
    #[serde(default, rename = "updated_at")]
    updated_at: Option<f64>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ArkcliPeriod {
    label: String,
    percent: f64,
    #[serde(default, rename = "reset_at")]
    reset_at: Option<String>,
}

fn resolve_arkcli_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("ARKCLI_PATH") {
        let p = PathBuf::from(path.trim());
        if p.is_file() {
            return Some(p);
        }
    }
    which::which("arkcli").ok()
}

fn is_arkcli_auth_error(message: &str) -> bool {
    let n = message.to_ascii_lowercase();
    [
        "not logged in",
        "not authenticated",
        "authentication required",
        "login required",
        "please login",
        "please log in",
    ]
    .iter()
    .any(|s| n.contains(s))
}

fn run_arkcli_usage_plan() -> Result<Vec<u8>, ProviderError> {
    let bin = resolve_arkcli_binary().ok_or_else(|| {
        ProviderError::NotInstalled(
            "arkcli was not found. Install arkcli, run 'arkcli auth login', or configure Doubao API credentials."
                .into(),
        )
    })?;
    let mut child = Command::new(&bin)
        .args(["usage", "plan", "--format", "json"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| ProviderError::Other(format!("Failed to launch arkcli: {e}")))?;

    // ponytail: 15s wall-clock via join timeout isn't available on std Command;
    // kill after wait timeout via a simple timed poll loop.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() >= deadline => {
                // Best-effort teardown of the timed-out child; the outcome is already
                // reported as timed out.
                let _killed = child.kill();
                let _reaped = child.wait();
                return Err(ProviderError::Other(
                    "arkcli usage timed out. Check arkcli authentication and try again.".into(),
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => {
                return Err(ProviderError::Other(format!("arkcli wait failed: {e}")));
            }
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|e| ProviderError::Other(format!("arkcli wait failed: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr.split_whitespace().collect::<Vec<_>>().join(" ");
        if is_arkcli_auth_error(&message) {
            return Err(ProviderError::AuthRequired);
        }
        let code = output.status.code().unwrap_or(-1);
        return Err(ProviderError::Other(format!(
            "arkcli usage failed ({code}): {}",
            if message.is_empty() {
                "unknown error"
            } else {
                &message
            }
        )));
    }
    if output.stdout.len() > 256 * 1024 {
        return Err(ProviderError::Other(
            "arkcli returned too much output. Update arkcli and try again.".into(),
        ));
    }
    Ok(output.stdout)
}

fn decode_arkcli_usage(bytes: &[u8]) -> Result<CodingPlanResult, ProviderError> {
    let response: ArkcliUsageResponse = serde_json::from_slice(bytes)
        .map_err(|e| ProviderError::Parse(format!("Failed to parse arkcli usage: {e}")))?;

    if let Some(method) = response
        .viewer
        .as_ref()
        .and_then(|v| v.auth_method.as_deref())
        .map(str::trim)
        && method.eq_ignore_ascii_case("none")
    {
        return Err(ProviderError::AuthRequired);
    }

    let supported = [
        "agent-plan",
        "coding-plan",
        "agent-plan-team",
        "coding-plan-team",
    ];
    for item in &response.items {
        let product = item.product.to_ascii_lowercase();
        if !supported.iter().any(|p| *p == product) {
            continue;
        }
        if item.subscribed == Some(false) {
            continue;
        }
        let periods_empty = item.periods.as_ref().map(|p| p.is_empty()).unwrap_or(true);
        if periods_empty {
            let message = item
                .error
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("{product} has no usage periods"));
            // Incomplete but keep scanning other products.
            let _ = message;
        }
    }

    let mut quotas = Vec::new();
    let mut update_ts: Option<f64> = None;
    let mut status = response
        .viewer
        .and_then(|v| v.auth_method)
        .filter(|s| !s.trim().is_empty());

    for item in response.items {
        let product = item.product.to_ascii_lowercase();
        let level_prefix = match product.as_str() {
            "agent-plan" => "agent_",
            "coding-plan" => "",
            "agent-plan-team" => "agent_team_",
            "coding-plan-team" => "coding_team_",
            _ => continue,
        };
        if item.subscribed == Some(false) {
            continue;
        }
        let periods = item.periods.unwrap_or_default();
        if !periods.is_empty()
            && let Some(updated_at) = item.updated_at.filter(|v| *v > 0.0)
        {
            // arkcli may emit ms or seconds; 1e11 is the unit threshold.
            let seconds = if updated_at >= 1e11 {
                updated_at / 1000.0
            } else {
                updated_at
            };
            if update_ts.map(|t| seconds > t).unwrap_or(true) {
                update_ts = Some(seconds);
            }
        }
        for period in periods {
            let level = format!("{level_prefix}{}", period.label);
            let reset_timestamp = period
                .reset_at
                .as_deref()
                .and_then(|raw| DateTime::parse_from_rfc3339(raw.trim()).ok())
                .map(|d| d.timestamp() as f64);
            quotas.push(CodingPlanQuota {
                level,
                percent: period.percent,
                reset_timestamp,
            });
        }
        if status.is_none() {
            status = Some(product);
        }
    }

    if quotas.is_empty() {
        return Err(ProviderError::Parse(
            "arkcli returned no active Coding or Agent Plan usage.".into(),
        ));
    }

    Ok(CodingPlanResult {
        status,
        update_timestamp: update_ts,
        quota_usage: quotas,
    })
}

fn fetch_arkcli_usage() -> Result<UsageSnapshot, ProviderError> {
    let stdout = run_arkcli_usage_plan()?;
    let usage = decode_arkcli_usage(&stdout)?;
    Ok(coding_plan_snapshot(usage))
}

fn datetime_from_epoch(timestamp: f64) -> Option<DateTime<Utc>> {
    if !timestamp.is_finite() || timestamp <= 0.0 {
        return None;
    }
    // Epoch seconds guarded finite and positive; the sub-second fraction is
    // below timestamp resolution.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "epoch seconds; sub-second fraction below timestamp resolution"
    )]
    let whole_seconds = timestamp as i64;
    Utc.timestamp_opt(whole_seconds, 0).single()
}

struct SignedVolcengineRequest {
    content_type: &'static str,
    host: String,
    timestamp: String,
    payload_hash: String,
    authorization: String,
}

fn sign_volcengine_request(
    credentials: &DoubaoCodingPlanCredentials,
    body: &[u8],
    now: DateTime<Utc>,
) -> Result<SignedVolcengineRequest, ProviderError> {
    let parsed = reqwest::Url::parse(DOUBAO_CODING_PLAN_URL)
        .map_err(|e| ProviderError::Other(format!("Invalid Doubao Coding Plan URL: {e}")))?;
    let host = parsed
        .host_str()
        .unwrap_or("open.volcengineapi.com")
        .to_string();
    let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date_stamp = now.format("%Y%m%d").to_string();
    let payload_hash = sha256_hex(body);
    let content_type = "application/x-www-form-urlencoded; charset=utf-8";
    let signed_headers = "content-type;host;x-content-sha256;x-date";
    let canonical_request = [
        "POST".to_string(),
        canonical_uri(&parsed),
        canonical_query_string(&parsed),
        format!("content-type:{content_type}"),
        format!("host:{host}"),
        format!("x-content-sha256:{payload_hash}"),
        format!("x-date:{timestamp}"),
        String::new(),
        signed_headers.to_string(),
        payload_hash.clone(),
    ]
    .join("\n");
    let credential_scope = format!("{}/{}/ark/request", date_stamp, credentials.region);
    let string_to_sign = [
        "HMAC-SHA256".to_string(),
        timestamp.clone(),
        credential_scope.clone(),
        sha256_hex(canonical_request.as_bytes()),
    ]
    .join("\n");
    let date_key = hmac_sha256(
        credentials.secret_access_key.as_bytes(),
        date_stamp.as_bytes(),
    );
    let region_key = hmac_sha256(&date_key, credentials.region.as_bytes());
    let service_key = hmac_sha256(&region_key, b"ark");
    let signing_key = hmac_sha256(&service_key, b"request");
    let signature = hex(&hmac_sha256(&signing_key, string_to_sign.as_bytes()));
    let authorization = format!(
        "HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}",
        credentials.access_key_id
    );
    Ok(SignedVolcengineRequest {
        content_type,
        host,
        timestamp,
        payload_hash,
        authorization,
    })
}

fn canonical_uri(url: &reqwest::Url) -> String {
    let path = url.path();
    if path.is_empty() {
        "/".to_string()
    } else {
        percent_encode(path, false)
    }
}

fn canonical_query_string(url: &reqwest::Url) -> String {
    let mut pairs = url
        .query_pairs()
        .map(|(key, value)| (percent_encode(&key, true), percent_encode(&value, true)))
        .collect::<Vec<_>>();
    pairs.sort();
    pairs
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn percent_encode(value: &str, encode_slash: bool) -> String {
    value
        .bytes()
        .flat_map(|byte| {
            let keep = byte.is_ascii_alphanumeric()
                || matches!(byte, b'-' | b'_' | b'.' | b'~')
                || (!encode_slash && byte == b'/');
            if keep {
                vec![byte as char]
            } else {
                format!("%{byte:02X}").chars().collect()
            }
        })
        .collect()
}

fn sanitized_body(body: &str) -> String {
    crate::core::sanitized_body(body, 200)
}

impl Default for DoubaoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for DoubaoProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Doubao
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto | SourceMode::OAuth => {
                if let Some(credentials) = Self::coding_plan_credentials(ctx.api_key.as_deref()) {
                    return Ok(ProviderFetchResult::new(
                        self.fetch_coding_plan(&credentials).await?,
                        "coding-plan",
                    ));
                }
                // Prefer arkcli when available so Agent Plan / team quotas surface
                // without signed API credentials (upstream 0.45).
                if resolve_arkcli_binary().is_some() {
                    match fetch_arkcli_usage() {
                        Ok(snap) => return Ok(ProviderFetchResult::new(snap, "arkcli")),
                        Err(ProviderError::AuthRequired) => {
                            return Err(ProviderError::AuthRequired);
                        }
                        Err(ProviderError::NotInstalled(_)) => {}
                        Err(_) => {
                            // Fall through to request-header probe if configured.
                        }
                    }
                }
                let api_key = Self::api_key(ctx.api_key.as_deref())?;
                Ok(ProviderFetchResult::new(
                    self.fetch_api(&api_key).await?,
                    "api",
                ))
            }
            SourceMode::Cli => {
                let snap = fetch_arkcli_usage()?;
                Ok(ProviderFetchResult::new(snap, "arkcli"))
            }
            SourceMode::Web => Err(ProviderError::UnsupportedSource(ctx.source_mode)),
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::OAuth, SourceMode::Cli]
    }
}

fn resolve_api_key(
    explicit: Option<&str>,
    credential_target: &str,
    env_names: &[&str],
) -> Result<String, ProviderError> {
    if let Some(key) = explicit
        && !key.trim().is_empty()
    {
        return Ok(key.trim().to_string());
    }
    if let Ok(entry) = keyring::Entry::new(credential_target, "api_key")
        && let Ok(key) = entry.get_password()
        && !key.trim().is_empty()
    {
        return Ok(key);
    }
    for env in env_names {
        if let Ok(key) = std::env::var(env)
            && !key.trim().is_empty()
        {
            return Ok(key);
        }
    }
    Err(ProviderError::NotInstalled(format!(
        "API key not found. Set {} in Preferences or environment.",
        env_names.join(" / ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};

    #[test]
    fn doubao_snapshot_uses_rate_limit_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-ratelimit-remaining-requests",
            HeaderValue::from_static("25"),
        );
        headers.insert(
            "x-ratelimit-limit-requests",
            HeaderValue::from_static("100"),
        );
        let snapshot =
            probe_result_from_response(reqwest::StatusCode::OK, &headers, &json!({})).snapshot;
        assert_eq!(snapshot.primary.used_percent, 75.0);
    }

    #[test]
    fn doubao_repeated_successful_zero_remaining_falls_back_to_active() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-ratelimit-remaining-requests",
            HeaderValue::from_static("0"),
        );
        headers.insert(
            "x-ratelimit-limit-requests",
            HeaderValue::from_static("1000"),
        );

        let result = probe_result_from_response(reqwest::StatusCode::OK, &headers, &json!({}));
        assert!(result.has_ambiguous_zero_remaining());

        let snapshot = snapshot_from_parts(
            result.remaining,
            result.limit,
            result.resets_at,
            result.total_tokens,
            false,
        );
        assert_eq!(snapshot.primary.used_percent, 0.0);
        assert_eq!(
            snapshot.primary.reset_description.as_deref(),
            Some("Active - check dashboard for details")
        );
    }

    #[test]
    fn doubao_rate_limit_with_limit_header_reports_exhausted() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-ratelimit-limit-requests",
            HeaderValue::from_static("1000"),
        );
        let snapshot = probe_result_from_response(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &headers,
            &json!({}),
        )
        .snapshot;

        assert_eq!(snapshot.primary.used_percent, 100.0);
        assert_eq!(
            snapshot.primary.reset_description.as_deref(),
            Some("1000/1000 requests")
        );
    }

    #[test]
    fn doubao_bare_rate_limit_uses_active_fallback() {
        let snapshot = probe_result_from_response(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &HeaderMap::new(),
            &json!({}),
        )
        .snapshot;

        assert_eq!(snapshot.primary.used_percent, 0.0);
        assert_eq!(
            snapshot.primary.reset_description.as_deref(),
            Some("Active - check dashboard for details")
        );
    }

    #[test]
    fn doubao_parses_coding_plan_usage() {
        let body = br#"{
            "Result": {
                "Status": "active",
                "UpdateTimestamp": 1783036800,
                "QuotaUsage": [
                    {"Level": "session", "Percent": 12.5, "ResetTimestamp": 1783040400},
                    {"Level": "weekly", "Percent": 50.0, "ResetTimestamp": 1783641600},
                    {"Level": "monthly", "Percent": 75.0, "ResetTimestamp": 1785628800}
                ]
            }
        }"#;
        let snapshot = coding_plan_snapshot(decode_coding_plan_usage(body).unwrap());
        assert_eq!(snapshot.primary.used_percent, 12.5);
        assert_eq!(snapshot.secondary.unwrap().used_percent, 50.0);
        let monthly = snapshot.tertiary.expect("monthly");
        assert_eq!(monthly.used_percent, 75.0);
        // ResetTimestamp 1785628800 = 2026-08-02 → prior month is 31 days.
        assert_eq!(monthly.window_minutes, Some(31 * 24 * 60));
        assert_eq!(snapshot.login_method.as_deref(), Some("active"));
    }

    #[test]
    fn doubao_parses_coding_plan_credentials() {
        let creds =
            DoubaoCodingPlanCredentials::parse("ak-test|sk-test|cn-shanghai").expect("creds");
        assert_eq!(creds.access_key_id, "ak-test");
        assert_eq!(creds.secret_access_key, "sk-test");
        assert_eq!(creds.region, "cn-shanghai");

        let json = r#"{"accessKeyId":"ak-json","secretAccessKey":"sk-json","region":"cn-beijing"}"#;
        let creds = DoubaoCodingPlanCredentials::parse(json).expect("json creds");
        assert_eq!(creds.access_key_id, "ak-json");
        assert_eq!(creds.secret_access_key, "sk-json");
    }

    #[test]
    fn doubao_signer_sets_required_volcengine_headers() {
        let creds = DoubaoCodingPlanCredentials {
            access_key_id: "AKID".into(),
            secret_access_key: "SECRET".into(),
            region: "cn-beijing".into(),
        };
        let signed = sign_volcengine_request(
            &creds,
            b"",
            Utc.with_ymd_and_hms(2026, 7, 3, 0, 0, 0).unwrap(),
        )
        .unwrap();
        assert_eq!(signed.host, "open.volcengineapi.com");
        assert_eq!(signed.timestamp, "20260703T000000Z");
        assert_eq!(signed.payload_hash, sha256_hex(b""));
        assert!(
            signed
                .authorization
                .starts_with("HMAC-SHA256 Credential=AKID/20260703/cn-beijing/ark/request")
        );
    }

    #[test]
    fn arkcli_usage_parses_coding_and_agent_plans() {
        let raw = br#"{
          "viewer": { "auth_method": "oauth" },
          "items": [
            {
              "product": "coding-plan",
              "subscribed": true,
              "updated_at": 1720000000,
              "periods": [
                { "label": "session", "percent": 12.5, "reset_at": "2026-07-21T10:00:00Z" },
                { "label": "weekly", "percent": 40.0, "reset_at": "2026-07-28T00:00:00Z" }
              ]
            },
            {
              "product": "agent-plan",
              "subscribed": true,
              "periods": [
                { "label": "session", "percent": 5.0 },
                { "label": "weekly", "percent": 15.0 }
              ]
            }
          ]
        }"#;
        let usage = decode_arkcli_usage(raw).expect("arkcli json");
        let snap = coding_plan_snapshot(usage);
        assert!((snap.primary.used_percent - 12.5).abs() < 0.01);
        assert!((snap.secondary.as_ref().unwrap().used_percent - 40.0).abs() < 0.01);
        assert!(
            snap.extra_rate_windows
                .iter()
                .any(|w| w.id == "doubao-agent-session")
        );
        assert!(
            snap.extra_rate_windows
                .iter()
                .any(|w| (w.window.used_percent - 5.0).abs() < 0.01)
        );
    }

    #[test]
    fn arkcli_auth_none_is_auth_required() {
        let raw = br#"{ "viewer": { "auth_method": "none" }, "items": [] }"#;
        let err = decode_arkcli_usage(raw).unwrap_err();
        assert!(matches!(err, ProviderError::AuthRequired));
    }
}
