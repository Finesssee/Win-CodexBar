//! Claude Admin API usage source.
//!
//! Mirrors upstream v0.27's organization cost/messages report path. This is an
//! additive Auto source: if no Admin API key is configured, the provider falls
//! through to OAuth/web/CLI.

use chrono::{DateTime, Duration, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;

use crate::core::{
    CostSnapshot, FetchContext, ProviderError, ProviderFetchResult, RateWindow, UsageSnapshot,
};

const COST_REPORT_URL: &str = "https://api.anthropic.com/v1/organizations/cost_report";
const MESSAGES_USAGE_URL: &str = "https://api.anthropic.com/v1/organizations/usage_report/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct ClaudeAdminApiFetcher {
    client: Client,
}

impl ClaudeAdminApiFetcher {
    pub fn new() -> Self {
        Self {
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    pub fn has_credentials(&self, ctx: &FetchContext) -> bool {
        Self::api_key(ctx).is_some()
    }

    pub async fn fetch(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        let api_key = Self::api_key(ctx).ok_or(ProviderError::AuthRequired)?;
        let now = Utc::now();
        let start = (now.date_naive() - Duration::days(29))
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| ProviderError::Parse("Invalid Claude Admin API start date".to_string()))?
            .and_utc();
        let end = (now.date_naive() + Duration::days(1))
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| ProviderError::Parse("Invalid Claude Admin API end date".to_string()))?
            .and_utc();

        let costs: CostReportResponse = self
            .fetch_json(
                COST_REPORT_URL,
                &[
                    ("starting_at", start.to_rfc3339()),
                    ("ending_at", end.to_rfc3339()),
                    ("bucket_width", "1d".to_string()),
                    ("limit", "31".to_string()),
                    ("group_by[]", "description".to_string()),
                ],
                &api_key,
                "cost_report",
            )
            .await?;
        let messages: MessagesUsageResponse = self
            .fetch_json(
                MESSAGES_USAGE_URL,
                &[
                    ("starting_at", start.to_rfc3339()),
                    ("ending_at", end.to_rfc3339()),
                    ("bucket_width", "1d".to_string()),
                    ("limit", "31".to_string()),
                    ("group_by[]", "model".to_string()),
                ],
                &api_key,
                "messages",
            )
            .await?;
        Ok(result_from_admin_usage(&costs, &messages, now))
    }

    fn api_key(ctx: &FetchContext) -> Option<String> {
        for env in ["ANTHROPIC_ADMIN_KEY", "ANTHROPIC_ADMIN_API_KEY"] {
            if let Ok(value) = std::env::var(env)
                && let Some(cleaned) = clean_key(&value)
            {
                return Some(cleaned);
            }
        }
        ctx.api_key
            .as_deref()
            .and_then(clean_key)
            .filter(|key| key.starts_with("sk-ant-admin") || key.contains("admin"))
    }

    async fn fetch_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        query: &[(&str, String)],
        api_key: &str,
        label: &str,
    ) -> Result<T, ProviderError> {
        let response = self
            .client
            .get(url)
            .query(query)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("Accept", "application/json")
            .header("User-Agent", "CodexBar/1.0")
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            || response.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(ProviderError::AuthRequired);
        }
        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Claude Admin API {label} returned status {}",
                response.status()
            )));
        }
        response.json().await.map_err(|e| {
            ProviderError::Parse(format!("Failed to parse Claude Admin API {label}: {e}"))
        })
    }
}

fn clean_key(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches('"').trim_matches('\'').trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[derive(Debug, Deserialize)]
struct CostReportResponse {
    data: Vec<CostBucket>,
}

#[derive(Debug, Deserialize)]
struct CostBucket {
    starting_at: String,
    #[allow(
        dead_code,
        reason = "admin API response fields are deserialized for parsing but not all are read"
    )]
    ending_at: String,
    results: Vec<CostResult>,
}

#[derive(Debug, Deserialize)]
struct CostResult {
    amount: String,
    description: Option<String>,
    cost_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MessagesUsageResponse {
    data: Vec<MessageBucket>,
}

#[derive(Debug, Deserialize)]
struct MessageBucket {
    starting_at: String,
    #[allow(
        dead_code,
        reason = "admin API response fields are deserialized for parsing but not all are read"
    )]
    ending_at: String,
    results: Vec<MessageResult>,
}

#[derive(Debug, Deserialize)]
struct MessageResult {
    model: Option<String>,
    uncached_input_tokens: Option<i64>,
    cache_read_input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_creation: Option<CacheCreation>,
}

#[derive(Debug, Deserialize)]
struct CacheCreation {
    total_input_tokens: Option<i64>,
}

fn result_from_admin_usage(
    costs: &CostReportResponse,
    messages: &MessagesUsageResponse,
    now: DateTime<Utc>,
) -> ProviderFetchResult {
    let cost_total: f64 = costs
        .data
        .iter()
        .flat_map(|bucket| &bucket.results)
        .map(|result| usd_from_lowest_unit(&result.amount))
        .sum();
    let input_tokens: i64 = messages
        .data
        .iter()
        .flat_map(|bucket| &bucket.results)
        .map(|r| {
            r.uncached_input_tokens.unwrap_or(0)
                + r.cache_creation
                    .as_ref()
                    .and_then(|c| c.total_input_tokens)
                    .unwrap_or(0)
                + r.cache_read_input_tokens.unwrap_or(0)
        })
        .sum();
    let output_tokens: i64 = messages
        .data
        .iter()
        .flat_map(|bucket| &bucket.results)
        .map(|r| r.output_tokens.unwrap_or(0))
        .sum();
    let total_tokens = input_tokens + output_tokens;
    let start = costs
        .data
        .first()
        .and_then(|b| DateTime::parse_from_rfc3339(&b.starting_at).ok())
        .or_else(|| {
            messages
                .data
                .first()
                .and_then(|b| DateTime::parse_from_rfc3339(&b.starting_at).ok())
        })
        .map(|dt| dt.with_timezone(&Utc));

    let mut usage = UsageSnapshot::new(RateWindow::with_details(
        0.0,
        None,
        start,
        Some(format!("${cost_total:.2} over last 30 days")),
    ))
    .with_secondary(RateWindow::with_details(
        0.0,
        None,
        None,
        Some(format!("{total_tokens} tokens")),
    ))
    .with_extra_rate_window(
        "input-tokens",
        "Input tokens",
        RateWindow::with_details(0.0, None, None, Some(format!("{input_tokens}"))),
    )
    .with_extra_rate_window(
        "output-tokens",
        "Output tokens",
        RateWindow::with_details(0.0, None, None, Some(format!("{output_tokens}"))),
    )
    .with_login_method("Admin API");
    usage.updated_at = now;

    let mut model_tokens: HashMap<String, i64> = HashMap::new();
    for result in messages.data.iter().flat_map(|bucket| &bucket.results) {
        let name = result
            .model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("Claude API");
        let tokens = result.uncached_input_tokens.unwrap_or(0)
            + result
                .cache_creation
                .as_ref()
                .and_then(|c| c.total_input_tokens)
                .unwrap_or(0)
            + result.cache_read_input_tokens.unwrap_or(0)
            + result.output_tokens.unwrap_or(0);
        *model_tokens.entry(name.to_string()).or_default() += tokens;
    }
    let mut top_models: Vec<_> = model_tokens.into_iter().collect();
    top_models.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    for (idx, (model, tokens)) in top_models.into_iter().take(3).enumerate() {
        usage = usage.with_extra_rate_window(
            format!("model-{idx}"),
            format!("Model: {model}"),
            RateWindow::with_details(0.0, None, None, Some(format!("{tokens} tokens"))),
        );
    }

    let mut cost_items: HashMap<String, f64> = HashMap::new();
    for result in costs.data.iter().flat_map(|bucket| &bucket.results) {
        let name = result
            .description
            .as_deref()
            .or(result.cost_type.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("Claude API");
        *cost_items.entry(name.to_string()).or_default() += usd_from_lowest_unit(&result.amount);
    }
    let mut top_items: Vec<_> = cost_items.into_iter().collect();
    top_items.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    for (idx, (item, cost)) in top_items.into_iter().take(3).enumerate() {
        usage = usage.with_extra_rate_window(
            format!("cost-{idx}"),
            format!("Cost: {item}"),
            RateWindow::with_details(0.0, None, None, Some(format!("${cost:.2}"))),
        );
    }

    ProviderFetchResult::new(usage, "admin-api").with_cost(CostSnapshot::new(
        cost_total,
        "USD",
        "Last 30 days",
    ))
}

fn usd_from_lowest_unit(raw: &str) -> f64 {
    raw.parse::<f64>().unwrap_or(0.0) / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_quoted_admin_key() {
        assert_eq!(
            clean_key(" 'sk-ant-admin-123' "),
            Some("sk-ant-admin-123".to_string())
        );
    }
}
