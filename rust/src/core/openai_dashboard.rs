//! OpenAI Dashboard Models
//!
//! Data structures for OpenAI/Codex dashboard usage breakdown and credits tracking.

// Serde data model mirroring the OpenAI dashboard payload plus its cache
// store; several fields and helpers have no consumer yet but must survive
// (de)serialization round-trips.
#![allow(
    dead_code,
    reason = "dashboard snapshot fields mirror the upstream OpenAI payload; not all are consumed yet"
)]

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crate::core::RateWindow;

/// OpenAI dashboard snapshot with usage and credits data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIDashboardSnapshot {
    /// Email of signed-in user
    pub signed_in_email: Option<String>,
    /// Code review remaining percentage (0-100)
    pub code_review_remaining_percent: Option<f64>,
    /// Credit events (purchases, usage, etc.)
    pub credit_events: Vec<CreditEvent>,
    /// Daily breakdown derived from credit events
    pub daily_breakdown: Vec<OpenAIDashboardDailyBreakdown>,
    /// Usage breakdown from dashboard chart (30 days)
    pub usage_breakdown: Vec<OpenAIDashboardDailyBreakdown>,
    /// URL to purchase more credits
    pub credits_purchase_url: Option<String>,
    /// Primary rate limit (e.g., session limit)
    pub primary_limit: Option<RateWindow>,
    /// Secondary rate limit (e.g., weekly limit)
    pub secondary_limit: Option<RateWindow>,
    /// Credits remaining
    pub credits_remaining: Option<f64>,
    /// Account plan name
    pub account_plan: Option<String>,
    /// When this snapshot was taken
    pub updated_at: DateTime<Utc>,
}

impl OpenAIDashboardSnapshot {
    pub fn new(
        signed_in_email: Option<String>,
        credit_events: Vec<CreditEvent>,
        updated_at: DateTime<Utc>,
    ) -> Self {
        let daily_breakdown = Self::make_daily_breakdown(&credit_events, 30);
        Self {
            signed_in_email,
            code_review_remaining_percent: None,
            credit_events,
            daily_breakdown,
            usage_breakdown: Vec::new(),
            credits_purchase_url: None,
            primary_limit: None,
            secondary_limit: None,
            credits_remaining: None,
            account_plan: None,
            updated_at,
        }
    }

    /// Create daily breakdown from credit events
    pub fn make_daily_breakdown(
        events: &[CreditEvent],
        max_days: usize,
    ) -> Vec<OpenAIDashboardDailyBreakdown> {
        if events.is_empty() {
            return Vec::new();
        }

        // Group by day and service
        let mut totals: HashMap<String, HashMap<String, f64>> = HashMap::new();

        for event in events {
            let day = event.date.format("%Y-%m-%d").to_string();
            let service_totals = totals.entry(day).or_default();
            *service_totals.entry(event.service.clone()).or_insert(0.0) += event.credits_used;
        }

        // Sort days descending and take max_days
        let mut sorted_days: Vec<_> = totals.keys().cloned().collect();
        sorted_days.sort_by(|a, b| b.cmp(a));
        sorted_days.truncate(max_days);

        sorted_days
            .into_iter()
            .map(|day| {
                let service_totals = totals.get(&day).cloned().unwrap_or_default();
                let mut services: Vec<_> = service_totals
                    .into_iter()
                    .map(|(service, credits_used)| OpenAIDashboardServiceUsage {
                        service,
                        credits_used,
                    })
                    .collect();

                // Sort by credits used descending, then by service name
                services.sort_by(|a, b| {
                    b.credits_used
                        .partial_cmp(&a.credits_used)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a.service.cmp(&b.service))
                });

                let total = services.iter().map(|s| s.credits_used).sum();
                OpenAIDashboardDailyBreakdown {
                    day,
                    services,
                    total_credits_used: total,
                }
            })
            .collect()
    }

    /// Set usage breakdown data
    pub fn with_usage_breakdown(mut self, breakdown: Vec<OpenAIDashboardDailyBreakdown>) -> Self {
        self.usage_breakdown = breakdown;
        self
    }

    /// Set primary limit
    pub fn with_primary_limit(mut self, limit: RateWindow) -> Self {
        self.primary_limit = Some(limit);
        self
    }

    /// Set secondary limit
    pub fn with_secondary_limit(mut self, limit: RateWindow) -> Self {
        self.secondary_limit = Some(limit);
        self
    }

    /// Set credits remaining
    pub fn with_credits_remaining(mut self, credits: f64) -> Self {
        self.credits_remaining = Some(credits);
        self
    }

    /// Set account plan
    pub fn with_account_plan(mut self, plan: impl Into<String>) -> Self {
        self.account_plan = Some(plan.into());
        self
    }
}

/// Credit event (purchase, usage, adjustment, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreditEvent {
    /// Date of the event
    pub date: NaiveDate,
    /// Service that used credits (e.g., "CLI", "API", "GitHub Code Review")
    pub service: String,
    /// Credits used (positive = consumption, negative = addition)
    pub credits_used: f64,
    /// Description of the event
    pub description: Option<String>,
}

impl CreditEvent {
    pub fn new(date: NaiveDate, service: impl Into<String>, credits_used: f64) -> Self {
        Self {
            date,
            service: service.into(),
            credits_used,
            description: None,
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// Daily usage breakdown
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAIDashboardDailyBreakdown {
    /// Day key in yyyy-MM-dd format
    pub day: String,
    /// Per-service usage
    pub services: Vec<OpenAIDashboardServiceUsage>,
    /// Total credits used this day
    pub total_credits_used: f64,
}

impl OpenAIDashboardDailyBreakdown {
    pub fn new(day: impl Into<String>, services: Vec<OpenAIDashboardServiceUsage>) -> Self {
        let total_credits_used = services.iter().map(|s| s.credits_used).sum();
        Self {
            day: day.into(),
            services,
            total_credits_used,
        }
    }
}

/// Per-service usage within a day
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAIDashboardServiceUsage {
    /// Service name
    pub service: String,
    /// Credits used
    pub credits_used: f64,
}

impl OpenAIDashboardServiceUsage {
    pub fn new(service: impl Into<String>, credits_used: f64) -> Self {
        Self {
            service: service.into(),
            credits_used,
        }
    }
}

/// Cached dashboard data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIDashboardCache {
    /// Account email for cache key
    pub account_email: String,
    /// Cached snapshot
    pub snapshot: OpenAIDashboardSnapshot,
}

impl OpenAIDashboardCache {
    pub fn new(account_email: impl Into<String>, snapshot: OpenAIDashboardSnapshot) -> Self {
        Self {
            account_email: account_email.into(),
            snapshot,
        }
    }
}

/// Cache store for OpenAI dashboard data
pub struct OpenAIDashboardCacheStore;

impl OpenAIDashboardCacheStore {
    /// Load cached dashboard data
    pub fn load() -> Option<OpenAIDashboardCache> {
        let url = Self::cache_path()?;
        let data = fs::read_to_string(&url).ok()?;
        serde_json::from_str(&data).ok()
    }

    /// Save dashboard data to cache
    pub fn save(cache: &OpenAIDashboardCache) {
        if let Some(url) = Self::cache_path() {
            if let Some(parent) = url.parent() {
                // Cache write is best-effort: failing to create the cache
                // dir just means no dashboard cache this run.
                let _created = fs::create_dir_all(parent);
            }
            if let Ok(data) = serde_json::to_string_pretty(cache) {
                // Same best-effort contract: a failed serialize/write leaves
                // the previous cache intact.
                let _written = fs::write(&url, data);
            }
        }
    }

    /// Clear cached data
    pub fn clear() {
        if let Some(url) = Self::cache_path() {
            // Cache invalidation is idempotent; a missing file is fine.
            let _removed = fs::remove_file(url);
        }
    }

    fn cache_path() -> Option<PathBuf> {
        dirs::data_local_dir().map(|d| d.join("CodexBar").join("openai-dashboard.json"))
    }
}

/// Credits snapshot for UI display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreditsSnapshot {
    /// Credits remaining
    pub remaining: f64,
    /// Recent credit events
    pub events: Vec<CreditEvent>,
    /// When this snapshot was taken
    pub updated_at: DateTime<Utc>,
}

impl CreditsSnapshot {
    pub fn new(remaining: f64, events: Vec<CreditEvent>, updated_at: DateTime<Utc>) -> Self {
        Self {
            remaining,
            events,
            updated_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_daily_breakdown() {
        let events = vec![
            CreditEvent::new(NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(), "CLI", 10.5),
            CreditEvent::new(NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(), "API", 5.0),
            CreditEvent::new(NaiveDate::from_ymd_opt(2026, 1, 14).unwrap(), "CLI", 3.0),
        ];

        let breakdown = OpenAIDashboardSnapshot::make_daily_breakdown(&events, 30);

        assert_eq!(breakdown.len(), 2);
        assert_eq!(breakdown[0].day, "2026-01-15");
        assert_eq!(breakdown[0].total_credits_used, 15.5);
        assert_eq!(breakdown[0].services.len(), 2);
        assert_eq!(breakdown[0].services[0].service, "CLI"); // Higher credits first
        assert_eq!(breakdown[1].day, "2026-01-14");
    }

    #[test]
    fn test_service_usage() {
        let usage = OpenAIDashboardServiceUsage::new("CLI", 10.5);
        assert_eq!(usage.service, "CLI");
        assert_eq!(usage.credits_used, 10.5);
    }
}
