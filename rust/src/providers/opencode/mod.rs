//! OpenCode provider implementation
//!
//! Fetches usage data from OpenCode (opencode.ai)
//! Uses browser cookies for authentication

pub mod billing;
pub mod scraper;

// Re-exports for advanced scraping
#[allow(unused_imports)]
pub use scraper::{OpenCodeError, OpenCodeUsageFetcher, OpenCodeUsageSnapshot};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde_json::Value;
use uuid::Uuid;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};

pub(super) const BASE_URL: &str = "https://opencode.ai";
pub(super) const SERVER_URL: &str = "https://opencode.ai/_server";
const MAX_RESET_SECONDS: i64 = 366 * 24 * 60 * 60;
const WORKSPACES_SERVER_ID: &str =
    "def39973159c7f0483d8793a822b8dbb10d067e12c65455fcb4608459ba0234f";
const SUBSCRIPTION_SERVER_ID: &str =
    "7abeebee372f304e050aaaf92be863f4a86490e382f8c79db68fd94040d691b4";
/// OpenCode provider
pub struct OpenCodeProvider {
    metadata: ProviderMetadata,
    pub(super) client: Client,
}

impl OpenCodeProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::OpenCode,
                display_name: "OpenCode",
                session_label: "5-hour",
                weekly_label: "Weekly",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://opencode.ai"),
                status_page_url: None,
            },
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    /// Fetch usage with cookie header
    async fn fetch_with_cookies(
        &self,
        cookie_header: &str,
    ) -> Result<UsageSnapshot, ProviderError> {
        // First get workspace ID
        let workspace_id = self.fetch_workspace_id(cookie_header).await?;

        // Then fetch subscription info. Pay-as-you-go workspaces have no
        // subscription object, so the subscription server answers null or
        // fails; their spend lives in the billing payload (upstream 0.49.5
        // #2504/#2697).
        let subscription = match self.fetch_subscription(&workspace_id, cookie_header).await {
            Ok(text) => text,
            Err(err) if Self::can_fall_back_to_billing(&err) => {
                match self
                    .fetch_pay_as_you_go_usage(&workspace_id, cookie_header)
                    .await
                {
                    Err(auth_err) => return Err(auth_err),
                    Ok(Some(snapshot)) => return Ok(snapshot),
                    Ok(None) => return Err(err),
                }
            }
            Err(err) => return Err(err),
        };

        // Parse the response
        self.parse_subscription(&subscription)
    }

    /// Fetch workspace ID from server
    async fn fetch_workspace_id(&self, cookie_header: &str) -> Result<String, ProviderError> {
        let url = format!("{}?id={}", SERVER_URL, WORKSPACES_SERVER_ID);

        let response = self
            .client
            .get(&url)
            .header("Cookie", cookie_header)
            .header("X-Server-Id", WORKSPACES_SERVER_ID)
            .header("X-Server-Instance", format!("server-fn:{}", Uuid::new_v4()))
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            )
            .header("Origin", BASE_URL)
            .header("Referer", BASE_URL)
            .header(
                "Accept",
                "text/javascript, application/json;q=0.9, */*;q=0.8",
            )
            .send()
            .await?;

        if !response.status().is_success() {
            if response.status().as_u16() == 401 || response.status().as_u16() == 403 {
                return Err(ProviderError::AuthRequired);
            }
            return Err(ProviderError::Other(format!(
                "OpenCode API returned {}",
                response.status()
            )));
        }

        let text = response.text().await?;

        // Check for sign-out indicators
        if self.looks_signed_out(&text) {
            return Err(ProviderError::AuthRequired);
        }

        // Parse workspace IDs
        let ids = self.parse_workspace_ids(&text);
        if ids.is_empty() {
            return Err(ProviderError::Parse("No workspace ID found".to_string()));
        }

        Ok(ids[0].clone())
    }

    /// Fetch subscription info for a workspace
    async fn fetch_subscription(
        &self,
        workspace_id: &str,
        cookie_header: &str,
    ) -> Result<String, ProviderError> {
        let referer = format!("https://opencode.ai/workspace/{}/billing", workspace_id);
        let args = serde_json::json!([workspace_id]);
        let encoded_args = Self::url_encode(&args.to_string());
        let url = format!(
            "{}?id={}&args={}",
            SERVER_URL, SUBSCRIPTION_SERVER_ID, encoded_args
        );

        let response = self
            .client
            .get(&url)
            .header("Cookie", cookie_header)
            .header("X-Server-Id", SUBSCRIPTION_SERVER_ID)
            .header("X-Server-Instance", format!("server-fn:{}", Uuid::new_v4()))
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            )
            .header("Origin", BASE_URL)
            .header("Referer", referer)
            .header(
                "Accept",
                "text/javascript, application/json;q=0.9, */*;q=0.8",
            )
            .send()
            .await?;

        if !response.status().is_success() {
            if response.status().as_u16() == 401 || response.status().as_u16() == 403 {
                return Err(ProviderError::AuthRequired);
            }
            return Err(ProviderError::Other(format!(
                "OpenCode subscription API returned {}",
                response.status()
            )));
        }

        let text = response.text().await?;

        if self.looks_signed_out(&text) {
            return Err(ProviderError::AuthRequired);
        }

        Ok(text)
    }

    /// Parse subscription response into UsageSnapshot
    fn parse_subscription(&self, text: &str) -> Result<UsageSnapshot, ProviderError> {
        let now = Utc::now();

        // Try to parse as JSON
        if let Ok(json) = serde_json::from_str::<Value>(text)
            && let Some(snapshot) = self.parse_usage_json(&json, now)
        {
            return Ok(snapshot);
        }

        // Fall back to regex-based parsing. Require at least one window —
        // plans may omit rolling or weekly without being unparseable.
        let rolling = self
            .extract_usage_regex(text, "rollingUsage")
            .or_else(|_| self.extract_usage_regex(text, "fiveHourUsage"))
            .or_else(|_| self.extract_usage_regex(text, "sessionUsage"))
            .ok();
        let weekly = self
            .extract_usage_regex(text, "weeklyUsage")
            .or_else(|_| self.extract_usage_regex(text, "weekUsage"))
            .or_else(|_| self.extract_usage_regex(text, "sevenDayUsage"))
            .ok();
        self.snapshot_from_windows(rolling, weekly, now, super::extract_renewal(text))
            .ok_or_else(|| {
                let has_usage_hint = text.contains("usagePercent")
                    || text.contains("usedPercent")
                    || text.contains("rolling")
                    || text.contains("weekly");
                tracing::debug!(
                    body_len = text.len(),
                    has_usage_hint,
                    "OpenCode subscription body missing rolling/weekly usage windows"
                );
                ProviderError::Parse("Missing usage percent (rolling or weekly)".into())
            })
    }

    /// Build a snapshot from optional rolling/weekly windows.
    /// Rolling is preferred as primary; weekly-only becomes primary.
    fn snapshot_from_windows(
        &self,
        rolling: Option<(f64, i64)>,
        weekly: Option<(f64, i64)>,
        now: DateTime<Utc>,
        renews_at: Option<DateTime<Utc>>,
    ) -> Option<UsageSnapshot> {
        let primary = match (rolling, weekly) {
            (Some(r), _) => RateWindow::with_details(
                r.0,
                Some(300), // 5 hours rolling
                Some(now + chrono::Duration::seconds(r.1)),
                None,
            ),
            (None, Some(w)) => RateWindow::with_details(
                w.0,
                Some(10080), // weekly used as sole window
                Some(now + chrono::Duration::seconds(w.1)),
                None,
            ),
            (None, None) => return None,
        };

        let mut usage = UsageSnapshot::new(primary).with_login_method("OpenCode");
        if rolling.is_some()
            && let Some(w) = weekly
        {
            usage = usage.with_secondary(RateWindow::with_details(
                w.0,
                Some(10080),
                Some(now + chrono::Duration::seconds(w.1)),
                None,
            ));
        }
        if let Some(renews_at) = renews_at {
            usage = usage.with_extra_rate_window(
                "renewal",
                "Renews",
                RateWindow::with_details(0.0, None, Some(renews_at), None),
            );
        }
        Some(usage)
    }

    /// Parse usage from JSON response
    fn parse_usage_json(&self, json: &Value, now: DateTime<Utc>) -> Option<UsageSnapshot> {
        // Prefer nested billing/subscription/data roots when present.
        for root_key in ["data", "subscription", "billing", "result", "payload"] {
            if let Some(nested) = json.get(root_key)
                && let Some(snapshot) = self.parse_usage_json(nested, now)
            {
                return Some(snapshot);
            }
        }

        let renews_at = self.find_datetime(json, &["renewAt", "renew_at", "renewsAt"]);

        let rolling = self.find_usage_window(
            json,
            &[
                "rollingUsage",
                "rolling",
                "rolling_usage",
                "fiveHourUsage",
                "five_hour",
                "fiveHour",
                "sessionUsage",
                "session",
                "rateLimit5h",
                "rate_limit_5h",
            ],
        );
        let weekly = self.find_usage_window(
            json,
            &[
                "weeklyUsage",
                "weekly",
                "weekly_usage",
                "weekUsage",
                "sevenDayUsage",
                "seven_day",
                "rateLimitWeekly",
                "rate_limit_weekly",
            ],
        );

        self.snapshot_from_windows(rolling, weekly, now, renews_at)
    }

    /// Find usage window in JSON by keys
    fn find_usage_window(&self, json: &Value, keys: &[&str]) -> Option<(f64, i64)> {
        for key in keys {
            if let Some(obj) = json.get(key)
                && let Some(window) = self.parse_window(obj)
            {
                return Some(window);
            }
        }

        // Try nested search (depth-limited by recursion over objects only).
        if let Some(obj) = json.as_object() {
            for (child_key, value) in obj {
                // Skip huge unrelated trees that often appear next to usage.
                if matches!(
                    child_key.as_str(),
                    "history" | "invoices" | "members" | "logs" | "events"
                ) {
                    continue;
                }
                if let Some(window) = self.find_usage_window(value, keys) {
                    return Some(window);
                }
            }
        }

        None
    }

    /// Parse a usage window object
    fn parse_window(&self, obj: &Value) -> Option<(f64, i64)> {
        // Bare number: treat as percent used (0–1 fractions scaled).
        if let Some(n) = obj.as_f64() {
            let percent = if n <= 1.0 { n * 100.0 } else { n };
            return Some((percent.clamp(0.0, 100.0), 0));
        }
        let percent = Self::window_percent(obj)?;
        let reset_sec = Self::window_reset_seconds(obj).unwrap_or(0);
        Some((percent.clamp(0.0, 100.0), reset_sec.max(0)))
    }

    fn window_percent(obj: &Value) -> Option<f64> {
        // Direct percent fields may arrive as 0..1 fractions or 0..100 percents.
        // Upstream #2331: only apply the *100 heuristic to *direct* fields — never to
        // a computed used/limit ratio (already 0..100; e.g. used=1, limit=100 → 1.0).
        const DIRECT_PERCENT_KEYS: &[&str] = &[
            "usagePercent",
            "usedPercent",
            "percentUsed",
            "percent",
            "pct",
            "usage_percent",
            "used_percent",
            "utilization",
            "utilizationPercent",
            "utilization_percent",
            "value",
        ];

        if let Some(val) = Self::first_f64(obj, DIRECT_PERCENT_KEYS) {
            let scaled = if (0.0..=1.0).contains(&val) {
                val * 100.0
            } else {
                val
            };
            return Some(scaled);
        }
        Self::percent_from_used_limit(obj)
    }

    fn percent_from_used_limit(obj: &Value) -> Option<f64> {
        let used = obj
            .get("used")
            .or(obj.get("usage"))
            .or(obj.get("consumed"))
            .or(obj.get("count"))
            .or(obj.get("usedTokens"))
            .and_then(|v| v.as_f64());
        let limit = obj
            .get("limit")
            .or(obj.get("total"))
            .or(obj.get("allowance"))
            .and_then(|v| v.as_f64());
        match (used, limit) {
            // Already 0..100 — do not run the direct-fraction *100 heuristic.
            (Some(used), Some(limit)) if limit > 0.0 => Some((used / limit) * 100.0),
            _ => None,
        }
    }

    fn window_reset_seconds(obj: &Value) -> Option<i64> {
        let reset_in_keys = [
            "resetInSec",
            "resetInSeconds",
            "resetSeconds",
            "reset_sec",
            "reset_in_sec",
            "resetsInSec",
            "resetsInSeconds",
            "resetIn",
            "resetSec",
        ];
        let reset_at_keys = [
            "resetAt",
            "resetsAt",
            "reset_at",
            "resets_at",
            "nextReset",
            "next_reset",
            "renewAt",
            "renew_at",
        ];

        Self::first_i64(obj, &reset_in_keys)
            .or_else(|| Self::reset_at_to_seconds(obj, &reset_at_keys))
    }

    fn reset_at_to_seconds(obj: &Value, keys: &[&str]) -> Option<i64> {
        let reset_at = Self::first_i64(obj, keys)?;
        let now = chrono::Utc::now().timestamp();
        reset_at
            .checked_sub(now)
            .filter(|seconds| (0..=MAX_RESET_SECONDS).contains(seconds))
    }

    fn find_datetime(&self, json: &Value, keys: &[&str]) -> Option<DateTime<Utc>> {
        for key in keys {
            if let Some(value) = json.get(key)
                && let Some(parsed) = Self::date_from_value(value)
            {
                return Some(parsed);
            }
        }

        if let Some(obj) = json.as_object() {
            for value in obj.values() {
                if let Some(parsed) = self.find_datetime(value, keys) {
                    return Some(parsed);
                }
            }
        }
        None
    }

    fn first_f64(obj: &Value, keys: &[&str]) -> Option<f64> {
        keys.iter().find_map(|key| obj.get(*key)?.as_f64())
    }

    fn first_i64(obj: &Value, keys: &[&str]) -> Option<i64> {
        keys.iter().find_map(|key| obj.get(*key)?.as_i64())
    }

    fn date_from_value(value: &Value) -> Option<DateTime<Utc>> {
        if let Some(number) = value.as_i64() {
            return Self::date_from_timestamp(number as f64);
        }
        if let Some(number) = value.as_f64() {
            return Self::date_from_timestamp(number);
        }
        let text = value.as_str()?.trim();
        if text.is_empty() {
            return None;
        }
        if let Ok(number) = text.parse::<f64>() {
            return Self::date_from_timestamp(number);
        }
        DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    }

    fn date_from_timestamp(number: f64) -> Option<DateTime<Utc>> {
        if !number.is_finite() || number <= 0.0 {
            return None;
        }
        let seconds = if number > 10_000_000_000.0 {
            number / 1000.0
        } else {
            number
        };
        DateTime::<Utc>::from_timestamp(seconds as i64, 0)
    }

    /// Extract usage via regex patterns (JS-object and JSON-ish payloads).
    fn extract_usage_regex(&self, text: &str, prefix: &str) -> Result<(f64, i64), ProviderError> {
        // Allow optional quotes around the window key and percent field names,
        // and accept the same percent aliases used by `window_percent`.
        let percent_pattern = format!(
            r#"{prefix}[^}}]{{0,500}}?(?:"usagePercent"|usagePercent|"usedPercent"|usedPercent|"percent"|percent)\s*[:=]\s*"?([0-9]+(?:\.[0-9]+)?)"?"#
        );
        let reset_pattern = format!(
            r#"{prefix}[^}}]{{0,500}}?(?:"resetInSec"|resetInSec|"resetInSeconds"|resetInSeconds)\s*[:=]\s*"?([0-9]+)"?"#
        );

        let percent = super::extract_number(&percent_pattern, text)
            .ok_or_else(|| ProviderError::Parse(format!("Missing {} percent", prefix)))?;

        let reset = super::extract_number(&reset_pattern, text)
            .map(|n| n as i64)
            .unwrap_or(0);

        Ok((percent, reset))
    }

    /// Parse workspace IDs from response
    fn parse_workspace_ids(&self, text: &str) -> Vec<String> {
        let pattern = r#"id\s*:\s*"(wrk_[^"]+)""#;
        let re = match regex_lite::Regex::new(pattern) {
            Ok(r) => r,
            Err(_) => return vec![],
        };

        re.captures_iter(text)
            .filter_map(|caps| caps.get(1).map(|m| m.as_str().to_string()))
            .collect()
    }

    /// Check if response indicates user is signed out
    pub(super) fn looks_signed_out(&self, text: &str) -> bool {
        let lower = text.to_lowercase();
        lower.contains("login") || lower.contains("sign in") || lower.contains("auth/authorize")
    }

    /// URL encode a string for query parameters
    pub(super) fn url_encode(s: &str) -> String {
        let mut result = String::with_capacity(s.len() * 3);
        for c in s.chars() {
            match c {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => {
                    result.push(c);
                }
                _ => {
                    for b in c.to_string().as_bytes() {
                        result.push_str(&format!("%{:02X}", b));
                    }
                }
            }
        }
        result
    }
}

impl Default for OpenCodeProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for OpenCodeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::OpenCode
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Fetching OpenCode usage");

        match ctx.source_mode {
            SourceMode::Auto | SourceMode::Web => {
                // Check for manual cookie header first
                if let Some(ref cookie_header) = ctx.manual_cookie_header {
                    let usage = self.fetch_with_cookies(cookie_header).await?;
                    return Ok(ProviderFetchResult::new(usage, "web"));
                }

                match crate::providers::browser_cookie_header(&["opencode.ai"]) {
                    Ok(cookie_header) => match self.fetch_with_cookies(&cookie_header).await {
                        Ok(usage) => return Ok(ProviderFetchResult::new(usage, "web")),
                        Err(ProviderError::AuthRequired) => {}
                        Err(e) => return Err(e),
                    },
                    Err(ProviderError::NoCookies) => {}
                    Err(e) => return Err(e),
                }

                Err(ProviderError::AuthRequired)
            }
            SourceMode::Cli => Err(ProviderError::UnsupportedSource(SourceMode::Cli)),
            SourceMode::OAuth => Err(ProviderError::UnsupportedSource(SourceMode::OAuth)),
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::Web]
    }

    fn supports_web(&self) -> bool {
        true
    }

    fn supports_cli(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_renewal_window() {
        let provider = OpenCodeProvider::new();
        let now = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let payload = serde_json::json!({
            "rollingUsage": { "usagePercent": 10, "resetInSec": 600 },
            "weeklyUsage": { "usagePercent": 50, "resetInSec": 3600 },
            "renewAt": "2026-06-01T12:00:00Z"
        });

        let snap = provider.parse_usage_json(&payload, now).expect("snapshot");
        let renewal = snap
            .extra_rate_windows
            .iter()
            .find(|window| window.id == "renewal")
            .expect("renewal window");
        assert_eq!(renewal.title, "Renews");
        assert_eq!(
            renewal.window.resets_at.unwrap().to_rfc3339(),
            "2026-06-01T12:00:00+00:00"
        );
    }

    #[test]
    fn ignores_out_of_range_reset_timestamps() {
        let payload = serde_json::json!({ "resetAt": i64::MAX });

        assert_eq!(OpenCodeProvider::window_reset_seconds(&payload), None);
    }

    #[test]
    fn parses_weekly_only_json_without_rolling() {
        let provider = OpenCodeProvider::new();
        let now = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let payload = serde_json::json!({
            "weeklyUsage": { "usagePercent": 76.0, "resetInSec": 86400 }
        });

        let snap = provider.parse_usage_json(&payload, now).expect("snapshot");
        assert!((snap.primary.used_percent - 76.0).abs() < f64::EPSILON);
        assert!(snap.secondary.is_none());
    }

    #[test]
    fn parses_rolling_only_json_without_weekly() {
        let provider = OpenCodeProvider::new();
        let now = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let payload = serde_json::json!({
            "rollingUsage": { "usagePercent": 12.5, "resetInSec": 600 }
        });

        let snap = provider.parse_usage_json(&payload, now).expect("snapshot");
        assert!((snap.primary.used_percent - 12.5).abs() < f64::EPSILON);
        assert!(snap.secondary.is_none());
    }

    #[test]
    fn regex_accepts_json_quoted_usage_percent() {
        let provider = OpenCodeProvider::new();
        let text = r#"{"rollingUsage":{"usagePercent":33.0,"resetInSec":120},"weeklyUsage":{"usagePercent":80}}"#;
        let snap = provider.parse_subscription(text).expect("snapshot");
        assert!((snap.primary.used_percent - 33.0).abs() < f64::EPSILON);
        assert!((snap.secondary.as_ref().unwrap().used_percent - 80.0).abs() < f64::EPSILON);
    }

    #[test]
    fn regex_weekly_only_does_not_error_on_missing_rolling() {
        let provider = OpenCodeProvider::new();
        // Not valid JSON for serde (trailing noise) so we exercise regex path.
        let text = r#"weeklyUsage: { usagePercent: 55, resetInSec: 99 } not-json"#;
        let snap = provider.parse_subscription(text).expect("snapshot");
        assert!((snap.primary.used_percent - 55.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parses_nested_data_subscription_shape() {
        let provider = OpenCodeProvider::new();
        let now = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let payload = serde_json::json!({
            "data": {
                "subscription": {
                    "rollingUsage": { "usedPercent": 18.5, "resetInSeconds": 1200 },
                    "weeklyUsage": { "percentUsed": 42, "resetsInSec": 86400 }
                }
            }
        });
        let snap = provider
            .parse_usage_json(&payload, now)
            .expect("nested snapshot");
        assert!((snap.primary.used_percent - 18.5).abs() < f64::EPSILON);
        assert!((snap.secondary.as_ref().unwrap().used_percent - 42.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parses_five_hour_and_week_aliases() {
        let provider = OpenCodeProvider::new();
        let now = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let payload = serde_json::json!({
            "fiveHourUsage": { "pct": 11, "resetInSec": 300 },
            "weekUsage": { "value": 0.25, "resetInSec": 7200 }
        });
        let snap = provider
            .parse_usage_json(&payload, now)
            .expect("alias snapshot");
        assert!((snap.primary.used_percent - 11.0).abs() < f64::EPSILON);
        // 0.25 fraction scales to 25%
        assert!((snap.secondary.as_ref().unwrap().used_percent - 25.0).abs() < f64::EPSILON);
    }

    #[test]
    fn sub_one_percent_computed_used_limit_is_not_rescaled_to_100() {
        // Upstream #2331: used=1, limit=100 → 1%, not 100% (false exhausted).
        let provider = OpenCodeProvider::new();
        let now = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let payload = serde_json::json!({
            "rollingUsage": { "used": 1, "limit": 100, "resetInSec": 600 },
            "weeklyUsage": { "used": 1, "limit": 200, "resetInSec": 86400 }
        });
        let snap = provider.parse_usage_json(&payload, now).expect("snapshot");
        assert!(
            (snap.primary.used_percent - 1.0).abs() < 0.001,
            "primary {}",
            snap.primary.used_percent
        );
        assert!(
            (snap.secondary.as_ref().unwrap().used_percent - 0.5).abs() < 0.001,
            "secondary {}",
            snap.secondary.as_ref().unwrap().used_percent
        );
    }

    #[test]
    fn direct_fractional_usage_percent_still_scales() {
        let provider = OpenCodeProvider::new();
        let now = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let payload = serde_json::json!({
            "rollingUsage": { "usagePercent": 0.25, "resetInSec": 600 }
        });
        let snap = provider.parse_usage_json(&payload, now).expect("snapshot");
        assert!((snap.primary.used_percent - 25.0).abs() < 0.001);
    }

    #[test]
    fn parse_subscription_accepts_realistic_server_fn_body() {
        let provider = OpenCodeProvider::new();
        // Shape closer to a live server-fn payload: nested under result/data.
        let body = r#"{
          "result": {
            "data": {
              "rollingUsage": { "usagePercent": 33.0, "resetInSec": 1800 },
              "weeklyUsage": { "usagePercent": 12.5, "resetInSec": 604800 },
              "renewAt": "2026-08-01T00:00:00Z"
            }
          }
        }"#;
        let snap = provider
            .parse_subscription(body)
            .expect("server-fn body should parse end-to-end");
        assert!((snap.primary.used_percent - 33.0).abs() < f64::EPSILON);
        assert!((snap.secondary.as_ref().unwrap().used_percent - 12.5).abs() < f64::EPSILON);
        assert_eq!(snap.login_method.as_deref(), Some("OpenCode"));
    }

    #[tokio::test]
    async fn web_mode_without_cookies_is_auth_not_unsupported() {
        let provider = OpenCodeProvider::new();
        let ctx = FetchContext {
            source_mode: SourceMode::Web,
            manual_cookie_header: None,
            ..FetchContext::default()
        };
        let err = provider
            .fetch_usage(&ctx)
            .await
            .expect_err("no cookies available");
        assert!(
            !matches!(err, ProviderError::UnsupportedSource(_)),
            "got UnsupportedSource: {err}"
        );
        assert!(
            matches!(
                err,
                ProviderError::AuthRequired | ProviderError::NoCookies | ProviderError::Other(_)
            ),
            "got: {err}"
        );
    }
}
