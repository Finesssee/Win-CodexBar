use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::commands::ProviderUsageSnapshot;
use crate::state::UpdateStatePayload;
use crate::surface::SurfaceMode;
use crate::surface_target::SurfaceTarget;

// ── Event name constants ─────────────────────────────────────────────

pub const SURFACE_MODE_CHANGED: &str = "surface-mode-changed";
pub const PROVIDER_UPDATED: &str = "provider-updated";
pub const REFRESH_STARTED: &str = "refresh-started";
pub const REFRESH_COMPLETE: &str = "refresh-complete";
pub const UPDATE_STATE_CHANGED: &str = "update-state-changed";
pub const LOCALE_CHANGED: &str = "locale-changed";
pub const SETTINGS_CHANGED: &str = "settings-changed";
pub const CODEX_ACCOUNTS_UPDATED: &str = "codex-accounts-updated";
pub const LOGIN_PHASE: &str = "login-phase";

// ── Payloads ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceModePayload {
    pub mode: &'static str,
    pub previous: &'static str,
    pub target: SurfaceTarget,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshCompletePayload {
    pub provider_count: usize,
    pub error_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshStartedPayload {
    pub provider_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginPhasePayload {
    pub provider_id: String,
    pub phase: String,
    pub auth_link: Option<String>,
}

// ── Emit helpers ─────────────────────────────────────────────────────

pub fn emit_surface_mode_changed(
    app: &AppHandle,
    from: SurfaceMode,
    to: SurfaceMode,
    target: SurfaceTarget,
) {
    let _ = app.emit(
        SURFACE_MODE_CHANGED,
        SurfaceModePayload {
            mode: to.as_str(),
            previous: from.as_str(),
            target,
        },
    );
}

pub fn emit_provider_updated(app: &AppHandle, snapshot: &ProviderUsageSnapshot) {
    let mut snapshot = snapshot.clone();
    let settings = codexbar::settings::Settings::load();
    crate::commands::filter_hidden_codex_spark_rows(
        &mut snapshot,
        settings.codex_spark_usage_visible(),
    );
    let presentation = crate::commands::ProviderUsagePresentationSnapshot::new(snapshot, &settings);
    let _ = app.emit(PROVIDER_UPDATED, presentation);
}

pub fn emit_refresh_started(app: &AppHandle, provider_ids: Vec<String>) {
    let _ = app.emit(REFRESH_STARTED, RefreshStartedPayload { provider_ids });
}

pub fn emit_refresh_complete(app: &AppHandle, provider_count: usize, error_count: usize) {
    let _ = app.emit(
        REFRESH_COMPLETE,
        RefreshCompletePayload {
            provider_count,
            error_count,
        },
    );
}

/// Broadcast that the Codex account snapshot store was refreshed (ADR 0003
/// multi-account lanes). Payload-less; listeners re-fetch via
/// `get_codex_accounts_state`.
pub fn emit_codex_accounts_updated(app: &AppHandle) {
    let _ = app.emit(CODEX_ACCOUNTS_UPDATED, ());
}

pub fn emit_update_state_changed(app: &AppHandle, payload: &UpdateStatePayload) {
    let _ = app.emit(UPDATE_STATE_CHANGED, payload);
}

/// Broadcast to every window that persisted settings changed, so surfaces in
/// other windows (e.g. the PopOut dashboard) re-read settings and re-render —
/// the detached Settings window and the main window are separate webviews and
/// do not share React state. Payload-less; listeners re-fetch the snapshot.
pub fn emit_settings_changed(app: &AppHandle) {
    let _ = app.emit(SETTINGS_CHANGED, ());
}

pub fn emit_login_phase(app: &AppHandle, provider_id: &str, phase: &str, auth_link: Option<&str>) {
    let _ = app.emit(
        LOGIN_PHASE,
        LoginPhasePayload {
            provider_id: provider_id.to_string(),
            phase: phase.to_string(),
            auth_link: auth_link.map(|s| s.to_string()),
        },
    );
}
