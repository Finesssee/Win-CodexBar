//! DeepInfra provider implementation.
//!
//! Fetches prepaid balance and monthly spend from DeepInfra billing APIs:
//! - `GET https://api.deepinfra.com/payment/checklist?compute_owed=true`
//! - `GET https://api.deepinfra.com/payment/usage?from=current`
//!
//! Ported from steipete/CodexBar `DeepInfraUsageFetcher`.

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

use crate::core::{
    CostSnapshot, FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId,
    ProviderMetadata, RateWindow, SourceMode, UsageSnapshot,
};

const CHECKLIST_URL: &str = "https://api.deepinfra.com/payment/checklist?compute_owed=true";
const USAGE_URL: &str = "https://api.deepinfra.com/payment/usage?from=current";
const CREDENTIAL_TARGET: &str = "codexbar-deepinfra";
const CENTS_PER_DOLLAR: f64 = 100.0;
const ENV_KEYS: &[&str] = &["DEEPINFRA_API_KEY", "DEEPINFRA_TOKEN"];

/// Checklist monetary fields are USD. Negative `stripe_balance` means prepaid funds.
#[derive(Debug, Deserialize, Clone)]
struct ChecklistResponse {
    stripe_balance: f64,
    recent: f64,
    limit: Option<f64>,
    #[serde(default)]
    suspended: bool,
    suspend_reason: Option<String>,
}

/// Usage endpoint reports `total_cost` in cents.
#[derive(Debug, Deserialize, Clone)]
struct UsageResponse {
    months: Vec<UsageMonth>,
    #[serde(default)]
    initial_month: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct UsageMonth {
    #[allow(
        dead_code,
        reason = "field present in the DeepInfra billing payload; kept so serde preserves it"
    )]
    period: String,
    /// Cost in cents (upstream field name is `total_cost`).
    total_cost: f64,
}

#[derive(Debug, Clone, PartialEq)]
struct DeepInfraSnapshot {
    available_balance_usd: f64,
    amount_owed_usd: f64,
    current_month_cost_usd: f64,
    recent_cost_usd: f64,
    spending_limit_usd: Option<f64>,
    suspended: bool,
    suspend_reason: Option<String>,
}

impl DeepInfraSnapshot {
    fn from_responses(checklist: &ChecklistResponse, usage: &UsageResponse) -> Self {
        let recent_cost = checklist.recent.max(0.0);
        let current_month_cost = usage
            .months
            .last()
            .map(|m| (m.total_cost / CENTS_PER_DOLLAR).max(0.0))
            .unwrap_or(recent_cost);
        let net_balance = checklist.stripe_balance + recent_cost;
        let spending_limit = checklist
            .limit
            .and_then(|limit| (limit > 0.0).then_some(limit));

        Self {
            available_balance_usd: (-net_balance).max(0.0),
            amount_owed_usd: net_balance.max(0.0),
            current_month_cost_usd: current_month_cost,
            recent_cost_usd: recent_cost,
            spending_limit_usd: spending_limit,
            suspended: checklist.suspended,
            suspend_reason: checklist.suspend_reason.clone(),
        }
    }

    fn to_usage_snapshot(&self) -> UsageSnapshot {
        // Upstream #2822: with a positive spending limit configured, the
        // primary percent reflects billing-cycle spend against that limit so
        // the automatic menu-bar metric tracks it; balance-depletion stays
        // the hard 100% signal.
        let used_percent =
            if self.suspended || self.amount_owed_usd > 0.0 || self.available_balance_usd <= 0.0 {
                100.0
            } else if let Some(limit) = self
                .spending_limit_usd
                .filter(|limit| *limit > 0.0 && limit.is_finite())
            {
                ((self.recent_cost_usd / limit) * 100.0).clamp(0.0, 100.0)
            } else {
                0.0
            };

        let balance_text = if self.amount_owed_usd > 0.0 {
            format!("${:.2} owed", self.amount_owed_usd)
        } else {
            format!("${:.2} available", self.available_balance_usd)
        };
        let spending_text = format!("${:.2} spent this month", self.current_month_cost_usd);
        let detail = if self.suspended {
            let reason = self
                .suspend_reason
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            match reason {
                Some(reason) => format!("Suspended: {reason} · {balance_text} · {spending_text}"),
                None => format!("Suspended · {balance_text} · {spending_text}"),
            }
        } else {
            format!("{balance_text} · {spending_text}")
        };

        let mut primary = RateWindow::new(used_percent);
        primary.reset_description = Some(detail);

        UsageSnapshot::new(primary).with_login_method(balance_text)
    }

    fn to_cost_snapshot(&self) -> Option<CostSnapshot> {
        self.spending_limit_usd.map(|limit| {
            CostSnapshot::new(self.recent_cost_usd, "USD", "Billing cycle").with_limit(limit)
        })
    }
}

pub struct DeepInfraProvider {
    metadata: ProviderMetadata,
    client: Client,
}

impl DeepInfraProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::DeepInfra,
                display_name: "DeepInfra",
                session_label: "Balance",
                weekly_label: "Balance",
                supports_opus: false,
                // Upstream marks supportsCredits=false; balance is shown via primary window text.
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://deepinfra.com/dash"),
                status_page_url: Some("https://status.deepinfra.com"),
            },
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    fn resolve_api_key(api_key: Option<&str>) -> Result<String, ProviderError> {
        let raw = crate::providers::resolve_api_key(api_key, CREDENTIAL_TARGET, ENV_KEYS)?;
        clean_api_key(&raw).ok_or_else(|| {
            ProviderError::NotInstalled(
                "DeepInfra API key not found. Set DEEPINFRA_API_KEY / DEEPINFRA_TOKEN or Preferences → Providers."
                    .to_string(),
            )
        })
    }

    async fn fetch_usage_api(
        &self,
        ctx: &FetchContext,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let api_key = Self::resolve_api_key(ctx.api_key.as_deref())?;
        let checklist = self
            .fetch_json::<ChecklistResponse>(CHECKLIST_URL, &api_key)
            .await?;
        let usage = self
            .fetch_json::<UsageResponse>(USAGE_URL, &api_key)
            .await?;
        let snapshot = DeepInfraSnapshot::from_responses(&checklist, &usage);

        let mut result = ProviderFetchResult::new(snapshot.to_usage_snapshot(), "api");
        if let Some(cost) = snapshot.to_cost_snapshot() {
            result = result.with_cost(cost);
        }
        Ok(result)
    }

    async fn fetch_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        api_key: &str,
    ) -> Result<T, ProviderError> {
        let resp = self
            .client
            .get(url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Accept", "application/json")
            .send()
            .await?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ProviderError::Other(
                "DeepInfra API key rejected (HTTP 401).".to_string(),
            ));
        }
        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(ProviderError::Other(
                "DeepInfra API key cannot access billing data (HTTP 403).".to_string(),
            ));
        }
        if !status.is_success() {
            return Err(ProviderError::Other(format!(
                "DeepInfra API error: HTTP {status}"
            )));
        }

        resp.json()
            .await
            .map_err(|e| ProviderError::Parse(format!("Failed to parse DeepInfra response: {e}")))
    }
}

impl Default for DeepInfraProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for DeepInfraProvider {
    fn id(&self) -> ProviderId {
        ProviderId::DeepInfra
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto | SourceMode::OAuth => self.fetch_usage_api(ctx).await,
            SourceMode::Web | SourceMode::Cli => {
                Err(ProviderError::UnsupportedSource(ctx.source_mode))
            }
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::OAuth]
    }
}

fn clean_api_key(raw: &str) -> Option<String> {
    let mut value = raw.trim().to_string();
    if (value.starts_with('"') && value.ends_with('"'))
        || (value.starts_with('\'') && value.ends_with('\''))
    {
        value = value[1..value.len() - 1].trim().to_string();
    }
    if let Some(stripped) = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
    {
        value = stripped.trim().to_string();
    }
    (!value.is_empty()).then_some(value)
}

/// Parse fixture JSON without network (used by unit tests).
fn parse_snapshot_for_testing(
    checklist_json: &str,
    usage_json: &str,
) -> Result<DeepInfraSnapshot, ProviderError> {
    let checklist: ChecklistResponse = serde_json::from_str(checklist_json)
        .map_err(|e| ProviderError::Parse(format!("Failed to parse DeepInfra checklist: {e}")))?;
    let usage: UsageResponse = serde_json::from_str(usage_json)
        .map_err(|e| ProviderError::Parse(format!("Failed to parse DeepInfra usage: {e}")))?;
    let _ = usage.initial_month.as_ref();
    Ok(DeepInfraSnapshot::from_responses(&checklist, &usage))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checklist_json(
        stripe_balance: f64,
        recent: f64,
        limit: Option<f64>,
        suspended: bool,
        suspend_reason: Option<&str>,
    ) -> String {
        let limit_json = match limit {
            Some(v) => v.to_string(),
            None => "null".to_string(),
        };
        let reason_json = match suspend_reason {
            Some(r) => format!("\"{r}\""),
            None => "null".to_string(),
        };
        format!(
            r#"{{
              "stripe_balance": {stripe_balance},
              "recent": {recent},
              "limit": {limit_json},
              "suspended": {suspended},
              "suspend_reason": {reason_json}
            }}"#
        )
    }

    fn usage_json(total_cost_cents: f64) -> String {
        format!(
            r#"{{
              "months": [
                {{
                  "period": "2026.07",
                  "items": [],
                  "total_cost": {total_cost_cents}
                }}
              ],
              "initial_month": "2026.07"
            }}"#
        )
    }

    #[test]
    fn converts_monthly_cents_and_deducts_recent_usage_from_prepaid_balance() {
        let snapshot = parse_snapshot_for_testing(
            &checklist_json(-99.75, 3.94, Some(20.0), false, None),
            &usage_json(394.0),
        )
        .unwrap();

        assert!((snapshot.available_balance_usd - 95.81).abs() < 1e-6);
        assert_eq!(snapshot.amount_owed_usd, 0.0);
        assert!((snapshot.current_month_cost_usd - 3.94).abs() < 1e-6);
        assert_eq!(snapshot.recent_cost_usd, 3.94);
        assert_eq!(snapshot.spending_limit_usd, Some(20.0));

        let usage = snapshot.to_usage_snapshot();
        // Upstream #2822: positive spending limit drives the percent from
        // billing-cycle spend (3.94 / 20.00 ≈ 19.7%), not a flat 0%.
        assert!((usage.primary.used_percent - 19.7).abs() < 0.05);
        assert_eq!(
            usage.primary.reset_description.as_deref(),
            Some("$95.81 available · $3.94 spent this month")
        );

        let cost = snapshot.to_cost_snapshot().unwrap();
        assert_eq!(cost.used, 3.94);
        assert_eq!(cost.limit, Some(20.0));
        assert_eq!(cost.period, "Billing cycle");
    }

    #[test]
    fn spend_over_limit_clamps_to_100_and_zero_limit_stays_binary() {
        // Spend beyond the positive limit clamps at 100% instead of exceeding.
        let snapshot = parse_snapshot_for_testing(
            &checklist_json(-50.0, 30.0, Some(20.0), false, None),
            &usage_json(3000.0),
        )
        .unwrap();
        assert_eq!(snapshot.to_usage_snapshot().primary.used_percent, 100.0);

        // No (or non-positive) limit keeps the legacy binary balance signal.
        let snapshot = parse_snapshot_for_testing(
            &checklist_json(-50.0, 1.0, Some(0.0), false, None),
            &usage_json(100.0),
        )
        .unwrap();
        assert_eq!(snapshot.to_usage_snapshot().primary.used_percent, 0.0);
    }

    #[test]
    fn positive_stripe_balance_is_reported_as_amount_owed() {
        let snapshot = parse_snapshot_for_testing(
            &checklist_json(2.75, 7.0, Some(-1.0), false, None),
            &usage_json(650.0),
        )
        .unwrap();

        assert_eq!(snapshot.available_balance_usd, 0.0);
        assert_eq!(snapshot.amount_owed_usd, 9.75);
        assert_eq!(snapshot.spending_limit_usd, None);

        let usage = snapshot.to_usage_snapshot();
        assert_eq!(usage.primary.used_percent, 100.0);
        assert_eq!(
            usage.primary.reset_description.as_deref(),
            Some("$9.75 owed · $6.50 spent this month")
        );
        assert!(snapshot.to_cost_snapshot().is_none());
    }

    #[test]
    fn suspended_account_is_marked_exhausted() {
        let snapshot = parse_snapshot_for_testing(
            &checklist_json(-5.0, 1.0, None, true, Some("Payment review")),
            &usage_json(100.0),
        )
        .unwrap()
        .to_usage_snapshot();

        assert_eq!(snapshot.primary.used_percent, 100.0);
        assert!(
            snapshot
                .primary
                .reset_description
                .as_deref()
                .unwrap_or("")
                .starts_with("Suspended: Payment review")
        );
    }

    #[test]
    fn rejects_malformed_billing_response() {
        let err = parse_snapshot_for_testing("{}", &usage_json(100.0)).unwrap_err();
        match err {
            ProviderError::Parse(msg) => assert!(msg.contains("checklist")),
            other => panic!("expected parse error, got {other:?}"),
        }
    }

    #[test]
    fn cleans_quoted_and_bearer_prefixed_keys() {
        assert_eq!(
            clean_api_key("  \"Bearer sk-test\"  ").as_deref(),
            Some("sk-test")
        );
        assert_eq!(clean_api_key("bearer sk-abc").as_deref(), Some("sk-abc"));
        assert_eq!(clean_api_key("   ").as_deref(), None);
    }

    #[test]
    fn metadata_matches_upstream_descriptor() {
        let provider = DeepInfraProvider::new();
        assert_eq!(provider.id(), ProviderId::DeepInfra);
        assert_eq!(provider.metadata().display_name, "DeepInfra");
        assert_eq!(
            provider.metadata().dashboard_url,
            Some("https://deepinfra.com/dash")
        );
        assert_eq!(
            provider.metadata().status_page_url,
            Some("https://status.deepinfra.com")
        );
        assert!(!provider.metadata().supports_credits);
        assert!(!provider.metadata().default_enabled);
    }
}
