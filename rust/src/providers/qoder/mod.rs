//! Qoder provider implementation.
//!
//! Uses browser/manual cookies against the global or China Qoder usage API.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde_json::Value;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};

const GLOBAL_API: &str = "https://qoder.com/api/v2/me/usages/big_model_credits";
const CHINA_API: &str = "https://qoder.com.cn/api/v2/me/usages/big_model_credits";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

pub struct QoderProvider {
    metadata: ProviderMetadata,
    client: Client,
}

impl QoderProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Qoder,
                display_name: "Qoder",
                session_label: "Credits",
                weekly_label: "Shared credits",
                supports_opus: false,
                supports_credits: true,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://qoder.com/account/usage"),
                status_page_url: None,
            },
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    async fn fetch_web(&self, cookie_header: &str) -> Result<ProviderFetchResult, ProviderError> {
        let cookie = normalize_cookie_header(cookie_header).ok_or(ProviderError::NoCookies)?;
        let regions = [
            (GLOBAL_API, "https://qoder.com/account/usage", "Qoder"),
            (
                CHINA_API,
                "https://qoder.com.cn/account/usage",
                "Qoder China",
            ),
        ];

        let mut auth_failed = false;
        let mut last_error = None;
        for (url, referer, login_method) in regions {
            match self.fetch_region(url, referer, &cookie).await {
                Ok(value) => {
                    return Ok(ProviderFetchResult::new(
                        snapshot_from_payload(&value, login_method)?,
                        "web",
                    ));
                }
                Err(ProviderError::AuthRequired) => auth_failed = true,
                Err(err) => last_error = Some(err),
            }
        }

        if auth_failed {
            Err(ProviderError::AuthRequired)
        } else {
            Err(last_error.unwrap_or_else(|| ProviderError::Parse("No Qoder usage payload".into())))
        }
    }

    async fn fetch_region(
        &self,
        url: &str,
        referer: &str,
        cookie: &str,
    ) -> Result<Value, ProviderError> {
        let response = self
            .client
            .get(url)
            .header("Cookie", cookie)
            .header("Accept", "application/json, text/plain, */*")
            .header("Referer", referer)
            .header("User-Agent", USER_AGENT)
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            || response.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(ProviderError::AuthRequired);
        }
        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Qoder usage returned status {}",
                response.status()
            )));
        }
        response
            .json::<Value>()
            .await
            .map_err(|e| ProviderError::Parse(format!("Failed to parse Qoder usage: {e}")))
    }
}

impl Default for QoderProvider {
    fn default() -> Self {
        Self::new()
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
    let pairs = header
        .split(';')
        .filter_map(|chunk| {
            let (name, value) = chunk.trim().split_once('=')?;
            let name = name.trim();
            let value = value.trim();
            (!name.is_empty() && !value.is_empty()).then(|| format!("{name}={value}"))
        })
        .collect::<Vec<_>>();
    (!pairs.is_empty()).then(|| pairs.join("; "))
}

fn snapshot_from_payload(
    value: &Value,
    login_method: &str,
) -> Result<UsageSnapshot, ProviderError> {
    let root = value.get("data").unwrap_or(value);
    if let Some(snapshot) = quota_summary_snapshot(root, login_method)? {
        return Ok(snapshot);
    }

    let windows = collect_credit_windows(root);
    let primary = windows
        .first()
        .cloned()
        .ok_or_else(|| ProviderError::Parse("Missing Qoder credit usage".into()))?;
    let mut snapshot = UsageSnapshot::new(primary).with_login_method(login_method);
    if let Some(shared) = windows.get(1).cloned() {
        snapshot = snapshot.with_secondary(shared);
    }
    for (idx, window) in windows.into_iter().skip(2).enumerate() {
        snapshot =
            snapshot.with_extra_rate_window(format!("qoder-{idx}"), "Additional credits", window);
    }
    Ok(snapshot)
}

#[derive(Debug, Clone, Copy)]
struct QoderQuotaSummary {
    used: f64,
    total: f64,
    remaining: f64,
    percentage: f64,
    unit: Option<&'static str>,
}

fn quota_summary_snapshot(
    root: &Value,
    login_method: &str,
) -> Result<Option<UsageSnapshot>, ProviderError> {
    let Some(base) = quota_summary(root, &["totalQuota", "total_quota"])? else {
        return Ok(None);
    };
    let shared = quota_summary(root, &["sharedQuota", "shared_quota"])?;
    let merged = if let Some(shared) = shared {
        merge_quota_summaries(base, shared)?
    } else {
        base
    };
    let reset =
        string_from_value_keys(root, &["nextResetAt", "next_reset_at"]).and_then(parse_datetime);
    let description = Some(format!(
        "{:.0}/{:.0} {} used, {:.0} remaining",
        merged.used,
        merged.total,
        merged.unit.unwrap_or("credits"),
        merged.remaining
    ));
    Ok(Some(
        UsageSnapshot::new(RateWindow::with_details(
            merged.percentage,
            None,
            reset,
            description,
        ))
        .with_login_method(login_method),
    ))
}

fn quota_summary(
    root: &Value,
    container_keys: &[&str],
) -> Result<Option<QoderQuotaSummary>, ProviderError> {
    let Some(container) = container_keys.iter().find_map(|key| root.get(*key)) else {
        return Ok(None);
    };
    let Some(summary) = container
        .get("quotaSummary")
        .or_else(|| container.get("quota_summary"))
    else {
        return Ok(None);
    };

    let used = number_from_value_keys(summary, &["usedValue", "used_value"])
        .ok_or_else(|| ProviderError::Parse("Missing Qoder usedValue".into()))?;
    let total = number_from_value_keys(summary, &["limitValue", "limit_value"])
        .ok_or_else(|| ProviderError::Parse("Missing Qoder limitValue".into()))?;
    let remaining = number_from_value_keys(summary, &["remainingValue", "remaining_value"])
        .unwrap_or_else(|| (total - used).max(0.0));
    let provided = number_from_value_keys(summary, &["usagePercentage", "usage_percentage"]);
    let percentage = usage_percentage(used, total, remaining, provided)?;
    let unit = string_from_value_keys(summary, &["unit"]).map(|unit| {
        if unit.eq_ignore_ascii_case("credit") || unit.eq_ignore_ascii_case("credits") {
            "credits"
        } else {
            "units"
        }
    });

    Ok(Some(QoderQuotaSummary {
        used,
        total,
        remaining,
        percentage,
        unit,
    }))
}

fn merge_quota_summaries(
    base: QoderQuotaSummary,
    shared: QoderQuotaSummary,
) -> Result<QoderQuotaSummary, ProviderError> {
    let used = base.used + shared.used;
    let total = base.total + shared.total;
    let remaining = base.remaining + shared.remaining;
    Ok(QoderQuotaSummary {
        used,
        total,
        remaining,
        percentage: usage_percentage(used, total, remaining, None)?,
        unit: base.unit.or(shared.unit),
    })
}

fn usage_percentage(
    used: f64,
    total: f64,
    remaining: f64,
    provided: Option<f64>,
) -> Result<f64, ProviderError> {
    if used < 0.0 || total < 0.0 || remaining < 0.0 {
        return Err(ProviderError::Parse(
            "Qoder quota values must be nonnegative".into(),
        ));
    }
    if total == 0.0 {
        if used != 0.0 || remaining != 0.0 {
            return Err(ProviderError::Parse(
                "Qoder zero total quota has nonzero usage".into(),
            ));
        }
        return Ok(provided.unwrap_or(100.0));
    }
    Ok(provided.unwrap_or(used / total * 100.0))
}

fn number_from_value_keys(value: &Value, keys: &[&str]) -> Option<f64> {
    let map = value.as_object()?;
    number_from_keys(map, keys)
}

fn string_from_value_keys(value: &Value, keys: &[&str]) -> Option<String> {
    let map = value.as_object()?;
    string_from_keys(map, keys)
}

fn collect_credit_windows(value: &Value) -> Vec<RateWindow> {
    let mut out = Vec::new();
    collect_credit_windows_inner(value, &mut out);
    out.sort_by(|a, b| b.used_percent.total_cmp(&a.used_percent));
    out
}

fn collect_credit_windows_inner(value: &Value, out: &mut Vec<RateWindow>) {
    match value {
        Value::Object(map) => {
            if let Some(window) = rate_window_from_object(map) {
                out.push(window);
            }
            for value in map.values() {
                collect_credit_windows_inner(value, out);
            }
        }
        Value::Array(items) => {
            for value in items {
                collect_credit_windows_inner(value, out);
            }
        }
        _ => {}
    }
}

fn rate_window_from_object(map: &serde_json::Map<String, Value>) -> Option<RateWindow> {
    let used = number_from_keys(
        map,
        &[
            "used",
            "usedCredits",
            "used_credits",
            "usage",
            "usedQuota",
            "used_quota",
        ],
    );
    let total = number_from_keys(
        map,
        &[
            "total",
            "totalCredits",
            "total_credits",
            "limit",
            "quota",
            "quotaLimit",
            "quota_limit",
        ],
    );
    let percent = number_from_keys(
        map,
        &[
            "usedPercent",
            "used_percent",
            "usagePercent",
            "usage_percent",
            "percent",
        ],
    )
    .or_else(|| match (used, total) {
        (Some(used), Some(total)) if total > 0.0 => Some(used / total * 100.0),
        _ => None,
    })?;
    let reset = string_from_keys(
        map,
        &["nextResetAt", "next_reset_at", "resetAt", "reset_at"],
    )
    .and_then(parse_datetime);
    let description = match (used, total) {
        (Some(used), Some(total)) => Some(format!("{used:.0}/{total:.0} credits")),
        (Some(used), None) => Some(format!("{used:.0} credits used")),
        _ => None,
    };
    Some(RateWindow::with_details(
        normalized_percent(percent),
        None,
        reset,
        description,
    ))
}

fn number_from_keys(map: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| match map.get(*key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    })
}

fn string_from_keys(map: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| match map.get(*key)? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

fn normalized_percent(value: f64) -> f64 {
    if value <= 1.0 { value * 100.0 } else { value }
}

fn parse_datetime(raw: String) -> Option<DateTime<Utc>> {
    if let Ok(number) = raw.parse::<f64>() {
        let seconds = if number > 10_000_000_000.0 {
            number / 1000.0
        } else {
            number
        };
        // Epoch seconds (or ms normalized above); real dates are far below i64::MAX.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "normalized epoch seconds fit i64"
        )]
        let secs = seconds as i64;
        return DateTime::<Utc>::from_timestamp(secs, 0);
    }
    DateTime::parse_from_rfc3339(&raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

#[async_trait]
impl Provider for QoderProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Qoder
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto | SourceMode::Web => {
                let cookie = match ctx.manual_cookie_header.as_deref() {
                    Some(cookie) => cookie.to_string(),
                    None => crate::providers::browser_cookie_header(&[
                        "qoder.com",
                        "www.qoder.com",
                        "qoder.com.cn",
                        "www.qoder.com.cn",
                    ])?,
                };
                self.fetch_web(&cookie).await
            }
            SourceMode::OAuth | SourceMode::Cli => {
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

    #[test]
    fn parses_qoder_credit_payload() {
        let payload = serde_json::json!({
            "data": {
                "totalCredits": 1000,
                "usedCredits": 125,
                "nextResetAt": "2026-07-03T00:00:00Z"
            }
        });
        let snapshot = snapshot_from_payload(&payload, "Qoder").unwrap();
        assert_eq!(snapshot.primary.used_percent, 12.5);
        assert!(snapshot.primary.resets_at.is_some());
    }

    #[test]
    fn parses_qoder_quota_summary_payload() {
        let payload = serde_json::json!({
            "totalQuota": {
                "quotaSummary": {
                    "usedValue": 50,
                    "limitValue": 200,
                    "remainingValue": 150,
                    "unit": "credits"
                }
            },
            "sharedQuota": {
                "quota_summary": {
                    "used_value": 25,
                    "limit_value": 100,
                    "remaining_value": 75,
                    "usage_percentage": 25
                }
            },
            "nextResetAt": 1783036800000_i64
        });
        let snapshot = snapshot_from_payload(&payload, "Qoder").unwrap();
        assert_eq!(snapshot.primary.used_percent, 25.0);
        assert_eq!(
            snapshot.primary.reset_description.as_deref(),
            Some("75/300 credits used, 225 remaining")
        );
        assert!(snapshot.primary.resets_at.is_some());
    }

    #[test]
    fn qoder_zero_total_quota_defaults_to_exhausted() {
        let payload = serde_json::json!({
            "totalQuota": {
                "quotaSummary": {
                    "usedValue": 0,
                    "limitValue": 0,
                    "remainingValue": 0
                }
            }
        });
        let snapshot = snapshot_from_payload(&payload, "Qoder").unwrap();
        assert_eq!(snapshot.primary.used_percent, 100.0);
    }

    #[test]
    fn normalizes_cookie_header() {
        assert_eq!(
            normalize_cookie_header("Cookie: a=1; empty=; b=2").as_deref(),
            Some("a=1; b=2")
        );
    }
}
