use chrono::{DateTime, Utc};
use reqwest::Client;

use crate::core::{FetchContext, ProviderError, ProviderFetchResult, RateWindow, UsageSnapshot};

const USAGE_API_URL: &str = "https://opencode.ai/zen/go/v1/usage";

pub(super) fn normalized_api_key(raw: Option<&str>) -> Option<String> {
    let mut value = raw?.trim();
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        value = value[1..value.len() - 1].trim();
    }
    (!value.is_empty()).then(|| value.to_string())
}

pub(super) fn resolve_api_key(ctx: &FetchContext) -> Option<String> {
    normalized_api_key(ctx.api_key.as_deref()).or_else(|| {
        std::env::var("OPENCODE_API_KEY")
            .ok()
            .and_then(|value| normalized_api_key(Some(&value)))
    })
}

pub(super) async fn fetch(
    client: &Client,
    ctx: &FetchContext,
    api_key: &str,
    source_label: &str,
) -> Result<ProviderFetchResult, ProviderError> {
    let response = client
        .get(USAGE_API_URL)
        .timeout(std::time::Duration::from_secs(ctx.web_timeout.max(1)))
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .header("User-Agent", "CodexBar")
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(ProviderError::AuthRequired);
    }
    if !status.is_success() {
        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|value| {
                ["message", "error", "detail"]
                    .into_iter()
                    .find_map(|key| value.get(key).and_then(serde_json::Value::as_str))
                    .map(str::to_string)
            })
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(ProviderError::Other(format!(
            "OpenCode Go API error: {message}"
        )));
    }
    let usage = parse_usage_text(&body, Utc::now())?;
    Ok(ProviderFetchResult::new(usage, source_label))
}

fn parse_usage_text(text: &str, now: DateTime<Utc>) -> Result<UsageSnapshot, ProviderError> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| ProviderError::Parse(format!("Invalid OpenCode Go API JSON: {e}")))?;
    let usage = value.get("usage").unwrap_or(&value);
    let rolling = api_window(usage, &["rolling", "rollingUsage", "rolling_usage"], now)
        .ok_or_else(|| ProviderError::Parse("Missing rolling usage window".to_string()))?;
    let mut snapshot = UsageSnapshot::new(RateWindow::with_details(
        rolling.0,
        Some(300),
        Some(rolling.1),
        None,
    ))
    .with_login_method("OpenCode Go");
    if let Some(weekly) = api_window(usage, &["weekly", "weeklyUsage", "weekly_usage"], now) {
        snapshot = snapshot.with_secondary(RateWindow::with_details(
            weekly.0,
            Some(10080),
            Some(weekly.1),
            None,
        ));
    }
    if let Some(monthly) = api_window(usage, &["monthly", "monthlyUsage", "monthly_usage"], now) {
        snapshot = snapshot.with_tertiary(RateWindow::with_details(
            monthly.0,
            RateWindow::monthly_window_minutes(Some(monthly.1)).or(Some(43200)),
            Some(monthly.1),
            None,
        ));
    }
    Ok(snapshot)
}

fn api_window(
    usage: &serde_json::Value,
    names: &[&str],
    now: DateTime<Utc>,
) -> Option<(f64, DateTime<Utc>)> {
    let object = names.iter().find_map(|name| usage.get(*name))?;
    let percent = [
        "percent",
        "usagePercent",
        "usedPercent",
        "percentUsed",
        "utilization",
    ]
    .into_iter()
    .find_map(|key| {
        object.get(key).and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        })
    })?;
    // GET /zen/go/v1/usage returns whole percentages in 0..=100 (including exact
    // 1% / 0.5%). Do not treat values in (0, 1] as 0..=1 fractions: that turns
    // a real 1% into a false 100% exhausted state (win-fork #247 / upstream #3216).
    let percent = percent.clamp(0.0, 100.0);
    let reset = [
        "resetInSec",
        "resetInSeconds",
        "resetSeconds",
        "reset_in_sec",
    ]
    .into_iter()
    .find_map(|key| {
        object.get(key).and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        })
    })
    .map(|seconds| now + chrono::Duration::seconds(seconds.max(0)))
    .or_else(|| {
        ["resetsAt", "resetAt", "resets_at", "reset_at"]
            .into_iter()
            .find_map(|key| object.get(key).and_then(serde_json::Value::as_str))
            .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
            .map(|timestamp| timestamp.with_timezone(&Utc))
    })?;
    Some((percent, reset))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_normalization_matches_upstream_settings_reader() {
        assert_eq!(
            normalized_api_key(Some("  go_test  ")).as_deref(),
            Some("go_test")
        );
        assert_eq!(
            normalized_api_key(Some("'go_quoted'")).as_deref(),
            Some("go_quoted")
        );
        assert_eq!(normalized_api_key(Some("   ")), None);
    }

    #[test]
    fn parses_public_usage_api_windows() {
        let now = DateTime::parse_from_rfc3339("2026-08-12T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let text = r#"{"usage":{"rolling":{"percent":12,"resetsAt":"2026-08-12T02:00:00.000Z"},"weekly":{"percent":8,"resetsAt":"2026-08-18T00:00:00.000Z"},"monthly":{"percent":35,"resetsAt":"2026-09-01T00:00:00.000Z"}}}"#;
        let snapshot = parse_usage_text(text, now).unwrap();
        assert!((snapshot.primary.used_percent - 12.0).abs() < 0.001);
        assert!((snapshot.secondary.as_ref().unwrap().used_percent - 8.0).abs() < 0.001);
        assert!((snapshot.tertiary.as_ref().unwrap().used_percent - 35.0).abs() < 0.001);
    }

    #[test]
    fn preserves_whole_percent_units_including_exact_one() {
        let now = DateTime::parse_from_rfc3339("2026-08-31T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        // Live Go API shape: percent is already 0..=100. Exact 1 must stay 1%.
        let text = r#"{"usage":{"rolling":{"status":"ok","percent":1,"resetsAt":"2026-08-31T17:12:09.085Z"},"weekly":{"status":"ok","percent":0,"resetsAt":"2026-09-07T00:00:00.085Z"},"monthly":{"status":"ok","percent":23,"resetsAt":"2026-09-20T04:39:33.085Z"}}}"#;
        let snapshot = parse_usage_text(text, now).unwrap();
        assert!(
            (snapshot.primary.used_percent - 1.0).abs() < 0.001,
            "rolling 1% must not be rescaled to 100%, got {}",
            snapshot.primary.used_percent
        );
        assert!((snapshot.secondary.as_ref().unwrap().used_percent - 0.0).abs() < 0.001);
        assert!((snapshot.tertiary.as_ref().unwrap().used_percent - 23.0).abs() < 0.001);
    }

    #[test]
    fn preserves_sub_one_percent_without_fraction_rescale() {
        let now = DateTime::parse_from_rfc3339("2026-08-31T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let text = r#"{"usage":{"rolling":{"percent":0.5,"resetsAt":"2026-08-31T17:00:00Z"},"weekly":{"percent":100,"resetsAt":"2026-09-07T00:00:00Z"}}}"#;
        let snapshot = parse_usage_text(text, now).unwrap();
        assert!(
            (snapshot.primary.used_percent - 0.5).abs() < 0.001,
            "0.5% must stay 0.5, got {}",
            snapshot.primary.used_percent
        );
        assert!((snapshot.secondary.as_ref().unwrap().used_percent - 100.0).abs() < 0.001);
    }
}
