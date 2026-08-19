use super::*;

/// Raw on-disk shape of [`Settings`] used purely for deserialization.
///
/// It mirrors the canonical `Settings` fields but ALSO accepts the legacy
/// flat per-provider fields (`codex_cookie_source`, `alibaba_api_region`,
/// `claude_avoid_keychain_prompts`, …) so existing `settings.json` files keep
/// loading. The `From<RawSettings> for Settings` impl folds any present
/// legacy field into the unified [`provider_configs`](Settings::provider_configs)
/// map.
///
/// Saves go through `Settings`'s derived `Serialize`, which writes only the
/// new format (no legacy flat fields).
#[derive(Debug, Deserialize)]
#[serde(default)]
pub(super) struct RawSettings {
    enabled_providers: HashSet<String>,
    refresh_interval_secs: u64,
    #[serde(default)]
    adaptive_refresh: bool,
    refresh_all_providers_on_menu_open: bool,
    #[serde(default)]
    low_power_mode: bool,
    #[serde(default)]
    low_power_mode_preference: Option<LowPowerModePreference>,

    start_minimized: bool,
    start_at_login: bool,
    show_notifications: bool,
    sound_enabled: bool,
    notification_sound_paths: NotificationSoundPaths,
    notification_sound_theme: NotificationSoundTheme,
    high_usage_threshold: f64,
    critical_usage_threshold: f64,
    provider_usage_thresholds: HashMap<String, UsageThresholdOverride>,
    merge_tray_icons: bool,
    tray_icon_mode: TrayIconMode,
    #[serde(default = "default_true")]
    switcher_shows_icons: bool,
    menu_bar_shows_highest_usage: bool,
    menu_bar_shows_percent: bool,
    show_as_used: bool,
    enable_animations: bool,
    reset_time_relative: bool,
    show_reset_when_exhausted: bool,
    predictive_pace_warning_enabled: bool,
    menu_bar_display_mode: String,
    show_all_token_accounts_in_menu: bool,

    // ── New unified per-provider map ─────────────────────────────────
    provider_configs: HashMap<ProviderId, ProviderConfig>,

    // ── Legacy flat per-provider fields (migrated on load) ───────────
    #[serde(default)]
    claude_usage_source: Option<String>,
    #[serde(default)]
    codex_usage_source: Option<String>,
    #[serde(default)]
    codex_cookie_source: Option<String>,
    #[serde(default)]
    codex_historical_tracking: Option<bool>,
    #[serde(default)]
    codex_openai_web_extras: Option<bool>,
    #[serde(default)]
    claude_cookie_source: Option<String>,
    #[serde(default)]
    cursor_cookie_source: Option<String>,
    #[serde(default)]
    opencode_cookie_source: Option<String>,
    #[serde(default)]
    opencode_workspace_id: Option<String>,
    #[serde(default)]
    factory_cookie_source: Option<String>,
    #[serde(default)]
    alibaba_cookie_source: Option<String>,
    #[serde(default)]
    alibaba_cookie_header: Option<String>,
    #[serde(default)]
    alibaba_api_region: Option<String>,
    #[serde(default)]
    kimi_cookie_source: Option<String>,
    #[serde(default)]
    kimi_manual_cookie_header: Option<String>,
    #[serde(default)]
    minimax_cookie_source: Option<String>,
    #[serde(default)]
    augment_cookie_source: Option<String>,
    #[serde(default)]
    augment_cookie_header: Option<String>,
    #[serde(default)]
    amp_cookie_source: Option<String>,
    #[serde(default)]
    amp_cookie_header: Option<String>,
    #[serde(default)]
    ollama_cookie_source: Option<String>,
    #[serde(default)]
    ollama_cookie_header: Option<String>,
    #[serde(default)]
    zai_api_region: Option<String>,
    #[serde(default)]
    jetbrains_ide_base_path: Option<String>,
    #[serde(default)]
    minimax_cookie_header: Option<String>,
    #[serde(default)]
    minimax_api_token: Option<String>,
    #[serde(default)]
    minimax_api_region: Option<String>,
    #[serde(default)]
    claude_avoid_keychain_prompts: Option<bool>,

    disable_keychain_access: bool,
    hide_personal_info: bool,
    update_channel: UpdateChannel,
    provider_metrics: HashMap<String, MetricPreference>,
    provider_order: Vec<String>,
    #[serde(default = "default_global_shortcut")]
    global_shortcut: String,
    codex_custom_sessions_dirs: Vec<String>,
    agent_sessions_enabled: bool,
    agent_session_ssh_hosts: Vec<String>,
    #[serde(default)]
    hooks_enabled: bool,
    #[serde(default)]
    http_proxy_enabled: bool,
    #[serde(default)]
    http_proxy_url: String,
    #[serde(default)]
    http_proxy_username: String,
    #[serde(default)]
    http_proxy_password: String,
    auto_download_updates: bool,
    install_updates_on_quit: bool,
    ui_language: Language,
    theme: ThemePreference,
    #[serde(default = "default_window_scale_percent")]
    window_scale_percent: u16,
    #[serde(default = "default_tray_scale_percent")]
    tray_scale_percent: u16,
    #[serde(default)]
    powertoys_status_pipe_enabled: bool,

    #[serde(default)]
    float_bar_enabled: bool,
    #[serde(default = "default_float_bar_opacity")]
    float_bar_opacity: u8,
    #[serde(default = "default_float_bar_scale")]
    float_bar_scale: u8,
    #[serde(default = "default_float_bar_orientation")]
    float_bar_orientation: String,
    #[serde(default = "default_float_bar_style")]
    float_bar_style: String,
    #[serde(default)]
    float_bar_click_through: bool,
    #[serde(default)]
    float_bar_provider_ids: Vec<String>,
    #[serde(default)]
    float_bar_dark_text: bool,
    #[serde(default)]
    float_bar_show_reset_inline: bool,
    #[serde(default)]
    float_bar_show_cost: bool,
    #[serde(default = "default_true")]
    promote_tray_icon: bool,
    #[serde(default = "default_true")]
    claude_daily_routines_usage_visible: bool,
    #[serde(default)]
    claude_allow_reading_claude_code_credentials: bool,
    #[serde(default)]
    weekly_progress_work_days: Option<u8>,
    #[serde(default = "default_alibaba_token_plan_region")]
    alibaba_token_plan_region: String,
    #[serde(default)]
    codex_external_oauth_sources_allowed: bool,
    #[serde(default)]
    cost_summary_display_style: CostSummaryDisplayStyle,
    #[serde(default)]
    open_codex_usage_logs_enabled: bool,
    #[serde(default)]
    hide_native_codex_cost_when_open_codex_present: bool,
}

impl Default for RawSettings {
    fn default() -> Self {
        let s = Settings::default();
        Self {
            enabled_providers: s.enabled_providers,
            refresh_interval_secs: s.refresh_interval_secs,
            adaptive_refresh: s.adaptive_refresh,
            refresh_all_providers_on_menu_open: s.refresh_all_providers_on_menu_open,
            low_power_mode: s.low_power_mode_preference == LowPowerModePreference::On,
            low_power_mode_preference: Some(s.low_power_mode_preference),
            start_minimized: s.start_minimized,
            start_at_login: s.start_at_login,
            show_notifications: s.show_notifications,
            sound_enabled: s.sound_enabled,
            notification_sound_paths: s.notification_sound_paths,
            notification_sound_theme: s.notification_sound_theme,
            high_usage_threshold: s.high_usage_threshold,
            critical_usage_threshold: s.critical_usage_threshold,
            provider_usage_thresholds: HashMap::new(),
            merge_tray_icons: s.merge_tray_icons,
            tray_icon_mode: s.tray_icon_mode,
            switcher_shows_icons: s.switcher_shows_icons,
            menu_bar_shows_highest_usage: s.menu_bar_shows_highest_usage,
            menu_bar_shows_percent: s.menu_bar_shows_percent,
            show_as_used: s.show_as_used,
            enable_animations: s.enable_animations,
            reset_time_relative: s.reset_time_relative,
            show_reset_when_exhausted: s.show_reset_when_exhausted,
            predictive_pace_warning_enabled: s.predictive_pace_warning_enabled,
            menu_bar_display_mode: s.menu_bar_display_mode,
            show_all_token_accounts_in_menu: s.show_all_token_accounts_in_menu,
            provider_configs: s.provider_configs,
            claude_usage_source: None,
            codex_usage_source: None,
            codex_cookie_source: None,
            codex_historical_tracking: None,
            codex_openai_web_extras: None,
            claude_cookie_source: None,
            cursor_cookie_source: None,
            opencode_cookie_source: None,
            opencode_workspace_id: None,
            factory_cookie_source: None,
            alibaba_cookie_source: None,
            alibaba_cookie_header: None,
            alibaba_api_region: None,
            kimi_cookie_source: None,
            kimi_manual_cookie_header: None,
            minimax_cookie_source: None,
            augment_cookie_source: None,
            augment_cookie_header: None,
            amp_cookie_source: None,
            amp_cookie_header: None,
            ollama_cookie_source: None,
            ollama_cookie_header: None,
            zai_api_region: None,
            jetbrains_ide_base_path: None,
            minimax_cookie_header: None,
            minimax_api_token: None,
            minimax_api_region: None,
            claude_avoid_keychain_prompts: None,
            disable_keychain_access: s.disable_keychain_access,
            hide_personal_info: s.hide_personal_info,
            update_channel: s.update_channel,
            provider_metrics: s.provider_metrics,
            provider_order: s.provider_order,
            global_shortcut: s.global_shortcut,
            codex_custom_sessions_dirs: s.codex_custom_sessions_dirs,
            agent_sessions_enabled: s.agent_sessions_enabled,
            agent_session_ssh_hosts: s.agent_session_ssh_hosts,
            hooks_enabled: s.hooks_enabled,
            http_proxy_enabled: s.http_proxy_enabled,
            http_proxy_url: s.http_proxy_url,
            http_proxy_username: s.http_proxy_username,
            http_proxy_password: s.http_proxy_password,
            auto_download_updates: s.auto_download_updates,
            install_updates_on_quit: s.install_updates_on_quit,
            ui_language: s.ui_language,
            theme: s.theme,
            window_scale_percent: s.window_scale_percent,
            tray_scale_percent: s.tray_scale_percent,
            powertoys_status_pipe_enabled: s.powertoys_status_pipe_enabled,
            float_bar_enabled: s.float_bar_enabled,
            float_bar_opacity: s.float_bar_opacity,
            float_bar_scale: s.float_bar_scale,
            float_bar_orientation: s.float_bar_orientation,
            float_bar_style: s.float_bar_style,
            float_bar_click_through: s.float_bar_click_through,
            float_bar_provider_ids: s.float_bar_provider_ids,
            float_bar_dark_text: s.float_bar_dark_text,
            float_bar_show_reset_inline: s.float_bar_show_reset_inline,
            float_bar_show_cost: s.float_bar_show_cost,
            promote_tray_icon: s.promote_tray_icon,
            claude_daily_routines_usage_visible: s.claude_daily_routines_usage_visible,
            claude_allow_reading_claude_code_credentials: s
                .claude_allow_reading_claude_code_credentials,
            weekly_progress_work_days: s.weekly_progress_work_days,
            alibaba_token_plan_region: s.alibaba_token_plan_region,
            codex_external_oauth_sources_allowed: s.codex_external_oauth_sources_allowed,
            cost_summary_display_style: s.cost_summary_display_style,
            open_codex_usage_logs_enabled: s.open_codex_usage_logs_enabled,
            hide_native_codex_cost_when_open_codex_present: s.hide_native_codex_cost_when_open_codex_present,
        }
    }
}

impl From<RawSettings> for Settings {
    fn from(raw: RawSettings) -> Self {
        let mut provider_configs = raw.provider_configs;

        // Helper closures to lazily insert per-provider configs from legacy
        // flat fields. Existing `provider_configs` entries take precedence.
        fn set_cookie_source(
            map: &mut HashMap<ProviderId, ProviderConfig>,
            id: ProviderId,
            value: Option<String>,
        ) {
            if let Some(v) = value {
                let entry = map.entry(id).or_default();
                if entry.cookie_source.is_none() {
                    entry.cookie_source = Some(v);
                }
            }
        }
        fn set_usage_source(
            map: &mut HashMap<ProviderId, ProviderConfig>,
            id: ProviderId,
            value: Option<String>,
        ) {
            if let Some(v) = value {
                let entry = map.entry(id).or_default();
                if entry.usage_source.is_none() {
                    entry.usage_source = Some(v);
                }
            }
        }
        fn set_region(
            map: &mut HashMap<ProviderId, ProviderConfig>,
            id: ProviderId,
            value: Option<String>,
        ) {
            if let Some(v) = value {
                let entry = map.entry(id).or_default();
                if entry.api_region.is_none() {
                    entry.api_region = Some(v);
                }
            }
        }
        fn set_header(
            map: &mut HashMap<ProviderId, ProviderConfig>,
            id: ProviderId,
            value: Option<String>,
        ) {
            if let Some(v) = value {
                let entry = map.entry(id).or_default();
                if entry.manual_cookie_header.is_none() {
                    entry.manual_cookie_header = Some(v);
                }
            }
        }

        set_cookie_source(
            &mut provider_configs,
            ProviderId::Codex,
            raw.codex_cookie_source,
        );
        set_cookie_source(
            &mut provider_configs,
            ProviderId::Claude,
            raw.claude_cookie_source,
        );
        set_cookie_source(
            &mut provider_configs,
            ProviderId::Cursor,
            raw.cursor_cookie_source,
        );
        set_cookie_source(
            &mut provider_configs,
            ProviderId::OpenCode,
            raw.opencode_cookie_source,
        );
        set_cookie_source(
            &mut provider_configs,
            ProviderId::Factory,
            raw.factory_cookie_source,
        );
        set_cookie_source(
            &mut provider_configs,
            ProviderId::Alibaba,
            raw.alibaba_cookie_source,
        );
        set_cookie_source(
            &mut provider_configs,
            ProviderId::Kimi,
            raw.kimi_cookie_source,
        );
        set_cookie_source(
            &mut provider_configs,
            ProviderId::MiniMax,
            raw.minimax_cookie_source,
        );
        set_cookie_source(
            &mut provider_configs,
            ProviderId::Augment,
            raw.augment_cookie_source,
        );
        set_cookie_source(
            &mut provider_configs,
            ProviderId::Amp,
            raw.amp_cookie_source,
        );
        set_cookie_source(
            &mut provider_configs,
            ProviderId::Ollama,
            raw.ollama_cookie_source,
        );

        set_usage_source(
            &mut provider_configs,
            ProviderId::Claude,
            raw.claude_usage_source,
        );
        set_usage_source(
            &mut provider_configs,
            ProviderId::Codex,
            raw.codex_usage_source,
        );

        set_region(
            &mut provider_configs,
            ProviderId::Alibaba,
            raw.alibaba_api_region,
        );
        set_region(&mut provider_configs, ProviderId::Zai, raw.zai_api_region);
        set_region(
            &mut provider_configs,
            ProviderId::MiniMax,
            raw.minimax_api_region,
        );
        set_region(
            &mut provider_configs,
            ProviderId::AlibabaTokenPlan,
            Some(raw.alibaba_token_plan_region.clone()).filter(|v| !v.trim().is_empty()),
        );

        set_header(
            &mut provider_configs,
            ProviderId::Alibaba,
            raw.alibaba_cookie_header,
        );
        set_header(
            &mut provider_configs,
            ProviderId::Kimi,
            raw.kimi_manual_cookie_header,
        );
        set_header(
            &mut provider_configs,
            ProviderId::Augment,
            raw.augment_cookie_header,
        );
        set_header(
            &mut provider_configs,
            ProviderId::Amp,
            raw.amp_cookie_header,
        );
        set_header(
            &mut provider_configs,
            ProviderId::Ollama,
            raw.ollama_cookie_header,
        );
        set_header(
            &mut provider_configs,
            ProviderId::MiniMax,
            raw.minimax_cookie_header,
        );

        if let Some(v) = raw.opencode_workspace_id {
            let entry = provider_configs.entry(ProviderId::OpenCode).or_default();
            if entry.workspace_id.is_none() {
                entry.workspace_id = Some(v);
            }
        }
        if let Some(v) = raw.minimax_api_token {
            let entry = provider_configs.entry(ProviderId::MiniMax).or_default();
            if entry.api_token.is_none() {
                entry.api_token = Some(v);
            }
        }
        if let Some(v) = raw.jetbrains_ide_base_path {
            let entry = provider_configs.entry(ProviderId::JetBrains).or_default();
            if entry.ide_base_path.is_none() {
                entry.ide_base_path = Some(v);
            }
        }
        if let Some(v) = raw.codex_openai_web_extras {
            let entry = provider_configs.entry(ProviderId::Codex).or_default();
            if entry.openai_web_extras.is_none() {
                entry.openai_web_extras = Some(v);
            }
        }
        if let Some(v) = raw.codex_historical_tracking
            && v
        {
            provider_configs
                .entry(ProviderId::Codex)
                .or_default()
                .historical_tracking = true;
        }
        if let Some(v) = raw.claude_avoid_keychain_prompts
            && v
        {
            provider_configs
                .entry(ProviderId::Claude)
                .or_default()
                .avoid_keychain_prompts = true;
        }

        let low_power_mode_preference = match raw.low_power_mode_preference {
            // A legacy writer may know only the boolean and leave the new field
            // at its serialized default. Preserve the previously enabled state.
            Some(LowPowerModePreference::Off) if raw.low_power_mode => LowPowerModePreference::On,
            Some(preference) => preference,
            None if raw.low_power_mode => LowPowerModePreference::On,
            None => LowPowerModePreference::Off,
        };

        Settings {
            enabled_providers: raw.enabled_providers,
            refresh_interval_secs: raw.refresh_interval_secs,
            adaptive_refresh: raw.adaptive_refresh,
            refresh_all_providers_on_menu_open: raw.refresh_all_providers_on_menu_open,
            low_power_mode_preference,
            start_minimized: raw.start_minimized,
            start_at_login: raw.start_at_login,
            show_notifications: raw.show_notifications,
            sound_enabled: raw.sound_enabled,
            notification_sound_paths: raw.notification_sound_paths,
            notification_sound_theme: raw.notification_sound_theme,
            high_usage_threshold: raw.high_usage_threshold,
            critical_usage_threshold: raw.critical_usage_threshold,
            provider_usage_thresholds: normalize_usage_threshold_overrides(
                raw.provider_usage_thresholds,
            ),
            merge_tray_icons: raw.merge_tray_icons,
            tray_icon_mode: raw.tray_icon_mode,
            switcher_shows_icons: raw.switcher_shows_icons,
            menu_bar_shows_highest_usage: raw.menu_bar_shows_highest_usage,
            menu_bar_shows_percent: raw.menu_bar_shows_percent,
            show_as_used: raw.show_as_used,
            enable_animations: raw.enable_animations,
            reset_time_relative: raw.reset_time_relative,
            show_reset_when_exhausted: raw.show_reset_when_exhausted,
            predictive_pace_warning_enabled: raw.predictive_pace_warning_enabled,
            menu_bar_display_mode: raw.menu_bar_display_mode,
            show_all_token_accounts_in_menu: raw.show_all_token_accounts_in_menu,
            provider_configs,
            disable_keychain_access: raw.disable_keychain_access,
            hide_personal_info: raw.hide_personal_info,
            update_channel: raw.update_channel,
            provider_metrics: raw.provider_metrics,
            provider_order: if raw.provider_order.is_empty() {
                Vec::new()
            } else {
                normalize_provider_order(&raw.provider_order)
            },
            global_shortcut: raw.global_shortcut,
            codex_custom_sessions_dirs: raw.codex_custom_sessions_dirs,
            agent_sessions_enabled: raw.agent_sessions_enabled,
            agent_session_ssh_hosts: raw.agent_session_ssh_hosts,
            hooks_enabled: raw.hooks_enabled,
            http_proxy_enabled: raw.http_proxy_enabled,
            http_proxy_url: raw.http_proxy_url,
            http_proxy_username: raw.http_proxy_username,
            http_proxy_password: raw.http_proxy_password,
            auto_download_updates: raw.auto_download_updates,
            install_updates_on_quit: raw.install_updates_on_quit,
            ui_language: raw.ui_language,
            theme: raw.theme,
            window_scale_percent: clamp_window_scale_percent(raw.window_scale_percent),
            tray_scale_percent: clamp_tray_scale_percent(raw.tray_scale_percent),
            powertoys_status_pipe_enabled: raw.powertoys_status_pipe_enabled,
            float_bar_enabled: raw.float_bar_enabled,
            float_bar_opacity: clamp_float_bar_opacity(raw.float_bar_opacity),
            float_bar_scale: clamp_float_bar_scale(raw.float_bar_scale),
            float_bar_orientation: normalize_float_bar_orientation(&raw.float_bar_orientation),
            float_bar_style: normalize_float_bar_style(&raw.float_bar_style),
            float_bar_click_through: raw.float_bar_click_through,
            float_bar_provider_ids: raw.float_bar_provider_ids,
            float_bar_dark_text: raw.float_bar_dark_text,
            float_bar_show_reset_inline: raw.float_bar_show_reset_inline,
            float_bar_show_cost: raw.float_bar_show_cost,
            promote_tray_icon: raw.promote_tray_icon,
            claude_daily_routines_usage_visible: raw.claude_daily_routines_usage_visible,
            claude_allow_reading_claude_code_credentials: raw
                .claude_allow_reading_claude_code_credentials,
            weekly_progress_work_days: raw.weekly_progress_work_days,
            alibaba_token_plan_region: {
                let trimmed = raw.alibaba_token_plan_region.trim();
                if trimmed.is_empty() {
                    "cn".to_string()
                } else {
                    trimmed.to_string()
                }
            },
            cost_summary_display_style: raw.cost_summary_display_style,
            open_codex_usage_logs_enabled: raw.open_codex_usage_logs_enabled,
            hide_native_codex_cost_when_open_codex_present: raw.hide_native_codex_cost_when_open_codex_present,
            codex_external_oauth_sources_allowed: raw.codex_external_oauth_sources_allowed,
        }
    }
}
