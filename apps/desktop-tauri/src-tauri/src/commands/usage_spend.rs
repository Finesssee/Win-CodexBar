//! Usage & Spend settings tab: 7-day / 30-day local cost aggregates.

use codexbar::cost_scanner::{CostScanner, CostSummary};
use codexbar::spend_contract::{
    SpendContract, build_local_spend_contract, build_local_spend_contract_from_summary,
};
use serde::Serialize;
use tauri::State;

use super::ProviderUsageSnapshot;
use crate::state::AppState;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSpendDailyPoint {
    pub day: String,
    pub amount: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSpendRow {
    pub provider_id: String,
    pub display_name: String,
    pub seven_day: Option<f64>,
    pub thirty_day: Option<f64>,
    pub seven_day_tokens: Option<u64>,
    pub thirty_day_tokens: Option<u64>,
    pub currency: String,
    pub source: String,
    /// Included in the shared Overview spend denominator.
    pub included_in_overview: bool,
    #[serde(default)]
    pub daily: Vec<UsageSpendDailyPoint>,
    /// F8 (upstream 0.48.0): true when the totals are served from a stale cache
    /// while a background re-scan rebuilds the artifact. Frontend shows a
    /// "refreshing" indicator.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub refreshing: bool,
    /// ISO 8601 timestamp of the stale snapshot (when `refreshing` is true).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale_updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSpendSummary {
    pub rows: Vec<UsageSpendRow>,
    pub contract: SpendContract,
}

#[derive(Clone)]
struct CachedUsageSpendSummary {
    key: String,
    summary: UsageSpendSummary,
}

static USAGE_SPEND_SUMMARY_CACHE: OnceLock<Mutex<Option<CachedUsageSpendSummary>>> =
    OnceLock::new();

fn usage_spend_summary_cache() -> &'static Mutex<Option<CachedUsageSpendSummary>> {
    USAGE_SPEND_SUMMARY_CACHE.get_or_init(|| Mutex::new(None))
}

#[tauri::command]
pub async fn get_usage_spend_summary(
    state: State<'_, Mutex<AppState>>,
    history_days: Option<u32>,
    force_refresh: Option<bool>,
) -> Result<UsageSpendSummary, String> {
    let cached = {
        let guard = state.lock().map_err(|e| e.to_string())?;
        guard.provider_cache.clone()
    };

    let selected_days = history_days.unwrap_or(30);
    let force_refresh = force_refresh.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        build_usage_spend_summary_cached(&cached, selected_days, force_refresh)
    })
    .await
    .map_err(|e| format!("usage spend worker failed: {e}"))?
}

#[tauri::command]
pub fn write_usage_spend_export(path: String, payload: String) -> Result<(), String> {
    const MAX_EXPORT_BYTES: usize = 8 * 1024 * 1024;
    let path = path.trim();
    if path.is_empty() {
        return Err("Export path must not be empty".to_string());
    }
    if payload.len() > MAX_EXPORT_BYTES {
        return Err("Usage & Spend export exceeds 8 MiB".to_string());
    }
    std::fs::write(path, payload.as_bytes()).map_err(|error| error.to_string())
}

fn build_usage_spend_summary_cached(
    cached: &[ProviderUsageSnapshot],
    selected_days: u32,
    force_refresh: bool,
) -> Result<UsageSpendSummary, String> {
    let key = usage_spend_cache_key(cached, selected_days);
    let mut guard = usage_spend_summary_cache()
        .lock()
        .map_err(|error| error.to_string())?;
    if !force_refresh
        && let Some(existing) = guard.as_ref()
        && existing.key == key
    {
        return Ok(existing.summary.clone());
    }
    // Hold the cache mutex while building: callers for the same app revision
    // coalesce behind this single scan instead of starting parallel rescans.
    let summary = build_usage_spend_summary(cached, selected_days);
    *guard = Some(CachedUsageSpendSummary {
        key,
        summary: summary.clone(),
    });
    Ok(summary)
}

fn usage_spend_cache_key(cached: &[ProviderUsageSnapshot], selected_days: u32) -> String {
    let settings = codexbar::settings::Settings::load();
    let mut revisions: Vec<String> = cached
        .iter()
        .map(|snapshot| {
            let cost = snapshot
                .cost
                .as_ref()
                .map(|cost| format!("{:.8}:{}", cost.used, cost.period))
                .unwrap_or_default();
            format!(
                "{}:{}:{}:{}",
                snapshot.provider_id, snapshot.updated_at, snapshot.source_label, cost
            )
        })
        .collect();
    revisions.sort();
    format!(
        "{}|{}|{}|{}|{}",
        chrono::Local::now().date_naive(),
        selected_days,
        settings.open_codex_usage_logs_enabled,
        settings.hide_native_codex_cost_when_open_codex_present,
        revisions.join(";")
    )
}

fn build_usage_spend_summary(
    cached: &[ProviderUsageSnapshot],
    selected_days: u32,
) -> UsageSpendSummary {
    let settings = codexbar::settings::Settings::load();
    let include_opencodex = settings.open_codex_usage_logs_enabled;
    let hide_native = settings.hide_native_codex_cost_when_open_codex_present;

    let codex_cache =
        codexbar::core::JsonlScanner::load_cache(codexbar::core::ProviderId::Codex, None);
    let codex_stale = !codex_cache.days.is_empty() && codex_cache.previous_report.is_some();
    let codex_stale_updated_at = codex_stale
        .then(|| {
            codex_cache
                .previous_report
                .as_ref()
                .and_then(|r| r.updated_at.clone())
        })
        .flatten();

    // Scan the local-log sources once per canonical window; all downstream
    // surfaces consume the same immutable row catalog built below.
    let codex_7_summary = CostScanner::new(7).scan_codex();
    let codex_30_summary = CostScanner::new(30).scan_codex();
    let claude_7_summary = CostScanner::new(7).scan_claude();
    let claude_30_summary = CostScanner::new(30).scan_claude();

    let codex_7_contract = build_local_spend_contract_from_summary(
        "codex",
        7,
        include_opencodex,
        hide_native,
        codex_7_summary.clone(),
    );
    let codex_30_contract = build_local_spend_contract_from_summary(
        "codex",
        30,
        include_opencodex,
        hide_native,
        codex_30_summary.clone(),
    );

    let mut provider_ids: BTreeSet<String> = settings.enabled_providers.iter().cloned().collect();
    provider_ids.extend(cached.iter().map(|snapshot| snapshot.provider_id.clone()));
    if include_opencodex {
        // OpenCodex is an enrichment source, never a standalone provider row.
        // Publish routed subscriptions even when no live provider snapshot exists.
        for id in ["codex", "opencodego", "kimi", "deepseek"] {
            let contract = match id {
                "codex" => None,
                _ => Some(build_local_spend_contract(id, 30, true)),
            };
            if contract
                .as_ref()
                .is_some_and(|contract| !contract.imports.is_empty())
            {
                provider_ids.insert(id.to_string());
            }
        }
    }

    let cached_by_id: HashMap<&str, &ProviderUsageSnapshot> = cached
        .iter()
        .map(|snapshot| (snapshot.provider_id.as_str(), snapshot))
        .collect();

    let mut rows = Vec::new();
    for provider_id in provider_ids {
        let cached_snapshot = cached_by_id.get(provider_id.as_str()).copied();
        let display_name = cached_snapshot
            .map(|snapshot| snapshot.display_name.trim())
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .or_else(|| {
                codexbar::core::ProviderId::from_cli_name(&provider_id).map(|id| {
                    codexbar::core::instantiate_provider(id)
                        .metadata()
                        .display_name
                        .to_string()
                })
            })
            .unwrap_or_else(|| provider_id.clone());

        let (
            seven_day,
            thirty_day,
            seven_day_tokens,
            thirty_day_tokens,
            source,
            refreshing,
            stale_updated_at,
        ) = match provider_id.as_str() {
            "codex" => (
                codex_7_contract.known_cost_usd,
                codex_30_contract.known_cost_usd,
                total_token_mix(&codex_7_contract.token_mix),
                total_token_mix(&codex_30_contract.token_mix),
                if include_opencodex && !codex_30_contract.imports.is_empty() {
                    "local logs + OpenCodex".to_string()
                } else {
                    "local logs".to_string()
                },
                codex_stale,
                codex_stale_updated_at.clone(),
            ),
            "claude" => (
                Some(claude_7_summary.total_cost_usd),
                Some(claude_30_summary.total_cost_usd),
                Some(
                    claude_7_summary
                        .input_tokens
                        .saturating_add(claude_7_summary.output_tokens),
                ),
                Some(
                    claude_30_summary
                        .input_tokens
                        .saturating_add(claude_30_summary.output_tokens),
                ),
                "local logs".to_string(),
                false,
                None,
            ),
            "opencodego" | "kimi" | "deepseek" if include_opencodex => {
                let seven = build_local_spend_contract(&provider_id, 7, true);
                let thirty = build_local_spend_contract(&provider_id, 30, true);
                let imported = !thirty.imports.is_empty();
                if imported {
                    (
                        seven.known_cost_usd,
                        thirty.known_cost_usd,
                        total_token_mix(&seven.token_mix),
                        total_token_mix(&thirty.token_mix),
                        if provider_id == "opencodego" {
                            "local logs + OpenCodex".to_string()
                        } else {
                            "OpenCodex".to_string()
                        },
                        false,
                        None,
                    )
                } else {
                    cached_spend(cached_snapshot)
                }
            }
            "grok" => {
                let seven = codexbar::providers::grok::local_sessions::summarize(7);
                let thirty = codexbar::providers::grok::local_sessions::summarize(30);
                let cached = cached_spend(cached_snapshot);
                (
                    cached.0,
                    cached.1,
                    (seven.session_count > 0).then_some(seven.total_tokens),
                    (thirty.session_count > 0).then_some(thirty.total_tokens),
                    if thirty.session_count > 0 {
                        "local Grok sessions".to_string()
                    } else {
                        cached.4
                    },
                    cached.5,
                    cached.6,
                )
            }
            _ => cached_spend(cached_snapshot),
        };

        let currency = cached_snapshot
            .and_then(|snapshot| snapshot.cost.as_ref())
            .map(|cost| cost.currency_code.clone())
            .unwrap_or_else(|| "USD".to_string());
        let daily = cached_snapshot
            .and_then(|snapshot| snapshot.cost.as_ref())
            .map(|cost| {
                cost.daily
                    .iter()
                    .map(|point| UsageSpendDailyPoint {
                        day: point.day.clone(),
                        amount: point.amount,
                    })
                    .collect()
            })
            .unwrap_or_default();
        rows.push(UsageSpendRow {
            provider_id: provider_id.clone(),
            display_name,
            seven_day,
            thirty_day,
            seven_day_tokens,
            thirty_day_tokens,
            currency,
            source,
            included_in_overview: settings.enabled_providers.contains(&provider_id)
                || cached_snapshot.is_some(),
            daily,
            refreshing,
            stale_updated_at,
        });
    }

    let history_days = if selected_days == 0 {
        365
    } else {
        selected_days.clamp(1, 365)
    };
    let selected_summary: CostSummary = match history_days {
        7 => codex_7_summary,
        30 => codex_30_summary,
        days => CostScanner::new(days).scan_codex(),
    };
    let contract = build_local_spend_contract_from_summary(
        "codex",
        history_days,
        include_opencodex,
        hide_native,
        selected_summary,
    );
    UsageSpendSummary { rows, contract }
}

fn total_token_mix(mix: &codexbar::spend_contract::SpendTokenMix) -> Option<u64> {
    let values = [
        mix.input_tokens,
        mix.output_tokens,
        mix.cache_creation_tokens,
    ];
    let mut saw = false;
    let mut total = 0u64;
    for value in values.into_iter().flatten() {
        saw = true;
        total = total.saturating_add(value);
    }
    saw.then_some(total)
}

fn cached_spend(
    snapshot: Option<&ProviderUsageSnapshot>,
) -> (
    Option<f64>,
    Option<f64>,
    Option<u64>,
    Option<u64>,
    String,
    bool,
    Option<String>,
) {
    let Some(snapshot) = snapshot else {
        return (
            None,
            None,
            None,
            None,
            "unavailable".to_string(),
            false,
            None,
        );
    };
    let Some(cost) = snapshot.cost.as_ref() else {
        return (
            None,
            None,
            None,
            None,
            if snapshot.error.is_some() {
                "unavailable".to_string()
            } else {
                snapshot.source_label.clone()
            },
            false,
            None,
        );
    };
    let period = cost.period.trim();
    let period_lower = period.to_ascii_lowercase();
    let (seven_day, thirty_day) = if cost.daily.is_empty() {
        (
            None,
            (period_lower.contains("30 day") || period_lower.contains("30-day"))
                .then_some(cost.used),
        )
    } else {
        let today = chrono::Utc::now().date_naive();
        let seven_cutoff = today - chrono::Duration::days(6);
        let mut seven = 0.0;
        let mut thirty = 0.0;
        let mut saw_seven = false;
        let mut saw_thirty = false;
        for point in &cost.daily {
            let Ok(day) = chrono::NaiveDate::parse_from_str(&point.day, "%Y-%m-%d") else {
                continue;
            };
            if day > today {
                continue;
            }
            thirty += point.amount;
            saw_thirty = true;
            if day >= seven_cutoff {
                seven += point.amount;
                saw_seven = true;
            }
        }
        (saw_seven.then_some(seven), saw_thirty.then_some(thirty))
    };
    (
        seven_day,
        thirty_day,
        None,
        None,
        if period.is_empty() {
            snapshot.source_label.clone()
        } else {
            format!("period ({period})")
        },
        false,
        None,
    )
}
