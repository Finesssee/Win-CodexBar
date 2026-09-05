use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;

use crate::core::{NamedRateWindow, ProviderError, RateWindow, UsageSnapshot};

const WINDOW_ID_PREFIX: &str = "antigravity-quota-summary-";
const SESSION_MINUTES: u32 = 300;
const WEEKLY_MINUTES: u32 = 7 * 24 * 60;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaSummaryEnvelope {
    response: Option<QuotaSummaryPayload>,
    summary: Option<QuotaSummaryPayload>,
    description: Option<String>,
    groups: Option<Vec<QuotaSummaryGroup>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaSummaryPayload {
    #[allow(dead_code, reason = "mirrors the local quota-summary response")]
    description: Option<String>,
    #[serde(default)]
    groups: Vec<QuotaSummaryGroup>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaSummaryGroup {
    display_name: Option<String>,
    #[allow(dead_code, reason = "mirrors the local quota-summary response")]
    description: Option<String>,
    #[serde(default)]
    buckets: Vec<QuotaSummaryBucket>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaSummaryBucket {
    bucket_id: Option<String>,
    display_name: Option<String>,
    description: Option<String>,
    disabled: Option<bool>,
    remaining_fraction: Option<f64>,
    remaining: Option<QuotaSummaryRemaining>,
    reset_time: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaSummaryRemaining {
    remaining_fraction: Option<f64>,
    #[serde(rename = "case")]
    oneof_case: Option<String>,
    value: Option<f64>,
}

impl QuotaSummaryBucket {
    fn resolved_remaining_fraction(&self) -> Option<f64> {
        self.remaining_fraction.or_else(|| {
            let remaining = self.remaining.as_ref()?;
            remaining.remaining_fraction.or_else(|| {
                (remaining.oneof_case.as_deref() == Some("remainingFraction"))
                    .then_some(remaining.value)
                    .flatten()
            })
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum BucketKind {
    Session,
    Weekly,
    Other,
}

pub(super) fn parse_usage_snapshot(data: &[u8]) -> Result<UsageSnapshot, ProviderError> {
    let envelope: QuotaSummaryEnvelope = serde_json::from_slice(data)
        .map_err(|error| ProviderError::Parse(format!("Antigravity quota summary: {error}")))?;
    let payload = envelope
        .response
        .or(envelope.summary)
        .or_else(|| {
            envelope.groups.map(|groups| QuotaSummaryPayload {
                description: envelope.description,
                groups,
            })
        })
        .ok_or_else(|| ProviderError::Parse("Antigravity quota summary missing payload".into()))?;

    let windows = quota_windows(payload.groups);
    if !windows.iter().any(|window| window.usage_known) {
        return Err(ProviderError::Parse(
            "Antigravity quota summary has no usable quota buckets".into(),
        ));
    }

    let primary =
        most_constrained(&windows, SESSION_MINUTES).unwrap_or_else(RateWindow::no_active_session);
    let secondary = most_constrained(&windows, WEEKLY_MINUTES);
    let mut snapshot = UsageSnapshot::new(primary).with_primary_label("Session");
    if let Some(weekly) = secondary {
        snapshot = snapshot.with_secondary(weekly);
    }
    snapshot.extra_rate_windows = windows;
    Ok(snapshot)
}

fn quota_windows(groups: Vec<QuotaSummaryGroup>) -> Vec<NamedRateWindow> {
    let mut indexed_groups = groups.into_iter().enumerate().collect::<Vec<_>>();
    indexed_groups.sort_by_key(|(index, group)| (group_rank(group), *index));

    let mut windows = Vec::new();
    for (_, group) in indexed_groups {
        let group_title = group_title(&group);
        let mut buckets = group.buckets.into_iter().enumerate().collect::<Vec<_>>();
        buckets.sort_by_key(|(index, bucket)| (bucket_kind(bucket), *index));
        for (_, bucket) in buckets {
            let Some(bucket_id) = non_empty(bucket.bucket_id.as_deref()) else {
                continue;
            };
            let kind = bucket_kind(&bucket);
            let title = format!("{} {}", group_title, bucket_title(&bucket, kind));
            let remaining = bucket.resolved_remaining_fraction();
            let usage_known = !bucket.disabled.unwrap_or(false) && remaining.is_some();
            let used_percent = remaining
                .map(|fraction| 100.0 - (fraction * 100.0).clamp(0.0, 100.0))
                .unwrap_or(0.0);
            let window_minutes = match kind {
                BucketKind::Session => Some(SESSION_MINUTES),
                BucketKind::Weekly => Some(WEEKLY_MINUTES),
                BucketKind::Other => None,
            };
            let reset = bucket.reset_time.as_deref().and_then(parse_reset_time);
            let window = RateWindow::with_details(
                used_percent,
                window_minutes,
                reset,
                bucket.description.clone(),
            );
            windows.push(
                NamedRateWindow::new(format!("{WINDOW_ID_PREFIX}{bucket_id}"), title, window)
                    .with_usage_known(usage_known),
            );
        }
    }
    windows
}

fn most_constrained(windows: &[NamedRateWindow], minutes: u32) -> Option<RateWindow> {
    windows
        .iter()
        .filter(|row| row.usage_known && row.window.window_minutes == Some(minutes))
        .max_by(|left, right| {
            left.window
                .used_percent
                .partial_cmp(&right.window.used_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.id.cmp(&left.id))
        })
        .map(|row| row.window.clone())
}

fn group_rank(group: &QuotaSummaryGroup) -> u8 {
    let title = group
        .display_name
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if title.contains("gemini") {
        0
    } else if title.contains("claude") || title.contains("gpt") {
        1
    } else {
        2
    }
}

fn group_title(group: &QuotaSummaryGroup) -> String {
    let title = non_empty(group.display_name.as_deref()).unwrap_or("Quota");
    let lower = title.to_ascii_lowercase();
    if lower.contains("gemini") {
        "Gemini".into()
    } else if lower.contains("claude") || lower.contains("gpt") {
        "Claude/GPT".into()
    } else {
        title.to_string()
    }
}

fn bucket_kind(bucket: &QuotaSummaryBucket) -> BucketKind {
    let mut candidates = Vec::new();
    for raw in [bucket.bucket_id.as_deref(), bucket.display_name.as_deref()]
        .into_iter()
        .flatten()
    {
        let normalized = raw.trim().to_ascii_lowercase().replace('_', "-");
        if normalized.is_empty() {
            continue;
        }
        candidates.push(normalized.clone());
        if let Some(stripped) = normalized.strip_suffix(" limit") {
            candidates.push(stripped.to_string());
        }
    }
    const SESSION_ALIASES: [&str; 5] = ["session", "5h", "5-hour", "five hour", "five-hour"];
    if candidates.iter().any(|candidate| {
        SESSION_ALIASES
            .iter()
            .any(|alias| candidate == alias || candidate.ends_with(&format!("-{alias}")))
    }) {
        BucketKind::Session
    } else if candidates
        .iter()
        .any(|candidate| candidate == "weekly" || candidate.ends_with("-weekly"))
    {
        BucketKind::Weekly
    } else {
        BucketKind::Other
    }
}

fn bucket_title(bucket: &QuotaSummaryBucket, kind: BucketKind) -> String {
    match kind {
        BucketKind::Session => "5-hour".into(),
        BucketKind::Weekly => "weekly".into(),
        BucketKind::Other => non_empty(bucket.display_name.as_deref())
            .or_else(|| non_empty(bucket.bucket_id.as_deref()))
            .unwrap_or("quota")
            .to_string(),
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn parse_reset_time(raw: &str) -> Option<DateTime<Utc>> {
    let raw = raw.trim();
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|value| value.with_timezone(&Utc))
        .or_else(|| {
            let seconds = raw.parse::<f64>().ok()?;
            if !seconds.is_finite() {
                return None;
            }
            let whole = seconds.trunc();
            if whole < i64::MIN as f64 || whole > i64::MAX as f64 {
                return None;
            }
            #[allow(
                clippy::cast_possible_truncation,
                reason = "finite epoch seconds are range-checked before this conversion"
            )]
            let whole_seconds = whole as i64;
            Utc.timestamp_opt(whole_seconds, 0).single()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_most_constrained_session_and_weekly_independently() {
        let data = br#"{
          "response": {"groups": [
            {"displayName":"Gemini Models","buckets":[
              {"bucketId":"gemini-5h","displayName":"5h","remainingFraction":0.9},
              {"bucketId":"gemini-weekly","displayName":"Weekly","remainingFraction":0.1}
            ]},
            {"displayName":"Claude and GPT","buckets":[
              {"bucketId":"3p-5h","displayName":"5-hour limit","remainingFraction":0.2},
              {"bucketId":"3p-weekly","displayName":"Weekly","remainingFraction":0.8}
            ]}
          ]}
        }"#;
        let snapshot = parse_usage_snapshot(data).unwrap();
        assert!((snapshot.primary.used_percent - 80.0).abs() < 0.001);
        assert_eq!(snapshot.primary.window_minutes, Some(300));
        let weekly = snapshot.secondary.unwrap();
        assert!((weekly.used_percent - 90.0).abs() < 0.001);
        assert_eq!(weekly.window_minutes, Some(10_080));
        assert_eq!(snapshot.extra_rate_windows.len(), 4);
    }

    #[test]
    fn nested_oneof_remaining_and_disabled_buckets_preserve_unknown_state() {
        let data = br#"{
          "groups": [{"displayName":"Gemini","buckets":[
            {"bucketId":"gemini_session","displayName":"Session","remaining":{"case":"remainingFraction","value":0.25}},
            {"bucketId":"gemini-weekly","displayName":"Weekly","disabled":true,"remainingFraction":0.01},
            {"bucketId":"future","displayName":"Daily"}
          ]}]
        }"#;
        let snapshot = parse_usage_snapshot(data).unwrap();
        assert!((snapshot.primary.used_percent - 75.0).abs() < 0.001);
        assert!(snapshot.secondary.is_none());
        assert_eq!(snapshot.extra_rate_windows.len(), 3);
        assert!(snapshot.extra_rate_windows[0].usage_known);
        assert!(!snapshot.extra_rate_windows[1].usage_known);
        assert!(!snapshot.extra_rate_windows[2].usage_known);
    }

    #[test]
    fn rejects_empty_or_unusable_quota_summary() {
        assert!(parse_usage_snapshot(br#"{}"#).is_err());
        assert!(parse_usage_snapshot(
            br#"{"groups":[{"displayName":"Gemini","buckets":[{"bucketId":"weekly","displayName":"Weekly"}]}]}"#
        )
        .is_err());
    }
}
