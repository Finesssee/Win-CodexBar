//! Codex local-log cost aggregation helpers.

#[cfg(test)]
use chrono::Local;
use chrono::{Duration, NaiveDate};
use std::path::Path;

use crate::core::{CodexUsageRecord, CostUsageDayRange, CostUsagePricing, JsonlScanner};
use crate::cost_scanner::{CostSummary, ModelTokenCounts};

pub(crate) fn codex_period_start(today: NaiveDate, days: u32) -> NaiveDate {
    today - Duration::days(days.saturating_sub(1) as i64)
}

pub(crate) fn codex_scan_dates(range: &CostUsageDayRange) -> Vec<NaiveDate> {
    let Some(mut date) = CostUsageDayRange::parse_day_key(&range.scan_since_key) else {
        return Vec::new();
    };
    let Some(until) = CostUsageDayRange::parse_day_key(&range.scan_until_key) else {
        return Vec::new();
    };
    let mut dates = Vec::new();
    while date <= until {
        dates.push(date);
        date += Duration::days(1);
    }
    dates
}

pub(crate) fn add_codex_records_to_summary(
    summary: &mut CostSummary,
    records: &[CodexUsageRecord],
    range: &CostUsageDayRange,
) -> (f64, bool) {
    let mut total_cost = 0.0;
    let mut has_tokens = false;

    for record in records.iter().filter(|record| {
        CostUsageDayRange::is_in_range(&record.day_key, &range.since_key, &range.until_key)
    }) {
        let tokens = CodexTokenCounts::from_values(record.input, record.cached, record.output);
        if let Some(cost) = add_codex_tokens_to_summary(summary, &record.model, tokens) {
            total_cost += cost;
            has_tokens = true;
        }
    }

    (total_cost, has_tokens)
}

/// Merge billable records into a day→model→`[input,cached,output]` map.
pub(crate) fn merge_codex_records_into_days(
    days: &mut std::collections::HashMap<String, std::collections::HashMap<String, Vec<i32>>>,
    records: &[CodexUsageRecord],
) {
    for record in records {
        let models = days.entry(record.day_key.clone()).or_default();
        let packed = models
            .entry(record.model.clone())
            .or_insert_with(|| vec![0, 0, 0]);
        if packed.len() < 3 {
            packed.resize(3, 0);
        }
        packed[0] = packed[0].saturating_add(record.input.max(0));
        packed[1] = packed[1].saturating_add(record.cached.max(0));
        packed[2] = packed[2].saturating_add(record.output.max(0));
    }
}

/// Apply one packed `[input, cached, output]` triple to a summary.
pub(crate) fn add_codex_packed_tokens_to_summary(
    summary: &mut CostSummary,
    model: &str,
    packed: &[i32],
) -> Option<f64> {
    let input = packed.first().copied().unwrap_or(0);
    let cached = packed.get(1).copied().unwrap_or(0);
    let output = packed.get(2).copied().unwrap_or(0);
    add_codex_tokens_to_summary(
        summary,
        model,
        CodexTokenCounts::from_values(input, cached, output),
    )
}

/// Fold day→model→packed token maps into a cost summary (range-filtered).
/// Returns `(session_cost, has_tokens)` — caller adds cost to `total_cost_usd`.
pub(crate) fn add_codex_days_map_to_summary(
    summary: &mut CostSummary,
    days: &std::collections::HashMap<String, std::collections::HashMap<String, Vec<i32>>>,
    range: &CostUsageDayRange,
) -> (f64, bool) {
    let mut total_cost = 0.0;
    let mut has_tokens = false;
    for (day_key, models) in days {
        if !CostUsageDayRange::is_in_range(day_key, &range.since_key, &range.until_key) {
            continue;
        }
        for (model, packed) in models {
            if let Some(cost) = add_codex_packed_tokens_to_summary(summary, model, packed) {
                total_cost += cost;
                has_tokens = true;
            }
        }
    }
    (total_cost, has_tokens)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn scan_codex_file_cost_for_range(path: &Path, range: &CostUsageDayRange) -> f64 {
    let parse_result = match JsonlScanner::parse_codex_file(path, range, 0, None, None) {
        Ok(result) => result,
        Err(_) => return 0.0,
    };

    codex_records_cost(&parse_result.records, range)
}

#[cfg(test)]
pub(crate) fn scan_codex_file_cost(path: &Path) -> f64 {
    let today = Local::now().date_naive();
    let range = CostUsageDayRange::new(codex_period_start(today, 30), today);
    scan_codex_file_cost_for_range(path, &range)
}

#[derive(Clone, Copy)]
struct CodexTokenCounts {
    input: u64,
    cached: u64,
    output: u64,
}

impl CodexTokenCounts {
    fn from_values(input: i32, cached: i32, output: i32) -> Self {
        let input = input.max(0) as u64;
        Self {
            input,
            cached: (cached.max(0) as u64).min(input),
            output: output.max(0) as u64,
        }
    }

    fn is_empty(self) -> bool {
        self.input == 0 && self.cached == 0 && self.output == 0
    }
}

fn add_tokens(summary: &mut ModelTokenCounts, tokens: CodexTokenCounts) {
    summary.input_tokens += tokens.input;
    summary.output_tokens += tokens.output;
    summary.cached_tokens += tokens.cached;
}

fn add_codex_tokens_to_summary(
    summary: &mut CostSummary,
    model: &str,
    tokens: CodexTokenCounts,
) -> Option<f64> {
    if tokens.is_empty() {
        return None;
    }

    let model_key = if CostUsagePricing::is_codex_unattributed_model(model) {
        CostUsagePricing::CODEX_UNATTRIBUTED_MODEL.to_string()
    } else {
        model.to_string()
    };

    // Unattributed usage is visible but never priced and must not trigger a
    // models.dev catalog refresh (it is deliberately unpriced, not "unknown yet").
    if CostUsagePricing::is_codex_unattributed_model(&model_key) {
        summary.input_tokens += tokens.input;
        summary.cached_tokens += tokens.cached;
        summary.output_tokens += tokens.output;
        summary.by_model.entry(model_key.clone()).or_insert(0.0);
        add_tokens(
            summary.by_model_tokens.entry(model_key).or_default(),
            tokens,
        );
        return Some(0.0);
    }

    let uses_fallback_pricing =
        CostUsagePricing::codex_cost_usd(&model_key, tokens.input, tokens.cached, tokens.output)
            .is_none();
    let cost = codex_cost_usd(&model_key, tokens.input, tokens.cached, tokens.output);
    if uses_fallback_pricing {
        summary.unknown_models.insert(model_key.clone());
    }

    summary.input_tokens += tokens.input;
    summary.cached_tokens += tokens.cached;
    summary.output_tokens += tokens.output;
    *summary.by_model.entry(model_key.clone()).or_insert(0.0) += cost;

    let speed_bucket = codex_speed_bucket(&model_key);
    *summary
        .by_speed
        .entry(speed_bucket.to_string())
        .or_insert(0.0) += cost;
    add_tokens(
        summary.by_model_tokens.entry(model_key).or_default(),
        tokens,
    );
    add_tokens(
        summary
            .by_speed_tokens
            .entry(speed_bucket.to_string())
            .or_default(),
        tokens,
    );
    Some(cost)
}

#[cfg_attr(not(test), allow(dead_code))]
fn codex_records_cost(records: &[CodexUsageRecord], range: &CostUsageDayRange) -> f64 {
    let mut total_cost = 0.0;

    for record in records.iter().filter(|record| {
        CostUsageDayRange::is_in_range(&record.day_key, &range.since_key, &range.until_key)
    }) {
        if CostUsagePricing::is_codex_unattributed_model(&record.model) {
            continue;
        }
        let tokens = CodexTokenCounts::from_values(record.input, record.cached, record.output);
        if !tokens.is_empty() {
            total_cost += codex_cost_usd(&record.model, tokens.input, tokens.cached, tokens.output);
        }
    }

    total_cost
}

fn codex_speed_bucket(model: &str) -> &'static str {
    let normalized = model.to_ascii_lowercase();
    if normalized.contains("fast")
        || normalized.contains("priority")
        || normalized.contains("spark")
        || normalized.contains("smoke")
    {
        "fast"
    } else {
        "standard"
    }
}

fn codex_cost_usd(model: &str, input: u64, cached: u64, output: u64) -> f64 {
    if CostUsagePricing::is_codex_unattributed_model(model) {
        return 0.0;
    }
    if let Some(cost) = CostUsagePricing::codex_cost_usd(model, input, cached, output) {
        return cost;
    }

    let (input_price, cached_price, output_price) = match model.to_lowercase().as_str() {
        m if m.contains("gpt-4o-mini") => (0.15, 0.075, 0.60),
        m if m.contains("gpt-4o") => (2.50, 1.25, 10.00),
        m if m.contains("gpt-4-turbo") => (10.00, 5.00, 30.00),
        m if m.contains("gpt-4") => (30.00, 15.00, 60.00),
        m if m.contains("o1-mini") => (3.00, 1.50, 12.00),
        m if m.contains("o1") => (15.00, 7.50, 60.00),
        _ => (2.50, 1.25, 10.00),
    };

    let cached = cached.min(input);
    let non_cached = input.saturating_sub(cached);
    let input_cost = (non_cached as f64 / 1_000_000.0) * input_price;
    let cached_cost = (cached as f64 / 1_000_000.0) * cached_price;
    let output_cost = (output as f64 / 1_000_000.0) * output_price;

    input_cost + cached_cost + output_cost
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::CodexUsageRecord;

    #[test]
    fn test_codex_pricing() {
        // Test GPT-4o pricing: $2.50/1M input, $10/1M output
        let cost = codex_cost_usd("gpt-4o", 1_000_000, 0, 1_000_000);
        assert!((cost - 12.50).abs() < 0.01);
    }

    #[test]
    fn test_codex_pricing_uses_gpt55_standard_short_context_rates() {
        let cost = codex_cost_usd("gpt-5.5", 1_000_000, 400_000, 1_000_000);

        // GPT-5.5 standard short-context pricing:
        // 600k non-cached input at $5/M, 400k cached input at $0.50/M,
        // and 1M output at $30/M.
        assert!((cost - 33.20).abs() < 0.01);
    }

    #[test]
    fn codex_summary_prices_gpt56_usage_records_individually() {
        let target = NaiveDate::from_ymd_opt(2026, 5, 31).unwrap();
        let range = CostUsageDayRange::new(target, target);
        let records = vec![
            CodexUsageRecord {
                day_key: "2026-05-31".to_string(),
                model: "gpt-5.6-sol".to_string(),
                input: 200_000,
                cached: 0,
                output: 0,
            },
            CodexUsageRecord {
                day_key: "2026-05-31".to_string(),
                model: "gpt-5.6-sol".to_string(),
                input: 200_000,
                cached: 0,
                output: 0,
            },
            CodexUsageRecord {
                day_key: "2026-05-30".to_string(),
                model: "gpt-5.6-sol".to_string(),
                input: 200_000,
                cached: 0,
                output: 0,
            },
        ];
        let mut summary = CostSummary::default();

        let (cost, has_tokens) = add_codex_records_to_summary(&mut summary, &records, &range);

        assert!(has_tokens);
        assert_eq!(summary.input_tokens, 400_000);
        assert!((cost - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn model_less_codex_usage_is_visible_but_unpriced() {
        let target = NaiveDate::from_ymd_opt(2026, 5, 31).unwrap();
        let range = CostUsageDayRange::new(target, target);
        let records = vec![CodexUsageRecord {
            day_key: "2026-05-31".to_string(),
            model: CostUsagePricing::CODEX_UNATTRIBUTED_MODEL.to_string(),
            input: 55_000_000,
            cached: 0,
            output: 0,
        }];
        let mut summary = CostSummary::default();

        let (cost, has_tokens) = add_codex_records_to_summary(&mut summary, &records, &range);

        assert!(has_tokens);
        assert_eq!(cost, 0.0);
        assert_eq!(summary.input_tokens, 55_000_000);
        assert_eq!(
            summary
                .by_model
                .get(CostUsagePricing::CODEX_UNATTRIBUTED_MODEL)
                .copied(),
            Some(0.0)
        );
        assert!(summary.unknown_models.is_empty());
    }

    #[test]
    fn records_unknown_codex_model_while_using_fallback_cost() {
        let target = NaiveDate::from_ymd_opt(2026, 5, 31).unwrap();
        let range = CostUsageDayRange::new(target, target);
        let records = vec![CodexUsageRecord {
            day_key: "2026-05-31".to_string(),
            model: "gpt-mystery".to_string(),
            input: 1_000_000,
            cached: 0,
            output: 1_000_000,
        }];
        let mut summary = CostSummary::default();

        let (cost, has_tokens) = add_codex_records_to_summary(&mut summary, &records, &range);

        assert!(has_tokens);
        assert!(cost > 0.0);
        assert!(summary.unknown_models.contains("gpt-mystery"));
    }

    #[test]
    fn test_codex_speed_bucket() {
        assert_eq!(codex_speed_bucket("gpt-5.5-fast"), "fast");
        assert_eq!(codex_speed_bucket("gpt-5.3-codex-spark"), "fast");
        assert_eq!(codex_speed_bucket("gpt-5-codex"), "standard");
    }
}
