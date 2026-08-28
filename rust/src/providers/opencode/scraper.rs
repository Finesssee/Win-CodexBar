//! OpenCode Advanced Web Scraping
//!
//! Multi-stage fetcher for opencode.ai with:
//! - Workspace ID resolution
//! - Subscription info fetching via custom X-Server-Id headers
//! - Robust JSON parsing with multiple fallback strategies

use chrono::{DateTime, Duration, Utc};
use regex_lite::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// OpenCode server IDs for API endpoints
const WORKSPACES_SERVER_ID: &str =
    "def39973159c7f0483d8793a822b8dbb10d067e12c65455fcb4608459ba0234f";
const SUBSCRIPTION_SERVER_ID: &str =
    "7abeebee372f304e050aaaf92be863f4a86490e382f8c79db68fd94040d691b4";
const BASE_URL: &str = "https://opencode.ai";
const SERVER_URL: &str = "https://opencode.ai/_server";

/// Keys to look for when parsing percent values
const PERCENT_KEYS: &[&str] = &[
    "usagePercent",
    "usedPercent",
    "percentUsed",
    "percent",
    "usage_percent",
    "used_percent",
    "utilization",
    "utilizationPercent",
    "utilization_percent",
    // Note: raw "usage" is often a token count — handled via used/limit only.
];

/// Keys to look for when parsing reset time in seconds
const RESET_IN_KEYS: &[&str] = &[
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

/// Keys to look for when parsing reset timestamp
const RESET_AT_KEYS: &[&str] = &[
    "resetAt",
    "resetsAt",
    "reset_at",
    "resets_at",
    "nextReset",
    "next_reset",
    "renewAt",
    "renew_at",
];

/// OpenCode usage snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenCodeUsageSnapshot {
    /// Rolling window usage percent (5-hour window)
    pub rolling_usage_percent: f64,
    /// Weekly usage percent
    pub weekly_usage_percent: f64,
    /// Rolling window reset in seconds
    pub rolling_reset_in_sec: i64,
    /// Weekly reset in seconds
    pub weekly_reset_in_sec: i64,
    /// Account renewal date when exposed by OpenCode.
    pub renews_at: Option<DateTime<Utc>>,
    /// When this snapshot was captured
    pub updated_at: DateTime<Utc>,
}

impl OpenCodeUsageSnapshot {
    /// Get rolling window reset time
    pub fn rolling_resets_at(&self) -> DateTime<Utc> {
        self.updated_at + Duration::seconds(self.rolling_reset_in_sec)
    }

    /// Get weekly reset time
    pub fn weekly_resets_at(&self) -> DateTime<Utc> {
        self.updated_at + Duration::seconds(self.weekly_reset_in_sec)
    }
}

/// OpenCode usage fetcher errors
#[derive(Debug, Clone)]
pub enum OpenCodeError {
    /// Session cookie is invalid or expired
    InvalidCredentials,
    /// Network error
    NetworkError(String),
    /// API error
    ApiError(String),
    /// Parse error
    ParseFailed(String),
}

impl std::fmt::Display for OpenCodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenCodeError::InvalidCredentials => {
                write!(f, "OpenCode session cookie is invalid or expired")
            }
            OpenCodeError::NetworkError(msg) => write!(f, "OpenCode network error: {}", msg),
            OpenCodeError::ApiError(msg) => write!(f, "OpenCode API error: {}", msg),
            OpenCodeError::ParseFailed(msg) => write!(f, "OpenCode parse error: {}", msg),
        }
    }
}

impl std::error::Error for OpenCodeError {}

/// Server request configuration
struct ServerRequest {
    server_id: String,
    args: Option<Vec<serde_json::Value>>,
    method: String,
    referer: String,
}

/// OpenCode usage fetcher
pub struct OpenCodeUsageFetcher;

impl OpenCodeUsageFetcher {
    /// Fetch usage data from OpenCode
    pub async fn fetch_usage(
        cookie_header: &str,
        timeout_secs: u64,
        workspace_id_override: Option<&str>,
    ) -> Result<OpenCodeUsageSnapshot, OpenCodeError> {
        let now = Utc::now();

        // Get or resolve workspace ID
        let workspace_id = if let Some(override_id) = workspace_id_override {
            Self::normalize_workspace_id(override_id)
                .ok_or_else(|| OpenCodeError::ParseFailed("Invalid workspace ID".to_string()))?
        } else {
            Self::fetch_workspace_id(cookie_header, timeout_secs).await?
        };

        // Fetch subscription info
        let subscription_text =
            Self::fetch_subscription_info(&workspace_id, cookie_header, timeout_secs).await?;

        // Parse subscription
        Self::parse_subscription(&subscription_text, now)
    }

    /// Fetch workspace ID from OpenCode
    async fn fetch_workspace_id(
        cookie_header: &str,
        timeout_secs: u64,
    ) -> Result<String, OpenCodeError> {
        let request = ServerRequest {
            server_id: WORKSPACES_SERVER_ID.to_string(),
            args: None,
            method: "GET".to_string(),
            referer: BASE_URL.to_string(),
        };

        let text = Self::fetch_server_text(&request, cookie_header, timeout_secs).await?;

        if Self::looks_signed_out(&text) {
            return Err(OpenCodeError::InvalidCredentials);
        }

        // Try parsing workspace IDs
        let mut ids = Self::parse_workspace_ids(&text);
        if ids.is_empty() {
            ids = Self::parse_workspace_ids_from_json(&text);
        }

        if ids.is_empty() {
            // Retry with POST
            let retry_request = ServerRequest {
                server_id: WORKSPACES_SERVER_ID.to_string(),
                args: Some(vec![]),
                method: "POST".to_string(),
                referer: BASE_URL.to_string(),
            };

            let fallback =
                Self::fetch_server_text(&retry_request, cookie_header, timeout_secs).await?;

            if Self::looks_signed_out(&fallback) {
                return Err(OpenCodeError::InvalidCredentials);
            }

            ids = Self::parse_workspace_ids(&fallback);
            if ids.is_empty() {
                ids = Self::parse_workspace_ids_from_json(&fallback);
            }

            if ids.is_empty() {
                return Err(OpenCodeError::ParseFailed(
                    "Missing workspace id".to_string(),
                ));
            }
        }

        Ok(ids.into_iter().next().unwrap())
    }

    /// Fetch subscription info for a workspace
    async fn fetch_subscription_info(
        workspace_id: &str,
        cookie_header: &str,
        timeout_secs: u64,
    ) -> Result<String, OpenCodeError> {
        let referer = format!("{}/workspace/{}/billing", BASE_URL, workspace_id);
        let request = ServerRequest {
            server_id: SUBSCRIPTION_SERVER_ID.to_string(),
            args: Some(vec![serde_json::Value::String(workspace_id.to_string())]),
            method: "GET".to_string(),
            referer,
        };

        let text = Self::fetch_server_text(&request, cookie_header, timeout_secs).await?;

        if Self::looks_signed_out(&text) {
            return Err(OpenCodeError::InvalidCredentials);
        }

        // Check if we got valid data, otherwise retry with POST
        if Self::parse_subscription_json(&text, Utc::now()).is_none()
            && Self::extract_double(
                r#"rollingUsage[^}]*?usagePercent\s*:\s*([0-9]+(?:\.[0-9]+)?)"#,
                &text,
            )
            .is_none()
        {
            let retry_request = ServerRequest {
                server_id: SUBSCRIPTION_SERVER_ID.to_string(),
                args: Some(vec![serde_json::Value::String(workspace_id.to_string())]),
                method: "POST".to_string(),
                referer: format!("{}/workspace/{}/billing", BASE_URL, workspace_id),
            };

            let fallback =
                Self::fetch_server_text(&retry_request, cookie_header, timeout_secs).await?;

            if Self::looks_signed_out(&fallback) {
                return Err(OpenCodeError::InvalidCredentials);
            }

            return Ok(fallback);
        }

        Ok(text)
    }

    /// Normalize a workspace ID from various formats
    pub fn normalize_workspace_id(raw: &str) -> Option<String> {
        let trimmed = raw.trim();

        // Already a valid workspace ID
        if trimmed.starts_with("wrk_") && trimmed.len() > 4 {
            return Some(trimmed.to_string());
        }

        // Try to parse as URL
        if let Ok(url) = reqwest::Url::parse(trimmed) {
            let parts: Vec<&str> = url.path_segments().map(|s| s.collect()).unwrap_or_default();
            if let Some(idx) = parts.iter().position(|&p| p == "workspace")
                && parts.len() > idx + 1
            {
                let candidate = parts[idx + 1];
                if candidate.starts_with("wrk_") && candidate.len() > 4 {
                    return Some(candidate.to_string());
                }
            }
        }

        // Try regex extraction
        static WRK_REGEX: OnceLock<Regex> = OnceLock::new();
        let regex = WRK_REGEX.get_or_init(|| Regex::new(r"wrk_[A-Za-z0-9]+").unwrap());

        regex.find(trimmed).map(|m| m.as_str().to_string())
    }

    /// Fetch server text with custom headers
    async fn fetch_server_text(
        request: &ServerRequest,
        cookie_header: &str,
        timeout_secs: u64,
    ) -> Result<String, OpenCodeError> {
        let url = Self::build_server_url(&request.server_id, &request.args, &request.method);

        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| OpenCodeError::NetworkError(e.to_string()))?;

        let mut req = if request.method == "GET" {
            client.get(&url)
        } else {
            client.post(&url)
        };

        req = req
            .header("Cookie", cookie_header)
            .header("X-Server-Id", &request.server_id)
            .header("X-Server-Instance", format!("server-fn:{}", uuid::Uuid::new_v4()))
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/120.0.0.0 Safari/537.36")
            .header("Origin", BASE_URL)
            .header("Referer", &request.referer)
            .header("Accept", "text/javascript, application/json;q=0.9, */*;q=0.8");

        if request.method != "GET"
            && let Some(args) = &request.args
        {
            req = req.header("Content-Type", "application/json").json(args);
        }

        let response = req
            .send()
            .await
            .map_err(|e| OpenCodeError::NetworkError(e.to_string()))?;

        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| OpenCodeError::NetworkError(e.to_string()))?;

        if !status.is_success() {
            if Self::looks_signed_out(&text) {
                return Err(OpenCodeError::InvalidCredentials);
            }
            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(OpenCodeError::InvalidCredentials);
            }
            return Err(OpenCodeError::ApiError(format!("HTTP {}", status)));
        }

        Ok(text)
    }

    /// Build server request URL
    fn build_server_url(
        server_id: &str,
        args: &Option<Vec<serde_json::Value>>,
        method: &str,
    ) -> String {
        if method != "GET" {
            return SERVER_URL.to_string();
        }

        let mut url = format!("{}?id={}", SERVER_URL, server_id);
        if let Some(args) = args
            && !args.is_empty()
            && let Ok(json_str) = serde_json::to_string(args)
        {
            url.push_str(&format!("&args={}", Self::url_encode(&json_str)));
        }
        url
    }

    /// URL encode a string for query parameters
    fn url_encode(s: &str) -> String {
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

    /// Parse subscription text into snapshot
    pub fn parse_subscription(
        text: &str,
        now: DateTime<Utc>,
    ) -> Result<OpenCodeUsageSnapshot, OpenCodeError> {
        // Try JSON parsing first
        if let Some(snapshot) = Self::parse_subscription_json(text, now) {
            return Ok(snapshot);
        }

        // Fall back to regex parsing
        let rolling_percent = Self::extract_double(
            r#"rollingUsage[^}]*?usagePercent\s*:\s*([0-9]+(?:\.[0-9]+)?)"#,
            text,
        );
        let rolling_reset =
            Self::extract_int(r#"rollingUsage[^}]*?resetInSec\s*:\s*([0-9]+)"#, text);
        let weekly_percent = Self::extract_double(
            r#"weeklyUsage[^}]*?usagePercent\s*:\s*([0-9]+(?:\.[0-9]+)?)"#,
            text,
        );
        let weekly_reset = Self::extract_int(r#"weeklyUsage[^}]*?resetInSec\s*:\s*([0-9]+)"#, text);

        match (rolling_percent, rolling_reset, weekly_percent, weekly_reset) {
            (Some(rp), Some(rr), Some(wp), Some(wr)) => Ok(OpenCodeUsageSnapshot {
                rolling_usage_percent: rp,
                weekly_usage_percent: wp,
                rolling_reset_in_sec: rr,
                weekly_reset_in_sec: wr,
                renews_at: crate::providers::extract_renewal(text),
                updated_at: now,
            }),
            _ => Err(OpenCodeError::ParseFailed(
                "Missing usage fields".to_string(),
            )),
        }
    }

    /// Parse subscription JSON
    fn parse_subscription_json(text: &str, now: DateTime<Utc>) -> Option<OpenCodeUsageSnapshot> {
        let json: serde_json::Value = serde_json::from_str(text).ok()?;
        let renews_at = Self::find_datetime(&json, &["renewAt", "renew_at"]);

        // Try direct parsing
        if let Some(mut snapshot) = Self::parse_usage_dict(&json, now) {
            snapshot.renews_at = renews_at;
            return Some(snapshot);
        }

        // Try nested keys
        for key in ["data", "result", "usage", "billing", "payload"] {
            if let Some(nested) = json.get(key)
                && let Some(mut snapshot) = Self::parse_usage_dict(nested, now)
            {
                snapshot.renews_at = snapshot.renews_at.or(renews_at);
                return Some(snapshot);
            }
        }

        None
    }

    /// Parse usage from a JSON object
    fn parse_usage_dict(
        json: &serde_json::Value,
        now: DateTime<Utc>,
    ) -> Option<OpenCodeUsageSnapshot> {
        let rolling_keys = ["rollingUsage", "rolling", "rolling_usage"];
        let weekly_keys = ["weeklyUsage", "weekly", "weekly_usage"];

        let rolling = rolling_keys.iter().find_map(|k| json.get(k));
        let weekly = weekly_keys.iter().find_map(|k| json.get(k));

        if let (Some(rolling), Some(weekly)) = (rolling, weekly) {
            let rolling_window = Self::parse_window(rolling, now)?;
            let weekly_window = Self::parse_window(weekly, now)?;

            return Some(OpenCodeUsageSnapshot {
                rolling_usage_percent: rolling_window.0,
                weekly_usage_percent: weekly_window.0,
                rolling_reset_in_sec: rolling_window.1,
                weekly_reset_in_sec: weekly_window.1,
                renews_at: Self::find_datetime(json, &["renewAt", "renew_at"]),
                updated_at: now,
            });
        }

        None
    }

    /// Parse a window object into (percent, reset_in_sec)
    fn parse_window(json: &serde_json::Value, _now: DateTime<Utc>) -> Option<(f64, i64)> {
        // Direct percent field (may be 0..1 fraction or 0..100).
        let direct = PERCENT_KEYS
            .iter()
            .find_map(|k| json.get(k).and_then(|v| v.as_f64()));
        let percent_is_direct = direct.is_some();

        let percent = direct.or_else(|| {
            // Computed used/limit is already 0..100 — never apply fraction rescale (#2331).
            let used = ["used", "usage", "consumed", "count", "usedTokens"]
                .iter()
                .find_map(|k| json.get(*k).and_then(|v| v.as_f64()));
            let limit = ["limit", "total", "allowance"]
                .iter()
                .find_map(|k| json.get(*k).and_then(|v| v.as_f64()));
            match (used, limit) {
                (Some(u), Some(l)) if l > 0.0 => Some((u / l) * 100.0),
                _ => None,
            }
        })?;

        let reset_in = RESET_IN_KEYS
            .iter()
            .find_map(|k| json.get(k).and_then(|v| v.as_i64()))
            .unwrap_or(0);

        let normalized_percent = if percent_is_direct && (0.0..=1.0).contains(&percent) {
            percent * 100.0
        } else {
            percent.clamp(0.0, 100.0)
        };
        Some((normalized_percent.clamp(0.0, 100.0), reset_in.max(0)))
    }

    fn find_datetime(json: &serde_json::Value, keys: &[&str]) -> Option<DateTime<Utc>> {
        for key in keys {
            if let Some(value) = json.get(*key)
                && let Some(parsed) = Self::date_from_value(value)
            {
                return Some(parsed);
            }
        }

        if let Some(obj) = json.as_object() {
            for value in obj.values() {
                if let Some(parsed) = Self::find_datetime(value, keys) {
                    return Some(parsed);
                }
            }
        }
        None
    }

    fn date_from_value(value: &serde_json::Value) -> Option<DateTime<Utc>> {
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
        #[allow(
            clippy::cast_possible_truncation,
            reason = "timestamps are normalized to whole seconds (ms input divided by 1000) before the cast"
        )]
        DateTime::<Utc>::from_timestamp(seconds as i64, 0)
    }

    /// Parse workspace IDs from text using regex
    fn parse_workspace_ids(text: &str) -> Vec<String> {
        // Try JSON parsing first
        let json_ids = Self::parse_workspace_ids_from_json(text);
        if !json_ids.is_empty() {
            return json_ids;
        }

        // Fall back to regex for JavaScript object notation (unquoted keys)
        static WRK_ID_REGEX: OnceLock<Regex> = OnceLock::new();
        let regex = WRK_ID_REGEX.get_or_init(|| {
            Regex::new(r#""id"\s*:\s*"(wrk_[^"]+)"|id\s*:\s*"(wrk_[^"]+)""#).unwrap()
        });

        regex
            .captures_iter(text)
            .filter_map(|cap| {
                cap.get(1)
                    .or_else(|| cap.get(2))
                    .map(|m| m.as_str().to_string())
            })
            .collect()
    }

    /// Parse workspace IDs from JSON
    fn parse_workspace_ids_from_json(text: &str) -> Vec<String> {
        let json: serde_json::Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(_) => return vec![],
        };

        let mut results = Vec::new();
        Self::collect_workspace_ids(&json, &mut results);
        results
    }

    /// Recursively collect workspace IDs from JSON
    fn collect_workspace_ids(value: &serde_json::Value, out: &mut Vec<String>) {
        match value {
            serde_json::Value::String(s) if s.starts_with("wrk_") && !out.contains(s) => {
                out.push(s.clone());
            }
            serde_json::Value::Array(arr) => {
                for item in arr {
                    Self::collect_workspace_ids(item, out);
                }
            }
            serde_json::Value::Object(obj) => {
                for v in obj.values() {
                    Self::collect_workspace_ids(v, out);
                }
            }
            _ => {}
        }
    }

    /// Check if response indicates signed out
    fn looks_signed_out(text: &str) -> bool {
        let lower = text.to_lowercase();
        lower.contains("login") || lower.contains("sign in") || lower.contains("auth/authorize")
    }

    /// Extract a double from text using regex
    fn extract_double(pattern: &str, text: &str) -> Option<f64> {
        let regex = Regex::new(pattern).ok()?;
        let captures = regex.captures(text)?;
        let value_str = captures.get(1)?.as_str();
        value_str.parse().ok()
    }

    /// Extract an int from text using regex
    fn extract_int(pattern: &str, text: &str) -> Option<i64> {
        let regex = Regex::new(pattern).ok()?;
        let captures = regex.captures(text)?;
        let value_str = captures.get(1)?.as_str();
        value_str.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_workspace_id_direct() {
        assert_eq!(
            OpenCodeUsageFetcher::normalize_workspace_id("wrk_abc123"),
            Some("wrk_abc123".to_string())
        );
    }

    #[test]
    fn test_normalize_workspace_id_url() {
        assert_eq!(
            OpenCodeUsageFetcher::normalize_workspace_id(
                "https://opencode.ai/workspace/wrk_xyz789/billing"
            ),
            Some("wrk_xyz789".to_string())
        );
    }

    #[test]
    fn test_normalize_workspace_id_invalid() {
        assert_eq!(
            OpenCodeUsageFetcher::normalize_workspace_id("invalid"),
            None
        );
    }

    #[test]
    fn test_parse_workspace_ids() {
        let text = r#"{"workspaces":[{"id":"wrk_abc123"},{"id":"wrk_def456"}]}"#;
        let ids = OpenCodeUsageFetcher::parse_workspace_ids(text);
        assert_eq!(ids, vec!["wrk_abc123", "wrk_def456"]);
    }

    #[test]
    fn test_looks_signed_out() {
        assert!(OpenCodeUsageFetcher::looks_signed_out(
            "Please login to continue"
        ));
        assert!(OpenCodeUsageFetcher::looks_signed_out("Sign in required"));
        assert!(!OpenCodeUsageFetcher::looks_signed_out(
            "Welcome to OpenCode"
        ));
    }

    #[test]
    fn test_parse_subscription_json() {
        let json = r#"{
            "rollingUsage": {"usagePercent": 45.5, "resetInSec": 3600},
            "weeklyUsage": {"usagePercent": 20.0, "resetInSec": 604800}
        }"#;

        let snapshot = OpenCodeUsageFetcher::parse_subscription(json, Utc::now()).unwrap();
        assert!((snapshot.rolling_usage_percent - 45.5).abs() < 0.01);
        assert!((snapshot.weekly_usage_percent - 20.0).abs() < 0.01);
        assert_eq!(snapshot.rolling_reset_in_sec, 3600);
        assert_eq!(snapshot.weekly_reset_in_sec, 604800);
    }
}
