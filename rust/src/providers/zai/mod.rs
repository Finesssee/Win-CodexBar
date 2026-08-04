//! z.ai provider implementation
//!
//! Fetches usage data from z.ai's quota API
//! Uses API token stored in Windows Credential Manager

pub mod mcp_details;

// Re-exports for MCP details menu
#[allow(unused_imports)]
pub use mcp_details::{
    McpDetailsMenu, ZaiLimitEntry, ZaiLimitType, ZaiLimitUnit, ZaiUsageDetail, ZaiUsageSnapshot,
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Url;
use serde::Deserialize;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};

/// z.ai API endpoint for quota/usage
const ZAI_API_URL: &str = "https://api.z.ai/api/monitor/usage/quota/limit";
const ZAI_BIGMODEL_CN_API_URL: &str = "https://open.bigmodel.cn/api/monitor/usage/quota/limit";
const ZAI_QUOTA_URL_ENV: &str = "Z_AI_QUOTA_URL";
const ZAI_API_HOST_ENV: &str = "Z_AI_API_HOST";
const ZAI_API_KEY_ENV: &str = "Z_AI_API_KEY";
const ZAI_LEGACY_API_KEY_ENV: &str = "ZAI_API_TOKEN";
const ZAI_USAGE_SCOPE_ENV: &str = "Z_AI_USAGE_SCOPE";
const ZAI_BIGMODEL_ORG_ENV: &str = "Z_AI_BIGMODEL_ORGANIZATION";
const ZAI_BIGMODEL_PROJECT_ENV: &str = "Z_AI_BIGMODEL_PROJECT";

/// Windows Credential Manager target for z.ai API token
const ZAI_CREDENTIAL_TARGET: &str = "codexbar-zai";

/// z.ai quota response structure
#[derive(Debug, Deserialize)]
struct ZaiQuotaResponse {
    #[serde(default)]
    code: Option<i32>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    data: Option<ZaiQuotaData>,
    /// Legacy flat limits array (backwards compat)
    #[serde(default)]
    limits: Vec<ZaiLimit>,
}

#[derive(Debug, Deserialize)]
struct ZaiQuotaData {
    #[serde(default)]
    limits: Vec<ZaiLimit>,
    #[serde(rename = "planName")]
    plan_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ZaiLimit {
    /// Limit type: "TOKENS_LIMIT" or "TIME_LIMIT" (upstream) or "tokens"/"mcp" (legacy)
    #[serde(rename = "type")]
    limit_type: Option<String>,
    /// Used amount (legacy response)
    used: Option<f64>,
    /// Total limit (current response)
    usage: Option<f64>,
    /// Current value (alternative to used)
    #[serde(rename = "currentValue")]
    current_value: Option<f64>,
    /// Total limit
    limit: Option<f64>,
    /// Remaining amount
    remaining: Option<f64>,
    /// Used percentage (current response)
    percentage: Option<f64>,
    /// Time unit enum: 1=days, 3=hours, 5=minutes, 6=weeks
    unit: Option<i32>,
    /// Number of time units in the window
    number: Option<i32>,
    /// Reset time (ISO 8601)
    #[serde(rename = "resetAt")]
    reset_at: Option<String>,
    /// Reset time as Unix epoch milliseconds (current response)
    #[serde(rename = "nextResetTime")]
    next_reset_time: Option<i64>,
}

/// z.ai provider
pub struct ZaiProvider {
    metadata: ProviderMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ZaiTeamContext {
    organization_id: String,
    project_id: String,
}

impl ZaiProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Zai,
                display_name: "z.ai",
                session_label: "Tokens",
                weekly_label: "MCP",
                supports_opus: false,
                supports_credits: true,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://z.ai/manage-apikey/coding-plan/personal/my-plan"),
                status_page_url: None,
            },
        }
    }

    /// Get API token from ctx, Windows Credential Manager, or env
    fn get_api_token(api_key: Option<&str>) -> Result<String, ProviderError> {
        // Check ctx.api_key first (from settings)
        if let Some(key) = api_key
            && let Some(cleaned) = clean_string(key)
        {
            return Ok(cleaned);
        }

        // Try Windows Credential Manager
        match keyring::Entry::new(ZAI_CREDENTIAL_TARGET, "api_token") {
            Ok(entry) => match entry.get_password() {
                Ok(token) => Ok(token),
                Err(_) => Self::api_token_from_env(),
            },
            Err(_) => Self::api_token_from_env(),
        }
    }

    fn api_token_from_env() -> Result<String, ProviderError> {
        [ZAI_API_KEY_ENV, ZAI_LEGACY_API_KEY_ENV]
            .iter()
            .find_map(|key| std::env::var(key).ok().and_then(|value| clean_string(&value)))
            .ok_or_else(|| {
                ProviderError::NotInstalled(
                    "z.ai API token not found. Set in Preferences → Providers, Z_AI_API_KEY, or ZAI_API_TOKEN."
                        .to_string(),
                )
            })
    }

    fn quota_url(ctx: &FetchContext) -> Result<Url, ProviderError> {
        if let Ok(raw) = std::env::var(ZAI_QUOTA_URL_ENV)
            && let Some(value) = clean_string(&raw)
        {
            return parse_https_url(&value);
        }
        if let Ok(raw) = std::env::var(ZAI_API_HOST_ENV)
            && let Some(value) = clean_string(&raw)
        {
            return quota_url_from_host(&value);
        }

        let base = match ctx
            .api_region
            .as_deref()
            .map(|region| region.trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("cn") | Some("bigmodel") | Some("bigmodel-cn") | Some("bigmodel_cn") => {
                ZAI_BIGMODEL_CN_API_URL
            }
            _ => ZAI_API_URL,
        };
        Url::parse(base).map_err(|e| ProviderError::Other(e.to_string()))
    }

    fn request_url(
        ctx: &FetchContext,
        team_context: Option<&ZaiTeamContext>,
    ) -> Result<Url, ProviderError> {
        let mut url = Self::quota_url(ctx)?;
        if team_context.is_some() {
            url.query_pairs_mut().append_pair("type", "2");
        }
        Ok(url)
    }

    fn team_context(ctx: &FetchContext) -> Result<Option<ZaiTeamContext>, ProviderError> {
        let explicit_scope = std::env::var(ZAI_USAGE_SCOPE_ENV)
            .ok()
            .and_then(|value| clean_string(&value))
            .is_some_and(|value| value.eq_ignore_ascii_case("team"));
        let context = ctx
            .workspace_id
            .as_deref()
            .and_then(parse_team_context_pair)
            .or_else(ZaiTeamContext::from_env);

        if explicit_scope && context.is_none() {
            return Err(ProviderError::Other(
                "z.ai team usage requires Z_AI_BIGMODEL_ORGANIZATION and Z_AI_BIGMODEL_PROJECT, or workspace_id as organization|project."
                    .to_string(),
            ));
        }
        Ok(context)
    }

    /// Fetch usage from z.ai API
    async fn fetch_usage_api(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProviderError> {
        let api_token = Self::get_api_token(ctx.api_key.as_deref())?;

        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        let team_context = Self::team_context(ctx)?;
        let request_url = Self::request_url(ctx, team_context.as_ref())?;
        let mut request = client
            .get(request_url)
            .header("Authorization", authorization_header(&api_token))
            .header("Accept", "application/json");
        if let Some(team) = &team_context {
            request = request
                .header("Bigmodel-Organization", team.organization_id.as_str())
                .header("Bigmodel-Project", team.project_id.as_str());
        }
        let resp = request.send().await?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ProviderError::AuthRequired);
        }

        if !resp.status().is_success() {
            return Err(ProviderError::Other(format!(
                "z.ai API returned status {}",
                resp.status()
            )));
        }

        let resp_bytes = resp
            .bytes()
            .await
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        // Handle empty response body (can happen with wrong region/endpoint)
        if resp_bytes.is_empty() {
            return Err(ProviderError::Parse(
                "Empty response body from z.ai API. Check API region and token.".to_string(),
            ));
        }

        let quota: ZaiQuotaResponse =
            serde_json::from_slice(&resp_bytes).map_err(|e| ProviderError::Parse(e.to_string()))?;

        self.parse_quota_response(&quota)
    }

    fn parse_quota_response(
        &self,
        quota: &ZaiQuotaResponse,
    ) -> Result<UsageSnapshot, ProviderError> {
        if quota.code.is_some_and(|code| code != 0 && code != 200) {
            return Err(ProviderError::Other(
                quota
                    .message
                    .as_deref()
                    .filter(|message| !message.trim().is_empty())
                    .unwrap_or("z.ai API returned an error")
                    .to_string(),
            ));
        }

        // Get limits from data.limits (upstream) or flat limits (legacy)
        let limits = if let Some(data) = &quota.data {
            &data.limits
        } else {
            &quota.limits
        };
        let plan_name = quota
            .data
            .as_ref()
            .and_then(|d| d.plan_name.as_deref())
            .unwrap_or("z.ai");

        // Collect TOKENS_LIMIT entries (upstream uses "TOKENS_LIMIT", legacy uses "tokens")
        let mut token_limits: Vec<&ZaiLimit> = limits
            .iter()
            .filter(|l| {
                matches!(
                    l.limit_type.as_deref(),
                    Some("TOKENS_LIMIT") | Some("tokens")
                )
            })
            .collect();

        // Find TIME_LIMIT entry (or legacy "mcp")
        let time_limit = limits
            .iter()
            .find(|l| matches!(l.limit_type.as_deref(), Some("TIME_LIMIT") | Some("mcp")));

        // Sort token limits by window_minutes: shortest first
        token_limits.sort_by_key(|l| Self::window_minutes(l));

        // Compute used percent for a limit entry
        fn compute_percent(l: &ZaiLimit) -> f64 {
            if let Some(percentage) = l.percentage {
                return percentage.clamp(0.0, 100.0);
            }

            let limit = l.limit.or(l.usage).unwrap_or(0.0);
            if limit <= 0.0 {
                return if l.used.unwrap_or(0.0) > 0.0 || l.current_value.unwrap_or(0.0) > 0.0 {
                    100.0
                } else {
                    0.0
                };
            }
            let used = {
                let from_remaining = l.remaining.map(|r| limit - r);
                let from_current = l.current_value;
                let from_used = l.used;
                // Use max of available signals
                let candidates = [from_remaining, from_current, from_used];
                candidates.iter().filter_map(|&v| v).fold(0.0_f64, f64::max)
            };
            ((used / limit) * 100.0).clamp(0.0, 100.0)
        }

        fn make_window(l: &ZaiLimit, window_mins: Option<u32>) -> RateWindow {
            let resets_at = l
                .next_reset_time
                .and_then(DateTime::<Utc>::from_timestamp_millis)
                .or_else(|| {
                    l.reset_at
                        .as_deref()
                        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                        .map(|timestamp| timestamp.with_timezone(&Utc))
                });
            let reset_description = rate_window_description(l, window_mins);
            RateWindow::with_details(
                compute_percent(l),
                window_mins,
                resets_at,
                reset_description,
            )
        }

        fn rate_window_minutes(l: &ZaiLimit) -> Option<u32> {
            // Upstream #2431 / #2566: verified z.ai duration table.
            // MCP 1-minute marker and bare TIME_LIMIT → monthly sentinel;
            // explicit unit/number durations kept as-is; tokens use computed minutes.
            if is_mcp_monthly_marker(l) {
                return Some(30 * 24 * 60);
            }
            let explicit = ZaiProvider::window_minutes(l);
            let is_time = matches!(l.limit_type.as_deref(), Some("TIME_LIMIT") | Some("mcp"));
            if is_time {
                explicit.or(Some(30 * 24 * 60))
            } else {
                explicit
            }
        }

        fn rate_window_description(l: &ZaiLimit, window_mins: Option<u32>) -> Option<String> {
            if is_mcp_monthly_marker(l) {
                return Some("Monthly".into());
            }
            if let Some(label) = window_label(l) {
                return Some(label);
            }
            if matches!(l.limit_type.as_deref(), Some("TIME_LIMIT") | Some("mcp")) {
                // Bare time limit with monthly sentinel.
                if window_mins == Some(30 * 24 * 60) {
                    return Some("Monthly".into());
                }
            }
            None
        }

        // Build windows based on upstream layout:
        // If 2+ TOKENS_LIMIT: shortest → session (5-hour), longest → weekly (primary)
        // TIME_LIMIT → secondary
        let (primary, secondary, tertiary) = match token_limits.len() {
            0 => {
                // No token limits; use time_limit as primary if available
                let p = time_limit
                    .map(|l| make_window(l, rate_window_minutes(l)))
                    .unwrap_or_else(|| RateWindow::new(0.0));
                (p, None, None)
            }
            1 => {
                let p = make_window(token_limits[0], rate_window_minutes(token_limits[0]));
                let s = time_limit.map(|l| make_window(l, rate_window_minutes(l)));
                (p, s, None)
            }
            _ => {
                // 2+ token limits: longest → primary (weekly), shortest → tertiary (5-hour)
                let weekly = token_limits.last().unwrap();
                let session = token_limits.first().unwrap();
                let p = make_window(weekly, rate_window_minutes(weekly));
                let s = time_limit.map(|l| make_window(l, rate_window_minutes(l)));
                let t = Some(make_window(session, rate_window_minutes(session)));
                (p, s, t)
            }
        };

        let mut usage = UsageSnapshot::new(primary).with_login_method(plan_name);
        if let Some(sec) = secondary {
            usage = usage.with_secondary(sec);
        }
        if let Some(ter) = tertiary {
            usage = usage.with_model_specific(ter);
        }

        Ok(usage)
    }

    /// Compute window_minutes from a limit's unit + number fields.
    /// Returns `None` when number ≤ 0 or unit is unknown (upstream windowMinutes).
    fn window_minutes(l: &ZaiLimit) -> Option<u32> {
        let number = l.number.filter(|&n| n > 0)? as u32;
        let unit = l.unit?;
        let minutes_per_unit = match unit {
            1 => 1440,  // days
            3 => 60,    // hours
            5 => 1,     // minutes
            6 => 10080, // weeks
            _ => return None,
        };
        Some(number * minutes_per_unit)
    }
}

fn is_mcp_monthly_marker(l: &ZaiLimit) -> bool {
    // Upstream: timeLimit + unit.minutes + number == 1 → monthly MCP marker.
    matches!(l.limit_type.as_deref(), Some("TIME_LIMIT") | Some("mcp"))
        && l.unit == Some(5)
        && l.number == Some(1)
}

fn window_label(l: &ZaiLimit) -> Option<String> {
    let number = l.number.filter(|&n| n > 0)?;
    let unit = l.unit?;
    let unit_label = match unit {
        1 => {
            if number == 1 {
                "day"
            } else {
                "days"
            }
        }
        3 => {
            if number == 1 {
                "hour"
            } else {
                "hours"
            }
        }
        5 => {
            if number == 1 {
                "minute"
            } else {
                "minutes"
            }
        }
        6 => {
            if number == 1 {
                "week"
            } else {
                "weeks"
            }
        }
        _ => return None,
    };
    Some(format!("{number} {unit_label} window"))
}

impl ZaiTeamContext {
    fn from_env() -> Option<Self> {
        let organization_id = std::env::var(ZAI_BIGMODEL_ORG_ENV)
            .ok()
            .and_then(|value| clean_string(&value))?;
        let project_id = std::env::var(ZAI_BIGMODEL_PROJECT_ENV)
            .ok()
            .and_then(|value| clean_string(&value))?;
        Some(Self {
            organization_id,
            project_id,
        })
    }
}

fn clean_string(raw: &str) -> Option<String> {
    let mut value = raw.trim();
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        value = &value[1..value.len() - 1];
    }
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn parse_https_url(raw: &str) -> Result<Url, ProviderError> {
    let value = if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    let url = Url::parse(&value).map_err(|e| ProviderError::Other(e.to_string()))?;
    if url.scheme() != "https" {
        return Err(ProviderError::Other(
            "z.ai endpoint overrides must use HTTPS.".to_string(),
        ));
    }
    Ok(url)
}

fn quota_url_from_host(raw: &str) -> Result<Url, ProviderError> {
    let mut url = parse_https_url(raw)?;
    url.set_path("api/monitor/usage/quota/limit");
    url.set_query(None);
    Ok(url)
}

fn parse_team_context_pair(raw: &str) -> Option<ZaiTeamContext> {
    let (organization_id, project_id) = raw
        .split_once('|')
        .or_else(|| raw.split_once(','))
        .or_else(|| raw.split_once(';'))?;
    Some(ZaiTeamContext {
        organization_id: clean_string(organization_id)?,
        project_id: clean_string(project_id)?,
    })
}

fn authorization_header(token: &str) -> String {
    let trimmed = token.trim();
    if trimmed.to_ascii_lowercase().starts_with("bearer ") {
        trimmed.to_string()
    } else {
        format!("Bearer {trimmed}")
    }
}

impl Default for ZaiProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for ZaiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Zai
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Fetching z.ai usage");

        // z.ai only supports OAuth/API token - no CLI or web cookie fallback
        match ctx.source_mode {
            SourceMode::Auto | SourceMode::OAuth => {
                let usage = self.fetch_usage_api(ctx).await?;
                Ok(ProviderFetchResult::new(usage, "oauth"))
            }
            SourceMode::Web | SourceMode::Cli => {
                // z.ai doesn't support web cookies or CLI
                Err(ProviderError::UnsupportedSource(ctx.source_mode))
            }
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::OAuth]
    }

    fn supports_web(&self) -> bool {
        false
    }

    fn supports_cli(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_url_adds_team_type_query_for_team_context() {
        let ctx = FetchContext::default();
        let team = ZaiTeamContext {
            organization_id: "org".to_string(),
            project_id: "project".to_string(),
        };

        let url = ZaiProvider::request_url(&ctx, Some(&team)).expect("url");

        assert_eq!(
            url.as_str(),
            "https://api.z.ai/api/monitor/usage/quota/limit?type=2"
        );
    }

    #[test]
    fn quota_url_uses_bigmodel_cn_region_aliases() {
        let ctx = FetchContext {
            api_region: Some("bigmodel-cn".to_string()),
            ..FetchContext::default()
        };

        let url = ZaiProvider::quota_url(&ctx).expect("url");

        assert_eq!(
            url.as_str(),
            "https://open.bigmodel.cn/api/monitor/usage/quota/limit"
        );
    }

    #[test]
    fn parses_workspace_pair_as_team_context() {
        let parsed = parse_team_context_pair(" org-team | project-team ").expect("team context");

        assert_eq!(parsed.organization_id, "org-team");
        assert_eq!(parsed.project_id, "project-team");
    }

    #[test]
    fn parses_successful_response_without_message() {
        let provider = ZaiProvider::new();
        let quota: ZaiQuotaResponse = serde_json::from_value(serde_json::json!({
            "code": 200,
            "data": {
                "planName": "BigModel CN",
                "limits": [{
                    "type": "TOKENS_LIMIT",
                    "used": 10,
                    "limit": 100,
                    "unit": 3,
                    "number": 5
                }]
            }
        }))
        .unwrap();

        let usage = provider.parse_quota_response(&quota).unwrap();

        assert_eq!(usage.login_method.as_deref(), Some("BigModel CN"));
        assert_eq!(usage.primary.used_percent, 10.0);
    }

    #[test]
    fn parses_current_api_percentage_and_reset_time() {
        let provider = ZaiProvider::new();
        let quota: ZaiQuotaResponse = serde_json::from_value(serde_json::json!({
            "code": 200,
            "data": {
                "limits": [{
                    "type": "TOKENS_LIMIT",
                    "unit": 3,
                    "number": 5,
                    "usage": 800000000,
                    "currentValue": 600000000,
                    "remaining": 200000000,
                    "percentage": 75,
                    "nextResetTime": 1770648402389_i64
                }]
            }
        }))
        .unwrap();

        let usage = provider.parse_quota_response(&quota).unwrap();

        assert_eq!(usage.primary.used_percent, 75.0);
        assert_eq!(usage.primary.window_minutes, Some(300));
        assert!(usage.primary.resets_at.is_some());
    }

    #[test]
    fn time_limit_with_explicit_duration_keeps_minutes() {
        let provider = ZaiProvider::new();
        let quota: ZaiQuotaResponse = serde_json::from_value(serde_json::json!({
            "code": 200,
            "data": {
                "limits": [{
                    "type": "TIME_LIMIT",
                    "unit": 3,
                    "number": 5,
                    "usage": 100,
                    "currentValue": 20,
                    "remaining": 80,
                    "percentage": 25,
                    "nextResetTime": 123000_i64
                }]
            }
        }))
        .unwrap();
        let usage = provider.parse_quota_response(&quota).unwrap();
        assert_eq!(usage.primary.window_minutes, Some(300));
        assert_eq!(
            usage.primary.reset_description.as_deref(),
            Some("5 hours window")
        );
    }

    #[test]
    fn time_limit_without_duration_uses_monthly_sentinel() {
        let provider = ZaiProvider::new();
        let quota: ZaiQuotaResponse = serde_json::from_value(serde_json::json!({
            "code": 200,
            "data": {
                "limits": [{
                    "type": "TIME_LIMIT",
                    "unit": 1,
                    "number": 0,
                    "usage": 100,
                    "currentValue": 20,
                    "remaining": 80,
                    "percentage": 25,
                    "nextResetTime": 123000_i64
                }]
            }
        }))
        .unwrap();
        let usage = provider.parse_quota_response(&quota).unwrap();
        assert_eq!(usage.primary.window_minutes, Some(30 * 24 * 60));
        assert_eq!(usage.primary.reset_description.as_deref(), Some("Monthly"));
    }

    #[test]
    fn mcp_one_minute_marker_is_monthly_sentinel() {
        let provider = ZaiProvider::new();
        let quota: ZaiQuotaResponse = serde_json::from_value(serde_json::json!({
            "code": 200,
            "data": {
                "limits": [
                    {
                        "type": "TOKENS_LIMIT",
                        "unit": 6,
                        "number": 1,
                        "percentage": 34
                    },
                    {
                        "type": "TIME_LIMIT",
                        "unit": 5,
                        "number": 1,
                        "percentage": 10
                    }
                ]
            }
        }))
        .unwrap();
        let usage = provider.parse_quota_response(&quota).unwrap();
        let secondary = usage.secondary.expect("time limit secondary");
        assert_eq!(secondary.window_minutes, Some(30 * 24 * 60));
        assert_eq!(secondary.reset_description.as_deref(), Some("Monthly"));
    }

    #[test]
    fn preserves_api_code_error_message() {
        let provider = ZaiProvider::new();
        let quota: ZaiQuotaResponse = serde_json::from_value(serde_json::json!({
            "code": 401,
            "message": "invalid token"
        }))
        .unwrap();

        let error = provider.parse_quota_response(&quota).unwrap_err();

        assert!(error.to_string().contains("invalid token"));
    }
}
