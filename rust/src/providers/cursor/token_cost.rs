//! Cursor dashboard token-cost events (upstream #1745).
//!
//! `POST /api/dashboard/get-filtered-usage-events` — per-model API-rate totals
//! from `tokenUsage.totalCents` and plan-metered totals from `chargedCents`.

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::json;

use crate::core::{CostSnapshot, NamedRateWindow, ProviderError, RateWindow};

const EVENTS_PATH: &str = "/api/dashboard/get-filtered-usage-events";
const PAGE_SIZE: usize = 200;
/// Keep fetches bounded for menu-bar refresh latency.
const MAX_PAGES: usize = 5;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageEventsPage {
    total_usage_events_count: Option<i64>,
    #[serde(default)]
    usage_events_display: Vec<UsageEvent>,
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
    #[serde(default, deserialize_with = "deserialize_opt_f64")]
    cursor_token_fee: Option<f64>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct EventTokenUsage {
    #[serde(default, deserialize_with = "deserialize_i64")]
    input_tokens: i64,
    #[serde(default, deserialize_with = "deserialize_i64")]
    output_tokens: i64,
    #[serde(default, deserialize_with = "deserialize_i64")]
    cache_write_tokens: i64,
    #[serde(default, deserialize_with = "deserialize_i64")]
    cache_read_tokens: i64,
    #[serde(default, deserialize_with = "deserialize_opt_f64")]
    total_cents: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct CursorTokenCostReport {
    /// Sum of vendor list-price cents (API rate) converted to USD.
    pub api_rate_usd: f64,
    /// Sum of plan-metered chargedCents when every event has a value.
    pub metered_usd: Option<f64>,
    /// Per-model API-rate spend for extra tray windows.
    pub by_model_usd: Vec<(String, f64)>,
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
    let mut metered_complete = !events.is_empty();

    for event in events {
        let model = event
            .model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("unknown")
            .to_string();
        if let Some(list_cents) = event.token_usage.as_ref().and_then(|u| u.total_cents) {
            if !list_cents.is_finite() || list_cents < 0.0 {
                invalid_models.insert(model.clone());
                by_model.remove(&model);
            } else if !invalid_models.contains(&model) {
                *by_model.entry(model.clone()).or_insert(0.0) += list_cents;
            }
        }

        match event.charged_cents.filter(|v| v.is_finite() && *v >= 0.0) {
            Some(c) => metered_cents += c,
            None => {
                // cursorTokenFee is sometimes the only metered field.
                if let Some(fee) = event
                    .cursor_token_fee
                    .filter(|v| v.is_finite() && *v >= 0.0)
                {
                    metered_cents += fee;
                } else {
                    metered_complete = false;
                }
            }
        }
    }

    let mut by_model_usd: Vec<(String, f64)> = by_model
        .into_iter()
        .filter(|(model, _)| !invalid_models.contains(model))
        .map(|(m, cents)| (m, cents / 100.0))
        .filter(|(_, usd)| *usd > 0.0)
        .collect();
    by_model_usd.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let api_rate_usd = by_model_usd.iter().map(|(_, usd)| *usd).sum();

    CursorTokenCostReport {
        api_rate_usd,
        metered_usd: if metered_complete && metered_cents > 0.0 {
            Some(metered_cents / 100.0)
        } else {
            None
        },
        by_model_usd,
    }
}

/// Default lookback when billing cycle start is unknown: 30 days.
pub fn default_since() -> DateTime<Utc> {
    Utc::now() - Duration::days(30)
}

fn deserialize_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = i64;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("int or string int")
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<i64, E> {
            Ok(v)
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<i64, E> {
            // Usage-event counts from the Cursor API are small non-negative
            // integers, far below i64::MAX.
            #[expect(
                clippy::cast_possible_wrap,
                reason = "usage-event counts are small non-negative integers"
            )]
            let count = v as i64;
            Ok(count)
        }
        fn visit_f64<E: de::Error>(self, v: f64) -> Result<i64, E> {
            // Token counts are whole numbers; the fractional part is rounding
            // noise from JSON float parsing.
            #[expect(
                clippy::cast_possible_truncation,
                reason = "token counts are whole numbers; fractional part is rounding noise"
            )]
            let count = v as i64;
            Ok(count)
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<i64, E> {
            v.parse().map_err(E::custom)
        }
    }
    deserializer.deserialize_any(V)
}

fn deserialize_opt_i64<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(deserialize_i64(deserializer)?))
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
                    total_cents: Some(25.0),
                }),
                charged_cents: Some(10.0),
                cursor_token_fee: None,
            },
            UsageEvent {
                timestamp_ms: Some(2),
                model: Some("claude-4".into()),
                token_usage: Some(EventTokenUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_write_tokens: 0,
                    cache_read_tokens: 0,
                    total_cents: Some(75.0),
                }),
                charged_cents: Some(40.0),
                cursor_token_fee: None,
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
                    total_cents: Some(25.0),
                }),
                charged_cents: Some(0.0),
                cursor_token_fee: None,
            },
            UsageEvent {
                timestamp_ms: Some(2),
                model: Some("gpt-5".into()),
                token_usage: Some(EventTokenUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_write_tokens: 0,
                    cache_read_tokens: 0,
                    total_cents: None,
                }),
                charged_cents: Some(0.0),
                cursor_token_fee: None,
            },
        ];
        let report = summarize_events(&events);
        assert!((report.api_rate_usd - 0.25).abs() < 0.001);
        assert_eq!(report.by_model_usd, vec![("gpt-5".to_string(), 0.25)]);
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
                    total_cents: Some(-1.0),
                }),
                charged_cents: Some(0.0),
                cursor_token_fee: None,
            },
            UsageEvent {
                timestamp_ms: Some(2),
                model: Some("gpt-5".into()),
                token_usage: Some(EventTokenUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_write_tokens: 0,
                    cache_read_tokens: 0,
                    total_cents: Some(50.0),
                }),
                charged_cents: Some(0.0),
                cursor_token_fee: None,
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
                total_cents: Some(10.0),
            }),
            charged_cents: None,
            cursor_token_fee: None,
        }];
        let report = summarize_events(&events);
        assert!(report.metered_usd.is_none());
        assert!((report.api_rate_usd - 0.1).abs() < 0.001);
    }
}
