//! Persistence budgets for the Codex cost-usage cache (upstream 0.48.0 #2637/#2646/#2703).
//!
//! The on-disk cache is a single JSON document holding one entry per scanned
//! session file plus the aggregated day map. An unbounded corpus can otherwise
//! grow it to multiple gigabytes, and decoding that document on every refresh
//! materializes an object graph roughly an order of magnitude larger than the
//! artifact (upstream #2637 traced multi-GiB `MALLOC_LARGE` spikes to exactly
//! that decode). The bounds here mirror the scan-side byte budgets so the
//! artifact stays small enough to decode in one shot.
//!
//! ## Overshoot contract (upstream #2703)
//!
//! `save` bounds the artifact to [`CostUsageCacheBudget::max_file_bytes`].
//! When protected entries (partially parsed files that hold catch-up
//! progress) cannot be trimmed further, the artifact may overshoot the save
//! budget only up to [`CostUsageCacheBudget::max_load_bytes`]. Anything above
//! the load cap is a legacy or foreign artifact that is cheaper to rebuild
//! bounded than to decode in one shot, so `load` refuses it and `save` drops
//! it instead of persisting an unloadable document.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::core::{CostUsageDayRange, CostUsageFileUsage, CostUsagePricing, ProviderId};

/// Persistence budget for the Codex cost cache.
///
/// Constants mirror upstream `CostUsageCacheIO.maxCacheFileBytes` /
/// `maxCacheFileEntries` / `maxCacheLoadBytes` (256 MiB / 25 000 / 320 MiB).
/// Only Codex persistence is bounded; other providers write unbounded caches.
pub struct CostUsageCacheBudget;

impl CostUsageCacheBudget {
    /// Maximum encoded artifact size the save path targets.
    pub const MAX_FILE_BYTES: usize = 256 * 1024 * 1024;
    /// Maximum number of per-file entries the save path keeps.
    pub const MAX_FILE_ENTRIES: usize = 25_000;
    /// Maximum artifact size `load` is willing to decode. Save may overshoot
    /// [`Self::MAX_FILE_BYTES`] only up to this cap; anything larger is refused
    /// at load and dropped at save.
    pub const MAX_LOAD_BYTES: usize = 320 * 1024 * 1024;
}

/// A file entry that should not be dropped by budget pruning.
///
/// Locally every file is parsed to completion in one pass, so the only
/// protected state is a partially parsed (growing) rollout file whose
/// `parsed_bytes` is behind `size` — its cached offset is catch-up progress
/// that a later append-only resume still needs (upstream #2648). Upstream also
/// protects fork-parent lineages; the Windows port has no fork lineage state.
fn is_protected(entry: &CostUsageFileUsage) -> bool {
    entry
        .parsed_bytes
        .is_some_and(|parsed| parsed > 0 && parsed < entry.size)
}

/// Whether any of `entry`'s usage days fall inside the active scan window.
fn touches_window(entry: &CostUsageFileUsage, since_key: &str, until_key: &str) -> bool {
    entry
        .days
        .keys()
        .any(|day| CostUsageDayRange::is_in_range(day, since_key, until_key))
}

/// Cheap per-entry byte estimate (conservative overhead) so the save path can
/// decide whether to prune *before* materializing the encoded document. Mirrors
/// upstream `estimatedCodexCacheBytes`'s per-entry shape; it deliberately
/// overestimates so pruning triggers at or before the real byte budget.
fn estimated_entry_bytes(entry: &CostUsageFileUsage) -> usize {
    let mut bytes = 240;
    for (day, models) in &entry.days {
        bytes += day.len() + 32;
        for (model, packed) in models {
            bytes += model.len() + 40 + packed.len() * 10;
        }
    }
    bytes
}

/// Conservative estimate of the encoded artifact size.
pub fn estimated_cache_bytes(
    files: &HashMap<String, CostUsageFileUsage>,
    days: &HashMap<String, HashMap<String, Vec<i32>>>,
) -> usize {
    let mut bytes = 4096;
    bytes += files.len() * 160;
    for entry in files.values() {
        bytes += estimated_entry_bytes(entry);
    }
    for (day, models) in days {
        bytes += day.len() + 32;
        for (model, packed) in models {
            bytes += model.len() + 40 + packed.len() * 10;
        }
    }
    bytes
}

/// Prune out-of-window file entries from the cache to bring it under budget.
///
/// Out-of-window entries are never read by the current report (the scanner
/// filters by the active requested scan window), so dropping them — with the
/// same day-aggregate subtraction the scanner applies — keeps the artifact from
/// growing without limit. Protected (partially parsed) entries are kept so
/// append-only resume keeps making progress. Returns the removed path keys.
///
/// Mirrors upstream `pruneCodexCacheForBudget`, narrowed to the local cache
/// shape (no fork lineages, no discovery/lookback state).
pub fn prune_out_of_window_for_budget(
    files: &mut HashMap<String, CostUsageFileUsage>,
    days: &mut HashMap<String, HashMap<String, Vec<i32>>>,
    scan_since_key: Option<&str>,
    scan_until_key: Option<&str>,
    force: bool,
) -> Vec<String> {
    let Some(since_key) = scan_since_key else {
        return Vec::new();
    };
    let Some(until_key) = scan_until_key else {
        return Vec::new();
    };

    let over_entries = files.len() > CostUsageCacheBudget::MAX_FILE_ENTRIES;
    if !force && !over_entries {
        return Vec::new();
    }

    let removable: Vec<String> = files
        .iter()
        .filter_map(|(key, entry)| {
            if touches_window(entry, since_key, until_key) {
                return None;
            }
            if is_protected(entry) {
                return None;
            }
            Some(key.clone())
        })
        .collect();

    let mut removed = Vec::new();
    for key in &removable {
        if let Some(entry) = files.remove(key) {
            subtract_entry_days(days, &entry.days);
            removed.push(key.clone());
        }
    }
    removed
}

/// Last-resort budget trim: drop the oldest completed in-window entries until
/// the estimated payload fits the byte budget. At least the newest entry is
/// always kept so the artifact retains window data even when a single entry
/// alone exceeds the budget. Protected (partially parsed) entries are kept so
/// append-only resume keeps its catch-up progress (upstream #2648).
///
/// Mirrors upstream `trimInWindowEntriesForBudget`, narrowed to the local
/// cache shape.
pub fn trim_in_window_for_budget(
    files: &mut HashMap<String, CostUsageFileUsage>,
    days: &mut HashMap<String, HashMap<String, Vec<i32>>>,
    scan_since_key: Option<&str>,
    scan_until_key: Option<&str>,
    max_bytes: usize,
) -> Vec<String> {
    let Some(since_key) = scan_since_key else {
        return Vec::new();
    };
    let Some(until_key) = scan_until_key else {
        return Vec::new();
    };

    let mut droppable: Vec<String> = files
        .iter()
        .filter_map(|(key, entry)| {
            if !touches_window(entry, since_key, until_key) {
                return None;
            }
            if is_protected(entry) {
                return None;
            }
            Some(key.clone())
        })
        .collect();
    if droppable.is_empty() {
        return Vec::new();
    }

    // Drop oldest usage first so recent sessions keep their catch-up detail.
    droppable.sort_by(|a, b| {
        let a_day = files[a]
            .days
            .keys()
            .min()
            .map(String::as_str)
            .unwrap_or("9999");
        let b_day = files[b]
            .days
            .keys()
            .min()
            .map(String::as_str)
            .unwrap_or("9999");
        a_day.cmp(b_day)
    });

    let target = (max_bytes * 3) / 4;
    let mut estimate =
        estimated_cache_bytes(files, days) - estimated_entry_bytes(&files[&droppable[0]]);
    let mut dropped = Vec::new();
    for (index, key) in droppable.iter().enumerate() {
        // Always keep at least the newest entry.
        if index >= droppable.len() - 1 {
            break;
        }
        if estimate <= target {
            break;
        }
        estimate = estimate.saturating_sub(estimated_entry_bytes(&files[key]));
        dropped.push(key.clone());
    }

    let mut removed = Vec::new();
    for key in &dropped {
        if let Some(entry) = files.remove(key) {
            subtract_entry_days(days, &entry.days);
            removed.push(key.clone());
        }
    }
    removed
}

/// Subtract a per-file day map from the aggregated cache day map (the inverse
/// of the scanner's `rebuild_cache_days` accumulation), so pruned entries do
/// not inflate totals.
fn subtract_entry_days(
    days: &mut HashMap<String, HashMap<String, Vec<i32>>>,
    entry_days: &HashMap<String, HashMap<String, Vec<i32>>>,
) {
    let mut empty_days = Vec::new();
    for (day, models) in entry_days {
        let Some(day_entry) = days.get_mut(day) else {
            continue;
        };
        let mut empty_models = Vec::new();
        for (model, packed) in models {
            let Some(dest) = day_entry.get_mut(model) else {
                continue;
            };
            for (i, value) in packed.iter().take(3).enumerate() {
                if i < dest.len() {
                    dest[i] = dest[i].saturating_sub(*value);
                }
            }
            if dest.iter().all(|v| *v == 0) {
                empty_models.push(model.clone());
            }
        }
        for model in &empty_models {
            day_entry.remove(model);
        }
        if day_entry.is_empty() {
            empty_days.push(day.clone());
        }
    }
    for day in &empty_days {
        days.remove(day);
    }
}

/// File size of the on-disk cache artifact, or 0 when unreadable.
pub fn artifact_file_size(path: &Path) -> i64 {
    fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0)
}

/// True only for the Codex provider: only Codex persistence carries bounded
/// resume/discovery scan state, so only Codex is bounded on save and refused
/// on load (upstream: "Provider-specific by design").
pub fn is_bounded_provider(provider: ProviderId) -> bool {
    provider == ProviderId::Codex
}

/// Sentinel names of Codex routing rows that are deliberately unpriced and so
/// must never fall back to the bundled price table. Upstream treats
/// `codex-auto-review` (and the model-less sentinel) as cost-nil routing rows
/// so the dashboard retains priced model rows from the same history.
pub fn is_unpriced_codex_routing_model(model: &str) -> bool {
    let normalized = CostUsagePricing::normalize_codex_model(model);
    normalized == CostUsagePricing::CODEX_UNATTRIBUTED_MODEL
        || normalized.eq_ignore_ascii_case("codex-auto-review")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(days: &[&str], parsed: Option<i64>, size: i64) -> CostUsageFileUsage {
        let mut day_map: HashMap<String, HashMap<String, Vec<i32>>> = HashMap::new();
        for day in days {
            day_map.insert(
                (*day).to_string(),
                HashMap::from([("gpt-5.6-sol".to_string(), vec![10, 0, 1])]),
            );
        }
        CostUsageFileUsage {
            mtime_unix_ms: 0,
            size,
            days: day_map,
            parsed_bytes: parsed,
            last_model: None,
            last_totals: None,
        }
    }

    type TestCache = (
        HashMap<String, CostUsageFileUsage>,
        HashMap<String, HashMap<String, Vec<i32>>>,
    );

    fn cache(files: &[(&str, CostUsageFileUsage)]) -> TestCache {
        let mut file_map = HashMap::new();
        let mut days: HashMap<String, HashMap<String, Vec<i32>>> = HashMap::new();
        for (key, entry) in files {
            for (day, models) in &entry.days {
                let day_entry = days.entry(day.clone()).or_default();
                for (model, packed) in models {
                    let dest = day_entry
                        .entry(model.clone())
                        .or_insert_with(|| vec![0, 0, 0]);
                    for (i, v) in packed.iter().take(3).enumerate() {
                        if i < dest.len() {
                            dest[i] += v;
                        }
                    }
                }
            }
            file_map.insert((*key).to_string(), entry.clone());
        }
        (file_map, days)
    }

    #[test]
    fn out_of_window_entries_are_pruned_and_days_subtracted() {
        let (mut files, mut days) = cache(&[
            ("old", entry(&["2026-01-01"], None, 100)),
            ("in1", entry(&["2026-01-10"], None, 100)),
            ("in2", entry(&["2026-01-15"], None, 100)),
        ]);

        let removed = prune_out_of_window_for_budget(
            &mut files,
            &mut days,
            Some("2026-01-08"),
            Some("2026-01-20"),
            true,
        );

        assert_eq!(removed, vec!["old".to_string()]);
        assert!(!files.contains_key("old"));
        // Aggregated day for the pruned entry is gone.
        assert!(!days.contains_key("2026-01-01"));
        // In-window entries kept.
        assert!(files.contains_key("in1"));
        assert!(days.contains_key("2026-01-10"));
    }

    #[test]
    fn partially_parsed_entries_are_protected_from_pruning() {
        let (mut files, mut days) = cache(&[
            ("old-growing", entry(&["2026-01-01"], Some(50), 100)),
            ("in1", entry(&["2026-01-10"], None, 100)),
        ]);

        let removed = prune_out_of_window_for_budget(
            &mut files,
            &mut days,
            Some("2026-01-08"),
            Some("2026-01-20"),
            true,
        );

        assert!(
            removed.is_empty(),
            "protected entry not pruned: {removed:?}"
        );
        assert!(files.contains_key("old-growing"));
    }

    #[test]
    fn in_window_trim_keeps_newest_and_subtracts_days() {
        // Build a cache where every entry is in-window; force a trim below the
        // natural estimate by using a tiny budget.
        let (mut files, mut days) = cache(&[
            ("a", entry(&["2026-01-09"], None, 100)),
            ("b", entry(&["2026-01-10"], None, 100)),
            ("c", entry(&["2026-01-11"], None, 100)),
        ]);

        let removed = trim_in_window_for_budget(
            &mut files,
            &mut days,
            Some("2026-01-01"),
            Some("2026-01-31"),
            // Tiny budget forces dropping entries; newest (c) is always kept.
            1024,
        );

        assert!(!removed.is_empty());
        // Newest entry is retained.
        assert!(files.contains_key("c"));
        // Dropped entries' files are removed from the cache.
        for key in &removed {
            assert!(!files.contains_key(key));
        }
        // At least one dropped entry's day is gone from the aggregate when no
        // surviving entry contributed to it.
        let any_day_gone = ["2026-01-09", "2026-01-10", "2026-01-11"]
            .iter()
            .any(|day| !days.contains_key(*day));
        assert!(
            any_day_gone,
            "expected at least one pruned day key to vanish"
        );
    }

    // budget_constants_match_upstream builds at compile-time, not per test run.
    const _: () = {
        assert!(CostUsageCacheBudget::MAX_FILE_BYTES == 256 * 1024 * 1024);
        assert!(CostUsageCacheBudget::MAX_FILE_ENTRIES == 25_000);
        assert!(CostUsageCacheBudget::MAX_LOAD_BYTES == 320 * 1024 * 1024);
        assert!(CostUsageCacheBudget::MAX_FILE_BYTES < CostUsageCacheBudget::MAX_LOAD_BYTES);
    };

    #[test]
    fn is_unpriced_codex_routing_model_flags_auto_review_and_unattributed() {
        assert!(is_unpriced_codex_routing_model("codex-auto-review"));
        assert!(is_unpriced_codex_routing_model("unknown"));
        assert!(is_unpriced_codex_routing_model(""));
        assert!(!is_unpriced_codex_routing_model("gpt-5.6-sol"));
    }
}
