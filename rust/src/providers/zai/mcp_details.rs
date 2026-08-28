//! Zai (z.ai) Usage Statistics and MCP Details
//!
//! Provides detailed usage tracking for Zai provider including:
//! - Token limits
//! - Time limits
//! - Per-model usage details for MCP (Model Context Protocol)

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Z.ai limit types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ZaiLimitType {
    /// Token-based limit
    TokensLimit,
    /// Credit-based limit (credit Coding Plans, upstream 0.49.0 #2724)
    CreditLimit,
    /// Time-based limit
    TimeLimit,
}

impl ZaiLimitType {
    pub fn from_string(s: &str) -> Option<Self> {
        match s {
            "TOKENS_LIMIT" => Some(ZaiLimitType::TokensLimit),
            "CREDIT_LIMIT" => Some(ZaiLimitType::CreditLimit),
            "TIME_LIMIT" => Some(ZaiLimitType::TimeLimit),
            _ => None,
        }
    }
}

/// Z.ai limit time unit
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ZaiLimitUnit {
    Unknown,
    Days,
    Hours,
    Minutes,
}

impl ZaiLimitUnit {
    pub fn from_int(n: i32) -> Self {
        match n {
            1 => ZaiLimitUnit::Days,
            3 => ZaiLimitUnit::Hours,
            5 => ZaiLimitUnit::Minutes,
            _ => ZaiLimitUnit::Unknown,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            ZaiLimitUnit::Days => "day",
            ZaiLimitUnit::Hours => "hour",
            ZaiLimitUnit::Minutes => "minute",
            ZaiLimitUnit::Unknown => "unit",
        }
    }
}

/// Per-model usage detail for MCP tools
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZaiUsageDetail {
    /// Model code (e.g., "claude-3-opus", "claude-3-sonnet")
    pub model_code: String,
    /// Token usage count
    pub usage: i64,
}

impl ZaiUsageDetail {
    pub fn new(model_code: impl Into<String>, usage: i64) -> Self {
        Self {
            model_code: model_code.into(),
            usage,
        }
    }

    /// Format usage as human-readable string
    pub fn format_usage(&self) -> String {
        if self.usage >= 1_000_000 {
            format!("{:.1}M tokens", self.usage as f64 / 1_000_000.0)
        } else if self.usage >= 1_000 {
            format!("{:.1}K tokens", self.usage as f64 / 1_000.0)
        } else {
            format!("{} tokens", self.usage)
        }
    }
}

/// A single limit entry from Z.ai
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZaiLimitEntry {
    /// Type of limit
    pub limit_type: ZaiLimitType,
    /// Time unit for the window
    pub unit: ZaiLimitUnit,
    /// Number of units in the window
    pub number: i32,
    /// Total usage allowed
    pub usage: i64,
    /// Current value used
    pub current_value: i64,
    /// Remaining allocation
    pub remaining: i64,
    /// Usage percentage (0-100)
    pub percentage: f64,
    /// Per-model usage details
    pub usage_details: Vec<ZaiUsageDetail>,
    /// When the limit resets
    pub next_reset_time: Option<DateTime<Utc>>,
}

impl ZaiLimitEntry {
    /// Calculate used percentage from values
    pub fn used_percent(&self) -> f64 {
        if self.usage <= 0 {
            return self.percentage;
        }

        let limit = self.usage.max(0);
        if limit == 0 {
            return 0.0;
        }

        let used_from_remaining = limit - self.remaining;
        let used = used_from_remaining
            .max(self.current_value)
            .max(0)
            .min(limit);
        let percent = (used as f64 / limit as f64) * 100.0;
        percent.clamp(0.0, 100.0)
    }

    /// Get window duration in minutes
    pub fn window_minutes(&self) -> Option<i32> {
        if self.number <= 0 {
            return None;
        }
        match self.unit {
            ZaiLimitUnit::Minutes => Some(self.number),
            ZaiLimitUnit::Hours => Some(self.number * 60),
            ZaiLimitUnit::Days => Some(self.number * 24 * 60),
            ZaiLimitUnit::Unknown => None,
        }
    }

    /// Get window description (e.g., "1 hour", "7 days")
    pub fn window_description(&self) -> Option<String> {
        if self.number <= 0 {
            return None;
        }
        if self.unit == ZaiLimitUnit::Unknown {
            return None;
        }

        let unit_label = self.unit.label();
        if self.number == 1 {
            Some(format!("{} {}", self.number, unit_label))
        } else {
            Some(format!("{} {}s", self.number, unit_label))
        }
    }

    /// Get window label for display (e.g., "1 hour window")
    pub fn window_label(&self) -> Option<String> {
        self.window_description().map(|d| format!("{} window", d))
    }
}

/// Complete Z.ai usage snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZaiUsageSnapshot {
    /// Token limit entry
    pub token_limit: Option<ZaiLimitEntry>,
    /// Time limit entry
    pub time_limit: Option<ZaiLimitEntry>,
    /// User's plan name
    pub plan_name: Option<String>,
    /// When this snapshot was captured
    pub updated_at: DateTime<Utc>,
}

impl ZaiUsageSnapshot {
    /// Check if this snapshot contains valid data
    pub fn is_valid(&self) -> bool {
        self.token_limit.is_some() || self.time_limit.is_some()
    }

    /// Get the primary limit (tokens preferred over time)
    pub fn primary_limit(&self) -> Option<&ZaiLimitEntry> {
        self.token_limit.as_ref().or(self.time_limit.as_ref())
    }

    /// Get the secondary limit
    pub fn secondary_limit(&self) -> Option<&ZaiLimitEntry> {
        if self.token_limit.is_some() && self.time_limit.is_some() {
            self.time_limit.as_ref()
        } else {
            None
        }
    }
}

/// MCP Details menu data for UI
#[derive(Debug, Clone)]
pub struct McpDetailsMenu {
    /// Window label (e.g., "1 hour window")
    pub window_label: Option<String>,
    /// Reset time description
    pub reset_description: Option<String>,
    /// Sorted per-model usage details
    pub usage_details: Vec<ZaiUsageDetail>,
}

impl McpDetailsMenu {
    /// Build menu data from a Z.ai snapshot
    pub fn from_snapshot(snapshot: &ZaiUsageSnapshot) -> Option<Self> {
        let time_limit = snapshot.time_limit.as_ref()?;
        if time_limit.usage_details.is_empty() {
            return None;
        }

        let window_label = time_limit.window_label();

        let reset_description = time_limit.next_reset_time.map(|reset| {
            let now = Utc::now();
            if reset <= now {
                "now".to_string()
            } else {
                let duration = reset - now;
                let hours = duration.num_hours();
                let minutes = duration.num_minutes() % 60;

                if hours > 24 {
                    let days = hours / 24;
                    format!("{}d {}h", days, hours % 24)
                } else if hours > 0 {
                    format!("{}h {}m", hours, minutes)
                } else {
                    format!("{}m", minutes)
                }
            }
        });

        // Sort by model code
        let mut usage_details = time_limit.usage_details.clone();
        usage_details.sort_by(|a, b| {
            a.model_code
                .to_lowercase()
                .cmp(&b.model_code.to_lowercase())
        });

        Some(Self {
            window_label,
            reset_description,
            usage_details,
        })
    }

    /// Generate menu items as (label, value) pairs
    pub fn menu_items(&self) -> Vec<(String, String)> {
        let mut items = Vec::new();

        if let Some(window) = &self.window_label {
            items.push(("Window".to_string(), window.clone()));
        }

        if let Some(reset) = &self.reset_description {
            items.push(("Resets".to_string(), reset.clone()));
        }

        for detail in &self.usage_details {
            items.push((detail.model_code.clone(), detail.format_usage()));
        }

        items
    }
}

/// API response structures for parsing
#[derive(Debug, Deserialize)]
pub(crate) struct ZaiQuotaLimitResponse {
    pub code: i32,
    pub msg: String,
    pub data: Option<ZaiQuotaLimitData>,
    pub success: bool,
}

impl ZaiQuotaLimitResponse {
    pub fn is_success(&self) -> bool {
        self.success && self.code == 200
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ZaiQuotaLimitData {
    pub limits: Vec<ZaiLimitRaw>,
    #[serde(alias = "plan")]
    #[serde(alias = "plan_type")]
    #[serde(alias = "packageName")]
    pub plan_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ZaiLimitRaw {
    #[serde(rename = "type")]
    pub limit_type: String,
    pub unit: i32,
    pub number: i32,
    pub usage: i64,
    #[serde(rename = "currentValue")]
    pub current_value: i64,
    pub remaining: i64,
    pub percentage: i32,
    #[serde(rename = "usageDetails")]
    pub usage_details: Option<Vec<ZaiUsageDetailRaw>>,
    #[serde(rename = "nextResetTime")]
    pub next_reset_time: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ZaiUsageDetailRaw {
    #[serde(rename = "modelCode")]
    pub model_code: String,
    pub usage: i64,
}

impl ZaiLimitRaw {
    pub fn to_limit_entry(&self) -> Option<ZaiLimitEntry> {
        let limit_type = ZaiLimitType::from_string(&self.limit_type)?;
        let unit = ZaiLimitUnit::from_int(self.unit);

        let next_reset = self.next_reset_time.map(|ms| {
            let secs = ms / 1000;
            // ms % 1000 is at most 999, so nanoseconds stay below u32::MAX.
            #[expect(
                clippy::cast_possible_truncation,
                reason = "remainder < 1000 keeps nanoseconds within u32"
            )]
            let nsecs = ((ms % 1000) * 1_000_000) as u32;
            DateTime::from_timestamp(secs, nsecs).unwrap_or_else(Utc::now)
        });

        let usage_details = self
            .usage_details
            .as_ref()
            .map(|details| {
                details
                    .iter()
                    .map(|d| ZaiUsageDetail::new(&d.model_code, d.usage))
                    .collect()
            })
            .unwrap_or_default();

        Some(ZaiLimitEntry {
            limit_type,
            unit,
            number: self.number,
            usage: self.usage,
            current_value: self.current_value,
            remaining: self.remaining,
            percentage: self.percentage as f64,
            usage_details,
            next_reset_time: next_reset,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_limit_type_from_string() {
        assert_eq!(
            ZaiLimitType::from_string("TOKENS_LIMIT"),
            Some(ZaiLimitType::TokensLimit)
        );
        assert_eq!(
            ZaiLimitType::from_string("TIME_LIMIT"),
            Some(ZaiLimitType::TimeLimit)
        );
        assert_eq!(ZaiLimitType::from_string("INVALID"), None);
    }

    #[test]
    fn test_limit_unit_from_int() {
        assert_eq!(ZaiLimitUnit::from_int(1), ZaiLimitUnit::Days);
        assert_eq!(ZaiLimitUnit::from_int(3), ZaiLimitUnit::Hours);
        assert_eq!(ZaiLimitUnit::from_int(5), ZaiLimitUnit::Minutes);
        assert_eq!(ZaiLimitUnit::from_int(99), ZaiLimitUnit::Unknown);
    }

    #[test]
    fn test_usage_detail_format() {
        let detail = ZaiUsageDetail::new("claude-3-opus", 1_500_000);
        assert_eq!(detail.format_usage(), "1.5M tokens");

        let detail = ZaiUsageDetail::new("claude-3-sonnet", 5_000);
        assert_eq!(detail.format_usage(), "5.0K tokens");

        let detail = ZaiUsageDetail::new("gpt-4", 500);
        assert_eq!(detail.format_usage(), "500 tokens");
    }

    #[test]
    fn test_window_description() {
        let entry = ZaiLimitEntry {
            limit_type: ZaiLimitType::TimeLimit,
            unit: ZaiLimitUnit::Hours,
            number: 1,
            usage: 1000,
            current_value: 500,
            remaining: 500,
            percentage: 50.0,
            usage_details: vec![],
            next_reset_time: None,
        };

        assert_eq!(entry.window_description(), Some("1 hour".to_string()));
        assert_eq!(entry.window_label(), Some("1 hour window".to_string()));
    }

    #[test]
    fn test_window_minutes() {
        let mut entry = ZaiLimitEntry {
            limit_type: ZaiLimitType::TimeLimit,
            unit: ZaiLimitUnit::Hours,
            number: 2,
            usage: 1000,
            current_value: 0,
            remaining: 1000,
            percentage: 0.0,
            usage_details: vec![],
            next_reset_time: None,
        };

        assert_eq!(entry.window_minutes(), Some(120));

        entry.unit = ZaiLimitUnit::Days;
        entry.number = 7;
        assert_eq!(entry.window_minutes(), Some(7 * 24 * 60));
    }
}
