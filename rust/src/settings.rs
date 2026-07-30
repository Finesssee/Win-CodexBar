//! Settings management for CodexBar
//!
//! Handles persistent configuration including:
//! - Enabled/disabled providers
//! - Refresh interval
//! - Manual cookies
//! - Other user preferences

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::core::ProviderId;

mod api_keys;
mod manual_cookies;
mod provider_workspace;
mod raw;
mod status;
mod types;

pub use api_keys::*;
pub use manual_cookies::*;
pub use provider_workspace::*;
use raw::RawSettings;
pub use status::*;
pub use types::*;

#[cfg(test)]
mod tests;

/// Application settings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "RawSettings", default)]
pub struct Settings {
    /// Enabled provider IDs (by CLI name)
    pub enabled_providers: HashSet<String>,

    /// Refresh interval in seconds (0 = manual only).
    /// Ignored when [`Self::adaptive_refresh`] is true.
    pub refresh_interval_secs: u64,

    /// When true, ignore the fixed interval and use adaptive refresh delays.
    #[serde(default)]
    pub adaptive_refresh: bool,

    /// Force-refresh enabled providers whenever the tray/menu surface opens.
    #[serde(default)]
    pub refresh_all_providers_on_menu_open: bool,

    /// Whether to start minimized
    pub start_minimized: bool,

    /// Whether to start at login
    pub start_at_login: bool,

    /// Whether to show notifications
    pub show_notifications: bool,

    /// Whether to play sound effects for threshold alerts
    pub sound_enabled: bool,

    /// Sound volume for alerts (0-100)
    pub sound_volume: u8,

    /// High usage threshold for warnings (percentage)
    pub high_usage_threshold: f64,

    /// Critical usage threshold for alerts (percentage)
    pub critical_usage_threshold: f64,

    pub provider_usage_thresholds: HashMap<String, UsageThresholdOverride>,

    /// Merge mode: show all enabled providers in a single tray icon
    pub merge_tray_icons: bool,

    /// Tray icon display mode: single icon or per-provider icons
    #[serde(default)]
    pub tray_icon_mode: TrayIconMode,

    /// Show provider icons in the merged switcher UI
    #[serde(default = "default_true")]
    pub switcher_shows_icons: bool,

    /// Prefer the provider closest to its limit in merged menu bar display
    #[serde(default)]
    pub menu_bar_shows_highest_usage: bool,

    /// Replace bar-only tray display with provider branding plus percent text where supported
    #[serde(default)]
    pub menu_bar_shows_percent: bool,

    /// Show usage bars as "used" (true) or "remaining" (false)
    pub show_as_used: bool,

    /// Enable UI animations (chart entrances, transitions)
    pub enable_animations: bool,

    /// Show reset times as relative (e.g., "2h 30m" instead of "3:00 PM")
    pub reset_time_relative: bool,

    /// Replace exhausted quota text with its concrete future reset time.
    #[serde(default)]
    pub show_reset_when_exhausted: bool,

    /// Warn when Codex or Claude pace predicts exhaustion before reset.
    #[serde(default)]
    pub predictive_pace_warning_enabled: bool,

    /// Menu bar display mode: "minimal", "compact", or "detailed"
    pub menu_bar_display_mode: String,

    /// Show all token accounts in provider menus instead of collapsing behind switchers
    #[serde(default)]
    pub show_all_token_accounts_in_menu: bool,

    /// Per-provider configuration map (cookie/usage source, region, manual
    /// headers, API tokens, etc). Replaces the legacy flat per-provider
    /// fields; legacy `settings.json` files are migrated via [`RawSettings`].
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub provider_configs: HashMap<ProviderId, ProviderConfig>,

    /// Disable credential/keychain-style reads where supported
    #[serde(default)]
    pub disable_keychain_access: bool,

    /// Hide personal info (emails, account names) for streaming/sharing
    pub hide_personal_info: bool,

    /// Update channel for receiving updates (Stable or Beta)
    pub update_channel: UpdateChannel,

    /// Per-provider metric preference for tray display
    #[serde(default)]
    pub provider_metrics: HashMap<String, MetricPreference>,

    /// Preferred display order of provider IDs (CLI names).
    ///
    /// An empty list means "fall back to the canonical `ProviderId::all()`
    /// order". Unknown or duplicated ids are filtered out on load; new
    /// providers are appended in their canonical order.
    #[serde(default)]
    pub provider_order: Vec<String>,

    /// Global keyboard shortcut to open the menu (e.g., "Ctrl+Shift+U")
    #[serde(default = "default_global_shortcut")]
    pub global_shortcut: String,

    /// Additional Codex home or sessions directories to include in local cost scans.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub codex_custom_sessions_dirs: Vec<String>,

    /// Discover local and configured SSH Codex/Claude sessions.
    #[serde(default)]
    pub agent_sessions_enabled: bool,

    /// SSH targets queried for remote agent sessions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_session_ssh_hosts: Vec<String>,

    /// Master switch for external hooks (`hooks.json` next to settings).
    #[serde(default)]
    pub hooks_enabled: bool,

    /// Route provider/app HTTPS through a user-configured HTTP(S) proxy (#235).
    #[serde(default)]
    pub http_proxy_enabled: bool,

    /// Proxy base URL, e.g. `http://127.0.0.1:7890`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub http_proxy_url: String,

    /// Optional proxy basic-auth username.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub http_proxy_username: String,

    /// Optional proxy basic-auth password (stored in local settings.json).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub http_proxy_password: String,

    /// Automatically download updates in the background
    #[serde(default)]
    pub auto_download_updates: bool,

    /// Install pending updates when quitting the application
    #[serde(default)]
    pub install_updates_on_quit: bool,

    /// UI language for the application (English default for backward compatibility)
    #[serde(default)]
    pub ui_language: Language,

    /// UI theme preference (Phase 12). Defaults to Auto (prefers-color-scheme).
    #[serde(default)]
    pub theme: ThemePreference,

    /// Main PopOut window display scale, in the inclusive range 100..=250.
    /// 100 % is normal size; higher values enlarge the window content.
    #[serde(default = "default_window_scale_percent")]
    pub window_scale_percent: u16,

    /// Tray flyout display scale, in the inclusive range 100..=200.
    /// 100 % is normal size; higher values enlarge the flyout content.
    #[serde(default = "default_tray_scale_percent")]
    pub tray_scale_percent: u16,

    /// Enable the local PowerToys Command Palette status pipe.
    #[serde(default)]
    pub powertoys_status_pipe_enabled: bool,

    /// Show the always-on-top floating capacity bar.
    #[serde(default)]
    pub float_bar_enabled: bool,

    /// Opacity of the floating bar window, in the inclusive range 30..=100.
    /// Stored as `u8` so the on-disk format remains stable.
    #[serde(default = "default_float_bar_opacity")]
    pub float_bar_opacity: u8,

    /// Floating-bar visual scale, in the inclusive range 75..=200.
    #[serde(default = "default_float_bar_scale")]
    pub float_bar_scale: u8,

    /// Floating-bar orientation: "horizontal" (default) or "vertical".
    #[serde(default = "default_float_bar_orientation")]
    pub float_bar_orientation: String,

    /// Floating-bar visual style: "floating" (default) or "taskbar".
    #[serde(default = "default_float_bar_style")]
    pub float_bar_style: String,

    /// When true the floating bar is fully click-through (overlay mode).
    #[serde(default)]
    pub float_bar_click_through: bool,

    /// Provider CLI names to display in the floating bar. Empty = all enabled.
    #[serde(default)]
    pub float_bar_provider_ids: Vec<String>,

    /// When true, the floating bar uses a dark-on-light palette so it
    /// stays legible on light desktop backgrounds. Defaults to false
    /// (light-on-dark, the original look).
    #[serde(default)]
    pub float_bar_dark_text: bool,

    /// When true, show the primary window's next reset inline in each pill.
    #[serde(default)]
    pub float_bar_show_reset_inline: bool,

    /// When true, show local cost summaries in the floating bar.
    #[serde(default)]
    pub float_bar_show_cost: bool,

    /// Promote the tray icon out of the Windows hidden-icons overflow area.
    /// Only has effect on Windows 11 (build ≥ 22000); silently ignored elsewhere.
    /// Defaults on so upgrades keep the icon pinned to the taskbar notification area.
    #[serde(default = "default_true")]
    pub promote_tray_icon: bool,

    /// When true, show Claude Daily Routines usage in extras.
    /// Defaults on to match upstream visibility.
    #[serde(default = "default_true")]
    pub claude_daily_routines_usage_visible: bool,

    /// Optional work-week length [2,6] for session-equivalent weekly forecast.
    /// `None` uses wall-clock time until weekly reset.
    #[serde(default)]
    pub weekly_progress_work_days: Option<u8>,

    /// Alibaba Token Plan API region: "cn" | "intl" | "cn-personal" | "intl-personal".
    #[serde(default = "default_alibaba_token_plan_region")]
    pub alibaba_token_plan_region: String,
}

fn default_window_scale_percent() -> u16 {
    100
}

fn default_alibaba_token_plan_region() -> String {
    "cn".to_string()
}

pub fn clamp_window_scale_percent(value: u16) -> u16 {
    value.clamp(100, 250)
}

fn default_tray_scale_percent() -> u16 {
    100
}

pub fn clamp_tray_scale_percent(value: u16) -> u16 {
    value.clamp(100, 200)
}

fn default_float_bar_opacity() -> u8 {
    80
}

fn default_float_bar_scale() -> u8 {
    100
}

fn default_float_bar_orientation() -> String {
    "horizontal".to_string()
}

fn default_float_bar_style() -> String {
    "floating".to_string()
}

/// Clamp the floating-bar opacity to the supported range.
///
/// Opacity values below 30% would make the bar effectively invisible, so we
/// pin the lower bound; the upper bound is the natural 100%.
pub fn clamp_float_bar_opacity(value: u8) -> u8 {
    value.clamp(30, 100)
}

/// Clamp the floating-bar visual scale to the supported range.
pub fn clamp_float_bar_scale(value: u8) -> u8 {
    value.clamp(75, 200)
}

/// Normalize a floating-bar orientation string. Unknown values fall back to
/// the default ("horizontal") so a corrupt settings file can't put the
/// renderer into an undefined state.
pub fn normalize_float_bar_orientation(value: &str) -> String {
    match value {
        "vertical" => "vertical".to_string(),
        _ => "horizontal".to_string(),
    }
}

/// Normalize a floating-bar style string. Unknown values fall back to the
/// original floating style so existing settings keep their previous look.
pub fn normalize_float_bar_style(value: &str) -> String {
    match value {
        "taskbar" => "taskbar".to_string(),
        _ => "floating".to_string(),
    }
}

/// Canonicalize a requested provider display order.
///
/// Keeps requested provider IDs that map to a real [`ProviderId`], drops
/// duplicates, and appends omitted providers in canonical order. An empty
/// request intentionally returns the full canonical order so display callers
/// can use one path for default and customized ordering.
pub fn normalize_provider_order(requested: &[String]) -> Vec<String> {
    let canonical = ProviderId::all()
        .iter()
        .map(|provider| provider.cli_name().to_string())
        .collect::<Vec<_>>();
    let valid = canonical.iter().map(String::as_str).collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(canonical.len());

    for provider_id in requested {
        if valid.contains(provider_id.as_str()) && seen.insert(provider_id.clone()) {
            out.push(provider_id.clone());
        }
    }
    for provider_id in canonical {
        if seen.insert(provider_id.clone()) {
            out.push(provider_id);
        }
    }

    out
}

fn default_global_shortcut() -> String {
    "Ctrl+Shift+U".to_string()
}

fn default_true() -> bool {
    true
}

/// Default cookie source value for browser-authenticated providers.
///
/// Browser cookie extraction reads browser profile databases and decrypts
/// Chromium cookies via Windows DPAPI, which can trigger behavior-based AV
/// engines. Keep that path explicit opt-in by default.
const DEFAULT_COOKIE_SOURCE: &str = "manual";

/// Default usage source value for any provider.
const DEFAULT_PROVIDER_SOURCE: &str = "auto";

/// Default API region for providers that expose one.
fn default_api_region(id: ProviderId) -> &'static str {
    match id {
        ProviderId::Alibaba => crate::providers::AlibabaRegion::Singapore.settings_value(),
        ProviderId::AlibabaTokenPlan => "cn",
        ProviderId::Zai | ProviderId::MiniMax => "global",
        _ => "",
    }
}

/// Default for the codex `openai_web_extras` boolean (true = show extras).
const DEFAULT_CODEX_OPENAI_WEB_EXTRAS: bool = true;
const DEFAULT_CODEX_SPARK_USAGE_VISIBLE: bool = true;

impl Default for Settings {
    fn default() -> Self {
        let mut enabled = HashSet::new();
        // Default enabled providers
        enabled.insert("claude".to_string());
        enabled.insert("codex".to_string());

        Self {
            enabled_providers: enabled,
            refresh_interval_secs: 300, // 5 minutes
            adaptive_refresh: false,
            refresh_all_providers_on_menu_open: false,
            start_minimized: false,
            start_at_login: false,
            show_notifications: true,
            sound_enabled: true,
            sound_volume: 100,
            high_usage_threshold: 70.0,
            critical_usage_threshold: 90.0,
            provider_usage_thresholds: HashMap::new(),
            merge_tray_icons: false, // Show single provider by default
            tray_icon_mode: TrayIconMode::default(), // Single icon by default
            switcher_shows_icons: true,
            menu_bar_shows_highest_usage: false,
            menu_bar_shows_percent: false,
            show_as_used: true,        // Show as "used" by default
            enable_animations: true,   // Animations enabled by default
            reset_time_relative: true, // Show relative times by default
            show_reset_when_exhausted: false,
            predictive_pace_warning_enabled: false,
            menu_bar_display_mode: "detailed".to_string(), // Detailed mode by default
            show_all_token_accounts_in_menu: false,
            provider_configs: HashMap::new(),
            disable_keychain_access: false,
            hide_personal_info: false, // Show personal info by default
            update_channel: UpdateChannel::default(), // Stable by default
            provider_metrics: HashMap::new(), // Empty = use Automatic for all
            provider_order: Vec::new(), // Empty = canonical ProviderId::all() order
            global_shortcut: default_global_shortcut(), // Ctrl+Shift+U by default
            codex_custom_sessions_dirs: Vec::new(),
            agent_sessions_enabled: false,
            agent_session_ssh_hosts: Vec::new(),
            hooks_enabled: false,
            http_proxy_enabled: false,
            http_proxy_url: String::new(),
            http_proxy_username: String::new(),
            http_proxy_password: String::new(),
            auto_download_updates: false, // Require explicit opt-in for background downloads
            install_updates_on_quit: false, // Don't auto-install on quit by default
            ui_language: Language::default(), // English by default
            theme: ThemePreference::default(), // Auto (follows prefers-color-scheme)
            window_scale_percent: default_window_scale_percent(),
            tray_scale_percent: default_tray_scale_percent(),
            powertoys_status_pipe_enabled: false,
            float_bar_enabled: false,
            float_bar_opacity: default_float_bar_opacity(),
            float_bar_scale: default_float_bar_scale(),
            float_bar_orientation: default_float_bar_orientation(),
            float_bar_style: default_float_bar_style(),
            float_bar_click_through: false,
            float_bar_provider_ids: Vec::new(),
            float_bar_dark_text: false,
            float_bar_show_reset_inline: false,
            float_bar_show_cost: false,
            promote_tray_icon: true,
            claude_daily_routines_usage_visible: true,
            weekly_progress_work_days: None,
            alibaba_token_plan_region: default_alibaba_token_plan_region(),
        }
    }
}

impl Settings {
    /// Get the settings file path
    pub fn settings_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("CodexBar").join("settings.json"))
    }

    /// Load settings from disk
    pub fn load() -> Self {
        #[allow(unused_mut)]
        let mut settings = match Self::settings_path() {
            Some(path) if path.exists() => match crate::secure_file::read_string(&path) {
                Ok(content) => {
                    serde_json::from_str(content.trim_start_matches('\u{feff}')).unwrap_or_default()
                }
                Err(_) => Self::default(),
            },
            _ => Self::default(),
        };

        // Sync autostart toggle with actual registry state and repair stale commands from older builds.
        #[cfg(target_os = "windows")]
        {
            settings.start_at_login = Self::sync_start_at_login_registry();
            settings.apply_promote_tray_default_migration();
        }

        settings
    }

    /// Marker written after the one-shot "pin tray by default" migration (issue #237).
    fn promote_tray_default_marker_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("CodexBar").join(".tray-pin-default-v1"))
    }

    /// Old builds defaulted `promote_tray_icon` to false and persisted that on any
    /// settings save. Flip those installs to the new default once; later opt-outs
    /// are preserved because the marker file remains.
    fn should_migrate_promote_tray_default(
        promote_tray_icon: bool,
        already_migrated: bool,
    ) -> bool {
        !already_migrated && !promote_tray_icon
    }

    fn apply_promote_tray_default_migration(&mut self) {
        let Some(marker) = Self::promote_tray_default_marker_path() else {
            return;
        };
        let already_migrated = marker.exists();
        if Self::should_migrate_promote_tray_default(self.promote_tray_icon, already_migrated) {
            self.promote_tray_icon = true;
            if let Err(error) = self.save() {
                tracing::warn!("Failed to persist promote_tray_icon default migration: {error}");
            }
        }
        if !already_migrated && let Some(parent) = marker.parent() {
            let _ = std::fs::create_dir_all(parent);
            if let Err(error) = std::fs::write(&marker, b"1") {
                tracing::warn!("Failed to write promote_tray_icon migration marker: {error}");
            }
        }
    }

    /// Save settings to disk
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::settings_path()
            .ok_or_else(|| anyhow::anyhow!("Could not determine settings path"))?;

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string_pretty(self)?;
        crate::secure_file::write_string(&path, &json)?;

        Ok(())
    }

    fn start_at_login_exe_path(current_exe: &std::path::Path) -> std::path::PathBuf {
        let file_name = current_exe.file_name().and_then(|name| name.to_str());
        if file_name.is_some_and(|name| {
            name.eq_ignore_ascii_case("codexbar-cli.exe")
                || name.eq_ignore_ascii_case("codexbar-desktop.exe")
        }) && let Some(desktop_exe) = current_exe
            .parent()
            .map(|dir| dir.join("codexbar.exe"))
            .filter(|path| path.exists())
        {
            return desktop_exe;
        }

        current_exe.to_path_buf()
    }

    fn start_at_login_command(current_exe: &std::path::Path) -> String {
        let exe_path = Self::start_at_login_exe_path(current_exe);
        format!("\"{}\"", exe_path.display())
    }

    fn start_at_login_command_needs_repair(existing: &str, current_exe: &std::path::Path) -> bool {
        existing != Self::start_at_login_command(current_exe)
    }

    #[cfg(target_os = "windows")]
    pub fn apply_start_at_login_registry(enabled: bool) -> anyhow::Result<()> {
        use winreg::RegKey;
        use winreg::enums::*;

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let run_key = hkcu.open_subkey_with_flags(
            r"Software\Microsoft\Windows\CurrentVersion\Run",
            KEY_READ | KEY_WRITE,
        )?;

        if enabled {
            let exe_path = std::env::current_exe()?;
            let command = Self::start_at_login_command(&exe_path);
            run_key.set_value("CodexBar", &command)?;
        } else {
            let _ = run_key.delete_value("CodexBar");
        }

        Ok(())
    }

    #[cfg(target_os = "windows")]
    fn sync_start_at_login_registry() -> bool {
        use winreg::RegKey;
        use winreg::enums::*;

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let Ok(run_key) = hkcu.open_subkey_with_flags(
            r"Software\Microsoft\Windows\CurrentVersion\Run",
            KEY_READ | KEY_WRITE,
        ) else {
            return false;
        };

        let Ok(existing) = run_key.get_value::<String, _>("CodexBar") else {
            return false;
        };

        match std::env::current_exe() {
            Ok(exe_path) if Self::start_at_login_command_needs_repair(&existing, &exe_path) => {
                let command = Self::start_at_login_command(&exe_path);
                if let Err(error) = run_key.set_value("CodexBar", &command) {
                    tracing::warn!("Failed to repair CodexBar start-at-login command: {error}");
                }
            }
            Err(error) => {
                tracing::warn!(
                    "Failed to resolve current executable for start-at-login sync: {error}"
                );
            }
            _ => {}
        }

        true
    }

    #[cfg(not(target_os = "windows"))]
    pub fn apply_start_at_login_registry(_enabled: bool) -> anyhow::Result<()> {
        Ok(())
    }

    /// Set start at login (updates Windows registry)
    pub fn set_start_at_login(&mut self, enabled: bool) -> anyhow::Result<()> {
        self.start_at_login = enabled;
        Self::apply_start_at_login_registry(enabled)?;
        Ok(())
    }

    /// Check if start at login is actually enabled in registry
    #[cfg(target_os = "windows")]
    pub fn is_start_at_login_enabled() -> bool {
        use winreg::RegKey;
        use winreg::enums::*;

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(run_key) = hkcu.open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run") {
            run_key.get_value::<String, _>("CodexBar").is_ok()
        } else {
            false
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub fn is_start_at_login_enabled() -> bool {
        false
    }

    /// Check if a provider is enabled
    pub fn is_provider_enabled(&self, id: ProviderId) -> bool {
        self.enabled_providers.contains(id.cli_name())
    }

    /// Enable a provider
    pub fn enable_provider(&mut self, id: ProviderId) {
        self.enabled_providers.insert(id.cli_name().to_string());
    }

    /// Disable a provider
    pub fn disable_provider(&mut self, id: ProviderId) {
        self.enabled_providers.remove(id.cli_name());
    }

    /// Toggle a provider's enabled state
    pub fn toggle_provider(&mut self, id: ProviderId) -> bool {
        let name = id.cli_name().to_string();
        if self.enabled_providers.contains(&name) {
            self.enabled_providers.remove(&name);
            false
        } else {
            self.enabled_providers.insert(name);
            true
        }
    }

    /// Get list of enabled provider IDs
    pub fn get_enabled_provider_ids(&self) -> Vec<ProviderId> {
        self.provider_display_order()
            .into_iter()
            .filter(|id| self.is_provider_enabled(*id))
            .collect()
    }

    /// Get all available providers with their enabled status
    pub fn get_all_providers_status(&self) -> Vec<ProviderStatus> {
        self.provider_display_order()
            .into_iter()
            .map(|id| ProviderStatus {
                id: id.cli_name().to_string(),
                name: id.display_name().to_string(),
                enabled: self.is_provider_enabled(id),
            })
            .collect()
    }

    /// Provider display order as typed IDs, falling back to canonical order
    /// when no custom order has been persisted.
    pub fn provider_display_order(&self) -> Vec<ProviderId> {
        normalize_provider_order(&self.provider_order)
            .into_iter()
            .filter_map(|provider_id| ProviderId::from_cli_name(&provider_id))
            .collect()
    }

    /// Provider display order as CLI-name strings.
    pub fn provider_display_order_names(&self) -> Vec<String> {
        normalize_provider_order(&self.provider_order)
    }

    /// Get the metric preference for a provider
    pub fn get_provider_metric(&self, id: ProviderId) -> MetricPreference {
        self.provider_metrics
            .get(id.cli_name())
            .copied()
            .unwrap_or_default()
    }

    /// Set the metric preference for a provider
    pub fn set_provider_metric(&mut self, id: ProviderId, metric: MetricPreference) {
        self.provider_metrics
            .insert(id.cli_name().to_string(), metric);
    }

    // ── Per-provider configuration accessors ─────────────────────────
    //
    // These thin wrappers around `provider_configs` apply provider-specific
    // defaults (e.g. cookie/usage source defaults to `"auto"`) so callers
    // never have to reach into the raw `Option<String>` fields. The
    // `*_str` / boolean / setter pairs intentionally mirror the names of
    // the legacy flat fields so call-site migration is mechanical.

    /// Read-only access to a provider's stored config, if any.
    pub fn provider_config(&self, id: ProviderId) -> Option<&ProviderConfig> {
        self.provider_configs.get(&id)
    }

    /// Mutable access to a provider's config, lazily creating an empty
    /// entry if none exists.
    pub fn provider_config_mut(&mut self, id: ProviderId) -> &mut ProviderConfig {
        self.provider_configs.entry(id).or_default()
    }

    /// Cookie source for `id`, or the default `"manual"` if unset.
    pub fn cookie_source(&self, id: ProviderId) -> &str {
        self.provider_configs
            .get(&id)
            .and_then(|c| c.cookie_source.as_deref())
            .unwrap_or(DEFAULT_COOKIE_SOURCE)
    }

    pub fn set_cookie_source(&mut self, id: ProviderId, source: impl Into<String>) {
        self.provider_config_mut(id).cookie_source = Some(source.into());
    }

    /// Usage source for `id`, or the default `"auto"` if unset.
    pub fn usage_source(&self, id: ProviderId) -> &str {
        self.provider_configs
            .get(&id)
            .and_then(|c| c.usage_source.as_deref())
            .unwrap_or(DEFAULT_PROVIDER_SOURCE)
    }

    pub fn set_usage_source(&mut self, id: ProviderId, source: impl Into<String>) {
        self.provider_config_mut(id).usage_source = Some(source.into());
    }

    /// API region for `id`, or the provider-specific default if unset.
    pub fn api_region(&self, id: ProviderId) -> &str {
        if id == ProviderId::AlibabaTokenPlan && !self.alibaba_token_plan_region.trim().is_empty() {
            return self.alibaba_token_plan_region.as_str();
        }
        self.provider_configs
            .get(&id)
            .and_then(|c| c.api_region.as_deref())
            .unwrap_or_else(|| default_api_region(id))
    }

    pub fn set_api_region(&mut self, id: ProviderId, region: impl Into<String>) {
        self.provider_config_mut(id).api_region = Some(region.into());
    }

    /// Manual cookie header for `id`, or `""` if unset.
    pub fn manual_cookie_header(&self, id: ProviderId) -> &str {
        self.provider_configs
            .get(&id)
            .and_then(|c| c.manual_cookie_header.as_deref())
            .unwrap_or("")
    }

    pub fn set_manual_cookie_header(&mut self, id: ProviderId, header: impl Into<String>) {
        self.provider_config_mut(id).manual_cookie_header = Some(header.into());
    }

    /// API token for `id`, or `""` if unset.
    pub fn api_token(&self, id: ProviderId) -> &str {
        self.provider_configs
            .get(&id)
            .and_then(|c| c.api_token.as_deref())
            .unwrap_or("")
    }

    pub fn set_api_token(&mut self, id: ProviderId, token: impl Into<String>) {
        self.provider_config_mut(id).api_token = Some(token.into());
    }

    /// Workspace ID override for `id`, or `""` if unset.
    pub fn workspace_id(&self, id: ProviderId) -> &str {
        self.provider_configs
            .get(&id)
            .and_then(|c| c.workspace_id.as_deref())
            .unwrap_or("")
    }

    pub fn set_workspace_id(&mut self, id: ProviderId, value: impl Into<String>) {
        self.provider_config_mut(id).workspace_id = Some(value.into());
    }

    /// Wayfinder gateway URL, defaulting to the local loopback gateway.
    pub fn gateway_url(&self, id: ProviderId) -> &str {
        self.provider_configs
            .get(&id)
            .and_then(|c| c.gateway_url.as_deref())
            .unwrap_or_else(|| {
                if id == ProviderId::Wayfinder {
                    crate::providers::wayfinder::DEFAULT_GATEWAY_URL
                } else {
                    ""
                }
            })
    }

    pub fn set_gateway_url(&mut self, id: ProviderId, value: impl Into<String>) {
        self.provider_config_mut(id).gateway_url = Some(value.into());
    }

    /// IDE base path override for `id`, or `""` if unset.
    pub fn ide_base_path(&self, id: ProviderId) -> &str {
        self.provider_configs
            .get(&id)
            .and_then(|c| c.ide_base_path.as_deref())
            .unwrap_or("")
    }

    pub fn set_ide_base_path(&mut self, id: ProviderId, value: impl Into<String>) {
        self.provider_config_mut(id).ide_base_path = Some(value.into());
    }

    /// Codex `openai_web_extras` toggle, default `true`.
    pub fn openai_web_extras(&self, id: ProviderId) -> bool {
        self.provider_configs
            .get(&id)
            .and_then(|c| c.openai_web_extras)
            .unwrap_or(DEFAULT_CODEX_OPENAI_WEB_EXTRAS)
    }

    pub fn set_openai_web_extras(&mut self, id: ProviderId, value: bool) {
        self.provider_config_mut(id).openai_web_extras = Some(value);
    }

    /// Codex Spark rows are visible by default.
    pub fn spark_usage_visible(&self, id: ProviderId) -> bool {
        self.provider_configs
            .get(&id)
            .and_then(|c| c.spark_usage_visible)
            .unwrap_or(DEFAULT_CODEX_SPARK_USAGE_VISIBLE)
    }

    pub fn set_spark_usage_visible(&mut self, id: ProviderId, value: bool) {
        self.provider_config_mut(id).spark_usage_visible = Some(value);
    }

    /// Per-provider historical-tracking toggle (currently codex-only).
    pub fn historical_tracking(&self, id: ProviderId) -> bool {
        self.provider_configs
            .get(&id)
            .map(|c| c.historical_tracking)
            .unwrap_or(false)
    }

    pub fn set_historical_tracking(&mut self, id: ProviderId, value: bool) {
        self.provider_config_mut(id).historical_tracking = value;
    }

    /// Per-provider "avoid keychain prompts" toggle (currently claude-only).
    pub fn avoid_keychain_prompts(&self, id: ProviderId) -> bool {
        self.provider_configs
            .get(&id)
            .map(|c| c.avoid_keychain_prompts)
            .unwrap_or(false)
    }

    pub fn set_avoid_keychain_prompts(&mut self, id: ProviderId, value: bool) {
        self.provider_config_mut(id).avoid_keychain_prompts = value;
    }

    // ── Legacy field-name aliases ────────────────────────────────────
    //
    // Keep the names of the old flat per-provider fields available as
    // accessor methods so existing call sites only need a `()` (read) or
    // `set_` prefix (write). New code should prefer the typed accessors
    // above.

    pub fn codex_cookie_source(&self) -> &str {
        self.cookie_source(ProviderId::Codex)
    }
    pub fn set_codex_cookie_source(&mut self, v: impl Into<String>) {
        self.set_cookie_source(ProviderId::Codex, v)
    }
    pub fn claude_cookie_source(&self) -> &str {
        self.cookie_source(ProviderId::Claude)
    }
    pub fn set_claude_cookie_source(&mut self, v: impl Into<String>) {
        self.set_cookie_source(ProviderId::Claude, v)
    }
    pub fn cursor_cookie_source(&self) -> &str {
        self.cookie_source(ProviderId::Cursor)
    }
    pub fn set_cursor_cookie_source(&mut self, v: impl Into<String>) {
        self.set_cookie_source(ProviderId::Cursor, v)
    }
    pub fn opencode_cookie_source(&self) -> &str {
        self.cookie_source(ProviderId::OpenCode)
    }
    pub fn set_opencode_cookie_source(&mut self, v: impl Into<String>) {
        self.set_cookie_source(ProviderId::OpenCode, v)
    }
    pub fn factory_cookie_source(&self) -> &str {
        self.cookie_source(ProviderId::Factory)
    }
    pub fn set_factory_cookie_source(&mut self, v: impl Into<String>) {
        self.set_cookie_source(ProviderId::Factory, v)
    }
    pub fn alibaba_cookie_source(&self) -> &str {
        self.cookie_source(ProviderId::Alibaba)
    }
    pub fn set_alibaba_cookie_source(&mut self, v: impl Into<String>) {
        self.set_cookie_source(ProviderId::Alibaba, v)
    }
    pub fn kimi_cookie_source(&self) -> &str {
        self.cookie_source(ProviderId::Kimi)
    }
    pub fn set_kimi_cookie_source(&mut self, v: impl Into<String>) {
        self.set_cookie_source(ProviderId::Kimi, v)
    }
    pub fn minimax_cookie_source(&self) -> &str {
        self.cookie_source(ProviderId::MiniMax)
    }
    pub fn set_minimax_cookie_source(&mut self, v: impl Into<String>) {
        self.set_cookie_source(ProviderId::MiniMax, v)
    }
    pub fn augment_cookie_source(&self) -> &str {
        self.cookie_source(ProviderId::Augment)
    }
    pub fn set_augment_cookie_source(&mut self, v: impl Into<String>) {
        self.set_cookie_source(ProviderId::Augment, v)
    }
    pub fn amp_cookie_source(&self) -> &str {
        self.cookie_source(ProviderId::Amp)
    }
    pub fn set_amp_cookie_source(&mut self, v: impl Into<String>) {
        self.set_cookie_source(ProviderId::Amp, v)
    }
    pub fn ollama_cookie_source(&self) -> &str {
        self.cookie_source(ProviderId::Ollama)
    }
    pub fn set_ollama_cookie_source(&mut self, v: impl Into<String>) {
        self.set_cookie_source(ProviderId::Ollama, v)
    }

    pub fn claude_usage_source(&self) -> &str {
        self.usage_source(ProviderId::Claude)
    }
    pub fn set_claude_usage_source(&mut self, v: impl Into<String>) {
        self.set_usage_source(ProviderId::Claude, v)
    }
    pub fn codex_usage_source(&self) -> &str {
        self.usage_source(ProviderId::Codex)
    }
    pub fn set_codex_usage_source(&mut self, v: impl Into<String>) {
        self.set_usage_source(ProviderId::Codex, v)
    }

    pub fn alibaba_api_region(&self) -> &str {
        self.api_region(ProviderId::Alibaba)
    }
    pub fn set_alibaba_api_region(&mut self, v: impl Into<String>) {
        self.set_api_region(ProviderId::Alibaba, v)
    }
    pub fn zai_api_region(&self) -> &str {
        self.api_region(ProviderId::Zai)
    }
    pub fn set_zai_api_region(&mut self, v: impl Into<String>) {
        self.set_api_region(ProviderId::Zai, v)
    }
    pub fn minimax_api_region(&self) -> &str {
        self.api_region(ProviderId::MiniMax)
    }
    pub fn set_minimax_api_region(&mut self, v: impl Into<String>) {
        self.set_api_region(ProviderId::MiniMax, v)
    }

    pub fn alibaba_cookie_header(&self) -> &str {
        self.manual_cookie_header(ProviderId::Alibaba)
    }
    pub fn set_alibaba_cookie_header(&mut self, v: impl Into<String>) {
        self.set_manual_cookie_header(ProviderId::Alibaba, v)
    }
    pub fn kimi_manual_cookie_header(&self) -> &str {
        self.manual_cookie_header(ProviderId::Kimi)
    }
    pub fn set_kimi_manual_cookie_header(&mut self, v: impl Into<String>) {
        self.set_manual_cookie_header(ProviderId::Kimi, v)
    }
    pub fn augment_cookie_header(&self) -> &str {
        self.manual_cookie_header(ProviderId::Augment)
    }
    pub fn set_augment_cookie_header(&mut self, v: impl Into<String>) {
        self.set_manual_cookie_header(ProviderId::Augment, v)
    }
    pub fn amp_cookie_header(&self) -> &str {
        self.manual_cookie_header(ProviderId::Amp)
    }
    pub fn set_amp_cookie_header(&mut self, v: impl Into<String>) {
        self.set_manual_cookie_header(ProviderId::Amp, v)
    }
    pub fn ollama_cookie_header(&self) -> &str {
        self.manual_cookie_header(ProviderId::Ollama)
    }
    pub fn set_ollama_cookie_header(&mut self, v: impl Into<String>) {
        self.set_manual_cookie_header(ProviderId::Ollama, v)
    }
    pub fn minimax_cookie_header(&self) -> &str {
        self.manual_cookie_header(ProviderId::MiniMax)
    }
    pub fn set_minimax_cookie_header(&mut self, v: impl Into<String>) {
        self.set_manual_cookie_header(ProviderId::MiniMax, v)
    }

    pub fn opencode_workspace_id(&self) -> &str {
        self.workspace_id(ProviderId::OpenCode)
    }
    pub fn set_opencode_workspace_id(&mut self, v: impl Into<String>) {
        self.set_workspace_id(ProviderId::OpenCode, v)
    }
    pub fn minimax_api_token(&self) -> &str {
        self.api_token(ProviderId::MiniMax)
    }
    pub fn set_minimax_api_token(&mut self, v: impl Into<String>) {
        self.set_api_token(ProviderId::MiniMax, v)
    }
    pub fn jetbrains_ide_base_path(&self) -> &str {
        self.ide_base_path(ProviderId::JetBrains)
    }
    pub fn set_jetbrains_ide_base_path(&mut self, v: impl Into<String>) {
        self.set_ide_base_path(ProviderId::JetBrains, v)
    }

    pub fn codex_openai_web_extras(&self) -> bool {
        self.openai_web_extras(ProviderId::Codex)
    }
    pub fn set_codex_openai_web_extras(&mut self, v: bool) {
        self.set_openai_web_extras(ProviderId::Codex, v)
    }
    pub fn codex_spark_usage_visible(&self) -> bool {
        self.spark_usage_visible(ProviderId::Codex)
    }
    pub fn set_codex_spark_usage_visible(&mut self, v: bool) {
        self.set_spark_usage_visible(ProviderId::Codex, v)
    }
    pub fn codex_historical_tracking(&self) -> bool {
        self.historical_tracking(ProviderId::Codex)
    }
    pub fn set_codex_historical_tracking(&mut self, v: bool) {
        self.set_historical_tracking(ProviderId::Codex, v)
    }
    pub fn claude_avoid_keychain_prompts(&self) -> bool {
        self.avoid_keychain_prompts(ProviderId::Claude)
    }
    pub fn set_claude_avoid_keychain_prompts(&mut self, v: bool) {
        self.set_avoid_keychain_prompts(ProviderId::Claude, v)
    }
}
