use super::*;

// ── Settings mutation ─────────────────────────────────────────────────

/// Partial settings update — every field is optional so the frontend can
/// send only what changed.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SettingsUpdate {
    pub enabled_providers: Option<Vec<String>>,
    pub refresh_interval_secs: Option<u64>,
    pub adaptive_refresh: Option<bool>,
    pub refresh_all_providers_on_menu_open: Option<bool>,
    pub low_power_mode: Option<bool>,
    pub start_at_login: Option<bool>,
    pub start_minimized: Option<bool>,
    pub show_notifications: Option<bool>,
    pub sound_enabled: Option<bool>,
    pub notification_sound_theme: Option<codexbar::settings::NotificationSoundTheme>,
    pub notification_sound_paths: Option<codexbar::settings::NotificationSoundPaths>,
    pub high_usage_threshold: Option<f64>,
    pub critical_usage_threshold: Option<f64>,
    pub provider_usage_thresholds:
        Option<std::collections::HashMap<String, codexbar::settings::UsageThresholdOverride>>,
    pub predictive_pace_warning_enabled: Option<bool>,
    pub tray_icon_mode: Option<String>,
    pub switcher_shows_icons: Option<bool>,
    pub menu_bar_shows_highest_usage: Option<bool>,
    pub menu_bar_shows_percent: Option<bool>,
    pub show_as_used: Option<bool>,
    pub show_all_token_accounts_in_menu: Option<bool>,
    pub enable_animations: Option<bool>,
    pub reset_time_relative: Option<bool>,
    pub show_reset_when_exhausted: Option<bool>,
    pub menu_bar_display_mode: Option<String>,
    pub hide_personal_info: Option<bool>,
    pub update_channel: Option<String>,
    pub auto_download_updates: Option<bool>,
    pub install_updates_on_quit: Option<bool>,
    pub global_shortcut: Option<String>,
    pub codex_custom_sessions_dirs: Option<Vec<String>>,
    pub agent_sessions_enabled: Option<bool>,
    pub agent_session_ssh_hosts: Option<Vec<String>>,
    pub hooks_enabled: Option<bool>,
    pub http_proxy_enabled: Option<bool>,
    pub http_proxy_url: Option<String>,
    pub http_proxy_username: Option<String>,
    pub http_proxy_password: Option<String>,
    pub ui_language: Option<String>,
    pub theme: Option<String>,
    pub window_scale_percent: Option<u16>,
    pub tray_scale_percent: Option<u16>,
    pub powertoys_status_pipe_enabled: Option<bool>,
    pub claude_avoid_keychain_prompts: Option<bool>,
    pub codex_spark_usage_visible: Option<bool>,
    pub disable_keychain_access: Option<bool>,
    /// Map of provider CLI name → metric preference label.
    pub provider_metrics: Option<std::collections::HashMap<String, String>>,
    pub float_bar_enabled: Option<bool>,
    pub float_bar_opacity: Option<u8>,
    pub float_bar_scale: Option<u8>,
    pub float_bar_orientation: Option<String>,
    pub float_bar_style: Option<String>,
    pub float_bar_click_through: Option<bool>,
    pub float_bar_provider_ids: Option<Vec<String>>,
    pub float_bar_dark_text: Option<bool>,
    pub float_bar_show_reset_inline: Option<bool>,
    pub float_bar_show_cost: Option<bool>,
    pub promote_tray_icon: Option<bool>,
    pub claude_daily_routines_usage_visible: Option<bool>,
    pub alibaba_token_plan_region: Option<String>,
    pub weekly_progress_work_days: Option<u8>,
    pub cost_summary_display_style: Option<String>,
}

impl SettingsUpdate {
    fn refreshes_provider_data(&self) -> bool {
        self.enabled_providers.is_some()
            || self.claude_daily_routines_usage_visible.is_some()
            || self.alibaba_token_plan_region.is_some()
            || self.weekly_progress_work_days.is_some()
    }

    fn notifies_float_bar(&self) -> bool {
        self.enabled_providers.is_some()
            || self.refresh_interval_secs.is_some()
            || self.low_power_mode.is_some()
            || self.adaptive_refresh.is_some()
            || self.codex_custom_sessions_dirs.is_some()
            || self.high_usage_threshold.is_some()
            || self.critical_usage_threshold.is_some()
            || self.provider_usage_thresholds.is_some()
            || self.show_as_used.is_some()
            || self.reset_time_relative.is_some()
            || self.show_reset_when_exhausted.is_some()
    }

    fn rebuilds_tray_menu(&self) -> bool {
        self.float_bar_enabled.is_some() || self.ui_language.is_some()
    }

    pub fn changes_tray_promotion(&self) -> bool {
        self.promote_tray_icon.is_some()
    }

    fn refreshes_tray_presentation(&self) -> bool {
        self.tray_icon_mode.is_some()
            || self.switcher_shows_icons.is_some()
            || self.menu_bar_shows_highest_usage.is_some()
            || self.menu_bar_shows_percent.is_some()
            || self.show_as_used.is_some()
            || self.reset_time_relative.is_some()
            || self.menu_bar_display_mode.is_some()
            || self.provider_metrics.is_some()
            || self.codex_spark_usage_visible.is_some()
            || self.enabled_providers.is_some()
            || self.ui_language.is_some()
    }

    fn validate_shortcut_change(
        &self,
        app: &tauri::AppHandle,
        current_shortcut: &str,
    ) -> Result<(), String> {
        let Some(new_shortcut) = &self.global_shortcut else {
            return Ok(());
        };

        if new_shortcut.trim().is_empty() {
            crate::shortcut_bridge::unregister_shortcut(app, current_shortcut)?;
        } else if new_shortcut != current_shortcut {
            crate::shortcut_bridge::reregister_shortcut(app, current_shortcut, new_shortcut)?;
        }

        Ok(())
    }

    fn apply_provider_settings(self, settings: &mut Settings) -> Self {
        if let Some(providers) = self.enabled_providers.clone() {
            settings.enabled_providers = providers.into_iter().collect::<HashSet<_>>();
        }
        if let Some(v) = self.refresh_interval_secs {
            settings.refresh_interval_secs = v;
        }
        if let Some(v) = self.adaptive_refresh {
            settings.adaptive_refresh = v;
        }
        if let Some(v) = self.refresh_all_providers_on_menu_open {
            settings.refresh_all_providers_on_menu_open = v;
        }
        if let Some(v) = self.low_power_mode {
            settings.low_power_mode = v;
        }
        if let Some(ref s) = self.tray_icon_mode
            && let Some(mode) = parse_tray_icon_mode(s)
        {
            settings.tray_icon_mode = mode;
        }
        if let Some(v) = self.provider_metrics.clone() {
            apply_provider_metrics(settings, v);
        }
        self
    }

    fn apply_general_settings(self, settings: &mut Settings) -> Result<Self, String> {
        if let Some(v) = self.start_at_login {
            settings.set_start_at_login(v).map_err(|e| e.to_string())?;
        }
        if let Some(v) = self.start_minimized {
            settings.start_minimized = v;
        }
        if let Some(v) = self.global_shortcut.clone() {
            settings.global_shortcut = v;
        }
        if let Some(v) = self.ui_language.as_deref().and_then(parse_language)
            && settings.ui_language != v
        {
            settings.ui_language = v;
        }
        if let Some(v) = self.theme.as_deref().and_then(parse_theme) {
            settings.theme = v;
        }
        Ok(self)
    }

    fn apply_display_settings(self, settings: &mut Settings) -> Self {
        if let Some(v) = self.show_as_used {
            settings.show_as_used = v;
        }
        if let Some(v) = self.reset_time_relative {
            settings.reset_time_relative = v;
        }
        if let Some(v) = self.show_reset_when_exhausted {
            settings.show_reset_when_exhausted = v;
        }
        if let Some(v) = self.menu_bar_display_mode.clone() {
            settings.menu_bar_display_mode = v;
        }
        if let Some(v) = self.window_scale_percent {
            settings.window_scale_percent = codexbar::settings::clamp_window_scale_percent(v);
        }
        if let Some(v) = self.tray_scale_percent {
            settings.tray_scale_percent = codexbar::settings::clamp_tray_scale_percent(v);
        }
        if let Some(v) = self.switcher_shows_icons {
            settings.switcher_shows_icons = v;
        }
        if let Some(v) = self.menu_bar_shows_highest_usage {
            settings.menu_bar_shows_highest_usage = v;
        }
        if let Some(v) = self.menu_bar_shows_percent {
            settings.menu_bar_shows_percent = v;
        }
        if let Some(v) = self.show_all_token_accounts_in_menu {
            settings.show_all_token_accounts_in_menu = v;
        }
        if let Some(v) = self.promote_tray_icon {
            settings.promote_tray_icon = v;
        }
        self
    }

    fn apply_notification_settings(self, settings: &mut Settings) -> Result<Self, String> {
        if let Some(v) = self.show_notifications {
            settings.show_notifications = v;
        }
        if let Some(v) = self.sound_enabled {
            settings.sound_enabled = v;
        }
        if let Some(v) = self.notification_sound_theme {
            settings.notification_sound_theme = v;
        }
        if let Some(v) = self.notification_sound_paths.clone() {
            codexbar::sound::validate_custom_sound_path_updates(
                &settings.notification_sound_paths,
                &v,
            )
            .map_err(|error| error.to_string())?;
            settings.notification_sound_paths = v;
        }
        if let Some(v) = self.high_usage_threshold {
            settings.high_usage_threshold = v.clamp(0.0, 100.0);
        }
        if let Some(v) = self.critical_usage_threshold {
            settings.critical_usage_threshold = v.clamp(0.0, 100.0);
        }
        if let Some(values) = self.provider_usage_thresholds.clone() {
            settings.provider_usage_thresholds =
                codexbar::settings::normalize_usage_threshold_overrides(values);
        }
        if let Some(v) = self.predictive_pace_warning_enabled {
            settings.predictive_pace_warning_enabled = v;
        }
        Ok(self)
    }

    fn apply_advanced_settings(self, settings: &mut Settings) -> Self {
        if let Some(v) = self.enable_animations {
            settings.enable_animations = v;
        }
        if let Some(v) = self.hide_personal_info {
            settings.hide_personal_info = v;
        }
        if let Some(v) = self
            .update_channel
            .as_deref()
            .and_then(parse_update_channel)
        {
            settings.update_channel = v;
        }
        if let Some(v) = self.auto_download_updates {
            settings.auto_download_updates = v;
        }
        if let Some(v) = self.codex_custom_sessions_dirs.clone() {
            settings.codex_custom_sessions_dirs = normalize_custom_sessions_dirs(v);
        }
        if let Some(v) = self.agent_sessions_enabled {
            settings.agent_sessions_enabled = v;
        }
        if let Some(v) = self.agent_session_ssh_hosts.clone() {
            settings.agent_session_ssh_hosts =
                codexbar::agent_sessions::RemoteSessionFetcher::sanitized_hosts(&v);
        }
        if let Some(v) = self.hooks_enabled {
            settings.hooks_enabled = v;
        }
        if let Some(v) = self.http_proxy_enabled {
            settings.http_proxy_enabled = v;
        }
        if let Some(v) = self.http_proxy_url.clone() {
            settings.http_proxy_url = v.trim().to_string();
        }
        if let Some(v) = self.http_proxy_username.clone() {
            settings.http_proxy_username = v.trim().to_string();
        }
        if let Some(v) = self.http_proxy_password.clone() {
            settings.http_proxy_password = v;
        }
        if let Some(v) = self.install_updates_on_quit {
            settings.install_updates_on_quit = v;
        }
        if let Some(v) = self.powertoys_status_pipe_enabled {
            settings.powertoys_status_pipe_enabled = v;
        }
        if let Some(v) = self.claude_avoid_keychain_prompts {
            settings.set_claude_avoid_keychain_prompts(v);
        }
        if let Some(v) = self.codex_spark_usage_visible {
            settings.set_codex_spark_usage_visible(v);
        }
        if let Some(v) = self.disable_keychain_access {
            settings.disable_keychain_access = v;
            if v {
                settings.set_claude_avoid_keychain_prompts(true);
            }
        }
        if let Some(v) = self.claude_daily_routines_usage_visible {
            settings.claude_daily_routines_usage_visible = v;
        }
        if let Some(v) = self.alibaba_token_plan_region.as_deref() {
            let region = codexbar::providers::AlibabaTokenPlanRegion::from_settings_value(Some(v));
            settings.set_api_region(
                codexbar::core::ProviderId::AlibabaTokenPlan,
                region.as_str(),
            );
        }
        if let Some(v) = self.weekly_progress_work_days {
            settings.weekly_progress_work_days = if (2..=6).contains(&v) { Some(v) } else { None };
        }
        if let Some(v) = self
            .cost_summary_display_style
            .as_deref()
            .and_then(crate::commands::bridge::parse_cost_summary_display_style)
        {
            settings.cost_summary_display_style = v;
        }
        self
    }

    fn float_bar_patch(&self) -> crate::floatbar::SettingsPatch {
        crate::floatbar::SettingsPatch {
            enabled: self.float_bar_enabled,
            opacity: self.float_bar_opacity,
            scale: self.float_bar_scale,
            orientation: self.float_bar_orientation.clone(),
            style: self.float_bar_style.clone(),
            click_through: self.float_bar_click_through,
            provider_ids: self.float_bar_provider_ids.clone(),
            dark_text: self.float_bar_dark_text,
            show_reset_inline: self.float_bar_show_reset_inline,
            show_cost: self.float_bar_show_cost,
        }
    }

    fn apply_to(self, settings: &mut Settings) -> Result<crate::floatbar::SettingsPatch, String> {
        let float_bar_patch = self.float_bar_patch();
        self.apply_provider_settings(settings)
            .apply_general_settings(settings)?
            .apply_display_settings(settings)
            .apply_notification_settings(settings)?
            .apply_advanced_settings(settings);
        float_bar_patch.apply(settings);
        Ok(float_bar_patch)
    }
}

fn normalize_custom_sessions_dirs(dirs: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();

    for dir in dirs {
        let trimmed = dir.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = trimmed.replace('/', "\\").to_ascii_lowercase();
        if seen.insert(key) {
            out.push(trimmed.to_string());
        }
    }

    out
}

fn apply_provider_metrics(
    settings: &mut Settings,
    metrics_map: std::collections::HashMap<String, String>,
) {
    for (provider, label) in metrics_map {
        if let Some(pref) = parse_metric_preference(&label) {
            settings.provider_metrics.insert(provider, pref);
        }
    }
}

fn parse_tray_icon_mode(s: &str) -> Option<TrayIconMode> {
    match s {
        "single" => Some(TrayIconMode::Single),
        "perProvider" => Some(TrayIconMode::PerProvider),
        _ => None,
    }
}

fn parse_update_channel(s: &str) -> Option<UpdateChannel> {
    match s {
        "stable" => Some(UpdateChannel::Stable),
        "beta" => Some(UpdateChannel::Beta),
        _ => None,
    }
}

fn parse_language(s: &str) -> Option<Language> {
    Language::resolve(s)
}

#[tauri::command]
pub async fn update_settings(
    app: tauri::AppHandle,
    patch: SettingsUpdate,
) -> Result<SettingsSnapshot, String> {
    let mut settings = Settings::load();
    let notify_float_bar = patch.notifies_float_bar();
    let refresh_provider_data = patch.refreshes_provider_data();
    let clear_local_usage_cache = patch.codex_custom_sessions_dirs.is_some();
    let rebuild_tray_menu = patch.rebuilds_tray_menu();
    let refresh_tray_presentation = patch.refreshes_tray_presentation();
    let tray_promotion_changed = patch.changes_tray_promotion();
    let previous_promoted = settings.promote_tray_icon;
    let previous_language = settings.ui_language;

    patch.validate_shortcut_change(&app, &settings.global_shortcut)?;
    let float_bar_patch = patch.apply_to(&mut settings)?;

    if settings.ui_language != previous_language {
        let _ = app.emit(events::LOCALE_CHANGED, language_label(settings.ui_language));
    }

    settings.save().map_err(|e| e.to_string())?;
    if clear_local_usage_cache {
        crate::commands::clear_provider_local_usage_cache();
    }

    if refresh_provider_data {
        // Invalidate any in-flight publish work and drop disabled providers from
        // the live cache before a follow-up refresh starts.
        let enabled_ids = settings.get_enabled_provider_ids();
        let state = app.state::<Mutex<AppState>>();
        let _ =
            crate::commands::invalidate_provider_refresh_and_prune_disabled(&state, &enabled_ids);
    }

    crate::floatbar::after_settings_saved(&app, &float_bar_patch, &settings, notify_float_bar);
    if rebuild_tray_menu {
        crate::tray_bridge::rebuild_tray_menu(&app);
    }
    if refresh_tray_presentation {
        crate::tray_bridge::refresh_tray_presentation(&app);
    }
    if tray_promotion_changed {
        let new_promoted = settings.promote_tray_icon;
        if new_promoted
            || crate::tray_visibility::should_write_demotion(previous_promoted, new_promoted)
        {
            crate::tray_visibility::apply_promotion(new_promoted);
        }
    }

    // Notify other windows (PopOut dashboard, tray, float bar) so they re-read
    // settings live — e.g. the Display tab's window-scale slider takes effect
    // immediately instead of only after the PopOut is reopened.
    events::emit_settings_changed(&app);
    if refresh_provider_data {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let _ = crate::commands::do_refresh_providers(&app).await;
        });
    }

    Ok(SettingsSnapshot::from(settings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_data_affecting_settings_refresh_providers() {
        assert!(
            SettingsUpdate {
                enabled_providers: Some(vec!["codex".to_string()]),
                ..Default::default()
            }
            .refreshes_provider_data()
        );
        assert!(
            !SettingsUpdate {
                provider_metrics: Some(Default::default()),
                tray_icon_mode: Some("single".to_string()),
                ..Default::default()
            }
            .refreshes_provider_data()
        );
    }

    #[test]
    fn display_settings_that_affect_tray_trigger_presentation_refresh() {
        assert!(
            SettingsUpdate {
                switcher_shows_icons: Some(false),
                ..Default::default()
            }
            .refreshes_tray_presentation()
        );
        assert!(
            SettingsUpdate {
                reset_time_relative: Some(false),
                ..Default::default()
            }
            .refreshes_tray_presentation()
        );
    }

    #[test]
    fn ui_language_change_refreshes_tray_presentation() {
        assert!(
            SettingsUpdate {
                ui_language: Some("japanese".to_string()),
                ..Default::default()
            }
            .refreshes_tray_presentation()
        );
    }

    #[test]
    fn apply_display_settings_clamps_window_scale_percent() {
        let mut settings = Settings::default();

        SettingsUpdate {
            window_scale_percent: Some(300),
            ..Default::default()
        }
        .apply_display_settings(&mut settings);
        assert_eq!(settings.window_scale_percent, 250);

        SettingsUpdate {
            window_scale_percent: Some(50),
            ..Default::default()
        }
        .apply_display_settings(&mut settings);
        assert_eq!(settings.window_scale_percent, 100);
    }

    #[test]
    fn apply_display_settings_clamps_tray_scale_percent() {
        let mut settings = Settings::default();

        SettingsUpdate {
            tray_scale_percent: Some(300),
            ..Default::default()
        }
        .apply_display_settings(&mut settings);
        assert_eq!(settings.tray_scale_percent, 200);

        SettingsUpdate {
            tray_scale_percent: Some(50),
            ..Default::default()
        }
        .apply_display_settings(&mut settings);
        assert_eq!(settings.tray_scale_percent, 100);
    }

    #[test]
    fn apply_notification_settings_updates_sound() {
        let mut settings = Settings::default();

        SettingsUpdate {
            notification_sound_theme: Some(codexbar::settings::NotificationSoundTheme::CodexBar),
            ..Default::default()
        }
        .apply_notification_settings(&mut settings)
        .expect("apply sound theme");

        assert_eq!(
            settings.notification_sound_theme,
            codexbar::settings::NotificationSoundTheme::CodexBar
        );
    }

    #[test]
    fn apply_notification_settings_rejects_invalid_custom_sound() {
        let mut settings = Settings::default();
        let result = SettingsUpdate {
            notification_sound_paths: Some(codexbar::settings::NotificationSoundPaths {
                high_usage: Some("relative.wav".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        }
        .apply_notification_settings(&mut settings);

        assert!(result.is_err());
        assert_eq!(
            settings.notification_sound_paths,
            codexbar::settings::NotificationSoundPaths::default()
        );
    }
}
