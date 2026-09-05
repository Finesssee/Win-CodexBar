//! Provider chart data commands and DTOs.
//!
//! Cost history comes from the shared JSONL cost scanner and is available for
//! every provider. Credits history + usage breakdowns currently only apply to
//! the Codex / OpenAI dashboard cache and require an `account_email` to scope
//! reads to the right cached bundle.

use codexbar::core::OpenAIDashboardCacheStore;
use codexbar::cost_scanner::{
    CostScanner, CostSummary, get_daily_cost_history, get_daily_token_history,
};
use codexbar::locale::{self, LocaleKey};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const LOCAL_USAGE_TTL: Duration = Duration::from_secs(30);

/// A single (date, value) point for cost or credits history charts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyCostPoint {
    pub date: String,
    pub value: Option<f64>,
}

/// A single (date, tokens) point for the Tokens chart mode (upstream 0.50.0
/// #2930 — exact local token totals).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyTokenPoint {
    pub date: String,
    pub tokens: u64,
}

/// A single service's usage within a day for the stacked usage breakdown chart.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceUsagePoint {
    pub service: String,
    pub credits_used: f64,
}

/// One day's stacked usage breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyUsageBreakdown {
    pub day: String,
    pub services: Vec<ServiceUsagePoint>,
    pub total_credits_used: f64,
}

/// Real local usage summary from Codex / Claude log files.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderLocalUsageSummary {
    pub today_cost: Option<f64>,
    pub thirty_day_cost: Option<f64>,
    pub thirty_day_tokens: Option<u64>,
    pub latest_tokens: Option<u64>,
    pub top_model: Option<String>,
    pub estimate_note: String,
    pub token_cost_updated_at_ms: i64,
}

/// Full chart data bundle for one provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderChartData {
    pub provider_id: String,
    pub cost_history: Vec<DailyCostPoint>,
    pub credits_history: Vec<DailyCostPoint>,
    pub usage_breakdown: Vec<DailyUsageBreakdown>,
    pub local_usage: Option<ProviderLocalUsageSummary>,
    /// Daily exact local token totals for the Tokens mode; incomplete
    /// backfill keeps the marker true so the UI can show "Refreshing".
    pub tokens_history: Vec<DailyTokenPoint>,
    pub tokens_incomplete: bool,
}

#[tauri::command]
pub async fn get_provider_chart_data(
    provider_id: String,
    account_email: Option<String>,
) -> ProviderChartData {
    let fallback_provider_id = provider_id.clone();
    let cancel = register_chart_scan(&provider_id);
    tauri::async_runtime::spawn_blocking(move || {
        build_provider_chart_data_with_cancel(provider_id, account_email, Some(cancel))
    })
    .await
    .unwrap_or_else(|err| {
        tracing::warn!("Provider chart data worker failed: {}", err);
        ProviderChartData::empty(fallback_provider_id)
    })
}

#[tauri::command]
pub async fn get_provider_local_usage_summary(
    provider_id: String,
) -> Option<ProviderLocalUsageSummary> {
    let failure_provider_id = provider_id.clone();
    tauri::async_runtime::spawn_blocking(move || load_provider_local_usage_summary(&provider_id))
        .await
        .unwrap_or_else(|err| {
            tracing::warn!("Provider local usage worker failed: {}", err);
            record_local_usage_fetch_failure(&failure_provider_id, CostFetchFailure::Failed);
            None
        })
}

#[cfg(test)]
pub(crate) fn build_provider_chart_data(
    provider_id: String,
    account_email: Option<String>,
) -> ProviderChartData {
    build_provider_chart_data_with_cancel(provider_id, account_email, None)
}

fn build_provider_chart_data_with_cancel(
    provider_id: String,
    account_email: Option<String>,
    cancel: Option<Arc<AtomicBool>>,
) -> ProviderChartData {
    let raw_cost = get_daily_cost_history(&provider_id, 30);
    let cost_history: Vec<DailyCostPoint> = raw_cost
        .into_iter()
        .map(|(date, value)| DailyCostPoint { date, value })
        .collect();

    let (raw_tokens, tokens_incomplete) = get_daily_token_history(&provider_id, 30);
    let tokens_history: Vec<DailyTokenPoint> = raw_tokens
        .into_iter()
        .map(|(date, tokens)| DailyTokenPoint { date, tokens })
        .collect();

    let (credits_history, usage_breakdown) =
        load_openai_dashboard_chart_data(&provider_id, account_email.as_deref());
    let local_usage = if cancel
        .as_deref()
        .is_some_and(|flag| flag.load(Ordering::Relaxed))
    {
        None
    } else {
        load_local_usage_summary_cached(&provider_id, cancel.as_deref())
    };

    ProviderChartData {
        provider_id,
        cost_history,
        credits_history,
        usage_breakdown,
        local_usage,
        tokens_history,
        tokens_incomplete,
    }
}

impl ProviderChartData {
    fn empty(provider_id: String) -> Self {
        Self {
            provider_id,
            cost_history: Vec::new(),
            credits_history: Vec::new(),
            usage_breakdown: Vec::new(),
            local_usage: None,
            tokens_history: Vec::new(),
            tokens_incomplete: false,
        }
    }
}

fn active_chart_scans() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    static ACTIVE: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_chart_scan(provider_id: &str) -> Arc<AtomicBool> {
    let next = Arc::new(AtomicBool::new(false));
    if let Ok(mut active) = active_chart_scans().lock()
        && let Some(previous) = active.insert(provider_id.to_string(), next.clone())
    {
        previous.store(true, Ordering::Relaxed);
    }
    next
}

fn load_local_usage_summary(
    provider_id: &str,
    cancel: Option<&AtomicBool>,
) -> Option<ProviderLocalUsageSummary> {
    load_local_usage_summary_with_unknown_models(provider_id, cancel).0
}

fn load_local_usage_summary_with_unknown_models(
    provider_id: &str,
    cancel: Option<&AtomicBool>,
) -> (Option<ProviderLocalUsageSummary>, HashSet<String>) {
    let Some(thirty_day) = scan_local_cost(provider_id, 30, cancel) else {
        return (None, HashSet::new());
    };
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return (None, HashSet::new());
    }
    let today = scan_local_cost(provider_id, 1, cancel).unwrap_or_default();
    let unknown_models = thirty_day
        .unknown_models
        .union(&today.unknown_models)
        .cloned()
        .collect();

    let thirty_day_tokens = total_tokens(&thirty_day);
    let latest_tokens = total_tokens(&today);
    let has_usage =
        thirty_day.sessions_count > 0 || thirty_day.total_cost_usd > 0.0 || thirty_day_tokens > 0;
    if !has_usage {
        return (None, unknown_models);
    }

    let lang = locale::current_language();
    (
        Some(ProviderLocalUsageSummary {
            today_cost: non_zero_f64(today.total_cost_usd),
            thirty_day_cost: non_zero_f64(thirty_day.total_cost_usd),
            thirty_day_tokens: non_zero_u64(thirty_day_tokens),
            latest_tokens: non_zero_u64(latest_tokens),
            top_model: top_model(&thirty_day),
            estimate_note: localized_estimate_note(provider_id, lang),
            token_cost_updated_at_ms: current_unix_ms(),
        }),
        unknown_models,
    )
}

pub(crate) fn load_provider_local_usage_summary(
    provider_id: &str,
) -> Option<ProviderLocalUsageSummary> {
    load_local_usage_summary_cached(provider_id, None)
}

struct CachedLocalUsage {
    loaded_at: Instant,
    summary: Option<ProviderLocalUsageSummary>,
}

fn local_usage_cache() -> &'static Mutex<HashMap<String, CachedLocalUsage>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedLocalUsage>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn clear_provider_local_usage_cache() {
    if let Ok(mut guard) = local_usage_cache().lock() {
        guard.clear();
    }
}

pub(crate) fn cached_provider_local_usage_summary(
    provider_id: &str,
) -> Option<ProviderLocalUsageSummary> {
    let Ok(guard) = local_usage_cache().lock() else {
        return None;
    };
    guard
        .get(provider_id)
        .and_then(|entry| entry.summary.clone())
}

pub(crate) async fn refresh_provider_local_usage_cache(provider_ids: Vec<String>) {
    if provider_ids.is_empty() {
        return;
    }

    let failure_provider_ids = provider_ids.clone();
    let scans = match tauri::async_runtime::spawn_blocking(move || {
        provider_ids
            .into_iter()
            .map(|provider_id| {
                let (summary, unknown_models) =
                    load_local_usage_summary_with_unknown_models(&provider_id, None);
                (provider_id, summary, unknown_models)
            })
            .collect::<Vec<_>>()
    })
    .await
    {
        Ok(scans) => scans,
        Err(err) => {
            tracing::warn!("Provider local usage refresh worker failed: {err}");
            for provider_id in failure_provider_ids {
                record_local_usage_fetch_failure(&provider_id, CostFetchFailure::Failed);
            }
            return;
        }
    };

    for (provider_id, mut summary, unknown_models) in scans {
        let pricing_provider = match provider_id.as_str() {
            "codex" => Some("openai"),
            "claude" => Some("anthropic"),
            _ => None,
        };
        if let Some(pricing_provider) = pricing_provider
            && codexbar::core::refresh_unknown_models_if_needed(pricing_provider, &unknown_models)
                .await
        {
            let rescan_provider = provider_id.clone();
            summary = tauri::async_runtime::spawn_blocking(move || {
                load_local_usage_summary(&rescan_provider, None)
            })
            .await
            .unwrap_or(summary);
        }
        store_local_usage_summary(&provider_id, summary);
    }
}

#[cfg(test)]
pub(crate) fn cache_provider_local_usage_summary_for_test(
    provider_id: &str,
    summary: Option<ProviderLocalUsageSummary>,
) {
    store_local_usage_summary(provider_id, summary);
}

fn load_local_usage_summary_cached(
    provider_id: &str,
    cancel: Option<&AtomicBool>,
) -> Option<ProviderLocalUsageSummary> {
    let cache = local_usage_cache();
    if let Ok(guard) = cache.lock()
        && let Some(entry) = guard.get(provider_id)
        && token_cost_cache_is_fresh(Some(entry.loaded_at), Instant::now(), LOCAL_USAGE_TTL)
    {
        return entry.summary.clone();
    }

    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return None;
    }

    let summary = load_local_usage_summary(provider_id, cancel);
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return None;
    }

    store_local_usage_summary(provider_id, summary.clone());
    summary
}

fn store_local_usage_summary(provider_id: &str, summary: Option<ProviderLocalUsageSummary>) {
    if let Ok(mut guard) = local_usage_cache().lock() {
        guard.insert(
            provider_id.to_string(),
            CachedLocalUsage {
                loaded_at: Instant::now(),
                summary,
            },
        );
    }
}

fn record_local_usage_fetch_failure(provider_id: &str, failure: CostFetchFailure) {
    let loaded_at = if cost_fetch_failure_allows_early_retry(failure) {
        Instant::now() - LOCAL_USAGE_TTL - Duration::from_secs(1)
    } else {
        Instant::now()
    };
    if let Ok(mut guard) = local_usage_cache().lock() {
        guard.insert(
            provider_id.to_string(),
            CachedLocalUsage {
                loaded_at,
                summary: None,
            },
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "chart command helper reserved for future dashboard integration"
)]
pub(crate) enum CostFetchFailure {
    Failed,
    TimedOut,
}

pub(crate) fn token_cost_cache_is_fresh(
    loaded_at: Option<Instant>,
    now: Instant,
    ttl: Duration,
) -> bool {
    loaded_at
        .and_then(|loaded| now.checked_duration_since(loaded))
        .map(|age| age <= ttl)
        .unwrap_or(false)
}

pub(crate) fn cost_fetch_failure_allows_early_retry(failure: CostFetchFailure) -> bool {
    !matches!(failure, CostFetchFailure::TimedOut)
}

fn current_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn localized_estimate_note(provider_id: &str, lang: codexbar::settings::Language) -> String {
    match provider_id {
        "claude" => locale::get_text(lang, LocaleKey::PanelEstimatedFromLocalLogsClaude),
        _ => locale::get_text(lang, LocaleKey::PanelEstimatedFromLocalLogs),
    }
}

fn scan_local_cost(
    provider_id: &str,
    days: u32,
    cancel: Option<&AtomicBool>,
) -> Option<CostSummary> {
    let scanner = CostScanner::new(days);
    match provider_id {
        "codex" => Some(scanner.scan_codex_with_cancel(cancel)),
        "claude" => Some(scanner.scan_claude_with_cancel(cancel)),
        "opencodego" => Some(scanner.scan_opencodego_with_cancel(cancel)),
        _ => None,
    }
}

fn total_tokens(summary: &CostSummary) -> u64 {
    summary.input_tokens + summary.output_tokens
}

fn non_zero_f64(value: f64) -> Option<f64> {
    (value > 0.0).then_some(value)
}

fn non_zero_u64(value: u64) -> Option<u64> {
    (value > 0).then_some(value)
}

fn top_model(summary: &CostSummary) -> Option<String> {
    summary
        .by_model_tokens
        .iter()
        .max_by_key(|(_, counts)| counts.total())
        .map(|(model, _)| model.clone())
        .or_else(|| {
            summary
                .by_model
                .iter()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(model, _)| model.clone())
        })
}

fn load_openai_dashboard_chart_data(
    provider_id: &str,
    account_email: Option<&str>,
) -> (Vec<DailyCostPoint>, Vec<DailyUsageBreakdown>) {
    if provider_id != "codex" && provider_id != "openai" {
        return (Vec::new(), Vec::new());
    }

    let Some(account_email) = account_email else {
        return (Vec::new(), Vec::new());
    };

    let Some(cache) = OpenAIDashboardCacheStore::load() else {
        return (Vec::new(), Vec::new());
    };

    if !cache.account_email.eq_ignore_ascii_case(account_email) {
        return (Vec::new(), Vec::new());
    }

    let snapshot = &cache.snapshot;

    let breakdown_source = if !snapshot.daily_breakdown.is_empty() {
        &snapshot.daily_breakdown
    } else if !snapshot.usage_breakdown.is_empty() {
        &snapshot.usage_breakdown
    } else {
        return (Vec::new(), Vec::new());
    };

    let credits_history: Vec<DailyCostPoint> = breakdown_source
        .iter()
        .map(|d| DailyCostPoint {
            date: d.day.clone(),
            value: Some(d.total_credits_used),
        })
        .collect();

    let usage_breakdown: Vec<DailyUsageBreakdown> = snapshot
        .usage_breakdown
        .iter()
        .map(|d| DailyUsageBreakdown {
            day: d.day.clone(),
            services: d
                .services
                .iter()
                .map(|s| ServiceUsagePoint {
                    service: s.service.clone(),
                    credits_used: s.credits_used,
                })
                .collect(),
            total_credits_used: d.total_credits_used,
        })
        .collect();

    (credits_history, usage_breakdown)
}

#[cfg(test)]
mod tests {
    use super::{
        CostFetchFailure, ProviderLocalUsageSummary, cost_fetch_failure_allows_early_retry,
        localized_estimate_note, token_cost_cache_is_fresh,
    };
    use crate::commands::is_provider_cache_fresh;
    use codexbar::settings::Language;
    use std::time::{Duration, Instant};

    #[test]
    fn token_cost_age_does_not_use_provider_quota_age() {
        let now = Instant::now();
        let token_loaded = now - Duration::from_secs(31);
        let provider_updated = now;
        assert!(!token_cost_cache_is_fresh(
            Some(token_loaded),
            now,
            Duration::from_secs(30)
        ));
        assert!(is_provider_cache_fresh(
            Some(provider_updated),
            Duration::from_secs(30)
        ));
    }

    #[test]
    fn fast_cost_failures_allow_the_next_pass_to_retry() {
        assert!(cost_fetch_failure_allows_early_retry(
            CostFetchFailure::Failed
        ));
        assert!(!cost_fetch_failure_allows_early_retry(
            CostFetchFailure::TimedOut
        ));
    }

    #[test]
    fn local_usage_summary_serializes_token_cost_timestamp() {
        let summary = ProviderLocalUsageSummary {
            today_cost: Some(1.0),
            thirty_day_cost: Some(2.0),
            thirty_day_tokens: Some(300),
            latest_tokens: Some(40),
            top_model: Some("gpt-5".to_string()),
            estimate_note: "estimated".to_string(),
            token_cost_updated_at_ms: 1234,
        };

        let json = serde_json::to_value(summary).expect("serialize summary");
        assert_eq!(
            json.get("tokenCostUpdatedAtMs").and_then(|v| v.as_i64()),
            Some(1234)
        );
    }

    #[test]
    fn japanese_estimate_note_is_localized() {
        assert_eq!(
            localized_estimate_note("codex", Language::Japanese),
            "ローカルログから推定したもので、請求書と異なる場合があります"
        );
        assert_eq!(
            localized_estimate_note("claude", Language::Japanese),
            "ClaudeのローカルログからAPIレートで推定したもので、トークン総数が請求書と異なる場合があります"
        );
    }

    #[test]
    fn english_estimate_note_is_localized() {
        assert_eq!(
            localized_estimate_note("codex", Language::English),
            "Estimated from local logs; may differ from your bill"
        );
        assert_eq!(
            localized_estimate_note("claude", Language::English),
            "Estimated from local Claude logs at API rates; token totals may differ from your bill"
        );
    }
}
