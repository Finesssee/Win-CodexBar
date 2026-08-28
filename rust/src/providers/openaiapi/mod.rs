//! OpenAI API usage provider.
//!
//! Tracks organization usage from the Admin API, with the older platform credit
//! balance endpoint as a fallback for project/user keys.

use async_trait::async_trait;
use chrono::{DateTime, Duration, TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;

use crate::core::{
    CostSnapshot, FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId,
    ProviderMetadata, RateWindow, SourceMode, UsageSnapshot,
};

const OPENAI_CREDIT_GRANTS_URL: &str = "https://api.openai.com/v1/dashboard/billing/credit_grants";
const OPENAI_ORG_COSTS_URL: &str = "https://api.openai.com/v1/organization/costs";
const OPENAI_ORG_COMPLETIONS_URL: &str = "https://api.openai.com/v1/organization/usage/completions";
const OPENAI_API_CREDENTIAL_TARGET: &str = "codexbar-openaiapi";

#[derive(Debug, Deserialize)]
struct CreditGrantsResponse {
    total_granted: f64,
    total_used: f64,
    total_available: f64,
    grants: Option<CreditGrantList>,
}

#[derive(Debug, Deserialize)]
struct CreditGrantList {
    data: Vec<CreditGrant>,
}

#[derive(Debug, Deserialize)]
struct CreditGrant {
    expires_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CostsResponse {
    data: Vec<CostBucket>,
}

#[derive(Debug, Deserialize)]
struct CostBucket {
    start_time: i64,
    #[allow(
        dead_code,
        reason = "field present in the OpenAI API billing payload; kept so serde preserves it"
    )]
    end_time: i64,
    results: Vec<CostResult>,
}

#[derive(Debug, Deserialize)]
struct CostResult {
    amount: Option<CostAmount>,
    line_item: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CostAmount {
    value: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct CompletionsUsageResponse {
    data: Vec<CompletionsUsageBucket>,
}

#[derive(Debug, Deserialize)]
struct CompletionsUsageBucket {
    start_time: i64,
    #[allow(
        dead_code,
        reason = "field present in the OpenAI API billing payload; kept so serde preserves it"
    )]
    end_time: i64,
    results: Vec<CompletionsUsageResult>,
}

#[derive(Debug, Deserialize)]
struct CompletionsUsageResult {
    model: Option<String>,
    input_tokens: Option<i64>,
    input_cached_tokens: Option<i64>,
    output_tokens: Option<i64>,
    input_audio_tokens: Option<i64>,
    output_audio_tokens: Option<i64>,
    num_model_requests: Option<i64>,
}

pub struct OpenAIApiProvider {
    metadata: ProviderMetadata,
    client: Client,
}

impl OpenAIApiProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::OpenAIApi,
                display_name: "OpenAI",
                session_label: "Spend",
                weekly_label: "Requests",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://platform.openai.com/usage"),
                status_page_url: Some("https://status.openai.com"),
            },
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    fn api_key(api_key: Option<&str>) -> Result<String, ProviderError> {
        resolve_api_key(
            api_key,
            OPENAI_API_CREDENTIAL_TARGET,
            &[
                "OPENAI_ADMIN_KEY",
                "OPENAI_ADMIN_API_KEY",
                "OPENAI_API_KEY",
                "OPENAI_PLATFORM_API_KEY",
            ],
        )
    }

    async fn fetch_api(&self, api_key: &str) -> Result<ProviderFetchResult, ProviderError> {
        let response = self
            .client
            .get(OPENAI_CREDIT_GRANTS_URL)
            .bearer_auth(api_key)
            .header("Accept", "application/json")
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::FORBIDDEN {
            return Err(ProviderError::AuthRequired);
        }
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ProviderError::AuthRequired);
        }
        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "OpenAI API credit balance returned status {}",
                response.status()
            )));
        }

        let decoded: CreditGrantsResponse = response.json().await.map_err(|e| {
            ProviderError::Parse(format!("Failed to parse OpenAI API credit grants: {e}"))
        })?;
        Ok(result_from_grants(&decoded))
    }

    async fn fetch_admin_usage(
        &self,
        api_key: &str,
        project_id: Option<&str>,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let now = Utc::now();
        let (start, end) = admin_time_range(now)?;
        let project_id = clean_project_id(project_id);

        let costs: CostsResponse = self
            .fetch_admin_json(
                OPENAI_ORG_COSTS_URL,
                &admin_query(
                    ("start_time", start.to_string()),
                    ("end_time", end.to_string()),
                    ("bucket_width", "1d".to_string()),
                    ("limit", "31".to_string()),
                    ("group_by", "line_item".to_string()),
                    project_id.as_deref(),
                ),
                api_key,
                "costs",
            )
            .await?;

        let completions: CompletionsUsageResponse = self
            .fetch_admin_json(
                OPENAI_ORG_COMPLETIONS_URL,
                &admin_query(
                    ("start_time", start.to_string()),
                    ("end_time", end.to_string()),
                    ("bucket_width", "1d".to_string()),
                    ("limit", "31".to_string()),
                    ("group_by", "model".to_string()),
                    project_id.as_deref(),
                ),
                api_key,
                "completions",
            )
            .await?;

        Ok(result_from_admin_usage(
            &costs,
            &completions,
            now,
            project_id.as_deref(),
        ))
    }

    async fn fetch_admin_json<T: serde::de::DeserializeOwned>(
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
            .bearer_auth(api_key)
            .header("Accept", "application/json")
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            || response.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(ProviderError::AuthRequired);
        }
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let detail = response_error_detail(&body);
            return Err(ProviderError::Other(if detail.is_empty() {
                format!("OpenAI API {label} returned status {status}")
            } else {
                format!("OpenAI API {label} returned status {status}: {detail}")
            }));
        }
        response
            .json()
            .await
            .map_err(|e| ProviderError::Parse(format!("Failed to parse OpenAI API {label}: {e}")))
    }
}

fn result_from_grants(grants: &CreditGrantsResponse) -> ProviderFetchResult {
    let used_percent = if grants.total_granted > 0.0 {
        grants.total_used / grants.total_granted * 100.0
    } else if grants.total_available > 0.0 {
        0.0
    } else {
        100.0
    };
    let next_expiry = grants.grants.as_ref().and_then(|list| {
        list.data
            .iter()
            .filter_map(|grant| grant.expires_at)
            .filter_map(|ts| Utc.timestamp_opt(ts, 0).single())
            .filter(|date| *date > Utc::now())
            .min()
    });

    let mut primary = RateWindow::with_details(
        used_percent,
        None,
        next_expiry,
        Some(format!("${:.2} available", grants.total_available.max(0.0))),
    );
    if grants.total_granted <= 0.0 && grants.total_available > 0.0 {
        primary.used_percent = 0.0;
    }

    let usage = UsageSnapshot::new(primary).with_login_method(format!(
        "API balance: ${:.2}",
        grants.total_available.max(0.0)
    ));
    let cost = CostSnapshot::new(grants.total_used.max(0.0), "USD", "API credits")
        .with_limit(grants.total_granted.max(0.0));
    let cost = if let Some(expiry) = next_expiry {
        cost.with_resets_at(expiry)
    } else {
        cost
    };
    ProviderFetchResult::new(usage, "api").with_cost(cost)
}

fn result_from_admin_usage(
    costs: &CostsResponse,
    completions: &CompletionsUsageResponse,
    now: DateTime<Utc>,
    project_id: Option<&str>,
) -> ProviderFetchResult {
    let cost_total: f64 = costs
        .data
        .iter()
        .flat_map(|bucket| &bucket.results)
        .map(|result| {
            result
                .amount
                .as_ref()
                .and_then(|a| number_value(&a.value))
                .unwrap_or(0.0)
        })
        .sum();
    let request_total: i64 = completions
        .data
        .iter()
        .flat_map(|bucket| &bucket.results)
        .map(|result| result.num_model_requests.unwrap_or(0))
        .sum();
    let token_total: i64 = completions
        .data
        .iter()
        .flat_map(|bucket| &bucket.results)
        .map(|result| {
            result.input_tokens.unwrap_or(0)
                + result.input_cached_tokens.unwrap_or(0)
                + result.output_tokens.unwrap_or(0)
                + result.input_audio_tokens.unwrap_or(0)
                + result.output_audio_tokens.unwrap_or(0)
        })
        .sum();

    let first_bucket = costs
        .data
        .first()
        .map(|b| b.start_time)
        .or_else(|| completions.data.first().map(|b| b.start_time));
    let start = first_bucket.and_then(|ts| Utc.timestamp_opt(ts, 0).single());

    let mut model_tokens: HashMap<String, i64> = HashMap::new();
    for result in completions.data.iter().flat_map(|bucket| &bucket.results) {
        let model = result
            .model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("Responses and Chat Completions");
        let tokens = result.input_tokens.unwrap_or(0)
            + result.input_cached_tokens.unwrap_or(0)
            + result.output_tokens.unwrap_or(0)
            + result.input_audio_tokens.unwrap_or(0)
            + result.output_audio_tokens.unwrap_or(0);
        *model_tokens.entry(model.to_string()).or_default() += tokens;
    }

    let mut line_item_costs: HashMap<String, f64> = HashMap::new();
    for result in costs.data.iter().flat_map(|bucket| &bucket.results) {
        let line_item = result
            .line_item
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("API");
        *line_item_costs.entry(line_item.to_string()).or_default() += result
            .amount
            .as_ref()
            .and_then(|a| number_value(&a.value))
            .unwrap_or(0.0);
    }

    let mut usage = UsageSnapshot::new(RateWindow::with_details(
        0.0,
        None,
        start,
        Some(format!("${cost_total:.2} over last 30 days")),
    ))
    .with_extra_rate_window(
        "requests",
        "Requests",
        RateWindow::informational(format!("{request_total} requests")),
    )
    .with_extra_rate_window(
        "tokens",
        "Tokens",
        RateWindow::informational(format!("{token_total} tokens")),
    )
    .with_login_method(
        project_id
            .filter(|id| !id.is_empty())
            .map(|id| format!("Admin API: {id}"))
            .unwrap_or_else(|| "Admin API".to_string()),
    );
    if let Some(project_id) = project_id.filter(|id| !id.is_empty()) {
        usage = usage.with_organization(format!("Project: {project_id}"));
    }
    usage.updated_at = now;

    let mut top_models: Vec<_> = model_tokens.into_iter().collect();
    top_models.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    for (idx, (model, tokens)) in top_models.into_iter().take(3).enumerate() {
        usage = usage.with_extra_rate_window(
            format!("model-{idx}"),
            format!("Model: {model}"),
            RateWindow::informational(format!("{tokens} tokens")),
        );
    }

    let mut top_items: Vec<_> = line_item_costs.into_iter().collect();
    top_items.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    for (idx, (item, amount)) in top_items.into_iter().take(3).enumerate() {
        usage = usage.with_extra_rate_window(
            format!("line-item-{idx}"),
            format!("Cost: {item}"),
            RateWindow::informational(format!("${amount:.2}")),
        );
    }

    ProviderFetchResult::new(usage, "admin-api").with_cost(CostSnapshot::new(
        cost_total,
        "USD",
        "Last 30 days",
    ))
}

fn admin_query(
    start: (&'static str, String),
    end: (&'static str, String),
    bucket_width: (&'static str, String),
    limit: (&'static str, String),
    group_by: (&'static str, String),
    project_id: Option<&str>,
) -> Vec<(&'static str, String)> {
    let mut query = vec![start, end, bucket_width, limit, group_by];
    if let Some(project_id) = clean_project_id(project_id) {
        query.push(("project_ids", project_id));
    }
    query
}

fn admin_time_range(now: DateTime<Utc>) -> Result<(i64, i64), ProviderError> {
    let start = (now.date_naive() - Duration::days(29))
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| ProviderError::Parse("Invalid OpenAI usage start date".to_string()))?
        .and_utc()
        .timestamp();
    Ok((start, now.timestamp()))
}
fn clean_project_id(project_id: Option<&str>) -> Option<String> {
    project_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned)
}
fn response_error_detail(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed)
        && let Some(error) = value.get("error")
    {
        if let Some(message) = error.get("message").and_then(serde_json::Value::as_str) {
            return message.to_string();
        }
        if let Some(message) = error.as_str() {
            return message.to_string();
        }
    }

    trimmed.chars().take(500).collect()
}

fn number_value(value: &serde_json::Value) -> Option<f64> {
    let value = match value {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(text) => text.trim().replace(',', "").parse().ok(),
        _ => None,
    }?;
    value.is_finite().then_some(value)
}

impl Default for OpenAIApiProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for OpenAIApiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::OpenAIApi
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto | SourceMode::OAuth => {
                let api_key = Self::api_key(ctx.api_key.as_deref())?;
                match self
                    .fetch_admin_usage(&api_key, ctx.workspace_id.as_deref())
                    .await
                {
                    Ok(result) => Ok(result),
                    Err(admin_error) => match self.fetch_api(&api_key).await {
                        Ok(result) => Ok(ProviderFetchResult {
                            source_label: "billing-api".to_string(),
                            ..result
                        }),
                        Err(balance_error) => {
                            if matches!(admin_error, ProviderError::AuthRequired) {
                                Err(balance_error)
                            } else {
                                Err(admin_error)
                            }
                        }
                    },
                }
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

fn resolve_api_key(
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

#[allow(
    dead_code,
    reason = "OpenAI API helper reserved for future dashboard integration"
)]
fn _assert_datetime_send(_: DateTime<Utc>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_api_credit_snapshot_formats_available_balance() {
        let result = result_from_grants(&CreditGrantsResponse {
            total_granted: 100.0,
            total_used: 25.0,
            total_available: 75.0,
            grants: None,
        });
        assert_eq!(result.usage.primary.used_percent, 25.0);
        assert_eq!(result.cost.unwrap().remaining(), Some(75.0));
    }

    #[test]
    fn openai_admin_usage_accepts_numeric_string_cost_amounts() {
        let costs = CostsResponse {
            data: vec![CostBucket {
                start_time: 1_797_638_400,
                end_time: 1_797_724_800,
                results: vec![CostResult {
                    amount: Some(CostAmount {
                        value: serde_json::json!("12.50"),
                    }),
                    line_item: Some("API".to_string()),
                }],
            }],
        };
        let completions = CompletionsUsageResponse {
            data: vec![CompletionsUsageBucket {
                start_time: 1_797_638_400,
                end_time: 1_797_724_800,
                results: vec![CompletionsUsageResult {
                    model: Some("gpt-5.1".to_string()),
                    input_tokens: Some(100),
                    input_cached_tokens: Some(25),
                    output_tokens: Some(50),
                    input_audio_tokens: None,
                    output_audio_tokens: None,
                    num_model_requests: Some(7),
                }],
            }],
        };
        let now = Utc.timestamp_opt(1_797_724_800, 0).single().unwrap();
        let result = result_from_admin_usage(&costs, &completions, now, Some("proj_demo"));
        assert_eq!(result.cost.unwrap().used, 12.5);
        assert_eq!(
            result.usage.account_organization.as_deref(),
            Some("Project: proj_demo")
        );
        assert_eq!(
            result.usage.login_method.as_deref(),
            Some("Admin API: proj_demo")
        );
        assert!(result.usage.secondary.is_none());
        let requests = result
            .usage
            .extra_rate_windows
            .iter()
            .find(|window| window.id == "requests")
            .unwrap();
        assert!(requests.window.is_informational);
        assert_eq!(
            requests.window.reset_description.as_deref(),
            Some("7 requests")
        );
        assert!(
            result.usage.extra_rate_windows.iter().any(|window| window
                .window
                .reset_description
                .as_deref()
                == Some("175 tokens"))
        );
    }

    #[test]
    fn openai_admin_usage_rejects_nonfinite_cost_amounts() {
        assert_eq!(number_value(&serde_json::json!("NaN")), None);
        assert_eq!(number_value(&serde_json::json!("Infinity")), None);
    }

    #[test]
    fn openai_admin_query_scopes_project_ids_when_configured() {
        let query = admin_query(
            ("start_time", "1".to_string()),
            ("end_time", "2".to_string()),
            ("bucket_width", "1d".to_string()),
            ("limit", "31".to_string()),
            ("group_by", "model".to_string()),
            Some("  proj_123  "),
        );
        assert!(query.contains(&("project_ids", "proj_123".to_string())));
    }

    #[test]
    fn openai_admin_time_range_does_not_end_in_the_future() {
        let now = Utc.timestamp_opt(1_783_981_862, 0).single().unwrap();
        let (start, end) = admin_time_range(now).unwrap();

        assert_eq!(end, now.timestamp());
        assert!(start < end);
    }

    #[test]
    fn openai_response_error_detail_prefers_api_message() {
        assert_eq!(
            response_error_detail(r#"{"error":{"message":"end_time must not be in the future"}}"#),
            "end_time must not be in the future"
        );
        assert_eq!(
            response_error_detail("plain text failure"),
            "plain text failure"
        );
        assert_eq!(response_error_detail("  "), "");
    }
}
