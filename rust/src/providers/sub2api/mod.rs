//! sub2api provider implementation.
//!
//! Mirrors upstream v0.43.0: fetch group key quota, subscription limits,
//! usage totals, and wallet balance from `GET /v1/usage`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::{Client, Url};
use serde::Deserialize;

use crate::core::{
    CostSnapshot, FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId,
    ProviderMetadata, RateWindow, SourceMode, UsageSnapshot,
};

const CREDENTIAL_TARGET: &str = "codexbar-sub2api";
const API_KEY_ENV: &str = "SUB2API_API_KEY";
const BASE_URL_ENV: &str = "SUB2API_BASE_URL";

pub struct Sub2ApiProvider {
    metadata: ProviderMetadata,
    client: Client,
}

#[derive(Debug, Clone, PartialEq)]
struct ParsedUsage {
    mode: String,
    is_valid: bool,
    plan_name: Option<String>,
    unit: String,
    balance: Option<f64>,
    /// Top-level remaining (wallet-ish) when balance/quota are absent.
    remaining: Option<f64>,
    quota: Option<ParsedQuota>,
    rate_limits: Vec<ParsedRateLimit>,
    subscription: Option<ParsedSubscription>,
    today: Option<ParsedTotals>,
    total: Option<ParsedTotals>,
    expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
struct ParsedQuota {
    limit: f64,
    used: f64,
    remaining: f64,
    unit: String,
}

#[derive(Debug, Clone, PartialEq)]
struct ParsedRateLimit {
    window: String,
    limit: f64,
    used: f64,
    remaining: f64,
    reset_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
struct ParsedSubscription {
    daily_usage_usd: f64,
    weekly_usage_usd: f64,
    monthly_usage_usd: f64,
    daily_limit_usd: Option<f64>,
    weekly_limit_usd: Option<f64>,
    monthly_limit_usd: Option<f64>,
    expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
struct ParsedTotals {
    requests: i64,
    total_tokens: i64,
    actual_cost_usd: f64,
}

#[derive(Debug, Deserialize)]
struct UsageResponse {
    mode: Option<String>,
    #[serde(rename = "isValid", default)]
    is_valid: Option<bool>,
    #[serde(rename = "planName")]
    plan_name: Option<String>,
    /// Wallet-ish remaining when balance/quota are absent.
    remaining: Option<f64>,
    unit: Option<String>,
    balance: Option<f64>,
    quota: Option<QuotaResponse>,
    #[serde(default, rename = "rate_limits")]
    rate_limits: Option<Vec<RateLimitResponse>>,
    subscription: Option<SubscriptionResponse>,
    usage: Option<UsageBlockResponse>,
    #[serde(rename = "expires_at")]
    expires_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QuotaResponse {
    limit: f64,
    used: f64,
    remaining: f64,
    unit: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RateLimitResponse {
    window: String,
    limit: f64,
    used: f64,
    remaining: f64,
    #[serde(rename = "reset_at")]
    reset_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SubscriptionResponse {
    #[serde(rename = "daily_usage_usd")]
    daily_usage_usd: Option<f64>,
    #[serde(rename = "weekly_usage_usd")]
    weekly_usage_usd: Option<f64>,
    #[serde(rename = "monthly_usage_usd")]
    monthly_usage_usd: Option<f64>,
    #[serde(rename = "daily_limit_usd")]
    daily_limit_usd: Option<f64>,
    #[serde(rename = "weekly_limit_usd")]
    weekly_limit_usd: Option<f64>,
    #[serde(rename = "monthly_limit_usd")]
    monthly_limit_usd: Option<f64>,
    #[serde(rename = "expires_at")]
    expires_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UsageBlockResponse {
    today: Option<TotalsResponse>,
    total: Option<TotalsResponse>,
}

#[derive(Debug, Deserialize)]
struct TotalsResponse {
    requests: Option<i64>,
    #[serde(rename = "total_tokens")]
    total_tokens: Option<i64>,
    #[serde(rename = "actual_cost")]
    actual_cost: Option<f64>,
}

impl Sub2ApiProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Sub2Api,
                display_name: "sub2api",
                session_label: "Quota",
                weekly_label: "Weekly quota",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: None,
                status_page_url: None,
            },
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    async fn fetch_api(
        &self,
        api_key: &str,
        base_url: &Url,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let request_url = usage_request_url(base_url)?;
        let response = self
            .client
            .get(request_url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Accept", "application/json")
            .send()
            .await?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ProviderError::AuthRequired);
        }
        if !status.is_success() {
            return Err(ProviderError::Other(format!(
                "sub2api usage returned status {status}"
            )));
        }

        let body = response.text().await.map_err(|e| {
            ProviderError::Parse(format!("Failed to read sub2api usage response: {e}"))
        })?;
        let parsed = parse_usage_body(&body)?;
        if !parsed.is_valid {
            return Err(ProviderError::AuthRequired);
        }
        Ok(snapshot_from_parsed(parsed))
    }
}

impl Default for Sub2ApiProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for Sub2ApiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Sub2Api
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto | SourceMode::OAuth => {
                let key = resolve_api_key(ctx)?;
                let base = resolve_base_url(ctx)?;
                self.fetch_api(&key, &base).await
            }
            SourceMode::Web | SourceMode::Cli => {
                Err(ProviderError::UnsupportedSource(ctx.source_mode))
            }
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        // Upstream: Auto + API. Win maps API-token sources to OAuth.
        vec![SourceMode::Auto, SourceMode::OAuth]
    }
}

fn resolve_api_key(ctx: &FetchContext) -> Result<String, ProviderError> {
    let key = crate::providers::resolve_api_key(
        ctx.api_key.as_deref(),
        CREDENTIAL_TARGET,
        &[API_KEY_ENV],
    )?;
    let cleaned = clean_env_value(&key).ok_or(ProviderError::AuthRequired)?;
    Ok(cleaned)
}

fn resolve_base_url(ctx: &FetchContext) -> Result<Url, ProviderError> {
    if let Some(base) = ctx
        .workspace_id
        .as_deref()
        .and_then(clean_env_value)
        .filter(|value| !value.is_empty())
    {
        return validated_sub2api_base_url(&base);
    }

    let raw = std::env::var(BASE_URL_ENV)
        .ok()
        .and_then(|value| clean_env_value(&value))
        .ok_or_else(|| {
            ProviderError::NotInstalled(
                "Missing or invalid sub2api base URL. Add one in Settings or set SUB2API_BASE_URL."
                    .into(),
            )
        })?;
    validated_sub2api_base_url(&raw)
}

/// Strip whitespace and surrounding quotes from env/settings values.
pub(crate) fn clean_env_value(raw: &str) -> Option<String> {
    let mut value = raw.trim().to_string();
    if value.is_empty() {
        return None;
    }
    if (value.starts_with('"') && value.ends_with('"'))
        || (value.starts_with('\'') && value.ends_with('\''))
    {
        value = value[1..value.len().saturating_sub(1)].to_string();
        value = value.trim().to_string();
    }
    if value.is_empty() { None } else { Some(value) }
}

/// Validate a sub2api base URL: HTTPS or loopback HTTP, no userinfo/query/fragment.
pub(crate) fn validated_sub2api_base_url(raw: &str) -> Result<Url, ProviderError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ProviderError::Other(
            "sub2api base URL must use HTTPS, or loopback HTTP for local development, without embedded credentials."
                .into(),
        ));
    }
    let lower = trimmed.to_ascii_lowercase();
    if ["%2f", "%5c", "%3f", "%23", "%40", "%3a"]
        .iter()
        .any(|encoded| lower.contains(encoded))
    {
        return Err(ProviderError::Other(
            "sub2api base URL must not contain encoded host delimiters".into(),
        ));
    }

    let url = Url::parse(trimmed).map_err(|e| {
        ProviderError::Other(format!(
            "Missing or invalid sub2api base URL ({e}). Add one in Settings or set SUB2API_BASE_URL."
        ))
    })?;

    let host = url
        .host_str()
        .ok_or_else(|| ProviderError::Other("sub2api base URL must include a host".into()))?;

    let scheme_ok = match url.scheme() {
        "https" => true,
        "http" => is_loopback_host(host),
        _ => false,
    };

    if !scheme_ok
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || host.contains('%')
        || host.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(ProviderError::Other(
            "sub2api base URL must use HTTPS, or loopback HTTP for local development, without embedded credentials."
                .into(),
        ));
    }

    Ok(url)
}

fn is_loopback_host(host: &str) -> bool {
    let normalized = host.trim_end_matches('.').to_ascii_lowercase();
    if normalized == "localhost" {
        return true;
    }
    let ip_candidate = normalized
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(&normalized);
    match ip_candidate.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => ip.is_loopback(),
        Ok(std::net::IpAddr::V6(ip)) => ip.is_loopback(),
        Err(_) => false,
    }
}

/// Join `{base}` with `/v1/usage` following upstream path rules.
pub(crate) fn usage_url(base_url: &Url) -> Result<Url, ProviderError> {
    let path = base_url.path().trim_end_matches('/');
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    if segments.len() >= 2 && segments[segments.len() - 2..] == ["v1", "usage"] {
        // Normalize trailing slash differences while keeping path intact.
        let mut exact = base_url.clone();
        exact.set_path(&format!("/{}", segments.join("/")));
        exact.set_query(None);
        exact.set_fragment(None);
        return Ok(exact);
    }

    let mut joined = base_url.clone();
    joined.set_query(None);
    joined.set_fragment(None);
    if segments.last() == Some(&"v1") {
        joined.set_path(&format!("{path}/usage"));
        return Ok(joined);
    }

    if path.is_empty() {
        joined.set_path("/v1/usage");
    } else {
        joined.set_path(&format!("{path}/v1/usage"));
    }
    Ok(joined)
}

fn usage_request_url(base_url: &Url) -> Result<Url, ProviderError> {
    let mut url = usage_url(base_url)?;
    let timezone = local_timezone_identifier();
    url.query_pairs_mut()
        .append_pair("days", "30")
        .append_pair("timezone", &timezone);
    Ok(url)
}

fn local_timezone_identifier() -> String {
    crate::core::local_timezone_name()
}

fn parse_usage_body(body: &str) -> Result<ParsedUsage, ProviderError> {
    let response: UsageResponse = serde_json::from_str(body)
        .map_err(|e| ProviderError::Parse(format!("Could not parse sub2api usage: {e}")))?;
    Ok(parse_usage_response(response))
}

fn parse_usage_response(response: UsageResponse) -> ParsedUsage {
    let unit = response
        .unit
        .clone()
        .or_else(|| response.quota.as_ref().and_then(|q| q.unit.clone()))
        .unwrap_or_else(|| "USD".to_string());

    ParsedUsage {
        mode: response.mode.unwrap_or_else(|| "unknown".to_string()),
        is_valid: response.is_valid.unwrap_or(true),
        plan_name: response.plan_name,
        unit: unit.clone(),
        balance: response.balance,
        remaining: response.remaining.filter(|r| r.is_finite()),
        quota: response.quota.map(|q| ParsedQuota {
            limit: q.limit,
            used: q.used,
            remaining: q.remaining,
            unit: q.unit.unwrap_or_else(|| unit.clone()),
        }),
        rate_limits: response
            .rate_limits
            .unwrap_or_default()
            .into_iter()
            .map(|rl| ParsedRateLimit {
                window: rl.window,
                limit: rl.limit,
                used: rl.used,
                remaining: rl.remaining,
                reset_at: parse_date(rl.reset_at.as_deref()),
            })
            .collect(),
        subscription: response.subscription.map(|s| ParsedSubscription {
            daily_usage_usd: s.daily_usage_usd.unwrap_or(0.0),
            weekly_usage_usd: s.weekly_usage_usd.unwrap_or(0.0),
            monthly_usage_usd: s.monthly_usage_usd.unwrap_or(0.0),
            daily_limit_usd: s.daily_limit_usd,
            weekly_limit_usd: s.weekly_limit_usd,
            monthly_limit_usd: s.monthly_limit_usd,
            expires_at: parse_date(s.expires_at.as_deref()),
        }),
        today: usage_totals(response.usage.as_ref().and_then(|u| u.today.as_ref())),
        total: usage_totals(response.usage.as_ref().and_then(|u| u.total.as_ref())),
        expires_at: parse_date(response.expires_at.as_deref()),
    }
}

fn usage_totals(totals: Option<&TotalsResponse>) -> Option<ParsedTotals> {
    totals.map(|t| ParsedTotals {
        requests: t.requests.unwrap_or(0),
        total_tokens: t.total_tokens.unwrap_or(0),
        actual_cost_usd: t.actual_cost.unwrap_or(0.0),
    })
}

fn parse_date(raw: Option<&str>) -> Option<DateTime<Utc>> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            // Accept fractional-second variants serde/chrono sometimes miss.
            chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.fZ")
                .ok()
                .map(|naive| naive.and_utc())
        })
}

fn snapshot_from_parsed(parsed: ParsedUsage) -> ProviderFetchResult {
    let kind = classify_usage_kind(&parsed);
    let mut cost: Option<CostSnapshot> = None;

    let mut snapshot = match kind {
        Kind::Subscription => {
            let sub = parsed
                .subscription
                .as_ref()
                .expect("subscription kind requires subscription payload");
            let primary = rate_window_from_usage(
                sub.daily_usage_usd,
                sub.daily_limit_usd,
                Some(24 * 60),
                &parsed.unit,
            )
            .or_else(|| informational_usage_window(sub.daily_usage_usd, "day", &parsed.unit));
            let secondary = rate_window_from_usage(
                sub.weekly_usage_usd,
                sub.weekly_limit_usd,
                Some(7 * 24 * 60),
                &parsed.unit,
            )
            .or_else(|| informational_usage_window(sub.weekly_usage_usd, "week", &parsed.unit));
            let tertiary = rate_window_from_usage(
                sub.monthly_usage_usd,
                sub.monthly_limit_usd,
                Some(30 * 24 * 60),
                &parsed.unit,
            )
            .or_else(|| informational_usage_window(sub.monthly_usage_usd, "month", &parsed.unit));

            let mut snap = UsageSnapshot::new(
                primary.unwrap_or_else(|| RateWindow::informational("Subscription active")),
            );
            if let Some(secondary) = secondary {
                snap = snap.with_secondary(secondary);
            }
            if let Some(tertiary) = tertiary {
                snap = snap.with_tertiary(tertiary);
            }
            // Surface wallet balance alongside subscription when present.
            if let Some(balance) = parsed.balance {
                cost = Some(
                    CostSnapshot::new(0.0, normalize_currency(&parsed.unit), "balance")
                        .with_limit(balance.max(0.0)),
                );
            }
            snap
        }
        Kind::KeyQuota => {
            let mut snap = if let Some(quota) = &parsed.quota {
                UsageSnapshot::new(RateWindow::with_details(
                    used_percent(quota.used, quota.limit),
                    None,
                    None,
                    Some(amount_description(quota.used, quota.limit, &quota.unit)),
                ))
            } else if let Some(first) = parsed.rate_limits.first() {
                // Rate-limit-only payload: promote the first window to primary.
                UsageSnapshot::new(RateWindow::with_details(
                    used_percent(first.used, first.limit),
                    window_minutes(&first.window),
                    first.reset_at,
                    Some(amount_description(first.used, first.limit, &parsed.unit)),
                ))
            } else {
                UsageSnapshot::new(RateWindow::informational("Key quota"))
            };
            let skip_first_rate_limit = parsed.quota.is_none() && !parsed.rate_limits.is_empty();
            for (idx, rate_limit) in parsed.rate_limits.iter().enumerate() {
                if skip_first_rate_limit && idx == 0 {
                    continue;
                }
                snap = snap.with_extra_rate_window(
                    rate_limit.window.clone(),
                    rate_limit_title(&rate_limit.window),
                    RateWindow::with_details(
                        used_percent(rate_limit.used, rate_limit.limit),
                        window_minutes(&rate_limit.window),
                        rate_limit.reset_at,
                        Some(amount_description(
                            rate_limit.used,
                            rate_limit.limit,
                            &parsed.unit,
                        )),
                    ),
                );
            }
            snap
        }
        Kind::Wallet => {
            let balance = parsed
                .balance
                .or_else(|| wallet_remaining(&parsed))
                .unwrap_or(0.0);
            let description = format!("{} balance", currency_string(balance, &parsed.unit));
            cost = Some(
                CostSnapshot::new(0.0, normalize_currency(&parsed.unit), "balance")
                    .with_limit(balance.max(0.0)),
            );
            UsageSnapshot::new(RateWindow::informational(description))
        }
        Kind::Unknown => {
            // Prefer any totals over a blank "No quota data" row.
            if let Some(today) = &parsed.today {
                UsageSnapshot::new(RateWindow::informational(totals_description(
                    today,
                    &parsed.unit,
                )))
            } else if let Some(total) = &parsed.total {
                UsageSnapshot::new(RateWindow::informational(totals_description(
                    total,
                    &parsed.unit,
                )))
            } else {
                UsageSnapshot::new(RateWindow::informational("No quota data"))
            }
        }
    };

    // Rate-limit extras for subscription/wallet kinds that also ship them.
    if kind != Kind::KeyQuota {
        for rate_limit in &parsed.rate_limits {
            snapshot = snapshot.with_extra_rate_window(
                rate_limit.window.clone(),
                rate_limit_title(&rate_limit.window),
                RateWindow::with_details(
                    used_percent(rate_limit.used, rate_limit.limit),
                    window_minutes(&rate_limit.window),
                    rate_limit.reset_at,
                    Some(amount_description(
                        rate_limit.used,
                        rate_limit.limit,
                        &parsed.unit,
                    )),
                ),
            );
        }
    }

    if let Some(today) = &parsed.today {
        // Avoid duplicating the primary informational totals row.
        let already_primary = kind == Kind::Unknown
            && snapshot
                .primary
                .reset_description
                .as_deref()
                .is_some_and(|d| d == totals_description(today, &parsed.unit));
        if !already_primary {
            snapshot = snapshot.with_extra_rate_window(
                "today",
                "Today",
                RateWindow::informational(totals_description(today, &parsed.unit)),
            );
        }
    }
    if let Some(total) = &parsed.total {
        snapshot = snapshot.with_extra_rate_window(
            "total",
            "Total",
            RateWindow::informational(totals_description(total, &parsed.unit)),
        );
    }

    // Plan is a tier/login label, not organization identity.
    if let Some(plan) = parsed.plan_name.as_ref().filter(|p| !p.trim().is_empty()) {
        let mut login = plan.clone();
        if !parsed.mode.is_empty()
            && parsed.mode != "unknown"
            && !plan.eq_ignore_ascii_case(&parsed.mode)
        {
            login = format!("{plan} ({})", parsed.mode);
        }
        snapshot = snapshot.with_login_method(login);
    } else if !parsed.mode.is_empty() && parsed.mode != "unknown" {
        snapshot = snapshot.with_login_method(parsed.mode.clone());
    }

    let expires = parsed
        .subscription
        .as_ref()
        .and_then(|s| s.expires_at)
        .or(parsed.expires_at);
    if let Some(expires_at) = expires {
        snapshot = snapshot.with_extra_rate_window(
            "expires",
            "Expires",
            RateWindow::with_details(
                0.0,
                None,
                Some(expires_at),
                Some(expires_at.format("%Y-%m-%d").to_string()),
            ),
        );
    }

    let mut result = ProviderFetchResult::new(snapshot, "api");
    if let Some(cost) = cost {
        result = result.with_cost(cost);
    }
    result
}

fn classify_usage_kind(parsed: &ParsedUsage) -> Kind {
    if parsed.subscription.is_some() {
        Kind::Subscription
    } else if parsed.quota.is_some() || !parsed.rate_limits.is_empty() {
        Kind::KeyQuota
    } else if parsed.balance.is_some() || wallet_remaining(parsed).is_some() {
        Kind::Wallet
    } else {
        Kind::Unknown
    }
}

/// Top-level remaining as a wallet signal only when quota/subscription are absent.
fn wallet_remaining(parsed: &ParsedUsage) -> Option<f64> {
    if parsed.quota.is_some() || parsed.subscription.is_some() {
        return None;
    }
    parsed.remaining.filter(|r| r.is_finite())
}

fn informational_usage_window(usage: f64, period: &str, unit: &str) -> Option<RateWindow> {
    if !usage.is_finite() {
        return None;
    }
    Some(RateWindow::informational(format!(
        "{} used / {period}",
        currency_string(usage, unit)
    )))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    KeyQuota,
    Subscription,
    Wallet,
    Unknown,
}

fn rate_window_from_usage(
    usage: f64,
    limit: Option<f64>,
    window_minutes: Option<u32>,
    unit: &str,
) -> Option<RateWindow> {
    let limit = limit.filter(|l| *l > 0.0)?;
    Some(RateWindow::with_details(
        used_percent(usage, limit),
        window_minutes,
        None,
        Some(amount_description(usage, limit, unit)),
    ))
}

fn used_percent(usage: f64, limit: f64) -> f64 {
    if limit > 0.0 {
        (usage / limit * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    }
}

fn amount_description(used: f64, limit: f64, unit: &str) -> String {
    format!(
        "{} / {}",
        currency_string(used, unit),
        currency_string(limit, unit)
    )
}

fn currency_string(value: f64, unit: &str) -> String {
    if unit.eq_ignore_ascii_case("USD") {
        format!("${value:.2}")
    } else {
        format!("{value:.2} {unit}")
    }
}

fn normalize_currency(raw: &str) -> String {
    let trimmed = raw.trim().to_ascii_uppercase();
    if trimmed.is_empty() {
        "USD".to_string()
    } else {
        trimmed
    }
}

fn window_minutes(window: &str) -> Option<u32> {
    match window.to_ascii_lowercase().as_str() {
        "5h" => Some(5 * 60),
        "1d" => Some(24 * 60),
        "7d" => Some(7 * 24 * 60),
        _ => None,
    }
}

fn rate_limit_title(window: &str) -> String {
    match window.to_ascii_lowercase().as_str() {
        "5h" => "5 hour limit".to_string(),
        "1d" => "Daily limit".to_string(),
        "7d" => "7 day limit".to_string(),
        other => format!("{other} limit"),
    }
}

fn totals_description(totals: &ParsedTotals, unit: &str) -> String {
    format!(
        "{} requests, {} tokens, {}",
        totals.requests,
        totals.total_tokens,
        currency_string(totals.actual_cost_usd, unit)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quota_limited_key_usage() {
        let json = r#"
        {
          "mode": "quota_limited",
          "isValid": true,
          "status": "active",
          "remaining": 75,
          "unit": "USD",
          "quota": {
            "limit": 100,
            "used": 25,
            "remaining": 75,
            "unit": "USD"
          },
          "rate_limits": [
            {
              "window": "5h",
              "limit": 20,
              "used": 5,
              "remaining": 15,
              "reset_at": "2026-07-11T12:30:00Z"
            },
            {
              "window": "7d",
              "limit": 200,
              "used": 40,
              "remaining": 160
            }
          ],
          "expires_at": "2026-08-01T00:00:00Z",
          "usage": {
            "today": {
              "requests": 4,
              "total_tokens": 1200,
              "actual_cost": 1.25
            },
            "total": {
              "requests": 40,
              "total_tokens": 12000,
              "actual_cost": 25
            }
          }
        }
        "#;

        let parsed = parse_usage_body(json).unwrap();
        assert_eq!(parsed.mode, "quota_limited");
        assert_eq!(parsed.quota.as_ref().unwrap().remaining, 75.0);
        assert_eq!(parsed.rate_limits.len(), 2);
        assert_eq!(parsed.today.as_ref().unwrap().total_tokens, 1200);

        let result = snapshot_from_parsed(parsed);
        assert!((result.usage.primary.used_percent - 25.0).abs() < f64::EPSILON);
        assert!(result.cost.is_none());
        assert!(
            result
                .usage
                .extra_rate_windows
                .iter()
                .any(|w| w.id == "5h" && w.window.window_minutes == Some(300))
        );
        assert!(result.usage.extra_rate_windows.iter().any(|w| {
            w.id == "today"
                && w.window
                    .reset_description
                    .as_deref()
                    .is_some_and(|d| d.contains("1200 tokens"))
        }));
        assert!(
            result
                .usage
                .extra_rate_windows
                .iter()
                .any(|w| w.id == "expires")
        );
    }

    #[test]
    fn parses_subscription_usage_windows() {
        let json = r#"
        {
          "mode": "unrestricted",
          "isValid": true,
          "planName": "Claude Team",
          "remaining": 8,
          "unit": "USD",
          "subscription": {
            "daily_usage_usd": 2,
            "weekly_usage_usd": 10,
            "monthly_usage_usd": 30,
            "daily_limit_usd": 10,
            "weekly_limit_usd": 40,
            "monthly_limit_usd": 100,
            "expires_at": "2026-08-15T00:00:00.123Z"
          }
        }
        "#;

        let result = snapshot_from_parsed(parse_usage_body(json).unwrap());
        assert!((result.usage.primary.used_percent - 20.0).abs() < f64::EPSILON);
        assert!(
            (result.usage.secondary.as_ref().unwrap().used_percent - 25.0).abs() < f64::EPSILON
        );
        assert!((result.usage.tertiary.as_ref().unwrap().used_percent - 30.0).abs() < f64::EPSILON);
        assert!(result.usage.account_organization.is_none());
        assert_eq!(
            result.usage.login_method.as_deref(),
            Some("Claude Team (unrestricted)")
        );
        assert!(
            result
                .usage
                .extra_rate_windows
                .iter()
                .any(|w| w.id == "expires")
        );
    }

    #[test]
    fn subscription_without_limits_shows_usage_not_empty() {
        let json = r#"
        {
          "mode": "unrestricted",
          "planName": "Soft plan",
          "unit": "USD",
          "subscription": {
            "daily_usage_usd": 2.5,
            "weekly_usage_usd": 10,
            "monthly_usage_usd": 30
          },
          "balance": 12.0
        }
        "#;
        let result = snapshot_from_parsed(parse_usage_body(json).unwrap());
        assert!(result.usage.primary.is_informational);
        assert!(
            result
                .usage
                .primary
                .reset_description
                .as_deref()
                .is_some_and(|d| d.contains("$2.50") && d.contains("day"))
        );
        assert!(result.cost.is_some());
        assert_eq!(result.cost.as_ref().unwrap().limit, Some(12.0));
        assert!(result.usage.account_organization.is_none());
        assert!(
            result
                .usage
                .login_method
                .as_deref()
                .is_some_and(|m| m.starts_with("Soft plan"))
        );
    }

    #[test]
    fn preserves_authoritative_subscription_windows() {
        let json = r#"
        {
          "mode": "unrestricted",
          "subscription": {
            "daily_usage_usd": 120.23,
            "weekly_usage_usd": 229.20,
            "monthly_usage_usd": 1296.23,
            "daily_limit_usd": 120,
            "weekly_limit_usd": 700,
            "monthly_limit_usd": 2800
          }
        }
        "#;

        let result = snapshot_from_parsed(parse_usage_body(json).unwrap());
        assert!((result.usage.primary.used_percent - 100.0).abs() < f64::EPSILON);
        assert!(
            (result.usage.secondary.as_ref().unwrap().used_percent - (229.20 / 700.0 * 100.0))
                .abs()
                < 0.001
        );
        assert_eq!(
            result.usage.primary.reset_description.as_deref(),
            Some("$120.23 / $120.00")
        );
        assert_eq!(
            result
                .usage
                .secondary
                .as_ref()
                .and_then(|w| w.reset_description.as_deref()),
            Some("$229.20 / $700.00")
        );
    }

    #[test]
    fn parses_wallet_balance_only() {
        let json = r#"
        {
          "mode": "unrestricted",
          "isValid": true,
          "planName": "Wallet plan",
          "remaining": 42.5,
          "unit": "USD",
          "balance": 42.5
        }
        "#;

        let result = snapshot_from_parsed(parse_usage_body(json).unwrap());
        assert!(result.usage.primary.is_informational);
        assert_eq!(
            result.usage.primary.reset_description.as_deref(),
            Some("$42.50 balance")
        );
        assert_eq!(
            result.usage.login_method.as_deref(),
            Some("Wallet plan (unrestricted)")
        );
        let cost = result.cost.unwrap();
        assert_eq!(cost.limit, Some(42.5));
        assert_eq!(cost.period, "balance");
    }

    #[test]
    fn invalid_credentials_flag_is_detected() {
        let json = r#"{"mode":"unrestricted","isValid":false}"#;
        let parsed = parse_usage_body(json).unwrap();
        assert!(!parsed.is_valid);
    }

    #[test]
    fn usage_url_accepts_root_versioned_and_complete_urls() {
        let root = Url::parse("https://api.example.com").unwrap();
        assert_eq!(
            usage_url(&root).unwrap().as_str(),
            "https://api.example.com/v1/usage"
        );

        let versioned = Url::parse("https://api.example.com/v1").unwrap();
        assert_eq!(
            usage_url(&versioned).unwrap().as_str(),
            "https://api.example.com/v1/usage"
        );

        let complete = Url::parse("https://api.example.com/v1/usage").unwrap();
        assert_eq!(
            usage_url(&complete).unwrap().as_str(),
            "https://api.example.com/v1/usage"
        );
    }

    #[test]
    fn settings_allow_https_and_loopback_http_only() {
        assert!(validated_sub2api_base_url("https://api.example.com").is_ok());
        assert!(validated_sub2api_base_url("http://127.0.0.1:8080").is_ok());
        assert!(validated_sub2api_base_url("http://api.example.com").is_err());
        assert!(validated_sub2api_base_url("https://user:pass@api.example.com").is_err());
        assert!(validated_sub2api_base_url("https://api.example.com?token=secret").is_err());
        assert!(validated_sub2api_base_url("https://api.example.com#fragment").is_err());
    }

    #[test]
    fn cleans_quoted_env_values() {
        assert_eq!(
            clean_env_value("  \"sk-test\"  ").as_deref(),
            Some("sk-test")
        );
        assert_eq!(clean_env_value("''"), None);
    }

    #[test]
    fn usage_request_includes_days_and_timezone() {
        let base = Url::parse("https://api.example.com").unwrap();
        let url = usage_request_url(&base).unwrap();
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query.get("days").map(String::as_str), Some("30"));
        assert!(query.contains_key("timezone"));
        assert!(!query.get("timezone").unwrap().is_empty());
    }
}
