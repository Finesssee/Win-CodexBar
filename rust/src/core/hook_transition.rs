//! Hook transition detector (upstream #2536 / `HookTransitionDetector`).
//!
//! Turns successive provider observations into edge-triggered hook events.
//! Platform-neutral and side-effect free: decides *what fired* and never
//! fetches or runs commands. State is in-memory only — a restart starts fresh
//! and the first sample of any lane establishes a baseline without firing.
//!
//! # Rules implemented
//!
//! - **Baseline-only first sample**: first reading of a lane/status never fires.
//! - **quota_low**: fires only when usage fraction crosses a watched threshold
//!   upward (`previous < t && current >= t`). Rules with an explicit `threshold`
//!   watch only that value; rules without one use the lane's
//!   `fallback_thresholds` (provider notification thresholds as used fractions).
//! - **quota_low rule narrowing**: only rules whose own threshold crossed this
//!   poll are attached to the dispatch (avoids re-firing lower thresholds).
//! - **quota_reached**: session lane only; fires on upward edge into
//!   `reached_threshold` (default 1.0). Weekly lanes never fire `quota_reached`.
//! - **quota_reset**: fires when the reset boundary advances
//!   (`current_resets_at > previous_resets_at`) **or** usage drops by at least
//!   `reset_drop_threshold` (default 0.2). A reset suppresses depletion edges
//!   (`quota_low` / `quota_reached`) in the same poll.
//! - **provider_unavailable / provider_recovered**: edge on definite outage
//!   state (`minor`/`major`/`critical` ↔ `none`). `maintenance` and `unknown`
//!   never flip tracked state.
//! - **refresh_failed**: emits a coarse failure status without disturbing quota
//!   or status baselines.
//! - **Lane lifecycle**: synthetic/informational or missing lanes forget their
//!   baseline; lanes that disappear between polls are pruned so reappearance
//!   starts fresh.
//! - **Config revision**: `reset_if_configuration_changed` clears all baselines
//!   so rule edits do not fire for crossings that spanned the change.
//! - **Disabled / over-capacity config**: `enabled == false` or more than
//!   `HooksConfig::MAX_RULES` rules → no events.

use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};

use super::hooks::{HookEvent, HookEventType, HookRule, HooksConfig};
use super::rate_window::RateWindow;

/// Identifies one quota lane for hook transition tracking.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HookQuotaLaneKey {
    pub provider: String,
    pub window: HookQuotaWindow,
    pub account_discriminator: Option<String>,
    pub window_id: Option<String>,
}

impl HookQuotaLaneKey {
    pub fn new(
        provider: impl Into<String>,
        window: HookQuotaWindow,
        account_discriminator: Option<String>,
        window_id: Option<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            window,
            account_discriminator,
            window_id,
        }
    }
}

/// Quota lane kind mirrored from upstream `QuotaWarningWindow`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookQuotaWindow {
    Session,
    Weekly,
}

impl HookQuotaWindow {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Weekly => "weekly",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Session => "Session",
            Self::Weekly => "Weekly",
        }
    }
}

/// One quota lane observed in a single poll.
#[derive(Debug, Clone)]
pub struct HookQuotaLaneObservation {
    pub key: HookQuotaLaneKey,
    /// Display label for the event payload (e.g. "Session", "Weekly").
    pub label: String,
    /// `None` means the lane was not reported this poll.
    pub rate_window: Option<RateWindow>,
    /// Provider notification thresholds as usage fractions (0…1).
    pub fallback_thresholds: Vec<f64>,
    pub account_display_name: Option<String>,
}

/// Coarse provider availability, mirroring status-indicator semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HookProviderStatus {
    None,
    Minor,
    Major,
    Critical,
    Maintenance,
    #[default]
    Unknown,
}

impl HookProviderStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minor => "minor",
            Self::Major => "major",
            Self::Critical => "critical",
            Self::Maintenance => "maintenance",
            Self::Unknown => "unknown",
        }
    }

    /// `maintenance` and `unknown` never flip tracked state.
    pub fn outage_state(self) -> Option<bool> {
        match self {
            Self::Minor | Self::Major | Self::Critical => Some(true),
            Self::None => Some(false),
            Self::Maintenance | Self::Unknown => None,
        }
    }
}

/// Everything observed for one provider in a single poll.
#[derive(Debug, Clone)]
pub struct HookProviderObservation {
    pub provider: String,
    pub lanes: Vec<HookQuotaLaneObservation>,
    pub status: HookProviderStatus,
    /// Coarse failure category when the refresh itself failed (never a raw error).
    pub refresh_failure_status: Option<String>,
    pub account_display_name: Option<String>,
}

impl HookProviderObservation {
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            lanes: Vec::new(),
            status: HookProviderStatus::Unknown,
            refresh_failure_status: None,
            account_display_name: None,
        }
    }
}

/// One event to dispatch, optionally narrowed to specific rules (`quota_low`).
#[derive(Debug, Clone)]
pub struct HookDispatch {
    pub event: HookEvent,
    /// When set (currently only for `quota_low`), only these rules should run.
    pub rules: Option<Vec<HookRule>>,
}

#[derive(Debug, Clone, Copy)]
struct LaneSample {
    usage: f64,
    resets_at: Option<DateTime<Utc>>,
}

/// Turns successive provider observations into hook events.
#[derive(Debug, Default)]
pub struct HookTransitionDetector {
    window_observation: HashMap<HookQuotaLaneKey, LaneSample>,
    provider_status_had_issue: HashMap<String, bool>,
    config_revision: Option<i64>,
    reached_threshold: f64,
    reset_drop_threshold: f64,
}

impl HookTransitionDetector {
    pub fn new() -> Self {
        Self {
            reached_threshold: 1.0,
            reset_drop_threshold: 0.2,
            ..Self::default()
        }
    }

    pub fn with_thresholds(reached_threshold: f64, reset_drop_threshold: f64) -> Self {
        Self {
            reached_threshold,
            reset_drop_threshold,
            ..Self::default()
        }
    }

    /// Drops every baseline when the hook configuration changed.
    pub fn reset_if_configuration_changed(&mut self, revision: i64) {
        if self.config_revision == Some(revision) {
            return;
        }
        self.window_observation.clear();
        self.provider_status_had_issue.clear();
        self.config_revision = Some(revision);
    }

    /// Evaluates one poll of one provider and returns the events to dispatch.
    pub fn evaluate(
        &mut self,
        observation: &HookProviderObservation,
        config: &HooksConfig,
    ) -> Vec<HookDispatch> {
        self.evaluate_at(observation, config, Utc::now())
    }

    pub fn evaluate_at(
        &mut self,
        observation: &HookProviderObservation,
        config: &HooksConfig,
        now: DateTime<Utc>,
    ) -> Vec<HookDispatch> {
        if !config.enabled || config.events.len() > HooksConfig::MAX_RULES {
            return Vec::new();
        }

        if let Some(failure) = observation.refresh_failure_status.as_deref() {
            let event = HookEvent::new(HookEventType::RefreshFailed, observation.provider.clone())
                .with_status(failure)
                .with_timestamp(now);
            let event = match &observation.account_display_name {
                Some(account) => event.with_account(account.clone()),
                None => event,
            };
            // Failed refresh must not disturb baselines.
            return vec![HookDispatch { event, rules: None }];
        }

        let mut dispatches = self.status_events(observation, now);

        let observed_keys: HashSet<HookQuotaLaneKey> =
            observation.lanes.iter().map(|l| l.key.clone()).collect();
        for lane in &observation.lanes {
            dispatches.extend(self.lane_events(lane, &observation.provider, config, now));
        }
        self.prune_lanes(&observation.provider, &observed_keys);

        dispatches
    }

    fn status_events(
        &mut self,
        observation: &HookProviderObservation,
        now: DateTime<Utc>,
    ) -> Vec<HookDispatch> {
        let Some(is_outage) = observation.status.outage_state() else {
            return Vec::new();
        };
        let previous = self
            .provider_status_had_issue
            .insert(observation.provider.clone(), is_outage);
        let Some(previous) = previous else {
            return Vec::new();
        };
        if previous == is_outage {
            return Vec::new();
        }

        let event_type = if is_outage {
            HookEventType::ProviderUnavailable
        } else {
            HookEventType::ProviderRecovered
        };
        let event = HookEvent::new(event_type, observation.provider.clone())
            .with_status(observation.status.as_str())
            .with_timestamp(now);
        let event = match &observation.account_display_name {
            Some(account) => event.with_account(account.clone()),
            None => event,
        };
        vec![HookDispatch { event, rules: None }]
    }

    fn lane_events(
        &mut self,
        lane: &HookQuotaLaneObservation,
        provider: &str,
        config: &HooksConfig,
        now: DateTime<Utc>,
    ) -> Vec<HookDispatch> {
        // Informational / synthetic stand-ins carry no usage to compare. Forget
        // so a later real reading starts fresh.
        let Some(rate_window) = lane.rate_window.as_ref() else {
            self.window_observation.remove(&lane.key);
            return Vec::new();
        };
        if rate_window.is_informational {
            self.window_observation.remove(&lane.key);
            return Vec::new();
        }

        let current = (rate_window.used_percent / 100.0).clamp(0.0, 1.0);
        let previous_sample = self.window_observation.insert(
            lane.key.clone(),
            LaneSample {
                usage: current,
                resets_at: rate_window.resets_at,
            },
        );

        let Some(previous) = previous_sample else {
            return Vec::new();
        };

        if let Some(reset_event) =
            self.reset_event(lane, provider, previous, current, rate_window, now)
        {
            return vec![HookDispatch {
                event: reset_event,
                rules: None,
            }];
        }

        let mut dispatches = self.quota_low_events(lane, provider, previous, current, config, now);

        if lane.key.window == HookQuotaWindow::Session
            && previous.usage < self.reached_threshold
            && current >= self.reached_threshold
        {
            dispatches.push(HookDispatch {
                event: build_lane_event(
                    HookEventType::QuotaReached,
                    provider,
                    lane,
                    current,
                    rate_window.resets_at,
                    now,
                ),
                rules: None,
            });
        }

        dispatches
    }

    fn reset_event(
        &self,
        lane: &HookQuotaLaneObservation,
        provider: &str,
        previous: LaneSample,
        current: f64,
        rate_window: &RateWindow,
        now: DateTime<Utc>,
    ) -> Option<HookEvent> {
        let boundary_moved = match (previous.resets_at, rate_window.resets_at) {
            (Some(prev), Some(curr)) => curr > prev,
            _ => false,
        };
        let usage_dropped = previous.usage - current >= self.reset_drop_threshold;
        if !boundary_moved && !usage_dropped {
            return None;
        }
        Some(build_lane_event(
            HookEventType::QuotaReset,
            provider,
            lane,
            current,
            rate_window.resets_at,
            now,
        ))
    }

    fn quota_low_events(
        &self,
        lane: &HookQuotaLaneObservation,
        provider: &str,
        previous: LaneSample,
        current: f64,
        config: &HooksConfig,
        now: DateTime<Utc>,
    ) -> Vec<HookDispatch> {
        let rules: Vec<&HookRule> = config
            .events
            .iter()
            .filter(|rule| {
                rule.enabled
                    && rule_watches_quota_low(rule)
                    && (rule.provider.is_none() || rule.provider.as_deref() == Some(provider))
            })
            .collect();
        if rules.is_empty() {
            return Vec::new();
        }

        let crossed: Vec<HookRule> = rules
            .into_iter()
            .filter(|rule| {
                quota_low_threshold_crossed(
                    rule.threshold,
                    previous.usage,
                    current,
                    &lane.fallback_thresholds,
                )
            })
            .cloned()
            .collect();
        if crossed.is_empty() {
            return Vec::new();
        }

        vec![HookDispatch {
            event: build_lane_event(
                HookEventType::QuotaLow,
                provider,
                lane,
                current,
                lane.rate_window.as_ref().and_then(|w| w.resets_at),
                now,
            ),
            rules: Some(crossed),
        }]
    }

    fn prune_lanes(&mut self, provider: &str, keeping: &HashSet<HookQuotaLaneKey>) {
        self.window_observation
            .retain(|key, _| key.provider != provider || keeping.contains(key));
    }
}

fn rule_watches_quota_low(rule: &HookRule) -> bool {
    rule.event == Some(HookEventType::QuotaLow) || rule.events.contains(&HookEventType::QuotaLow)
}

/// Returns true when any watched threshold was crossed upward.
pub fn quota_low_threshold_crossed(
    rule_threshold: Option<f64>,
    previous_usage: f64,
    current_usage: f64,
    fallback_thresholds: &[f64],
) -> bool {
    let watched: Vec<f64> = match rule_threshold {
        Some(t) => vec![t],
        None => fallback_thresholds.to_vec(),
    };
    watched
        .into_iter()
        .any(|t| previous_usage < t && current_usage >= t)
}

fn build_lane_event(
    event_type: HookEventType,
    provider: &str,
    lane: &HookQuotaLaneObservation,
    usage_fraction: f64,
    _resets_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> HookEvent {
    let mut event = HookEvent::new(event_type, provider)
        .with_window(lane.label.clone())
        .with_usage_fraction(usage_fraction)
        .with_timestamp(now);
    if let Some(account) = &lane.account_display_name {
        event = event.with_account(account.clone());
    }
    event
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::path::PathBuf;

    const PROVIDER: &str = "codex";

    fn lane_key(window: HookQuotaWindow, account: Option<&str>) -> HookQuotaLaneKey {
        HookQuotaLaneKey::new(PROVIDER, window, account.map(str::to_string), None)
    }

    fn rate_window(used_percent: f64, resets_at: Option<DateTime<Utc>>) -> RateWindow {
        let mut w = RateWindow::new(used_percent);
        w.window_minutes = Some(300);
        w.resets_at = resets_at;
        w
    }

    fn informational_window(used_percent: f64) -> RateWindow {
        let mut w = RateWindow::informational("placeholder");
        w.used_percent = used_percent;
        w
    }

    fn lane(
        used_percent: Option<f64>,
        key: HookQuotaLaneKey,
        resets_at: Option<DateTime<Utc>>,
        thresholds: &[f64],
        informational: bool,
    ) -> HookQuotaLaneObservation {
        let label = key.window.display_name().to_string();
        let account = key.account_discriminator.clone();
        HookQuotaLaneObservation {
            key,
            label,
            rate_window: used_percent.map(|p| {
                if informational {
                    informational_window(p)
                } else {
                    rate_window(p, resets_at)
                }
            }),
            fallback_thresholds: thresholds.to_vec(),
            account_display_name: account,
        }
    }

    fn observation(
        lanes: Vec<HookQuotaLaneObservation>,
        status: HookProviderStatus,
        refresh_failure: Option<&str>,
    ) -> HookProviderObservation {
        HookProviderObservation {
            provider: PROVIDER.into(),
            lanes,
            status,
            refresh_failure_status: refresh_failure.map(str::to_string),
            account_display_name: None,
        }
    }

    fn rule(event: HookEventType, threshold: Option<f64>, provider: Option<&str>) -> HookRule {
        HookRule {
            enabled: true,
            event: Some(event),
            events: Vec::new(),
            provider: provider.map(str::to_string),
            threshold,
            executable: PathBuf::from("/bin/true"),
            arguments: Vec::new(),
            timeout_secs: 10,
        }
    }

    fn config(enabled: bool, rules: Option<Vec<HookRule>>) -> HooksConfig {
        HooksConfig {
            enabled,
            events: rules.unwrap_or_else(|| {
                vec![
                    rule(HookEventType::QuotaLow, None, None),
                    rule(HookEventType::QuotaReached, None, None),
                    rule(HookEventType::QuotaReset, None, None),
                    rule(HookEventType::ProviderUnavailable, None, None),
                    rule(HookEventType::ProviderRecovered, None, None),
                    rule(HookEventType::RefreshFailed, None, None),
                ]
            }),
        }
    }

    fn events_of(dispatches: &[HookDispatch]) -> Vec<HookEventType> {
        dispatches.iter().map(|d| d.event.event).collect()
    }

    #[test]
    fn first_sample_establishes_baseline_without_firing() {
        let mut detector = HookTransitionDetector::new();
        let cfg = config(true, None);
        let dispatches = detector.evaluate(
            &observation(
                vec![lane(
                    Some(95.0),
                    lane_key(HookQuotaWindow::Session, None),
                    None,
                    &[0.8],
                    false,
                )],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        assert!(dispatches.is_empty());
    }

    #[test]
    fn quota_low_fires_once_on_upward_crossing() {
        let mut detector = HookTransitionDetector::new();
        let cfg = config(true, None);
        let key = lane_key(HookQuotaWindow::Session, None);
        let _ = detector.evaluate(
            &observation(
                vec![lane(Some(50.0), key.clone(), None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );

        let crossing = detector.evaluate(
            &observation(
                vec![lane(Some(85.0), key.clone(), None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        assert_eq!(events_of(&crossing), vec![HookEventType::QuotaLow]);

        let persisting = detector.evaluate(
            &observation(
                vec![lane(Some(90.0), key, None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        assert!(persisting.is_empty());
    }

    #[test]
    fn quota_low_dispatches_only_rule_whose_threshold_crossed() {
        let mut detector = HookTransitionDetector::new();
        let cfg = config(
            true,
            Some(vec![
                rule(HookEventType::QuotaLow, Some(0.5), None),
                rule(HookEventType::QuotaLow, Some(0.8), None),
            ]),
        );
        let key = lane_key(HookQuotaWindow::Session, None);
        let _ = detector.evaluate(
            &observation(
                vec![lane(Some(60.0), key.clone(), None, &[], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );

        let dispatches = detector.evaluate(
            &observation(
                vec![lane(Some(85.0), key, None, &[], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        assert_eq!(dispatches.len(), 1);
        let rules = dispatches[0].rules.as_ref().expect("narrowed rules");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].threshold, Some(0.8));
    }

    #[test]
    fn quota_reached_fires_on_upward_edge_only() {
        let mut detector = HookTransitionDetector::new();
        let cfg = config(true, None);
        let key = lane_key(HookQuotaWindow::Session, None);
        let _ = detector.evaluate(
            &observation(
                vec![lane(Some(90.0), key.clone(), None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );

        let reached = detector.evaluate(
            &observation(
                vec![lane(Some(100.0), key.clone(), None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        assert!(
            reached
                .iter()
                .any(|d| d.event.event == HookEventType::QuotaReached)
        );

        let still_full = detector.evaluate(
            &observation(
                vec![lane(Some(100.0), key, None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        assert!(
            !still_full
                .iter()
                .any(|d| d.event.event == HookEventType::QuotaReached)
        );
    }

    #[test]
    fn quota_reached_never_fires_for_weekly_lane() {
        let mut detector = HookTransitionDetector::new();
        let cfg = config(true, None);
        let key = lane_key(HookQuotaWindow::Weekly, None);
        let _ = detector.evaluate(
            &observation(
                vec![lane(Some(90.0), key.clone(), None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        let dispatches = detector.evaluate(
            &observation(
                vec![lane(Some(100.0), key, None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        assert!(
            !dispatches
                .iter()
                .any(|d| d.event.event == HookEventType::QuotaReached)
        );
    }

    #[test]
    fn quota_reset_fires_when_reset_boundary_advances() {
        let mut detector = HookTransitionDetector::new();
        let cfg = config(true, None);
        let key = lane_key(HookQuotaWindow::Session, None);
        let first = Utc.timestamp_opt(1_000_000, 0).unwrap();
        let second = first + chrono::Duration::seconds(18_000);

        let _ = detector.evaluate(
            &observation(
                vec![lane(Some(100.0), key.clone(), Some(first), &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        let dispatches = detector.evaluate(
            &observation(
                vec![lane(Some(0.0), key, Some(second), &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        assert_eq!(events_of(&dispatches), vec![HookEventType::QuotaReset]);
    }

    #[test]
    fn quota_reset_fires_on_usage_drop_without_boundary() {
        let mut detector = HookTransitionDetector::new();
        let cfg = config(true, None);
        let key = lane_key(HookQuotaWindow::Session, None);
        let _ = detector.evaluate(
            &observation(
                vec![lane(Some(95.0), key.clone(), None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        let dispatches = detector.evaluate(
            &observation(
                vec![lane(Some(10.0), key, None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        assert_eq!(events_of(&dispatches), vec![HookEventType::QuotaReset]);
    }

    #[test]
    fn reset_suppresses_depletion_edge_in_same_poll() {
        let mut detector = HookTransitionDetector::new();
        let cfg = config(true, None);
        let key = lane_key(HookQuotaWindow::Session, None);
        let _ = detector.evaluate(
            &observation(
                vec![lane(Some(95.0), key.clone(), None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        let dispatches = detector.evaluate(
            &observation(
                vec![lane(Some(5.0), key, None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        assert!(
            !dispatches
                .iter()
                .any(|d| d.event.event == HookEventType::QuotaReached)
        );
        assert!(
            !dispatches
                .iter()
                .any(|d| d.event.event == HookEventType::QuotaLow)
        );
    }

    #[test]
    fn provider_status_fires_outage_and_recovery_edges() {
        let mut detector = HookTransitionDetector::new();
        let cfg = config(true, None);
        let _ = detector.evaluate(&observation(vec![], HookProviderStatus::None, None), &cfg);

        let outage = detector.evaluate(&observation(vec![], HookProviderStatus::Major, None), &cfg);
        assert_eq!(events_of(&outage), vec![HookEventType::ProviderUnavailable]);

        let persisting = detector.evaluate(
            &observation(vec![], HookProviderStatus::Critical, None),
            &cfg,
        );
        assert!(persisting.is_empty());

        let recovered =
            detector.evaluate(&observation(vec![], HookProviderStatus::None, None), &cfg);
        assert_eq!(
            events_of(&recovered),
            vec![HookEventType::ProviderRecovered]
        );
    }

    #[test]
    fn unknown_and_maintenance_never_flip_status_state() {
        let mut detector = HookTransitionDetector::new();
        let cfg = config(true, None);
        let _ = detector.evaluate(&observation(vec![], HookProviderStatus::None, None), &cfg);

        assert!(
            detector
                .evaluate(
                    &observation(vec![], HookProviderStatus::Unknown, None),
                    &cfg
                )
                .is_empty()
        );
        assert!(
            detector
                .evaluate(
                    &observation(vec![], HookProviderStatus::Maintenance, None),
                    &cfg
                )
                .is_empty()
        );

        let outage = detector.evaluate(&observation(vec![], HookProviderStatus::Major, None), &cfg);
        assert_eq!(events_of(&outage), vec![HookEventType::ProviderUnavailable]);
    }

    #[test]
    fn first_definite_status_does_not_fire() {
        let mut detector = HookTransitionDetector::new();
        let dispatches = detector.evaluate(
            &observation(vec![], HookProviderStatus::Critical, None),
            &config(true, None),
        );
        assert!(dispatches.is_empty());
    }

    #[test]
    fn refresh_failure_emits_coarse_status_only() {
        let mut detector = HookTransitionDetector::new();
        let dispatches = detector.evaluate(
            &observation(vec![], HookProviderStatus::Unknown, Some("timeout")),
            &config(true, None),
        );
        assert_eq!(events_of(&dispatches), vec![HookEventType::RefreshFailed]);
        assert_eq!(dispatches[0].event.status.as_deref(), Some("timeout"));
    }

    #[test]
    fn refresh_failure_does_not_disturb_quota_baselines() {
        let mut detector = HookTransitionDetector::new();
        let cfg = config(true, None);
        let key = lane_key(HookQuotaWindow::Session, None);
        let _ = detector.evaluate(
            &observation(
                vec![lane(Some(50.0), key.clone(), None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        let _ = detector.evaluate(
            &observation(vec![], HookProviderStatus::Unknown, Some("offline")),
            &cfg,
        );
        let dispatches = detector.evaluate(
            &observation(
                vec![lane(Some(85.0), key, None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        assert!(
            dispatches
                .iter()
                .any(|d| d.event.event == HookEventType::QuotaLow)
        );
    }

    #[test]
    fn disabled_hooks_produce_no_events() {
        let mut detector = HookTransitionDetector::new();
        let cfg = config(false, None);
        let key = lane_key(HookQuotaWindow::Session, None);
        let _ = detector.evaluate(
            &observation(
                vec![lane(Some(50.0), key.clone(), None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        let dispatches = detector.evaluate(
            &observation(
                vec![lane(Some(95.0), key, None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        assert!(dispatches.is_empty());
    }

    #[test]
    fn configuration_change_clears_baselines() {
        let mut detector = HookTransitionDetector::new();
        let cfg = config(true, None);
        let key = lane_key(HookQuotaWindow::Session, None);
        detector.reset_if_configuration_changed(1);
        let _ = detector.evaluate(
            &observation(
                vec![lane(Some(50.0), key.clone(), None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        detector.reset_if_configuration_changed(2);
        let dispatches = detector.evaluate(
            &observation(
                vec![lane(Some(95.0), key, None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        assert!(dispatches.is_empty());
    }

    #[test]
    fn synthetic_placeholder_lane_never_fires() {
        let mut detector = HookTransitionDetector::new();
        let cfg = config(true, None);
        let key = lane_key(HookQuotaWindow::Session, None);
        let _ = detector.evaluate(
            &observation(
                vec![lane(Some(50.0), key.clone(), None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        let dispatches = detector.evaluate(
            &observation(
                vec![lane(Some(100.0), key, None, &[0.8], true)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        assert!(dispatches.is_empty());
    }

    #[test]
    fn disappearing_lane_resets_baseline() {
        let mut detector = HookTransitionDetector::new();
        let cfg = config(true, None);
        let key = lane_key(HookQuotaWindow::Session, None);
        let _ = detector.evaluate(
            &observation(
                vec![lane(Some(50.0), key.clone(), None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        let _ = detector.evaluate(
            &observation(vec![], HookProviderStatus::Unknown, None),
            &cfg,
        );
        let dispatches = detector.evaluate(
            &observation(
                vec![lane(Some(95.0), key, None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        assert!(dispatches.is_empty());
    }

    #[test]
    fn accounts_on_same_provider_track_independently() {
        let mut detector = HookTransitionDetector::new();
        let cfg = config(true, None);
        let first = lane_key(HookQuotaWindow::Session, Some("a@example.com"));
        let second = lane_key(HookQuotaWindow::Session, Some("b@example.com"));
        let _ = detector.evaluate(
            &observation(
                vec![
                    lane(Some(50.0), first.clone(), None, &[0.8], false),
                    lane(Some(50.0), second.clone(), None, &[0.8], false),
                ],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        let dispatches = detector.evaluate(
            &observation(
                vec![
                    lane(Some(85.0), first, None, &[0.8], false),
                    lane(Some(55.0), second, None, &[0.8], false),
                ],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].event.event, HookEventType::QuotaLow);
    }

    #[test]
    fn quota_low_respects_explicit_rule_threshold_over_fallback() {
        let mut detector = HookTransitionDetector::new();
        let cfg = config(
            true,
            Some(vec![rule(HookEventType::QuotaLow, Some(0.9), None)]),
        );
        let key = lane_key(HookQuotaWindow::Session, None);
        let _ = detector.evaluate(
            &observation(
                vec![lane(Some(50.0), key.clone(), None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        let below = detector.evaluate(
            &observation(
                vec![lane(Some(85.0), key.clone(), None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        assert!(below.is_empty());
        let above = detector.evaluate(
            &observation(
                vec![lane(Some(95.0), key, None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        assert_eq!(events_of(&above), vec![HookEventType::QuotaLow]);
    }

    #[test]
    fn quota_low_ignores_rules_scoped_to_another_provider() {
        let mut detector = HookTransitionDetector::new();
        let cfg = config(
            true,
            Some(vec![rule(HookEventType::QuotaLow, None, Some("claude"))]),
        );
        let key = lane_key(HookQuotaWindow::Session, None);
        let _ = detector.evaluate(
            &observation(
                vec![lane(Some(50.0), key.clone(), None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        let dispatches = detector.evaluate(
            &observation(
                vec![lane(Some(95.0), key, None, &[0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
        );
        assert!(dispatches.is_empty());
    }

    #[test]
    fn rate_limiter_suppresses_duplicate_refresh_failed() {
        use super::super::hooks::HookRateLimiter;
        use std::time::Duration;

        let limiter = HookRateLimiter::new(Duration::from_secs(600));
        let event = HookEvent::new(HookEventType::RefreshFailed, "codex").with_status("timeout");
        assert!(limiter.allow(&event));
        assert!(!limiter.allow(&event));

        // Quota events are not rate-limited by HookEventType::is_rate_limited.
        assert!(!HookEventType::QuotaLow.is_rate_limited());
        assert!(HookEventType::RefreshFailed.is_rate_limited());
        assert!(HookEventType::ProviderUnavailable.is_rate_limited());
    }
}
