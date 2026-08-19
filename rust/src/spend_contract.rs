//! Unified Usage & Spend accounting contract for upstream 0.53 parity.
//! Accounting semantics live here so UI/CLI never infer unknown vs zero.

mod opencodex;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use chrono::{Datelike, Local, Timelike};
use serde::{Deserialize, Serialize};

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
    pub known_cost_usd: Option<f64>,
    pub token_mix: SpendTokenMix,
    pub coverage: CostCoverageCounts,
    pub models: Vec<SpendModelRow>,
    pub daily: Vec<SpendDailyPoint>,
    pub hourly_activity: Vec<SpendActivityCell>,
}

struct NativeSpendData {
    projects: Vec<ProjectUsage>,
    conversations: Vec<SessionUsage>,
    project_source_status: Option<SourceStatus>,
    activity: Vec<SpendActivityCell>,
    daily: Vec<SpendDailyPoint>,
}

struct ResolvedSpendData {
    known_cost_usd: Option<f64>,
    price_coverage: CostCoverageCounts,
    token_mix: SpendTokenMix,
    models: Vec<SpendModelRow>,
    daily: Vec<SpendDailyPoint>,
    hourly_activity: Vec<SpendActivityCell>,
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
        self.cost_parts(
            counts.input_tokens,
            counts.output_tokens,
            counts.cached_tokens,
            0,
        )
    }

    fn cost_parts(&self, input: u64, output: u64, cache_read: u64, cache_write: u64) -> Option<f64> {
        let cached = cache_read.min(input);
        let uncached = input.saturating_sub(cached);
        let mut total = 0.0;
        if uncached > 0 { total += uncached as f64 * self.input? / 1_000_000.0; }
        if output > 0 { total += output as f64 * self.output? / 1_000_000.0; }
        if cached > 0 { total += cached as f64 * self.cache_read? / 1_000_000.0; }
        if cache_write > 0 { total += cache_write as f64 * self.cache_write? / 1_000_000.0; }
        total.is_finite().then_some(total)
    }
}

/// Build a stable accounting contract for a local-log provider.
///  means the upstream All-time UI window, bounded to 365 days locally.
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
    build_local_spend_contract_from_summary(
        provider_id,
        history_days,
        include_opencodex,
        false,
        summary,
    )
}

/// Build the accounting contract from an already-computed summary so callers do not rescan logs.
pub fn build_local_spend_contract_from_summary(
    provider_id: &str,
    history_days: u32,
    include_opencodex: bool,
    hide_native_codex_when_opencodex_present: bool,
    summary: CostSummary,
) -> SpendContract {
    let history_days = history_days.clamp(1, 365);
    let custom = CustomPricing::load();
    let native_models = model_rows(provider_id, &summary, &custom);
    let native_coverage = coverage_for_models(&native_models);
    let native_cost = known_subtotal(&native_models, &summary);
    let native_token_mix = SpendTokenMix {
        input_tokens: Some(summary.input_tokens),
        output_tokens: Some(summary.output_tokens),
        cache_read_tokens: Some(summary.cached_tokens),
        cache_creation_tokens: None,
        reasoning_tokens: None,
    };

    let native = load_native_spend(provider_id, history_days);
    let imports: Vec<_> = if provider_id == "codex" && include_opencodex {
        opencodex::load(history_days, &custom).into_iter().collect()
    } else {
        Vec::new()
    };
    let imported = imports.first();
    let replace_native = hide_native_codex_when_opencodex_present && imported.is_some();
    let resolved = resolve_spend(
        native_cost,
        native_coverage,
        native_token_mix,
        native_models,
        native.daily.clone(),
        native.activity.clone(),
        imported,
        replace_native,
    );

    let native_conversations = if native.conversations.is_empty() {
        summary.sessions_count
    } else {
        native.conversations.len().min(u32::MAX as usize) as u32
    };
    let imported_conversations = imported.map_or(0, |source| source.conversation_count);
    let conversation_count = if replace_native {
        imported_conversations
    } else {
        native_conversations.saturating_add(imported_conversations)
    };
    let known_zero = if replace_native {
        imported.is_some_and(|source| source.known_cost_usd == Some(0.0) && source.coverage.unpriced == 0)
    } else {
        summary.known_zero && imports.is_empty()
    };

    SpendContract {
        provider_id: provider_id.to_string(),
        history_days,
        known_cost_usd: resolved.known_cost_usd,
        known_zero,
        provenance: if resolved.known_cost_usd.is_some() { CostProvenance::ListPriceEstimate } else { CostProvenance::Unknown },
        price_coverage_ratio: resolved.price_coverage.coverage_ratio(),
        price_coverage: resolved.price_coverage,
        history_coverage_established: summary.history_coverage_established,
        token_mix: resolved.token_mix,
        conversation_count,
        models: resolved.models,
        projects: native.projects,
        conversations: native.conversations,
        daily: resolved.daily,
        hourly_activity: resolved.hourly_activity,
        project_source_status: native.project_source_status,
        custom_pricing_active: !custom.entries.is_empty(),
        imports,
    }
}

fn load_native_spend(provider_id: &str, history_days: u32) -> NativeSpendData {
    if provider_id != "codex" {
        return NativeSpendData { projects: Vec::new(), conversations: Vec::new(), project_source_status: None, activity: Vec::new(), daily: daily_points(provider_id, history_days) };
    }
    match CodexWorkspacesIndex::new(history_days).load_snapshot(false, |_| {}) {
        Ok(snapshot) => {
            let activity = activity_from_sessions(&snapshot.sessions);
            let daily = snapshot.daily.iter().map(|point| SpendDailyPoint { day: point.day.clone(), cost_usd: point.estimated_cost_usd, total_tokens: Some(point.total_tokens) }).collect();
            NativeSpendData { projects: snapshot.projects, conversations: snapshot.sessions, project_source_status: Some(snapshot.source_status), activity, daily }
        }
        Err(_) => NativeSpendData { projects: Vec::new(), conversations: Vec::new(), project_source_status: None, activity: Vec::new(), daily: daily_points(provider_id, history_days) },
    }
}

fn resolve_spend(
    native_cost: Option<f64>, native_coverage: CostCoverageCounts, native_token_mix: SpendTokenMix,
    native_models: Vec<SpendModelRow>, native_daily: Vec<SpendDailyPoint>, native_activity: Vec<SpendActivityCell>,
    imported: Option<&ImportedSpendSource>, replace_native: bool,
) -> ResolvedSpendData {
    match imported {
        Some(imported) if replace_native => ResolvedSpendData {
            known_cost_usd: imported.known_cost_usd, price_coverage: imported.coverage.clone(), token_mix: imported.token_mix.clone(),
            models: imported.models.clone(), daily: imported.daily.clone(), hourly_activity: imported.hourly_activity.clone(),
        },
        Some(imported) => ResolvedSpendData {
            known_cost_usd: sum_optional_cost(native_cost, imported.known_cost_usd),
            price_coverage: merge_coverage(native_coverage, &imported.coverage),
            token_mix: merge_token_mix(native_token_mix, &imported.token_mix),
            models: merge_models(native_models, &imported.models), daily: merge_daily(native_daily, &imported.daily),
            hourly_activity: merge_activity(native_activity, &imported.hourly_activity),
        },
        None => ResolvedSpendData { known_cost_usd: native_cost, price_coverage: native_coverage, token_mix: native_token_mix, models: native_models, daily: native_daily, hourly_activity: native_activity },
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
fn sum_optional_cost(left: Option<f64>, right: Option<f64>) -> Option<f64> {
    match (left, right) {
        (Some(left), Some(right)) => (left + right).is_finite().then_some(left + right),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn merge_coverage(mut left: CostCoverageCounts, right: &CostCoverageCounts) -> CostCoverageCounts {
    left.priced = left.priced.saturating_add(right.priced);
    left.unpriced = left.unpriced.saturating_add(right.unpriced);
    left.unmetered = left.unmetered.saturating_add(right.unmetered);
    left.estimated = left.estimated.saturating_add(right.estimated);
    left
}

fn merge_token_mix(mut left: SpendTokenMix, right: &SpendTokenMix) -> SpendTokenMix {
    left.input_tokens = add_optional(left.input_tokens, right.input_tokens);
    left.output_tokens = add_optional(left.output_tokens, right.output_tokens);
    left.cache_read_tokens = add_optional(left.cache_read_tokens, right.cache_read_tokens);
    left.cache_creation_tokens = add_optional(left.cache_creation_tokens, right.cache_creation_tokens);
    left.reasoning_tokens = add_optional(left.reasoning_tokens, right.reasoning_tokens);
    left
}

fn add_optional(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => left.checked_add(right),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

fn merge_models(mut left: Vec<SpendModelRow>, right: &[SpendModelRow]) -> Vec<SpendModelRow> {
    for incoming in right {
        if let Some(existing) = left.iter_mut().find(|row| row.model == incoming.model) {
            existing.cost_usd = sum_optional_cost(existing.cost_usd, incoming.cost_usd);
            existing.input_tokens = existing.input_tokens.saturating_add(incoming.input_tokens);
            existing.output_tokens = existing.output_tokens.saturating_add(incoming.output_tokens);
            existing.cache_read_tokens = existing.cache_read_tokens.saturating_add(incoming.cache_read_tokens);
            existing.total_tokens = existing.total_tokens.saturating_add(incoming.total_tokens);
            existing.custom_pricing |= incoming.custom_pricing;
        } else {
            left.push(incoming.clone());
        }
    }
    left.sort_by(|a, b| b.cost_usd.partial_cmp(&a.cost_usd).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.model.cmp(&b.model)));
    left
}

fn merge_daily(left: Vec<SpendDailyPoint>, right: &[SpendDailyPoint]) -> Vec<SpendDailyPoint> {
    let mut days: BTreeMap<String, SpendDailyPoint> = left.into_iter().map(|point| (point.day.clone(), point)).collect();
    for incoming in right {
        let entry = days.entry(incoming.day.clone()).or_insert_with(|| SpendDailyPoint {
            day: incoming.day.clone(),
            cost_usd: None,
            total_tokens: None,
        });
        entry.cost_usd = sum_optional_cost(entry.cost_usd, incoming.cost_usd);
        entry.total_tokens = add_optional(entry.total_tokens, incoming.total_tokens);
    }
    days.into_values().collect()
}

fn merge_activity(left: Vec<SpendActivityCell>, right: &[SpendActivityCell]) -> Vec<SpendActivityCell> {
    let mut cells: BTreeMap<(u8, u8), u32> = left.into_iter().map(|cell| ((cell.weekday, cell.hour), cell.conversations)).collect();
    for cell in right {
        let current = cells.get(&(cell.weekday, cell.hour)).copied().unwrap_or(0);
        cells.insert((cell.weekday, cell.hour), current.saturating_add(cell.conversations));
    }
    cells.into_iter().map(|((weekday, hour), conversations)| SpendActivityCell { weekday, hour, conversations }).collect()
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


}
