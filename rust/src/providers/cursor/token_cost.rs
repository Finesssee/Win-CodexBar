//! Cursor dashboard token-cost events (upstream #1745).
//!
//! `POST /api/dashboard/get-filtered-usage-events` — per-model API-rate totals
//! from `tokenUsage.totalCents` and plan-metered totals from `chargedCents`.

use chrono::{DateTime, Duration, TimeZone, Utc};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use crate::core::{CostSnapshot, CostUsagePricing, NamedRateWindow, ProviderError, RateWindow};

const EVENTS_PATH: &str = "/api/dashboard/get-filtered-usage-events";
const PAGE_SIZE: usize = 200;
/// Keep fetches bounded for menu-bar refresh latency.
const MAX_PAGES: usize = 5;

#[derive(Debug)]
struct UsageEventsPage {
    total_usage_events_count: Option<i64>,
    usage_events_display: Vec<UsageEvent>,
}

impl<'de> Deserialize<'de> for UsageEventsPage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| D::Error::custom("Cursor usage events response must be an object"))?;
        if object.is_empty() {
            return Ok(Self {
                total_usage_events_count: Some(0),
                usage_events_display: Vec::new(),
            });
        }
        if object.contains_key("error") {
            return Err(D::Error::custom(
                "Cursor usage events error envelope is not an empty result",
            ));
        }

        let total_usage_events_count = match object.get("totalUsageEventsCount") {
            Some(value) => {
                let count = strict_i64(value).ok_or_else(|| {
                    D::Error::custom("Cursor usage event count must be a finite integer")
                })?;
                if count < 0 {
                    return Err(D::Error::custom(
                        "Cursor usage event count cannot be negative",
                    ));
                }
                Some(count)
            }
            None => None,
        };
        if object.len() == 1 && total_usage_events_count.is_some() {
            return Ok(Self {
                total_usage_events_count,
                usage_events_display: Vec::new(),
            });
        }

        let events = object
            .get("usageEventsDisplay")
            .ok_or_else(|| D::Error::custom("Cursor usage events array is missing"))?;
        if !events.is_array() {
            return Err(D::Error::custom(
                "Cursor usage events field must be an array",
            ));
        }
        let usage_events_display = serde_json::from_value(events.clone())
            .map_err(|error| D::Error::custom(format!("Cursor usage events: {error}")))?;
        Ok(Self {
            total_usage_events_count,
            usage_events_display,
        })
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct UsageEvent {
    #[serde(
        default,
        deserialize_with = "deserialize_opt_i64",
        rename = "timestamp"
    )]
    timestamp_ms: Option<i64>,
    model: Option<String>,
    token_usage: Option<EventTokenUsage>,
    #[serde(default, deserialize_with = "deserialize_opt_f64")]
    charged_cents: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum EventCost {
    Valid(f64),
    Omitted,
    Invalid,
}

#[derive(Debug, Clone)]
struct EventTokenUsage {
    input_tokens: i64,
    output_tokens: i64,
    cache_write_tokens: i64,
    cache_read_tokens: i64,
    cost: EventCost,
}

impl<'de> Deserialize<'de> for EventTokenUsage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| D::Error::custom("Cursor token usage must be an object"))?;
        let count = |key: &str| object.get(key).and_then(strict_i64).unwrap_or(0);
        let cost = match object.get("totalCents") {
            None | Some(Value::Null) => EventCost::Omitted,
            Some(value) => match finite_f64(value) {
                Some(cents) if cents >= 0.0 => EventCost::Valid(cents),
                _ => EventCost::Invalid,
            },
        };
        Ok(Self {
            input_tokens: count("inputTokens"),
            output_tokens: count("outputTokens"),
            cache_write_tokens: count("cacheWriteTokens"),
            cache_read_tokens: count("cacheReadTokens"),
            cost,
        })
    }
}

impl EventTokenUsage {
    fn validated_counts(&self) -> Option<(u64, u64, u64, u64)> {
        let input = u64::try_from(self.input_tokens).ok()?;
        let output = u64::try_from(self.output_tokens).ok()?;
        let cache_write = u64::try_from(self.cache_write_tokens).ok()?;
        let cache_read = u64::try_from(self.cache_read_tokens).ok()?;
        let total = input
            .checked_add(output)?
            .checked_add(cache_write)?
            .checked_add(cache_read)?;
        (total > 0).then_some((input, output, cache_write, cache_read))
    }
}

#[derive(Debug, Clone, Default)]
pub struct CursorTokenCostReport {
    /// Sum of vendor list-price cents (API rate) converted to USD.
    pub api_rate_usd: f64,
    /// Sum of plan-metered chargedCents when every event has a value.
    pub metered_usd: Option<f64>,
    /// Per-model API-rate spend for extra tray windows.
    pub by_model_usd: Vec<(String, f64)>,
    /// Vendor-reported list-price requests.
    pub priced_requests: u32,
    /// Requests estimated from model pricing because Cursor omitted totalCents.
    pub estimated_requests: u32,
    /// Requests whose API-rate cost is unknown or invalid.
    pub unpriced_requests: u32,
}

impl CursorTokenCostReport {
    pub fn to_extra_windows(&self) -> Vec<NamedRateWindow> {
        let mut out = Vec::new();
        let max = self
            .by_model_usd
            .iter()
            .map(|(_, c)| *c)
            .fold(0.0_f64, |a, b| a.max(b))
            .max(0.01);
        for (model, cost) in self.by_model_usd.iter().take(6) {
            let percent = ((cost / max) * 100.0).clamp(0.0, 100.0);
            let mut window = RateWindow::new(percent);
            window.is_informational = true;
            window.reset_description = Some(format!("${cost:.2} API-rate"));
            out.push(NamedRateWindow::new(
                format!("cursor-model-{}", sanitize_id(model)),
                model.clone(),
                window,
            ));
        }
        out
    }

    pub fn merge_into_cost(&self, base: Option<CostSnapshot>) -> Option<CostSnapshot> {
        if self.api_rate_usd <= 0.0 && self.metered_usd.unwrap_or(0.0) <= 0.0 {
            return base;
        }
        let used = self
            .metered_usd
            .filter(|v| *v > 0.0)
            .unwrap_or(self.api_rate_usd);
        let period = if self.metered_usd.is_some() {
            "Token cost (metered, billing window)"
        } else if self.unpriced_requests > 0 {
            "Token cost (API-rate partial, billing window)"
        } else if self.estimated_requests > 0 {
            "Token cost (API-rate estimate, billing window)"
        } else {
            "Token cost (API-rate, billing window)"
        };
        if let Some(mut base) = base {
            // Prefer richer token-cost used when base is plan/on-demand only.
            if base.used < used {
                base.used = used;
            }
            base.period = period.into();
            return Some(base);
        }
        Some(CostSnapshot::new(used, "USD", period))
    }
}

fn sanitize_id(model: &str) -> String {
    model
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

pub async fn fetch_token_cost_report(
    client: &reqwest::Client,
    cookie_header: &str,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> Result<CursorTokenCostReport, ProviderError> {
    let mut all = Vec::new();
    let mut expected_total: Option<i64> = None;
    for page in 1..=MAX_PAGES {
        let page_body = fetch_page(client, cookie_header, page, since, until).await?;
        if let Some(total) = page_body.total_usage_events_count {
            expected_total = Some(total.max(0));
        }
        if page_body.usage_events_display.is_empty() {
            break;
        }
        let count = page_body.usage_events_display.len();
        all.extend(page_body.usage_events_display);
        if count < PAGE_SIZE {
            break;
        }
        // Collected events are bounded by MAX_PAGES * PAGE_SIZE, so the
        // length always fits in i64.
        #[expect(
            clippy::cast_possible_wrap,
            reason = "event count bounded by MAX_PAGES * PAGE_SIZE"
        )]
        let collected = all.len() as i64;
        if let Some(expected) = expected_total
            && collected >= expected
        {
            break;
        }
    }
    Ok(summarize_events(&all))
}

async fn fetch_page(
    client: &reqwest::Client,
    cookie_header: &str,
    page: usize,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> Result<UsageEventsPage, ProviderError> {
    let url = format!("https://cursor.com{EVENTS_PATH}");
    let body = json!({
        "page": page,
        "pageSize": PAGE_SIZE,
        "startDate": since.map(|d| d.timestamp_millis().to_string()),
        "endDate": until.map(|d| d.timestamp_millis().to_string()),
    });
    let response = client
        .post(&url)
        .header("Cookie", cookie_header)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await?;
    if response.status() == 401 || response.status() == 403 {
        return Err(ProviderError::AuthRequired);
    }
    if !response.status().is_success() {
        return Err(ProviderError::Other(format!(
            "Cursor usage events returned {}",
            response.status()
        )));
    }
    response
        .json()
        .await
        .map_err(|e| ProviderError::Parse(format!("Cursor usage events: {e}")))
}

fn summarize_events(events: &[UsageEvent]) -> CursorTokenCostReport {
    use std::collections::{HashMap, HashSet};

    let mut by_model: HashMap<String, f64> = HashMap::new();
    let mut invalid_models: HashSet<String> = HashSet::new();
    let mut metered_cents = 0.0;
    let mut metered_complete = true;
    let mut saw_metered_event = false;
    let mut priced_requests = 0u32;
    let mut estimated_requests = 0u32;
    let mut unpriced_requests = 0u32;

    for event in events {
        let Some(timestamp_ms) = event.timestamp_ms.filter(|value| *value > 0) else {
            continue;
        };

        saw_metered_event = true;
        match event
            .charged_cents
            .filter(|value| value.is_finite() && *value >= 0.0)
        {
            Some(cents) => {
                let next = metered_cents + cents;
                if next.is_finite() {
                    metered_cents = next;
                } else {
                    metered_complete = false;
                }
            }
            None => metered_complete = false,
        }

        let Some(usage) = event.token_usage.as_ref() else {
            continue;
        };
        if usage.validated_counts().is_none() {
            continue;
        }
        let model = event
            .model
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("unknown")
            .to_string();

        match usage.cost {
            EventCost::Valid(cents) if cents.is_finite() && cents >= 0.0 => {
                priced_requests = priced_requests.saturating_add(1);
                add_model_cost(&mut by_model, &mut invalid_models, &model, cents);
            }
            EventCost::Omitted => {
                if let Some(cents) = estimated_list_price_cents(usage, &model, timestamp_ms) {
                    estimated_requests = estimated_requests.saturating_add(1);
                    add_model_cost(&mut by_model, &mut invalid_models, &model, cents);
                } else {
                    unpriced_requests = unpriced_requests.saturating_add(1);
                }
            }
            EventCost::Invalid | EventCost::Valid(_) => {
                unpriced_requests = unpriced_requests.saturating_add(1);
                invalid_models.insert(model.clone());
                by_model.remove(&model);
            }
        }
    }

    let mut by_model_usd: Vec<(String, f64)> = by_model
        .into_iter()
        .filter(|(model, _)| !invalid_models.contains(model))
        .map(|(model, cents)| (model, cents / 100.0))
        .filter(|(_, usd)| usd.is_finite() && *usd > 0.0)
        .collect();
    by_model_usd.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });
    let api_rate_usd = by_model_usd.iter().map(|(_, usd)| *usd).sum();

    CursorTokenCostReport {
        api_rate_usd,
        metered_usd: if saw_metered_event && metered_complete {
            Some(metered_cents / 100.0)
        } else {
            None
        },
        by_model_usd,
        priced_requests,
        estimated_requests,
        unpriced_requests,
    }
}

fn add_model_cost(
    by_model: &mut std::collections::HashMap<String, f64>,
    invalid_models: &mut std::collections::HashSet<String>,
    model: &str,
    cents: f64,
) {
    if invalid_models.contains(model) {
        return;
    }
    let next = by_model.get(model).copied().unwrap_or(0.0) + cents;
    if next.is_finite() && next >= 0.0 {
        by_model.insert(model.to_string(), next);
    } else {
        invalid_models.insert(model.to_string());
        by_model.remove(model);
    }
}

fn estimated_list_price_cents(
    usage: &EventTokenUsage,
    model: &str,
    timestamp_ms: i64,
) -> Option<f64> {
    if usage.cost != EventCost::Omitted {
        return None;
    }
    let (input, output, cache_write, cache_read) = usage.validated_counts()?;
    let pricing_day = Utc
        .timestamp_millis_opt(timestamp_ms)
        .single()?
        .date_naive();

    let codex_input = input.checked_add(cache_read)?.checked_add(cache_write)?;
    if let Some(usd) = CostUsagePricing::codex_cost_usd_at_date(
        model,
        codex_input,
        cache_read,
        output,
        pricing_day,
    ) {
        let cents = usd * 100.0;
        return cents.is_finite().then_some(cents);
    }

    let alias = cursor_claude_catalog_model(model);
    let input = i32::try_from(input).ok()?;
    let output = i32::try_from(output).ok()?;
    let cache_write = i32::try_from(cache_write).ok()?;
    let cache_read = i32::try_from(cache_read).ok()?;
    CostUsagePricing::claude_cost_usd(&alias, input, cache_read, cache_write, output).and_then(
        |usd| {
            let cents = usd * 100.0;
            cents.is_finite().then_some(cents)
        },
    )
}

fn cursor_claude_catalog_model(model: &str) -> String {
    let Some(rest) = model.strip_prefix("claude-") else {
        return model.to_string();
    };
    let mut parts = rest.splitn(3, '-');
    let Some(version) = parts.next() else {
        return model.to_string();
    };
    let Some(family) = parts.next() else {
        return model.to_string();
    };
    if !matches!(family, "sonnet" | "opus" | "haiku") {
        return model.to_string();
    }
    let Some((major, minor)) = version.split_once('.') else {
        return model.to_string();
    };
    if major.is_empty()
        || minor.is_empty()
        || !major.chars().all(|ch| ch.is_ascii_digit())
        || !minor.chars().all(|ch| ch.is_ascii_digit())
    {
        return model.to_string();
    }
    match parts.next() {
        Some(suffix) if !suffix.is_empty() => format!("claude-{family}-{major}-{minor}-{suffix}"),
        _ => format!("claude-{family}-{major}-{minor}"),
    }
}

/// Default lookback when billing cycle start is unknown: 30 days.
pub fn default_since() -> DateTime<Utc> {
    Utc::now() - Duration::days(30)
}

fn finite_f64(value: &Value) -> Option<f64> {
    let parsed = value
        .as_f64()
        .or_else(|| value.as_i64().map(|number| number as f64))
        .or_else(|| value.as_u64().map(|number| number as f64))
        .or_else(|| value.as_str()?.trim().parse::<f64>().ok())?;
    parsed.is_finite().then_some(parsed)
}

fn strict_i64(value: &Value) -> Option<i64> {
    if let Some(number) = value.as_i64() {
        return Some(number);
    }
    if let Some(number) = value.as_u64() {
        return i64::try_from(number).ok();
    }
    if let Some(text) = value.as_str() {
        let text = text.trim();
        if let Ok(number) = text.parse::<i64>() {
            return Some(number);
        }
    }
    let parsed = value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse::<f64>().ok())?;
    if !parsed.is_finite()
        || parsed.fract() != 0.0
        || parsed < i64::MIN as f64
        || parsed > i64::MAX as f64
    {
        return None;
    }
    #[allow(
        clippy::cast_possible_truncation,
        reason = "range and integrality are checked before the cast"
    )]
    Some(parsed as i64)
}
fn deserialize_opt_i64<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(strict_i64(&value))
}

fn deserialize_opt_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = Option<f64>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("number or string number")
        }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
            Ok(if v.is_finite() { Some(v) } else { None })
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok(Some(v as f64))
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(Some(v as f64))
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(v.parse().ok().filter(|n: &f64| n.is_finite()))
        }
    }
    deserializer.deserialize_any(V)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_per_model_and_metered() {
        let events = vec![
            UsageEvent {
                timestamp_ms: Some(1),
                model: Some("gpt-5".into()),
                token_usage: Some(EventTokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_write_tokens: 0,
                    cache_read_tokens: 0,
                    cost: EventCost::Valid(25.0),
                }),
                charged_cents: Some(10.0),
            },
            UsageEvent {
                timestamp_ms: Some(2),
                model: Some("claude-4".into()),
                token_usage: Some(EventTokenUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_write_tokens: 0,
                    cache_read_tokens: 0,
                    cost: EventCost::Valid(75.0),
                }),
                charged_cents: Some(40.0),
            },
        ];
        let report = summarize_events(&events);
        assert!((report.api_rate_usd - 1.0).abs() < 0.001);
        assert_eq!(report.metered_usd, Some(0.5));
        assert_eq!(report.by_model_usd[0].0, "claude-4");
        let windows = report.to_extra_windows();
        assert_eq!(windows.len(), 2);
        assert!(windows[0].window.is_informational);
    }

    #[test]
    fn missing_model_cost_keeps_priced_siblings() {
        let events = vec![
            UsageEvent {
                timestamp_ms: Some(1),
                model: Some("gpt-5".into()),
                token_usage: Some(EventTokenUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_write_tokens: 0,
                    cache_read_tokens: 0,
                    cost: EventCost::Valid(25.0),
                }),
                charged_cents: Some(0.0),
            },
            UsageEvent {
                timestamp_ms: Some(2),
                model: Some("gpt-5".into()),
                token_usage: Some(EventTokenUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_write_tokens: 0,
                    cache_read_tokens: 0,
                    cost: EventCost::Omitted,
                }),
                charged_cents: Some(0.0),
            },
        ];
        let report = summarize_events(&events);
        assert!(report.api_rate_usd > 0.25);
        assert_eq!(report.priced_requests, 1);
        assert_eq!(report.estimated_requests, 1);
        assert_eq!(report.unpriced_requests, 0);
    }

    #[test]
    fn invalid_model_cost_latches_and_cannot_revive() {
        let events = vec![
            UsageEvent {
                timestamp_ms: Some(1),
                model: Some("gpt-5".into()),
                token_usage: Some(EventTokenUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_write_tokens: 0,
                    cache_read_tokens: 0,
                    cost: EventCost::Invalid,
                }),
                charged_cents: Some(0.0),
            },
            UsageEvent {
                timestamp_ms: Some(2),
                model: Some("gpt-5".into()),
                token_usage: Some(EventTokenUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_write_tokens: 0,
                    cache_read_tokens: 0,
                    cost: EventCost::Valid(50.0),
                }),
                charged_cents: Some(0.0),
            },
        ];
        let report = summarize_events(&events);
        assert_eq!(report.api_rate_usd, 0.0);
        assert!(report.by_model_usd.is_empty());
    }

    #[test]
    fn incomplete_metered_is_none() {
        let events = vec![UsageEvent {
            timestamp_ms: Some(1),
            model: Some("gpt-5".into()),
            token_usage: Some(EventTokenUsage {
                input_tokens: 1,
                output_tokens: 1,
                cache_write_tokens: 0,
                cache_read_tokens: 0,
                cost: EventCost::Valid(10.0),
            }),
            charged_cents: None,
        }];
        let report = summarize_events(&events);
        assert!(report.metered_usd.is_none());
        assert!((report.api_rate_usd - 0.1).abs() < 0.001);
    }

    #[test]
    fn strict_page_decoder_accepts_literal_and_count_only_empty_results() {
        let empty: UsageEventsPage = serde_json::from_str("{}").unwrap();
        assert_eq!(empty.total_usage_events_count, Some(0));
        assert!(empty.usage_events_display.is_empty());

        let counted: UsageEventsPage =
            serde_json::from_str(r#"{"totalUsageEventsCount":2}"#).unwrap();
        assert_eq!(counted.total_usage_events_count, Some(2));
        assert!(counted.usage_events_display.is_empty());
    }

    #[test]
    fn strict_page_decoder_rejects_error_and_malformed_empty_envelopes() {
        for json in [
            r#"{"error":"temporarily unavailable"}"#,
            r#"{"usageEventsDisplay":null}"#,
            r#"{"usageEventsDisplay":{}}"#,
            r#"{"unknown":null}"#,
            r#"{"totalUsageEventsCount":-1,"usageEventsDisplay":[]}"#,
            r#"{"totalUsageEventsCount":"Infinity","usageEventsDisplay":[]}"#,
            "[]",
            "null",
        ] {
            assert!(
                serde_json::from_str::<UsageEventsPage>(json).is_err(),
                "must reject {json}"
            );
        }
    }

    #[test]
    fn omitted_known_cost_is_estimated_but_unknown_model_stays_unpriced() {
        let timestamp = 1_700_000_000_000;
        let events = vec![
            UsageEvent {
                timestamp_ms: Some(timestamp),
                model: Some("gpt-5".into()),
                token_usage: Some(EventTokenUsage {
                    input_tokens: 200,
                    output_tokens: 20,
                    cache_write_tokens: 0,
                    cache_read_tokens: 0,
                    cost: EventCost::Omitted,
                }),
                charged_cents: Some(10.0),
            },
            UsageEvent {
                timestamp_ms: Some(timestamp + 1),
                model: Some("fixture-model".into()),
                token_usage: Some(EventTokenUsage {
                    input_tokens: 7,
                    output_tokens: 0,
                    cache_write_tokens: 0,
                    cache_read_tokens: 0,
                    cost: EventCost::Omitted,
                }),
                charged_cents: Some(10.0),
            },
        ];
        let report = summarize_events(&events);
        assert!((report.api_rate_usd - 0.00045).abs() < 1e-9);
        assert_eq!(report.priced_requests, 0);
        assert_eq!(report.estimated_requests, 1);
        assert_eq!(report.unpriced_requests, 1);
        assert_eq!(report.metered_usd, Some(0.20));
    }

    #[test]
    fn cursor_claude_alias_estimate_keeps_cache_buckets_disjoint() {
        let usage = EventTokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_write_tokens: 300,
            cache_read_tokens: 200,
            cost: EventCost::Omitted,
        };
        let cents = estimated_list_price_cents(&usage, "claude-4.5-sonnet", 1_700_000_000_000)
            .expect("known Claude alias price");
        let expected = CostUsagePricing::claude_cost_usd("claude-sonnet-4-5", 100, 200, 300, 50)
            .unwrap()
            * 100.0;
        assert!((cents - expected).abs() < 1e-12);
    }

    #[test]
    fn invalid_total_cents_is_never_reestimated() {
        let page: UsageEventsPage = serde_json::from_str(
            r#"{"totalUsageEventsCount":1,"usageEventsDisplay":[{"timestamp":"1700000000000","model":"gpt-5","chargedCents":5,"tokenUsage":{"inputTokens":200,"outputTokens":20,"totalCents":"NaN"}}]}"#,
        )
        .unwrap();
        let report = summarize_events(&page.usage_events_display);
        assert_eq!(report.api_rate_usd, 0.0);
        assert_eq!(report.estimated_requests, 0);
        assert_eq!(report.unpriced_requests, 1);
        assert_eq!(report.metered_usd, Some(0.05));
    }
}
