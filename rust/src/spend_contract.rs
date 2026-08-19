//! Unified Usage & Spend accounting contract for upstream 0.53 parity.
//! Accounting semantics live here so UI/CLI never infer unknown vs zero.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Datelike, Local, TimeZone, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::codex_workspaces::{CodexWorkspacesIndex, ProjectUsage, SessionUsage, SourceStatus};
use crate::cost_scanner::{
    CostScanner, CostSummary, ModelTokenCounts, get_daily_cost_history, get_daily_token_history,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CostProvenance {
    ListPriceEstimate,
    VendorMetered,
    Mixed,
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostCoverageCounts {
    pub priced: u32,
    pub unpriced: u32,
    pub unmetered: u32,
    pub estimated: u32,
}

impl CostCoverageCounts {
    pub fn total(&self) -> u32 {
        self.priced + self.unpriced + self.unmetered + self.estimated
    }

    pub fn coverage_ratio(&self) -> Option<f64> {
        let denominator = self.total();
        (denominator > 0).then(|| (self.priced + self.estimated) as f64 / denominator as f64)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendTokenMix {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendModelRow {
    pub model: String,
    /// None = unknown/unpriced. Some(0.0) = known free.
    pub cost_usd: Option<f64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_tokens: u64,
    pub custom_pricing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendDailyPoint {
    pub day: String,
    pub cost_usd: Option<f64>,
    pub total_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendActivityCell {
    /// Monday=0, Sunday=6.
    pub weekday: u8,
    pub hour: u8,
    pub conversations: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedSpendSource {
    pub source_id: String,
    pub display_name: String,
    pub request_count: u32,
    pub conversation_count: u32,
    pub token_mix: SpendTokenMix,
    pub coverage: CostCoverageCounts,
    pub models: Vec<SpendModelRow>,
    pub hourly_activity: Vec<SpendActivityCell>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendContract {
    pub provider_id: String,
    pub history_days: u32,
    /// Known subtotal for this window. None means unknown, never implicit zero.
    pub known_cost_usd: Option<f64>,
    pub known_zero: bool,
    pub provenance: CostProvenance,
    pub price_coverage: CostCoverageCounts,
    pub price_coverage_ratio: Option<f64>,
    pub history_coverage_established: bool,
    pub token_mix: SpendTokenMix,
    pub conversation_count: u32,
    pub models: Vec<SpendModelRow>,
    pub projects: Vec<ProjectUsage>,
    pub conversations: Vec<SessionUsage>,
    pub daily: Vec<SpendDailyPoint>,
    pub hourly_activity: Vec<SpendActivityCell>,
    pub project_source_status: Option<SourceStatus>,
    pub custom_pricing_active: bool,
    pub imports: Vec<ImportedSpendSource>,
}

#[derive(Debug, Clone, Default)]
struct CustomPricing {
    entries: HashMap<String, CustomRates>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct CustomRates {
    input: Option<f64>,
    output: Option<f64>,
    #[serde(rename = "cacheRead", alias = "cache_read")]
    cache_read: Option<f64>,
    #[serde(
        rename = "cacheWrite",
        alias = "cache_write",
        alias = "cacheCreation",
        alias = "cache_creation"
    )]
    cache_write: Option<f64>,
}

impl CustomPricing {
    fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|path| path.join("CodexBar").join("custom-pricing.json"))
    }

    fn load() -> Self {
        Self::default_path()
            .and_then(|path| fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<HashMap<String, CustomRates>>(&bytes).ok())
            .map(|entries| Self {
                entries: entries
                    .into_iter()
                    .filter_map(|(key, rates)| {
                        let key = key.trim().to_ascii_lowercase();
                        (!key.is_empty() && rates.is_valid()).then_some((key, rates))
                    })
                    .collect(),
            })
            .unwrap_or_default()
    }

    fn rates(&self, provider_id: &str, model: &str) -> Option<&CustomRates> {
        let model_key = model.trim().to_ascii_lowercase();
        let provider_key = format!("{}/{}", provider_id.trim().to_ascii_lowercase(), model_key);
        self.entries
            .get(&provider_key)
            .or_else(|| self.entries.get(&model_key))
    }
}

impl CustomRates {
    fn is_valid(&self) -> bool {
        [self.input, self.output, self.cache_read, self.cache_write]
            .into_iter()
            .flatten()
            .all(|value| value.is_finite() && value >= 0.0)
    }

    fn cost(&self, counts: &ModelTokenCounts) -> Option<f64> {
        let cached = counts.cached_tokens.min(counts.input_tokens);
        let uncached = counts.input_tokens.saturating_sub(cached);
        let mut total = 0.0;
        if uncached > 0 {
            total += uncached as f64 * self.input? / 1_000_000.0;
        }
        if counts.output_tokens > 0 {
            total += counts.output_tokens as f64 * self.output? / 1_000_000.0;
        }
        if cached > 0 {
            total += cached as f64 * self.cache_read? / 1_000_000.0;
        }
        total.is_finite().then_some(total)
    }
}

/// Build a stable accounting contract for a local-log provider.
/// `days=0` means the upstream All-time UI window, bounded to 365 days locally.
pub fn build_local_spend_contract(
    provider_id: &str,
    days: u32,
    include_opencodex: bool,
) -> SpendContract {
    let history_days = if days == 0 { 365 } else { days.clamp(1, 365) };
    let scanner = CostScanner::new(history_days);
    let summary = match provider_id {
        "codex" => scanner.scan_codex(),
        "claude" => scanner.scan_claude(),
        "opencodego" => scanner.scan_opencodego_with_cancel(None),
        _ => CostSummary::default(),
    };
    let custom = CustomPricing::load();
    let models = model_rows(provider_id, &summary, &custom);
    let price_coverage = coverage_for_models(&models);
    let known_cost_usd = known_subtotal(&models, &summary);
    let token_mix = SpendTokenMix {
        input_tokens: Some(summary.input_tokens),
        output_tokens: Some(summary.output_tokens),
        cache_read_tokens: Some(summary.cached_tokens),
        cache_creation_tokens: None,
        reasoning_tokens: None,
    };

    let (projects, conversations, project_source_status, hourly_activity, codex_daily) =
        if provider_id == "codex" {
            match CodexWorkspacesIndex::new(history_days).load_snapshot(false, |_| {}) {
                Ok(snapshot) => {
                    let activity = activity_from_sessions(&snapshot.sessions);
                    let daily = snapshot
                        .daily
                        .iter()
                        .map(|point| SpendDailyPoint {
                            day: point.day.clone(),
                            cost_usd: point.estimated_cost_usd,
                            total_tokens: Some(point.total_tokens),
                        })
                        .collect();
                    (
                        snapshot.projects,
                        snapshot.sessions,
                        Some(snapshot.source_status),
                        activity,
                        Some(daily),
                    )
                }
                Err(_) => (Vec::new(), Vec::new(), None, Vec::new(), None),
            }
        } else {
            (Vec::new(), Vec::new(), None, Vec::new(), None)
        };

    let daily = codex_daily.unwrap_or_else(|| daily_points(provider_id, history_days));
    let imports = if provider_id == "codex" && include_opencodex {
        load_opencodex_import(&custom).into_iter().collect()
    } else {
        Vec::new()
    };
    let imported_conversations = imports.iter().map(|source| source.conversation_count).sum::<u32>();

    SpendContract {
        provider_id: provider_id.to_string(),
        history_days,
        known_cost_usd,
        known_zero: summary.known_zero && imports.is_empty(),
        provenance: if known_cost_usd.is_some() {
            CostProvenance::ListPriceEstimate
        } else {
            CostProvenance::Unknown
        },
        price_coverage_ratio: price_coverage.coverage_ratio(),
        price_coverage,
        history_coverage_established: summary.history_coverage_established,
        token_mix,
        conversation_count: if conversations.is_empty() {
            summary.sessions_count.saturating_add(imported_conversations)
        } else {
            (conversations.len().min(u32::MAX as usize) as u32).saturating_add(imported_conversations)
        },
        models,
        projects,
        conversations,
        daily,
        hourly_activity,
        project_source_status,
        custom_pricing_active: !custom.entries.is_empty(),
        imports,
    }
}

fn model_rows(
    provider_id: &str,
    summary: &CostSummary,
    custom: &CustomPricing,
) -> Vec<SpendModelRow> {
    let mut names: HashSet<String> = summary.by_model.keys().cloned().collect();
    names.extend(summary.by_model_tokens.keys().cloned());
    names.extend(summary.unknown_models.iter().cloned());
    let mut rows: Vec<_> = names
        .into_iter()
        .map(|model| {
            let counts = summary.by_model_tokens.get(&model).cloned().unwrap_or_default();
            let custom_rates = custom.rates(provider_id, &model);
            // Exact-match overlay is authoritative when present. Missing fields
            // remain unknown rather than falling back to built-in/model.dev rates.
            let cost_usd = if let Some(rates) = custom_rates {
                rates.cost(&counts)
            } else if summary.unknown_models.contains(&model) {
                None
            } else {
                summary
                    .by_model
                    .get(&model)
                    .copied()
                    .filter(|value| value.is_finite() && *value >= 0.0)
            };
            SpendModelRow {
                model,
                cost_usd,
                input_tokens: counts.input_tokens,
                output_tokens: counts.output_tokens,
                cache_read_tokens: counts.cached_tokens,
                total_tokens: counts.total(),
                custom_pricing: custom_rates.is_some(),
            }
        })
        .collect();
    rows.sort_by(|left, right| match (left.cost_usd, right.cost_usd) {
        (Some(a), Some(b)) => b
            .partial_cmp(&a)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.model.cmp(&right.model)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => left.model.cmp(&right.model),
    });
    rows
}

fn coverage_for_models(models: &[SpendModelRow]) -> CostCoverageCounts {
    let mut coverage = CostCoverageCounts::default();
    for model in models {
        if model.cost_usd.is_some() {
            coverage.estimated = coverage.estimated.saturating_add(1);
        } else {
            coverage.unpriced = coverage.unpriced.saturating_add(1);
        }
    }
    coverage
}

fn known_subtotal(models: &[SpendModelRow], summary: &CostSummary) -> Option<f64> {
    if models.is_empty() {
        return summary.known_zero.then_some(0.0);
    }
    let mut total = 0.0;
    let mut saw_known = false;
    for model in models {
        if let Some(cost) = model.cost_usd {
            total += cost;
            saw_known = true;
        }
    }
    (saw_known && total.is_finite()).then_some(total)
}

fn daily_points(provider_id: &str, days: u32) -> Vec<SpendDailyPoint> {
    let costs: HashMap<String, f64> =
        get_daily_cost_history(provider_id, days).into_iter().collect();
    let (tokens, incomplete) = get_daily_token_history(provider_id, days);
    tokens
        .into_iter()
        .map(|(day, total_tokens)| SpendDailyPoint {
            cost_usd: costs.get(&day).copied().filter(|_| !incomplete),
            day,
            total_tokens: (!incomplete).then_some(total_tokens),
        })
        .collect()
}

fn activity_from_sessions(sessions: &[SessionUsage]) -> Vec<SpendActivityCell> {
    let mut cells: BTreeMap<(u8, u8), u32> = BTreeMap::new();
    for session in sessions {
        let Some(timestamp) = session.latest_activity.or(session.started_at) else {
            continue;
        };
        let local = timestamp.with_timezone(&Local);
        let key = (
            local.weekday().num_days_from_monday() as u8,
            local.hour() as u8,
        );
        let next = cells.get(&key).copied().unwrap_or(0).saturating_add(1);
        cells.insert(key, next);
    }
    cells
        .into_iter()
        .map(|((weekday, hour), conversations)| SpendActivityCell {
            weekday,
            hour,
            conversations,
        })
        .collect()
}

fn load_opencodex_import(custom: &CustomPricing) -> Option<ImportedSpendSource> {
    let path = opencodex_usage_path()?;
    let text = fs::read_to_string(path).ok()?;
    let mut request_count = 0u32;
    let mut conversations = HashSet::new();
    let mut model_counts: HashMap<String, ModelTokenCounts> = HashMap::new();
    let mut token_mix = SpendTokenMix::default();
    let mut coverage = CostCoverageCounts::default();
    let mut activity: BTreeMap<(u8, u8), u32> = BTreeMap::new();

    for line in text.lines() {
        let Some(entry) = parse_opencodex_line(line) else {
            continue;
        };
        request_count = request_count.saturating_add(1);
        if let Some(conversation) = entry.conversation_id {
            conversations.insert(conversation);
        }
        let counts = model_counts.entry(entry.model).or_default();
        counts.input_tokens = counts.input_tokens.saturating_add(entry.input_tokens.unwrap_or(0));
        counts.output_tokens = counts.output_tokens.saturating_add(entry.output_tokens.unwrap_or(0));
        counts.cached_tokens = counts
            .cached_tokens
            .saturating_add(entry.cache_read_tokens.unwrap_or(0));
        token_mix.input_tokens = add_optional(token_mix.input_tokens, entry.input_tokens);
        token_mix.output_tokens = add_optional(token_mix.output_tokens, entry.output_tokens);
        token_mix.cache_read_tokens = add_optional(token_mix.cache_read_tokens, entry.cache_read_tokens);
        token_mix.cache_creation_tokens =
            add_optional(token_mix.cache_creation_tokens, entry.cache_creation_tokens);
        token_mix.reasoning_tokens = add_optional(token_mix.reasoning_tokens, entry.reasoning_tokens);
        match entry.usage_status.as_str() {
            "reported" => coverage.priced = coverage.priced.saturating_add(1),
            "estimated" => coverage.estimated = coverage.estimated.saturating_add(1),
            "unsupported" => coverage.unmetered = coverage.unmetered.saturating_add(1),
            _ => coverage.unpriced = coverage.unpriced.saturating_add(1),
        }
        let local = entry.timestamp.with_timezone(&Local);
        let key = (
            local.weekday().num_days_from_monday() as u8,
            local.hour() as u8,
        );
        let next = activity.get(&key).copied().unwrap_or(0).saturating_add(1);
        activity.insert(key, next);
    }

    if request_count == 0 {
        return None;
    }
    let synthetic = CostSummary {
        by_model_tokens: model_counts,
        ..CostSummary::default()
    };
    let models = model_rows("opencodex", &synthetic, custom);
    Some(ImportedSpendSource {
        source_id: "opencodex".to_string(),
        display_name: "OpenCodex".to_string(),
        request_count,
        conversation_count: conversations.len().min(u32::MAX as usize) as u32,
        token_mix,
        coverage,
        models,
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

fn opencodex_usage_path() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("OPENCODEX_HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed).join("usage.jsonl"));
        }
    }
    dirs::home_dir().map(|home| home.join(".opencodex").join("usage.jsonl"))
}

struct OpenCodexLine {
    timestamp: DateTime<Utc>,
    model: String,
    usage_status: String,
    conversation_id: Option<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_tokens: Option<u64>,
    cache_creation_tokens: Option<u64>,
    reasoning_tokens: Option<u64>,
}

fn parse_opencodex_line(line: &str) -> Option<OpenCodexLine> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    let model = value.get("model")?.as_str()?.trim().to_string();
    if model.is_empty() {
        return None;
    }
    let timestamp = parse_timestamp(value.get("timestamp")?)?;
    let usage = value.get("usage").and_then(Value::as_object);
    Some(OpenCodexLine {
        timestamp,
        model,
        usage_status: value
            .get("usageStatus")
            .and_then(Value::as_str)
            .unwrap_or("unreported")
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
    })
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
    let whole = seconds.trunc() as i64;
    let nanos = (seconds.fract().abs() * 1_000_000_000.0) as u32;
    Utc.timestamp_opt(whole, nanos).single()
}

fn nonnegative_u64(value: Option<&Value>) -> Option<u64> {
    let value = value?;
    if let Some(number) = value.as_u64() {
        return Some(number);
    }
    let number = value.as_f64()?;
    (number.is_finite() && number >= 0.0 && number <= u64::MAX as f64)
        .then_some(number as u64)
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
    use super::*;

    #[test]
    fn coverage_ratio_counts_estimated_as_covered() {
        let coverage = CostCoverageCounts {
            priced: 1,
            unpriced: 1,
            unmetered: 0,
            estimated: 2,
        };
        assert_eq!(coverage.coverage_ratio(), Some(0.75));
    }

    #[test]
    fn explicit_zero_custom_rate_is_known_free_but_missing_rate_is_unknown() {
        let counts = ModelTokenCounts {
            input_tokens: 1_000_000,
            output_tokens: 0,
            cached_tokens: 0,
        };
        let free = CustomRates {
            input: Some(0.0),
            ..CustomRates::default()
        };
        let missing = CustomRates ::default();
        assert_eq!(free.cost(&counts), Some(0.0));
        assert_eq!(missing.cost(&counts), None);
    }

    #[test]
    fn opencodex_parser_keeps_reported_token_classes() {
        let line = r#"{"requestId":"r1","timestamp":"2026-08-18T10:00:00Z","provider":"openai","model":"gpt-test","usageStatus":"reported","conversationId":"c1","usage":{"inputTokens":10,"outputTokens":4,"cachedInputTokens":3,"reasoningOutputTokens":2}}"#;
        let entry = parse_opencodex_line(line).expect("entry");
        assert_eq!(entry.model, "gpt-test");
        assert_eq!(entry.input_tokens, Some(10));
        assert_eq!(entry.output_tokens, Some(4));
        assert_eq!(entry.cache_read_tokens, Some(3));
        assert_eq!(entry.reasoning_tokens, Some(2));
    }
}
