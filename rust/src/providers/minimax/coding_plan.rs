//! MiniMax coding-plan parser — upstream-parity port of the cookie/web fetch.
//!
//! Parses the coding-plan page JSON, `__NEXT_DATA__` payloads, the remains API
//! response, and visible-text HTML scrape into a snapshot that maps onto the
//! shared `UsageSnapshot`/`RateWindow` model. See the plan in issue #246 for
//! the exact formulas; upstream reference is `steipete/CodexBar`'
//! `MiniMaxUsageFetcher.swift`.

use chrono::{DateTime, Duration, FixedOffset, TimeZone, Utc};
use regex_lite::Regex;
use serde_json::Value;

use crate::core::{ProviderError, RateWindow, UsageSnapshot};

use super::{scalar_string, value_f64, value_i64};

/// Parsed result of a coding-plan fetch (page JSON, remains API, or HTML scrape).
#[derive(Debug, Clone)]
pub(super) enum MiniMaxCodingPlanSnapshot {
    /// Multi-service shape: `data.services[]`.
    Services(Vec<ServiceRow>),
    /// Single-service shape: `model_remains[]` entries.
    Remains {
        plan_name: Option<String>,
        rows: Vec<RemainsRow>,
    },
    /// Visible-text HTML scrape fallback.
    Html {
        plan_name: Option<String>,
        available_prompts: Option<i64>,
        window_minutes: Option<u32>,
        used_percent: Option<f64>,
        resets_at: Option<DateTime<Utc>>,
    },
}

/// A row in the multi-service `data.services[]` shape.
#[derive(Debug, Clone)]
pub(super) struct ServiceRow {
    pub service_type: String,
    pub window_type: String,
    pub time_range: String,
    pub percent: f64,
    pub resets_at: Option<DateTime<Utc>>,
    pub reset_description: Option<String>,
}

/// A row in the single-service `model_remains[]` shape.
#[derive(Debug, Clone)]
pub(super) struct RemainsRow {
    pub service_type: String,
    pub model_name: String,
    pub window_type: String,
    pub percent: f64,
    pub window_minutes: Option<u32>,
    pub resets_at: Option<DateTime<Utc>>,
    pub reset_description: Option<String>,
    pub is_unlimited: bool,
    pub is_weekly: bool,
    pub usage: Option<i64>,
    pub limit: Option<i64>,
}

// ponytail: skipped from upstream — optional `Authorization: Bearer` on cookie
// requests (our cookie import has no token concept), `GroupId` remains query
// param (no group id in cookie mode), `pointsBalance`, `formatMiniMaxDateTimeRange`
// for non-weekly rows, IANA timezone names, host/env URL overrides
// (`MiniMaxSettingsReader`).

/// Look up a value trying snake_case then camelCase (upstream normalizes the
/// camelCase aliases onto the snake_case keys).
fn get_field<'a>(obj: &'a Value, snake: &str, camel: &str) -> Option<&'a Value> {
    obj.get(snake).or_else(|| obj.get(camel))
}

/// Decode an epoch integer that may be in milliseconds, seconds, or unparseable.
fn date_from_epoch(value: Option<i64>) -> Option<DateTime<Utc>> {
    let raw = value?;
    if raw > 1_000_000_000_000 {
        Utc.timestamp_opt(raw / 1000, 0).single()
    } else if raw > 1_000_000_000 {
        Utc.timestamp_opt(raw, 0).single()
    } else {
        None
    }
}

/// `usedPercent(remainingPercent:)` — `min(100, max(0, 100 - remainingPercent))`.
fn used_percent_from_remaining(remaining_percent: f64) -> f64 {
    100.0 - remaining_percent.clamp(0.0, 100.0)
}

/// `usedPercent(total:remaining:)` — `max(0, total-remaining)/total*100` clamped.
fn used_percent_from_counts(total: i64, remaining: i64) -> Option<f64> {
    if total <= 0 {
        return None;
    }
    let used = (total - remaining).max(0);
    Some((used as f64 / total as f64 * 100.0).clamp(0.0, 100.0))
}

/// `percentQuotaLimit(boostPermille:)` — `boost > 0 ? max(1, round(boost/10)) : 100`.
fn percent_quota_limit(boost_permille: Option<i64>) -> i64 {
    match boost_permille {
        Some(boost) if boost > 0 => ((boost as f64 / 10.0).round() as i64).max(1),
        _ => 100,
    }
}

/// `windowMinutes(start:end:)`.
fn window_minutes(start: Option<DateTime<Utc>>, end: Option<DateTime<Utc>>) -> Option<u32> {
    let (start, end) = (start?, end?);
    let minutes = (end - start).num_minutes();
    if minutes > 0 {
        Some(minutes as u32)
    } else {
        None
    }
}

/// `resetsAt(end:remains:now:)`.
fn resets_at(
    end: Option<DateTime<Utc>>,
    remains: Option<i64>,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    if let Some(end) = end
        && end > now
    {
        return Some(end);
    }
    let remains = remains?;
    if remains <= 0 {
        return None;
    }
    let seconds = if remains > 1_000_000 {
        remains as f64 / 1000.0
    } else {
        remains as f64
    };
    Some(now + Duration::seconds(seconds as i64))
}

/// Upstream `mapModelNameToServiceType`.
fn map_model_name_to_service_type(model_name: &str) -> String {
    let lower = model_name.trim().to_lowercase();
    if lower == "general" || lower == "video" {
        return lower;
    }
    if is_text_generation_model_name(model_name) {
        return "Text Generation".to_string();
    }
    if lower.contains("speech") {
        return "Text to Speech".to_string();
    }
    if lower.contains("hailuo") && lower.contains("fast") {
        return "Image to Video".to_string();
    }
    if lower.contains("hailuo") {
        return "Text to Video".to_string();
    }
    if lower.starts_with("image-") {
        return "Image Generation".to_string();
    }
    if lower.contains("music") {
        return "Music Generation".to_string();
    }
    model_name.to_string()
}

/// Upstream `isTextGenerationModelName`.
fn is_text_generation_model_name(model_name: &str) -> bool {
    let lower = model_name.to_lowercase();
    lower == "general" || lower.contains("minimax-m") || lower.starts_with("m2.")
}

/// Upstream `shouldRenderWeeklyWindow` — weekly rows only for text-generation models.
fn should_render_weekly_window(model_name: &str) -> bool {
    is_text_generation_model_name(model_name)
}

/// Upstream `isUnavailableQuotaPlaceholder` (with the unlimited-weekly exception).
fn is_unavailable_quota_placeholder(
    service_type: &str,
    window_type_override: Option<&str>,
    status: Option<i64>,
    total: Option<i64>,
    remaining: Option<i64>,
    remaining_percent: Option<f64>,
) -> bool {
    // The unlimited-weekly exception is NOT a placeholder.
    if let Some(window) = window_type_override
        && is_unlimited_quota_window(service_type, window, status, remaining_percent)
    {
        return false;
    }
    status == Some(3)
        && total.unwrap_or(0) == 0
        && remaining.unwrap_or(0) == 0
        && remaining_percent.map(|p| p >= 100.0).unwrap_or(false)
}

/// Upstream `isUnlimitedQuotaWindow`.
fn is_unlimited_quota_window(
    service_type: &str,
    window_type: &str,
    status: Option<i64>,
    remaining_percent: Option<f64>,
) -> bool {
    let normalized_service = service_type.trim().to_lowercase();
    let normalized_window = window_type.trim().to_lowercase();
    status == Some(3)
        && matches!(normalized_service.as_str(), "text generation" | "general")
        && normalized_window == "weekly"
        && remaining_percent.map(|p| p >= 100.0).unwrap_or(false)
}

/// Window type from duration start→end (upstream `parseWindowInfo`).
fn window_type_from_duration(start: Option<DateTime<Utc>>, end: Option<DateTime<Utc>>) -> String {
    let (Some(start), Some(end)) = (start, end) else {
        return "Unknown".to_string();
    };
    let duration_hours = (end - start).num_seconds() as f64 / 3600.0;
    if (23.0..=25.0).contains(&duration_hours) {
        "Today".to_string()
    } else if (4.0..=6.0).contains(&duration_hours) {
        "5 hours".to_string()
    } else if (1.0..23.0).contains(&duration_hours) {
        format!("{} hours", duration_hours as i64)
    } else {
        "Custom".to_string()
    }
}

/// `timeRange` formatted with UTC+8 offset.
fn time_range_string(start: Option<DateTime<Utc>>, end: Option<DateTime<Utc>>) -> String {
    let (Some(start), Some(end)) = (start, end) else {
        return "N/A".to_string();
    };
    let tz = FixedOffset::east_opt(8 * 3600).unwrap();
    format!(
        "{}-{}(UTC+8)",
        start.with_timezone(&tz).format("%H:%M"),
        end.with_timezone(&tz).format("%H:%M")
    )
}

/// Weekly `timeRange` in `MM/dd HH:mm - MM/dd HH:mm(UTC+8)` format.
fn weekly_time_range_string(
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> Option<String> {
    let start = start?;
    let end = end?;
    let tz = FixedOffset::east_opt(8 * 3600).unwrap();
    Some(format!(
        "{} - {}(UTC+8)",
        start.with_timezone(&tz).format("%m/%d %H:%M"),
        end.with_timezone(&tz).format("%m/%d %H:%M")
    ))
}

/// Upstream `resetDescription(for:timeRange:now:resetsAt:)`.
fn reset_description(
    window_type: &str,
    time_range: &str,
    now: DateTime<Utc>,
    resets_at: Option<DateTime<Utc>>,
) -> String {
    if let Some(resets) = resets_at
        && resets > now
    {
        let interval = (resets - now).num_seconds();
        if interval < 60 {
            return format!("Resets in {} seconds", interval);
        } else if interval < 3600 {
            let minutes = interval / 60;
            return format!(
                "Resets in {} minute{}",
                minutes,
                if minutes == 1 { "" } else { "s" }
            );
        } else if interval < 86400 {
            let hours = interval / 3600;
            return format!(
                "Resets in {} hour{}",
                hours,
                if hours == 1 { "" } else { "s" }
            );
        } else {
            let days = interval / 86400;
            return format!("Resets in {} day{}", days, if days == 1 { "" } else { "s" });
        }
    }
    format!("{window_type}: {time_range}")
}

/// Build a `RemainsRow` from the raw interval/weekly fields (upstream
/// `makeServiceUsage`). Returns `None` when the row is a placeholder that
/// should be skipped.
#[allow(clippy::too_many_arguments)]
fn make_remains_row(
    service_type: &str,
    window_type_override: Option<&str>,
    total: Option<i64>,
    remaining: Option<i64>,
    remaining_percent: Option<f64>,
    status: Option<i64>,
    start: Option<i64>,
    end: Option<i64>,
    remains_time: Option<i64>,
    boost_permille: Option<i64>,
    now: DateTime<Utc>,
    is_weekly: bool,
) -> Option<RemainsRow> {
    if is_unavailable_quota_placeholder(
        service_type,
        window_type_override,
        status,
        total,
        remaining,
        remaining_percent,
    ) {
        return None;
    }

    let start_dt = date_from_epoch(start);
    let end_dt = date_from_epoch(end);

    let mut window_type = window_type_from_duration(start_dt, end_dt);
    if let Some(override_wt) = window_type_override {
        window_type = override_wt.to_string();
    }
    let mut time_range = time_range_string(start_dt, end_dt);
    if window_type.eq_ignore_ascii_case("weekly")
        && let Some(weekly_range) = weekly_time_range_string(start_dt, end_dt)
    {
        time_range = weekly_range;
    }

    let is_unlimited =
        is_unlimited_quota_window(service_type, &window_type, status, remaining_percent);
    let resets = if is_unlimited {
        None
    } else {
        resets_at(end_dt, remains_time, now)
    };
    let desc = if is_unlimited {
        "Unlimited".to_string()
    } else {
        reset_description(&window_type, &time_range, now, resets)
    };

    let win_minutes = window_minutes(start_dt, end_dt);

    let (percent, usage, limit) = if is_unlimited {
        (0.0, Some(0), Some(0))
    } else if let Some(rp) = remaining_percent {
        let quota_limit = percent_quota_limit(boost_permille);
        let pct = used_percent_from_remaining(rp);
        let u = (pct * quota_limit as f64 / 100.0).round() as i64;
        (pct, Some(u), Some(quota_limit))
    } else {
        let total = total?;
        if total <= 0 {
            return None;
        }
        let remaining = remaining.unwrap_or(0);
        let used = (total - remaining).max(0);
        let pct = (used as f64 / total as f64 * 100.0).clamp(0.0, 100.0);
        (pct, Some(used), Some(total))
    };

    Some(RemainsRow {
        service_type: service_type.to_string(),
        model_name: String::new(),
        window_type,
        percent,
        window_minutes: win_minutes,
        resets_at: resets,
        reset_description: Some(desc),
        is_unlimited,
        is_weekly,
        usage,
        limit,
    })
}

/// Try the multi-service `data.services[]` shape.
fn parse_multi_service(json: &Value) -> Option<Vec<ServiceRow>> {
    let services = json
        .get("data")
        .and_then(|d| d.get("services"))
        .or_else(|| json.get("services"))
        .and_then(|s| s.as_array())?;
    if services.is_empty() {
        return None;
    }
    let mut rows = Vec::new();
    for item in services {
        let service_type = scalar_string(get_field(item, "service_type", "serviceType"))?;
        let window_type = scalar_string(get_field(item, "window_type", "windowType"))?;
        let time_range = scalar_string(get_field(item, "time_range", "timeRange"))?;
        let usage = value_i64(item.get("usage"))?;
        let limit = value_i64(item.get("limit"))?;
        if limit <= 0 {
            continue;
        }
        let percent = match value_f64(item.get("percent")) {
            Some(p) => p,
            None => (usage as f64 / limit as f64) * 100.0,
        };
        let percent = percent.clamp(0.0, 100.0);
        let resets_at = parse_resets_at_from_time_range(&time_range, &window_type, Utc::now());
        let desc = reset_description(&window_type, &time_range, Utc::now(), resets_at);
        rows.push(ServiceRow {
            service_type,
            window_type,
            time_range,
            percent,
            resets_at,
            reset_description: Some(desc),
        });
    }
    if rows.is_empty() { None } else { Some(rows) }
}

/// Parse a reset timestamp from a multi-service `time_range`.
fn parse_resets_at_from_time_range(
    time_range: &str,
    window_type: &str,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let lower = window_type.trim().to_lowercase();
    let tz = FixedOffset::east_opt(8 * 3600).unwrap();

    if lower == "today" {
        let parts: Vec<&str> = time_range.splitn(2, '-').collect();
        if parts.len() != 2 {
            return None;
        }
        let end_str = parts[1].trim();
        // "yyyy/MM/dd HH:mm" in UTC+8
        let dt = chrono::NaiveDateTime::parse_from_str(end_str, "%Y/%m/%d %H:%M").ok()?;
        return Some(dt.and_local_timezone(tz).single()?.with_timezone(&Utc));
    }

    if lower.contains("hour") || lower.contains('h') {
        let parts: Vec<&str> = time_range.split('-').collect();
        if parts.len() < 2 {
            return None;
        }
        let end_part = parts[1].trim();
        // strip "(...)" suffix
        let end_clean = {
            let re = Regex::new(r#"\(.*\)"#).ok()?;
            re.replace_all(end_part, "").trim().to_string()
        };
        let time = chrono::NaiveTime::parse_from_str(&end_clean, "%H:%M").ok()?;
        let today = now.date_naive();
        let dt = today.and_time(time).and_local_timezone(tz).single()?;
        let mut candidate = dt.with_timezone(&Utc);
        if candidate < now {
            candidate = now + Duration::days(1);
        }
        return Some(candidate);
    }

    None
}

/// Extract the plan name from coding-plan data fields (upstream `parsePlanName(data:)`).
fn parse_plan_name_from_data(data: &Value) -> Option<String> {
    for key in [
        ("current_subscribe_title", "currentSubscribeTitle"),
        ("plan_name", "planName"),
        ("combo_title", "comboTitle"),
        ("current_plan_title", "currentPlanTitle"),
    ] {
        if let Some(val) = get_field(data, key.0, key.1)
            && let Some(s) = scalar_string(Some(val))
            && !s.trim().is_empty()
        {
            return Some(s.trim().to_string());
        }
    }
    // current_combo_card.title
    if let Some(card) = get_field(data, "current_combo_card", "currentComboCard")
        && let Some(title) = card.get("title")
        && let Some(s) = scalar_string(Some(title))
        && !s.trim().is_empty()
    {
        return Some(s.trim().to_string());
    }
    // inferred token plan name
    let model_remains = get_field(data, "model_remains", "modelRemains").and_then(|v| v.as_array());
    if let Some(entries) = model_remains {
        let has_text_generation = entries.iter().any(|e| {
            scalar_string(get_field(e, "model_name", "modelName"))
                .map(|n| is_text_generation_model_name(&n))
                .unwrap_or(false)
        });
        let has_unavailable_video = entries.iter().any(|e| {
            let name = scalar_string(get_field(e, "model_name", "modelName"))
                .map(|n| n.trim().to_lowercase())
                .unwrap_or_default();
            name == "video"
                && is_unavailable_quota_placeholder(
                    "Text to Video",
                    None,
                    value_i64(get_field(
                        e,
                        "current_interval_status",
                        "currentIntervalStatus",
                    )),
                    value_i64(get_field(
                        e,
                        "current_interval_total_count",
                        "currentIntervalTotalCount",
                    )),
                    value_i64(get_field(
                        e,
                        "current_interval_usage_count",
                        "currentIntervalUsageCount",
                    )),
                    value_f64(get_field(
                        e,
                        "current_interval_remaining_percent",
                        "currentIntervalRemainingPercent",
                    )),
                )
        });
        if has_text_generation && has_unavailable_video {
            return Some("Plus".to_string());
        }
    }
    None
}

/// The single-service `model_remains[]` parser (upstream
/// `parseCodingPlanRemains(payload:now:)`).
fn parse_remains(
    json: &Value,
    now: DateTime<Utc>,
) -> Result<MiniMaxCodingPlanSnapshot, ProviderError> {
    // base_resp from data first, then root
    let base_resp = json
        .get("data")
        .and_then(|d| d.get("base_resp"))
        .or_else(|| json.get("base_resp"))
        .or_else(|| json.get("baseResp"))
        .or_else(|| json.get("data").and_then(|d| d.get("baseResp")));

    if let Some(base) = base_resp
        && let Some(status_code) = value_i64(base.get("status_code"))
        && status_code != 0
    {
        let status_msg = scalar_string(base.get("status_msg"))
            .or_else(|| scalar_string(base.get("statusMessage")))
            .unwrap_or_else(|| format!("MiniMax coding plan status {status_code}"));
        let lower = status_msg.to_lowercase();
        if status_code == 1004
            || lower.contains("cookie")
            || lower.contains("log in")
            || lower.contains("login")
        {
            return Err(ProviderError::AuthRequired);
        }
        return Err(ProviderError::Other(status_msg));
    }

    // model_remains from data first, then root
    let model_remains = get_field(json, "model_remains", "modelRemains")
        .or_else(|| {
            json.get("data")
                .and_then(|d| get_field(d, "model_remains", "modelRemains"))
        })
        .and_then(|v| v.as_array());

    let entries = match model_remains {
        Some(arr) if !arr.is_empty() => arr,
        _ => {
            return Err(ProviderError::Parse("Missing coding plan data.".into()));
        }
    };

    // Locate data for plan-name extraction
    let data_obj = json.get("data").unwrap_or(json);
    let plan_name = parse_plan_name_from_data(data_obj);

    let mut rows: Vec<RemainsRow> = Vec::new();
    for entry in entries {
        let model_name = match scalar_string(get_field(entry, "model_name", "modelName")) {
            Some(n) => n,
            None => continue,
        };
        let service_type = map_model_name_to_service_type(&model_name);

        // Interval row
        if let Some(mut row) = make_remains_row(
            &service_type,
            None,
            value_i64(get_field(
                entry,
                "current_interval_total_count",
                "currentIntervalTotalCount",
            )),
            value_i64(get_field(
                entry,
                "current_interval_usage_count",
                "currentIntervalUsageCount",
            )),
            value_f64(get_field(
                entry,
                "current_interval_remaining_percent",
                "currentIntervalRemainingPercent",
            )),
            value_i64(get_field(
                entry,
                "current_interval_status",
                "currentIntervalStatus",
            )),
            value_i64(get_field(entry, "start_time", "startTime")),
            value_i64(get_field(entry, "end_time", "endTime")),
            value_i64(get_field(entry, "remains_time", "remainsTime")),
            value_i64(get_field(
                entry,
                "interval_boost_permill",
                "interval_boost_permille",
            ))
            .or_else(|| {
                value_i64(get_field(
                    entry,
                    "interval_boost_permille",
                    "interval_boost_permill",
                ))
            }),
            now,
            false,
        ) {
            row.model_name = model_name.clone();
            rows.push(row);
        }

        // Weekly row (only for text-generation models)
        if should_render_weekly_window(&model_name)
            && let Some(mut row) = make_remains_row(
                &service_type,
                Some("Weekly"),
                value_i64(get_field(
                    entry,
                    "current_weekly_total_count",
                    "currentWeeklyTotalCount",
                )),
                value_i64(get_field(
                    entry,
                    "current_weekly_usage_count",
                    "currentWeeklyUsageCount",
                )),
                value_f64(get_field(
                    entry,
                    "current_weekly_remaining_percent",
                    "currentWeeklyRemainingPercent",
                )),
                value_i64(get_field(
                    entry,
                    "current_weekly_status",
                    "currentWeeklyStatus",
                )),
                value_i64(get_field(entry, "weekly_start_time", "weeklyStartTime")),
                value_i64(get_field(entry, "weekly_end_time", "weeklyEndTime")),
                value_i64(get_field(entry, "weekly_remains_time", "weeklyRemainsTime")),
                value_i64(get_field(
                    entry,
                    "weekly_boost_permill",
                    "weekly_boost_permille",
                ))
                .or_else(|| {
                    value_i64(get_field(
                        entry,
                        "weekly_boost_permille",
                        "weekly_boost_permill",
                    ))
                }),
                now,
                true,
            )
        {
            row.model_name = model_name;
            rows.push(row);
        }
    }

    Ok(MiniMaxCodingPlanSnapshot::Remains { plan_name, rows })
}

/// The one parser used for page JSON, `__NEXT_DATA__` payloads, and remains API.
pub(super) fn parse_coding_plan_value(
    json: &Value,
    now: DateTime<Utc>,
) -> Result<MiniMaxCodingPlanSnapshot, ProviderError> {
    // Try multi-service shape first
    if let Some(rows) = parse_multi_service(json) {
        return Ok(MiniMaxCodingPlanSnapshot::Services(rows));
    }
    // Fall through to single-service (model_remains)
    parse_remains(json, now)
}

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
    let lower = unit.to_lowercase();
    if lower.starts_with('d') {
        return (value * 24.0 * 60.0).round() as u32;
    }
    if lower.starts_with('h') {
        return (value * 60.0).round() as u32;
    }
    if lower.starts_with('m') {
        return value.round() as u32;
    }
    if lower.starts_with('s') {
        return ((value / 60.0).round() as u32).max(1);
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
        return Some(now + Duration::seconds(seconds as i64));
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

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0).unwrap()
    }

    #[test]
    fn parses_remains_json_snake_case() {
        let json = serde_json::json!({
            "data": {
                "current_subscribe_title": "MiniMax Pro",
                "model_remains": [{
                    "model_name": "General",
                    "current_interval_total_count": 100,
                    "current_interval_usage_count": 73,
                    "start_time": 1785792000,
                    "end_time": 1785878400,
                    "current_interval_status": 0,
                    "current_weekly_total_count": 1000,
                    "current_weekly_usage_count": 850,
                    "current_weekly_remaining_percent": 85,
                    "weekly_start_time": 1785792000,
                    "weekly_end_time": 1786396800,
                    "current_weekly_status": 0
                }]
            }
        });
        let snapshot = parse_coding_plan_value(&json, now()).unwrap();
        let MiniMaxCodingPlanSnapshot::Remains { plan_name, rows } = snapshot else {
            panic!("expected Remains");
        };
        assert_eq!(plan_name.as_deref(), Some("MiniMax Pro"));
        // remaining percent 73 → used 27
        assert!((rows[0].percent - 27.0).abs() < 0.01);
        // window_minutes = (end-start)/60 = 86400/60 = 1440
        assert_eq!(rows[0].window_minutes, Some(1440));
        assert!(rows[0].resets_at.is_some());
        // General is text-gen → weekly row generated
        assert_eq!(rows.len(), 2);
        assert!(rows[1].is_weekly);
    }

    #[test]
    fn parses_counts_only_entry() {
        let json = serde_json::json!({
            "data": {
                "model_remains": [{
                    "model_name": "General",
                    "current_interval_total_count": 100,
                    "current_interval_usage_count": 40,
                    "start_time": 1722691200,
                    "end_time": 1722777600,
                    "current_interval_status": 0
                }]
            }
        });
        let snapshot = parse_coding_plan_value(&json, now()).unwrap();
        let MiniMaxCodingPlanSnapshot::Remains { rows, .. } = snapshot else {
            panic!("expected Remains");
        };
        // total=100, remaining=40 → used 60/100 = 60%
        assert!((rows[0].percent - 60.0).abs() < 0.01);
    }

    #[test]
    fn parses_boost_into_row_limit() {
        let json = serde_json::json!({
            "data": {
                "model_remains": [{
                    "model_name": "General",
                    "current_interval_total_count": 0,
                    "current_interval_usage_count": 0,
                    "current_interval_remaining_percent": 50,
                    "interval_boost_permill": 1500,
                    "start_time": 1722691200,
                    "end_time": 1722777600,
                    "current_interval_status": 0
                }]
            }
        });
        let snapshot = parse_coding_plan_value(&json, now()).unwrap();
        let MiniMaxCodingPlanSnapshot::Remains { rows, .. } = snapshot else {
            panic!("expected Remains");
        };
        // boost 1500 → percentQuotaLimit = max(1, round(1500/10)) = 150
        assert_eq!(rows[0].limit, Some(150));
        // remainingPercent 50 → used 50%
        assert!((rows[0].percent - 50.0).abs() < 0.01);
    }

    #[test]
    fn placeholder_lane_skipped_for_video() {
        let json = serde_json::json!({
            "data": {
                "model_remains": [{
                    "model_name": "video",
                    "current_interval_total_count": 0,
                    "current_interval_usage_count": 0,
                    "current_interval_remaining_percent": 100,
                    "current_interval_status": 3
                }]
            }
        });
        let snapshot = parse_coding_plan_value(&json, now()).unwrap();
        let MiniMaxCodingPlanSnapshot::Remains { rows, .. } = snapshot else {
            panic!("expected Remains");
        };
        // video is not text-gen → no weekly; interval is placeholder → skipped
        assert!(rows.is_empty());
    }

    #[test]
    fn placeholder_general_weekly_kept_as_unlimited() {
        let json = serde_json::json!({
            "data": {
                "model_remains": [{
                    "model_name": "General",
                    "current_interval_total_count": 0,
                    "current_interval_usage_count": 0,
                    "current_interval_remaining_percent": 100,
                    "current_interval_status": 3,
                    "current_weekly_total_count": 0,
                    "current_weekly_usage_count": 0,
                    "current_weekly_remaining_percent": 100,
                    "current_weekly_status": 3
                }]
            }
        });
        let snapshot = parse_coding_plan_value(&json, now()).unwrap();
        let MiniMaxCodingPlanSnapshot::Remains { rows, .. } = snapshot else {
            panic!("expected Remains");
        };
        // interval is placeholder → skipped, but weekly is unlimited → kept
        let weekly = rows.iter().find(|r| r.is_weekly).unwrap();
        assert!(weekly.is_unlimited);
        assert_eq!(weekly.reset_description.as_deref(), Some("Unlimited"));
        assert!((weekly.percent - 0.0).abs() < 0.01);
    }

    #[test]
    fn base_resp_1004_is_auth_required() {
        let json = serde_json::json!({
            "data": {
                "base_resp": { "status_code": 1004 },
                "model_remains": []
            }
        });
        let err = parse_coding_plan_value(&json, now()).unwrap_err();
        assert!(matches!(err, ProviderError::AuthRequired));
    }

    #[test]
    fn base_resp_status_msg_login_is_auth_required() {
        let json = serde_json::json!({
            "data": {
                "base_resp": { "status_code": 2000, "status_msg": "please log in" },
                "model_remains": []
            }
        });
        let err = parse_coding_plan_value(&json, now()).unwrap_err();
        assert!(matches!(err, ProviderError::AuthRequired));
    }

    #[test]
    fn base_resp_other_status_is_other_error() {
        let json = serde_json::json!({
            "data": {
                "base_resp": { "status_code": 2000, "status_msg": "quota sync failed" },
                "model_remains": []
            }
        });
        let err = parse_coding_plan_value(&json, now()).unwrap_err();
        assert!(matches!(err, ProviderError::Other(_)));
    }

    #[test]
    fn base_resp_without_status_code_parses_fine() {
        let json = serde_json::json!({
            "data": {
                "base_resp": {},
                "model_remains": [{
                    "model_name": "General",
                    "current_interval_total_count": 100,
                    "current_interval_usage_count": 30,
                    "current_interval_remaining_percent": 70,
                    "start_time": 1785792000,
                    "end_time": 1785878400,
                    "current_interval_status": 0
                }]
            }
        });
        let snapshot = parse_coding_plan_value(&json, now()).unwrap();
        let MiniMaxCodingPlanSnapshot::Remains { rows, .. } = snapshot else {
            panic!("expected Remains");
        };
        // remainingPercent 70 → used 30%; no error from absent status_code
        assert!((rows[0].percent - 30.0).abs() < 0.01);
    }

    #[test]
    fn empty_model_remains_is_parse_error() {
        let json = serde_json::json!({
            "data": {
                "model_remains": []
            }
        });
        let err = parse_coding_plan_value(&json, now()).unwrap_err();
        assert!(matches!(err, ProviderError::Parse(_)));
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
    fn parses_multi_service_json() {
        let json = serde_json::json!({
            "data": {
                "services": [{
                    "service_type": "Text Generation Pro",
                    "window_type": "Today",
                    "time_range": "2026/08/03 00:00 - 2026/08/04 00:00",
                    "usage": 250,
                    "limit": 1000
                }]
            }
        });
        let snapshot = parse_coding_plan_value(&json, now()).unwrap();
        match snapshot {
            MiniMaxCodingPlanSnapshot::Services(rows) => {
                assert_eq!(rows.len(), 1);
                assert!((rows[0].percent - 25.0).abs() < 0.01);
                assert!(rows[0].service_type.contains("Pro"));
            }
            _ => panic!("expected Services"),
        }
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
                usage: Some(27),
                limit: Some(100),
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
                usage: Some(15),
                limit: Some(100),
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
