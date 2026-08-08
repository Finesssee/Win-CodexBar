//! Rate window model - represents a usage limit window (e.g., 5-hour session, 7-day weekly)

use super::session_equivalent_forecast::{
    MONTHLY_WINDOW_MINUTES, SESSION_WINDOW_MINUTES, WEEKLY_WINDOW_MINUTES,
};
use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};

/// Duration cadence of a rate-limit window (upstream 0.48.0 F5).
///
/// Codex exposes three lanes: a 5-hour session, a 7-day weekly, and a 30-day
/// monthly window. Centralizing the cadence here keeps duration-first labels
/// and bucketing consistent across the ambient provider, the managed multi-
/// account stack, and the CLI/tray surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RateWindowCadence {
    /// 5-hour session window (300 minutes).
    Session,
    /// 7-day weekly window (10 080 minutes).
    Weekly,
    /// 30-day monthly window (43 200 minutes).
    Monthly,
    /// Any other window length the upstream buckets don't recognize.
    Unknown,
}

impl RateWindowCadence {
    /// Classify a window length given in minutes into a cadence.
    ///
    /// Bucketing (upstream `RateLane` + `classifyRateWindow`):
    /// - exactly 300 → Session
    /// - 43 200+    → Monthly
    /// - 10 080..<43 200 → Weekly
    /// - anything else → Unknown
    pub fn from_minutes(minutes: u32) -> Self {
        if minutes == SESSION_WINDOW_MINUTES {
            Self::Session
        } else if minutes >= MONTHLY_WINDOW_MINUTES {
            Self::Monthly
        } else if minutes >= WEEKLY_WINDOW_MINUTES {
            Self::Weekly
        } else {
            Self::Unknown
        }
    }

    /// Classify from seconds (the API and `UsageWindowSnapshot` report seconds).
    pub fn from_seconds(seconds: i64) -> Self {
        if seconds <= 0 {
            return Self::Unknown;
        }
        Self::from_minutes(((seconds + 59) / 60) as u32)
    }

    /// Human-readable label key for this cadence (matches upstream lane names).
    pub fn label_key(&self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
            Self::Unknown => "unknown",
        }
    }
}

/// Represents a rate limit window with usage percentage and reset time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateWindow {
    /// Percentage of the window that has been used (0-100)
    pub used_percent: f64,

    /// Duration of the window in minutes (e.g., 300 for 5-hour, 10080 for 7-day)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<u32>,

    /// When the window resets
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<DateTime<Utc>>,

    /// Human-readable reset description (e.g., "Jan 15 at 3:00pm")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_description: Option<String>,

    /// Whether this row is an informational value rather than a quota.
    #[serde(default)]
    pub is_informational: bool,
}

impl RateWindow {
    /// Create a new rate window
    pub fn new(used_percent: f64) -> Self {
        Self {
            used_percent: Self::finite_percent(used_percent),
            window_minutes: None,
            resets_at: None,
            reset_description: None,
            is_informational: false,
        }
    }

    /// Create an informational row without implying a percentage quota.
    pub fn informational(description: impl Into<String>) -> Self {
        Self {
            reset_description: Some(description.into()),
            is_informational: true,
            ..Self::new(0.0)
        }
    }

    /// Informational placeholder for an absent 5-hour session lane.
    ///
    /// Weekly-only plans (Codex, Claude web) occasionally report no active
    /// session window. Informational primaries are omitted from quota math
    /// and rendered as unavailable, so one canonical shape keeps providers
    /// and downstream surfaces from diverging.
    pub fn no_active_session() -> Self {
        Self {
            window_minutes: Some(SESSION_WINDOW_MINUTES),
            ..Self::informational("No active 5h session")
        }
    }

    /// Create a rate window with full details
    pub fn with_details(
        used_percent: f64,
        window_minutes: Option<u32>,
        resets_at: Option<DateTime<Utc>>,
        reset_description: Option<String>,
    ) -> Self {
        Self {
            used_percent: Self::finite_percent(used_percent),
            window_minutes,
            resets_at,
            reset_description,
            is_informational: false,
        }
    }

    /// Real UTC Gregorian month length ending at `resets_at`, in minutes.
    ///
    /// Mirrors upstream `ProviderPaceCapability.inferredMonthlyWindowMinutes`
    /// (reset − 1 calendar month). Used so monthly pace scores the actual cycle
    /// (28–31 days) instead of a flat 30-day sentinel.
    pub fn calendar_month_window_minutes(resets_at: DateTime<Utc>) -> Option<u32> {
        let start = subtract_one_calendar_month(resets_at)?;
        let minutes = (resets_at - start).num_minutes();
        if minutes > 0 {
            Some(minutes as u32)
        } else {
            None
        }
    }

    /// Monthly window minutes from a known reset. Returns `None` when there is
    /// no reset (upstream leaves windowMinutes unset in that case).
    pub fn monthly_window_minutes(resets_at: Option<DateTime<Utc>>) -> Option<u32> {
        resets_at.and_then(Self::calendar_month_window_minutes)
    }

    /// Get the remaining percentage (100 - used)
    pub fn remaining_percent(&self) -> f64 {
        100.0 - self.used_percent
    }

    /// Check if the window is exhausted (>= 100% used)
    pub fn is_exhausted(&self) -> bool {
        self.used_percent >= 100.0
    }

    /// Check if the window is nearly exhausted (>= 90% used)
    pub fn is_nearly_exhausted(&self) -> bool {
        self.used_percent >= 90.0
    }

    /// Format the reset time as a countdown string
    pub fn format_countdown(&self) -> Option<String> {
        let resets_at = self.resets_at?;
        let now = Utc::now();

        if resets_at <= now {
            return Some("now".to_string());
        }

        let duration = resets_at - now;
        let hours = duration.num_hours();
        let total_minutes = ((duration.num_seconds() + 59) / 60).max(1);
        let minutes = total_minutes % 60;

        if hours > 24 {
            let days = hours / 24;
            Some(format!("{}d {}h", days, hours % 24))
        } else if hours > 0 {
            Some(format!("{}h {}m", hours, minutes))
        } else {
            Some(format!("{}m", minutes))
        }
    }

    fn finite_percent(value: f64) -> f64 {
        if value.is_finite() {
            value.clamp(0.0, 100.0)
        } else {
            0.0
        }
    }
}

/// Subtract one Gregorian calendar month in UTC (upstream Calendar.date(byAdding: .month, -1)).
fn subtract_one_calendar_month(dt: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let y = dt.year();
    let m = dt.month();
    let (py, pm) = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
    let max_day = days_in_month(py, pm);
    let day = dt.day().min(max_day);
    dt.date_naive()
        .with_year(py)?
        .with_month(pm)?
        .with_day(day)
        .map(|d| d.and_time(dt.time()).and_utc())
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if chrono::NaiveDate::from_ymd_opt(year, 2, 29).is_some() {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

impl Default for RateWindow {
    fn default() -> Self {
        Self::new(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn no_active_session_is_informational_five_hour_placeholder() {
        let window = RateWindow::no_active_session();

        assert!(window.is_informational);
        assert_eq!(window.window_minutes, Some(SESSION_WINDOW_MINUTES));
        assert_eq!(
            window.reset_description.as_deref(),
            Some("No active 5h session")
        );
        assert_eq!(window.used_percent, 0.0);
        assert_eq!(window.resets_at, None);
    }

    #[test]
    fn test_remaining_percent() {
        let window = RateWindow::new(75.0);
        assert!((window.remaining_percent() - 25.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_clamping() {
        let window = RateWindow::new(150.0);
        assert!((window.used_percent - 100.0).abs() < f64::EPSILON);

        let window = RateWindow::new(-10.0);
        assert!(window.used_percent.abs() < f64::EPSILON);
    }

    #[test]
    fn test_exhausted() {
        assert!(RateWindow::new(100.0).is_exhausted());
        assert!(!RateWindow::new(99.0).is_exhausted());
    }

    #[test]
    fn countdown_uses_one_minute_for_sub_minute_future_reset() {
        let window = RateWindow::with_details(
            10.0,
            None,
            Some(Utc::now() + chrono::Duration::seconds(30)),
            None,
        );

        assert_eq!(window.format_countdown().as_deref(), Some("1m"));
    }

    #[test]
    fn calendar_month_window_uses_real_cycle_length() {
        // March 1 2026 ends a 28-day February cycle (upstream ProviderPaceCapabilityTests).
        let resets = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();
        assert_eq!(
            RateWindow::calendar_month_window_minutes(resets),
            Some(28 * 24 * 60)
        );
        // 31-day cycle ending Aug 1.
        let resets = Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap();
        assert_eq!(
            RateWindow::calendar_month_window_minutes(resets),
            Some(31 * 24 * 60)
        );
        assert_eq!(RateWindow::monthly_window_minutes(None), None);
    }

    #[test]
    fn cadence_boundary_session_exactly_300() {
        assert_eq!(
            RateWindowCadence::from_minutes(300),
            RateWindowCadence::Session
        );
    }

    #[test]
    fn cadence_boundary_weekly_10080() {
        assert_eq!(
            RateWindowCadence::from_minutes(10_080),
            RateWindowCadence::Weekly
        );
    }

    #[test]
    fn cadence_boundary_below_monthly_43199_is_weekly() {
        assert_eq!(
            RateWindowCadence::from_minutes(43_199),
            RateWindowCadence::Weekly
        );
    }

    #[test]
    fn cadence_boundary_monthly_43200() {
        assert_eq!(
            RateWindowCadence::from_minutes(43_200),
            RateWindowCadence::Monthly
        );
    }

    #[test]
    fn cadence_from_seconds_rounding() {
        // from_seconds rounds up: (seconds + 59) / 60
        // 300 min = 18000s exactly → Session
        assert_eq!(
            RateWindowCadence::from_seconds(18_000),
            RateWindowCadence::Session
        );
        // 18001s → (18001+59)/60 = 301 min → Unknown (not exactly 300)
        assert_eq!(
            RateWindowCadence::from_seconds(18_001),
            RateWindowCadence::Unknown
        );
        // 10080 min = 604800s → Weekly
        assert_eq!(
            RateWindowCadence::from_seconds(604_800),
            RateWindowCadence::Weekly
        );
        // 43200 min = 2592000s → Monthly
        assert_eq!(
            RateWindowCadence::from_seconds(2_592_000),
            RateWindowCadence::Monthly
        );
        // 0 or negative → Unknown
        assert_eq!(
            RateWindowCadence::from_seconds(0),
            RateWindowCadence::Unknown
        );
        assert_eq!(
            RateWindowCadence::from_seconds(-1),
            RateWindowCadence::Unknown
        );
    }

    #[test]
    fn cadence_label_keys() {
        assert_eq!(RateWindowCadence::Session.label_key(), "session");
        assert_eq!(RateWindowCadence::Weekly.label_key(), "weekly");
        assert_eq!(RateWindowCadence::Monthly.label_key(), "monthly");
        assert_eq!(RateWindowCadence::Unknown.label_key(), "unknown");
    }
}
