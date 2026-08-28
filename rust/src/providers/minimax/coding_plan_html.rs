//! MiniMax coding-plan HTML scrape — `__NEXT_DATA__` and visible-text fallback.
//!
//! Companion to `coding_plan` (JSON/remains parsers). Upstream reference is
//! `steipete/CodexBar`' `MiniMaxUsageFetcher.swift`.

use chrono::{DateTime, Duration, FixedOffset, Utc};
use regex_lite::Regex;
use serde_json::Value;

use crate::core::{ProviderError, RateWindow, UsageSnapshot};

use super::coding_plan::{MiniMaxCodingPlanSnapshot, parse_coding_plan_value};
#[cfg(test)]
use super::coding_plan::{RemainsRow, ServiceRow};

/// Recursively walk the JSON object tree for the first object having
/// `model_remains` or `modelRemains` (upstream `findCodingPlanPayload`).
fn find_coding_plan_payload(obj: &Value) -> Option<&Value> {
    if let Value::Object(map) = obj {
        if map.contains_key("model_remains") || map.contains_key("modelRemains") {
            return Some(obj);
        }
        for value in map.values() {
            if let Some(matched) = find_coding_plan_payload(value) {
                return Some(matched);
            }
        }
    }
    if let Value::Array(arr) = obj {
        for value in arr {
            if let Some(matched) = find_coding_plan_payload(value) {
                return Some(matched);
            }
        }
    }
    None
}

/// Extract `__NEXT_DATA__` JSON from HTML (upstream `nextDataJSONData`).
fn next_data_json(html: &str) -> Option<Value> {
    let needle = "id=\"__NEXT_DATA__\"";
    let id_pos = html.find(needle)?;
    let after_id = &html[id_pos + needle.len()..];
    let open_tag_end = after_id.find('>')?;
    let content_start = &after_id[open_tag_end + 1..];
    let close_pos = content_start.find("</script>")?;
    let raw = &content_start[..close_pos];
    let trimmed = raw.trim_matches(|c: char| c == '\t' || c == '\n' || c == '\r' || c == ' ');
    let trimmed = trimmed.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

/// Strip HTML tags to visible text (upstream `visibleText` + `stripHTML`).
fn visible_text(html: &str) -> String {
    let patterns: &[(&str, &str)] = &[
        (r"(?is)<script\b[^>]*>.*?</script>", ""),
        (r"(?is)<style\b[^>]*>.*?</style>", ""),
        (r"(?is)<!--.*?-->", ""),
        (r"<[^>]+>", " "),
        (r"\s+", " "),
    ];
    let mut text = html.to_string();
    for (pattern, replacement) in patterns {
        if let Ok(re) = Regex::new(pattern) {
            text = re.replace_all(&text, *replacement).to_string();
        }
    }
    text.trim().to_string()
}

/// Visible text for scraping, with HTML entity decoding.
fn strip_html(html: &str) -> String {
    let mut text = visible_text(html);
    text = text.replace("&nbsp;", " ");
    text = text.replace("&amp;", "&");
    text = text.replace("&lt;", "<");
    text = text.replace("&gt;", ">");
    // collapse whitespace again after entity expansion
    if let Ok(re) = Regex::new(r"\s+") {
        text = re.replace_all(&text, " ").to_string();
    }
    text.trim().to_string()
}

/// Upstream `looksSignedOut`.
fn looks_signed_out(html: &str) -> bool {
    let lower = visible_text(html).to_lowercase();
    lower.contains("sign in")
        || lower.contains("log in")
        || lower.contains("登录")
        || lower.contains("登入")
}

/// Extract first capture group from a regex.
fn extract_first(pattern: &str, text: &str) -> Option<String> {
    let re = Regex::new(pattern).ok()?;
    let caps = re.captures(text)?;
    caps.get(1).map(|m| m.as_str().trim().to_string())
}

/// Extract all capture groups from a regex.
fn extract_match(pattern: &str, text: &str) -> Option<Vec<String>> {
    let re = Regex::new(pattern).ok()?;
    let caps = re.captures(text)?;
    let groups: Vec<String> = (1..caps.len())
        .map(|i| {
            caps.get(i)
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default()
        })
        .collect();
    if groups.is_empty() {
        None
    } else {
        Some(groups)
    }
}

/// Parse the plan name from HTML (upstream `parsePlanName(html:text:)`).
fn parse_plan_name_from_html(html: &str, text: &str) -> Option<String> {
    let candidates = [
        extract_first(r#"(?i)"planName"\s*:\s*"([^"]+)""#, html),
        extract_first(r#"(?i)"plan"\s*:\s*"([^"]+)""#, html),
        extract_first(r#"(?i)"packageName"\s*:\s*"([^"]+)""#, html),
        extract_first(
            r#"(?i)Coding\s*Plan\s*([A-Za-z0-9][A-Za-z0-9\s._-]{0,32})"#,
            text,
        ),
    ];
    for candidate in candidates.into_iter().flatten() {
        // strip trailing " available usage..."
        let cleaned = if let Ok(re) = Regex::new(r"(?i)\s+available\s+usage.*$") {
            re.replace_all(&candidate, "").trim().to_string()
        } else {
            candidate.trim().to_string()
        };
        if !cleaned.is_empty() {
            return Some(cleaned);
        }
    }
    None
}

/// Parse "Available usage: N prompts / X hours" (upstream `parseAvailableUsage`).
fn parse_available_usage(text: &str) -> Option<(i64, u32)> {
    let pattern = r#"(?i)available\s+usage[:\s]*([0-9][0-9,]*)\s*prompts?\s*/\s*([0-9]+(?:\.[0-9]+)?)\s*(hours?|hrs?|h|minutes?|mins?|m|days?|d)"#;
    let caps = extract_match(pattern, text)?;
    if caps.len() < 3 {
        return None;
    }
    let prompts_raw = &caps[0];
    let duration_raw = &caps[1];
    let unit_raw = &caps[2];
    let prompts: i64 = prompts_raw.replace(',', "").parse().ok()?;
    if prompts <= 0 {
        return None;
    }
    let duration: f64 = duration_raw.parse().ok()?;
    let window_minutes = minutes_from_duration(duration, unit_raw);
    if window_minutes == 0 {
        return None;
    }
    Some((prompts, window_minutes))
}

/// Convert a duration + unit to minutes (upstream `minutes(from:unit:)`).
fn minutes_from_duration(value: f64, unit: &str) -> u32 {
    // Window lengths come from the provider's own dashboard text and are
    // minutes-scale; u32 overflow would need a >8000-year window.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "dashboard window durations are minutes-scale; u32 is far beyond any real window"
    )]
    let to_minutes = |scaled: f64| -> u32 { scaled.round() as u32 };
    let lower = unit.to_lowercase();
    if lower.starts_with('d') {
        return to_minutes(value * 24.0 * 60.0);
    }
    if lower.starts_with('h') {
        return to_minutes(value * 60.0);
    }
    if lower.starts_with('m') {
        return to_minutes(value);
    }
    if lower.starts_with('s') {
        return to_minutes(value / 60.0).max(1);
    }
    0
}

/// Parse "37% used" or "used 37%" (upstream `parseUsedPercent`).
fn parse_used_percent(text: &str) -> Option<f64> {
    let patterns = [
        r#"(?i)([0-9]{1,3}(?:\.[0-9]+)?)\s*%\s*used"#,
        r#"(?i)used\s*([0-9]{1,3}(?:\.[0-9]+)?)\s*%"#,
    ];
    for pattern in patterns {
        if let Some(raw) = extract_first(pattern, text)
            && let Ok(value) = raw.parse::<f64>()
            && (0.0..=100.0).contains(&value)
        {
            return Some(value);
        }
    }
    None
}

/// Parse a reset timestamp from visible text (upstream `parseResetsAt`).
fn parse_resets_at_from_text(text: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    // "Resets in N <unit>"
    let pattern = r#"(?i)resets?\s+in\s+([0-9]+)\s*(seconds?|secs?|s|minutes?|mins?|m|hours?|hrs?|h|days?|d)"#;
    if let Some(caps) = extract_match(pattern, text)
        && caps.len() >= 2
        && let Ok(value) = caps[0].parse::<f64>()
    {
        let unit = &caps[1];
        let seconds = seconds_from_duration(value, unit);
        // "Resets in N <unit>" values are parsed from dashboard text and
        // rounded to whole seconds.
        #[allow(
            clippy::cast_possible_truncation,
            reason = "reset countdown truncated to whole seconds by design"
        )]
        let total_seconds = seconds as i64;
        return Some(now + Duration::seconds(total_seconds));
    }

    // "Resets at HH:mm (zone hint)"
    let pattern = r#"(?i)resets?\s+at\s+([0-9]{1,2}:[0-9]{2})(?:\s*\(([^)]+)\))?"#;
    if let Some(caps) = extract_match(pattern, text)
        && !caps.is_empty()
    {
        let time_text = &caps[0];
        let tz_hint = caps.get(1).map(|s| s.as_str());
        return date_for_time(time_text, tz_hint, now);
    }

    None
}

/// Convert a duration + unit to seconds (upstream `seconds(from:unit:)`).
fn seconds_from_duration(value: f64, unit: &str) -> f64 {
    let lower = unit.to_lowercase();
    if lower.starts_with('d') {
        return value * 24.0 * 60.0 * 60.0;
    }
    if lower.starts_with('h') {
        return value * 60.0 * 60.0;
    }
    if lower.starts_with('m') {
        return value * 60.0;
    }
    value
}

/// Parse "HH:mm" with a timezone hint, rolling to tomorrow if past (upstream
/// `dateForTime`). TZ hint supports only `UTC±H[:MM]`/`GMT±H[:MM]`; non-numeric
/// hints default to UTC (chrono has no IANA database).
fn date_for_time(time: &str, tz_hint: Option<&str>, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let tz = match tz_hint {
        Some(hint) => timezone_from_hint(hint),
        None => FixedOffset::east_opt(0).unwrap(),
    };
    let time_only = chrono::NaiveTime::parse_from_str(time.trim(), "%H:%M").ok()?;
    let today = now.date_naive();
    let dt = today.and_time(time_only).and_local_timezone(tz).single()?;
    let mut candidate = dt.with_timezone(&Utc);
    if candidate < now {
        candidate = now + Duration::days(1);
    }
    Some(candidate)
}

/// Parse a `UTC±H[:MM]`/`GMT±H[:MM]` hint into a `FixedOffset`. Non-numeric hints
/// return UTC.
fn timezone_from_hint(hint: &str) -> chrono::offset::FixedOffset {
    let trimmed = hint.trim();
    let pattern = r"(?i)^(?:UTC|GMT)\s*([+-])\s*(\d{1,2})(?::?(\d{2}))?$";
    if let Some(caps) = extract_match(pattern, trimmed)
        && caps.len() >= 2
    {
        let sign = if caps[0] == "-" { -1 } else { 1 };
        let hours: i32 = caps[1].parse().unwrap_or(0);
        let minutes: i32 = if caps.len() >= 3 {
            caps[2].parse().unwrap_or(0)
        } else {
            0
        };
        let total = sign * (hours * 3600 + minutes * 60);
        if let Some(offset) = FixedOffset::east_opt(total) {
            return offset;
        }
    }
    FixedOffset::east_opt(0).unwrap()
}

/// The HTML parser (upstream `parse(html:now:)`).
pub(super) fn parse_coding_plan_html(
    html: &str,
    now: DateTime<Utc>,
) -> Result<MiniMaxCodingPlanSnapshot, ProviderError> {
    // Signed-out check first
    if looks_signed_out(html) {
        return Err(ProviderError::AuthRequired);
    }

    // __NEXT_DATA__ first
    if let Some(json) = next_data_json(html)
        && let Some(payload) = find_coding_plan_payload(&json)
        && let Ok(snapshot) = parse_coding_plan_value(payload, now)
    {
        return Ok(snapshot);
    }

    // Visible-text scrape
    let text = strip_html(html);
    let plan_name = parse_plan_name_from_html(html, &text);
    let available = parse_available_usage(&text);
    let used_percent = parse_used_percent(&text);
    let resets_at = parse_resets_at_from_text(&text, now);

    if plan_name.is_none() && available.is_none() && used_percent.is_none() {
        return Err(ProviderError::Parse("Missing coding plan data.".into()));
    }

    Ok(MiniMaxCodingPlanSnapshot::Html {
        plan_name,
        available_prompts: available.map(|(p, _)| p),
        window_minutes: available.map(|(_, w)| w),
        used_percent,
        resets_at,
    })
}

/// Map a parsed snapshot onto the shared `UsageSnapshot` model.
pub(super) fn to_usage_snapshot(
    snapshot: &MiniMaxCodingPlanSnapshot,
    _now: DateTime<Utc>,
) -> Result<UsageSnapshot, ProviderError> {
    match snapshot {
        MiniMaxCodingPlanSnapshot::Services(rows) => {
            if rows.is_empty() {
                return Err(ProviderError::Parse("Missing coding plan data.".into()));
            }
            let first = &rows[0];
            let primary = RateWindow::with_details(
                first.percent,
                None,
                first.resets_at,
                first.reset_description.clone(),
            );
            let mut usage = UsageSnapshot::new(primary);

            // Secondary = second row if present
            if rows.len() >= 2 {
                let second = &rows[1];
                usage = usage.with_secondary(RateWindow::with_details(
                    second.percent,
                    None,
                    second.resets_at,
                    second.reset_description.clone(),
                ));
            }

            // Extra windows for remaining rows
            for row in rows.iter().skip(if rows.len() >= 2 { 2 } else { 1 }) {
                usage = usage.with_extra_rate_window(
                    &row.service_type,
                    format!("{} · {}", row.service_type, row.window_type),
                    RateWindow::with_details(
                        row.percent,
                        None,
                        row.resets_at,
                        row.reset_description.clone(),
                    ),
                );
            }

            // Login method from first service_type containing "pro" or "max"
            let login = rows.iter().find_map(|r| {
                let lower = r.service_type.to_lowercase();
                if lower.contains("pro") || lower.contains("max") {
                    Some(r.service_type.clone())
                } else {
                    None
                }
            });
            if let Some(lm) = login {
                usage = usage.with_login_method(lm);
            }

            Ok(usage)
        }
        MiniMaxCodingPlanSnapshot::Remains { plan_name, rows } => {
            if rows.is_empty() {
                return Err(ProviderError::Parse("Missing coding plan data.".into()));
            }

            // Primary = first row with a computable percent (skip placeholders)
            let primary_row = rows
                .iter()
                .find(|r| !r.is_unlimited)
                .or_else(|| rows.first())
                .ok_or_else(|| ProviderError::Parse("Missing coding plan data.".into()))?;

            let primary = RateWindow::with_details(
                primary_row.percent,
                primary_row.window_minutes,
                primary_row.resets_at,
                primary_row.reset_description.clone(),
            );
            let mut usage = UsageSnapshot::new(primary);

            // Secondary = first qualifying weekly row
            if let Some(weekly) = rows.iter().find(|r| r.is_weekly) {
                usage = usage.with_secondary(RateWindow::with_details(
                    weekly.percent,
                    weekly.window_minutes,
                    weekly.resets_at,
                    weekly.reset_description.clone(),
                ));
            }

            // Every row as an extra window
            for row in rows {
                let id = if row.is_weekly {
                    format!("{}:weekly", row.service_type)
                } else {
                    row.service_type.clone()
                };
                let title = format!("{} · {}", row.service_type, row.window_type);
                usage = usage.with_extra_rate_window(
                    id,
                    title,
                    RateWindow::with_details(
                        row.percent,
                        row.window_minutes,
                        row.resets_at,
                        row.reset_description.clone(),
                    ),
                );
            }

            if let Some(pn) = plan_name {
                usage = usage.with_login_method(pn);
            }

            Ok(usage)
        }
        MiniMaxCodingPlanSnapshot::Html {
            plan_name,
            available_prompts: _,
            window_minutes,
            used_percent,
            resets_at,
        } => {
            let pct = used_percent.unwrap_or(0.0);
            let is_informational = used_percent.is_none();
            let mut primary = RateWindow::with_details(pct, *window_minutes, *resets_at, None);
            primary.is_informational = is_informational;
            let mut usage = UsageSnapshot::new(primary);
            if let Some(pn) = plan_name {
                usage = usage.with_login_method(pn);
            }
            Ok(usage)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0).unwrap()
    }

    #[test]
    fn parses_camelcase_next_data_html() {
        let entry = serde_json::json!({
            "model_name": "General",
            "current_interval_total_count": 100,
            "current_interval_usage_count": 30,
            "current_interval_remaining_percent": 70,
            "start_time": 1722691200,
            "end_time": 1722777600,
            "current_interval_status": 0
        });
        let html = format!(
            r#"<html><head></head><body><script id="__NEXT_DATA__" type="application/json">{{"props":{{"pageProps":{{"data":{{"modelRemains":[{}]}}}}}}}}</script></body></html>"#,
            entry
        );
        let snapshot = parse_coding_plan_html(&html, now()).unwrap();
        match snapshot {
            MiniMaxCodingPlanSnapshot::Remains { rows, .. } => {
                // remainingPercent 70 → used 30%
                assert!((rows[0].percent - 30.0).abs() < 0.01);
            }
            _ => panic!("expected Remains from __NEXT_DATA__"),
        }
    }
    #[test]
    fn parses_html_scrape() {
        let html = r#"<html><body>
            <div>"planName": "Plus"</div>
            <p>Available usage: 1,234 prompts / 5 hours</p>
            <span>37% used</span>
            <div>Resets in 2 hours</div>
        </body></html>"#;
        let n = now();
        let snapshot = parse_coding_plan_html(html, n).unwrap();
        match snapshot {
            MiniMaxCodingPlanSnapshot::Html {
                plan_name,
                available_prompts,
                window_minutes,
                used_percent,
                resets_at,
            } => {
                assert_eq!(plan_name.as_deref(), Some("Plus"));
                assert_eq!(available_prompts, Some(1234));
                assert_eq!(window_minutes, Some(300));
                assert!((used_percent.unwrap() - 37.0).abs() < 0.01);
                let resets = resets_at.unwrap();
                let diff = (resets - n).num_seconds();
                assert!((diff - 7200).abs() < 5);
            }
            _ => panic!("expected Html"),
        }
    }

    #[test]
    fn signed_out_html_is_auth_required() {
        let html = "<html><body><h1>请登录</h1></body></html>";
        let err = parse_coding_plan_html(html, now()).unwrap_err();
        assert!(matches!(err, ProviderError::AuthRequired));
    }
    #[test]
    fn to_usage_snapshot_remains_maps_primary_and_weekly() {
        let rows = vec![
            RemainsRow {
                service_type: "general".to_string(),
                model_name: "General".to_string(),
                window_type: "Today".to_string(),
                percent: 27.0,
                window_minutes: Some(1440),
                resets_at: Some(now() + Duration::hours(12)),
                reset_description: Some("Resets in 12 hours".to_string()),
                is_unlimited: false,
                is_weekly: false,
            },
            RemainsRow {
                service_type: "general".to_string(),
                model_name: "General".to_string(),
                window_type: "Weekly".to_string(),
                percent: 15.0,
                window_minutes: Some(10080),
                resets_at: Some(now() + Duration::days(5)),
                reset_description: Some("Resets in 5 days".to_string()),
                is_unlimited: false,
                is_weekly: true,
            },
        ];
        let snapshot = MiniMaxCodingPlanSnapshot::Remains {
            plan_name: Some("MiniMax Pro".to_string()),
            rows,
        };
        let usage = to_usage_snapshot(&snapshot, now()).unwrap();
        assert!((usage.primary.used_percent - 27.0).abs() < 0.01);
        assert_eq!(usage.primary.window_minutes, Some(1440));
        assert!(usage.secondary.is_some());
        assert!((usage.secondary.unwrap().used_percent - 15.0).abs() < 0.01);
        assert_eq!(usage.login_method.as_deref(), Some("MiniMax Pro"));
        // both rows should be in extra windows
        assert_eq!(usage.extra_rate_windows.len(), 2);
    }

    #[test]
    fn to_usage_snapshot_html_informational_when_no_percent() {
        let snapshot = MiniMaxCodingPlanSnapshot::Html {
            plan_name: Some("Plus".to_string()),
            available_prompts: Some(1000),
            window_minutes: Some(300),
            used_percent: None,
            resets_at: None,
        };
        let usage = to_usage_snapshot(&snapshot, now()).unwrap();
        assert!(usage.primary.is_informational);
        assert_eq!(usage.login_method.as_deref(), Some("Plus"));
    }

    #[test]
    fn to_usage_snapshot_services_maps_plan_name() {
        let rows = vec![ServiceRow {
            service_type: "Text Generation Pro".to_string(),
            window_type: "Today".to_string(),
            time_range: "2026/08/03 00:00 - 2026/08/04 00:00".to_string(),
            percent: 25.0,
            resets_at: None,
            reset_description: Some("Today: test".to_string()),
        }];
        let snapshot = MiniMaxCodingPlanSnapshot::Services(rows);
        let usage = to_usage_snapshot(&snapshot, now()).unwrap();
        assert!((usage.primary.used_percent - 25.0).abs() < 0.01);
        assert_eq!(usage.login_method.as_deref(), Some("Text Generation Pro"));
    }
}
