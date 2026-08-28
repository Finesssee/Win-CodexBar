//! ZenMux Management API usage provider (upstream 0.44).
//!
//! - `GET https://zenmux.ai/api/v1/management/subscription/detail`
//! - Optional PAYG: `GET .../payg/balance`

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;

use crate::core::{
    CostSnapshot, FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId,
    ProviderMetadata, RateWindow, SourceMode, UsageSnapshot,
};

const MANAGEMENT_BASE: &str = "https://zenmux.ai/api/v1/management";
const CREDENTIAL_TARGET: &str = "codexbar-zenmux";
const ENV_KEYS: &[&str] = &["ZENMUX_MANAGEMENT_API_KEY", "ZENMUX_API_KEY"];

#[derive(Debug, Deserialize)]
struct SubscriptionEnvelope {
    success: bool,
    data: SubscriptionData,
}

#[derive(Debug, Deserialize)]
struct SubscriptionData {
    plan: PlanInfo,
    #[serde(default)]
    account_status: String,
    quota_5_hour: QuotaInfo,
    quota_7_day: QuotaInfo,
}

#[derive(Debug, Deserialize)]
struct PlanInfo {
    #[serde(default)]
    tier: String,
    expires_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QuotaInfo {
    usage_percentage: f64,
    resets_at: Option<String>,
    max_flows: f64,
    used_flows: f64,
    #[allow(
        dead_code,
        reason = "field mirrors the ZenMux API payload; deserialized for round-trip fidelity but not read yet"
    )]
    remaining_flows: f64,
}

#[derive(Debug, Deserialize)]
struct BalanceEnvelope {
    success: bool,
    data: BalanceData,
}

#[derive(Debug, Deserialize)]
struct BalanceData {
    currency: String,
    total_credits: f64,
}

pub struct ZenMuxProvider {
    metadata: ProviderMetadata,
    client: Client,
}

impl ZenMuxProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::ZenMux,
                display_name: "ZenMux",
                session_label: "5-hour quota",
                weekly_label: "Weekly quota",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://zenmux.ai/platform/management"),
                status_page_url: None,
            },
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    fn resolve_key(api_key: Option<&str>) -> Result<String, ProviderError> {
        crate::providers::resolve_api_key(api_key, CREDENTIAL_TARGET, ENV_KEYS)
    }

    async fn fetch_json(&self, path: &str, key: &str) -> Result<serde_json::Value, ProviderError> {
        let url = format!("{MANAGEMENT_BASE}/{path}");
        let resp = self
            .client
            .get(&url)
            .bearer_auth(key)
            .header("Accept", "application/json")
            .send()
            .await?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ProviderError::AuthRequired);
        }
        if !status.is_success() {
            return Err(ProviderError::Other(format!(
                "ZenMux Management API returned HTTP {status}"
            )));
        }
        resp.json()
            .await
            .map_err(|e| ProviderError::Parse(format!("Failed to parse ZenMux response: {e}")))
    }
}

impl Default for ZenMuxProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for ZenMuxProvider {
    fn id(&self) -> ProviderId {
        ProviderId::ZenMux
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto | SourceMode::OAuth => {
                let key = Self::resolve_key(ctx.api_key.as_deref())?;
                let sub_val = self.fetch_json("subscription/detail", &key).await?;
                let snapshot = snapshot_from_subscription(&sub_val)?;

                let mut result = ProviderFetchResult::new(snapshot, "api");
                // PAYG balance is optional; failures do not fail the whole fetch.
                if let Ok(bal_val) = self.fetch_json("payg/balance", &key).await
                    && let Ok(cost) = payg_cost_from_balance(&bal_val)
                {
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
}

fn parse_iso(raw: Option<&str>) -> Option<DateTime<Utc>> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn quota_window(q: &QuotaInfo, minutes: u32) -> RateWindow {
    let used = (q.usage_percentage * 100.0).clamp(0.0, 100.0);
    let mut w = RateWindow::new(used);
    w.window_minutes = Some(minutes);
    w.resets_at = parse_iso(q.resets_at.as_deref());
    w.reset_description = Some(format!(
        "{} / {} flows",
        format_amount(q.used_flows),
        format_amount(q.max_flows)
    ));
    w
}

fn format_amount(value: f64) -> String {
    if (value - value.round()).abs() < f64::EPSILON {
        format!("{:.0}", value)
    } else {
        format!("{:.2}", value)
    }
}

fn snapshot_from_subscription(value: &serde_json::Value) -> Result<UsageSnapshot, ProviderError> {
    let env: SubscriptionEnvelope = serde_json::from_value(value.clone())
        .map_err(|e| ProviderError::Parse(format!("Failed to parse ZenMux subscription: {e}")))?;
    if !env.success {
        return Err(ProviderError::Parse(
            "ZenMux subscription response reported failure".into(),
        ));
    }
    let plan = env.data.plan.tier.trim();
    let status = env.data.account_status.trim();
    let login = if status.eq_ignore_ascii_case("healthy") || status.is_empty() {
        if plan.is_empty() {
            None
        } else {
            Some(format!("{} plan", capitalize(plan)))
        }
    } else if plan.is_empty() {
        Some(capitalize(status))
    } else {
        Some(format!(
            "{} plan · {}",
            capitalize(plan),
            capitalize(status)
        ))
    };

    let mut snap = UsageSnapshot::new(quota_window(&env.data.quota_5_hour, 5 * 60))
        .with_secondary(quota_window(&env.data.quota_7_day, 7 * 24 * 60));
    if let Some(login) = login {
        snap = snap.with_login_method(login);
    }
    let _expires = parse_iso(env.data.plan.expires_at.as_deref());
    Ok(snap)
}

fn payg_cost_from_balance(value: &serde_json::Value) -> Result<CostSnapshot, ProviderError> {
    let env: BalanceEnvelope = serde_json::from_value(value.clone())
        .map_err(|e| ProviderError::Parse(format!("Failed to parse ZenMux balance: {e}")))?;
    if !env.success {
        return Err(ProviderError::Parse(
            "ZenMux balance response reported failure".into(),
        ));
    }
    if !env.data.currency.trim().eq_ignore_ascii_case("usd") {
        return Err(ProviderError::Parse(
            "ZenMux balance currency is not USD".into(),
        ));
    }
    Ok(CostSnapshot::new(
        env.data.total_credits,
        "USD",
        "ZenMux PAYG balance",
    ))
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_subscription_windows() {
        let value = json!({
            "success": true,
            "data": {
                "plan": { "tier": "pro", "expires_at": "2026-08-01T00:00:00Z" },
                "account_status": "healthy",
                "quota_5_hour": {
                    "usage_percentage": 0.25,
                    "resets_at": "2026-07-21T10:00:00Z",
                    "max_flows": 100,
                    "used_flows": 25,
                    "remaining_flows": 75
                },
                "quota_7_day": {
                    "usage_percentage": 0.1,
                    "resets_at": "2026-07-28T00:00:00Z",
                    "max_flows": 1000,
                    "used_flows": 100,
                    "remaining_flows": 900
                }
            }
        });
        let snap = snapshot_from_subscription(&value).unwrap();
        assert!((snap.primary.used_percent - 25.0).abs() < 0.01);
        assert_eq!(snap.primary.window_minutes, Some(300));
        let weekly = snap.secondary.unwrap();
        assert!((weekly.used_percent - 10.0).abs() < 0.01);
        assert_eq!(snap.login_method.as_deref(), Some("Pro plan"));
    }

    #[test]
    fn parses_payg_balance() {
        let value = json!({
            "success": true,
            "data": { "currency": "USD", "total_credits": 12.5 }
        });
        let cost = payg_cost_from_balance(&value).unwrap();
        assert!((cost.used - 12.5).abs() < 0.001);
        assert_eq!(cost.currency_code, "USD");
    }
}
