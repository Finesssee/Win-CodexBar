//! Local Grok CLI session-token activity for Usage & Spend (upstream 0.54 #3085).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Local};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyBucket {
    pub day: String,
    pub total_tokens: u64,
    pub session_count: u32,
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub session_count: u32,
    pub total_tokens: u64,
    pub daily: Vec<DailyBucket>,
}

pub fn summarize(lookback_days: u32) -> Summary {
    let root = grok_home().join("sessions");
    summarize_root(&root, lookback_days, Local::now())
}

fn grok_home() -> PathBuf {
    std::env::var("GROK_HOME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".grok")))
        .unwrap_or_else(|| PathBuf::from(".grok"))
}

fn summarize_root(root: &Path, lookback_days: u32, now: DateTime<Local>) -> Summary {
    let cutoff = now - Duration::days(i64::from(lookback_days.max(1)));
    let mut stack = vec![root.to_path_buf()];
    let mut session_count = 0u32;
    let mut total_tokens = 0u64;
    let mut daily_tokens: BTreeMap<String, u64> = BTreeMap::new();
    let mut daily_sessions: BTreeMap<String, u32> = BTreeMap::new();
    let mut daily_models: HashMap<String, HashMap<String, u32>> = HashMap::new();

    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.file_name().and_then(|name| name.to_str()) != Some("signals.json") {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let Ok(modified) = metadata.modified() else {
                continue;
            };
            let modified: DateTime<Local> = modified.into();
            if modified < cutoff || modified > now {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(json) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let before = json
                .get("totalTokensBeforeCompaction")
                .and_then(nonnegative_u64)
                .unwrap_or(0);
            let context = json
                .get("contextTokensUsed")
                .and_then(nonnegative_u64)
                .unwrap_or(0);
            let tokens = before.saturating_add(context);
            session_count = session_count.saturating_add(1);
            total_tokens = total_tokens.saturating_add(tokens);
            let day = modified.format("%Y-%m-%d").to_string();
            let day_total = daily_tokens.entry(day.clone()).or_default();
            *day_total = day_total.saturating_add(tokens);
            let day_sessions = daily_sessions.entry(day.clone()).or_default();
            *day_sessions = day_sessions.saturating_add(1);

            let mut models = Vec::new();
            if let Some(model) = json
                .get("primaryModelId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                models.push(model.to_string());
            }
            if let Some(extra) = json.get("modelsUsed").and_then(Value::as_array) {
                models.extend(
                    extra
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string),
                );
            }
            for model in models {
                *daily_models
                    .entry(day.clone())
                    .or_default()
                    .entry(model)
                    .or_default() += 1;
            }
        }
    }

    let daily = daily_tokens
        .into_iter()
        .map(|(day, total_tokens)| {
            let mut models: Vec<_> = daily_models
                .remove(&day)
                .unwrap_or_default()
                .into_iter()
                .collect();
            models.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
            DailyBucket {
                session_count: daily_sessions.get(&day).copied().unwrap_or(0),
                day,
                total_tokens,
                models: models.into_iter().map(|(model, _)| model).collect(),
            }
        })
        .collect();
    Summary {
        session_count,
        total_tokens,
        daily,
    }
}

fn nonnegative_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_signal_token_totals_by_local_day() {
        let root = tempfile::tempdir().unwrap();
        let session = root.path().join("cwd").join("session");
        fs::create_dir_all(&session).unwrap();
        fs::write(
            session.join("signals.json"),
            r#"{"totalTokensBeforeCompaction":100,"contextTokensUsed":25,"primaryModelId":"grok-4","modelsUsed":["grok-4-fast"]}"#,
        )
        .unwrap();
        let summary = summarize_root(root.path(), 30, Local::now() + Duration::seconds(1));
        assert_eq!(summary.session_count, 1);
        assert_eq!(summary.total_tokens, 125);
        assert_eq!(summary.daily.len(), 1);
        assert_eq!(summary.daily[0].total_tokens, 125);
        assert_eq!(summary.daily[0].models, vec!["grok-4", "grok-4-fast"]);
    }
}
