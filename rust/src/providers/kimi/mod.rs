//! Kimi AI provider implementation
//!
//! Fetches usage data from Kimi (Moonshot AI)
//! Uses JWT from kimi-auth cookie for authentication
//! Tracks weekly quota + 5-hour rate limit

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::{Client, Url};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::browser::cookies::get_cookie_header;
use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};

const KIMI_WEB_USAGE_URL: &str =
    "https://www.kimi.com/apiv2/kimi.gateway.billing.v1.BillingService/GetUsages";
const KIMI_SUBSCRIPTION_STATS_URL: &str =
    "https://www.kimi.com/apiv2/kimi.gateway.membership.v2.MembershipService/GetSubscriptionStats";
const KIMI_COOKIE_DOMAINS: [&str; 2] = ["www.kimi.com", "kimi.moonshot.cn"];
const KIMI_CODE_API_BASE: &str = "https://api.kimi.com";
const KIMI_CODE_API_KEY_ENV: &str = "KIMI_CODE_API_KEY";
const KIMI_CODE_BASE_URL_ENV: &str = "KIMI_CODE_BASE_URL";
const KIMI_CODE_HOME_ENV: &str = "KIMI_CODE_HOME";
const KIMI_CODE_OAUTH_HOST_ENV: &str = "KIMI_CODE_OAUTH_HOST";
const KIMI_OAUTH_HOST_ENV: &str = "KIMI_OAUTH_HOST";
const KIMI_CODE_CLI_PLATFORM: &str = "kimi_code_cli";
/// CLI access tokens must remain valid for at least this long to be reused.
const KIMI_CODE_CREDENTIAL_MIN_TTL_SECS: f64 = 60.0;

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

    /// Extract JWT token from kimi-auth cookie
    fn get_auth_token(&self) -> Result<String, ProviderError> {
        let mut saw_cookie_header = false;
        let mut last_error = None;
        for domain in KIMI_COOKIE_DOMAINS {
            match get_cookie_header(domain) {
                Ok(header) if !header.is_empty() => {
                    saw_cookie_header = true;
                    if let Ok(token) = Self::auth_token_from_cookie_header(&header) {
                        return Ok(token);
                    }
                }
                Ok(_) => {}
                Err(e) => last_error = Some(e),
            }
        }

        if !saw_cookie_header && let Some(e) = last_error {
            return Err(ProviderError::Other(format!(
                "Failed to get cookies: {}",
                e
            )));
        }

        Err(ProviderError::AuthRequired)
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

    /// Fetch usage via Kimi web API
    async fn fetch_via_web(
        &self,
        cookie_header: Option<&str>,
    ) -> Result<UsageSnapshot, ProviderError> {
        let token = match cookie_header {
            Some(header) if !header.trim().is_empty() => {
                Self::auth_token_from_cookie_header(header)
            }
            _ => self.get_auth_token(),
        }?;

        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        let resp = kimi_web_post(
            &client,
            KIMI_WEB_USAGE_URL,
            &token,
            serde_json::json!({ "scope": ["FEATURE_CODING"] }),
        )
        .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(ProviderError::AuthRequired);
            }
            return Err(ProviderError::Other(format!("API error: {}", status)));
        }

        let usage: KimiWebUsageResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;

        let subscription = match kimi_web_post(
            &client,
            KIMI_SUBSCRIPTION_STATS_URL,
            &token,
            serde_json::json!({}),
        )
        .await
        {
            Ok(response) if response.status().is_success() => response.json().await.ok(),
            _ => None,
        };

        Self::snapshot_from_web_usage_response(usage, subscription)
    }

    async fn fetch_via_code_api(
        &self,
        api_key: Option<&str>,
        identity_headers: Option<&[(&str, String)]>,
        login_method: &str,
    ) -> Result<UsageSnapshot, ProviderError> {
        let api_key = Self::code_api_key(api_key)?;
        let base_url = Self::code_api_base_url()?;
        let endpoint = Self::code_api_usage_endpoint(&base_url)?;
        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        let mut request = client
            .get(endpoint)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Accept", "application/json");
        if let Some(headers) = identity_headers {
            for (name, value) in headers {
                request = request.header(*name, value);
            }
        }

        let resp = request.send().await?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED
            || resp.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(ProviderError::AuthRequired);
        }
        if !resp.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Kimi Code API returned status {}",
                resp.status()
            )));
        }

        let json: KimiCodeApiUsageResponse = resp.json().await.map_err(|e| {
            ProviderError::Parse(format!("Failed to parse Kimi Code API response: {e}"))
        })?;
        let mut snapshot = Self::snapshot_from_code_api_response(json)?;
        snapshot.login_method = Some(login_method.to_string());
        Ok(snapshot)
    }

    fn code_api_key(explicit: Option<&str>) -> Result<String, ProviderError> {
        if let Some(key) = explicit.map(str::trim).filter(|key| !key.is_empty()) {
            return Ok(key.to_string());
        }
        cleaned_env(KIMI_CODE_API_KEY_ENV).ok_or(ProviderError::AuthRequired)
    }

    fn code_api_base_url() -> Result<Url, ProviderError> {
        let raw =
            cleaned_env(KIMI_CODE_BASE_URL_ENV).unwrap_or_else(|| KIMI_CODE_API_BASE.to_string());
        crate::providers::validated_https_url(&raw, "Kimi Code API base")
    }

    /// Whether env base/OAuth overrides mean we must not reuse CLI-owned credentials.
    fn has_code_endpoint_override() -> bool {
        cleaned_env(KIMI_CODE_BASE_URL_ENV).is_some()
            || cleaned_env(KIMI_CODE_OAUTH_HOST_ENV).is_some()
            || cleaned_env(KIMI_OAUTH_HOST_ENV).is_some()
    }

    /// Home for Kimi Code CLI state (`%USERPROFILE%\.kimi-code` or `KIMI_CODE_HOME`).
    fn kimi_code_home() -> Option<PathBuf> {
        if let Some(override_home) = cleaned_env(KIMI_CODE_HOME_ENV) {
            return Some(PathBuf::from(override_home));
        }
        dirs::home_dir().map(|home| home.join(".kimi-code"))
    }

    /// Read-only access to a still-fresh Kimi Code CLI access token.
    ///
    /// Never refreshes or rewrites CLI-owned `credentials/kimi-code.json`.
    /// Skips when `KIMI_CODE_BASE_URL` / OAuth host overrides are set.
    fn kimi_code_cli_access_token(now_unix: f64) -> Option<String> {
        if Self::has_code_endpoint_override() {
            return None;
        }
        let home = Self::kimi_code_home()?;
        let credential = read_kimi_code_credential(&home)?;
        let token = cleaned_owned(credential.access_token)?;
        if !is_kimi_code_credential_fresh(credential.expires_at, now_unix) {
            return None;
        }
        Some(token)
    }

    fn kimi_code_cli_identity_headers(home: &Path) -> Vec<(&'static str, String)> {
        // Only send device id when the CLI file exists — never mint a fresh UUID
        // per fetch (unstable fingerprinting toward Moonshot).
        let device_id = read_kimi_code_device_id(home);
        let version = env!("CARGO_PKG_VERSION").to_string();
        let os_name = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        let model = format!("{os_name} {arch}");
        let mut headers = vec![
            ("User-Agent", format!("CodexBar/{version}")),
            ("X-Msh-Platform", KIMI_CODE_CLI_PLATFORM.to_string()),
            ("X-Msh-Version", version),
            ("X-Msh-Device-Name", "codexbar".to_string()),
            ("X-Msh-Device-Model", ascii_header_value(&model)),
            ("X-Msh-Os-Version", ascii_header_value(os_name)),
        ];
        if let Some(device_id) = device_id {
            headers.push(("X-Msh-Device-Id", device_id));
        }
        headers
    }

    fn code_api_usage_endpoint(base_url: &Url) -> Result<Url, ProviderError> {
        let base = base_url.as_str().trim_end_matches('/');
        let path = base_url.path().trim_matches('/');
        let endpoint = if path == "coding/v1" || path.ends_with("/coding/v1") {
            format!("{base}/usages")
        } else if path == "coding" || path.ends_with("/coding") {
            format!("{base}/v1/usages")
        } else {
            format!("{base}/coding/v1/usages")
        };
        Url::parse(&endpoint)
            .map_err(|_| ProviderError::Other("Kimi Code API usage endpoint is invalid".into()))
    }

    fn snapshot_from_code_api_response(
        response: KimiCodeApiUsageResponse,
    ) -> Result<UsageSnapshot, ProviderError> {
        let primary = Self::rate_window_from_usage_detail(&response.usage, None)?;
        let mut usage = UsageSnapshot::new(primary).with_login_method("Code API");

        if let Some(limit) = response.limits.unwrap_or_default().into_iter().next() {
            let window_minutes = limit.window.as_ref().and_then(kimi_window_minutes);
            let rate_limit = Self::rate_window_from_usage_detail(&limit.detail, window_minutes)?;
            usage = usage.with_secondary(rate_limit);
        }

        Ok(usage)
    }

    fn snapshot_from_web_usage_response(
        response: KimiWebUsageResponse,
        subscription: Option<KimiSubscriptionStatsResponse>,
    ) -> Result<UsageSnapshot, ProviderError> {
        let coding = response
            .usages
            .into_iter()
            .find(|usage| usage.scope == "FEATURE_CODING")
            .ok_or_else(|| ProviderError::Parse("Kimi FEATURE_CODING usage missing".into()))?;
        let primary = Self::rate_window_from_usage_detail(&coding.detail, Some(10080))?;
        let mut usage = UsageSnapshot::new(primary).with_login_method("Kimi");

        if let Some(limit) = coding.limits.unwrap_or_default().into_iter().next() {
            let window_minutes = limit.window.as_ref().and_then(kimi_window_minutes);
            let rate_limit = Self::rate_window_from_usage_detail(&limit.detail, window_minutes)?;
            usage = usage.with_secondary(rate_limit);
        }

        if let Some(subscription) = subscription {
            if let Some(balance) = subscription.subscription_balance
                && let Some(ratio) =
                    value_as_f64(balance.amount_used_ratio.as_ref()).filter(|v| v.is_finite())
            {
                usage = usage.with_extra_rate_window(
                    "kimi-monthly",
                    "Monthly",
                    RateWindow::with_details(
                        ratio * 100.0,
                        // Verified monthly sentinel (#2431 / #2566).
                        Some(30 * 24 * 60),
                        balance.expire_time.as_ref().and_then(parse_kimi_timestamp),
                        None,
                    ),
                );
            }

            if let Some(limit) = subscription.ratelimit_code7d
                && limit.enabled.unwrap_or(true)
                && let Some(ratio) = value_as_f64(limit.ratio.as_ref()).filter(|v| v.is_finite())
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
        }

        Ok(usage)
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

    /// Parse Kimi usage response
    fn parse_usage_response(
        &self,
        json: &serde_json::Value,
    ) -> Result<UsageSnapshot, ProviderError> {
        // Extract quota information
        // Kimi typically has: daily/weekly limits and 5-hour rate limits

        let quota = json.get("quota").or_else(|| json.get("usage"));

        // 5-hour rate limit (session-like)
        let five_hour_used = quota
            .and_then(|q| q.get("rate_limit_used").or_else(|| q.get("five_hour_used")))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let five_hour_limit = quota
            .and_then(|q| {
                q.get("rate_limit_total")
                    .or_else(|| q.get("five_hour_limit"))
            })
            .and_then(|v| v.as_f64())
            .unwrap_or(100.0);

        let five_hour_percent = if five_hour_limit > 0.0 {
            (five_hour_used / five_hour_limit) * 100.0
        } else {
            0.0
        };

        // Weekly quota
        let weekly_used = quota
            .and_then(|q| q.get("weekly_used").or_else(|| q.get("week_used")))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let weekly_limit = quota
            .and_then(|q| q.get("weekly_limit").or_else(|| q.get("week_limit")))
            .and_then(|v| v.as_f64())
            .unwrap_or(1000.0);

        let weekly_percent = if weekly_limit > 0.0 {
            (weekly_used / weekly_limit) * 100.0
        } else {
            0.0
        };

        // Get user info
        let nickname = json
            .get("nickname")
            .or_else(|| json.get("name"))
            .and_then(|v| v.as_str());

        let plan = json
            .get("vip_type")
            .or_else(|| json.get("plan"))
            .and_then(|v| v.as_str())
            .unwrap_or("Kimi");

        // Create primary rate window (weekly quota - more important for planning)
        let primary = RateWindow::new(weekly_percent);

        // Create secondary rate window (5-hour rate limit)
        let mut rate_limit = RateWindow::new(five_hour_percent);

        // Try to parse resetTime / reset_time from the response; fall back to 5h from now.
        let resets_at = quota
            .and_then(|q| q.get("resetTime").or_else(|| q.get("reset_time")))
            .and_then(|v| {
                if let Some(s) = v.as_str() {
                    chrono::DateTime::parse_from_rfc3339(s)
                        .map(|dt| dt.with_timezone(&chrono::Utc))
                        .ok()
                } else {
                    v.as_i64().map(|ts| {
                        chrono::DateTime::from_timestamp(ts, 0)
                            .unwrap_or_else(|| chrono::Utc::now() + chrono::Duration::hours(5))
                    })
                }
            })
            .unwrap_or_else(|| chrono::Utc::now() + chrono::Duration::hours(5));

        rate_limit.resets_at = Some(resets_at);

        // Try to parse windowMinutes / window_minutes; fall back to 300 (5 hours).
        let window_minutes = quota
            .and_then(|q| q.get("windowMinutes").or_else(|| q.get("window_minutes")))
            .and_then(|v| v.as_i64())
            .unwrap_or(300);

        rate_limit.window_minutes = Some(window_minutes as u32);

        let mut usage = UsageSnapshot::new(primary).with_login_method(plan);

        // Only add rate limit as secondary if we actually have rate limit data
        if five_hour_limit > 0.0 {
            usage = usage.with_secondary(rate_limit);
        }

        if let Some(name) = nickname {
            usage = usage.with_email(name.to_string());
        }

        Ok(usage)
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
                if Self::code_api_key(ctx.api_key.as_deref()).is_ok() {
                    match self
                        .fetch_via_code_api(ctx.api_key.as_deref(), None, "Code API")
                        .await
                    {
                        Ok(usage) => return Ok(ProviderFetchResult::new(usage, "code-api")),
                        Err(err) => {
                            tracing::debug!(
                                error = %err,
                                "Kimi Code API key fetch failed; trying CLI credential / web"
                            );
                        }
                    }
                }

                if let Some(cli_token) = Self::kimi_code_cli_access_token(unix_now_secs()) {
                    let home = Self::kimi_code_home().unwrap_or_default();
                    let headers = Self::kimi_code_cli_identity_headers(&home);
                    match self
                        .fetch_via_code_api(Some(&cli_token), Some(&headers), "Kimi Code CLI")
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

                let usage = self
                    .fetch_via_web(ctx.manual_cookie_header.as_deref())
                    .await?;
                Ok(ProviderFetchResult::new(usage, "web"))
            }
            SourceMode::OAuth => {
                let usage = self
                    .fetch_via_code_api(ctx.api_key.as_deref(), None, "Code API")
                    .await?;
                Ok(ProviderFetchResult::new(usage, "code-api"))
            }
            SourceMode::Web => {
                let usage = self
                    .fetch_via_web(ctx.manual_cookie_header.as_deref())
                    .await?;
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

#[derive(Debug, Deserialize)]
struct KimiCodeCredentialFile {
    #[serde(default, alias = "accessToken")]
    access_token: String,
    #[serde(default)]
    #[allow(dead_code)]
    refresh_token: Option<String>,
    #[serde(default, alias = "expiresAt")]
    expires_at: Option<serde_json::Value>,
}

fn read_kimi_code_credential(home: &Path) -> Option<KimiCodeCredentialFile> {
    let path = home.join("credentials").join("kimi-code.json");
    let data = std::fs::read(path).ok()?;
    serde_json::from_slice(&data).ok()
}

fn read_kimi_code_device_id(home: &Path) -> Option<String> {
    let path = home.join("device_id");
    let raw = std::fs::read_to_string(path).ok()?;
    cleaned_owned(raw)
}

fn is_kimi_code_credential_fresh(expires_at: Option<serde_json::Value>, now_unix: f64) -> bool {
    let Some(expires) = value_as_f64(expires_at.as_ref()) else {
        return false;
    };
    if !expires.is_finite() {
        return false;
    }
    // Support both seconds and millisecond epoch values.
    let expires_secs = if expires > 10_000_000_000.0 {
        expires / 1000.0
    } else {
        expires
    };
    expires_secs > now_unix + KIMI_CODE_CREDENTIAL_MIN_TTL_SECS
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
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn write_temp_kimi_code_home(
        access_token: &str,
        expires_at: Option<serde_json::Value>,
    ) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let credentials = dir.path().join("credentials");
        std::fs::create_dir_all(&credentials).expect("mkdir credentials");
        let mut payload = serde_json::Map::new();
        payload.insert("access_token".into(), json!(access_token));
        payload.insert("refresh_token".into(), json!("refresh"));
        if let Some(expires) = expires_at {
            payload.insert("expires_at".into(), expires);
        }
        std::fs::write(
            credentials.join("kimi-code.json"),
            serde_json::to_vec_pretty(&serde_json::Value::Object(payload)).unwrap(),
        )
        .expect("write credentials");
        dir
    }

    #[test]
    fn code_api_usage_endpoint_normalizes_base_paths() {
        let root = Url::parse("https://api.kimi.com").unwrap();
        assert_eq!(
            KimiProvider::code_api_usage_endpoint(&root)
                .unwrap()
                .as_str(),
            "https://api.kimi.com/coding/v1/usages"
        );
        let coding = Url::parse("https://proxy.example/kimi/coding").unwrap();
        assert_eq!(
            KimiProvider::code_api_usage_endpoint(&coding)
                .unwrap()
                .as_str(),
            "https://proxy.example/kimi/coding/v1/usages"
        );
        let versioned = Url::parse("https://proxy.example/kimi/coding/v1").unwrap();
        assert_eq!(
            KimiProvider::code_api_usage_endpoint(&versioned)
                .unwrap()
                .as_str(),
            "https://proxy.example/kimi/coding/v1/usages"
        );
    }

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

        let snapshot = KimiProvider::snapshot_from_code_api_response(response).unwrap();
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

        let snapshot = KimiProvider::snapshot_from_code_api_response(response).unwrap();
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

        let snapshot =
            KimiProvider::snapshot_from_web_usage_response(usage, Some(subscription)).unwrap();

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
    fn reuses_fresh_cli_credential_without_rewriting_file() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let now = 1_800_000_000.0_f64;
        let home = write_temp_kimi_code_home("oauth-token", Some(json!(now + 3600.0)));
        let cred_path = home.path().join("credentials").join("kimi-code.json");
        let original = std::fs::read(&cred_path).unwrap();
        let original_modified = std::fs::metadata(&cred_path).unwrap().modified().unwrap();

        // SAFETY: guarded by env_lock for process-wide env mutation in tests.
        unsafe {
            std::env::remove_var(KIMI_CODE_BASE_URL_ENV);
            std::env::remove_var(KIMI_CODE_OAUTH_HOST_ENV);
            std::env::remove_var(KIMI_OAUTH_HOST_ENV);
            std::env::set_var(KIMI_CODE_HOME_ENV, home.path());
        }

        let token = KimiProvider::kimi_code_cli_access_token(now);
        assert_eq!(token.as_deref(), Some("oauth-token"));

        let after = std::fs::read(&cred_path).unwrap();
        let after_modified = std::fs::metadata(&cred_path).unwrap().modified().unwrap();
        assert_eq!(after, original);
        assert_eq!(after_modified, original_modified);

        let headers = KimiProvider::kimi_code_cli_identity_headers(home.path());
        assert!(
            headers
                .iter()
                .any(|(k, v)| *k == "X-Msh-Platform" && v == KIMI_CODE_CLI_PLATFORM)
        );

        unsafe {
            std::env::remove_var(KIMI_CODE_HOME_ENV);
        }
    }

    #[test]
    fn rejects_expired_or_missing_expiry_cli_credentials() {
        let now = 1_800_000_000.0_f64;
        for expires in [Some(json!(now + 30.0)), None, Some(json!("not-a-time"))] {
            let home = write_temp_kimi_code_home("oauth", expires);
            let cred = read_kimi_code_credential(home.path()).expect("credential present");
            assert!(!is_kimi_code_credential_fresh(cred.expires_at, now));
        }

        let home = write_temp_kimi_code_home("oauth", Some(json!(now + 120.0)));
        let cred = read_kimi_code_credential(home.path()).unwrap();
        assert!(is_kimi_code_credential_fresh(cred.expires_at, now));
    }

    #[test]
    fn skips_cli_credential_when_endpoint_overrides_present() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let now = 1_800_000_000.0_f64;
        let home = write_temp_kimi_code_home("oauth-token", Some(json!(now + 3600.0)));

        unsafe {
            std::env::set_var(KIMI_CODE_HOME_ENV, home.path());
            std::env::set_var(KIMI_CODE_BASE_URL_ENV, "https://proxy.example.com/kimi");
        }
        assert!(KimiProvider::has_code_endpoint_override());
        assert!(KimiProvider::kimi_code_cli_access_token(now).is_none());

        unsafe {
            std::env::remove_var(KIMI_CODE_BASE_URL_ENV);
            std::env::set_var(KIMI_CODE_OAUTH_HOST_ENV, "https://oauth.example.com");
        }
        assert!(KimiProvider::kimi_code_cli_access_token(now).is_none());

        unsafe {
            std::env::remove_var(KIMI_CODE_OAUTH_HOST_ENV);
            std::env::remove_var(KIMI_CODE_HOME_ENV);
        }
    }

    #[test]
    fn credential_freshness_accepts_millisecond_expiry() {
        let now = 1_800_000_000.0_f64;
        assert!(is_kimi_code_credential_fresh(
            Some(json!((now + 3600.0) * 1000.0)),
            now
        ));
    }

    #[test]
    fn cleaned_env_strips_quotes() {
        assert_eq!(cleaned_owned("  \"token\"  ").as_deref(), Some("token"));
        assert_eq!(cleaned_owned("'token'").as_deref(), Some("token"));
        assert!(cleaned_owned("   ").is_none());
    }

    #[test]
    fn credential_freshness_requires_sixty_second_margin() {
        assert!((KIMI_CODE_CREDENTIAL_MIN_TTL_SECS - 60.0).abs() < f64::EPSILON);
    }
}
