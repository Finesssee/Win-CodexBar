//! OpenRouter provider implementation
//!
//! Fetches credit balance and usage data from OpenRouter's REST API
//! Requires API key for authentication

mod activity;

use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;

use crate::core::{
    CostSnapshot, FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId,
    ProviderMetadata, RateWindow, SourceMode, UsageSnapshot,
};

/// OpenRouter API base URL — the bare `/api/v1` prefix, matching upstream
/// (steipete/CodexBar `OpenRouterSettingsReader.apiURL`).
///
/// Both endpoints append their path to this base: `/credits` and `/key`.
/// The fork's original bug baked `/auth` into the base (`.../api/v1/auth`),
/// which turned the credits call into `/api/v1/auth/credits` -> 404.
const OPENROUTER_API_BASE: &str = "https://openrouter.ai/api/v1";
const OPENROUTER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Optional key-quota enrichment joins on a one-second fast deadline
/// (upstream 0.49.0 #2778) so a slow `/key` endpoint can never stall the
/// refresh; degraded enrichment is logged and skipped, never fatal.
const OPENROUTER_KEY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);
const OPENROUTER_ACTIVITY_URL: &str = "https://openrouter.ai/api/v1/activity";
const OPENROUTER_MANAGEMENT_ENV: &str = "OPENROUTER_MANAGEMENT_API_KEY";

/// Windows Credential Manager target for OpenRouter API token
const OPENROUTER_CREDENTIAL_TARGET: &str = "codexbar-openrouter";

/// OpenRouter /credits response
#[derive(Debug, Deserialize)]
struct CreditsResponse {
    data: CreditsData,
}

#[derive(Debug, Deserialize)]
struct CreditsData {
    total_credits: f64,
    total_usage: f64,
}

impl CreditsData {
    fn balance(&self) -> f64 {
        (self.total_credits - self.total_usage).max(0.0)
    }

    fn used_percent(&self) -> f64 {
        if self.total_credits > 0.0 {
            ((self.total_usage / self.total_credits) * 100.0).min(100.0)
        } else {
            0.0
        }
    }
}

/// OpenRouter /key response
#[derive(Debug, Deserialize)]
struct KeyResponse {
    data: KeyData,
}

#[derive(Debug, Deserialize)]
struct KeyData {
    limit: Option<f64>,
    /// Server-reported current-period remaining for the key limit
    /// (upstream 0.48.0 F14: `limit_remaining`).
    limit_remaining: Option<f64>,
    /// Declared reset window for the key limit, e.g. `"monthly"`
    /// (`limit_reset`); picks which period usage field is the quota fallback.
    limit_reset: Option<String>,
    usage: Option<f64>,
    usage_daily: Option<f64>,
    usage_weekly: Option<f64>,
    usage_monthly: Option<f64>,
    rate_limit: Option<RateLimitInfo>,
}

#[derive(Debug, Deserialize)]
struct RateLimitInfo {
    requests: Option<i64>,
    interval: Option<String>,
}

/// OpenRouter provider
pub struct OpenRouterProvider {
    metadata: ProviderMetadata,
}

/// Usage value for quota math when the server does not report remaining: the
/// field matching the declared reset window when known, otherwise cumulative
/// usage (upstream `OpenRouterUsageSnapshot.quotaFallbackUsage`).
fn quota_fallback_usage(key_data: &KeyData) -> Option<f64> {
    let reset_usage = match key_data
        .limit_reset
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("daily") => key_data.usage_daily,
        Some("weekly") => key_data.usage_weekly,
        Some("monthly") => key_data.usage_monthly,
        _ => None,
    };
    reset_usage.or(key_data.usage)
}

impl OpenRouterProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::OpenRouter,
                display_name: "OpenRouter",
                session_label: "Credits",
                weekly_label: "API key limit",
                supports_opus: false,
                supports_credits: true,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://openrouter.ai/settings/credits"),
                status_page_url: Some("https://status.openrouter.ai"),
            },
        }
    }

    /// Get API token from ctx, Windows Credential Manager, or env
    fn get_api_token(api_key: Option<&str>) -> Result<String, ProviderError> {
        if let Some(key) = api_key
            && !key.is_empty()
        {
            return Ok(key.to_string());
        }

        match keyring::Entry::new(OPENROUTER_CREDENTIAL_TARGET, "api_token") {
            Ok(entry) => match entry.get_password() {
                Ok(token) => Ok(token),
                Err(_) => std::env::var("OPENROUTER_API_KEY").map_err(|_| {
                    ProviderError::NotInstalled(
                        "OpenRouter API key not found. Set in Preferences → Providers or OPENROUTER_API_KEY environment variable.".to_string(),
                    )
                }),
            },
            Err(_) => std::env::var("OPENROUTER_API_KEY").map_err(|_| {
                ProviderError::NotInstalled(
                    "OpenRouter API key not found. Set in Preferences → Providers or OPENROUTER_API_KEY environment variable.".to_string(),
                )
            }),
        }
    }

    /// Fetch usage from OpenRouter API. Management Activity spend is optional
    /// enrichment: a missing/denied management key never discards credits/quota.
    async fn fetch_usage_api(
        &self,
        ctx: &FetchContext,
    ) -> Result<(UsageSnapshot, Option<CostSnapshot>), ProviderError> {
        let api_key = Self::get_api_token(ctx.api_key.as_deref())?;
        let client = Self::build_client(OPENROUTER_TIMEOUT)?;
        let credits = Self::fetch_credits(&client, &api_key).await?;
        let mut usage = Self::build_credits_usage(&credits.data);

        if let Some(key_data) = Self::fetch_key_data(&api_key).await? {
            Self::enrich_usage_with_key_data(&mut usage, key_data);
        }

        let management_key = crate::settings::Settings::load()
            .management_api_token(ProviderId::OpenRouter)
            .map(str::to_string)
            .or_else(|| {
                std::env::var(OPENROUTER_MANAGEMENT_ENV)
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
            });
        let cost = match management_key.as_deref() {
            Some(key) => match Self::fetch_activity_cost(key).await {
                Ok(cost) => Some(cost),
                Err(error) => {
                    tracing::debug!(%error, "OpenRouter management Activity degraded; preserving credits/quota");
                    None
                }
            },
            None => None,
        };

        Ok((usage, cost))
    }

    async fn fetch_activity_cost(management_key: &str) -> Result<CostSnapshot, ProviderError> {
        let client = Self::build_client(OPENROUTER_KEY_TIMEOUT)?;
        let now = Utc::now();
        let latest_completed = (now.date_naive() - chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        let history = Self::fetch_activity_payload(&client, management_key, None).await?;
        let latest_completed_payload =
            Self::fetch_activity_payload(&client, management_key, Some(&latest_completed)).await?;
        activity::parse_activity_cost(&[history, latest_completed_payload], now)
    }

    async fn fetch_activity_payload(
        client: &reqwest::Client,
        management_key: &str,
        date: Option<&str>,
    ) -> Result<Value, ProviderError> {
        let mut request = client
            .get(OPENROUTER_ACTIVITY_URL)
            .header("Authorization", format!("Bearer {management_key}"))
            .header("Accept", "application/json");
        if let Some(date) = date {
            request = request.query(&[("date", date)]);
        }
        let response = request.send().await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            || response.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(ProviderError::AuthRequired);
        }
        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "OpenRouter Activity returned status {}",
                response.status()
            )));
        }
        response.json::<Value>().await.map_err(|error| {
            ProviderError::Parse(format!("Invalid OpenRouter Activity response: {error}"))
        })
    }

    fn build_client(timeout: std::time::Duration) -> Result<reqwest::Client, ProviderError> {
        crate::core::credentialed_http_client_builder()
            .timeout(timeout)
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))
    }

    async fn fetch_credits(
        client: &reqwest::Client,
        api_key: &str,
    ) -> Result<CreditsResponse, ProviderError> {
        let credits_url = format!("{}/credits", OPENROUTER_API_BASE);
        let resp = client
            .get(&credits_url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Accept", "application/json")
            .send()
            .await?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ProviderError::AuthRequired);
        }

        if !resp.status().is_success() {
            return Err(ProviderError::Other(format!(
                "OpenRouter API returned status {}",
                resp.status()
            )));
        }

        resp.json()
            .await
            .map_err(|e| ProviderError::Parse(format!("Failed to parse credits response: {}", e)))
    }

    fn build_credits_usage(credits: &CreditsData) -> UsageSnapshot {
        let balance = credits.balance();
        let mut primary = RateWindow::new(credits.used_percent());
        primary.reset_description = Some(format!("${:.2} remaining", balance));

        UsageSnapshot::new(primary).with_login_method(format!("${:.2} balance", balance))
    }

    async fn fetch_key_data(api_key: &str) -> Result<Option<KeyData>, ProviderError> {
        let key_client = Self::build_client(OPENROUTER_KEY_TIMEOUT)?;
        let key_resp = match Self::send_key_request(&key_client, api_key).await {
            Ok(resp) => resp,
            // Upstream 0.49.0 #2778: make the degraded fast join explicit —
            // core usage stays authoritative, only the optional key meter is
            // dropped.
            Err(err) => {
                tracing::debug!(
                    error = %err,
                    "OpenRouter key-quota fast join degraded; continuing without key meter"
                );
                return Ok(None);
            }
        };

        if !key_resp.status().is_success() {
            tracing::debug!(
                status = %key_resp.status(),
                "OpenRouter key-quota fast join degraded; continuing without key meter"
            );
            return Ok(None);
        }

        Ok(key_resp
            .json::<KeyResponse>()
            .await
            .map(|key_response| key_response.data)
            .ok())
    }

    async fn send_key_request(
        client: &reqwest::Client,
        api_key: &str,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let key_url = format!("{}/key", OPENROUTER_API_BASE);
        client
            .get(&key_url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Accept", "application/json")
            .send()
            .await
    }

    fn enrich_usage_with_key_data(usage: &mut UsageSnapshot, key_data: KeyData) {
        Self::add_key_quota(usage, &key_data);
        Self::add_spend_window(
            usage,
            key_data.usage_daily,
            "daily-spend",
            "Daily spend",
            "today",
        );
        Self::add_spend_window(
            usage,
            key_data.usage_weekly,
            "weekly-spend",
            "Weekly spend",
            "this week",
        );
        Self::add_spend_window(
            usage,
            key_data.usage_monthly,
            "monthly-spend",
            "Monthly spend",
            "this month",
        );
    }

    /// Key-limit meter derivation (upstream 0.48.0 #2612): prefer the
    /// server-reported current-period remaining (`limit_remaining`), clamped to
    /// [0, limit] so an overspent key reads 100% and an above-limit reading
    /// reads 0%. Without it, fall back to the period usage matching the
    /// declared reset window, then cumulative usage; with no usable source the
    /// meter stays hidden.
    fn add_key_quota(usage: &mut UsageSnapshot, key_data: &KeyData) {
        let Some(limit) = key_data.limit else {
            return;
        };
        if limit <= 0.0 || !limit.is_finite() {
            return;
        }

        let used = if let Some(remaining) = key_data.limit_remaining {
            if !remaining.is_finite() {
                return;
            }
            limit - remaining.clamp(0.0, limit)
        } else {
            let Some(fallback) = quota_fallback_usage(key_data) else {
                return;
            };
            if fallback < 0.0 || !fallback.is_finite() {
                return;
            }
            fallback
        };

        let key_percent = ((used / limit) * 100.0).clamp(0.0, 100.0);
        let mut key_window = RateWindow::new(key_percent);
        key_window.reset_description = Some(format!(
            "${used:.2}/${limit:.2} spending cap · Spending cap, not balance"
        ));
        *usage = usage.clone().with_secondary(key_window);
    }

    fn add_spend_window(
        usage: &mut UsageSnapshot,
        value: Option<f64>,
        id: &'static str,
        label: &'static str,
        period: &'static str,
    ) {
        let Some(spend) = value else {
            return;
        };

        let mut window = RateWindow::new(0.0);
        window.reset_description = Some(format!("${spend:.2} {period}"));
        *usage = usage.clone().with_extra_rate_window(id, label, window);
    }
}

impl Default for OpenRouterProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for OpenRouterProvider {
    fn id(&self) -> ProviderId {
        ProviderId::OpenRouter
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Fetching OpenRouter usage");

        match ctx.source_mode {
            SourceMode::Auto | SourceMode::OAuth => {
                let (usage, cost) = self.fetch_usage_api(ctx).await?;
                let mut result = ProviderFetchResult::new(usage, "api");
                if let Some(cost) = cost {
                    result = result.with_cost(cost);
                }
                Ok(result)
            }
            SourceMode::Web | SourceMode::Cli => {
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

    // Regression guard for the `/auth/credits` 404 bug: the base must be the
    // bare `/api/v1` prefix. Credits and key live on DIFFERENT subpaths, so a
    // base that bakes in `/auth` (or anything else) silently breaks one of them.
    #[test]
    fn api_base_is_bare_v1_prefix() {
        assert_eq!(OPENROUTER_API_BASE, "https://openrouter.ai/api/v1");
    }

    // Credits endpoint: `/api/v1/credits` (verified HTTP 200 against live API).
    // The old base `.../api/v1/auth` produced `/api/v1/auth/credits` -> 404.
    #[test]
    fn credits_url_resolves_to_canonical_path() {
        let url = format!("{}/credits", OPENROUTER_API_BASE);
        assert_eq!(url, "https://openrouter.ai/api/v1/credits");
    }

    // Key introspection endpoint: `/api/v1/key` (verified HTTP 200), matching
    // upstream's `{base}/key` append. (OpenRouter also aliases `/auth/key`, but
    // we mirror upstream's canonical path.)
    #[test]
    fn key_url_resolves_to_canonical_path() {
        let url = format!("{}/key", OPENROUTER_API_BASE);
        assert_eq!(url, "https://openrouter.ai/api/v1/key");
    }

    // ── F14: server-reported current-period remaining drives the key meter ──

    fn key_data(
        limit: Option<f64>,
        remaining: Option<f64>,
        reset: Option<&str>,
        usage: Option<f64>,
        daily: Option<f64>,
        weekly: Option<f64>,
        monthly: Option<f64>,
    ) -> KeyData {
        KeyData {
            limit,
            limit_remaining: remaining,
            limit_reset: reset.map(str::to_string),
            usage,
            usage_daily: daily,
            usage_weekly: weekly,
            usage_monthly: monthly,
            rate_limit: None,
        }
    }

    fn key_quota_percent(key_data: KeyData) -> Option<f64> {
        let mut usage = UsageSnapshot::new(RateWindow::new(0.0));
        OpenRouterProvider::add_key_quota(&mut usage, &key_data);
        usage.secondary.map(|window| window.used_percent)
    }

    #[test]
    fn key_limit_copy_stays_distinct_from_account_balance() {
        let provider = OpenRouterProvider::new();
        assert_eq!(provider.metadata.weekly_label, "API key limit");

        let credits = CreditsData {
            total_credits: 5.0,
            total_usage: 3.1,
        };
        let mut usage = OpenRouterProvider::build_credits_usage(&credits);
        OpenRouterProvider::add_key_quota(
            &mut usage,
            &key_data(
                Some(30.0),
                Some(30.0),
                Some("monthly"),
                Some(0.0),
                None,
                None,
                Some(0.0),
            ),
        );
        assert_eq!(usage.login_method.as_deref(), Some("$1.90 balance"));
        let key = usage.secondary.expect("key spending cap");
        assert_eq!(key.used_percent, 0.0);
        assert_eq!(
            key.reset_description.as_deref(),
            Some("$0.00/$30.00 spending cap · Spending cap, not balance")
        );
    }
    #[test]
    fn server_remaining_replaces_lifetime_usage_for_meter() {
        // limit 50, server says 12.50 left this period → 75% used, even though
        // cumulative lifetime usage would imply a different ratio.
        let pct = key_quota_percent(key_data(
            Some(50.0),
            Some(12.5),
            None,
            Some(40.0),
            None,
            None,
            None,
        ));
        assert_eq!(pct, Some(75.0));
    }

    #[test]
    fn negative_server_remaining_reads_exhausted() {
        // Upstream: "treat negative remaining as exhausted quota".
        let pct = key_quota_percent(key_data(
            Some(50.0),
            Some(-3.0),
            None,
            Some(10.0),
            None,
            None,
            None,
        ));
        assert_eq!(pct, Some(100.0));
    }

    #[test]
    fn above_limit_server_remaining_reads_zero() {
        // Inclusive [0, keyLimit] clamp: a server remaining above the
        // configured limit renders 0% used, not a suppressed meter.
        let pct = key_quota_percent(key_data(
            Some(50.0),
            Some(75.0),
            None,
            Some(10.0),
            None,
            None,
            None,
        ));
        assert_eq!(pct, Some(0.0));
    }

    #[test]
    fn reset_window_usage_is_the_preferred_fallback() {
        // No remaining: `limit_reset: "monthly"` picks usage_monthly (25/50).
        let pct = key_quota_percent(key_data(
            Some(50.0),
            None,
            Some("monthly"),
            Some(40.0),
            Some(1.0),
            Some(2.0),
            Some(25.0),
        ));
        assert_eq!(pct, Some(50.0));
        // Case-insensitive reset label.
        let pct = key_quota_percent(key_data(
            Some(50.0),
            None,
            Some("WEEKLY"),
            Some(40.0),
            Some(1.0),
            Some(2.0),
            Some(25.0),
        ));
        assert_eq!(pct, Some(4.0));
    }

    #[test]
    fn cumulative_usage_is_the_last_fallback() {
        let pct = key_quota_percent(key_data(
            Some(50.0),
            None,
            None,
            Some(20.0),
            Some(1.0),
            None,
            None,
        ));
        assert_eq!(pct, Some(40.0));
    }

    #[test]
    fn no_usable_quota_source_hides_the_meter() {
        assert_eq!(
            key_quota_percent(key_data(Some(50.0), None, None, None, None, None, None)),
            None
        );
        assert_eq!(
            key_quota_percent(key_data(
                Some(0.0),
                Some(5.0),
                None,
                Some(1.0),
                None,
                None,
                None
            )),
            None
        );
        assert_eq!(
            key_quota_percent(key_data(None, Some(5.0), None, Some(1.0), None, None, None)),
            None
        );
    }

    #[test]
    fn parsed_key_wire_fields_decode() {
        let parsed: KeyResponse = serde_json::from_str(
            r#"{"data":{"limit":50,"limit_remaining":12.5,"limit_reset":"monthly","usage":40,"usage_monthly":25}}"#,
        )
        .unwrap();
        assert_eq!(parsed.data.limit, Some(50.0));
        assert_eq!(parsed.data.limit_remaining, Some(12.5));
        assert_eq!(parsed.data.limit_reset.as_deref(), Some("monthly"));
    }
}
