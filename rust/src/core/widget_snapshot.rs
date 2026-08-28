//! Widget Snapshot
//!
//! Data export structures for widgets and external integrations.
//! Provides a serializable snapshot of all provider usage data.

#![allow(
    dead_code,
    reason = "widget snapshot types reserved for future UI integration"
)]

use crate::core::{ProviderId, RateWindow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Token usage summary for a provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsageSummary {
    /// Session cost in USD
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_cost_usd: Option<f64>,
    /// Session token count
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_tokens: Option<i64>,
    /// Last 30 days cost in USD
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_30_days_cost_usd: Option<f64>,
    /// Last 30 days token count
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_30_days_tokens: Option<i64>,
}

impl TokenUsageSummary {
    pub fn new() -> Self {
        Self {
            session_cost_usd: None,
            session_tokens: None,
            last_30_days_cost_usd: None,
            last_30_days_tokens: None,
        }
    }

    pub fn with_session(mut self, cost_usd: f64, tokens: i64) -> Self {
        self.session_cost_usd = Some(cost_usd);
        self.session_tokens = Some(tokens);
        self
    }

    pub fn with_last_30_days(mut self, cost_usd: f64, tokens: i64) -> Self {
        self.last_30_days_cost_usd = Some(cost_usd);
        self.last_30_days_tokens = Some(tokens);
        self
    }
}

impl Default for TokenUsageSummary {
    fn default() -> Self {
        Self::new()
    }
}

/// Daily usage data point
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyUsagePoint {
    /// Day key in yyyy-MM-dd format
    pub day_key: String,
    /// Total tokens used
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<i64>,
    /// Cost in USD
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

impl DailyUsagePoint {
    pub fn new(day_key: impl Into<String>) -> Self {
        Self {
            day_key: day_key.into(),
            total_tokens: None,
            cost_usd: None,
        }
    }

    pub fn with_tokens(mut self, tokens: i64) -> Self {
        self.total_tokens = Some(tokens);
        self
    }

    pub fn with_cost(mut self, cost_usd: f64) -> Self {
        self.cost_usd = Some(cost_usd);
        self
    }
}

/// Widget entry for a single provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidgetProviderEntry {
    /// Provider identifier
    pub provider: ProviderId,
    /// When this data was last updated
    pub updated_at: DateTime<Utc>,
    /// Primary rate limit
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary: Option<RateWindow>,
    /// Secondary rate limit
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary: Option<RateWindow>,
    /// Tertiary rate limit
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tertiary: Option<RateWindow>,
    /// Credits remaining
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits_remaining: Option<f64>,
    /// Code review remaining percentage (0-100)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_review_remaining_percent: Option<f64>,
    /// Token usage summary
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<TokenUsageSummary>,
    /// Daily usage data points
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub daily_usage: Vec<DailyUsagePoint>,
    /// Account email if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
    /// Login method/plan info (e.g., "Claude Pro", "Claude Max")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login_method: Option<String>,
}

impl WidgetProviderEntry {
    pub fn new(provider: ProviderId, updated_at: DateTime<Utc>) -> Self {
        Self {
            provider,
            updated_at,
            primary: None,
            secondary: None,
            tertiary: None,
            credits_remaining: None,
            code_review_remaining_percent: None,
            token_usage: None,
            daily_usage: Vec::new(),
            account_email: None,
            login_method: None,
        }
    }

    pub fn with_primary(mut self, rate: RateWindow) -> Self {
        self.primary = Some(rate);
        self
    }

    pub fn with_secondary(mut self, rate: RateWindow) -> Self {
        self.secondary = Some(rate);
        self
    }

    pub fn with_tertiary(mut self, rate: RateWindow) -> Self {
        self.tertiary = Some(rate);
        self
    }

    pub fn with_credits_remaining(mut self, credits: f64) -> Self {
        self.credits_remaining = Some(credits);
        self
    }

    pub fn with_code_review_remaining(mut self, percent: f64) -> Self {
        self.code_review_remaining_percent = Some(percent);
        self
    }

    pub fn with_token_usage(mut self, summary: TokenUsageSummary) -> Self {
        self.token_usage = Some(summary);
        self
    }

    pub fn with_daily_usage(mut self, points: Vec<DailyUsagePoint>) -> Self {
        self.daily_usage = points;
        self
    }

    pub fn with_account_email(mut self, email: impl Into<String>) -> Self {
        self.account_email = Some(email.into());
        self
    }

    pub fn with_login_method(mut self, method: impl Into<String>) -> Self {
        self.login_method = Some(method.into());
        self
    }
}

/// Complete widget snapshot with all provider data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidgetSnapshot {
    /// Provider entries
    pub entries: Vec<WidgetProviderEntry>,
    /// List of enabled providers
    pub enabled_providers: Vec<ProviderId>,
    /// When this snapshot was generated
    pub generated_at: DateTime<Utc>,
}

impl WidgetSnapshot {
    pub fn new(entries: Vec<WidgetProviderEntry>, generated_at: DateTime<Utc>) -> Self {
        let enabled_providers = entries.iter().map(|e| e.provider).collect();
        Self {
            entries,
            enabled_providers,
            generated_at,
        }
    }

    pub fn with_enabled_providers(mut self, providers: Vec<ProviderId>) -> Self {
        self.enabled_providers = providers;
        self
    }

    /// Get entry for a specific provider
    pub fn entry_for(&self, provider: ProviderId) -> Option<&WidgetProviderEntry> {
        self.entries.iter().find(|e| e.provider == provider)
    }

    /// Check if a provider is enabled
    pub fn is_enabled(&self, provider: ProviderId) -> bool {
        self.enabled_providers.contains(&provider)
    }
}

/// Widget snapshot store
pub struct WidgetSnapshotStore;

impl WidgetSnapshotStore {
    const FILENAME: &'static str = "widget-snapshot.json";

    /// Load widget snapshot from disk
    pub fn load() -> Option<WidgetSnapshot> {
        let path = Self::snapshot_path()?;
        let data = fs::read_to_string(&path).ok()?;
        serde_json::from_str(&data).ok()
    }

    /// Save widget snapshot to disk
    pub fn save(snapshot: &WidgetSnapshot) -> Result<(), WidgetSnapshotError> {
        let path = Self::snapshot_path().ok_or(WidgetSnapshotError::PathNotAvailable)?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string_pretty(snapshot)?;
        fs::write(&path, json)?;

        tracing::debug!("Saved widget snapshot to {:?}", path);
        Ok(())
    }

    /// Clear widget snapshot
    pub fn clear() {
        if let Some(path) = Self::snapshot_path() {
            // Best-effort cleanup; a missing snapshot file is not an error.
            let _cleared_snapshot = fs::remove_file(path);
        }
    }

    fn snapshot_path() -> Option<PathBuf> {
        dirs::data_local_dir().map(|d| d.join("CodexBar").join(Self::FILENAME))
    }
}

/// Widget selection store for tracking user's selected provider
pub struct WidgetSelectionStore;

impl WidgetSelectionStore {
    const FILENAME: &'static str = "widget-selection.json";

    /// Load selected provider
    pub fn load_selected_provider() -> Option<ProviderId> {
        let path = Self::selection_path()?;
        let data = fs::read_to_string(&path).ok()?;

        #[derive(Deserialize)]
        struct Selection {
            provider: ProviderId,
        }

        let selection: Selection = serde_json::from_str(&data).ok()?;
        Some(selection.provider)
    }

    /// Save selected provider
    pub fn save_selected_provider(provider: ProviderId) -> Result<(), WidgetSnapshotError> {
        let path = Self::selection_path().ok_or(WidgetSnapshotError::PathNotAvailable)?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        #[derive(Serialize)]
        struct Selection {
            provider: ProviderId,
        }

        let json = serde_json::to_string(&Selection { provider })?;
        fs::write(&path, json)?;
        Ok(())
    }

    fn selection_path() -> Option<PathBuf> {
        dirs::data_local_dir().map(|d| d.join("CodexBar").join(Self::FILENAME))
    }
}

/// Widget snapshot errors
#[derive(Debug, thiserror::Error)]
pub enum WidgetSnapshotError {
    #[error("Path not available")]
    PathNotAvailable,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_usage_summary() {
        let summary = TokenUsageSummary::new()
            .with_session(1.50, 10000)
            .with_last_30_days(45.00, 300000);

        assert_eq!(summary.session_cost_usd, Some(1.50));
        assert_eq!(summary.session_tokens, Some(10000));
        assert_eq!(summary.last_30_days_cost_usd, Some(45.00));
        assert_eq!(summary.last_30_days_tokens, Some(300000));
    }

    #[test]
    fn test_daily_usage_point() {
        let point = DailyUsagePoint::new("2026-01-15")
            .with_tokens(5000)
            .with_cost(0.75);

        assert_eq!(point.day_key, "2026-01-15");
        assert_eq!(point.total_tokens, Some(5000));
        assert_eq!(point.cost_usd, Some(0.75));
    }

    #[test]
    fn test_widget_provider_entry() {
        let entry =
            WidgetProviderEntry::new(ProviderId::Claude, Utc::now()).with_credits_remaining(100.0);

        assert_eq!(entry.provider, ProviderId::Claude);
        assert_eq!(entry.credits_remaining, Some(100.0));
    }

    #[test]
    fn test_widget_snapshot() {
        let entry = WidgetProviderEntry::new(ProviderId::Codex, Utc::now());
        let snapshot = WidgetSnapshot::new(vec![entry], Utc::now());

        assert_eq!(snapshot.entries.len(), 1);
        assert!(snapshot.is_enabled(ProviderId::Codex));
        assert!(!snapshot.is_enabled(ProviderId::Claude));
    }
}
