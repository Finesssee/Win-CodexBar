use std::str::FromStr;
use std::sync::OnceLock;

use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use regex_lite::Regex;

use crate::core::{NamedRateWindow, RateWindow};

fn regex(cell: &'static OnceLock<Regex>, pattern: &'static str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid Claude CLI regex"))
}

pub(super) fn parse_percent_line(line: &str) -> Option<f64> {
    static PERCENT: OnceLock<Regex> = OnceLock::new();
    let captures = regex(
        &PERCENT,
        r"(?i)(\d{1,3}(?:\.\d+)?)\s*%\s*(used|spent|consumed|left|remaining|available)",
    )
    .captures(line)?;
    let value: f64 = captures.get(1)?.as_str().parse().ok()?;
    match captures.get(2)?.as_str().to_ascii_lowercase().as_str() {
        "left" | "remaining" | "available" => Some((100.0 - value).max(0.0)),
        _ => Some(value.min(100.0)),
    }
}

pub(super) fn normalized_for_label_search(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

pub(super) fn starts_next_usage_section(line: &str, current_label: &str) -> bool {
    let normalized = normalized_for_label_search(line);
    normalized.starts_with("current") && !normalized.contains(current_label)
}

pub(super) fn extract_cli_scoped_weekly_limits(
    text: &str,
    now: DateTime<Utc>,
) -> Vec<NamedRateWindow> {
    static LABEL: OnceLock<Regex> = OnceLock::new();
    static RESET: OnceLock<Regex> = OnceLock::new();
    let label_re = regex(&LABEL, r"(?i)current\s*week\s*\(([^)]+)\)");
    let reset_re = regex(&RESET, r"(?i)resets.*$");
    let lines: Vec<&str> = text.lines().collect();
    let mut limits = Vec::new();

    for (idx, line) in lines.iter().enumerate() {
        let Some(label_match) = label_re.captures(line).and_then(|captures| captures.get(1)) else {
            continue;
        };
        let title = normalize_scoped_weekly_title(label_match.as_str());
        if title.is_empty() || normalized_for_label_search(&title) == "allmodels" {
            continue;
        }
        let id = format!(
            "claude-weekly-scoped-{}",
            slug_claude_model(title.trim_end_matches(" only").trim())
        );
        if id == "claude-weekly-scoped-"
            || limits.iter().any(|limit: &NamedRateWindow| limit.id == id)
        {
            continue;
        }

        let mut used_percent = None;
        let mut reset_description = None;
        let current_label = normalized_for_label_search(line);
        for (offset, section_line) in lines.iter().skip(idx).take(14).enumerate() {
            if offset > 0 && starts_next_usage_section(section_line, &current_label) {
                break;
            }
            used_percent = used_percent.or_else(|| parse_percent_line(section_line));
            reset_description = reset_description.or_else(|| {
                reset_re
                    .find(section_line)
                    .map(|value| value.as_str().trim().to_string())
            });
        }
        let Some(used_percent) = used_percent else {
            continue;
        };
        let resets_at = reset_description
            .as_deref()
            .and_then(|reset| parse_claude_reset_date(reset, now, Some(10080)));
        limits.push(NamedRateWindow::new(
            id,
            title,
            RateWindow::with_details(used_percent, Some(10080), resets_at, reset_description),
        ));
    }

    limits
}

pub(super) fn slug_claude_model(label: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for character in label.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            previous_dash = false;
        } else if !slug.is_empty() && !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
    }
    slug.trim_matches('-').to_string()
}

fn normalize_scoped_weekly_title(label: &str) -> String {
    static ONLY_SUFFIX: OnceLock<Regex> = OnceLock::new();
    let label = label.trim();
    if let Some(suffix) = regex(&ONLY_SUFFIX, r"(?i)only$").find(label) {
        let model = label[..suffix.start()].trim_end();
        if !model.is_empty() {
            return format!("{model} only");
        }
    }
    label.to_string()
}

pub(super) fn parse_claude_reset_date(
    text: &str,
    now: DateTime<Utc>,
    expected_window_minutes: Option<u32>,
) -> Option<DateTime<Utc>> {
    let system_timezone =
        Tz::from_str(&crate::core::local_timezone_name()).unwrap_or(chrono_tz::UTC);
    parse_claude_reset_date_in_system_zone(text, now, expected_window_minutes, system_timezone)
}

pub(super) fn parse_claude_reset_date_in_system_zone(
    text: &str,
    now: DateTime<Utc>,
    expected_window_minutes: Option<u32>,
    system_timezone: Tz,
) -> Option<DateTime<Utc>> {
    let (raw, timezone) = normalize_claude_reset_text(text, system_timezone)?;
    let components = parse_claude_reset_components(&raw)?;
    let now_local = now.with_timezone(&timezone);
    let candidates = match (components.year, components.month, components.day) {
        (Some(year), Some(month), Some(day)) => local_reset_occurrences(
            timezone,
            year,
            month,
            day,
            components.hour,
            components.minute,
        ),
        (None, Some(month), Some(day)) => (now_local.year() - 8..=now_local.year() + 8)
            .flat_map(|year| {
                local_reset_occurrences(
                    timezone,
                    year,
                    month,
                    day,
                    components.hour,
                    components.minute,
                )
            })
            .collect(),
        (None, None, None) => (-1..=1)
            .flat_map(|offset| {
                let date = now_local.date_naive() + Duration::days(offset);
                local_reset_occurrences(
                    timezone,
                    date.year(),
                    date.month(),
                    date.day(),
                    components.hour,
                    components.minute,
                )
            })
            .collect(),
        _ => return None,
    };

    resolve_claude_reset_occurrence(candidates, now, expected_window_minutes)
}

fn normalize_claude_reset_text(text: &str, system_timezone: Tz) -> Option<(String, Tz)> {
    static MONTH_BOUNDARY: OnceLock<Regex> = OnceLock::new();
    static COMPACT_AT: OnceLock<Regex> = OnceLock::new();
    let mut raw = text
        .trim()
        .strip_prefix("Resets")
        .or_else(|| text.trim().strip_prefix("resets"))
        .unwrap_or(text)
        .trim()
        .to_string();
    let timezone = raw
        .rfind('(')
        .filter(|_| raw.ends_with(')'))
        .and_then(|start| {
            let timezone = Tz::from_str(raw[start + 1..raw.len() - 1].trim()).ok();
            raw.truncate(start);
            timezone
        })
        .unwrap_or(system_timezone);
    raw = raw.replace(" at ", " ");
    raw = regex(&MONTH_BOUNDARY, r"(?i)([a-z]{3})(\d)")
        .replace(&raw, "$1 $2")
        .into_owned();
    raw = regex(&COMPACT_AT, r"(?i)(\d)at(\d)")
        .replace(&raw, "$1 $2")
        .into_owned();
    (!raw.trim().is_empty()).then(|| (raw.trim().to_string(), timezone))
}

struct ClaudeResetComponents {
    year: Option<i32>,
    month: Option<u32>,
    day: Option<u32>,
    hour: u32,
    minute: u32,
}

fn parse_claude_reset_components(raw: &str) -> Option<ClaudeResetComponents> {
    static DATE_TIME: OnceLock<Regex> = OnceLock::new();
    static TIME: OnceLock<Regex> = OnceLock::new();
    let date_time = regex(
        &DATE_TIME,
        r"(?i)^([a-z]{3})\s+(\d{1,2})(?:,\s*|\s+)(?:(\d{4})(?:,\s*|\s+))?(\d{1,2})(?::(\d{2}))?\s*(am|pm)?$",
    );
    if let Some(captures) = date_time.captures(raw) {
        let month = claude_month(captures.get(1)?.as_str())?;
        let day = captures.get(2)?.as_str().parse().ok()?;
        let year = captures
            .get(3)
            .and_then(|value| value.as_str().parse().ok());
        let (hour, minute) = parse_claude_hour(
            captures.get(4)?.as_str(),
            captures.get(5).map(|value| value.as_str()),
            captures.get(6).map(|value| value.as_str()),
        )?;
        return Some(ClaudeResetComponents {
            year,
            month: Some(month),
            day: Some(day),
            hour,
            minute,
        });
    }

    let captures = regex(&TIME, r"(?i)^(\d{1,2})(?::(\d{2}))?\s*(am|pm)?$").captures(raw)?;
    let (hour, minute) = parse_claude_hour(
        captures.get(1)?.as_str(),
        captures.get(2).map(|value| value.as_str()),
        captures.get(3).map(|value| value.as_str()),
    )?;
    Some(ClaudeResetComponents {
        year: None,
        month: None,
        day: None,
        hour,
        minute,
    })
}

fn parse_claude_hour(
    hour: &str,
    minute: Option<&str>,
    meridiem: Option<&str>,
) -> Option<(u32, u32)> {
    let mut hour = hour.parse::<u32>().ok()?;
    let minute = minute.unwrap_or("0").parse::<u32>().ok()?;
    if minute > 59 {
        return None;
    }
    match meridiem.map(str::to_ascii_lowercase).as_deref() {
        Some("am") if (1..=12).contains(&hour) => {
            if hour == 12 {
                hour = 0;
            }
        }
        Some("pm") if (1..=12).contains(&hour) => {
            if hour != 12 {
                hour += 12;
            }
        }
        Some(_) => return None,
        None if hour > 23 => return None,
        None => {}
    }
    Some((hour, minute))
}

fn claude_month(month: &str) -> Option<u32> {
    match month.to_ascii_lowercase().as_str() {
        "jan" => Some(1),
        "feb" => Some(2),
        "mar" => Some(3),
        "apr" => Some(4),
        "may" => Some(5),
        "jun" => Some(6),
        "jul" => Some(7),
        "aug" => Some(8),
        "sep" => Some(9),
        "oct" => Some(10),
        "nov" => Some(11),
        "dec" => Some(12),
        _ => None,
    }
}

fn local_reset_occurrences(
    timezone: Tz,
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
) -> Vec<DateTime<Utc>> {
    let Some(naive) = NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|date| date.and_hms_opt(hour, minute, 0))
    else {
        return Vec::new();
    };
    match timezone.from_local_datetime(&naive) {
        LocalResult::Single(value) => vec![value.with_timezone(&Utc)],
        LocalResult::Ambiguous(first, second) => {
            vec![first.with_timezone(&Utc), second.with_timezone(&Utc)]
        }
        LocalResult::None => Vec::new(),
    }
}

fn resolve_claude_reset_occurrence(
    mut candidates: Vec<DateTime<Utc>>,
    now: DateTime<Utc>,
    expected_window_minutes: Option<u32>,
) -> Option<DateTime<Utc>> {
    candidates.sort_unstable();
    candidates.dedup();
    let future = candidates
        .iter()
        .copied()
        .find(|candidate| *candidate >= now);
    let fallback = candidates.last().copied();
    let future = future.or(fallback)?;
    let Some(expected_window) =
        expected_window_minutes.map(|minutes| Duration::minutes(minutes.into()))
    else {
        return Some(future);
    };
    let past = candidates
        .iter()
        .copied()
        .rev()
        .find(|candidate| *candidate < now);
    let past_is_plausible = past.is_some_and(|candidate| now - candidate <= expected_window);
    let future_is_plausible = future - now <= expected_window;
    if past_is_plausible && !future_is_plausible {
        past
    } else {
        Some(future)
    }
}
