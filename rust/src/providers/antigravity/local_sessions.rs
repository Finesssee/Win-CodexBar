use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Local, TimeZone, Utc};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LocalSessionSummary {
    pub total_tokens: u64,
    pub session_count: usize,
}

pub fn summarize(days: u32) -> LocalSessionSummary {
    summarize_paths(&tokscale_paths(None), Utc::now(), days)
}

fn tokscale_paths(home: Option<&Path>) -> Vec<PathBuf> {
    let base = if let Some(home) = home {
        home.join(".config")
            .join("tokscale")
            .join("antigravity-cache")
            .join("sessions")
    } else if let Ok(root) = std::env::var("TOKSCALE_CONFIG_DIR") {
        PathBuf::from(root)
            .join("antigravity-cache")
            .join("sessions")
    } else {
        let Some(home) = dirs::home_dir() else {
            return Vec::new();
        };
        home.join(".config")
            .join("tokscale")
            .join("antigravity-cache")
            .join("sessions")
    };
    let Ok(entries) = fs::read_dir(base) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("jsonl"))
        .collect();
    paths.sort();
    paths
}

fn summarize_paths(paths: &[PathBuf], now: DateTime<Utc>, days: u32) -> LocalSessionSummary {
    let first_day = now.with_timezone(&Local).date_naive()
        - Duration::days(i64::from(days.clamp(1, 365).saturating_sub(1)));
    let mut total_tokens = 0_u64;
    let mut sessions_with_usage = HashSet::new();
    let mut seen_response_ids = HashSet::new();

    for path in paths {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let mut path_had_usage = false;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let kind = value.get("type").and_then(Value::as_str);
            if kind != Some("usage") && value.get("input").is_none() {
                continue;
            }
            if let Some(response_id) = value
                .get("responseId")
                .or_else(|| value.get("response_id"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                && !seen_response_ids.insert(response_id.to_string())
            {
                continue;
            }

            let timestamp_ms = value
                .get("timestamp")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let Some(at) = Utc.timestamp_millis_opt(timestamp_ms).single() else {
                continue;
            };
            if at > now || at.with_timezone(&Local).date_naive() < first_day {
                continue;
            }

            let input = token_field(&value, &["input"]);
            let output = token_field(&value, &["output"]);
            let cache_read = token_field(&value, &["cacheRead", "cache_read"]);
            let cache_write = token_field(&value, &["cacheWrite", "cache_write"]);
            let total = input
                .saturating_add(output)
                .saturating_add(cache_read)
                .saturating_add(cache_write);
            if total == 0 {
                continue;
            }
            total_tokens = total_tokens.saturating_add(total);
            path_had_usage = true;
        }
        if path_had_usage {
            sessions_with_usage.insert(path.clone());
        }
    }

    LocalSessionSummary {
        total_tokens,
        session_count: sessions_with_usage.len(),
    }
}

fn token_field(value: &Value, keys: &[&str]) -> u64 {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_u64))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_tokscale_jsonl_and_deduplicates_response_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-a.jsonl");
        fs::write(&path, concat!(
            "{\"type\":\"session_meta\",\"modelId\":\"test-model-antigravity-a\"}\n",
            "{\"type\":\"usage\",\"responseId\":\"r1\",\"timestamp\":1787572800000,\"input\":100,\"output\":20,\"cacheRead\":10,\"cacheWrite\":5}\n",
            "{\"type\":\"usage\",\"response_id\":\"r1\",\"timestamp\":1787572800000,\"input\":100,\"output\":20}\n"
        )).unwrap();
        let now = Utc.timestamp_millis_opt(1787576400000).single().unwrap();
        let summary = summarize_paths(&[path], now, 7);
        assert_eq!(summary.total_tokens, 135);
        assert_eq!(summary.session_count, 1);
    }

    #[test]
    fn excludes_usage_outside_requested_window() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-a.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"usage\",\"timestamp\":1787572800000,\"input\":10,\"output\":5}\n",
                "{\"type\":\"usage\",\"timestamp\":1784894400000,\"input\":99,\"output\":99}\n"
            ),
        )
        .unwrap();
        let now = Utc.timestamp_millis_opt(1787576400000).single().unwrap();
        let summary = summarize_paths(&[path], now, 7);
        assert_eq!(summary.total_tokens, 15);
        assert_eq!(summary.session_count, 1);
    }
}
