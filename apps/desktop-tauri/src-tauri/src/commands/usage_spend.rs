//! Usage & Spend settings tab: 7-day / 30-day local cost aggregates.

use codexbar::cost_scanner::{CostScanner, CostSummary};
use codexbar::spend_contract::{SpendContract, build_local_spend_contract_from_summary};
use serde::Serialize;
use tauri::State;

use super::ProviderUsageSnapshot;
use crate::state::AppState;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSpendRow {
    pub provider_id: String,
    pub display_name: String,
    pub seven_day: Option<f64>,
    pub thirty_day: Option<f64>,
    pub currency: String,
    pub source: String,
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

#[tauri::command]
pub async fn get_usage_spend_summary(
    state: State<'_, Mutex<AppState>>,
    history_days: Option<u32>,
) -> Result<UsageSpendSummary, String> {
    let cached = {
        let guard = state.lock().map_err(|e| e.to_string())?;
        guard.provider_cache.clone()
    };

    let selected_days = history_days.unwrap_or(30);
    tauri::async_runtime::spawn_blocking(move || build_usage_spend_summary(&cached, selected_days))
        .await
        .map_err(|e| format!("usage spend worker failed: {e}"))
}

fn build_usage_spend_summary(cached: &[ProviderUsageSnapshot], selected_days: u32) -> UsageSpendSummary {
    let codex_cache = codexbar::core::JsonlScanner::load_cache(codexbar::core::ProviderId::Codex, None);
    let codex_stale = !codex_cache.days.is_empty() && codex_cache.previous_report.is_some();
    let codex_stale_updated_at = codex_stale.then(|| codex_cache.previous_report.as_ref().and_then(|r| r.updated_at.clone())).flatten();

    let codex_7_summary = CostScanner::new(7).scan_codex();
    let codex_30_summary = CostScanner::new(30).scan_codex();
    let claude_7_summary = CostScanner::new(7).scan_claude();
    let claude_30_summary = CostScanner::new(30).scan_claude();

    let mut rows = vec![
        UsageSpendRow {
            provider_id: "codex".into(), display_name: "Codex".into(),
            seven_day: Some(codex_7_summary.total_cost_usd), thirty_day: Some(codex_30_summary.total_cost_usd),
            currency: "USD".into(), source: "local logs".into(), refreshing: codex_stale, stale_updated_at: codex_stale_updated_at,
        },
        UsageSpendRow {
            provider_id: "claude".into(), display_name: "Claude".into(),
            seven_day: Some(claude_7_summary.total_cost_usd), thirty_day: Some(claude_30_summary.total_cost_usd),
            currency: "USD".into(), source: "local logs".into(), refreshing: false, stale_updated_at: None,
        },
    ];

    for snapshot in cached {
        if snapshot.provider_id == "codex" || snapshot.provider_id == "claude" { continue; }
        let Some(cost) = &snapshot.cost else { continue; };
        rows.push(UsageSpendRow {
            provider_id: snapshot.provider_id.clone(),
            display_name: if snapshot.display_name.is_empty() { snapshot.provider_id.clone() } else { snapshot.display_name.clone() },
            refreshing: false, stale_updated_at: None, seven_day: None, thirty_day: None,
            currency: cost.currency_code.clone(), source: format!("period ({})", cost.period),
        });
    }

    let history_days = if selected_days == 0 { 365 } else { selected_days.clamp(1, 365) };
    let selected_summary: CostSummary = match history_days {
        7 => codex_7_summary,
        30 => codex_30_summary,
        days => CostScanner::new(days).scan_codex(),
    };
    let settings = codexbar::settings::Settings::load();
    let contract = build_local_spend_contract_from_summary(
        "codex", history_days, settings.open_codex_usage_logs_enabled, false, selected_summary,
    );
    UsageSpendSummary { rows, contract }
}
