//! Usage snapshot model - represents a point-in-time usage state for a provider

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::RateWindow;

/// Provider-specific operational data reported by a Wayfinder gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WayfinderUsageSnapshot {
    pub gateway_status: String,
    pub offline: bool,
    pub dry_run: bool,
    pub missing_keys: Vec<String>,
    pub model_count: usize,
    pub models: Vec<String>,
    pub requests: u64,
    pub estimated_requests: u64,
    pub tokens: u64,
    pub realized: f64,
    pub baseline: f64,
    pub saved: f64,
    pub saved_percent: f64,
    pub period_days: u32,
    pub unit: String,
    pub priced: bool,
    pub routes: Vec<WayfinderRouteSummary>,
}

/// Per-route savings data reported by Wayfinder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WayfinderRouteSummary {
    pub name: String,
    pub requests: u64,
    pub tokens: u64,
    pub realized: f64,
    pub baseline: f64,
    pub saved: f64,
}

/// A labeled extra usage window surfaced by provider APIs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedRateWindow {
    pub id: String,
    pub title: String,
    pub window: RateWindow,
    /// Whether the provider explicitly reported usage for this lane.
    /// In-memory presentation metadata only; external snapshot JSON stays stable.
    #[serde(default = "named_rate_window_usage_known_default", skip_serializing)]
    pub usage_known: bool,
}

fn named_rate_window_usage_known_default() -> bool {
    true
}

impl NamedRateWindow {
    pub fn new(id: impl Into<String>, title: impl Into<String>, window: RateWindow) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            window,
            usage_known: true,
        }
    }

    pub fn with_usage_known(mut self, usage_known: bool) -> Self {
        self.usage_known = usage_known;
        self
    }
}

/// A snapshot of usage data for a provider at a point in time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageSnapshot {
    /// Primary rate window (usually session-based, e.g., 5-hour for Claude)
    pub primary: RateWindow,

    /// Provider-resolved label for the primary rate window when the stable
    /// provider metadata label is not specific enough for this snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_label: Option<String>,

    /// Secondary rate window (usually weekly/monthly)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary: Option<RateWindow>,

    /// Provider-resolved label for the secondary rate window when metadata
    /// describes a model family rather than this snapshot's cadence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_label: Option<String>,

    /// Model-specific rate window (e.g., Opus quota for Claude)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_specific: Option<RateWindow>,

    /// Tertiary rate window (e.g., 30-day quota for Infini)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tertiary: Option<RateWindow>,

    /// Additional labeled windows that do not fit the primary/secondary/model slots.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_rate_windows: Vec<NamedRateWindow>,

    /// When this snapshot was captured
    pub updated_at: DateTime<Utc>,

    /// Account email if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,

    /// Account organization if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_organization: Option<String>,

    /// Login method/plan info (e.g., "Claude Pro", "Claude Max")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login_method: Option<String>,
}

impl UsageSnapshot {
    /// Create a new usage snapshot with just primary window
    pub fn new(primary: RateWindow) -> Self {
        Self {
            primary,
            primary_label: None,
            secondary: None,
            secondary_label: None,
            model_specific: None,
            tertiary: None,
            extra_rate_windows: Vec::new(),
            updated_at: Utc::now(),
            account_email: None,
            account_organization: None,
            login_method: None,
        }
    }

    /// Builder pattern: override the primary window label for this snapshot.
    pub fn with_primary_label(mut self, label: impl Into<String>) -> Self {
        self.primary_label = Some(label.into());
        self
    }

    /// Builder pattern: set secondary window
    pub fn with_secondary(mut self, secondary: RateWindow) -> Self {
        self.secondary = Some(secondary);
        self
    }

    /// Builder pattern: override the secondary window label for this snapshot.
    pub fn with_secondary_label(mut self, label: impl Into<String>) -> Self {
        self.secondary_label = Some(label.into());
        self
    }

    /// Builder pattern: set model-specific window
    pub fn with_model_specific(mut self, model_specific: RateWindow) -> Self {
        self.model_specific = Some(model_specific);
        self
    }

    /// Builder pattern: set tertiary window
    pub fn with_tertiary(mut self, tertiary: RateWindow) -> Self {
        self.tertiary = Some(tertiary);
        self
    }

    /// Builder pattern: append a labeled extra rate window
    pub fn with_extra_rate_window(
        mut self,
        id: impl Into<String>,
        title: impl Into<String>,
        window: RateWindow,
    ) -> Self {
        self.extra_rate_windows
            .push(NamedRateWindow::new(id, title, window));
        self
    }

    /// Builder pattern: set account email
    pub fn with_email(mut self, email: impl Into<String>) -> Self {
        self.account_email = Some(email.into());
        self
    }

    /// Builder pattern: set organization
    pub fn with_organization(mut self, org: impl Into<String>) -> Self {
        self.account_organization = Some(org.into());
        self
    }

    /// Builder pattern: set login method
    pub fn with_login_method(mut self, method: impl Into<String>) -> Self {
        self.login_method = Some(method.into());
        self
    }

    /// Get the most restrictive (highest used) rate window
    pub fn most_restrictive(&self) -> &RateWindow {
        let mut most = &self.primary;

        if let Some(ref secondary) = self.secondary
            && secondary.used_percent > most.used_percent
        {
            most = secondary;
        }

        if let Some(ref model_specific) = self.model_specific
            && model_specific.used_percent > most.used_percent
        {
            most = model_specific;
        }

        if let Some(ref tertiary) = self.tertiary
            && tertiary.used_percent > most.used_percent
        {
            most = tertiary;
        }

        for extra in &self.extra_rate_windows {
            if extra.window.used_percent > most.used_percent {
                most = &extra.window;
            }
        }

        most
    }

    /// Check if any rate window is exhausted
    pub fn any_exhausted(&self) -> bool {
        self.primary.is_exhausted()
            || self.secondary.as_ref().is_some_and(|w| w.is_exhausted())
            || self
                .model_specific
                .as_ref()
                .is_some_and(|w| w.is_exhausted())
            || self.tertiary.as_ref().is_some_and(|w| w.is_exhausted())
            || self
                .extra_rate_windows
                .iter()
                .any(|extra| extra.window.is_exhausted())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CostDailyPoint {
    pub day: String,
    pub amount: f64,
}

/// Cost/credits snapshot for providers that support it
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostSnapshot {
    /// Amount used in the current period
    pub used: f64,

    /// Limit for the current period (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,

    /// Currency code (e.g., "USD")
    pub currency_code: String,

    /// Optional currency symbol (e.g. "€", "$", "¥"). When present,
    /// surfaces carry it to the UI for localized currency rendering instead
    /// of deriving the symbol from the code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency_symbol: Option<String>,

    /// Period description (e.g., "Monthly", "Daily")
    pub period: String,

    /// When the period resets
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<DateTime<Utc>>,

    /// When this snapshot was captured
    pub updated_at: DateTime<Utc>,

    /// Remaining prepaid balance (currency units), separate from used/limit spend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance: Option<f64>,

    /// Exact daily spend points when the provider supplies them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub daily: Vec<CostDailyPoint>,

    /// Provider-owned presentation hint for spend that is itself a primary
    /// usage signal and must remain visible when optional local summaries are hidden.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub always_visible: bool,
}

impl CostSnapshot {
    /// Create a new cost snapshot
    pub fn new(used: f64, currency_code: impl Into<String>, period: impl Into<String>) -> Self {
        Self {
            used: finite_amount(used).unwrap_or(0.0),
            limit: None,
            currency_code: currency_code.into(),
            currency_symbol: None,
            period: period.into(),
            resets_at: None,
            updated_at: Utc::now(),
            balance: None,
            daily: Vec::new(),
            always_visible: false,
        }
    }

    /// Builder pattern: set limit
    pub fn with_limit(mut self, limit: f64) -> Self {
        self.limit = finite_amount(limit);
        self
    }

    /// Builder pattern: set remaining prepaid balance (finite, ≥ 0 only)
    pub fn with_balance(mut self, balance: f64) -> Self {
        self.balance = finite_amount(balance);
        self
    }

    pub fn with_daily(mut self, daily: Vec<CostDailyPoint>) -> Self {
        self.daily = daily;
        self
    }

    /// Keep this provider-metered spend visible even when optional local cost summaries are hidden.
    pub fn always_visible(mut self) -> Self {
        self.always_visible = true;
        self
    }

    /// Builder pattern: set currency symbol for localized rendering.
    pub fn with_currency_symbol(mut self, symbol: impl Into<String>) -> Self {
        self.currency_symbol = Some(symbol.into());
        self
    }

    /// Builder pattern: set reset time
    pub fn with_resets_at(mut self, resets_at: DateTime<Utc>) -> Self {
        self.resets_at = Some(resets_at);
        self
    }

    /// Get remaining amount if limit is set
    pub fn remaining(&self) -> Option<f64> {
        self.limit.map(|l| (l - self.used).max(0.0))
    }

    /// Get usage percentage if limit is set
    pub fn used_percent(&self) -> Option<f64> {
        self.limit.map(|l| {
            if l > 0.0 {
                (self.used / l * 100.0).min(100.0)
            } else {
                100.0
            }
        })
    }

    /// Format the cost as a currency string
    pub fn format_used(&self) -> String {
        format_currency(self.used, &self.currency_code)
    }

    /// Format the limit as a currency string
    pub fn format_limit(&self) -> Option<String> {
        self.limit.map(|l| format_currency(l, &self.currency_code))
    }

    /// Format the prepaid balance as a currency string
    pub fn format_balance(&self) -> Option<String> {
        self.balance
            .map(|b| format_currency(b, &self.currency_code))
    }
}

/// Format a value as currency
fn format_currency(value: f64, currency_code: &str) -> String {
    let value = finite_amount(value).unwrap_or(0.0);
    match currency_code.to_uppercase().as_str() {
        "USD" => format!("${:.2}", value),
        "EUR" => format!("€{:.2}", value),
        "GBP" => format!("£{:.2}", value),
        _ => format!("{:.2} {}", value, currency_code),
    }
}

fn finite_amount(value: f64) -> Option<f64> {
    value.is_finite().then_some(value.max(0.0))
}

/// Combined fetch result containing usage and optional cost data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderFetchResult {
    /// Usage data
    pub usage: UsageSnapshot,

    /// Cost/credits data if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<CostSnapshot>,

    /// Provider-specific operational data that is not quota or identity data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wayfinder_usage: Option<WayfinderUsageSnapshot>,

    /// Label describing the data source (e.g., "oauth", "web", "cli")
    pub source_label: String,
}

impl ProviderFetchResult {
    /// Create a new fetch result
    pub fn new(usage: UsageSnapshot, source_label: impl Into<String>) -> Self {
        Self {
            usage,
            cost: None,
            wayfinder_usage: None,
            source_label: source_label.into(),
        }
    }

    /// Builder pattern: set cost
    pub fn with_cost(mut self, cost: CostSnapshot) -> Self {
        self.cost = Some(cost);
        self
    }

    /// Builder pattern: set Wayfinder operational data.
    pub fn with_wayfinder_usage(mut self, usage: WayfinderUsageSnapshot) -> Self {
        self.wayfinder_usage = Some(usage);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_snapshot_ignores_non_finite_values() {
        let cost = CostSnapshot::new(f64::NAN, "USD", "Monthly").with_limit(f64::INFINITY);

        assert_eq!(cost.used, 0.0);
        assert_eq!(cost.limit, None);
        assert_eq!(cost.used_percent(), None);
        assert_eq!(cost.format_used(), "$0.00");
    }
}
