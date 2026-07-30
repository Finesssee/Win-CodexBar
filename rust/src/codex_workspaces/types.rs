//! Snapshot DTOs for Codex local Workspaces indexing.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Progress phases emitted while building a workspaces snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProgressPhase {
    ScanningLogs,
    IndexingProjects,
    Saving,
}

/// Progress callback payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub phase: ProgressPhase,
    pub processed_file_count: Option<u32>,
    pub total_file_count: Option<u32>,
    pub indexed_file_count: u32,
    pub skipped_file_count: u32,
}

impl Progress {
    pub fn phase(phase: ProgressPhase) -> Self {
        Self {
            phase,
            processed_file_count: None,
            total_file_count: None,
            indexed_file_count: 0,
            skipped_file_count: 0,
        }
    }
}

/// Completeness of the read-only Codex thread catalog (`state_5.sqlite`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceStatus {
    Complete,
    CatalogMissing,
    CatalogLocked,
    CatalogCorrupt,
    CatalogIncompatible,
}

impl SourceStatus {
    pub fn is_partial(self) -> bool {
        self != Self::Complete
    }
}

/// Known vs unknown cost split. Unknown pricing never becomes a synthetic zero.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostEstimate {
    pub known_usd: f64,
    pub unknown_tokens: u64,
}

impl Default for CostEstimate {
    fn default() -> Self {
        Self {
            known_usd: 0.0,
            unknown_tokens: 0,
        }
    }
}

impl CostEstimate {
    pub fn add_assign(&mut self, other: &Self) {
        self.known_usd += other.known_usd;
        self.unknown_tokens = self.unknown_tokens.saturating_add(other.unknown_tokens);
    }
}

/// Aggregated token totals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

impl UsageTotals {
    pub fn add_assign(&mut self, other: &Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(other.cached_input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
    }

    pub fn from_parts(input: u64, cached: u64, output: u64) -> Self {
        let cached = cached.min(input);
        Self {
            input_tokens: input,
            cached_input_tokens: cached,
            output_tokens: output,
            total_tokens: input.saturating_add(output),
        }
    }
}

/// One calendar-day aggregate point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyPoint {
    pub day: String,
    pub total_tokens: u64,
    pub cached_input_tokens: u64,
    pub estimated_cost_usd: Option<f64>,
}

/// Per-session usage row (top sessions only on projects).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUsage {
    pub id: String,
    pub project_id: String,
    pub display_title: String,
    pub cwd: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub latest_activity: Option<DateTime<Utc>>,
    pub totals: UsageTotals,
    pub cost_estimate: CostEstimate,
    pub top_model: Option<String>,
}

/// Per-project usage aggregate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectUsage {
    pub id: String,
    pub display_name: String,
    pub path: Option<String>,
    pub totals: UsageTotals,
    pub cost_estimate: CostEstimate,
    pub session_count: u32,
    pub latest_activity: Option<DateTime<Utc>>,
    pub top_model: Option<String>,
    pub top_sessions: Vec<SessionUsage>,
}

/// Complete workspaces snapshot published to the sidecar and presentation layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexLocalProjectUsageSnapshot {
    pub updated_at: DateTime<Utc>,
    pub history_days: u32,
    pub scope_signature: String,
    pub indexed_file_count: u32,
    pub skipped_file_count: u32,
    pub total: UsageTotals,
    pub projects: Vec<ProjectUsage>,
    pub daily: Vec<DailyPoint>,
    pub source_status: SourceStatus,
}

impl CodexLocalProjectUsageSnapshot {
    /// Strip paths/titles for presentation when hide-personal-info is on.
    /// Does not rewrite the sidecar.
    pub fn redact_for_privacy(&mut self) {
        for project in &mut self.projects {
            if project.id == crate::codex_workspaces::CHATS_PROJECT_ID {
                project.display_name = crate::codex_workspaces::CHATS_DISPLAY_NAME.to_string();
            } else {
                project.display_name = "Workspace".to_string();
            }
            project.path = None;
            for session in &mut project.top_sessions {
                session.display_title = "Local Codex chat".to_string();
                session.cwd = None;
            }
        }
    }
}
