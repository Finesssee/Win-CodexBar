use std::collections::HashSet;
use std::sync::Mutex;

use codexbar::core::{
    FetchContext, ProviderAccountData, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, TokenAccount, TokenAccountOverride, TokenAccountStore,
    instantiate_provider,
};
use codexbar::locale;
use codexbar::providers::copilot::{CopilotApi, device_flow::CopilotDeviceFlow};
use codexbar::login::{self, LoginOutcome, LoginPhase};
use codexbar::secure_file::{self, SecureFileStatus};
use codexbar::settings::{
    ApiKeys, Language, ManualCookies, MetricPreference, Settings, ThemePreference, TrayIconMode,
    UpdateChannel,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::{Emitter, Manager};

use crate::events;
use crate::state::AppState;
use crate::surface::SurfaceMode;
use crate::surface_target::SurfaceTarget;

mod chart;
mod tokens;
mod updater;
mod usage_spend;

mod agent_sessions;
mod bridge;
mod browser_import;
mod codex_accounts;
mod codex_workspaces;
mod credential_detection;
mod credentials;
mod locale_cmd;
mod provider_detail;
mod provider_settings;
mod providers;
mod settings;
mod shortcuts;
mod surface;
mod system;

pub use agent_sessions::*;
pub(crate) use bridge::*;
pub use browser_import::*;
pub use codex_accounts::*;
pub use codex_workspaces::*;
pub use credential_detection::*;
pub use credentials::*;
pub use locale_cmd::*;
pub use provider_detail::*;
pub use provider_settings::*;
pub use providers::*;
pub use settings::*;
pub use shortcuts::*;
pub use surface::*;
pub use system::*;

#[cfg(test)]
mod tests;

pub use chart::*;
pub use tokens::*;
pub use updater::*;
pub use usage_spend::*;

const PROVIDER_CACHE_STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(30);
const MAX_API_KEY_LEN: usize = 16 * 1024;
const MAX_COOKIE_HEADER_LEN: usize = 64 * 1024;
const MAX_LABEL_LEN: usize = 80;

fn parse_provider_arg(provider_id: &str) -> Result<ProviderId, String> {
    let trimmed = provider_id.trim();
    if trimmed.is_empty() {
        return Err("Provider id is empty".to_string());
    }
    if trimmed.len() > 64 || trimmed.chars().any(char::is_control) {
        return Err("Provider id is invalid".to_string());
    }
    ProviderId::from_cli_name(trimmed).ok_or_else(|| format!("Unknown provider: {trimmed}"))
}

fn canonical_provider_arg(provider_id: &str) -> Result<String, String> {
    Ok(parse_provider_arg(provider_id)?.cli_name().to_string())
}

fn validate_single_line_secret(value: &str, field: &str, max_len: usize) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{field} is empty"));
    }
    if trimmed.len() > max_len {
        return Err(format!("{field} is too long"));
    }
    if trimmed.contains('\r') || trimmed.contains('\n') {
        return Err(format!("{field} must be a single line"));
    }
    Ok(())
}

fn sanitize_optional_label(label: Option<String>) -> Result<Option<String>, String> {
    let Some(label) = label else {
        return Ok(None);
    };
    let trimmed = label.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.len() > MAX_LABEL_LEN || trimmed.chars().any(char::is_control) {
        return Err("Label is invalid".to_string());
    }
    Ok(Some(trimmed.to_string()))
}
