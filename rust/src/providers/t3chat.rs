//! T3 Chat provider implementation.
//!
//! Fetches usage from the T3 Chat tRPC customer-data endpoint using browser
//! cookies or a pasted full browser cURL capture.

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use regex_lite::Regex;
use serde::Deserialize;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};
use crate::providers::browser_cookie_header;

const BASE_URL: &str = "https://t3.chat";
const CUSTOMER_DATA_URL: &str = "https://t3.chat/api/trpc/getCustomerData";
const CUSTOMER_DATA_INPUT: &str =
    r#"{"0":{"json":{"sessionId":null},"meta":{"values":{"sessionId":["undefined"]}}}}"#;
const COOKIE_DOMAINS: [&str; 2] = ["t3.chat", "www.t3.chat"];

pub struct T3ChatProvider {
    metadata: ProviderMetadata,
}

#[derive(Debug, Clone)]
struct T3RequestContext {
    cookie_header: String,
    headers: Vec<(String, String)>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct T3CustomerData {
    sub_tier: Option<String>,
    subscription: Option<T3Subscription>,
    usage_band: Option<String>,
    usage_four_hour_percentage: Option<f64>,
    usage_month_percentage: Option<f64>,
    usage_four_hour_next_reset_at: Option<f64>,
    usage_period_percentage: Option<f64>,
    usage_window_next_reset_at: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct T3Subscription {
    product_name: Option<String>,
    current_period_end: Option<f64>,
}

impl T3ChatProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::T3Chat,
                display_name: "T3 Chat",
                session_label: "Base",
                weekly_label: "Overage",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://t3.chat/settings/customization"),
                status_page_url: None,
            },
        }
    }

    async fn fetch_via_web(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProviderError> {
        let request_context = self.resolve_request_context(ctx)?;
        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(ctx.web_timeout.max(1)))
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        let mut request = client
            .get(CUSTOMER_DATA_URL)
            .query(&[("batch", "1"), ("input", CUSTOMER_DATA_INPUT)])
            .header("Accept", "*/*")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Cache-Control", "no-cache")
            .header("Pragma", "no-cache")
            .header("Origin", BASE_URL)
            .header("Referer", "https://t3.chat/settings/customization")
            .header("Sec-Fetch-Dest", "empty")
            .header("Sec-Fetch-Mode", "cors")
            .header("Sec-Fetch-Site", "same-origin")
            .header("Priority", "u=4")
            .header("trpc-accept", "application/jsonl")
            .header("x-trpc-batch", "true")
            .header("x-trpc-source", "web-client")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36",
            )
            .header("Cookie", request_context.cookie_header);

        for (name, value) in request_context.headers {
            request = request.header(name, value);
        }

        let response = request.send().await?;
        let status = response.status();
        let is_vercel_challenge = response
            .headers()
            .get("x-vercel-mitigated")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("challenge"));
        let body = response.bytes().await?;

        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                return Err(ProviderError::AuthRequired);
            }
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS && is_vercel_challenge {
                return Err(ProviderError::Other(
                    "T3 Chat returned a Vercel security challenge. Paste the full browser cURL request, not just the Cookie header.".into(),
                ));
            }
            return Err(ProviderError::Other(format!(
                "T3 Chat API error: HTTP {status}"
            )));
        }

        let customer = Self::parse_json_lines(&body)?;
        Ok(Self::usage_from_customer_data(customer))
    }

    fn resolve_request_context(
        &self,
        ctx: &FetchContext,
    ) -> Result<T3RequestContext, ProviderError> {
        if let Some(raw) = ctx.manual_cookie_header.as_deref()
            && let Some(context) = Self::request_context_from_raw(raw)
        {
            return Ok(context);
        }

        let cookie_header = browser_cookie_header(&COOKIE_DOMAINS)?;
        Ok(T3RequestContext {
            cookie_header,
            headers: Vec::new(),
        })
    }

    fn request_context_from_raw(raw: &str) -> Option<T3RequestContext> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        let fields = Self::header_fields(raw);
        let cookie_header =
            Self::cookie_header_from_fields(&fields).or_else(|| normalize_cookie_header(raw))?;
        let headers = Self::forwarded_headers(&fields);
        Some(T3RequestContext {
            cookie_header,
            headers,
        })
    }

    fn header_fields(raw: &str) -> Vec<String> {
        let Ok(re) =
            Regex::new(r#"(?s)(?:^|\s)(?:-H|--header)(?:\s+|=)(?:'([^']*)'|"([^"]*)"|(\S+))"#)
        else {
            return Vec::new();
        };
        re.captures_iter(raw)
            .filter_map(|caps| {
                caps.get(1)
                    .or_else(|| caps.get(2))
                    .or_else(|| caps.get(3))
                    .map(|m| unescape_shell_segment(m.as_str()))
            })
            .collect()
    }

    fn cookie_header_from_fields(fields: &[String]) -> Option<String> {
        fields.iter().find_map(|field| {
            let (name, value) = split_header(field)?;
            name.eq_ignore_ascii_case("cookie")
                .then(|| normalize_cookie_header(value))
                .flatten()
        })
    }

    fn forwarded_headers(fields: &[String]) -> Vec<(String, String)> {
        const FORWARDED: &[(&str, &str)] = &[
            ("accept", "Accept"),
            ("accept-language", "Accept-Language"),
            ("cache-control", "Cache-Control"),
            ("pragma", "Pragma"),
            ("priority", "Priority"),
            ("referer", "Referer"),
            ("sec-fetch-dest", "Sec-Fetch-Dest"),
            ("sec-fetch-mode", "Sec-Fetch-Mode"),
            ("sec-fetch-site", "Sec-Fetch-Site"),
            ("trpc-accept", "trpc-accept"),
            ("user-agent", "User-Agent"),
            ("x-client-context", "x-client-context"),
            ("x-deployment-id", "X-Deployment-Id"),
            ("x-trpc-batch", "x-trpc-batch"),
            ("x-trpc-source", "x-trpc-source"),
        ];
        fields
            .iter()
            .filter_map(|field| {
                let (name, value) = split_header(field)?;
                let canonical = FORWARDED
                    .iter()
                    .find(|(candidate, _)| name.eq_ignore_ascii_case(candidate))
                    .map(|(_, canonical)| *canonical)?;
                (!value.trim().is_empty())
                    .then(|| (canonical.to_string(), value.trim().to_string()))
            })
            .collect()
    }

    fn parse_json_lines(data: &[u8]) -> Result<T3CustomerData, ProviderError> {
        let text = std::str::from_utf8(data)
            .map_err(|_| ProviderError::Parse("T3 Chat response is not UTF-8".into()))?;
        for line in text.lines() {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if let Some(customer) = find_customer_data(&value) {
                return serde_json::from_value(customer.clone()).map_err(|e| {
                    ProviderError::Parse(format!("Could not parse T3 Chat customer data: {e}"))
                });
            }
        }
        Err(ProviderError::Parse(
            "Missing T3 Chat customer data object".into(),
        ))
    }

    fn usage_from_customer_data(customer: T3CustomerData) -> UsageSnapshot {
        let base_reset = date_from_epoch(customer.usage_four_hour_next_reset_at)
            .or_else(|| date_from_epoch(customer.usage_window_next_reset_at));
        let overage_reset = customer
            .subscription
            .as_ref()
            .and_then(|subscription| date_from_epoch(subscription.current_period_end));
        let base_description = match customer.usage_band.as_deref().map(str::trim) {
            Some(band) if !band.is_empty() => format!("Base - {band}"),
            _ => "Base".to_string(),
        };

        let primary = RateWindow::with_details(
            percent(customer.usage_four_hour_percentage),
            Some(4 * 60),
            base_reset,
            Some(base_description),
        );
        let secondary = RateWindow::with_details(
            percent(
                customer
                    .usage_month_percentage
                    .or(customer.usage_period_percentage),
            ),
            None,
            overage_reset,
            Some("Overage".to_string()),
        );

        let mut usage = UsageSnapshot::new(primary).with_secondary(secondary);
        if let Some(plan) = plan_name(&customer) {
            usage = usage.with_login_method(plan);
        }
        usage
    }
}

fn normalize_cookie_header(raw: &str) -> Option<String> {
    let mut header = raw.trim();
    if header
        .get(.."cookie:".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("cookie:"))
    {
        header = header["cookie:".len()..].trim();
    }
    (!header.is_empty() && header.contains('=')).then(|| header.to_string())
}

fn split_header(field: &str) -> Option<(&str, &str)> {
    let colon = field.find(':')?;
    Some((field[..colon].trim(), field[colon + 1..].trim()))
}

fn unescape_shell_segment(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                output.push(next);
            }
        } else {
            output.push(ch);
        }
    }
    output
}

fn find_customer_data(value: &serde_json::Value) -> Option<&serde_json::Value> {
    match value {
        serde_json::Value::Object(map) => {
            if map.contains_key("usageFourHourPercentage")
                || map.contains_key("usageMonthPercentage")
                || (map.contains_key("subscription") && map.contains_key("usageBand"))
            {
                return Some(value);
            }
            map.values().find_map(find_customer_data)
        }
        serde_json::Value::Array(values) => values.iter().find_map(find_customer_data),
        _ => None,
    }
}

fn percent(value: Option<f64>) -> f64 {
    value.unwrap_or(0.0).clamp(0.0, 100.0)
}

fn date_from_epoch(value: Option<f64>) -> Option<DateTime<Utc>> {
    let raw = value?;
    if raw <= 0.0 {
        return None;
    }
    let seconds = if raw > 10_000_000_000.0 {
        raw / 1000.0
    } else {
        raw
    };
    // Epoch seconds (normalized above) are far below i64::MAX for real dates.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "normalized epoch seconds fit i64"
    )]
    let secs = seconds as i64;
    Utc.timestamp_opt(secs, 0).single()
}

fn plan_name(customer: &T3CustomerData) -> Option<String> {
    let raw = customer
        .subscription
        .as_ref()
        .and_then(|subscription| subscription.product_name.as_deref())
        .or(customer.sub_tier.as_deref())?
        .trim();
    if raw.is_empty() {
        return None;
    }
    Some(
        raw.split('-')
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    )
}

impl Default for T3ChatProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for T3ChatProvider {
    fn id(&self) -> ProviderId {
        ProviderId::T3Chat
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto | SourceMode::Web => {
                let usage = self.fetch_via_web(ctx).await?;
                Ok(ProviderFetchResult::new(usage, "web"))
            }
            SourceMode::Cli | SourceMode::OAuth => {
                Err(ProviderError::UnsupportedSource(ctx.source_mode))
            }
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::Web]
    }

    fn supports_web(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RESPONSE: &str = concat!(
        r#"{"json":{"0":[[0],[null,0,0]]}}"#,
        "\n",
        r#"{"json":[2,0,[[{"subTier":"pro","subscription":{"productName":"pro","currentPeriodEnd":1780763009000},"usageBand":"max","usageFourHourPercentage":12.5,"usageMonthPercentage":34.25,"usageFourHourNextResetAt":1779366216920,"usagePeriodPercentage":44,"usageWindowNextResetAt":1779366216920}]]]}"#
    );

    #[test]
    fn parses_customer_data_from_json_lines() {
        let customer = T3ChatProvider::parse_json_lines(SAMPLE_RESPONSE.as_bytes()).unwrap();
        assert_eq!(customer.sub_tier.as_deref(), Some("pro"));
        assert_eq!(customer.usage_band.as_deref(), Some("max"));
        assert_eq!(customer.usage_four_hour_percentage, Some(12.5));
    }

    #[test]
    fn maps_customer_data_to_windows() {
        let customer = T3ChatProvider::parse_json_lines(SAMPLE_RESPONSE.as_bytes()).unwrap();
        let usage = T3ChatProvider::usage_from_customer_data(customer);
        assert_eq!(usage.primary.used_percent, 12.5);
        assert_eq!(usage.primary.window_minutes, Some(240));
        assert_eq!(
            usage.primary.reset_description.as_deref(),
            Some("Base - max")
        );
        let secondary = usage.secondary.unwrap();
        assert_eq!(secondary.used_percent, 34.25);
        assert_eq!(secondary.reset_description.as_deref(), Some("Overage"));
        assert_eq!(usage.login_method.as_deref(), Some("Pro"));
    }

    #[test]
    fn extracts_cookie_and_forwarded_headers_from_curl() {
        let curl = r#"curl 'https://t3.chat/api/trpc/getCustomerData' -H 'User-Agent: Browser' --header "X-Deployment-Id: dpl_test" -H 'Cookie: session=abc; cf_clearance=token' "#;
        let context = T3ChatProvider::request_context_from_raw(curl).unwrap();
        assert_eq!(context.cookie_header, "session=abc; cf_clearance=token");
        assert!(
            context
                .headers
                .iter()
                .any(|(name, value)| name == "User-Agent" && value == "Browser")
        );
        assert!(
            context
                .headers
                .iter()
                .any(|(name, value)| name == "X-Deployment-Id" && value == "dpl_test")
        );
    }
}
