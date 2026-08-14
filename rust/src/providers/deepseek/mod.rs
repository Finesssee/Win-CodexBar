//! DeepSeek provider implementation.
//!
//! Fetches API account balance from DeepSeek's `/user/balance` endpoint.

use async_trait::async_trait;
use reqwest::Client;
use std::collections::HashMap;

use serde::Deserialize;

pub mod pricing;

use crate::core::{
    CostSnapshot, FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId,
    ProviderMetadata, RateWindow, SourceMode, UsageSnapshot,
};

const DEEPSEEK_API_BASE: &str = "https://api.deepseek.com";
const DEEPSEEK_CREDENTIAL_TARGET: &str = "codexbar-deepseek";

#[derive(Debug, Deserialize)]
struct BalanceResponse {
    #[serde(default)]
    is_available: bool,
    #[serde(default)]
    balance_infos: Vec<BalanceInfo>,
}

#[derive(Debug, Deserialize, Clone)]
struct BalanceInfo {
    currency: String,
    total_balance: String,
    granted_balance: String,
    topped_up_balance: String,
}

#[derive(Debug, Deserialize)]
struct UsageEnvelope<T> {
    code: Option<FlexibleI64>,
    msg: Option<String>,
    data: Option<UsageEnvelopeData<T>>,
}

#[derive(Debug, Deserialize)]
struct UsageEnvelopeData<T> {
    biz_code: Option<FlexibleI64>,
    biz_data: Option<T>,
}

#[derive(Debug, Deserialize, Default)]
struct DeepSeekAmountData {
    #[serde(default)]
    total: Vec<DeepSeekAmountItem>,
    #[serde(default)]
    days: Vec<DeepSeekAmountDay>,
}

#[derive(Debug, Deserialize, Default)]
struct DeepSeekAmountDay {
    #[serde(default)]
    date: String,
    #[serde(default)]
    models: Vec<DeepSeekAmountItem>,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct DeepSeekAmountItem {
    #[serde(default)]
    model: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    amount: FlexibleF64,
}

#[derive(Debug, Deserialize, Default)]
struct DeepSeekCostData {
    #[serde(default)]
    total: Vec<DeepSeekCostItem>,
    #[serde(default)]
    days: Vec<DeepSeekCostDay>,
}

#[derive(Debug, Deserialize, Default)]
struct DeepSeekCostDay {
    #[serde(default)]
    date: String,
    #[serde(default)]
    models: Vec<DeepSeekCostItem>,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct DeepSeekCostItem {
    #[serde(default)]
    model: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    cost: FlexibleF64,
}

#[derive(Debug, Deserialize, Clone, Copy, Default)]
struct FlexibleF64(#[serde(deserialize_with = "deserialize_f64")] f64);

#[derive(Debug, Deserialize, Clone, Copy)]
struct FlexibleI64(#[serde(deserialize_with = "deserialize_i64")] i64);

#[derive(Debug, Clone, Default, PartialEq)]
struct DeepSeekUsageSummary {
    today_tokens: f64,
    month_tokens: f64,
    today_requests: f64,
    month_requests: f64,
    today_cost: f64,
    month_cost: f64,
    top_model: Option<String>,
    category_tokens: Vec<(String, f64)>,
}

pub struct DeepSeekProvider {
    metadata: ProviderMetadata,
    client: Client,
}

impl DeepSeekProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::DeepSeek,
                display_name: "DeepSeek",
                session_label: "Balance",
                weekly_label: "Balance",
                supports_opus: false,
                supports_credits: true,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://platform.deepseek.com/usage"),
                status_page_url: Some("https://status.deepseek.com"),
            },
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    fn get_api_key(api_key: Option<&str>) -> Result<String, ProviderError> {
        if let Some(key) = api_key
            && !key.trim().is_empty()
        {
            return Ok(key.trim().to_string());
        }

        if let Ok(entry) = keyring::Entry::new(DEEPSEEK_CREDENTIAL_TARGET, "api_key")
            && let Ok(token) = entry.get_password()
            && !token.trim().is_empty()
        {
            return Ok(token);
        }

        for env in ["DEEPSEEK_API_KEY", "DEEPSEEK_KEY"] {
            if let Ok(token) = std::env::var(env)
                && !token.trim().is_empty()
            {
                return Ok(token);
            }
        }

        Err(ProviderError::NotInstalled(
            "DeepSeek API key not found. Set it in Preferences → Providers, DEEPSEEK_API_KEY, or DEEPSEEK_KEY."
                .to_string(),
        ))
    }

    async fn fetch_usage_api(
        &self,
        ctx: &FetchContext,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let api_key = Self::get_api_key(ctx.api_key.as_deref())?;

        let resp = self
            .client
            .get(format!("{DEEPSEEK_API_BASE}/user/balance"))
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Accept", "application/json")
            .send()
            .await?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ProviderError::AuthRequired);
        }
        if !resp.status().is_success() {
            return Err(ProviderError::Other(format!(
                "DeepSeek API returned status {}",
                resp.status()
            )));
        }

        let balance: BalanceResponse = resp.json().await.map_err(|e| {
            ProviderError::Parse(format!("Failed to parse DeepSeek balance response: {e}"))
        })?;
        let mut usage = Self::snapshot_from_balance(balance);
        let mut result = ProviderFetchResult::new(usage.clone(), "api");

        if let Ok(summary) = self.fetch_usage_summary(&api_key).await {
            usage = Self::apply_usage_summary(usage, &summary);
            result = ProviderFetchResult::new(usage, "api");
            if summary.month_cost > 0.0 {
                result = result.with_cost(CostSnapshot::new(
                    summary.month_cost,
                    "USD",
                    "Current month",
                ));
            }
        }

        Ok(result)
    }

    async fn fetch_usage_summary(
        &self,
        api_key: &str,
    ) -> Result<DeepSeekUsageSummary, ProviderError> {
        let amount: UsageEnvelope<DeepSeekAmountData> = self
            .fetch_usage_endpoint("/api/v0/usage/amount", api_key, "amount")
            .await?;
        let cost: UsageEnvelope<DeepSeekCostData> = self
            .fetch_usage_endpoint("/api/v0/usage/cost", api_key, "cost")
            .await?;
        DeepSeekUsageSummary::from_payloads(amount, cost)
    }

    async fn fetch_usage_endpoint<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        api_key: &str,
        label: &str,
    ) -> Result<T, ProviderError> {
        let resp = self
            .client
            .get(format!("https://platform.deepseek.com{path}"))
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Accept", "application/json")
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ProviderError::AuthRequired);
        }
        if !resp.status().is_success() {
            return Err(ProviderError::Other(format!(
                "DeepSeek usage {label} returned status {}",
                resp.status()
            )));
        }
        resp.json().await.map_err(|e| {
            ProviderError::Parse(format!("Failed to parse DeepSeek usage {label}: {e}"))
        })
    }

    fn snapshot_from_balance(balance: BalanceResponse) -> UsageSnapshot {
        let selected = select_balance_info(&balance.balance_infos).cloned();

        let Some(info) = selected else {
            let mut window = RateWindow::new(100.0);
            window.reset_description = Some("No balance information returned".to_string());
            return UsageSnapshot::new(window).with_login_method("Balance unavailable");
        };

        let total = parse_money(&info.total_balance);
        let granted = parse_money(&info.granted_balance);
        let topped_up = parse_money(&info.topped_up_balance);
        let symbol = currency_symbol(&info.currency);

        let mut window = RateWindow::new(if !balance.is_available || total <= 0.0 {
            100.0
        } else {
            0.0
        });

        window.reset_description = if !balance.is_available {
            Some("Balance unavailable for API calls".to_string())
        } else if total <= 0.0 {
            Some(format!(
                "{symbol}0.00 — add credits at platform.deepseek.com"
            ))
        } else {
            Some(format!(
                "{symbol}{total:.2} (Paid: {symbol}{topped_up:.2} / Granted: {symbol}{granted:.2})"
            ))
        };

        UsageSnapshot::new(window).with_login_method(format!(
            "{} balance: {symbol}{total:.2}",
            info.currency.to_uppercase()
        ))
    }

    fn apply_usage_summary(
        mut usage: UsageSnapshot,
        summary: &DeepSeekUsageSummary,
    ) -> UsageSnapshot {
        if summary.today_tokens > 0.0 {
            usage = usage.with_extra_rate_window(
                "tokens-today",
                "Tokens today",
                RateWindow::with_details(
                    0.0,
                    Some(24 * 60),
                    None,
                    Some(format_count(summary.today_tokens)),
                ),
            );
        }
        if summary.month_tokens > 0.0 {
            usage = usage.with_extra_rate_window(
                "tokens-month",
                "Tokens (month)",
                RateWindow::with_details(0.0, None, None, Some(format_count(summary.month_tokens))),
            );
        }
        if summary.today_requests > 0.0 {
            usage = usage.with_extra_rate_window(
                "requests-today",
                "Requests today",
                RateWindow::with_details(
                    0.0,
                    Some(24 * 60),
                    None,
                    Some(format_count(summary.today_requests)),
                ),
            );
        }
        if summary.month_requests > 0.0 {
            usage = usage.with_extra_rate_window(
                "requests-month",
                "Requests (month)",
                RateWindow::with_details(
                    0.0,
                    None,
                    None,
                    Some(format_count(summary.month_requests)),
                ),
            );
        }
        if let Some(model) = summary.top_model.as_deref() {
            usage = usage.with_extra_rate_window(
                "top-model",
                "Top model",
                RateWindow::with_details(0.0, None, None, Some(model.to_string())),
            );
        }
        for (idx, (category, tokens)) in summary.category_tokens.iter().take(3).enumerate() {
            usage = usage.with_extra_rate_window(
                format!("category-{idx}"),
                format!("Category: {}", category_label(category)),
                RateWindow::with_details(0.0, None, None, Some(format_count(*tokens))),
            );
        }
        usage
    }
}

impl DeepSeekUsageSummary {
    fn from_payloads(
        amount: UsageEnvelope<DeepSeekAmountData>,
        cost: UsageEnvelope<DeepSeekCostData>,
    ) -> Result<Self, ProviderError> {
        validate_usage_envelope(&amount, "amount")?;
        validate_usage_envelope(&cost, "cost")?;

        let amount = amount
            .data
            .and_then(|data| data.biz_data)
            .ok_or_else(|| ProviderError::Parse("Missing DeepSeek amount data".to_string()))?;
        let cost = cost
            .data
            .and_then(|data| data.biz_data)
            .ok_or_else(|| ProviderError::Parse("Missing DeepSeek cost data".to_string()))?;

        let today = latest_amount_day(&amount.days);
        let today_cost_day = latest_cost_day(&cost.days);
        let mut model_tokens: HashMap<String, f64> = HashMap::new();
        let mut category_tokens: HashMap<String, f64> = HashMap::new();

        for item in &amount.total {
            if !item.model.trim().is_empty() {
                *model_tokens.entry(item.model.clone()).or_default() += item.amount.0;
            }
            if !item.category.trim().is_empty() {
                *category_tokens.entry(item.category.clone()).or_default() += item.amount.0;
            }
        }

        let top_model = model_tokens
            .into_iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(model, _)| model);
        let mut category_tokens = category_tokens.into_iter().collect::<Vec<_>>();
        category_tokens.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        Ok(Self {
            today_tokens: today.map(sum_non_request_amount).unwrap_or_default(),
            month_tokens: amount
                .total
                .iter()
                .filter(|item| !is_request_category(&item.category))
                .map(|item| item.amount.0)
                .sum(),
            today_requests: today.map(sum_request_amount).unwrap_or_default(),
            month_requests: amount
                .total
                .iter()
                .filter(|item| is_request_category(&item.category))
                .map(|item| item.amount.0)
                .sum(),
            today_cost: today_cost_day.map(sum_cost).unwrap_or_default(),
            month_cost: cost.total.iter().map(|item| item.cost.0).sum(),
            top_model,
            category_tokens,
        })
    }
}

impl Default for DeepSeekProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for DeepSeekProvider {
    fn id(&self) -> ProviderId {
        ProviderId::DeepSeek
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

fn parse_money(value: &str) -> f64 {
    value.parse::<f64>().unwrap_or(0.0)
}

fn select_balance_info(balance_infos: &[BalanceInfo]) -> Option<&BalanceInfo> {
    balance_infos
        .iter()
        .find(|info| {
            info.currency.eq_ignore_ascii_case("USD") && parse_money(&info.total_balance) > 0.0
        })
        .or_else(|| {
            balance_infos
                .iter()
                .find(|info| parse_money(&info.total_balance) > 0.0)
        })
        .or_else(|| {
            balance_infos
                .iter()
                .find(|info| info.currency.eq_ignore_ascii_case("USD"))
        })
        .or_else(|| balance_infos.first())
}

fn validate_usage_envelope<T>(
    envelope: &UsageEnvelope<T>,
    label: &str,
) -> Result<(), ProviderError> {
    if envelope
        .code
        .map(|code| code.0)
        .is_some_and(|code| code != 0)
    {
        return Err(ProviderError::Other(format!(
            "DeepSeek usage {label} error: {}",
            envelope
                .msg
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        )));
    }
    if envelope
        .data
        .as_ref()
        .and_then(|data| data.biz_code.map(|code| code.0))
        .is_some_and(|code| code != 0)
    {
        return Err(ProviderError::Other(format!(
            "DeepSeek usage {label} business error"
        )));
    }
    Ok(())
}

fn latest_amount_day(days: &[DeepSeekAmountDay]) -> Option<&[DeepSeekAmountItem]> {
    days.iter()
        .max_by(|a, b| a.date.cmp(&b.date))
        .map(|day| day.models.as_slice())
}

fn latest_cost_day(days: &[DeepSeekCostDay]) -> Option<&[DeepSeekCostItem]> {
    days.iter()
        .max_by(|a, b| a.date.cmp(&b.date))
        .map(|day| day.models.as_slice())
}

fn is_request_category(category: &str) -> bool {
    category.eq_ignore_ascii_case("REQUEST")
}

fn sum_non_request_amount(items: &[DeepSeekAmountItem]) -> f64 {
    items
        .iter()
        .filter(|item| !is_request_category(&item.category))
        .map(|item| item.amount.0)
        .sum()
}

fn sum_request_amount(items: &[DeepSeekAmountItem]) -> f64 {
    items
        .iter()
        .filter(|item| is_request_category(&item.category))
        .map(|item| item.amount.0)
        .sum()
}

fn sum_cost(items: &[DeepSeekCostItem]) -> f64 {
    items.iter().map(|item| item.cost.0).sum()
}

fn category_label(category: &str) -> String {
    match category {
        "PROMPT_CACHE_HIT_TOKEN" => "Prompt cache hit tokens".to_string(),
        "PROMPT_CACHE_MISS_TOKEN" => "Prompt cache miss tokens".to_string(),
        "RESPONSE_TOKEN" => "Response tokens".to_string(),
        "REQUEST" => "Requests".to_string(),
        other => other.replace('_', " ").to_lowercase(),
    }
}

fn format_count(value: f64) -> String {
    let rounded = value.round();
    if (value - rounded).abs() < 0.01 {
        let raw = (rounded as i64).to_string();
        let mut output = String::new();
        for (idx, ch) in raw.chars().rev().enumerate() {
            if idx > 0 && idx % 3 == 0 {
                output.push(',');
            }
            output.push(ch);
        }
        output.chars().rev().collect()
    } else {
        format!("{value:.2}")
    }
}

fn deserialize_f64<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Number(n) => n
            .as_f64()
            .ok_or_else(|| serde::de::Error::custom("invalid number")),
        serde_json::Value::String(s) => s
            .replace(',', "")
            .parse::<f64>()
            .map_err(|_| serde::de::Error::custom("invalid number string")),
        _ => Ok(0.0),
    }
}

fn deserialize_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Number(n) => n
            .as_i64()
            .ok_or_else(|| serde::de::Error::custom("invalid integer")),
        serde_json::Value::String(s) => s
            .parse::<i64>()
            .map_err(|_| serde::de::Error::custom("invalid integer string")),
        _ => Ok(0),
    }
}

fn currency_symbol(currency: &str) -> &'static str {
    match currency.to_uppercase().as_str() {
        "CNY" | "RMB" => "¥",
        _ => "$",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_usd_balance_and_formats_paid_and_granted() {
        let snapshot = DeepSeekProvider::snapshot_from_balance(BalanceResponse {
            is_available: true,
            balance_infos: vec![
                BalanceInfo {
                    currency: "CNY".into(),
                    total_balance: "10".into(),
                    granted_balance: "1".into(),
                    topped_up_balance: "9".into(),
                },
                BalanceInfo {
                    currency: "USD".into(),
                    total_balance: "3.50".into(),
                    granted_balance: "0.50".into(),
                    topped_up_balance: "3.00".into(),
                },
            ],
        });

        assert_eq!(snapshot.primary.used_percent, 0.0);
        assert_eq!(
            snapshot.primary.reset_description.as_deref(),
            Some("$3.50 (Paid: $3.00 / Granted: $0.50)")
        );
    }

    #[test]
    fn uses_cny_balance_when_usd_is_empty() {
        let snapshot = DeepSeekProvider::snapshot_from_balance(BalanceResponse {
            is_available: true,
            balance_infos: vec![
                BalanceInfo {
                    currency: "USD".into(),
                    total_balance: "0".into(),
                    granted_balance: "0".into(),
                    topped_up_balance: "0".into(),
                },
                BalanceInfo {
                    currency: "CNY".into(),
                    total_balance: "42.25".into(),
                    granted_balance: "2.25".into(),
                    topped_up_balance: "40".into(),
                },
            ],
        });

        assert_eq!(snapshot.primary.used_percent, 0.0);
        assert_eq!(
            snapshot.primary.reset_description.as_deref(),
            Some("¥42.25 (Paid: ¥40.00 / Granted: ¥2.25)")
        );
        assert_eq!(
            snapshot.login_method.as_deref(),
            Some("CNY balance: ¥42.25")
        );
    }

    #[test]
    fn keeps_zero_usd_when_no_currency_has_balance() {
        let snapshot = DeepSeekProvider::snapshot_from_balance(BalanceResponse {
            is_available: true,
            balance_infos: vec![
                BalanceInfo {
                    currency: "CNY".into(),
                    total_balance: "0".into(),
                    granted_balance: "0".into(),
                    topped_up_balance: "0".into(),
                },
                BalanceInfo {
                    currency: "USD".into(),
                    total_balance: "0".into(),
                    granted_balance: "0".into(),
                    topped_up_balance: "0".into(),
                },
            ],
        });

        assert_eq!(snapshot.primary.used_percent, 100.0);
        assert_eq!(
            snapshot.primary.reset_description.as_deref(),
            Some("$0.00 — add credits at platform.deepseek.com")
        );
        assert_eq!(snapshot.login_method.as_deref(), Some("USD balance: $0.00"));
    }

    #[test]
    fn exhausted_when_balance_unavailable() {
        let snapshot = DeepSeekProvider::snapshot_from_balance(BalanceResponse {
            is_available: false,
            balance_infos: vec![BalanceInfo {
                currency: "USD".into(),
                total_balance: "1".into(),
                granted_balance: "1".into(),
                topped_up_balance: "0".into(),
            }],
        });

        assert_eq!(snapshot.primary.used_percent, 100.0);
        assert_eq!(
            snapshot.primary.reset_description.as_deref(),
            Some("Balance unavailable for API calls")
        );
    }

    #[test]
    fn parses_deepseek_usage_summary_payloads() {
        let amount: UsageEnvelope<DeepSeekAmountData> = serde_json::from_value(serde_json::json!({
            "code": 0,
            "data": {
                "biz_code": "0",
                "biz_data": {
                    "total": [
                        {"model": "deepseek-chat", "category": "PROMPT_CACHE_HIT_TOKEN", "amount": "100"},
                        {"model": "deepseek-chat", "category": "RESPONSE_TOKEN", "amount": 50},
                        {"model": "deepseek-reasoner", "category": "REQUEST", "amount": "3"}
                    ],
                    "days": [
                        {"date": "2026-05-26", "models": [{"model": "deepseek-chat", "category": "RESPONSE_TOKEN", "amount": 1}]},
                        {"date": "2026-05-27", "models": [
                            {"model": "deepseek-chat", "category": "PROMPT_CACHE_MISS_TOKEN", "amount": "20"},
                            {"model": "deepseek-chat", "category": "REQUEST", "amount": "2"}
                        ]}
                    ]
                }
            }
        })).unwrap();
        let cost: UsageEnvelope<DeepSeekCostData> = serde_json::from_value(serde_json::json!({
            "code": "0",
            "data": {
                "biz_code": 0,
                "biz_data": {
                    "total": [{"model": "deepseek-chat", "category": "RESPONSE_TOKEN", "cost": "1.25"}],
                    "days": [{"date": "2026-05-27", "models": [{"model": "deepseek-chat", "category": "RESPONSE_TOKEN", "cost": "0.10"}]}]
                }
            }
        })).unwrap();
        let summary = DeepSeekUsageSummary::from_payloads(amount, cost).unwrap();
        assert_eq!(summary.month_tokens, 150.0);
        assert_eq!(summary.today_tokens, 20.0);
        assert_eq!(summary.month_requests, 3.0);
        assert_eq!(summary.today_requests, 2.0);
        assert_eq!(summary.month_cost, 1.25);
        assert_eq!(summary.today_cost, 0.10);
        assert_eq!(summary.top_model.as_deref(), Some("deepseek-chat"));
    }

    #[test]
    fn applies_deepseek_summary_as_extra_windows() {
        let usage = UsageSnapshot::new(RateWindow::new(0.0));
        let usage = DeepSeekProvider::apply_usage_summary(
            usage,
            &DeepSeekUsageSummary {
                today_tokens: 20.0,
                month_tokens: 150.0,
                today_requests: 2.0,
                month_requests: 3.0,
                today_cost: 0.10,
                month_cost: 1.25,
                top_model: Some("deepseek-chat".to_string()),
                category_tokens: vec![("RESPONSE_TOKEN".to_string(), 50.0)],
            },
        );
        assert!(
            usage
                .extra_rate_windows
                .iter()
                .any(|window| window.id == "tokens-today")
        );
        assert!(
            usage
                .extra_rate_windows
                .iter()
                .any(|window| window.title == "Top model")
        );
    }
}
