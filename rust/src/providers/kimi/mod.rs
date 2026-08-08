//! Kimi AI provider implementation
//!
//! Fetches usage data from Kimi (Moonshot AI).
//!
//! Provider policies are centralized in the submodules:
//! - [`web`]: `kimi.com` cookie auth, browser-import gate (Cookie Source Off,
//!   upstream #2623), and the shared web-token resolution chain
//!   (manual cookie → Kimi Desktop session → browser import).
//! - [`code_api`]: Kimi Code API auth/endpoint/CLI-credential policy, plus the
//!   upstream 0.48.0 monthly-membership enrichment of Code API + CLI usage
//!   from a signed-in Kimi Desktop session (#2622).
//! - [`desktop_token`]: read-only, WAL-safe reader for the Kimi Desktop
//!   (Electron) Chromium cookie store.

mod code_api;
pub mod desktop_token;
mod web;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};

const KIMI_WEB_USAGE_URL: &str =
    "https://www.kimi.com/apiv2/kimi.gateway.billing.v1.BillingService/GetUsages";
const KIMI_SUBSCRIPTION_STATS_URL: &str =
    "https://www.kimi.com/apiv2/kimi.gateway.membership.v2.MembershipService/GetSubscriptionStats";
const KIMI_COOKIE_DOMAINS: [&str; 2] = ["www.kimi.com", "kimi.moonshot.cn"];

#[derive(Debug, Deserialize)]
struct KimiCodeApiUsageResponse {
    usage: KimiUsageDetail,
    #[serde(default)]
    limits: Option<Vec<KimiRateLimit>>,
}

#[derive(Debug, Deserialize)]
struct KimiWebUsageResponse {
    usages: Vec<KimiUsage>,
}

#[derive(Debug, Deserialize)]
struct KimiUsage {
    scope: String,
    detail: KimiUsageDetail,
    #[serde(default)]
    limits: Option<Vec<KimiRateLimit>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KimiSubscriptionStatsResponse {
    subscription_balance: Option<KimiSubscriptionBalance>,
    ratelimit_code7d: Option<KimiSubscriptionRateLimit>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KimiSubscriptionBalance {
    amount_used_ratio: Option<serde_json::Value>,
    expire_time: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KimiSubscriptionRateLimit {
    ratio: Option<serde_json::Value>,
    enabled: Option<bool>,
    reset_time: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct KimiUsageDetail {
    #[serde(default)]
    limit: Option<serde_json::Value>,
    #[serde(default)]
    used: Option<serde_json::Value>,
    #[serde(default)]
    remaining: Option<serde_json::Value>,
    #[serde(
        default,
        rename = "resetTime",
        alias = "resetAt",
        alias = "reset_time",
        alias = "reset_at"
    )]
    reset_time: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct KimiRateLimit {
    window: Option<KimiWindow>,
    detail: KimiUsageDetail,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KimiWindow {
    duration: u32,
    time_unit: String,
}

/// Kimi AI provider
pub struct KimiProvider {
    metadata: ProviderMetadata,
}

impl KimiProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Kimi,
                display_name: "Kimi",
                session_label: "Weekly",
                weekly_label: "Rate Limit",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://kimi.moonshot.cn"),
                status_page_url: None,
            },
        }
    }

    fn auth_token_from_cookie_headers(
        headers: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<String, ProviderError> {
        for header in headers {
            if let Ok(token) = Self::auth_token_from_cookie_header(header.as_ref()) {
                return Ok(token);
            }
        }
        Err(ProviderError::AuthRequired)
    }

    fn auth_token_from_cookie_header(cookie_header: &str) -> Result<String, ProviderError> {
        for cookie in cookie_header.split(';') {
            let cookie = cookie.trim();
            if cookie.starts_with("kimi-auth=")
                || cookie.starts_with("authorization=")
                || cookie.starts_with("access_token=")
            {
                let token = cookie.split('=').nth(1).unwrap_or("").trim();
                if !token.is_empty() {
                    return Ok(token.to_string());
                }
            }
        }
        Err(ProviderError::AuthRequired)
    }

    fn rate_window_from_usage_detail(
        detail: &KimiUsageDetail,
        window_minutes: Option<u32>,
    ) -> Result<RateWindow, ProviderError> {
        let limit = value_as_f64(detail.limit.as_ref())
            .filter(|limit| *limit > 0.0)
            .ok_or_else(|| ProviderError::Parse("Kimi usage limit missing".into()))?;
        let used = match (
            value_as_f64(detail.used.as_ref()),
            value_as_f64(detail.remaining.as_ref()),
        ) {
            (Some(used), _) => used,
            (None, Some(remaining)) => (limit - remaining).max(0.0),
            (None, None) => {
                return Err(ProviderError::Parse(
                    "Kimi usage used/remaining value missing".into(),
                ));
            }
        };
        let reset_at = detail.reset_time.as_ref().and_then(parse_kimi_timestamp);
        let description = Some(format!(
            "{}/{} credits",
            format_usage_amount(used),
            format_usage_amount(limit)
        ));

        Ok(RateWindow::with_details(
            (used / limit) * 100.0,
            window_minutes,
            reset_at,
            description,
        ))
    }
}

impl Default for KimiProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for KimiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Kimi
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Fetching Kimi usage");

        match ctx.source_mode {
            SourceMode::Auto => {
                if code_api::code_api_key(ctx.api_key.as_deref()).is_ok() {
                    match code_api::fetch_via_code_api(ctx, None, None, "Code API").await {
                        Ok(usage) => {
                            return Ok(ProviderFetchResult::new(usage, "code-api"));
                        }
                        Err(err) => {
                            tracing::debug!(
                                error = %err,
                                "Kimi Code API key fetch failed; trying CLI credential / web"
                            );
                        }
                    }
                }

                if let Some(cli_token) = code_api::kimi_code_cli_access_token(unix_now_secs()) {
                    let home = code_api::kimi_code_home().unwrap_or_default();
                    let headers = code_api::kimi_code_cli_identity_headers(&home);
                    match code_api::fetch_via_code_api(
                        ctx,
                        Some(&cli_token),
                        Some(&headers),
                        "Kimi Code CLI",
                    )
                    .await
                    {
                        Ok(usage) => {
                            return Ok(ProviderFetchResult::new(usage, "code-cli"));
                        }
                        Err(err) => {
                            tracing::debug!(
                                error = %err,
                                "Kimi Code CLI credential fetch failed; falling back to web"
                            );
                        }
                    }
                }

                let usage = web::fetch_via_web(ctx.manual_cookie_header.as_deref()).await?;
                Ok(ProviderFetchResult::new(usage, "web"))
            }
            SourceMode::OAuth => {
                let usage = code_api::fetch_via_code_api(ctx, None, None, "Code API").await?;
                Ok(ProviderFetchResult::new(usage, "code-api"))
            }
            SourceMode::Web => {
                let usage = web::fetch_via_web(ctx.manual_cookie_header.as_deref()).await?;
                Ok(ProviderFetchResult::new(usage, "web"))
            }
            SourceMode::Cli => Err(ProviderError::UnsupportedSource(SourceMode::Cli)),
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::Web, SourceMode::OAuth]
    }

    fn supports_web(&self) -> bool {
        true
    }

    fn supports_cli(&self) -> bool {
        false
    }

    fn supports_oauth(&self) -> bool {
        true
    }
}

fn kimi_window_minutes(window: &KimiWindow) -> Option<u32> {
    let unit = window
        .time_unit
        .trim()
        .trim_start_matches("TIME_UNIT_")
        .to_ascii_lowercase();
    match unit.as_str() {
        "second" | "seconds" => Some((window.duration / 60).max(1)),
        "minute" | "minutes" => Some(window.duration),
        "hour" | "hours" => Some(window.duration.saturating_mul(60)),
        "day" | "days" => Some(window.duration.saturating_mul(24 * 60)),
        _ => None,
    }
}

/// Shared merge of the membership-pool windows (`Monthly` + `Code 7-day`)
/// recovered from the subscription-stats endpoint — used by the web fetch and
/// by the upstream 0.48.0 Code-API/CLI enrichment (#2622).
fn apply_subscription_windows(
    mut usage: UsageSnapshot,
    subscription: &KimiSubscriptionStatsResponse,
) -> UsageSnapshot {
    if let Some(balance) = subscription.subscription_balance.as_ref()
        && let Some(ratio) =
            value_as_f64(balance.amount_used_ratio.as_ref()).filter(|value| value.is_finite())
    {
        // Verified monthly sentinel (#2431 / #2566).
        usage = usage.with_extra_rate_window(
            "kimi-monthly",
            "Monthly",
            RateWindow::with_details(
                ratio * 100.0,
                Some(30 * 24 * 60),
                balance.expire_time.as_ref().and_then(parse_kimi_timestamp),
                None,
            ),
        );
    }

    if let Some(limit) = subscription.ratelimit_code7d.as_ref()
        && limit.enabled.unwrap_or(true)
        && let Some(ratio) = value_as_f64(limit.ratio.as_ref()).filter(|value| value.is_finite())
    {
        usage = usage.with_extra_rate_window(
            "kimi-code-7d",
            "Code 7-day",
            RateWindow::with_details(
                ratio * 100.0,
                Some(10080),
                limit.reset_time.as_ref().and_then(parse_kimi_timestamp),
                None,
            ),
        );
    }

    usage
}

async fn kimi_web_post(
    client: &Client,
    url: &str,
    token: &str,
    body: serde_json::Value,
) -> Result<reqwest::Response, ProviderError> {
    client
        .post(url)
        .bearer_auth(token)
        .header("Cookie", format!("kimi-auth={token}"))
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
        )
        .json(&body)
        .send()
        .await
        .map_err(ProviderError::from)
}

fn value_as_f64(value: Option<&serde_json::Value>) -> Option<f64> {
    match value? {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(text) => text.trim().replace(',', "").parse().ok(),
        _ => None,
    }
}

fn parse_kimi_timestamp(value: &serde_json::Value) -> Option<DateTime<Utc>> {
    match value {
        serde_json::Value::String(text) => parse_kimi_timestamp_str(text),
        serde_json::Value::Number(number) => number.as_i64().and_then(timestamp_from_number),
        _ => None,
    }
}

fn parse_kimi_timestamp_str(text: &str) -> Option<DateTime<Utc>> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(text) {
        return Some(dt.with_timezone(&Utc));
    }
    text.parse::<i64>().ok().and_then(timestamp_from_number)
}

fn timestamp_from_number(raw: i64) -> Option<DateTime<Utc>> {
    let seconds = if raw > 10_000_000_000 {
        raw / 1000
    } else {
        raw
    };
    DateTime::from_timestamp(seconds, 0)
}

fn cleaned_env(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(cleaned_owned)
}

fn cleaned_owned(raw: impl AsRef<str>) -> Option<String> {
    let mut value = raw.as_ref().trim().to_string();
    if value.is_empty() {
        return None;
    }
    let quoted = (value.starts_with('"') && value.ends_with('"'))
        || (value.starts_with('\'') && value.ends_with('\''));
    if quoted {
        value = value[1..value.len().saturating_sub(1)].trim().to_string();
    }
    if value.is_empty() { None } else { Some(value) }
}

fn unix_now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn ascii_header_value(raw: &str) -> String {
    let ascii: String = raw
        .chars()
        .filter(|c| matches!(c, ' '..='~'))
        .collect::<String>()
        .trim()
        .to_string();
    if ascii.is_empty() {
        "unknown".to_string()
    } else {
        ascii
    }
}

fn format_usage_amount(value: f64) -> String {
    if (value.fract()).abs() < f64::EPSILON {
        format!("{}", value as i64)
    } else {
        format!("{value:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn auth_token_search_skips_unrelated_cookie_headers() {
        let token = KimiProvider::auth_token_from_cookie_headers([
            "locale=en-US; device_id=abc",
            "kimi-auth=valid-token",
        ])
        .unwrap();

        assert_eq!(token, "valid-token");
    }

    #[test]
    fn auth_token_from_empty_and_malformed_headers_fails() {
        for header in ["", "   ", "kimi-auth=", "kimi-auth=   ", "locale=en-US"] {
            assert!(
                KimiProvider::auth_token_from_cookie_header(header).is_err(),
                "{header:?} must not yield a token"
            );
        }
    }

    #[test]
    fn parses_code_api_usage_with_string_numbers() {
        let response: KimiCodeApiUsageResponse = serde_json::from_value(json!({
            "usage": {
                "limit": "1000",
                "used": "250",
                "remaining": "750",
                "reset_time": "1767225600"
            },
            "limits": [{
                "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                "detail": {
                    "limit": "100",
                    "remaining": "80",
                    "resetAt": "2026-01-01T00:00:00Z"
                }
            }]
        }))
        .unwrap();

        let snapshot = code_api::snapshot_from_code_api_response(response).unwrap();
        assert_eq!(snapshot.login_method.as_deref(), Some("Code API"));
        assert!((snapshot.primary.used_percent - 25.0).abs() < f64::EPSILON);
        let secondary = snapshot.secondary.unwrap();
        assert_eq!(secondary.window_minutes, Some(300));
        assert!((secondary.used_percent - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parses_code_api_usage_with_null_limits() {
        let response: KimiCodeApiUsageResponse = serde_json::from_value(json!({
            "usage": {
                "limit": "1000",
                "used": "125"
            },
            "limits": null
        }))
        .unwrap();

        let snapshot = code_api::snapshot_from_code_api_response(response).unwrap();
        assert!((snapshot.primary.used_percent - 12.5).abs() < f64::EPSILON);
        assert!(snapshot.secondary.is_none());
    }

    #[test]
    fn parses_web_usage_with_subscription_windows() {
        let usage: KimiWebUsageResponse = serde_json::from_value(json!({
            "usages": [{
                "scope": "FEATURE_CODING",
                "detail": { "limit": "2048", "used": "375", "resetTime": "2026-01-09T15:23:13Z" },
                "limits": [{
                    "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                    "detail": { "limit": "100", "used": "25" }
                }]
            }]
        }))
        .unwrap();
        let subscription: KimiSubscriptionStatsResponse = serde_json::from_value(json!({
            "subscriptionBalance": {
                "amountUsedRatio": 0.7716,
                "expireTime": "2026-07-23T00:00:00Z"
            },
            "ratelimitCode7d": {
                "ratio": 0.0946,
                "enabled": true,
                "resetTime": "2026-07-13T15:28:00Z"
            }
        }))
        .unwrap();

        let snapshot = web::snapshot_from_web_usage_response(usage, Some(subscription)).unwrap();

        assert!((snapshot.primary.used_percent - 18.310546875).abs() < f64::EPSILON);
        assert_eq!(
            snapshot.secondary.as_ref().unwrap().window_minutes,
            Some(300)
        );
        let monthly = snapshot
            .extra_rate_windows
            .iter()
            .find(|window| window.id == "kimi-monthly")
            .unwrap();
        assert_eq!(monthly.title, "Monthly");
        assert_eq!(monthly.window.window_minutes, Some(30 * 24 * 60));
        assert!((monthly.window.used_percent - 77.16).abs() < 0.0001);
        let code_7d = snapshot
            .extra_rate_windows
            .iter()
            .find(|window| window.id == "kimi-code-7d")
            .unwrap();
        assert_eq!(code_7d.title, "Code 7-day");
        assert_eq!(code_7d.window.window_minutes, Some(10080));
        assert!((code_7d.window.used_percent - 9.46).abs() < 0.0001);
    }

    #[test]
    fn cleaned_env_strips_quotes() {
        assert_eq!(cleaned_owned("  \"token\"  ").as_deref(), Some("token"));
        assert_eq!(cleaned_owned("'token'").as_deref(), Some("token"));
        assert!(cleaned_owned("   ").is_none());
    }
}
