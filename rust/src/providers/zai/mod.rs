//! z.ai provider implementation
//!
//! Fetches usage data from z.ai's quota API
//! Uses API token stored in Windows Credential Manager

pub mod mcp_details;
pub mod region;
pub mod settings;

// Re-exports for MCP details menu
#[allow(unused_imports)]
pub use mcp_details::{
    McpDetailsMenu, ZaiLimitEntry, ZaiLimitType, ZaiLimitUnit, ZaiUsageDetail, ZaiUsageSnapshot,
};
pub use region::ZaiRegion;
pub use settings::ZaiSettingsError;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Url;
use serde::Deserialize;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};

use settings::ZaiSettingsReader;

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
    /// Upstream plan-name fallbacks (`level` added in 0.48.0).
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default, rename = "packageName")]
    package_name: Option<String>,
    #[serde(default)]
    level: Option<String>,
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

    /// Effective API region (upstream 0.48.0): an explicit settings value
    /// wins; otherwise the region is inferred from canonical endpoint
    /// overrides (`inferredRegion`).
    fn effective_region(ctx: &FetchContext, env: &settings::EnvMap) -> ZaiRegion {
        match ctx
            .api_region
            .as_deref()
            .map(str::trim)
            .filter(|raw| !raw.is_empty())
        {
            Some(raw) => ZaiRegion::from_settings_value(Some(raw)),
            None => ZaiSettingsReader::inferred_region(env),
        }
    }

    /// Get API token from ctx, Windows Credential Manager, or region-bound env.
    fn get_api_token(
        api_key: Option<&str>,
        region: ZaiRegion,
        env: &settings::EnvMap,
    ) -> Result<String, ProviderError> {
        // Check ctx.api_key first (from settings)
        if let Some(key) = api_key
            && let Some(cleaned) = settings::cleaned(key)
        {
            return Ok(cleaned);
        }

        // Try Windows Credential Manager
        if let Ok(entry) = keyring::Entry::new(ZAI_CREDENTIAL_TARGET, "api_token")
            && let Ok(token) = entry.get_password()
        {
            return Ok(token);
        }

        let home = dirs::home_dir().unwrap_or_default();
        ZaiSettingsReader::api_token(env, &home, region).ok_or_else(|| {
            ProviderError::NotInstalled(match region {
                ZaiRegion::BigModelCn => "z.ai (BigModel CN) API token not found. Set in Preferences → Providers, Z_AI_API_KEY, BIGMODEL_API_KEY, ZHIPU_API_KEY, ZHIPUAI_API_KEY, or GLM_API_KEY.".to_string(),
                ZaiRegion::Global => "z.ai API token not found. Set in Preferences → Providers or Z_AI_API_KEY.".to_string(),
            })
        })
    }

    /// Validate endpoint overrides against the region *before* any bearer
    /// request (upstream #2621/#2623: canonical cross-region overrides are
    /// rejected pre-auth; custom relay hosts stay legal).
    fn validate_endpoint_overrides(
        env: &settings::EnvMap,
        region: ZaiRegion,
    ) -> Result<(), ProviderError> {
        ZaiSettingsReader::validate_endpoint_overrides(env, region)
            .map_err(|err| ProviderError::Other(err.to_string()))
    }

    /// Quota URL: `Z_AI_QUOTA_URL` full override → `Z_AI_API_HOST` host
    /// override → the selected region's canonical endpoint.
    fn quota_url(env: &settings::EnvMap, region: ZaiRegion) -> Result<Url, ProviderError> {
        let provider_err = |err: ZaiSettingsError| ProviderError::Other(err.to_string());
        let env_get = |key: &str| env.get(key).and_then(|raw| settings::cleaned(raw));
        if env_get(settings::ZAI_QUOTA_URL_ENV).is_some() {
            let url = ZaiSettingsReader::quota_url_override(env)
                .map_err(provider_err)?
                .expect("override present");
            return Ok(url);
        }
        if let Some(url) = ZaiSettingsReader::quota_url_from_api_host(env).map_err(provider_err)? {
            return Ok(url);
        }
        Ok(region.quota_limit_url())
    }

    fn request_url(
        env: &settings::EnvMap,
        region: ZaiRegion,
        team_context: Option<&ZaiTeamContext>,
    ) -> Result<Url, ProviderError> {
        let mut url = Self::quota_url(env, region)?;
        if team_context.is_some() {
            url.query_pairs_mut().append_pair("type", "2");
        }
        Ok(url)
    }

    fn team_context(ctx: &FetchContext) -> Result<Option<ZaiTeamContext>, ProviderError> {
        let explicit_scope = std::env::var(ZAI_USAGE_SCOPE_ENV)
            .ok()
            .and_then(|value| settings::cleaned(&value))
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
        let env = settings::process_env();
        let region = Self::effective_region(ctx, &env);
        // Canonical cross-region overrides are rejected before any bearer
        // token is sent (upstream #2621/#2623).
        Self::validate_endpoint_overrides(&env, region)?;
        let api_token = Self::get_api_token(ctx.api_key.as_deref(), region, &env)?;

        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        let team_context = Self::team_context(ctx)?;
        let request_url = Self::request_url(&env, region, team_context.as_ref())?;
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
        // Upstream 0.48.0 plan-name fallbacks: planName, plan, plan_type,
        // packageName, level — first non-empty trimmed wins.
        let plan_name = quota
            .data
            .as_ref()
            .and_then(|data| {
                [
                    data.plan_name.as_deref(),
                    data.plan.as_deref(),
                    data.plan_type.as_deref(),
                    data.package_name.as_deref(),
                    data.level.as_deref(),
                ]
                .into_iter()
                .filter_map(|raw| raw.map(str::trim))
                .find(|raw| !raw.is_empty())
            })
            .unwrap_or("z.ai");

        // Collect TOKENS_LIMIT entries (upstream uses "TOKENS_LIMIT", legacy uses "tokens")
        let is_tokens = |l: &&ZaiLimit| {
            matches!(
                l.limit_type.as_deref(),
                Some("TOKENS_LIMIT") | Some("tokens")
            )
        };
        let is_time =
            |l: &&ZaiLimit| matches!(l.limit_type.as_deref(), Some("TIME_LIMIT") | Some("mcp"));
        let mut token_limits: Vec<&ZaiLimit> = limits.iter().filter(is_tokens).collect();
        // Upstream ordering: ascending window minutes, unknown windows last.
        token_limits.sort_by_key(|l| Self::window_minutes(l).unwrap_or(u32::MAX));
        let time_limit = limits.iter().find(is_time);

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

        // Upstream 0.48.0 `rateWindow`/`resetDescription`: only token-type
        // windows keep duration minutes; TIME_LIMIT (MCP) carries the "MCP"
        // label and no window duration; 5-hour token windows are labeled
        // "5-hour"; otherwise the explicit window label is used.
        fn make_window(l: &ZaiLimit) -> RateWindow {
            let resets_at = l
                .next_reset_time
                .and_then(DateTime::<Utc>::from_timestamp_millis)
                .or_else(|| {
                    l.reset_at
                        .as_deref()
                        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                        .map(|timestamp| timestamp.with_timezone(&Utc))
                });
            let is_tokens = matches!(
                l.limit_type.as_deref(),
                Some("TOKENS_LIMIT") | Some("tokens")
            );
            let window_mins = if is_tokens {
                ZaiProvider::window_minutes(l)
            } else {
                None
            };
            RateWindow::with_details(
                compute_percent(l),
                window_mins,
                resets_at,
                rate_window_reset_description(l, window_mins),
            )
        }

        // Upstream 0.48.0 bucket split: with 2+ token limits, the shortest
        // window → session (5-hour GLM Coding Plan window) and the longest →
        // weekly token quota; with one token limit it stands alone; with none
        // the MCP (time) limit is the primary.
        let (token_limit, session_token_limit) = match token_limits.as_slice() {
            [] => (None, None),
            [single] => (Some(*single), None),
            _ => (token_limits.last().copied(), token_limits.first().copied()),
        };
        let primary_limit = session_token_limit.or(token_limit).or(time_limit);
        let secondary_limit = if session_token_limit.is_some() {
            token_limit
        } else {
            None
        };

        let primary = primary_limit
            .map(make_window)
            .unwrap_or_else(|| RateWindow::new(0.0));
        let mut usage = UsageSnapshot::new(primary).with_login_method(plan_name);
        if let Some(secondary) = secondary_limit {
            usage = usage.with_secondary(make_window(secondary));
        }
        // MCP usage is a separate named window whenever a coding-limit
        // primary exists; with no token limits it already owns the primary.
        if (token_limit.is_some() || session_token_limit.is_some())
            && let Some(mcp) = time_limit
        {
            usage = usage.with_extra_rate_window("zai-mcp", "MCP", make_window(mcp));
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

/// Upstream 0.48.0 `resetDescription`: MCP (TIME_LIMIT) → "MCP"; 5-hour
/// token window → "5-hour"; else the explicit window label, if any.
fn rate_window_reset_description(l: &ZaiLimit, window_mins: Option<u32>) -> Option<String> {
    if matches!(l.limit_type.as_deref(), Some("TIME_LIMIT") | Some("mcp")) {
        return Some("MCP".to_string());
    }
    if matches!(
        l.limit_type.as_deref(),
        Some("TOKENS_LIMIT") | Some("tokens")
    ) && window_mins == Some(300)
    {
        return Some("5-hour".to_string());
    }
    window_label(l)
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
            .and_then(|value| settings::cleaned(&value))?;
        let project_id = std::env::var(ZAI_BIGMODEL_PROJECT_ENV)
            .ok()
            .and_then(|value| settings::cleaned(&value))?;
        Some(Self {
            organization_id,
            project_id,
        })
    }
}

fn parse_team_context_pair(raw: &str) -> Option<ZaiTeamContext> {
    let (organization_id, project_id) = raw
        .split_once('|')
        .or_else(|| raw.split_once(','))
        .or_else(|| raw.split_once(';'))?;
    Some(ZaiTeamContext {
        organization_id: settings::cleaned(organization_id)?,
        project_id: settings::cleaned(project_id)?,
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
    use settings::EnvMap;
    use std::collections::HashMap;

    fn env_map(pairs: &[(&str, &str)]) -> EnvMap {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect::<HashMap<_, _>>()
    }

    #[test]
    fn request_url_adds_team_type_query_for_team_context() {
        let env = env_map(&[]);
        let team = ZaiTeamContext {
            organization_id: "org".to_string(),
            project_id: "project".to_string(),
        };

        let url = ZaiProvider::request_url(&env, ZaiRegion::Global, Some(&team)).expect("url");

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
        let env = env_map(&[]);
        let region = ZaiProvider::effective_region(&ctx, &env);
        assert_eq!(region, ZaiRegion::BigModelCn);

        let url = ZaiProvider::quota_url(&env, region).expect("url");

        assert_eq!(
            url.as_str(),
            "https://open.bigmodel.cn/api/monitor/usage/quota/limit"
        );
    }

    #[test]
    fn global_region_defaults_to_api_z_ai() {
        let env = env_map(&[]);

        let url = ZaiProvider::quota_url(&env, ZaiRegion::Global).expect("url");

        assert_eq!(
            url.as_str(),
            "https://api.z.ai/api/monitor/usage/quota/limit"
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
    fn time_limit_primary_carries_mcp_label_without_duration() {
        // Upstream 0.48.0: TIME_LIMIT (MCP) windows no longer keep explicit
        // duration minutes and label as "MCP", not the old monthly sentinel.
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
        assert_eq!(usage.primary.window_minutes, None);
        assert_eq!(usage.primary.reset_description.as_deref(), Some("MCP"));
        assert!(usage.primary.resets_at.is_some());
    }

    #[test]
    fn bare_time_limit_primary_has_no_window_duration() {
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
        assert_eq!(usage.primary.window_minutes, None);
        assert_eq!(usage.primary.reset_description.as_deref(), Some("MCP"));
    }

    #[test]
    fn mcp_limit_renders_separate_named_window() {
        // Upstream 0.48.0 GLM Coding Plan layout: coding-limit primary +
        // MCP as a named extra window; MCP 1-minute marker no longer maps
        // to a monthly sentinel secondary.
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

        assert_eq!(usage.primary.window_minutes, Some(10080));
        assert_eq!(
            usage.primary.reset_description.as_deref(),
            Some("1 week window")
        );
        assert!(usage.secondary.is_none());
        let mcp = usage
            .extra_rate_windows
            .iter()
            .find(|window| window.id == "zai-mcp")
            .expect("MCP extra window");
        assert_eq!(mcp.title, "MCP");
        assert_eq!(mcp.window.window_minutes, None);
        assert_eq!(mcp.window.reset_description.as_deref(), Some("MCP"));
        assert_eq!(mcp.window.used_percent, 10.0);
    }

    #[test]
    fn session_five_hour_window_becomes_primary_over_weekly() {
        // Upstream 0.48.0 GLM Coding Plan: 2+ TOKENS_LIMIT entries →
        // shortest (5-hour) window primary, longest (weekly) secondary.
        let provider = ZaiProvider::new();
        let quota: ZaiQuotaResponse = serde_json::from_value(serde_json::json!({
            "code": 200,
            "data": {
                "limits": [
                    {
                        "type": "TOKENS_LIMIT",
                        "unit": 3,
                        "number": 5,
                        "percentage": 55,
                        "nextResetTime": 1770648402389_i64
                    },
                    {
                        "type": "TOKENS_LIMIT",
                        "unit": 6,
                        "number": 1,
                        "percentage": 34
                    }
                ]
            }
        }))
        .unwrap();
        let usage = provider.parse_quota_response(&quota).unwrap();

        assert_eq!(usage.primary.used_percent, 55.0);
        assert_eq!(usage.primary.window_minutes, Some(300));
        assert_eq!(usage.primary.reset_description.as_deref(), Some("5-hour"));
        assert!(usage.primary.resets_at.is_some());
        let secondary = usage.secondary.expect("weekly secondary");
        assert_eq!(secondary.used_percent, 34.0);
        assert_eq!(secondary.window_minutes, Some(10080));
        assert!(usage.model_specific.is_none());
    }

    #[test]
    fn plan_name_falls_back_to_level_key() {
        let provider = ZaiProvider::new();
        let quota: ZaiQuotaResponse = serde_json::from_value(serde_json::json!({
            "code": 200,
            "data": {
                "level": "GLM Coding Plan",
                "limits": []
            }
        }))
        .unwrap();
        let usage = provider.parse_quota_response(&quota).unwrap();
        assert_eq!(usage.login_method.as_deref(), Some("GLM Coding Plan"));

        for key in ["plan", "plan_type", "packageName"] {
            let quota: ZaiQuotaResponse = serde_json::from_value(serde_json::json!({
                "code": 200,
                "data": { key: "Coding Plan", "limits": [] }
            }))
            .unwrap();
            let usage = provider.parse_quota_response(&quota).unwrap();
            assert_eq!(usage.login_method.as_deref(), Some("Coding Plan"), "{key}");
        }
    }

    #[test]
    fn empty_plan_fields_fall_back_to_default() {
        let provider = ZaiProvider::new();
        let quota: ZaiQuotaResponse = serde_json::from_value(serde_json::json!({
            "code": 200,
            "data": { "planName": "  ", "level": "", "limits": [] }
        }))
        .unwrap();
        let usage = provider.parse_quota_response(&quota).unwrap();
        assert_eq!(usage.login_method.as_deref(), Some("z.ai"));
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
