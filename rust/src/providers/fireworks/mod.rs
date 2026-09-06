//! Fireworks AI provider implementation.
//!
//! Fetches 30-day rated billing spend from the Fireworks billing API:
//! `GET https://api.fireworks.ai/v1/accounts/{slug}/billing/summary?startTime=&endTime=`
//!
//! Fireworks is prepaid with no quota windows and exposes no credit-balance
//! API, so rated spend is the only usable usage signal (upstream 0.49.0
//! #2687). Ported from steipete/CodexBar `FireworksUsageFetcher`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::{Client, StatusCode, Url};
use serde::Deserialize;
use std::collections::BTreeSet;

use crate::core::{
    CostSnapshot, FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId,
    ProviderMetadata, RateWindow, SourceMode, UsageSnapshot,
};

const BILLING_SUMMARY_URL: &str = "https://api.fireworks.ai/v1/accounts";
const CREDENTIAL_TARGET: &str = "codexbar-fireworks";
const ENV_KEYS: &[&str] = &["FIREWORKS_API_KEY"];
const SLUG_ENV_KEYS: &[&str] = &["FIREWORKS_ACCOUNT_SLUG"];
const LOOKBACK_DAYS: i64 = 30;
/// Characters permitted in a Fireworks account slug. Slugs are simple
/// lower-case ASCII path segments; restricting to this explicit ASCII set
/// means a misconfigured slug can never widen the request path or inject a
/// query (upstream `accountSlugAllowedCharacters`).
const SLUG_ALLOWED: fn(char) -> bool =
    |c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BillingSummaryResponse {
    #[serde(default)]
    line_items: Vec<LineItem>,
    #[serde(default)]
    #[allow(
        dead_code,
        reason = "field present in the Fireworks billing payload; kept so serde accepts and preserves it"
    )]
    usage_buckets: Vec<UsageBucket>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LineItem {
    #[serde(default)]
    #[allow(
        dead_code,
        reason = "deserialized for payload fidelity; cost math uses only total_cost"
    )]
    category: Option<String>,
    #[serde(default)]
    total_cost: Option<Money>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Money {
    currency_code: Option<String>,
    nanos: Option<i64>,
    /// Google-style money `units` serialized as a string.
    units: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageBucket {
    #[serde(default)]
    #[allow(
        dead_code,
        reason = "timestamp field of the upstream bucket schema; unused locally but required by the wire format"
    )]
    bucket_start_time: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountsResponse {
    #[serde(default)]
    accounts: Vec<FireworksAccount>,
    #[serde(default)]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FireworksAccount {
    name: Option<String>,
    account_id: Option<String>,
    id: Option<String>,
}

impl FireworksAccount {
    fn slug(&self) -> Option<String> {
        [&self.account_id, &self.id, &self.name]
            .into_iter()
            .flatten()
            .map(|value| value.trim())
            .find(|value| !value.is_empty())
            .and_then(|value| value.rsplit('/').next())
            .map(str::to_string)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct FireworksSummary {
    last_30_days_spend: Option<f64>,
    currency_code: Option<String>,
}

impl FireworksSummary {
    fn from_response(response: &BillingSummaryResponse) -> Self {
        // Rated line items arrive grouped by category/model; the newest-rated
        // currency decides the display currency and only rows in that
        // currency are summed (upstream `parseSummary`).
        let mut currency: Option<String> = None;
        let mut total = 0.0_f64;
        for item in &response.line_items {
            let Some(cost) = item.total_cost.as_ref() else {
                continue;
            };
            let Some(units) = cost
                .units
                .as_deref()
                .and_then(|units| units.parse::<f64>().ok())
            else {
                continue;
            };
            let Some(code) = cost
                .currency_code
                .as_deref()
                .map(str::trim)
                .filter(|code| !code.is_empty())
            else {
                continue;
            };
            if currency.is_none() {
                currency = Some(code.to_string());
            }
            if currency.as_deref() != Some(code) {
                continue;
            }
            total += units + cost.nanos.unwrap_or(0) as f64 / 1_000_000_000.0;
        }

        Self {
            last_30_days_spend: currency.as_ref().map(|_| total),
            currency_code: currency,
        }
    }

    fn to_usage_snapshot(&self) -> UsageSnapshot {
        // Fireworks is prepaid with no quota windows, so no RateWindows are
        // synthesized; the spend text rides the primary description (upstream
        // emits a cost-only snapshot).
        let spend_text = self
            .last_30_days_spend
            .zip(self.currency_code.as_deref())
            .map(|(spend, _)| format_money(spend));
        let mut primary = RateWindow::new(0.0);
        primary.reset_description = spend_text.clone();
        let mut snapshot = UsageSnapshot::new(primary);
        if let Some(text) = spend_text {
            snapshot = snapshot.with_login_method(text);
        }
        snapshot
    }

    fn to_cost_snapshot(&self) -> Option<CostSnapshot> {
        let spend = self.last_30_days_spend?;
        let currency = self.currency_code.as_deref().unwrap_or("USD");
        Some(CostSnapshot::new(spend, currency, "Last 30 days").always_visible())
    }
}

fn format_money(value: f64) -> String {
    format!("${value:.2}")
}

pub struct FireworksProvider {
    metadata: ProviderMetadata,
    client: Client,
}

impl FireworksProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Fireworks,
                display_name: "Fireworks",
                session_label: "Spend",
                weekly_label: "Spend",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://app.fireworks.ai"),
                status_page_url: None,
            },
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    fn resolve_api_key(api_key: Option<&str>) -> Result<String, ProviderError> {
        let raw = crate::providers::resolve_api_key(api_key, CREDENTIAL_TARGET, ENV_KEYS)?;
        let cleaned = raw.trim().to_string();
        if cleaned.is_empty() {
            return Err(ProviderError::NotInstalled(
                "Missing Fireworks API key. Add one in Settings or set FIREWORKS_API_KEY."
                    .to_string(),
            ));
        }
        Ok(cleaned)
    }

    /// Account slug from settings (provider workspace slot) or
    /// `FIREWORKS_ACCOUNT_SLUG`. Validated against the upstream slug charset
    /// so a bad slug surfaces as a config error, not a widened request path.
    fn resolve_account_slug(ctx: &FetchContext) -> Result<Option<String>, ProviderError> {
        let from_env = SLUG_ENV_KEYS.iter().find_map(|key| std::env::var(key).ok());
        let raw = from_env
            .or_else(|| ctx.workspace_id.as_deref().map(str::to_string))
            .unwrap_or_default();
        let slug = raw.trim().to_string();
        if slug.is_empty() {
            return Ok(None);
        }
        if !slug.chars().all(SLUG_ALLOWED) {
            return Err(ProviderError::Other(format!(
                "Invalid Fireworks account slug '{slug}'. Please double-check the account slug in Settings."
            )));
        }
        Ok(Some(slug))
    }

    fn summary_url(slug: &str, now: DateTime<Utc>) -> String {
        let start = now - chrono::Duration::days(LOOKBACK_DAYS);
        format!(
            "{BILLING_SUMMARY_URL}/{slug}/billing/summary?startTime={}&endTime={}",
            start.to_rfc3339(),
            now.to_rfc3339()
        )
    }

    fn accounts_url(page_token: Option<&str>) -> Result<Url, ProviderError> {
        let mut url = Url::parse(BILLING_SUMMARY_URL)
            .map_err(|e| ProviderError::Other(format!("Invalid Fireworks accounts URL: {e}")))?;
        if let Some(token) = page_token.filter(|token| !token.trim().is_empty()) {
            url.query_pairs_mut().append_pair("pageToken", token.trim());
        }
        Ok(url)
    }

    #[cfg(test)]
    fn parse_account_slugs(body: &str) -> Result<Vec<String>, ProviderError> {
        let page: AccountsResponse = serde_json::from_str(body).map_err(|e| {
            ProviderError::Parse(format!("Could not parse Fireworks accounts response: {e}"))
        })?;
        Ok(page
            .accounts
            .iter()
            .filter_map(FireworksAccount::slug)
            .filter(|slug| !slug.is_empty() && slug.chars().all(SLUG_ALLOWED))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect())
    }

    async fn list_account_slugs(&self, api_key: &str) -> Result<Vec<String>, ProviderError> {
        let mut slugs = BTreeSet::new();
        let mut page_token: Option<String> = None;
        for _ in 0..100 {
            let url = Self::accounts_url(page_token.as_deref())?;
            let resp = self
                .client
                .get(url)
                .header("Authorization", format!("Bearer {api_key}"))
                .header("Accept", "application/json")
                .send()
                .await?;
            let status = resp.status();
            if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                return Err(ProviderError::Other(
                    "Fireworks rejected the API key. Create a new key at app.fireworks.ai and update Settings."
                        .to_string(),
                ));
            }
            if status == StatusCode::TOO_MANY_REQUESTS {
                return Err(ProviderError::Other(
                    "Fireworks rate limit exceeded. Usage will refresh on the next cycle."
                        .to_string(),
                ));
            }
            if !status.is_success() {
                return Err(ProviderError::Other(format!(
                    "Fireworks accounts API returned HTTP {status}."
                )));
            }
            let body = resp.text().await.map_err(|e| {
                ProviderError::Parse(format!("Could not read Fireworks accounts response: {e}"))
            })?;
            let page: AccountsResponse = serde_json::from_str(&body).map_err(|e| {
                ProviderError::Parse(format!("Could not parse Fireworks accounts response: {e}"))
            })?;
            for slug in page
                .accounts
                .iter()
                .filter_map(FireworksAccount::slug)
                .filter(|slug| !slug.is_empty() && slug.chars().all(SLUG_ALLOWED))
            {
                slugs.insert(slug);
            }
            page_token = page
                .next_page_token
                .map(|token| token.trim().to_string())
                .filter(|token| !token.is_empty());
            if page_token.is_none() {
                return Ok(slugs.into_iter().collect());
            }
        }
        Err(ProviderError::Other(
            "Fireworks account discovery returned too many pages.".to_string(),
        ))
    }

    fn choose_discovered_slug(slugs: Vec<String>) -> Result<String, ProviderError> {
        match slugs.as_slice() {
            [] => Err(ProviderError::Other(
                "No Fireworks accounts are visible to this API key. Check the key in app.fireworks.ai or run 'firectl whoami'."
                    .to_string(),
            )),
            [slug] => Ok(slug.clone()),
            _ => Err(ProviderError::Other(format!(
                "This Fireworks API key can access multiple accounts: {}. Set the account slug in Settings or FIREWORKS_ACCOUNT_SLUG.",
                slugs.join(", ")
            ))),
        }
    }

    async fn fetch_summary(
        &self,
        api_key: &str,
        slug: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<FireworksSummary>, ProviderError> {
        let url = Self::summary_url(slug, now);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Accept", "application/json")
            .send()
            .await?;
        let status = resp.status();
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(ProviderError::Other(
                "Fireworks rejected the API key. Create a new key at app.fireworks.ai and update Settings."
                    .to_string(),
            ));
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(ProviderError::Other(
                "Fireworks rate limit exceeded. Usage will refresh on the next cycle.".to_string(),
            ));
        }
        if !status.is_success() {
            return Err(ProviderError::Other(format!(
                "Fireworks billing API returned HTTP {status}."
            )));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| ProviderError::Parse(format!("Could not read Fireworks usage: {e}")))?;
        parse_summary_for_testing(&body).map(Some)
    }

    async fn fetch_usage_api(
        &self,
        ctx: &FetchContext,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let api_key = Self::resolve_api_key(ctx.api_key.as_deref())?;
        let configured_slug = Self::resolve_account_slug(ctx)?;
        let now = Utc::now();

        let (slug, summary, discovered) = if let Some(slug) = configured_slug {
            match self.fetch_summary(&api_key, &slug, now).await? {
                Some(summary) if summary.last_30_days_spend.is_some() => (slug, summary, false),
                Some(summary) => {
                    let slugs = self.list_account_slugs(&api_key).await?;
                    if slugs.iter().any(|candidate| candidate == &slug) {
                        (slug, summary, false)
                    } else {
                        return Err(ProviderError::Other(format!(
                            "Fireworks account slug '{slug}' was not found for this API key. Leave it blank to auto-discover the account or run 'firectl whoami'."
                        )));
                    }
                }
                None => {
                    let discovered_slug =
                        Self::choose_discovered_slug(self.list_account_slugs(&api_key).await?)?;
                    let summary = self
                        .fetch_summary(&api_key, &discovered_slug, now)
                        .await?
                        .ok_or_else(|| {
                            ProviderError::Other(format!(
                                "Fireworks account slug '{discovered_slug}' was discovered but its billing endpoint returned 404."
                            ))
                        })?;
                    (discovered_slug, summary, true)
                }
            }
        } else {
            let slug = Self::choose_discovered_slug(self.list_account_slugs(&api_key).await?)?;
            let summary = self
                .fetch_summary(&api_key, &slug, now)
                .await?
                .ok_or_else(|| {
                    ProviderError::Other(format!(
                        "Fireworks account slug '{slug}' was discovered but its billing endpoint returned 404."
                    ))
                })?;
            (slug, summary, true)
        };

        let source = if discovered {
            format!("api / {slug} (auto-discovered)")
        } else {
            format!("api / {slug}")
        };
        let mut result = ProviderFetchResult::new(summary.to_usage_snapshot(), source);
        if let Some(cost) = summary.to_cost_snapshot() {
            result = result.with_cost(cost);
        }
        Ok(result)
    }
}

impl Default for FireworksProvider {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_summary_for_testing(body: &str) -> Result<FireworksSummary, ProviderError> {
    let response: BillingSummaryResponse = serde_json::from_str(body)
        .map_err(|e| ProviderError::Parse(format!("Could not parse Fireworks usage: {e}")))?;
    Ok(FireworksSummary::from_response(&response))
}

#[async_trait]
impl Provider for FireworksProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Fireworks
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_rated_line_items_in_first_currency() {
        let summary = parse_summary_for_testing(
            r#"{
              "lineItems": [
                {"category": "inference", "totalCost": {"currencyCode": "USD", "units": "12", "nanos": 500000000}},
                {"category": "fine-tuning", "totalCost": {"currencyCode": "USD", "units": "3", "nanos": 250000000}},
                {"category": "training", "totalCost": {"currencyCode": "EUR", "units": "1", "nanos": 0}},
                {"category": "unrated"}
              ],
              "usageBuckets": []
            }"#,
        )
        .unwrap();

        assert!((summary.last_30_days_spend.unwrap() - 15.75).abs() < 1e-9);
        assert_eq!(summary.currency_code.as_deref(), Some("USD"));

        let cost = summary.to_cost_snapshot().unwrap();
        assert!((cost.used - 15.75).abs() < 1e-9);
        assert_eq!(cost.currency_code, "USD");
        assert_eq!(cost.period, "Last 30 days");

        let usage = summary.to_usage_snapshot();
        assert_eq!(usage.primary.used_percent, 0.0);
        assert_eq!(usage.primary.reset_description.as_deref(), Some("$15.75"));
    }

    #[test]
    fn unrated_summary_yields_no_spend() {
        let summary = parse_summary_for_testing(
            r#"{"lineItems": [{"category": "pending"}], "usageBuckets": []}"#,
        )
        .unwrap();

        assert!(summary.last_30_days_spend.is_none());
        assert!(summary.currency_code.is_none());
        assert!(summary.to_cost_snapshot().is_none());
    }

    #[test]
    fn slug_validation_rejects_path_and_query_injection() {
        let ctx = |slug: &str| FetchContext {
            source_mode: SourceMode::OAuth,
            workspace_id: Some(slug.to_string()),
            ..FetchContext::default()
        };

        assert_eq!(
            FireworksProvider::resolve_account_slug(&ctx(" ")).unwrap(),
            None
        );
        assert_eq!(
            FireworksProvider::resolve_account_slug(&ctx("acme_corp.1-2")).unwrap(),
            Some("acme_corp.1-2".to_string())
        );
        assert!(FireworksProvider::resolve_account_slug(&ctx("../etc")).is_err());
        assert!(FireworksProvider::resolve_account_slug(&ctx("a?x=1")).is_err());

        let url = FireworksProvider::summary_url(
            "acme",
            DateTime::parse_from_rfc3339("2026-08-17T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        assert!(
            url.starts_with("https://api.fireworks.ai/v1/accounts/acme/billing/summary?startTime=")
        );
        assert!(url.contains("&endTime=2026-08-17T00:00:00"));
    }

    #[test]
    fn account_listing_parses_dedupes_and_sorts_slugs() {
        let slugs = FireworksProvider::parse_account_slugs(
            r#"{"accounts":[{"name":"accounts/zeta"},{"accountId":"alpha"},{"id":"alpha"},{"name":"accounts/bad?slug"}]}"#,
        )
        .unwrap();
        assert_eq!(slugs, vec!["alpha".to_string(), "zeta".to_string()]);
    }

    #[test]
    fn account_discovery_requires_exactly_one_visible_account() {
        let empty = FireworksProvider::choose_discovered_slug(vec![]).unwrap_err();
        assert!(empty.to_string().contains("No Fireworks accounts"));

        assert_eq!(
            FireworksProvider::choose_discovered_slug(vec!["team".to_string()]).unwrap(),
            "team"
        );

        let multiple = FireworksProvider::choose_discovered_slug(vec![
            "alpha".to_string(),
            "zeta".to_string(),
        ])
        .unwrap_err();
        assert!(multiple.to_string().contains("alpha, zeta"));
    }

    #[test]
    fn metadata_matches_upstream_descriptor() {
        let provider = FireworksProvider::new();
        assert_eq!(provider.id(), ProviderId::Fireworks);
        assert_eq!(provider.metadata().display_name, "Fireworks");
        assert_eq!(
            provider.metadata().dashboard_url,
            Some("https://app.fireworks.ai")
        );
        assert_eq!(provider.metadata().status_page_url, None);
        assert!(!provider.metadata().supports_credits);
        assert!(!provider.metadata().default_enabled);
    }
}
