//! Neuralwatt API-key usage provider (upstream 0.44 #2220).
//!
//! `GET https://api.neuralwatt.com/v1/quota` — subscription kWh + prepaid credits.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;

use crate::core::{
    CostSnapshot, FetchContext, NamedRateWindow, Provider, ProviderError, ProviderFetchResult,
    ProviderId, ProviderMetadata, RateWindow, SourceMode, UsageSnapshot,
};

const DEFAULT_API_BASE: &str = "https://api.neuralwatt.com";
const CREDENTIAL_TARGET: &str = "codexbar-neuralwatt";
const ENV_KEYS: &[&str] = &["NEURALWATT_API_KEY"];

#[derive(Debug, Deserialize, Default)]
struct QuotaResponse {
    balance: Option<Balance>,
    usage: Option<Usage>,
    subscription: Option<Subscription>,
    key: Option<KeyInfo>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct Balance {
    credits_remaining_usd: Option<f64>,
    total_credits_usd: Option<f64>,
    credits_used_usd: Option<f64>,
    accounting_method: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct Usage {
    current_month: Option<UsagePeriod>,
}

#[derive(Debug, Deserialize, Default)]
struct UsagePeriod {
    cost_usd: Option<f64>,
    energy_kwh: Option<f64>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct Subscription {
    plan: Option<String>,
    status: Option<String>,
    current_period_start: Option<String>,
    current_period_end: Option<String>,
    kwh_included: Option<f64>,
    kwh_used: Option<f64>,
    kwh_remaining: Option<f64>,
    #[allow(
        dead_code,
        reason = "field mirrors the NeuralWatt API payload; deserialized for round-trip fidelity but not read yet"
    )]
    in_overage: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
struct KeyInfo {
    allowance: Option<KeyAllowance>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct KeyAllowance {
    limit_usd: Option<f64>,
    period: Option<String>,
    spent_usd: Option<f64>,
    remaining_usd: Option<f64>,
    blocked: Option<bool>,
}

pub struct NeuralwattProvider {
    metadata: ProviderMetadata,
    client: Client,
}

impl NeuralwattProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Neuralwatt,
                display_name: "Neuralwatt",
                session_label: "Subscription",
                weekly_label: "Key allowance",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://portal.neuralwatt.com/dashboard"),
                status_page_url: None,
            },
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    fn quota_url() -> String {
        let base = std::env::var("NEURALWATT_API_URL")
            .ok()
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_API_BASE.to_string());
        if base.ends_with("/v1") {
            format!("{base}/quota")
        } else {
            format!("{base}/v1/quota")
        }
    }
}

impl Default for NeuralwattProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for NeuralwattProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Neuralwatt
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto | SourceMode::OAuth => {
                let key = crate::providers::resolve_api_key(
                    ctx.api_key.as_deref(),
                    CREDENTIAL_TARGET,
                    ENV_KEYS,
                )?;
                let resp = self
                    .client
                    .get(Self::quota_url())
                    .bearer_auth(key)
                    .header("Accept", "application/json")
                    .send()
                    .await?;
                let status = resp.status();
                if status == reqwest::StatusCode::UNAUTHORIZED
                    || status == reqwest::StatusCode::FORBIDDEN
                {
                    return Err(ProviderError::AuthRequired);
                }
                if !status.is_success() {
                    return Err(ProviderError::Other(format!(
                        "Neuralwatt API error: HTTP {status}"
                    )));
                }
                let body: QuotaResponse = resp.json().await.map_err(|e| {
                    ProviderError::Parse(format!("Failed to parse Neuralwatt quota: {e}"))
                })?;
                let (snap, cost) = snapshot_from_quota(&body)?;
                let mut result = ProviderFetchResult::new(snap, "api");
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
}

fn valid_nn(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite() && *x >= 0.0)
}

fn valid_pos(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite() && *x > 0.0)
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

fn format_kwh(value: f64) -> String {
    if (value - value.round()).abs() < f64::EPSILON {
        format!("{:.0}", value)
    } else {
        format!("{:.2}", value)
    }
}

fn subscription_window(sub: &Subscription) -> Option<RateWindow> {
    let total = valid_pos(sub.kwh_included).or_else(|| {
        let used = valid_nn(sub.kwh_used)?;
        let remaining = valid_nn(sub.kwh_remaining)?;
        let t = used + remaining;
        (t > 0.0).then_some(t)
    })?;
    let used = valid_nn(sub.kwh_used).or_else(|| {
        let remaining = valid_nn(sub.kwh_remaining)?;
        Some((total - remaining).max(0.0))
    })?;
    let mut w = RateWindow::new(((used / total) * 100.0).clamp(0.0, 100.0));
    if let (Some(start), Some(end)) = (
        parse_iso(sub.current_period_start.as_deref()),
        parse_iso(sub.current_period_end.as_deref()),
    ) {
        if end > start {
            // Billing-window minutes are clamped to u32 range, so the cast cannot truncate.
            #[expect(clippy::cast_possible_truncation, reason = "clamped to u32 range")]
            let mins = ((end - start).num_minutes()).clamp(1, u32::MAX as i64) as u32;
            w.window_minutes = Some(mins);
        }
        w.resets_at = Some(end);
    }
    w.reset_description = Some(format!("{} / {} kWh", format_kwh(used), format_kwh(total)));
    Some(w)
}

fn prepaid_remaining(bal: &Balance) -> Option<f64> {
    if let Some(r) = valid_nn(bal.credits_remaining_usd) {
        return Some(r);
    }
    let total = valid_pos(bal.total_credits_usd)?;
    let used = valid_nn(bal.credits_used_usd)?;
    Some((total - used).max(0.0))
}

fn snapshot_from_quota(
    body: &QuotaResponse,
) -> Result<(UsageSnapshot, Option<CostSnapshot>), ProviderError> {
    let sub_window = body
        .subscription
        .as_ref()
        .and_then(subscription_window)
        .unwrap_or_else(|| RateWindow::informational("No active subscription kWh"));

    let mut snap = UsageSnapshot::new(sub_window);
    if let Some(sub) = &body.subscription {
        if let Some(plan) = sub.plan.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let label = plan.replace('_', " ");
            snap = snap.with_login_method(format!(
                "{} plan",
                label
                    .split_whitespace()
                    .map(|w| {
                        let mut c = w.chars();
                        match c.next() {
                            None => String::new(),
                            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
        } else if let Some(method) = body
            .balance
            .as_ref()
            .and_then(|b| b.accounting_method.clone())
            .filter(|s| !s.is_empty())
        {
            snap = snap.with_login_method(method);
        }
        let _status = sub.status.as_ref();
    }

    if let Some(allowance) = body.key.as_ref().and_then(|k| k.allowance.clone()) {
        let percent = if allowance.blocked == Some(true) {
            Some(100.0)
        } else if let (Some(spent), Some(limit)) = (
            valid_nn(allowance.spent_usd),
            valid_pos(allowance.limit_usd),
        ) {
            Some(((spent / limit) * 100.0).clamp(0.0, 100.0))
        } else {
            None
        };
        if let Some(percent) = percent {
            let period = allowance
                .period
                .as_deref()
                .unwrap_or("allowance")
                .to_string();
            let title = format!(
                "Key {}",
                period
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().collect::<String>() + &period[c.len_utf8()..])
                    .unwrap_or_else(|| period)
            );
            snap.extra_rate_windows.push(NamedRateWindow::new(
                "key-allowance",
                title,
                RateWindow::new(percent),
            ));
            let _remaining = allowance.remaining_usd;
        }
    }

    let cost = body
        .balance
        .as_ref()
        .and_then(prepaid_remaining)
        .map(|remaining| CostSnapshot::new(remaining, "USD", "Neuralwatt prepaid balance"));

    let _month = body.usage.as_ref().and_then(|u| u.current_month.as_ref());
    Ok((snap, cost))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_kwh_and_prepaid() {
        let body: QuotaResponse = serde_json::from_str(
            r#"{
          "balance": {
            "credits_remaining_usd": 8.5,
            "total_credits_usd": 20.0,
            "credits_used_usd": 11.5
          },
          "subscription": {
            "plan": "starter",
            "kwh_included": 100.0,
            "kwh_used": 40.0,
            "kwh_remaining": 60.0,
            "current_period_end": "2026-08-01T00:00:00Z"
          },
          "key": {
            "allowance": {
              "limit_usd": 50.0,
              "period": "month",
              "spent_usd": 10.0,
              "blocked": false
            }
          }
        }"#,
        )
        .unwrap();
        let (snap, cost) = snapshot_from_quota(&body).unwrap();
        assert!((snap.primary.used_percent - 40.0).abs() < 0.01);
        assert!(snap.login_method.as_deref().unwrap().contains("Starter"));
        assert_eq!(snap.extra_rate_windows.len(), 1);
        assert!((snap.extra_rate_windows[0].window.used_percent - 20.0).abs() < 0.01);
        assert!((cost.unwrap().used - 8.5).abs() < 0.001);
    }
}
