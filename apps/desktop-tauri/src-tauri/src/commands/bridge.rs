use super::*;

// ── Bridge snapshot types ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RateWindowSnapshot {
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub window_minutes: Option<u32>,
    pub resets_at: Option<String>,
    pub reset_description: Option<String>,
    pub is_exhausted: bool,
    pub is_informational: bool,
    pub reserve_percent: Option<f64>,
    pub reserve_description: Option<String>,
    pub reserve_will_last_to_reset: bool,
    pub reserve_eta_seconds: Option<f64>,
}

impl RateWindowSnapshot {
    pub(super) fn from_rate_window(rw: &RateWindow) -> Self {
        Self {
            used_percent: rw.used_percent,
            remaining_percent: rw.remaining_percent(),
            window_minutes: rw.window_minutes,
            resets_at: rw.resets_at.map(|dt| dt.to_rfc3339()),
            reset_description: rw.reset_description.clone(),
            is_exhausted: rw.is_exhausted(),
            is_informational: rw.is_informational,
            reserve_percent: None,
            reserve_description: None,
            reserve_will_last_to_reset: false,
            reserve_eta_seconds: None,
        }
    }

    /// Enrich with raw reserve info derived from pace analysis.
    /// delta_percent = actual - expected; negative means ahead (in reserve).
    /// Only meaningful for longer windows (weekly); skip if reserve rounds to 0.
    /// Localization happens at render time so cached snapshots stay language-neutral.
    fn with_pace_reserve(mut self, pace: &codexbar::core::UsagePace) -> Self {
        let reserve = pace.delta_percent.abs().round();
        if pace.delta_percent < 0.0 && reserve > 0.0 {
            self.reserve_percent = Some(reserve);
            self.reserve_will_last_to_reset = pace.will_last_to_reset;
            self.reserve_eta_seconds = pace.eta_seconds;
        }
        self
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CostSnapshotBridge {
    pub used: f64,
    pub limit: Option<f64>,
    pub remaining: Option<f64>,
    pub currency_code: String,
    pub period: String,
    pub resets_at: Option<String>,
    pub formatted_used: String,
    pub formatted_limit: Option<String>,
    pub balance: Option<f64>,
    pub formatted_balance: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NamedRateWindowSnapshot {
    pub id: String,
    pub title: String,
    pub window: RateWindowSnapshot,
}

/// Pace prediction snapshot for tray/bridge display.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaceSnapshot {
    pub stage: &'static str,
    pub delta_percent: f64,
    pub will_last_to_reset: bool,
    pub eta_seconds: Option<f64>,
    pub expected_used_percent: f64,
    pub actual_used_percent: f64,
}

/// Session-equivalent weekly forecast for Claude/Codex menu secondary line.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEquivalentForecastSnapshot {
    pub estimated_windows_to_exhaust_weekly: f64,
    pub windows_until_reset: i64,
    pub available_windows_until_reset: f64,
    pub sample_count: usize,
    pub weekly_resets_at: String,
    pub weekly_used_percent: f64,
}

/// A frontend-friendly snapshot of one provider's usage data.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageSnapshot {
    pub tertiary_label: Option<String>,
    pub provider_id: String,
    pub display_name: String,
    pub primary: RateWindowSnapshot,
    pub primary_label: Option<String>,
    pub secondary: Option<RateWindowSnapshot>,
    pub secondary_label: Option<String>,
    pub model_specific: Option<RateWindowSnapshot>,
    pub tertiary: Option<RateWindowSnapshot>,
    pub extra_rate_windows: Vec<NamedRateWindowSnapshot>,
    pub cost: Option<CostSnapshotBridge>,
    pub plan_name: Option<String>,
    pub account_email: Option<String>,
    pub source_label: String,
    pub updated_at: String,
    pub error: Option<String>,
    pub pace: Option<PaceSnapshot>,
    pub account_organization: Option<String>,
    pub tray_status_label: Option<String>,
    pub fetch_duration_ms: Option<u128>,
    pub wayfinder_usage: Option<codexbar::core::WayfinderUsageSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_equivalent_forecast: Option<SessionEquivalentForecastSnapshot>,
}

pub(crate) fn filter_hidden_codex_spark_rows(
    snapshot: &mut ProviderUsageSnapshot,
    spark_usage_visible: bool,
) {
    if snapshot.provider_id == "codex" && !spark_usage_visible {
        snapshot
            .extra_rate_windows
            .retain(|extra| !matches!(extra.id.as_str(), "codex-spark" | "codex-spark-weekly"));
    }
}

pub(crate) fn pace_stage_str(stage: codexbar::core::PaceStage) -> &'static str {
    use codexbar::core::PaceStage;
    match stage {
        PaceStage::OnTrack => "on_track",
        PaceStage::SlightlyAhead => "slightly_ahead",
        PaceStage::Ahead => "ahead",
        PaceStage::FarAhead => "far_ahead",
        PaceStage::SlightlyBehind => "slightly_behind",
        PaceStage::Behind => "behind",
        PaceStage::FarBehind => "far_behind",
    }
}

impl ProviderUsageSnapshot {
    pub(super) fn from_fetch_result(
        id: ProviderId,
        metadata: &ProviderMetadata,
        result: &ProviderFetchResult,
    ) -> Self {
        let usage = &result.usage;

        // A missing session is represented by an informational primary so the
        // weekly lane keeps its canonical role. Use that weekly lane for the
        // provider-level pace summary instead of returning no pace at all.
        let primary_pace_window = if usage.primary.is_informational {
            usage.secondary.as_ref()
        } else {
            Some(&usage.primary)
        };
        let primary_pace = primary_pace_window
            .and_then(|window| codexbar::core::UsagePace::weekly(window, None, 10080));

        let pace = primary_pace.as_ref().map(|p| PaceSnapshot {
            stage: pace_stage_str(p.stage),
            delta_percent: p.delta_percent,
            will_last_to_reset: p.will_last_to_reset,
            eta_seconds: p.eta_seconds,
            expected_used_percent: p.expected_used_percent,
            actual_used_percent: p.actual_used_percent,
        });

        // Compute pace for secondary window (weekly) to derive reserve info
        let secondary_pace = usage
            .secondary
            .as_ref()
            .and_then(|sw| codexbar::core::UsagePace::weekly(sw, None, 10080));

        let primary_snap = RateWindowSnapshot::from_rate_window(&usage.primary);

        let secondary_snap = usage.secondary.as_ref().map(|sw| {
            let mut s = RateWindowSnapshot::from_rate_window(sw);
            if let Some(ref p) = secondary_pace {
                s = s.with_pace_reserve(p);
            }
            s
        });

        let session_equivalent_forecast =
            session_equivalent_forecast_for(id, &usage.primary, usage.secondary.as_ref());

        Self {
            provider_id: id.cli_name().to_string(),
            display_name: id.display_name().to_string(),
            primary: primary_snap,
            primary_label: Some(metadata.session_label.to_string()),
            secondary: secondary_snap,
            secondary_label: usage
                .secondary
                .as_ref()
                .map(|_| metadata.weekly_label.to_string()),
            model_specific: usage
                .model_specific
                .as_ref()
                .map(RateWindowSnapshot::from_rate_window),
            tertiary: usage
                .tertiary
                .as_ref()
                .map(RateWindowSnapshot::from_rate_window),
            // F5 (upstream 0.48.0): label the tertiary lane by its duration cadence
            // so surfaces (MenuCard, CLI, tray) can show "Monthly" instead of the
            // generic "DetailWindowTertiary" slot key.
            tertiary_label: usage.tertiary.as_ref().map(|w| {
                match codexbar::core::RateWindowCadence::from_minutes(w.window_minutes.unwrap_or(0))
                    .label_key()
                {
                    "monthly" => "monthly".to_string(),
                    other => other.to_string(),
                }
            }),
            extra_rate_windows: usage
                .extra_rate_windows
                .iter()
                .map(|extra| NamedRateWindowSnapshot {
                    id: extra.id.clone(),
                    title: extra.title.clone(),
                    window: RateWindowSnapshot::from_rate_window(&extra.window),
                })
                .collect(),
            cost: result.cost.as_ref().map(|c| CostSnapshotBridge {
                used: c.used,
                limit: c.limit,
                remaining: c.remaining(),
                currency_code: c.currency_code.clone(),
                period: c.period.clone(),
                resets_at: c.resets_at.map(|dt| dt.to_rfc3339()),
                formatted_used: c.format_used(),
                formatted_limit: c.format_limit(),
                balance: c.balance,
                formatted_balance: c.format_balance(),
            }),
            plan_name: usage.login_method.clone(),
            account_email: usage.account_email.clone(),
            source_label: result.source_label.clone(),
            updated_at: usage.updated_at.to_rfc3339(),
            error: None,
            pace,
            account_organization: usage.account_organization.clone(),
            tray_status_label: None,
            fetch_duration_ms: None,
            wayfinder_usage: result.wayfinder_usage.clone(),
            session_equivalent_forecast,
        }
    }

    pub(super) fn from_error(id: ProviderId, metadata: &ProviderMetadata, error: String) -> Self {
        let error = friendly_provider_error(id, &error);
        Self {
            provider_id: id.cli_name().to_string(),
            display_name: id.display_name().to_string(),
            primary: RateWindowSnapshot {
                used_percent: 0.0,
                remaining_percent: 100.0,
                window_minutes: None,
                resets_at: None,
                reset_description: None,
                is_exhausted: false,
                is_informational: false,
                reserve_percent: None,
                reserve_description: None,
                reserve_will_last_to_reset: false,
                reserve_eta_seconds: None,
            },
            primary_label: Some(metadata.session_label.to_string()),
            secondary: None,
            secondary_label: None,
            model_specific: None,
            tertiary: None,
            tertiary_label: None,
            extra_rate_windows: Vec::new(),
            cost: None,
            plan_name: None,
            account_email: None,
            source_label: String::new(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            error: Some(error),
            pace: None,
            account_organization: None,
            tray_status_label: None,
            fetch_duration_ms: None,
            wayfinder_usage: None,
            session_equivalent_forecast: None,
        }
    }
}

fn session_equivalent_forecast_for(
    id: ProviderId,
    session: &RateWindow,
    weekly: Option<&RateWindow>,
) -> Option<SessionEquivalentForecastSnapshot> {
    if !matches!(id, ProviderId::Claude | ProviderId::Codex) {
        return None;
    }
    let weekly = weekly?;
    let now = chrono::Utc::now();
    let provider_id = id.cli_name();
    codexbar::core::record_provider_windows(provider_id, session, Some(weekly), now);
    let work_days = Settings::load().weekly_progress_work_days;
    let forecast =
        codexbar::core::forecast_for_provider(provider_id, session, weekly, now, work_days)?;
    Some(SessionEquivalentForecastSnapshot {
        estimated_windows_to_exhaust_weekly: forecast.estimated_windows_to_exhaust_weekly,
        windows_until_reset: forecast.windows_until_reset,
        available_windows_until_reset: forecast.available_windows_until_reset,
        sample_count: forecast.sample_count,
        weekly_resets_at: forecast.weekly_resets_at.to_rfc3339(),
        weekly_used_percent: forecast.weekly_used_percent,
    })
}

/// Build a compact tray status label from a raw snapshot using the current language.
/// Localization is done at render time so cached snapshots stay language-neutral.
pub(crate) fn compact_tray_status_label(
    window: &RateWindowSnapshot,
    lang: codexbar::settings::Language,
) -> String {
    if window.is_informational {
        return window
            .reset_description
            .clone()
            .unwrap_or_else(|| "Unavailable".to_string());
    }

    let pct = format!("{:.0}%", window.used_percent);
    if let Some(reset) = compact_reset_description(window, lang) {
        format!("{pct} • {reset}")
    } else {
        pct
    }
}

fn compact_reset_description(
    window: &RateWindowSnapshot,
    lang: codexbar::settings::Language,
) -> Option<String> {
    if let Some(ref resets_at) = window.resets_at {
        let dt = chrono::DateTime::parse_from_rfc3339(resets_at)
            .ok()
            .map(|dt| dt.with_timezone(&chrono::Utc))?;
        return Some(format_compact_reset_countdown(dt, lang));
    }

    window
        .reset_description
        .as_deref()
        .map(|desc| normalize_reset_description(desc, lang))
        .filter(|desc| !desc.is_empty())
}

fn format_compact_reset_countdown(
    resets_at: chrono::DateTime<chrono::Utc>,
    lang: codexbar::settings::Language,
) -> String {
    let now = chrono::Utc::now();
    if resets_at <= now {
        return locale::get_text(lang, locale::LocaleKey::ResetInProgress);
    }

    let total_minutes = (resets_at - now).num_minutes().max(0);
    let days = total_minutes / 1440;
    let hours = (total_minutes % 1440) / 60;
    let minutes = total_minutes % 60;

    if days > 0 {
        locale::format_locale(
            lang,
            locale::LocaleKey::ResetsInDaysHours,
            &[&days.to_string(), &hours.to_string()],
        )
    } else {
        locale::format_locale(
            lang,
            locale::LocaleKey::ResetsInHoursMinutes,
            &[&hours.to_string(), &format!("{minutes:02}")],
        )
    }
}

fn normalize_reset_description(desc: &str, lang: codexbar::settings::Language) -> String {
    let trimmed = desc.trim();
    let lower = trimmed.to_ascii_lowercase();
    let prefix_len = ["resets in ", "reset in ", "in "]
        .iter()
        .find(|&&p| lower.starts_with(p))
        .map(|p| p.len())
        .unwrap_or(0);
    let body = trimmed[prefix_len..].trim_start();
    format!(
        "{} {body}",
        locale::get_text(lang, locale::LocaleKey::ResetsInShort)
    )
}

pub(crate) fn friendly_provider_error(id: ProviderId, error: &str) -> String {
    if id != ProviderId::Claude {
        return error.to_string();
    }

    let trimmed = error.trim();
    let lower = trimmed.to_lowercase();

    if lower.contains("swift.cancellationerror")
        || lower.contains("the operation couldn't be completed")
        || lower.contains("the operation could not be completed")
    {
        return "Claude usage fetch was cancelled before usage data was returned. Refresh Claude, or re-authenticate with Claude Code and try again.".to_string();
    }

    if lower.contains("claude oauth credentials not found") {
        return "Claude sign-in was not found. Run `claude` once to authenticate, then refresh Claude in Win-CodexBar.".to_string();
    }

    if lower.contains("oauth token expired") || lower.contains("token invalid or expired") {
        return "Claude sign-in expired. Run `claude` to refresh your Claude Code login, then refresh Claude in Win-CodexBar.".to_string();
    }

    if trimmed == "Authentication required" {
        return "Claude needs sign-in before Win-CodexBar can read usage. Run `claude` once, or add Claude cookies in Provider settings.".to_string();
    }

    if lower.starts_with("claude usage failed from all configured sources.") {
        return trimmed
            .replace(
                "OAuth: OAuth error: Claude OAuth credentials not found. Run `claude` to authenticate.",
                "OAuth: sign-in not found",
            )
            .replace(
                "Web: No cookies available for web API",
                "Web: no Claude cookies available",
            )
            .replace(
                "CLI: Provider not installed:",
                "CLI: not installed:",
            );
    }

    trimmed.to_string()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapState {
    pub(crate) contract_version: &'static str,
    pub(crate) providers: Vec<ProviderCatalogEntry>,
    pub(crate) settings: SettingsSnapshot,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentSurfaceState {
    pub mode: String,
    pub target: SurfaceTarget,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCatalogEntry {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) cookie_domain: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSnapshot {
    enabled_providers: Vec<String>,
    provider_order: Vec<String>,
    refresh_interval_secs: u64,
    adaptive_refresh: bool,
    refresh_all_providers_on_menu_open: bool,
    low_power_mode: bool,
    start_at_login: bool,
    start_minimized: bool,
    show_notifications: bool,
    sound_enabled: bool,
    notification_sound_theme: codexbar::settings::NotificationSoundTheme,
    notification_sound_paths: codexbar::settings::NotificationSoundPaths,
    high_usage_threshold: f64,
    critical_usage_threshold: f64,
    provider_usage_thresholds:
        std::collections::HashMap<String, codexbar::settings::UsageThresholdOverride>,
    predictive_pace_warning_enabled: bool,
    tray_icon_mode: &'static str,
    switcher_shows_icons: bool,
    menu_bar_shows_highest_usage: bool,
    menu_bar_shows_percent: bool,
    show_as_used: bool,
    show_all_token_accounts_in_menu: bool,
    enable_animations: bool,
    reset_time_relative: bool,
    show_reset_when_exhausted: bool,
    menu_bar_display_mode: String,
    hide_personal_info: bool,
    update_channel: &'static str,
    auto_download_updates: bool,
    install_updates_on_quit: bool,
    global_shortcut: String,
    codex_custom_sessions_dirs: Vec<String>,
    agent_sessions_enabled: bool,
    agent_session_ssh_hosts: Vec<String>,
    hooks_enabled: bool,
    http_proxy_enabled: bool,
    http_proxy_url: String,
    http_proxy_username: String,
    http_proxy_password: String,
    ui_language: &'static str,
    theme: &'static str,
    window_scale_percent: u16,
    tray_scale_percent: u16,
    powertoys_status_pipe_enabled: bool,
    claude_avoid_keychain_prompts: bool,
    codex_spark_usage_visible: bool,
    disable_keychain_access: bool,
    wayfinder_gateway_url: String,
    provider_metrics: std::collections::HashMap<String, &'static str>,
    float_bar_enabled: bool,
    float_bar_opacity: u8,
    float_bar_scale: u8,
    float_bar_orientation: String,
    float_bar_style: String,
    float_bar_click_through: bool,
    float_bar_provider_ids: Vec<String>,
    float_bar_dark_text: bool,
    float_bar_show_reset_inline: bool,
    float_bar_show_cost: bool,
    promote_tray_icon: bool,
    claude_daily_routines_usage_visible: bool,
    alibaba_token_plan_region: String,
    weekly_progress_work_days: Option<u8>,
}

#[tauri::command]
pub fn get_bootstrap_state() -> BootstrapState {
    let settings = Settings::load();
    BootstrapState {
        contract_version: "v1",
        providers: provider_catalog_for(&settings),
        settings: SettingsSnapshot::from(settings),
    }
}

#[tauri::command]
pub fn get_provider_catalog() -> Vec<ProviderCatalogEntry> {
    provider_catalog_for(&Settings::load())
}

#[tauri::command]
pub fn get_settings_snapshot() -> SettingsSnapshot {
    SettingsSnapshot::from(Settings::load())
}

impl From<Settings> for SettingsSnapshot {
    fn from(settings: Settings) -> Self {
        let avoid_keychain_prompts = settings.claude_avoid_keychain_prompts();
        let codex_spark_usage_visible = settings.codex_spark_usage_visible();
        let wayfinder_gateway_url = settings.gateway_url(ProviderId::Wayfinder).to_string();

        let provider_order = settings.provider_display_order_names();
        let enabled_providers = provider_order
            .iter()
            .filter(|provider_id| settings.enabled_providers.contains(*provider_id))
            .cloned()
            .collect();

        let provider_metrics = settings
            .provider_metrics
            .into_iter()
            .map(|(k, v)| (k, metric_preference_label(v)))
            .collect();

        Self {
            enabled_providers,
            provider_order,
            refresh_interval_secs: settings.refresh_interval_secs,
            adaptive_refresh: settings.adaptive_refresh,
            refresh_all_providers_on_menu_open: settings.refresh_all_providers_on_menu_open,
            low_power_mode: settings.low_power_mode,
            start_at_login: settings.start_at_login,
            start_minimized: settings.start_minimized,
            show_notifications: settings.show_notifications,
            sound_enabled: settings.sound_enabled,
            notification_sound_theme: settings.notification_sound_theme,
            notification_sound_paths: settings.notification_sound_paths,
            high_usage_threshold: settings.high_usage_threshold,
            critical_usage_threshold: settings.critical_usage_threshold,
            provider_usage_thresholds: settings.provider_usage_thresholds,
            predictive_pace_warning_enabled: settings.predictive_pace_warning_enabled,
            tray_icon_mode: tray_icon_mode_label(settings.tray_icon_mode),
            switcher_shows_icons: settings.switcher_shows_icons,
            menu_bar_shows_highest_usage: settings.menu_bar_shows_highest_usage,
            menu_bar_shows_percent: settings.menu_bar_shows_percent,
            show_as_used: settings.show_as_used,
            show_all_token_accounts_in_menu: settings.show_all_token_accounts_in_menu,
            enable_animations: settings.enable_animations,
            reset_time_relative: settings.reset_time_relative,
            show_reset_when_exhausted: settings.show_reset_when_exhausted,
            menu_bar_display_mode: settings.menu_bar_display_mode,
            hide_personal_info: settings.hide_personal_info,
            update_channel: update_channel_label(settings.update_channel),
            auto_download_updates: settings.auto_download_updates,
            install_updates_on_quit: settings.install_updates_on_quit,
            global_shortcut: settings.global_shortcut,
            codex_custom_sessions_dirs: settings.codex_custom_sessions_dirs,
            agent_sessions_enabled: settings.agent_sessions_enabled,
            agent_session_ssh_hosts: settings.agent_session_ssh_hosts,
            hooks_enabled: settings.hooks_enabled,
            http_proxy_enabled: settings.http_proxy_enabled,
            http_proxy_url: settings.http_proxy_url,
            http_proxy_username: settings.http_proxy_username,
            http_proxy_password: settings.http_proxy_password,
            ui_language: language_label(settings.ui_language),
            theme: theme_label(settings.theme),
            window_scale_percent: settings.window_scale_percent,
            tray_scale_percent: settings.tray_scale_percent,
            powertoys_status_pipe_enabled: settings.powertoys_status_pipe_enabled,
            claude_avoid_keychain_prompts: avoid_keychain_prompts,
            codex_spark_usage_visible,
            disable_keychain_access: settings.disable_keychain_access,
            wayfinder_gateway_url,
            provider_metrics,
            float_bar_enabled: settings.float_bar_enabled,
            float_bar_opacity: settings.float_bar_opacity,
            float_bar_scale: settings.float_bar_scale,
            float_bar_orientation: settings.float_bar_orientation,
            float_bar_style: settings.float_bar_style,
            float_bar_click_through: settings.float_bar_click_through,
            float_bar_provider_ids: settings.float_bar_provider_ids,
            float_bar_dark_text: settings.float_bar_dark_text,
            float_bar_show_reset_inline: settings.float_bar_show_reset_inline,
            float_bar_show_cost: settings.float_bar_show_cost,
            promote_tray_icon: settings.promote_tray_icon,
            claude_daily_routines_usage_visible: settings.claude_daily_routines_usage_visible,
            alibaba_token_plan_region: settings.alibaba_token_plan_region,
            weekly_progress_work_days: settings.weekly_progress_work_days,
        }
    }
}

pub(crate) fn provider_catalog_for(settings: &Settings) -> Vec<ProviderCatalogEntry> {
    // Soft-removed providers (upstream #2254) stay hidden in Settings unless already enabled.
    settings
        .provider_display_order()
        .into_iter()
        .filter(|provider| {
            !provider.is_deprecated() || settings.enabled_providers.contains(provider.cli_name())
        })
        .map(|provider| ProviderCatalogEntry {
            id: provider.cli_name().to_string(),
            display_name: provider.display_name().to_string(),
            cookie_domain: provider.cookie_domain().map(ToString::to_string),
        })
        .collect()
}

fn tray_icon_mode_label(mode: TrayIconMode) -> &'static str {
    match mode {
        TrayIconMode::Single => "single",
        TrayIconMode::PerProvider => "perProvider",
    }
}

pub(super) fn update_channel_label(channel: UpdateChannel) -> &'static str {
    match channel {
        UpdateChannel::Stable => "stable",
        UpdateChannel::Beta => "beta",
    }
}

pub(super) fn language_label(language: Language) -> &'static str {
    language.label()
}

fn theme_label(theme: ThemePreference) -> &'static str {
    match theme {
        ThemePreference::Auto => "auto",
        ThemePreference::Light => "light",
        ThemePreference::Dark => "dark",
    }
}

pub(super) fn parse_theme(s: &str) -> Option<ThemePreference> {
    match s {
        "auto" => Some(ThemePreference::Auto),
        "light" => Some(ThemePreference::Light),
        "dark" => Some(ThemePreference::Dark),
        _ => None,
    }
}

fn metric_preference_label(pref: MetricPreference) -> &'static str {
    match pref {
        MetricPreference::Automatic => "automatic",
        MetricPreference::Session => "session",
        MetricPreference::Weekly => "weekly",
        MetricPreference::Model => "model",
        MetricPreference::Tertiary => "tertiary",
        MetricPreference::Credits => "credits",
        MetricPreference::ExtraUsage => "extraUsage",
        MetricPreference::Average => "average",
    }
}

pub(super) fn parse_metric_preference(s: &str) -> Option<MetricPreference> {
    match s {
        "automatic" => Some(MetricPreference::Automatic),
        "session" => Some(MetricPreference::Session),
        "weekly" => Some(MetricPreference::Weekly),
        "model" => Some(MetricPreference::Model),
        "tertiary" => Some(MetricPreference::Tertiary),
        "credits" => Some(MetricPreference::Credits),
        "extraUsage" | "extrausage" => Some(MetricPreference::ExtraUsage),
        "average" => Some(MetricPreference::Average),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_window_with(
        used_percent: f64,
        window_minutes: Option<u32>,
        resets_at: Option<chrono::DateTime<chrono::Utc>>,
        reset_description: Option<String>,
    ) -> RateWindowSnapshot {
        RateWindowSnapshot {
            used_percent,
            remaining_percent: 100.0 - used_percent,
            window_minutes,
            resets_at: resets_at.map(|dt| dt.to_rfc3339()),
            reset_description,
            is_exhausted: false,
            is_informational: false,
            reserve_percent: None,
            reserve_description: None,
            reserve_will_last_to_reset: false,
            reserve_eta_seconds: None,
        }
    }

    #[test]
    fn tray_status_prefers_relative_reset_countdown() {
        let window = snapshot_window_with(
            13.0,
            Some(300),
            Some(chrono::Utc::now() + chrono::Duration::minutes(125)),
            Some("Jun 10 at 3:00PM".to_string()),
        );

        let label = compact_tray_status_label(&window, Language::English);

        assert!(label.starts_with("13% • Resets in 2h "));
        assert!(label.ends_with('m'));
        assert!(!label.contains("Jun 10"));
    }

    #[test]
    fn tray_status_normalizes_fallback_reset_description() {
        let window = snapshot_window_with(8.0, Some(300), None, Some("2h 05m".to_string()));

        assert_eq!(
            compact_tray_status_label(&window, Language::English),
            "8% • Resets in 2h 05m"
        );
    }

    #[test]
    fn japanese_tray_status_label_has_no_english_reset_text() {
        use codexbar::settings::Language;

        let window = snapshot_window_with(
            13.0,
            Some(300),
            Some(chrono::Utc::now() + chrono::Duration::minutes(125)),
            None,
        );

        let label = compact_tray_status_label(&window, Language::Japanese);

        assert!(label.contains("リセットまで"), "{label}");
        assert!(!label.to_ascii_lowercase().contains("resets in"), "{label}");
        assert!(label.contains("13%"), "{label}");
    }

    #[test]
    fn japanese_tray_status_strips_english_fallback_reset_prefix() {
        use codexbar::settings::Language;

        let window =
            snapshot_window_with(8.0, Some(300), None, Some("Resets in 2h 05m".to_string()));

        let label = compact_tray_status_label(&window, Language::Japanese);

        assert!(label.contains("リセットまで"), "{label}");
        assert!(!label.to_ascii_lowercase().contains("resets in"), "{label}");
        assert!(label.contains("2h 05m"), "{label}");
    }

    #[test]
    fn tray_status_label_relocalizes_without_refetch() {
        let window = snapshot_window_with(
            13.0,
            Some(300),
            Some(chrono::Utc::now() + chrono::Duration::minutes(125)),
            None,
        );

        let english = compact_tray_status_label(&window, Language::English);
        let japanese = compact_tray_status_label(&window, Language::Japanese);

        assert!(english.contains("Resets in"), "{english}");
        assert!(japanese.contains("リセットまで"), "{japanese}");
        assert!(
            !japanese.to_ascii_lowercase().contains("resets in"),
            "{japanese}"
        );
    }
}
