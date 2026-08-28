//! Provider Status Indicators
//!
//! Status overlay system for showing provider health in tray icons.
//! Integrates with status page monitoring (Statuspage.io, etc.)

#![allow(dead_code, reason = "status indicator types reserved for future UI integration")]

use serde::{Deserialize, Serialize};

/// Provider status level
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
pub enum StatusLevel {
    /// All systems operational
    #[default]
    Operational,
    /// Minor issues (small yellow dot)
    Minor,
    /// Major issues (orange dot)
    Major,
    /// Critical outage (red dot)
    Critical,
    /// Scheduled maintenance (blue dot)
    Maintenance,
    /// Status unknown or fetch failed (gray dot)
    Unknown,
}

impl StatusLevel {
    /// Get the RGB color for this status level
    pub fn color(&self) -> (u8, u8, u8) {
        match self {
            StatusLevel::Operational => (0, 255, 0),     // Green
            StatusLevel::Minor => (255, 255, 0),         // Yellow
            StatusLevel::Major => (255, 165, 0),         // Orange
            StatusLevel::Critical => (255, 0, 0),        // Red
            StatusLevel::Maintenance => (100, 149, 237), // Cornflower blue
            StatusLevel::Unknown => (128, 128, 128),     // Gray
        }
    }

    /// Get the RGBA color with alpha
    pub fn color_rgba(&self) -> (u8, u8, u8, u8) {
        let (r, g, b) = self.color();
        (r, g, b, 255)
    }

    /// Whether this status should show an overlay
    pub fn should_show_overlay(&self) -> bool {
        !matches!(self, StatusLevel::Operational)
    }

    /// Get a short label for the status
    pub fn label(&self) -> &'static str {
        match self {
            StatusLevel::Operational => "Operational",
            StatusLevel::Minor => "Minor Issues",
            StatusLevel::Major => "Major Issues",
            StatusLevel::Critical => "Critical Outage",
            StatusLevel::Maintenance => "Maintenance",
            StatusLevel::Unknown => "Unknown",
        }
    }

    /// Get an emoji for the status
    pub fn emoji(&self) -> &'static str {
        match self {
            StatusLevel::Operational => "✓",
            StatusLevel::Minor => "⚠",
            StatusLevel::Major => "⚠",
            StatusLevel::Critical => "✕",
            StatusLevel::Maintenance => "🔧",
            StatusLevel::Unknown => "?",
        }
    }

    /// Get a status prefix for menu items (colored Unicode dots)
    /// Returns empty string for Operational status (no dot needed)
    pub fn status_prefix(&self) -> &'static str {
        match self {
            StatusLevel::Operational => "",
            StatusLevel::Minor => "\u{1F7E1} ", // Yellow circle
            StatusLevel::Major => "\u{1F7E0} ", // Orange circle
            StatusLevel::Critical => "\u{1F534} ", // Red circle
            StatusLevel::Maintenance => "\u{1F535} ", // Blue circle
            StatusLevel::Unknown => "",         // No dot for unknown
        }
    }

    /// Get an ASCII-safe status prefix for menu items (fallback for systems without Unicode support)
    pub fn status_prefix_ascii(&self) -> &'static str {
        match self {
            StatusLevel::Operational => "",
            StatusLevel::Minor => "[!] ",
            StatusLevel::Major => "[!!] ",
            StatusLevel::Critical => "[X] ",
            StatusLevel::Maintenance => "[M] ",
            StatusLevel::Unknown => "",
        }
    }

    /// Parse from Statuspage.io status string
    pub fn from_statuspage(status: &str) -> Self {
        match status.to_lowercase().as_str() {
            "operational" | "none" => StatusLevel::Operational,
            "degraded_performance" | "degraded" | "minor" => StatusLevel::Minor,
            "partial_outage" | "partial" | "major" => StatusLevel::Major,
            "major_outage" | "critical" | "outage" => StatusLevel::Critical,
            "under_maintenance" | "maintenance" | "scheduled" => StatusLevel::Maintenance,
            _ => StatusLevel::Unknown,
        }
    }
}

/// Provider status information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderStatus {
    /// Current status level
    pub level: StatusLevel,
    /// Human-readable description
    pub description: Option<String>,
    /// URL to status page
    pub status_url: Option<String>,
    /// When this status was last updated
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// Active incidents count
    pub incident_count: usize,
}

impl Default for ProviderStatus {
    fn default() -> Self {
        Self {
            level: StatusLevel::Unknown,
            description: None,
            status_url: None,
            updated_at: chrono::Utc::now(),
            incident_count: 0,
        }
    }
}

impl ProviderStatus {
    pub fn operational() -> Self {
        Self {
            level: StatusLevel::Operational,
            description: Some("All systems operational".to_string()),
            ..Default::default()
        }
    }

    pub fn with_level(mut self, level: StatusLevel) -> Self {
        self.level = level;
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.status_url = Some(url.into());
        self
    }
}

/// Status overlay position on the icon
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverlayPosition {
    /// Top-right corner (default)
    #[default]
    TopRight,
    /// Top-left corner
    TopLeft,
    /// Bottom-right corner
    BottomRight,
    /// Bottom-left corner
    BottomLeft,
}

impl OverlayPosition {
    /// Get (x, y) offset from icon origin for a given icon size
    pub fn offset(&self, icon_size: u32, dot_size: u32) -> (i32, i32) {
        let padding = 1;
        let icon_size = icon_size as i32;
        let dot_size = dot_size as i32;

        match self {
            OverlayPosition::TopRight => (icon_size - dot_size - padding, padding),
            OverlayPosition::TopLeft => (padding, padding),
            OverlayPosition::BottomRight => (
                icon_size - dot_size - padding,
                icon_size - dot_size - padding,
            ),
            OverlayPosition::BottomLeft => (padding, icon_size - dot_size - padding),
        }
    }
}

/// Status overlay configuration
#[derive(Debug, Clone)]
pub struct StatusOverlayConfig {
    /// Position of the overlay dot
    pub position: OverlayPosition,
    /// Size of the overlay dot in pixels
    pub dot_size: u32,
    /// Whether to pulse/animate the dot for critical status
    pub animate_critical: bool,
}

impl Default for StatusOverlayConfig {
    fn default() -> Self {
        Self {
            position: OverlayPosition::TopRight,
            dot_size: 4,
            animate_critical: true,
        }
    }
}

/// Statuspage.io API response structures
#[derive(Debug, Deserialize)]
pub struct StatuspageResponse {
    pub status: StatuspageStatus,
    #[serde(default)]
    pub incidents: Vec<StatuspageIncident>,
}

#[derive(Debug, Deserialize)]
pub struct StatuspageStatus {
    pub indicator: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct StatuspageIncident {
    pub id: String,
    pub name: String,
    pub status: String,
    pub impact: String,
}

impl StatuspageResponse {
    /// Convert to ProviderStatus
    pub fn to_provider_status(&self, status_url: Option<&str>) -> ProviderStatus {
        let level = StatusLevel::from_statuspage(&self.status.indicator);
        ProviderStatus {
            level,
            description: Some(self.status.description.clone()),
            status_url: status_url.map(|s| s.to_string()),
            updated_at: chrono::Utc::now(),
            incident_count: self.incidents.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_level_ordering() {
        assert!(StatusLevel::Minor > StatusLevel::Operational);
        assert!(StatusLevel::Major > StatusLevel::Minor);
        assert!(StatusLevel::Critical > StatusLevel::Major);
    }

    #[test]
    fn test_status_level_colors() {
        let (r, g, b) = StatusLevel::Critical.color();
        assert_eq!(r, 255);
        assert_eq!(g, 0);
        assert_eq!(b, 0);
    }

    #[test]
    fn test_from_statuspage() {
        assert_eq!(
            StatusLevel::from_statuspage("operational"),
            StatusLevel::Operational
        );
        assert_eq!(
            StatusLevel::from_statuspage("degraded_performance"),
            StatusLevel::Minor
        );
        assert_eq!(
            StatusLevel::from_statuspage("major_outage"),
            StatusLevel::Critical
        );
        assert_eq!(
            StatusLevel::from_statuspage("under_maintenance"),
            StatusLevel::Maintenance
        );
    }

    #[test]
    fn test_overlay_position() {
        let (x, y) = OverlayPosition::TopRight.offset(16, 4);
        assert_eq!(x, 11); // 16 - 4 - 1
        assert_eq!(y, 1);
    }

    #[test]
    fn test_should_show_overlay() {
        assert!(!StatusLevel::Operational.should_show_overlay());
        assert!(StatusLevel::Minor.should_show_overlay());
        assert!(StatusLevel::Critical.should_show_overlay());
    }

    #[test]
    fn test_status_prefix() {
        // Operational should have no prefix
        assert_eq!(StatusLevel::Operational.status_prefix(), "");
        // Non-operational statuses should have colored dot prefixes
        assert!(!StatusLevel::Minor.status_prefix().is_empty());
        assert!(!StatusLevel::Major.status_prefix().is_empty());
        assert!(!StatusLevel::Critical.status_prefix().is_empty());
        assert!(!StatusLevel::Maintenance.status_prefix().is_empty());
        // Unknown should have no prefix
        assert_eq!(StatusLevel::Unknown.status_prefix(), "");
    }

    #[test]
    fn test_status_prefix_ascii() {
        // Operational should have no prefix
        assert_eq!(StatusLevel::Operational.status_prefix_ascii(), "");
        // Non-operational statuses should have ASCII prefixes
        assert_eq!(StatusLevel::Minor.status_prefix_ascii(), "[!] ");
        assert_eq!(StatusLevel::Major.status_prefix_ascii(), "[!!] ");
        assert_eq!(StatusLevel::Critical.status_prefix_ascii(), "[X] ");
        assert_eq!(StatusLevel::Maintenance.status_prefix_ascii(), "[M] ");
    }
}
