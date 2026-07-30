//! Session-equivalent weekly forecast.
//!
//! Estimates how many full 5-hour session quotas remain in the weekly window
//! from recent plan-utilization burn history (upstream SessionEquivalentForecast).

use chrono::{DateTime, Datelike, Duration, Utc, Weekday};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// Canonical session window length (5 hours).
pub const SESSION_WINDOW_MINUTES: u32 = 300;
/// Canonical weekly window length (7 days).
pub const WEEKLY_WINDOW_MINUTES: u32 = 10_080;
/// Reset-boundary grouping tolerance.
pub const RESET_TOLERANCE_SECS: i64 = 120;

/// Median weekly-percent burn observed per completed session window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionEquivalentBurnEstimate {
    pub median_weekly_percent_per_window: f64,
    pub sample_count: usize,
}

/// Forecast of remaining session-equivalent windows inside the weekly quota.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEquivalentForecast {
    pub estimated_windows_to_exhaust_weekly: f64,
    pub windows_until_reset: i64,
    pub available_windows_until_reset: f64,
    pub sample_count: usize,
    pub weekly_resets_at: DateTime<Utc>,
    pub weekly_used_percent: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekly_window_id: Option<String>,
}

/// One captured plan-utilization observation.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanUtilizationHistoryEntry {
    pub captured_at: DateTime<Utc>,
    pub used_percent: f64,
    pub resets_at: Option<DateTime<Utc>>,
}

/// Named plan-utilization series used by the burn estimator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlanUtilizationSeriesName {
    Session,
    Weekly,
}

impl PlanUtilizationSeriesName {
    pub fn canonical_window_minutes(self) -> u32 {
        match self {
            Self::Session => SESSION_WINDOW_MINUTES,
            Self::Weekly => WEEKLY_WINDOW_MINUTES,
        }
    }
}

/// Chronological history for one series.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanUtilizationSeriesHistory {
    pub name: PlanUtilizationSeriesName,
    pub window_minutes: u32,
    pub entries: Vec<PlanUtilizationHistoryEntry>,
}

/// Burn-estimator knobs matching upstream SessionEquivalentBurnEstimator.
pub struct SessionEquivalentBurnEstimator;

impl SessionEquivalentBurnEstimator {
    pub const DEFAULT_SAMPLE_LIMIT: usize = 7;
    pub const MINIMUM_SAMPLE_COUNT: usize = 3;

    /// Estimate median full-session weekly burn from plan-utilization histories.
    pub fn estimate(
        histories: &[PlanUtilizationSeriesHistory],
        current_session_resets_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
        sample_limit: usize,
    ) -> Option<SessionEquivalentBurnEstimate> {
        if sample_limit == 0 {
            return None;
        }

        let session_history = histories.iter().find(|h| {
            h.name == PlanUtilizationSeriesName::Session
                && h.window_minutes == SESSION_WINDOW_MINUTES
        })?;
        let weekly_history = histories.iter().find(|h| {
            h.name == PlanUtilizationSeriesName::Weekly && h.window_minutes == WEEKLY_WINDOW_MINUTES
        })?;

        let session_duration = Duration::seconds(i64::from(SESSION_WINDOW_MINUTES) * 60);
        let weekly_duration = Duration::seconds(i64::from(WEEKLY_WINDOW_MINUTES) * 60);

        if !is_chronologically_ordered(&session_history.entries)
            || !is_chronologically_ordered(&weekly_history.entries)
        {
            return None;
        }

        // Expired/invalid-remaining current session = idle: don't gate the burn
        // estimate on it. Implausible far-future resets still abort.
        let effective_current = match current_session_resets_at {
            None => None,
            Some(current) => {
                let remaining = (current - now).num_seconds() as f64;
                let max_remaining =
                    session_duration.num_seconds() as f64 + RESET_TOLERANCE_SECS as f64;
                if !remaining.is_finite() || remaining > max_remaining {
                    return None;
                }
                if remaining <= 0.0 {
                    None
                } else {
                    Some(current)
                }
            }
        };

        let mut groups: Vec<SessionGroup> = Vec::new();
        for entry in &session_history.entries {
            if !entry.used_percent.is_finite() || !(0.0..=100.0).contains(&entry.used_percent) {
                continue;
            }
            let Some(resets_at) = entry.resets_at else {
                continue;
            };
            if !is_plausible_reset(resets_at, entry.captured_at, session_duration) {
                continue;
            }

            if let Some(last) = groups.last_mut() {
                let delta = (last.resets_at - resets_at).num_seconds().abs();
                if delta <= RESET_TOLERANCE_SECS {
                    last.entries.push(entry.clone());
                    last.maximum_used_percent = last.maximum_used_percent.max(entry.used_percent);
                    continue;
                }
                if last.resets_at > resets_at {
                    return None;
                }
            }

            groups.push(SessionGroup {
                resets_at,
                entries: vec![entry.clone()],
                maximum_used_percent: entry.used_percent,
            });
        }

        let completed_active_groups: Vec<&SessionGroup> = groups
            .iter()
            .rev()
            .filter(|group| {
                let precedes_current = effective_current
                    .map(|cur| group.resets_at < cur - Duration::seconds(RESET_TOLERANCE_SECS))
                    .unwrap_or(true);
                precedes_current && group.resets_at <= now && group.maximum_used_percent > 0.0
            })
            .collect();

        let weekly_entries: Vec<&PlanUtilizationHistoryEntry> = weekly_history
            .entries
            .iter()
            .filter(|entry| {
                entry.used_percent.is_finite()
                    && (0.0..=100.0).contains(&entry.used_percent)
                    && entry
                        .resets_at
                        .is_some_and(|r| is_plausible_reset(r, entry.captured_at, weekly_duration))
            })
            .collect();
        if weekly_entries.is_empty() {
            return None;
        }

        let mut burns = Vec::new();
        for group in completed_active_groups.into_iter().take(sample_limit) {
            if let Some(burn) = normalized_burn(group, &weekly_entries, session_duration) {
                burns.push(burn);
            }
        }

        if burns.len() < Self::MINIMUM_SAMPLE_COUNT {
            return None;
        }
        burns.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let middle = burns.len() / 2;
        let median = if burns.len().is_multiple_of(2) {
            (burns[middle - 1] + burns[middle]) / 2.0
        } else {
            burns[middle]
        };
        if !median.is_finite() || median <= 0.0 {
            return None;
        }

        Some(SessionEquivalentBurnEstimate {
            median_weekly_percent_per_window: median,
            sample_count: burns.len(),
        })
    }
}

impl SessionEquivalentForecast {
    /// Build a forecast from current windows + burn estimate.
    pub fn make(
        session_window: &crate::core::RateWindow,
        weekly_window: &crate::core::RateWindow,
        burn_estimate: &SessionEquivalentBurnEstimate,
        weekly_window_id: Option<String>,
        now: DateTime<Utc>,
        work_days: Option<u8>,
    ) -> Option<Self> {
        if session_window.is_informational {
            return None;
        }
        if session_window.window_minutes != Some(SESSION_WINDOW_MINUTES) {
            return None;
        }
        if weekly_window.window_minutes != Some(WEEKLY_WINDOW_MINUTES) {
            return None;
        }
        let weekly_resets_at = weekly_window.resets_at?;
        if !weekly_window.used_percent.is_finite()
            || !(0.0..=100.0).contains(&weekly_window.used_percent)
        {
            return None;
        }
        if !burn_estimate.median_weekly_percent_per_window.is_finite()
            || burn_estimate.median_weekly_percent_per_window <= 0.0
            || burn_estimate.sample_count < SessionEquivalentBurnEstimator::MINIMUM_SAMPLE_COUNT
        {
            return None;
        }

        let session_seconds = f64::from(SESSION_WINDOW_MINUTES) * 60.0;
        let weekly_seconds = f64::from(WEEKLY_WINDOW_MINUTES) * 60.0;

        // Expired session resets mean the window is idle — still show the learned
        // estimate against the weekly lane. Only reject implausible far-future resets.
        if let Some(session_resets_at) = session_window.resets_at {
            let session_remaining = (session_resets_at - now).num_seconds() as f64;
            if !session_remaining.is_finite()
                || session_remaining > session_seconds + RESET_TOLERANCE_SECS as f64
            {
                return None;
            }
        }

        let weekly_remaining = (weekly_resets_at - now).num_seconds() as f64;
        if !weekly_remaining.is_finite()
            || weekly_remaining <= 0.0
            || weekly_remaining > weekly_seconds + RESET_TOLERANCE_SECS as f64
        {
            return None;
        }

        let remaining_weekly_percent = (100.0 - weekly_window.used_percent).clamp(0.0, 100.0);
        if remaining_weekly_percent <= 0.0 {
            return None;
        }
        let estimated_windows =
            remaining_weekly_percent / burn_estimate.median_weekly_percent_per_window;
        if !estimated_windows.is_finite() || estimated_windows < 0.0 {
            return None;
        }

        let remaining_seconds = effective_remaining_seconds(now, weekly_resets_at, work_days);
        if remaining_seconds < 0.0 {
            return None;
        }
        let available_windows_until_reset = remaining_seconds / session_seconds;
        let windows_until_reset = available_windows_until_reset.floor() as i64;

        Some(Self {
            estimated_windows_to_exhaust_weekly: estimated_windows,
            windows_until_reset,
            available_windows_until_reset,
            sample_count: burn_estimate.sample_count,
            weekly_resets_at,
            weekly_used_percent: weekly_window.used_percent,
            weekly_window_id,
        })
    }
}

#[derive(Debug, Clone)]
struct SessionGroup {
    resets_at: DateTime<Utc>,
    entries: Vec<PlanUtilizationHistoryEntry>,
    maximum_used_percent: f64,
}

#[derive(Debug, Clone)]
struct BurnObservation {
    session_used_percent: f64,
    weekly_entry: PlanUtilizationHistoryEntry,
}

fn normalized_burn(
    group: &SessionGroup,
    weekly_entries: &[&PlanUtilizationHistoryEntry],
    session_duration: Duration,
) -> Option<f64> {
    let first_session = group.entries.first()?;
    let last_session = group.entries.last()?;

    let mut observations: Vec<BurnObservation> = Vec::new();
    let window_start = group.resets_at - session_duration;

    if let Some(weekly_start) = nearest_entry(
        window_start,
        weekly_entries,
        Duration::seconds(RESET_TOLERANCE_SECS),
        true,
    ) && weekly_start.captured_at <= window_start
        && weekly_start.captured_at < first_session.captured_at
    {
        observations.push(BurnObservation {
            session_used_percent: 0.0,
            weekly_entry: weekly_start.clone(),
        });
    }

    for session_entry in &group.entries {
        // Upstream observationAlignmentTolerance is 0 → exact capture match only.
        if let Some(weekly_entry) = nearest_entry(
            session_entry.captured_at,
            weekly_entries,
            Duration::zero(),
            false,
        ) {
            observations.push(BurnObservation {
                session_used_percent: session_entry.used_percent,
                weekly_entry: weekly_entry.clone(),
            });
        }
    }

    if group.maximum_used_percent >= 100.0
        && let Some(weekly_end) = nearest_entry(
            group.resets_at,
            weekly_entries,
            Duration::seconds(RESET_TOLERANCE_SECS),
            true,
        )
        && weekly_end.captured_at <= group.resets_at
        && last_session.captured_at < weekly_end.captured_at
    {
        observations.push(BurnObservation {
            session_used_percent: 100.0,
            weekly_entry: weekly_end.clone(),
        });
    }

    observations.sort_by(|lhs, rhs| {
        match lhs
            .weekly_entry
            .captured_at
            .cmp(&rhs.weekly_entry.captured_at)
        {
            std::cmp::Ordering::Equal => lhs
                .session_used_percent
                .partial_cmp(&rhs.session_used_percent)
                .unwrap_or(std::cmp::Ordering::Equal),
            other => other,
        }
    });

    let start = observations.first()?;
    let end = observations.last()?;
    if start.weekly_entry.captured_at >= end.weekly_entry.captured_at {
        return None;
    }
    let start_reset = start.weekly_entry.resets_at?;
    let end_reset = end.weekly_entry.resets_at?;
    if (start_reset - end_reset).num_seconds().abs() > RESET_TOLERANCE_SECS {
        return None;
    }

    let session_consumption = end.session_used_percent - start.session_used_percent;
    let weekly_burn = end.weekly_entry.used_percent - start.weekly_entry.used_percent;
    if !session_consumption.is_finite()
        || session_consumption <= 0.0
        || !weekly_burn.is_finite()
        || weekly_burn <= 0.0
    {
        return None;
    }
    let full_allowance_burn = 100.0 * weekly_burn / session_consumption;
    if !full_allowance_burn.is_finite() || full_allowance_burn <= 0.0 {
        return None;
    }
    Some(full_allowance_burn)
}

fn nearest_entry<'a>(
    target: DateTime<Utc>,
    entries: &[&'a PlanUtilizationHistoryEntry],
    tolerance: Duration,
    require_not_after_target: bool,
) -> Option<&'a PlanUtilizationHistoryEntry> {
    if entries.is_empty() {
        return None;
    }
    let mut lower = 0usize;
    let mut upper = entries.len();
    while lower < upper {
        let middle = (lower + upper) / 2;
        if entries[middle].captured_at < target {
            lower = middle + 1;
        } else {
            upper = middle;
        }
    }

    let mut candidates = Vec::new();
    if lower < entries.len() {
        candidates.push(entries[lower]);
    }
    if lower > 0 {
        candidates.push(entries[lower - 1]);
    }

    let tol_secs = tolerance.num_seconds().abs();
    candidates
        .into_iter()
        .filter(|e| !require_not_after_target || e.captured_at <= target)
        .filter(|e| (e.captured_at - target).num_seconds().abs() <= tol_secs)
        .min_by_key(|e| (e.captured_at - target).num_seconds().abs())
}

fn is_chronologically_ordered(entries: &[PlanUtilizationHistoryEntry]) -> bool {
    entries
        .windows(2)
        .all(|w| w[0].captured_at <= w[1].captured_at)
}

fn is_plausible_reset(
    resets_at: DateTime<Utc>,
    captured_at: DateTime<Utc>,
    duration: Duration,
) -> bool {
    let remaining = (resets_at - captured_at).num_seconds();
    remaining >= -RESET_TOLERANCE_SECS && remaining <= duration.num_seconds() + RESET_TOLERANCE_SECS
}

/// Remaining seconds until weekly reset, optionally counting only work days.
///
/// `work_days` in `[2, 6]` keeps ISO weekdays `1..=work_days` (Mon..).
/// Anything else falls back to wall-clock remaining time.
pub fn effective_remaining_seconds(
    now: DateTime<Utc>,
    resets_at: DateTime<Utc>,
    work_days: Option<u8>,
) -> f64 {
    let wall = (resets_at - now).num_seconds().max(0) as f64;
    let Some(work_days) = work_days else {
        return wall;
    };
    if !(2..=6).contains(&work_days) {
        return wall;
    }

    let mut work_seconds = 0.0_f64;
    let mut cursor = now;
    while cursor < resets_at {
        let next_day = match start_of_next_utc_day(cursor) {
            Some(d) if d > cursor => d,
            _ => return wall,
        };
        let slice_end = if next_day < resets_at {
            next_day
        } else {
            resets_at
        };
        if is_workday(cursor, work_days) {
            work_seconds += (slice_end - cursor).num_seconds() as f64;
        }
        cursor = slice_end;
    }
    work_seconds
}

fn start_of_next_utc_day(dt: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let date = dt.date_naive() + Duration::days(1);
    date.and_hms_opt(0, 0, 0)
        .map(|naive| DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc))
}

fn is_workday(date: DateTime<Utc>, work_days: u8) -> bool {
    let iso = match date.weekday() {
        Weekday::Mon => 1,
        Weekday::Tue => 2,
        Weekday::Wed => 3,
        Weekday::Thu => 4,
        Weekday::Fri => 5,
        Weekday::Sat => 6,
        Weekday::Sun => 7,
    };
    iso <= work_days
}

// ── In-process history store ─────────────────────────────────────────

/// Append-only ring of session/weekly observations for burn estimation.
///
/// ponytail: history is process-local and lost on restart; upgrade path = disk cache
/// keyed by provider+account once forecast quality justifies persistence.
#[derive(Debug, Default)]
pub struct SessionEquivalentHistoryStore {
    by_provider: HashMap<String, ProviderHistory>,
}

#[derive(Debug, Default)]
struct ProviderHistory {
    session: Vec<PlanUtilizationHistoryEntry>,
    weekly: Vec<PlanUtilizationHistoryEntry>,
}

static HISTORY_STORE: LazyLock<Mutex<SessionEquivalentHistoryStore>> =
    LazyLock::new(|| Mutex::new(SessionEquivalentHistoryStore::default()));

impl SessionEquivalentHistoryStore {
    /// Record one observation pair for a provider (session + optional weekly).
    pub fn record(
        &mut self,
        provider_id: &str,
        session: Option<PlanUtilizationHistoryEntry>,
        weekly: Option<PlanUtilizationHistoryEntry>,
        sample_limit: usize,
    ) {
        let limit = sample_limit.max(1);
        // Keep more raw points than completed groups so grouping still has density.
        let ring = limit.saturating_mul(8).max(24);
        let hist = self.by_provider.entry(provider_id.to_string()).or_default();
        if let Some(entry) = session {
            push_ring(&mut hist.session, entry, ring);
        }
        if let Some(entry) = weekly {
            push_ring(&mut hist.weekly, entry, ring);
        }
    }

    pub fn histories(&self, provider_id: &str) -> Vec<PlanUtilizationSeriesHistory> {
        let Some(hist) = self.by_provider.get(provider_id) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if !hist.session.is_empty() {
            out.push(PlanUtilizationSeriesHistory {
                name: PlanUtilizationSeriesName::Session,
                window_minutes: SESSION_WINDOW_MINUTES,
                entries: hist.session.clone(),
            });
        }
        if !hist.weekly.is_empty() {
            out.push(PlanUtilizationSeriesHistory {
                name: PlanUtilizationSeriesName::Weekly,
                window_minutes: WEEKLY_WINDOW_MINUTES,
                entries: hist.weekly.clone(),
            });
        }
        out
    }
}

fn push_ring(
    buf: &mut Vec<PlanUtilizationHistoryEntry>,
    entry: PlanUtilizationHistoryEntry,
    ring: usize,
) {
    buf.push(entry);
    if buf.len() > ring {
        let drop_n = buf.len() - ring;
        buf.drain(0..drop_n);
    }
}

/// Global in-process history used by Claude/Codex snapshot refresh.
pub fn global_history_store() -> &'static Mutex<SessionEquivalentHistoryStore> {
    &HISTORY_STORE
}

/// Record session/weekly windows from a live provider usage snapshot.
pub fn record_provider_windows(
    provider_id: &str,
    session: &crate::core::RateWindow,
    weekly: Option<&crate::core::RateWindow>,
    now: DateTime<Utc>,
) {
    if session.is_informational
        || session.window_minutes != Some(SESSION_WINDOW_MINUTES)
        || !session.used_percent.is_finite()
    {
        return;
    }
    let session_entry = PlanUtilizationHistoryEntry {
        captured_at: now,
        used_percent: session.used_percent.clamp(0.0, 100.0),
        resets_at: session.resets_at,
    };
    let weekly_entry = weekly.and_then(|w| {
        if w.is_informational
            || w.window_minutes != Some(WEEKLY_WINDOW_MINUTES)
            || !w.used_percent.is_finite()
        {
            return None;
        }
        Some(PlanUtilizationHistoryEntry {
            captured_at: now,
            used_percent: w.used_percent.clamp(0.0, 100.0),
            resets_at: w.resets_at,
        })
    });

    if let Ok(mut guard) = global_history_store().lock() {
        guard.record(
            provider_id,
            Some(session_entry),
            weekly_entry,
            SessionEquivalentBurnEstimator::DEFAULT_SAMPLE_LIMIT,
        );
    }
}

/// Last learned full-session burn estimate retained across idle refreshes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetainedFullSessionEstimate {
    pub estimate: f64,
    pub updated_at: DateTime<Utc>,
}

/// Keep the last positive full-session estimate when a refresh yields none.
///
/// `fresh` wins when present and valid; otherwise `previous` is kept so the UI
/// does not blank while the session window is idle (upstream #2336).
pub fn retain_last_full_session_estimate(
    previous: Option<RetainedFullSessionEstimate>,
    fresh: Option<f64>,
    now: DateTime<Utc>,
) -> Option<RetainedFullSessionEstimate> {
    match fresh {
        Some(estimate) if estimate.is_finite() && estimate > 0.0 => {
            Some(RetainedFullSessionEstimate {
                estimate,
                updated_at: now,
            })
        }
        _ => previous,
    }
}

// ponytail: in-memory only, lost on restart; upgrade = persist to cache file.
#[derive(Debug, Default)]
struct LastFullSessionEstimateStore {
    by_provider: HashMap<String, RetainedFullSessionEstimate>,
}

static LAST_FULL_SESSION_ESTIMATE_STORE: LazyLock<Mutex<LastFullSessionEstimateStore>> =
    LazyLock::new(|| Mutex::new(LastFullSessionEstimateStore::default()));

fn last_full_session_estimate_store() -> &'static Mutex<LastFullSessionEstimateStore> {
    &LAST_FULL_SESSION_ESTIMATE_STORE
}

/// Remember/recall the last learned full-session burn estimate for a provider.
pub fn remember_full_session_estimate(
    provider_id: &str,
    fresh: Option<f64>,
    now: DateTime<Utc>,
) -> Option<f64> {
    let Ok(mut guard) = last_full_session_estimate_store().lock() else {
        return fresh.filter(|v| v.is_finite() && *v > 0.0);
    };
    let previous = guard.by_provider.get(provider_id).copied();
    let retained = retain_last_full_session_estimate(previous, fresh, now);
    if let Some(entry) = retained {
        guard.by_provider.insert(provider_id.to_string(), entry);
        Some(entry.estimate)
    } else {
        guard.by_provider.remove(provider_id);
        None
    }
}

/// Compute forecast for a provider using the in-process history ring.
pub fn forecast_for_provider(
    provider_id: &str,
    session: &crate::core::RateWindow,
    weekly: &crate::core::RateWindow,
    now: DateTime<Utc>,
    work_days: Option<u8>,
) -> Option<SessionEquivalentForecast> {
    let histories = global_history_store()
        .lock()
        .ok()
        .map(|g| g.histories(provider_id))
        .unwrap_or_default();
    let fresh_burn = SessionEquivalentBurnEstimator::estimate(
        &histories,
        session.resets_at,
        now,
        SessionEquivalentBurnEstimator::DEFAULT_SAMPLE_LIMIT,
    );
    let fresh_median = fresh_burn
        .as_ref()
        .map(|b| b.median_weekly_percent_per_window);
    let sample_count = fresh_burn
        .as_ref()
        .map(|b| b.sample_count)
        .unwrap_or(SessionEquivalentBurnEstimator::MINIMUM_SAMPLE_COUNT);
    let median = remember_full_session_estimate(provider_id, fresh_median, now)?;
    let burn = SessionEquivalentBurnEstimate {
        median_weekly_percent_per_window: median,
        sample_count,
    };
    SessionEquivalentForecast::make(session, weekly, &burn, None, now, work_days)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::RateWindow;
    use chrono::TimeZone;

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).single().unwrap()
    }

    fn entry(captured: i64, used: f64, resets: i64) -> PlanUtilizationHistoryEntry {
        PlanUtilizationHistoryEntry {
            captured_at: ts(captured),
            used_percent: used,
            resets_at: Some(ts(resets)),
        }
    }

    fn session_secs() -> i64 {
        i64::from(SESSION_WINDOW_MINUTES) * 60
    }

    fn weekly_secs() -> i64 {
        i64::from(WEEKLY_WINDOW_MINUTES) * 60
    }

    /// Build three completed full-burn sessions with aligned weekly observations.
    fn three_sample_histories(
        base: i64,
    ) -> (
        Vec<PlanUtilizationSeriesHistory>,
        DateTime<Utc>,
        DateTime<Utc>,
    ) {
        let s = session_secs();
        let w = weekly_secs();
        let weekly_reset = base + w;

        let mut session_entries = Vec::new();
        let mut weekly_entries = Vec::new();

        // Three completed sessions ending at base+s, base+2s, base+3s.
        for i in 0..3 {
            let start = base + i * s;
            let end = start + s;
            // Window-start weekly anchor (session used 0).
            weekly_entries.push(entry(start - 30, 10.0 + i as f64 * 5.0, weekly_reset));
            // Mid / end session captures with exact weekly alignment.
            session_entries.push(entry(start + 60, 40.0, end));
            weekly_entries.push(entry(start + 60, 12.0 + i as f64 * 5.0, weekly_reset));
            session_entries.push(entry(end - 60, 100.0, end));
            weekly_entries.push(entry(end - 60, 15.0 + i as f64 * 5.0, weekly_reset));
            // End-of-window weekly for max>=100 path.
            weekly_entries.push(entry(end, 15.5 + i as f64 * 5.0, weekly_reset));
        }

        // Current open session.
        let current_start = base + 3 * s;
        let current_end = current_start + s;
        let now_secs = current_start + 30 * 60;
        session_entries.push(entry(now_secs, 20.0, current_end));
        weekly_entries.push(entry(now_secs, 30.0, weekly_reset));

        session_entries.sort_by_key(|e| e.captured_at);
        weekly_entries.sort_by_key(|e| e.captured_at);

        let histories = vec![
            PlanUtilizationSeriesHistory {
                name: PlanUtilizationSeriesName::Session,
                window_minutes: SESSION_WINDOW_MINUTES,
                entries: session_entries,
            },
            PlanUtilizationSeriesHistory {
                name: PlanUtilizationSeriesName::Weekly,
                window_minutes: WEEKLY_WINDOW_MINUTES,
                entries: weekly_entries,
            },
        ];
        (histories, ts(now_secs), ts(current_end))
    }

    #[test]
    fn median_math_from_three_full_sessions() {
        let base = 1_700_000_000_i64;
        let (histories, now, current_end) = three_sample_histories(base);
        let estimate = SessionEquivalentBurnEstimator::estimate(
            &histories,
            Some(current_end),
            now,
            SessionEquivalentBurnEstimator::DEFAULT_SAMPLE_LIMIT,
        )
        .expect("estimate");
        assert_eq!(estimate.sample_count, 3);
        // Mid-session captures dominate when start/end anchors are optional:
        // weekly 12→15 over session 40→100 ⇒ 100 * 3 / 60 = 5.0
        // (window-start + end anchors yield the same full-allowance burn here).
        assert!(
            (estimate.median_weekly_percent_per_window - 5.0).abs() < 0.01,
            "median={}",
            estimate.median_weekly_percent_per_window
        );
    }

    #[test]
    fn grouping_tolerance_merges_nearby_resets() {
        let s = session_secs();
        let base = 1_700_100_000_i64;
        let weekly_reset = base + weekly_secs();
        let end = base + s;

        // Nearby resets within ±120s must merge into one group (not out-of-order).
        let single_group = vec![
            PlanUtilizationSeriesHistory {
                name: PlanUtilizationSeriesName::Session,
                window_minutes: SESSION_WINDOW_MINUTES,
                entries: vec![
                    entry(base + 60, 30.0, end),
                    entry(base + 120, 60.0, end + 100),
                    entry(base + 180, 90.0, end + 110),
                ],
            },
            PlanUtilizationSeriesHistory {
                name: PlanUtilizationSeriesName::Weekly,
                window_minutes: WEEKLY_WINDOW_MINUTES,
                entries: vec![
                    entry(base - 30, 5.0, weekly_reset),
                    entry(base + 60, 8.0, weekly_reset),
                    entry(base + 120, 10.0, weekly_reset),
                    entry(base + 180, 12.0, weekly_reset),
                ],
            },
        ];
        // One completed group → below minimum sample count, but not an ordering reject.
        assert!(
            SessionEquivalentBurnEstimator::estimate(
                &single_group,
                Some(ts(end + s)),
                ts(end + 60),
                7
            )
            .is_none()
        );

        // Strictly decreasing resets_at across groups is rejected.
        let out_of_order = vec![
            PlanUtilizationSeriesHistory {
                name: PlanUtilizationSeriesName::Session,
                window_minutes: SESSION_WINDOW_MINUTES,
                entries: vec![
                    entry(base + 60, 30.0, end + s),
                    entry(base + 120, 60.0, end), // jumps backward beyond tolerance
                ],
            },
            PlanUtilizationSeriesHistory {
                name: PlanUtilizationSeriesName::Weekly,
                window_minutes: WEEKLY_WINDOW_MINUTES,
                entries: vec![
                    entry(base + 60, 8.0, weekly_reset),
                    entry(base + 120, 10.0, weekly_reset),
                ],
            },
        ];
        assert!(
            SessionEquivalentBurnEstimator::estimate(
                &out_of_order,
                Some(ts(end + 2 * s)),
                ts(end + s + 60),
                7
            )
            .is_none()
        );
    }

    #[test]
    fn insufficient_samples_returns_none() {
        let base = 1_700_200_000_i64;
        let s = session_secs();
        let weekly_reset = base + weekly_secs();
        let end = base + s;
        let histories = vec![
            PlanUtilizationSeriesHistory {
                name: PlanUtilizationSeriesName::Session,
                window_minutes: SESSION_WINDOW_MINUTES,
                entries: vec![entry(base + 60, 50.0, end), entry(base + 120, 80.0, end)],
            },
            PlanUtilizationSeriesHistory {
                name: PlanUtilizationSeriesName::Weekly,
                window_minutes: WEEKLY_WINDOW_MINUTES,
                entries: vec![
                    entry(base - 30, 10.0, weekly_reset),
                    entry(base + 60, 12.0, weekly_reset),
                    entry(base + 120, 14.0, weekly_reset),
                ],
            },
        ];
        assert!(
            SessionEquivalentBurnEstimator::estimate(
                &histories,
                Some(ts(end + s)),
                ts(end + 60),
                7
            )
            .is_none()
        );
    }

    #[test]
    fn forecast_make_fractional_windows() {
        let now = ts(1_700_300_000);
        let session = RateWindow::with_details(
            25.0,
            Some(SESSION_WINDOW_MINUTES),
            Some(now + Duration::hours(3)),
            None,
        );
        let weekly = RateWindow::with_details(
            40.0,
            Some(WEEKLY_WINDOW_MINUTES),
            Some(now + Duration::days(3)),
            None,
        );
        let burn = SessionEquivalentBurnEstimate {
            median_weekly_percent_per_window: 12.0,
            sample_count: 3,
        };
        let forecast = SessionEquivalentForecast::make(&session, &weekly, &burn, None, now, None)
            .expect("forecast");
        // remaining weekly 60 / 12 = 5.0
        assert!((forecast.estimated_windows_to_exhaust_weekly - 5.0).abs() < 1e-9);
        assert!(forecast.available_windows_until_reset > 0.0);
        assert_eq!(
            forecast.windows_until_reset,
            forecast.available_windows_until_reset.floor() as i64
        );
        assert_eq!(forecast.sample_count, 3);
        assert!((forecast.weekly_used_percent - 40.0).abs() < 1e-9);
    }

    #[test]
    fn work_days_reduces_available_windows_vs_wall_clock() {
        // Friday 18:00 UTC → Monday 18:00 UTC (weekend in between).
        let friday = Utc
            .with_ymd_and_hms(2024, 6, 14, 18, 0, 0)
            .single()
            .unwrap();
        assert_eq!(friday.weekday(), Weekday::Fri);
        let monday = Utc
            .with_ymd_and_hms(2024, 6, 17, 18, 0, 0)
            .single()
            .unwrap();

        let wall = effective_remaining_seconds(friday, monday, None);
        let work5 = effective_remaining_seconds(friday, monday, Some(5));
        assert!((wall - 3.0 * 24.0 * 3600.0).abs() < 1.0);
        // Fri 18→24 = 6h, Mon 0→18 = 18h → 24h work seconds (no Sat/Sun).
        assert!((work5 - 24.0 * 3600.0).abs() < 1.0);
        assert!(work5 < wall);

        let session = RateWindow::with_details(
            10.0,
            Some(SESSION_WINDOW_MINUTES),
            Some(friday + Duration::hours(4)),
            None,
        );
        let weekly =
            RateWindow::with_details(20.0, Some(WEEKLY_WINDOW_MINUTES), Some(monday), None);
        let burn = SessionEquivalentBurnEstimate {
            median_weekly_percent_per_window: 10.0,
            sample_count: 4,
        };
        let wall_f =
            SessionEquivalentForecast::make(&session, &weekly, &burn, None, friday, None).unwrap();
        let work_f =
            SessionEquivalentForecast::make(&session, &weekly, &burn, None, friday, Some(5))
                .unwrap();
        assert!(work_f.available_windows_until_reset < wall_f.available_windows_until_reset);
        assert_eq!(
            work_f.windows_until_reset,
            work_f.available_windows_until_reset.floor() as i64
        );
    }

    #[test]
    fn work_days_out_of_range_uses_wall_clock() {
        let now = ts(1_700_400_000);
        let reset = now + Duration::days(2);
        let wall = effective_remaining_seconds(now, reset, None);
        assert!((effective_remaining_seconds(now, reset, Some(1)) - wall).abs() < 1e-9);
        assert!((effective_remaining_seconds(now, reset, Some(7)) - wall).abs() < 1e-9);
    }

    #[test]
    fn history_ring_retains_latest_samples() {
        let mut store = SessionEquivalentHistoryStore::default();
        let base = ts(1_700_500_000);
        for i in 0..30 {
            store.record(
                "claude",
                Some(PlanUtilizationHistoryEntry {
                    captured_at: base + Duration::minutes(i),
                    used_percent: i as f64,
                    resets_at: Some(base + Duration::hours(5)),
                }),
                None,
                7,
            );
        }
        let h = store.histories("claude");
        assert_eq!(h.len(), 1);
        assert!(h[0].entries.len() <= 7 * 8);
        assert!((h[0].entries.last().unwrap().used_percent - 29.0).abs() < 1e-9);
    }

    #[test]
    fn make_rejects_bad_windows_and_zero_remaining() {
        let now = ts(1_700_600_000);
        let burn = SessionEquivalentBurnEstimate {
            median_weekly_percent_per_window: 10.0,
            sample_count: 3,
        };
        let session = RateWindow::with_details(
            10.0,
            Some(SESSION_WINDOW_MINUTES),
            Some(now + Duration::hours(2)),
            None,
        );
        let exhausted = RateWindow::with_details(
            100.0,
            Some(WEEKLY_WINDOW_MINUTES),
            Some(now + Duration::days(2)),
            None,
        );
        assert!(
            SessionEquivalentForecast::make(&session, &exhausted, &burn, None, now, None).is_none()
        );

        let low_samples = SessionEquivalentBurnEstimate {
            median_weekly_percent_per_window: 10.0,
            sample_count: 2,
        };
        let weekly = RateWindow::with_details(
            50.0,
            Some(WEEKLY_WINDOW_MINUTES),
            Some(now + Duration::days(2)),
            None,
        );
        assert!(
            SessionEquivalentForecast::make(&session, &weekly, &low_samples, None, now, None)
                .is_none()
        );
    }

    #[test]
    fn retain_last_full_session_estimate_survives_idle_refresh() {
        let t0 = ts(1_700_000_000);
        let t1 = t0 + Duration::hours(1);
        let learned = retain_last_full_session_estimate(None, Some(12.5), t0)
            .expect("fresh estimate should stick");
        assert_eq!(learned.estimate, 12.5);
        assert_eq!(learned.updated_at, t0);

        // Idle refresh yields no fresh sample — keep the last learned value.
        let idle = retain_last_full_session_estimate(Some(learned), None, t1)
            .expect("idle refresh must keep last estimate");
        assert_eq!(idle.estimate, 12.5);
        assert_eq!(idle.updated_at, t0);

        // Non-positive / non-finite fresh values also keep previous.
        let still = retain_last_full_session_estimate(Some(idle), Some(0.0), t1)
            .expect("zero fresh must not clear");
        assert_eq!(still.estimate, 12.5);
        let still = retain_last_full_session_estimate(Some(still), Some(f64::NAN), t1)
            .expect("nan fresh must not clear");
        assert_eq!(still.estimate, 12.5);
    }

    #[test]
    fn retain_last_full_session_estimate_replaced_by_fresh_sample() {
        let t0 = ts(1_700_000_000);
        let t1 = t0 + Duration::hours(2);
        let previous = retain_last_full_session_estimate(None, Some(8.0), t0).unwrap();
        let next = retain_last_full_session_estimate(Some(previous), Some(15.0), t1)
            .expect("fresh sample replaces previous");
        assert_eq!(next.estimate, 15.0);
        assert_eq!(next.updated_at, t1);
    }

    #[test]
    fn make_keeps_forecast_when_session_reset_is_expired_idle() {
        let now = ts(1_700_600_000);
        let burn = SessionEquivalentBurnEstimate {
            median_weekly_percent_per_window: 10.0,
            sample_count: 3,
        };
        // Session reset already passed (idle) — forecast still derives from weekly.
        let session = RateWindow::with_details(
            0.0,
            Some(SESSION_WINDOW_MINUTES),
            Some(now - Duration::hours(1)),
            None,
        );
        let weekly = RateWindow::with_details(
            40.0,
            Some(WEEKLY_WINDOW_MINUTES),
            Some(now + Duration::days(3)),
            None,
        );
        let forecast = SessionEquivalentForecast::make(&session, &weekly, &burn, None, now, None)
            .expect("idle expired session should still forecast");
        assert!((forecast.estimated_windows_to_exhaust_weekly - 6.0).abs() < 1e-9);
    }
}
