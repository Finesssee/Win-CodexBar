//! Codex local Workspaces snapshot bridge.

use codexbar::codex_workspaces::{CodexLocalProjectUsageSnapshot, CodexWorkspacesIndex};
use codexbar::settings::Settings;
use tauri::State;

use crate::state::AppState;
use std::sync::Mutex;

/// Return the local Codex workspaces usage snapshot.
///
/// Privacy-redacts paths/titles when `settings.hide_personal_info` is set.
#[tauri::command]
pub async fn get_codex_workspaces_snapshot(
    _state: State<'_, Mutex<AppState>>,
    force_refresh: Option<bool>,
    history_days: Option<u32>,
) -> Result<CodexLocalProjectUsageSnapshot, String> {
    let force = force_refresh.unwrap_or(false);
    let days = history_days.unwrap_or(30);
    let hide = Settings::load().hide_personal_info;

    tauri::async_runtime::spawn_blocking(move || {
        let index = CodexWorkspacesIndex::new(days);
        let mut snapshot = index
            .load_snapshot(force, |_| {})
            .map_err(|e| e.to_string())?;
        if hide {
            snapshot.redact_for_privacy();
        }
        Ok(snapshot)
    })
    .await
    .map_err(|e| format!("workspaces worker failed: {e}"))?
}
