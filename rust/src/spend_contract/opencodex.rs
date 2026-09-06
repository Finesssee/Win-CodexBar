use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use chrono::{DateTime, Datelike, Duration, Local, TimeZone, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::CostUsagePricing;

use super::{
    CostCoverageCounts, CustomPricing, ImportedSpendSource, SpendActivityCell, SpendDailyPoint,
    SpendModelRow, SpendTokenMix,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenCodexEntry {
    request_id: String,
    timestamp: DateTime<Utc>,
    provider: String,
    model: String,
    usage_status: String,
    conversation_id: Option<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_tokens: Option<u64>,
    cache_creation_tokens: Option<u64>,
    reasoning_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

mod cache;

#[derive(Default)]
struct ModelAccumulator {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_creation: u64,
    total: Option<u64>,
    cost: Option<f64>,
    custom_pricing: bool,
}

#[derive(Default)]
struct DailyAccumulator {
    cost: f64,
    saw_cost: bool,
    total_tokens: u64,
    saw_tokens: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteTarget {
    Subscription(&'static str),
    TokenOnly,
    Unknown,
}

fn route_provider(provider: &str) -> RouteTarget {
    match provider.trim().to_ascii_lowercase().as_str() {
        "openai" => RouteTarget::Subscription("codex"),
        "opencode-go" => RouteTarget::Subscription("opencodego"),
        "kimi-coding" | "kimi-for-coding" => RouteTarget::Subscription("kimi"),
        "deepseek" => RouteTarget::Subscription("deepseek"),
        "opencode-free" | "opencode" => RouteTarget::TokenOnly,
        _ => RouteTarget::Unknown,
    }
}

fn route_model(model: &str) -> RouteTarget {
    let trimmed = model.trim();
    let Some((prefix, _)) = trimmed.split_once('/') else {
        return RouteTarget::Subscription("codex");
    };
    if prefix.is_empty() {
        RouteTarget::Unknown
    } else {
        route_provider(prefix)
    }
}

fn route_entry(entry: &OpenCodexEntry) -> RouteTarget {
    if entry.model.trim().contains('/') {
        let routed = route_model(&entry.model);
        if routed != RouteTarget::Unknown {
            return routed;
        }
    }
    route_provider(&entry.provider)
}

pub(super) fn load_for_subscription(
    provider_id: &str,
    history_days: u32,
    custom: &CustomPricing,
) -> Option<ImportedSpendSource> {
    let source_path = usage_path()?;
    let entries = cache::load_entries(&source_path)?;
    let entries = entries
        .into_iter()
        .filter(|entry| matches!(route_entry(entry), RouteTarget::Subscription(id) if id == provider_id))
        .collect();
    aggregate(entries, Utc::now(), history_days.clamp(1, 365), custom)
}

fn aggregate(
    entries: Vec<OpenCodexEntry>,
    now: DateTime<Utc>,
    history_days: u32,
    custom: &CustomPricing,
) -> Option<ImportedSpendSource> {
    let first_day = now.with_timezone(&Local).date_naive()
        - Duration::days(i64::from(history_days.saturating_sub(1)));

    // requestId is authoritative: a later row replaces an earlier row with the same id.
    let mut unique: HashMap<String, OpenCodexEntry> = HashMap::new();
    for entry in entries {
        unique.insert(entry.request_id.clone(), entry);
    }
    let mut entries: Vec<_> = unique
        .into_values()
        .filter(|entry| {
            entry.timestamp <= now
                && entry.timestamp.with_timezone(&Local).date_naive() >= first_day
        })
        .collect();
    entries.sort_by(|left, right| {
        left.timestamp
            .cmp(&right.timestamp)
            .then_with(|| left.request_id.cmp(&right.request_id))
    });
    if entries.is_empty() {
        return None;
    }

    let mut conversations = HashSet::new();
    let mut token_mix = SpendTokenMix::default();
    let mut coverage = CostCoverageCounts::default();
    let mut activity: BTreeMap<(u8, u8), u32> = BTreeMap::new();
    let mut models: HashMap<String, ModelAccumulator> = HashMap::new();
    let mut daily: BTreeMap<String, DailyAccumulator> = BTreeMap::new();
    let mut known_cost = 0.0;
    let mut saw_known_cost = false;
    // Upstream 0.55.0 #3136: resolve the dynamic pricing catalog once per
    // aggregate instead of re-checking its cache metadata for every usage row.
    let pricing_snapshot = crate::core::pricing_snapshot();

    for entry in &entries {
        if let Some(conversation) = entry.conversation_id.as_ref() {
            conversations.insert(conversation.clone());
        }
        token_mix.input_tokens = add_optional(token_mix.input_tokens, entry.input_tokens);
        token_mix.output_tokens = add_optional(token_mix.output_tokens, entry.output_tokens);
        token_mix.cache_read_tokens =
            add_optional(token_mix.cache_read_tokens, entry.cache_read_tokens);
        token_mix.cache_creation_tokens =
            add_optional(token_mix.cache_creation_tokens, entry.cache_creation_tokens);
        token_mix.reasoning_tokens =
            add_optional(token_mix.reasoning_tokens, entry.reasoning_tokens);

        let cost = entry_cost(entry, custom, &pricing_snapshot);
        match entry.usage_status.as_str() {
            "reported" if cost.is_some() => coverage.priced = coverage.priced.saturating_add(1),
            "estimated" if cost.is_some() => {
                coverage.estimated = coverage.estimated.saturating_add(1)
            }
            "unsupported" => coverage.unmetered = coverage.unmetered.saturating_add(1),
            _ => coverage.unpriced = coverage.unpriced.saturating_add(1),
        }
        if let Some(cost) = cost {
            known_cost += cost;
            saw_known_cost = true;
        }

        let local = entry.timestamp.with_timezone(&Local);
        // Weekday (0-6) and hour (0-23) both fit u8.
        #[allow(
            clippy::cast_possible_truncation,
            reason = "weekday (0-6) and hour (0-23) fit u8"
        )]
        let key = (
            local.weekday().num_days_from_monday() as u8,
            local.hour() as u8,
        );
        activity.insert(
            key,
            activity.get(&key).copied().unwrap_or(0).saturating_add(1),
        );

        let day = daily
            .entry(local.date_naive().format("%Y-%m-%d").to_string())
            .or_default();
        if let Some(cost) = cost {
            day.cost += cost;
            day.saw_cost = true;
        }
        if let Some(total) = entry.resolved_total_tokens() {
            day.total_tokens = day.total_tokens.saturating_add(total);
            day.saw_tokens = true;
        }

        let model = models.entry(entry.model.clone()).or_default();
        model.input = model.input.saturating_add(entry.input_tokens.unwrap_or(0));
        model.output = model
            .output
            .saturating_add(entry.output_tokens.unwrap_or(0));
        model.cache_read = model
            .cache_read
            .saturating_add(entry.cache_read_tokens.unwrap_or(0));
        model.cache_creation = model
            .cache_creation
            .saturating_add(entry.cache_creation_tokens.unwrap_or(0));
        if let Some(total) = entry.resolved_total_tokens() {
            model.total = Some(model.total.unwrap_or(0).saturating_add(total));
        }
        if let Some(cost) = cost {
            model.cost = Some(model.cost.unwrap_or(0.0) + cost);
        }
        model.custom_pricing |= custom.rates(&entry.provider, &entry.model).is_some();
    }

    let mut model_rows: Vec<_> = models
        .into_iter()
        .map(|(model, acc)| SpendModelRow {
            model,
            cost_usd: acc.cost,
            input_tokens: acc.input,
            output_tokens: acc.output,
            cache_read_tokens: acc.cache_read,
            total_tokens: acc.total.unwrap_or_else(|| {
                acc.input
                    .saturating_add(acc.output)
                    .saturating_add(acc.cache_creation)
            }),
            custom_pricing: acc.custom_pricing,
        })
        .collect();
    model_rows.sort_by(|left, right| match (left.cost_usd, right.cost_usd) {
        (Some(a), Some(b)) => b
            .partial_cmp(&a)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.model.cmp(&right.model)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => left.model.cmp(&right.model),
    });

    // Counts are clamped to u32::MAX before casting.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "clamped to u32::MAX before casting"
    )]
    let request_count = entries.len().min(u32::MAX as usize) as u32;
    #[allow(
        clippy::cast_possible_truncation,
        reason = "clamped to u32::MAX before casting"
    )]
    let conversation_count = conversations.len().min(u32::MAX as usize) as u32;

    Some(ImportedSpendSource {
        source_id: "opencodex".to_string(),
        display_name: "OpenCodex".to_string(),
        request_count,
        conversation_count,
        known_cost_usd: saw_known_cost.then_some(known_cost),
        token_mix,
        coverage,
        models: model_rows,
        daily: daily
            .into_iter()
            .map(|(day, acc)| SpendDailyPoint {
                day,
                cost_usd: acc.saw_cost.then_some(acc.cost),
                total_tokens: acc.saw_tokens.then_some(acc.total_tokens),
            })
            .collect(),
        hourly_activity: activity
            .into_iter()
            .map(|((weekday, hour), conversations)| SpendActivityCell {
                weekday,
                hour,
                conversations,
            })
            .collect(),
    })
}

fn entry_cost(
    entry: &OpenCodexEntry,
    custom: &CustomPricing,
    pricing_snapshot: &crate::core::ModelsDevPricingSnapshot,
) -> Option<f64> {
    if !matches!(entry.usage_status.as_str(), "reported" | "estimated") {
        return None;
    }
    let has_usage = entry.total_tokens.is_some()
        || entry.input_tokens.is_some()
        || entry.output_tokens.is_some()
        || entry.cache_read_tokens.is_some()
        || entry.cache_creation_tokens.is_some();
    if !has_usage {
        return None;
    }
    let input = entry.input_tokens.unwrap_or(0);
    let output = entry.output_tokens.unwrap_or(0);
    let cache_read = entry.cache_read_tokens.unwrap_or(0);
    let cache_write = entry.cache_creation_tokens.unwrap_or(0);
    if let Some(rates) = custom.rates(&entry.provider, &entry.model) {
        return rates.cost_parts(input, output, cache_read, cache_write);
    }
    let pricing_model = pricing_model(entry)?;
    CostUsagePricing::codex_cost_usd_at_date_with_pricing_snapshot(
        &pricing_model,
        input,
        cache_read,
        output,
        entry.timestamp.date_naive(),
        Some(pricing_snapshot),
    )
}

fn pricing_model(entry: &OpenCodexEntry) -> Option<String> {
    let target = route_entry(entry);
    let model = entry.model.trim();
    let model_tail = model.split_once('/').map(|(_, tail)| tail).unwrap_or(model);
    match target {
        RouteTarget::Subscription("codex") => Some(
            if model.contains('/')
                && model
                    .split_once('/')
                    .is_some_and(|(prefix, _)| prefix.eq_ignore_ascii_case("openai"))
            {
                model.to_string()
            } else {
                model_tail.to_string()
            },
        ),
        RouteTarget::Subscription("opencodego") => Some(format!("opencode/{model_tail}")),
        RouteTarget::Subscription("kimi") => Some(format!("kimi/{model_tail}")),
        RouteTarget::Subscription("deepseek") => Some(format!("deepseek/{model_tail}")),
        RouteTarget::Subscription(_) | RouteTarget::TokenOnly | RouteTarget::Unknown => None,
    }
}

fn usage_path() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("OPENCODEX_HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed).join("usage.jsonl"));
        }
    }
    dirs::home_dir().map(|home| home.join(".opencodex").join("usage.jsonl"))
}

fn parse_line(line: &str) -> Option<OpenCodexEntry> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    let request_id = value.get("requestId")?.as_str()?.trim().to_string();
    let model = value.get("model")?.as_str()?.trim().to_string();
    if request_id.is_empty() || model.is_empty() {
        return None;
    }
    let provider = value
        .get("provider")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("openai")
        .to_string();
    let timestamp = parse_timestamp(value.get("timestamp")?)?;
    let usage = value.get("usage").and_then(Value::as_object);
    Some(OpenCodexEntry {
        request_id,
        timestamp,
        provider,
        model,
        usage_status: value
            .get("usageStatus")
            .and_then(Value::as_str)
            .unwrap_or("unreported")
            .trim()
            .to_ascii_lowercase(),
        conversation_id: value
            .get("conversationId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        input_tokens: usage.and_then(|object| nonnegative_u64(object.get("inputTokens"))),
        output_tokens: usage.and_then(|object| nonnegative_u64(object.get("outputTokens"))),
        cache_read_tokens: usage.and_then(|object| {
            nonnegative_u64(object.get("cacheReadInputTokens"))
                .or_else(|| nonnegative_u64(object.get("cachedInputTokens")))
        }),
        cache_creation_tokens: usage
            .and_then(|object| nonnegative_u64(object.get("cacheCreationInputTokens"))),
        reasoning_tokens: usage
            .and_then(|object| nonnegative_u64(object.get("reasoningOutputTokens"))),
        total_tokens: value
            .get("totalTokens")
            .and_then(|value| nonnegative_u64(Some(value))),
    })
}

impl OpenCodexEntry {
    fn resolved_total_tokens(&self) -> Option<u64> {
        self.total_tokens.or_else(|| {
            let mut saw = false;
            let mut total = 0u64;
            for value in [
                self.input_tokens,
                self.output_tokens,
                self.cache_creation_tokens,
            ]
            .into_iter()
            .flatten()
            {
                saw = true;
                total = total.saturating_add(value);
            }
            saw.then_some(total)
        })
    }
}

fn parse_timestamp(value: &Value) -> Option<DateTime<Utc>> {
    if let Some(raw) = value.as_str() {
        if let Ok(parsed) = DateTime::parse_from_rfc3339(raw.trim()) {
            return Some(parsed.with_timezone(&Utc));
        }
        if let Ok(number) = raw.trim().parse::<f64>() {
            return timestamp_from_epoch(number);
        }
    }
    value.as_f64().and_then(timestamp_from_epoch)
}

fn timestamp_from_epoch(value: f64) -> Option<DateTime<Utc>> {
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    let seconds = if value >= 1_000_000_000_000.0 {
        value / 1000.0
    } else {
        value
    };
    // Epoch timestamps fit i64 seconds; float-to-int casts saturate otherwise.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "epoch timestamps fit i64 seconds"
    )]
    let whole = seconds.trunc() as i64;
    // Fractional seconds in [0, 1e9) fit u32.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "fractional seconds in [0, 1e9) fit u32"
    )]
    let nanos = (seconds.fract().abs() * 1_000_000_000.0) as u32;
    Utc.timestamp_opt(whole, nanos).single()
}

fn nonnegative_u64(value: Option<&Value>) -> Option<u64> {
    let value = value?;
    if let Some(number) = value.as_u64() {
        return Some(number);
    }
    let number = value.as_f64()?;
    // Guarded to finite non-negative values within u64 range.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "guarded to finite non-negative values within u64 range"
    )]
    let parsed =
        (number.is_finite() && number >= 0.0 && number <= u64::MAX as f64).then_some(number as u64);
    parsed
}

fn add_optional(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => left.checked_add(right),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::cache::{load_entries_with_cache, read_cache};
    use super::*;
    use std::fs;

    #[test]
    fn aggregate_deduplicates_requests_and_applies_history_window() {
        let now = DateTime::parse_from_rfc3339("2026-08-19T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let make = |request_id: &str, timestamp: &str, input: u64| OpenCodexEntry {
            request_id: request_id.into(),
            timestamp: DateTime::parse_from_rfc3339(timestamp)
                .unwrap()
                .with_timezone(&Utc),
            provider: "openai".into(),
            model: "gpt-5".into(),
            usage_status: "reported".into(),
            conversation_id: Some(request_id.into()),
            input_tokens: Some(input),
            output_tokens: Some(1),
            cache_read_tokens: Some(0),
            cache_creation_tokens: None,
            reasoning_tokens: None,
            total_tokens: Some(input + 1),
        };
        let source = aggregate(
            vec![
                make("same", "2026-08-18T10:00:00Z", 10),
                make("same", "2026-08-18T11:00:00Z", 20),
                make("old", "2026-08-01T10:00:00Z", 30),
            ],
            now,
            7,
            &CustomPricing::default(),
        )
        .expect("source");
        assert_eq!(source.request_count, 1);
        assert_eq!(source.conversation_count, 1);
        assert_eq!(source.token_mix.input_tokens, Some(20));
        assert_eq!(source.coverage.priced, 1);
        assert!(source.known_cost_usd.is_some());
    }

    fn entry(provider: &str, model: &str) -> OpenCodexEntry {
        OpenCodexEntry {
            request_id: format!("{provider}:{model}"),
            timestamp: DateTime::parse_from_rfc3339("2026-07-29T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            provider: provider.to_string(),
            model: model.to_string(),
            usage_status: "reported".to_string(),
            conversation_id: None,
            input_tokens: Some(100),
            output_tokens: Some(5),
            cache_read_tokens: Some(10),
            cache_creation_tokens: None,
            reasoning_tokens: None,
            total_tokens: Some(105),
        }
    }

    #[test]
    fn routes_opencodex_entries_into_subscription_rows() {
        assert_eq!(
            route_entry(&entry("openai", "gpt-5.6-sol")),
            RouteTarget::Subscription("codex")
        );
        assert_eq!(
            route_entry(&entry("opencode-go", "gpt-5.6-sol")),
            RouteTarget::Subscription("opencodego")
        );
        assert_eq!(
            route_entry(&entry("kimi-coding", "k2p5")),
            RouteTarget::Subscription("kimi")
        );
        assert_eq!(
            route_entry(&entry("deepseek", "deepseek-chat")),
            RouteTarget::Subscription("deepseek")
        );
        assert_eq!(
            route_entry(&entry("opencode-free", "free-model")),
            RouteTarget::TokenOnly
        );
    }

    #[test]
    fn model_prefix_wins_over_mismatched_provider_label() {
        assert_eq!(
            route_entry(&entry("openai", "opencode-go/deepseek-v4-flash")),
            RouteTarget::Subscription("opencodego")
        );
        assert_eq!(
            route_entry(&entry("opencode-go", "openai/gpt-5.6-sol")),
            RouteTarget::Subscription("codex")
        );
    }

    #[test]
    fn pricing_model_uses_routed_vendor_catalog() {
        assert_eq!(
            pricing_model(&entry("opencode-go", "gpt-5")).as_deref(),
            Some("opencode/gpt-5")
        );
        assert_eq!(
            pricing_model(&entry("kimi-coding", "k2p5")).as_deref(),
            Some("kimi/k2p5")
        );
        assert_eq!(
            pricing_model(&entry("deepseek", "deepseek-chat")).as_deref(),
            Some("deepseek/deepseek-chat")
        );
    }

    #[test]
    fn opencodex_uses_request_day_for_historical_gpt56_pricing() {
        let entry = entry("openai", "gpt-5.6-terra");
        let pricing_snapshot = crate::core::pricing_snapshot();
        let cost = entry_cost(&entry, &CustomPricing::default(), &pricing_snapshot).unwrap();
        let expected = 90.0 * 2.5e-6 + 10.0 * 2.5e-7 + 5.0 * 1.5e-5;
        assert!((cost - expected).abs() < 1e-12);
    }

    #[test]
    fn parser_keeps_reported_token_classes() {
        let value = serde_json::json!({
            "requestId": "r1", "timestamp": "2026-08-18T10:00:00Z", "provider": "openai",
            "model": "gpt-test", "usageStatus": "reported", "conversationId": "c1",
            "usage": {"inputTokens": 10, "outputTokens": 4, "cachedInputTokens": 3, "reasoningOutputTokens": 2}
        });
        let entry = parse_line(&value.to_string()).expect("entry");
        assert_eq!(entry.model, "gpt-test");
        assert_eq!(entry.input_tokens, Some(10));
        assert_eq!(entry.output_tokens, Some(4));
        assert_eq!(entry.cache_read_tokens, Some(3));
        assert_eq!(entry.reasoning_tokens, Some(2));
    }

    #[test]
    fn parser_normalizes_defaults_and_rejects_malformed_lines() {
        let minimal = serde_json::json!({
            "requestId": "  r1  ", "model": "gpt-test", "timestamp": "2026-08-18T10:00:00Z",
            "usageStatus": "  REPORTED ", "usage": {"cacheCreationInputTokens": 7}
        });
        let entry = parse_line(&minimal.to_string()).expect("entry");
        assert_eq!(entry.request_id, "r1", "ids are trimmed");
        assert_eq!(
            entry.provider, "openai",
            "missing provider defaults to openai"
        );
        assert_eq!(
            entry.usage_status, "reported",
            "status is lowercased and trimmed"
        );
        assert_eq!(entry.conversation_id, None);
        assert_eq!(entry.cache_creation_tokens, Some(7));

        for malformed in [
            "{}",
            r#"{"requestId": "", "model": "m", "timestamp": "2026-08-18T10:00:00Z"}"#,
            r#"{"requestId": "r1", "model": "   ", "timestamp": "2026-08-18T10:00:00Z"}"#,
            r#"{"requestId": "r1", "model": "m"}"#,
            "not json at all",
        ] {
            assert!(parse_line(malformed).is_none(), "rejected: {malformed}");
        }
    }

    #[test]
    fn incremental_cache_appends_only_newline_terminated_tail() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("usage.jsonl");
        let cache = dir.path().join("cache.sqlite");
        let row = |id: &str, input: u64| {
            format!(
                r#"{{"requestId":"{id}","model":"gpt-5","timestamp":"2026-08-18T10:00:00Z","usageStatus":"reported","usage":{{"inputTokens":{input}}}}}"#
            )
        };

        fs::write(&log, format!("{}\n{}\n", row("a", 1), row("b", 2))).unwrap();
        let first = load_entries_with_cache(&log, &cache).unwrap();
        assert_eq!(
            first
                .iter()
                .map(|entry| entry.request_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        let first_cursor = read_cache(&cache).unwrap().cursor;

        let mut file = fs::OpenOptions::new().append(true).open(&log).unwrap();
        use std::io::Write as _;
        writeln!(file, "{}", row("c", 3)).unwrap();
        drop(file);

        let second = load_entries_with_cache(&log, &cache).unwrap();
        assert_eq!(
            second
                .iter()
                .map(|entry| entry.request_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
        let second_cursor = read_cache(&cache).unwrap().cursor;
        assert!(second_cursor.parsed_offset > first_cursor.parsed_offset);
    }

    #[test]
    fn incomplete_trailing_opencodex_record_waits_for_newline() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("usage.jsonl");
        let cache = dir.path().join("cache.sqlite");
        let complete = r#"{"requestId":"a","model":"gpt-5","timestamp":"2026-08-18T10:00:00Z"}"#;
        let pending = r#"{"requestId":"b","model":"gpt-5","timestamp":"2026-08-18T10:00:00Z"}"#;
        let split = pending.len() / 2;
        fs::write(&log, format!("{complete}\n{}", &pending[..split])).unwrap();

        let first = load_entries_with_cache(&log, &cache).unwrap();
        assert_eq!(
            first
                .iter()
                .map(|entry| entry.request_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a"]
        );
        let cursor = read_cache(&cache).unwrap().cursor;
        assert_eq!(
            cursor.parsed_offset,
            u64::try_from(complete.len() + 1).unwrap()
        );

        let mut file = fs::OpenOptions::new().append(true).open(&log).unwrap();
        use std::io::Write as _;
        writeln!(file, "{}", &pending[split..]).unwrap();
        drop(file);
        let second = load_entries_with_cache(&log, &cache).unwrap();
        assert_eq!(
            second
                .iter()
                .map(|entry| entry.request_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[test]
    fn later_request_id_replaces_cached_entry_without_full_cache_loss() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("usage.jsonl");
        let cache = dir.path().join("cache.sqlite");
        let row = |id: &str, input: u64| {
            format!(
                r#"{{"requestId":"{id}","model":"gpt-5","timestamp":"2026-08-18T10:00:00Z","usageStatus":"reported","usage":{{"inputTokens":{input}}}}}"#
            )
        };
        fs::write(&log, format!("{}\n{}\n", row("dup", 1), row("keep", 2))).unwrap();
        let _ = load_entries_with_cache(&log, &cache).unwrap();
        let mut file = fs::OpenOptions::new().append(true).open(&log).unwrap();
        use std::io::Write as _;
        writeln!(file, "{}", row("dup", 9)).unwrap();
        drop(file);

        let entries = load_entries_with_cache(&log, &cache).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries
                .iter()
                .find(|entry| entry.request_id == "dup")
                .unwrap()
                .input_tokens,
            Some(9)
        );
        assert!(entries.iter().any(|entry| entry.request_id == "keep"));
    }

    #[test]
    fn truncation_invalidates_opencodex_cursor_and_rebuilds() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("usage.jsonl");
        let cache = dir.path().join("cache.sqlite");
        let old = r#"{"requestId":"old","model":"gpt-5","timestamp":"2026-08-18T10:00:00Z"}"#;
        let replacement =
            r#"{"requestId":"new","model":"gpt-5","timestamp":"2026-08-18T10:00:00Z"}"#;
        fs::write(&log, format!("{old}\n{old}\n")).unwrap();
        let _ = load_entries_with_cache(&log, &cache).unwrap();
        fs::write(&log, format!("{replacement}\n")).unwrap();

        let rebuilt = load_entries_with_cache(&log, &cache).unwrap();
        assert_eq!(
            rebuilt
                .iter()
                .map(|entry| entry.request_id.as_str())
                .collect::<Vec<_>>(),
            vec!["new"]
        );
    }

    #[test]
    fn timestamps_parse_rfc3339_epoch_seconds_and_millis() {
        let expected = Utc.with_ymd_and_hms(2026, 8, 18, 10, 0, 0).unwrap();
        let rfc3339 = serde_json::json!("2026-08-18T10:00:00Z");
        assert_eq!(parse_timestamp(&rfc3339), Some(expected));
        let epoch_seconds = serde_json::json!(1_787_047_200i64);
        assert_eq!(parse_timestamp(&epoch_seconds), Some(expected));
        let epoch_millis = serde_json::json!(1_787_047_200_000f64);
        assert_eq!(parse_timestamp(&epoch_millis), Some(expected));
        let numeric_string = serde_json::json!("1787047200.0");
        assert_eq!(parse_timestamp(&numeric_string), Some(expected));
        for invalid in [
            serde_json::json!("not a date"),
            serde_json::json!(0),
            serde_json::json!(-5.0),
            serde_json::Value::Null,
            serde_json::json!(true),
        ] {
            assert!(parse_timestamp(&invalid).is_none(), "rejected: {invalid}");
        }
    }

    #[test]
    fn nonnegative_u64_accepts_json_numbers_and_bounded_floats() {
        assert_eq!(nonnegative_u64(Some(&serde_json::json!(42))), Some(42));
        assert_eq!(nonnegative_u64(Some(&serde_json::json!(12.0))), Some(12));
        // Fractional floats are accepted via `as u64` truncation.
        assert_eq!(nonnegative_u64(Some(&serde_json::json!(1.5))), Some(1));
        // `u64::MAX as f64` rounds up to 2^64; f64 spacing there is 4096, so
        // +2048.0 rounds back into range. First out-of-range step is +4096.0.
        assert_eq!(
            nonnegative_u64(Some(&serde_json::json!(u64::MAX as f64 + 4096.0))),
            None
        );
        assert_eq!(nonnegative_u64(None), None);
    }
}
