//! Crof provider implementation.
//!
//! Fetches API key based credit/request quota data from Crof.

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};

const CROF_USAGE_URL: &str = "https://crof.ai/usage_api/";
const CROF_CREDENTIAL_TARGET: &str = "codexbar-crof";
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";

#[derive(Debug, Deserialize)]
struct CrofUsageResponse {
    credits: f64,
    #[serde(default, rename = "requests_plan")]
    requests_plan: Option<f64>,
    #[serde(default, rename = "usable_requests")]
    usable_requests: Option<f64>,
}

pub struct CrofProvider {
    metadata: ProviderMetadata,
    client: Client,
}

impl CrofProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Crof,
                display_name: "Crof",
                session_label: "Balance",
                weekly_label: "Requests",
                supports_opus: false,
                supports_credits: true,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://crof.ai"),
                status_page_url: None,
            },
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    fn api_key(api_key: Option<&str>) -> Result<String, ProviderError> {
        super_key(
            api_key,
            CROF_CREDENTIAL_TARGET,
            &["CROF_API_KEY", "CROFAI_API_KEY"],
        )
    }

    async fn fetch_api(&self, api_key: &str) -> Result<UsageSnapshot, ProviderError> {
        let response = self
            .client
            .get(CROF_USAGE_URL)
            .bearer_auth(api_key)
            .header("Accept", "application/json")
            .header("User-Agent", BROWSER_USER_AGENT)
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ProviderError::AuthRequired);
        }
        if status == reqwest::StatusCode::FORBIDDEN {
            if body.contains("cloudflare") || body.contains("Error 1010") {
                return Err(ProviderError::Other(
                    "Crof usage API blocked by Cloudflare (1010). Retry from the desktop app."
                        .into(),
                ));
            }
            return Err(ProviderError::AuthRequired);
        }
        if !status.is_success() {
            return Err(ProviderError::Other(format!(
                "Crof API returned status {status}"
            )));
        }

        let usage: CrofUsageResponse = serde_json::from_str(&body)
            .map_err(|e| ProviderError::Parse(format!("Failed to parse Crof usage: {e}")))?;
        Ok(snapshot_from_usage(&usage))
    }
}

fn snapshot_from_usage(usage: &CrofUsageResponse) -> UsageSnapshot {
    let credits = usage.credits.max(0.0);
    let display = if credits <= 0.0 {
        "$0.00".to_string()
    } else if credits >= 0.01 {
        format!("${:.2}", (credits * 100.0).floor() / 100.0)
    } else {
        format!("${credits:.4}")
    };
    let mut primary = RateWindow::new(if credits > 0.0 { 0.0 } else { 100.0 });
    primary.reset_description = Some(display.clone());

    let mut snapshot = UsageSnapshot::new(primary).with_login_method(format!("{display} balance"));

    if let (Some(plan), Some(usable)) = (usage.requests_plan, usage.usable_requests) {
        let remaining = usable.max(0.0).min(plan.max(0.0));
        let remaining_percent = if plan > 0.0 {
            ((remaining / plan) * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };
        let mut requests = RateWindow::new(100.0 - remaining_percent);
        requests.reset_description = Some(format!("{remaining:.0} requests left"));
        snapshot = snapshot.with_secondary(requests);
    }

    snapshot
}

impl Default for CrofProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for CrofProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Crof
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto | SourceMode::OAuth => {
                let api_key = Self::api_key(ctx.api_key.as_deref())?;
                Ok(ProviderFetchResult::new(
                    self.fetch_api(&api_key).await?,
                    "api",
                ))
            }
            SourceMode::Web | SourceMode::Cli => {
                Err(ProviderError::UnsupportedSource(ctx.source_mode))
            }
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::OAuth]
    }
}

fn super_key(
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

    #[test]
    fn crof_snapshot_formats_request_and_credit_windows() {
        let snapshot = snapshot_from_usage(&CrofUsageResponse {
            credits: 12.5,
            requests_plan: Some(100.0),
            usable_requests: Some(25.0),
        });
        assert_eq!(snapshot.primary.used_percent, 0.0);
        assert_eq!(
            snapshot.primary.reset_description.as_deref(),
            Some("$12.50")
        );
        assert_eq!(snapshot.secondary.unwrap().used_percent, 75.0);
    }

    #[test]
    fn crof_payg_balance_only_does_not_require_request_quota() {
        let snapshot = snapshot_from_usage(&CrofUsageResponse {
            credits: 3.019,
            requests_plan: None,
            usable_requests: None,
        });
        assert_eq!(snapshot.primary.used_percent, 0.0);
        assert_eq!(snapshot.primary.reset_description.as_deref(), Some("$3.01"));
        assert!(snapshot.secondary.is_none());
        assert_eq!(snapshot.login_method.as_deref(), Some("$3.01 balance"));
    }

    #[test]
    fn crof_sub_cent_balance_is_not_exhausted() {
        let snapshot = snapshot_from_usage(&CrofUsageResponse {
            credits: 0.0073,
            requests_plan: None,
            usable_requests: None,
        });
        assert_eq!(snapshot.primary.used_percent, 0.0);
        assert_eq!(
            snapshot.primary.reset_description.as_deref(),
            Some("$0.0073")
        );
    }

    #[test]
    fn crof_zero_balance_is_exhausted() {
        let snapshot = snapshot_from_usage(&CrofUsageResponse {
            credits: 0.0,
            requests_plan: None,
            usable_requests: None,
        });
        assert_eq!(snapshot.primary.used_percent, 100.0);
        assert_eq!(snapshot.primary.reset_description.as_deref(), Some("$0.00"));
    }
}
