//! Codex local Workspaces indexing foundation.
//!
//! Attributes local Codex rollout usage to projects, sessions, models, and days.
//! Codex-only; never writes the Codex catalog database.

mod indexer;
mod sidecar;
mod types;

pub use indexer::{CodexLocalDataScope, CodexWorkspacesIndex, IndexError, project_identity};
pub use sidecar::{PAYLOAD_FORMAT_VERSION, SCHEMA_VERSION, SidecarError, WorkspaceUsageSidecar};
pub use types::{
    CodexLocalProjectUsageSnapshot, CostEstimate, DailyPoint, Progress, ProgressPhase,
    ProjectUsage, SessionUsage, SourceStatus, UsageTotals,
};

pub const CHATS_PROJECT_ID: &str = "chats";
pub const CHATS_DISPLAY_NAME: &str = "Chats";
