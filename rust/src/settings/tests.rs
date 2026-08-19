use super::*;

#[test]
fn test_settings_default() {
    let settings = Settings::default();
    assert!(settings.enabled_providers.contains("claude"));
    assert!(settings.enabled_providers.contains("codex"));
    assert_eq!(settings.refresh_interval_secs, 300);
    assert!(settings.show_notifications);
    assert_eq!(
        settings.notification_sound_paths,
        NotificationSoundPaths::default()
    );
    assert_eq!(
        settings.notification_sound_theme,
        NotificationSoundTheme::Windows
    );
    assert_eq!(settings.high_usage_threshold, 70.0);
    assert_eq!(settings.critical_usage_threshold, 90.0);
    assert!(!settings.show_reset_when_exhausted);
    assert!(!settings.predictive_pace_warning_enabled);
    assert!(!settings.float_bar_show_cost);
    assert!(settings.promote_tray_icon);
    assert!(settings.claude_daily_routines_usage_visible);
    assert_eq!(settings.low_power_mode_preference, LowPowerModePreference::Off);
}

#[test]
fn low_power_mode_migrates_legacy_boolean_and_round_trips_preference() {
    let defaulted: Settings = serde_json::from_str(r#"{ "enabled_providers": [] }"#)
        .expect("missing low power fields defaults off");
    assert_eq!(defaulted.low_power_mode_preference, LowPowerModePreference::Off);

    let legacy: Settings = serde_json::from_str(r#"{ "enabled_providers": [], "low_power_mode": true }"#)
        .expect("legacy low_power_mode migrates");
    assert_eq!(legacy.low_power_mode_preference, LowPowerModePreference::On);

    let automatic = Settings {
        low_power_mode_preference: LowPowerModePreference::Automatic,
        ..Settings::default()
    };
    let json = serde_json::to_string(&automatic).expect("serialize low power preference");
    assert!(json.contains(r#""low_power_mode_preference":"automatic""#));
    let loaded: Settings = serde_json::from_str(&json).expect("deserialize low power preference");
    assert_eq!(loaded.low_power_mode_preference, LowPowerModePreference::Automatic);
}

#[test]
fn open_codex_usage_logs_default_off_and_round_trip() {
    let defaulted: Settings = serde_json::from_str(r#"{ "enabled_providers": [] }"#)
        .expect("missing open_codex_usage_logs_enabled defaults false");
    assert!(!defaulted.open_codex_usage_logs_enabled);

    let enabled = Settings {
        open_codex_usage_logs_enabled: true,
        ..Settings::default()
    };
    let json = serde_json::to_string(&enabled).expect("serialize OpenCodex usage opt-in");
    assert!(json.contains(r#""open_codex_usage_logs_enabled":true"#));

    let loaded: Settings = serde_json::from_str(&json).expect("deserialize OpenCodex usage opt-in");
    assert!(loaded.open_codex_usage_logs_enabled);
}

#[test]
fn notification_sound_paths_round_trip_and_default_for_existing_settings() {
    let settings = Settings {
        notification_sound_theme: NotificationSoundTheme::CodexBar,
        notification_sound_paths: NotificationSoundPaths {
            critical_usage: Some(r"C:\sounds\critical.wav".to_string()),
            ..NotificationSoundPaths::default()
        },
        ..Settings::default()
    };
    let json = serde_json::to_string(&settings).expect("serialize notification sound paths");
    assert!(json.contains("\"criticalUsage\":\"C:\\\\sounds\\\\critical.wav\""));

    let loaded: Settings =
        serde_json::from_str(&json).expect("deserialize notification sound paths");
    assert_eq!(
        loaded.notification_sound_paths,
        settings.notification_sound_paths
    );
    assert_eq!(
        loaded.notification_sound_theme,
        NotificationSoundTheme::CodexBar
    );

    let legacy: Settings = serde_json::from_str(r#"{ "enabled_providers": [] }"#)
        .expect("deserialize settings without notification sound paths");
    assert_eq!(
        legacy.notification_sound_paths,
        NotificationSoundPaths::default()
    );
    assert_eq!(
        legacy.notification_sound_theme,
        NotificationSoundTheme::Windows
    );
}

#[test]
fn promote_tray_icon_defaults_on_when_missing_from_disk() {
    let loaded: Settings = serde_json::from_str(
        r#"{
            "enabled_providers": ["claude", "codex"],
            "refresh_interval_secs": 300
        }"#,
    )
    .expect("parse settings without promote_tray_icon");
    assert!(loaded.promote_tray_icon);
}

#[test]
fn promote_tray_default_migration_flips_old_false_once() {
    assert!(Settings::should_migrate_promote_tray_default(false, false));
    assert!(!Settings::should_migrate_promote_tray_default(true, false));
    assert!(!Settings::should_migrate_promote_tray_default(false, true));
    assert!(!Settings::should_migrate_promote_tray_default(true, true));
}

#[test]
fn new_warning_and_reset_settings_are_backward_compatible() {
    let loaded: Settings = serde_json::from_str(
        r#"{
            "enabled_providers": ["claude", "codex"],
            "refresh_interval_secs": 300
        }"#,
    )
    .expect("parse legacy settings");

    assert!(!loaded.show_reset_when_exhausted);
    assert!(!loaded.predictive_pace_warning_enabled);
}

#[test]
fn usage_thresholds_inherit_from_window_provider_and_global_levels() {
    let mut settings = Settings::default();
    settings.provider_usage_thresholds.insert(
        "codex".into(),
        UsageThresholdOverride {
            high: Some(75.0),
            critical: None,
        },
    );
    settings.provider_usage_thresholds.insert(
        "codex:weekly".into(),
        UsageThresholdOverride {
            high: None,
            critical: Some(95.0),
        },
    );

    assert_eq!(
        settings.usage_thresholds(ProviderId::Codex, "weekly"),
        UsageThresholds {
            high: 75.0,
            critical: 95.0,
        }
    );
    assert_eq!(
        settings.usage_thresholds(ProviderId::Claude, "session"),
        UsageThresholds {
            high: 70.0,
            critical: 90.0,
        }
    );
}

#[test]
fn empty_and_out_of_range_threshold_overrides_are_normalized_on_load() {
    let loaded: Settings = serde_json::from_str(
        r#"{
            "provider_usage_thresholds": {
                "codex": {"high": 120.0},
                "claude": {},
                "codex:weekly": {"critical": -10.0}
            }
        }"#,
    )
    .expect("parse settings");

    assert_eq!(loaded.provider_usage_thresholds.len(), 2);
    assert_eq!(loaded.provider_usage_thresholds["codex"].high, Some(100.0));
    assert_eq!(
        loaded.provider_usage_thresholds["codex:weekly"].critical,
        Some(0.0)
    );
}

#[test]
fn float_bar_defaults_are_safe() {
    let settings = Settings::default();
    assert!(!settings.float_bar_enabled);
    assert_eq!(settings.float_bar_opacity, 80);
    assert_eq!(settings.float_bar_scale, 100);
    assert_eq!(settings.float_bar_orientation, "horizontal");
    assert_eq!(settings.float_bar_style, "floating");
    assert!(!settings.float_bar_click_through);
    assert!(settings.float_bar_provider_ids.is_empty());
    assert!(!settings.float_bar_dark_text);
    assert!(!settings.float_bar_show_reset_inline);
    assert!(!settings.float_bar_show_cost);
}

#[test]
fn main_window_scale_defaults_to_100_percent() {
    let settings = Settings::default();
    assert_eq!(settings.window_scale_percent, 100);
}

#[test]
fn main_window_scale_clamp_pins_to_supported_range() {
    assert_eq!(clamp_window_scale_percent(0), 100);
    assert_eq!(clamp_window_scale_percent(99), 100);
    assert_eq!(clamp_window_scale_percent(100), 100);
    assert_eq!(clamp_window_scale_percent(125), 125);
    assert_eq!(clamp_window_scale_percent(180), 180);
    assert_eq!(clamp_window_scale_percent(250), 250);
    assert_eq!(clamp_window_scale_percent(251), 250);
}

#[test]
fn raw_settings_clamps_main_window_scale_on_load() {
    let json = r#"{
            "enabled_providers": ["claude", "codex"],
            "refresh_interval_secs": 300,
            "window_scale_percent": 300
        }"#;
    let loaded: Settings = serde_json::from_str(json).expect("parse settings");
    assert_eq!(loaded.window_scale_percent, 250);
}

#[test]
fn tray_scale_defaults_to_100_percent() {
    let settings = Settings::default();
    assert_eq!(settings.tray_scale_percent, 100);
}

#[test]
fn tray_scale_clamp_pins_to_supported_range() {
    assert_eq!(clamp_tray_scale_percent(0), 100);
    assert_eq!(clamp_tray_scale_percent(99), 100);
    assert_eq!(clamp_tray_scale_percent(100), 100);
    assert_eq!(clamp_tray_scale_percent(125), 125);
    assert_eq!(clamp_tray_scale_percent(180), 180);
    assert_eq!(clamp_tray_scale_percent(200), 200);
    assert_eq!(clamp_tray_scale_percent(201), 200);
}

#[test]
fn raw_settings_clamps_tray_scale_on_load() {
    let json = r#"{
            "enabled_providers": ["claude", "codex"],
            "refresh_interval_secs": 300,
            "tray_scale_percent": 300
        }"#;
    let loaded: Settings = serde_json::from_str(json).expect("parse settings");
    assert_eq!(loaded.tray_scale_percent, 200);
}

#[test]
fn float_bar_opacity_clamp_pins_to_supported_range() {
    // Below 30 → 30 so the bar isn't accidentally invisible.
    assert_eq!(clamp_float_bar_opacity(0), 30);
    assert_eq!(clamp_float_bar_opacity(29), 30);
    // Within range → unchanged.
    assert_eq!(clamp_float_bar_opacity(45), 45);
    assert_eq!(clamp_float_bar_opacity(80), 80);
    // Above 100 → 100.
    assert_eq!(clamp_float_bar_opacity(150), 100);
    assert_eq!(clamp_float_bar_opacity(255), 100);
}

#[test]
fn float_bar_scale_clamp_pins_to_supported_range() {
    assert_eq!(clamp_float_bar_scale(0), 75);
    assert_eq!(clamp_float_bar_scale(74), 75);
    assert_eq!(clamp_float_bar_scale(100), 100);
    assert_eq!(clamp_float_bar_scale(150), 150);
    assert_eq!(clamp_float_bar_scale(250), 200);
}

#[test]
fn float_bar_orientation_normalization_rejects_unknown_values() {
    assert_eq!(normalize_float_bar_orientation("horizontal"), "horizontal");
    assert_eq!(normalize_float_bar_orientation("vertical"), "vertical");
    // Anything else collapses to horizontal so a corrupt settings file
    // can't poison the renderer with an unknown layout token.
    assert_eq!(normalize_float_bar_orientation(""), "horizontal");
    assert_eq!(normalize_float_bar_orientation("diagonal"), "horizontal");
    assert_eq!(normalize_float_bar_orientation("VERTICAL"), "horizontal");
}

#[test]
fn float_bar_style_normalization_rejects_unknown_values() {
    assert_eq!(normalize_float_bar_style("floating"), "floating");
    assert_eq!(normalize_float_bar_style("taskbar"), "taskbar");
    assert_eq!(normalize_float_bar_style(""), "floating");
    assert_eq!(normalize_float_bar_style("TASKBAR"), "floating");
    assert_eq!(normalize_float_bar_style("glass"), "floating");
}

#[test]
fn float_bar_settings_round_trip_through_raw() {
    // Serialize a Settings with custom float-bar values then deserialize
    // through the `from = "RawSettings"` path — values must survive intact
    // (after clamping/normalization).
    let s = Settings {
        float_bar_enabled: true,
        float_bar_opacity: 65,
        float_bar_scale: 140,
        float_bar_orientation: "vertical".to_string(),
        float_bar_style: "taskbar".to_string(),
        float_bar_click_through: true,
        float_bar_provider_ids: vec!["claude".into(), "codex".into()],
        float_bar_dark_text: true,
        float_bar_show_reset_inline: true,
        float_bar_show_cost: true,
        ..Settings::default()
    };

    let json = serde_json::to_string(&s).expect("serialize");
    let back: Settings = serde_json::from_str(&json).expect("deserialize");
    assert!(back.float_bar_enabled);
    assert_eq!(back.float_bar_opacity, 65);
    assert_eq!(back.float_bar_scale, 140);
    assert_eq!(back.float_bar_orientation, "vertical");
    assert_eq!(back.float_bar_style, "taskbar");
    assert!(back.float_bar_click_through);
    assert_eq!(back.float_bar_provider_ids, vec!["claude", "codex"]);
    assert!(back.float_bar_dark_text);
    assert!(back.float_bar_show_reset_inline);
    assert!(back.float_bar_show_cost);
}

#[test]
fn float_bar_raw_clamps_out_of_range_opacity_on_load() {
    // Simulate an externally-edited settings.json with a wild opacity.
    let json = r#"{
            "enabled_providers": [],
            "refresh_interval_secs": 300,
            "start_minimized": false,
            "start_at_login": false,
            "show_notifications": true,
            "sound_enabled": true,
            "high_usage_threshold": 70.0,
            "critical_usage_threshold": 90.0,
            "merge_tray_icons": false,
            "show_as_used": true,
            "enable_animations": true,
            "reset_time_relative": true,
            "menu_bar_display_mode": "detailed",
            "disable_keychain_access": false,
            "hide_personal_info": false,
            "float_bar_opacity": 250,
            "float_bar_scale": 250,
            "float_bar_orientation": "diagonal",
            "float_bar_style": "glass"
        }"#;
    let loaded: Settings = serde_json::from_str(json).expect("parse");
    assert_eq!(loaded.float_bar_opacity, 100);
    assert_eq!(loaded.float_bar_scale, 200);
    assert_eq!(loaded.float_bar_orientation, "horizontal");
    assert_eq!(loaded.float_bar_style, "floating");
}

#[test]
fn test_settings_provider_enabled() {
    let settings = Settings::default();
    assert!(settings.is_provider_enabled(ProviderId::Claude));
    assert!(settings.is_provider_enabled(ProviderId::Codex));
    assert!(!settings.is_provider_enabled(ProviderId::Gemini));
    assert!(!settings.is_provider_enabled(ProviderId::Wayfinder));
    assert_eq!(
        settings.gateway_url(ProviderId::Wayfinder),
        "http://127.0.0.1:8088"
    );
}

#[test]
fn wayfinder_gateway_round_trips_without_changing_settings_paths() {
    let mut settings = Settings::default();
    settings.set_gateway_url(
        ProviderId::Wayfinder,
        "https://gateway.example.test/wayfinder/",
    );

    let json = serde_json::to_string(&settings).expect("serialize settings");
    let loaded: Settings = serde_json::from_str(&json).expect("deserialize settings");
    assert_eq!(
        loaded.gateway_url(ProviderId::Wayfinder),
        "https://gateway.example.test/wayfinder/"
    );
}

#[test]
fn test_settings_toggle_provider() {
    let mut settings = Settings::default();

    // Claude starts enabled
    assert!(settings.is_provider_enabled(ProviderId::Claude));

    // Toggle off
    let enabled = settings.toggle_provider(ProviderId::Claude);
    assert!(!enabled);
    assert!(!settings.is_provider_enabled(ProviderId::Claude));

    // Toggle back on
    let enabled = settings.toggle_provider(ProviderId::Claude);
    assert!(enabled);
    assert!(settings.is_provider_enabled(ProviderId::Claude));
}

#[test]
fn test_settings_get_enabled_provider_ids() {
    let settings = Settings::default();
    let enabled = settings.get_enabled_provider_ids();
    assert!(enabled.contains(&ProviderId::Claude));
    assert!(enabled.contains(&ProviderId::Codex));
}

#[test]
fn provider_order_dedupes_unknowns_and_appends_canonical_ids() {
    let order = normalize_provider_order(&[
        "gemini".to_string(),
        "not-a-provider".to_string(),
        "claude".to_string(),
        "gemini".to_string(),
    ]);

    assert_eq!(order[0], "gemini");
    assert_eq!(order[1], "claude");
    assert!(!order.iter().any(|id| id == "not-a-provider"));
    assert_eq!(order.len(), ProviderId::all().len());
}

#[test]
fn enabled_provider_ids_follow_custom_provider_order() {
    let settings = Settings {
        enabled_providers: ["claude", "codex", "gemini"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        provider_order: normalize_provider_order(&[
            "gemini".to_string(),
            "claude".to_string(),
            "codex".to_string(),
        ]),
        ..Settings::default()
    };

    assert_eq!(
        settings.get_enabled_provider_ids(),
        vec![ProviderId::Gemini, ProviderId::Claude, ProviderId::Codex]
    );
}

#[test]
fn test_settings_get_all_providers_status() {
    let settings = Settings::default();
    let status = settings.get_all_providers_status();
    assert_eq!(status.len(), ProviderId::all().len());

    let claude_status = status.iter().find(|s| s.id == "claude").unwrap();
    assert_eq!(claude_status.name, "Claude");
    assert!(claude_status.enabled);

    let gemini_status = status.iter().find(|s| s.id == "gemini").unwrap();
    assert!(!gemini_status.enabled);
}

#[test]
fn test_api_key_provider_catalog_includes_token_providers() {
    let providers = get_api_key_providers();
    for id in [
        ProviderId::Kilo,
        ProviderId::Bedrock,
        ProviderId::Codebuff,
        ProviderId::DeepSeek,
        ProviderId::DeepInfra,
        ProviderId::AiAnd,
        ProviderId::ElevenLabs,
        ProviderId::Deepgram,
        ProviderId::Grok,
        ProviderId::Groq,
        ProviderId::LLMProxy,
        ProviderId::Xai,
    ] {
        assert!(
            providers.iter().any(|provider| provider.id == id),
            "{id} should be configurable from the API Keys UI"
        );
    }
}

#[test]
fn test_t3_chat_is_cookie_configured_not_api_key_configured() {
    let providers = get_api_key_providers();
    assert!(
        !providers
            .iter()
            .any(|provider| provider.id == ProviderId::T3Chat),
        "T3 Chat fetches usage from browser cookies or pasted cURL, not API keys"
    );
}

#[test]
fn test_refresh_interval_options() {
    let options = get_refresh_interval_options();
    assert!(!options.is_empty());
    assert!(options.iter().any(|o| o.value == 60));
    assert!(options.iter().any(|o| o.value == 300));
}

#[test]
fn test_manual_cookies_default() {
    let cookies = ManualCookies::default();
    assert!(cookies.cookies.is_empty());
}

#[test]
fn test_manual_cookies_set_get_remove() {
    let mut cookies = ManualCookies::default();

    // Set a cookie
    cookies.set("claude", "session=abc123");
    assert_eq!(cookies.get("claude"), Some("session=abc123"));

    // Remove it
    cookies.remove("claude");
    assert_eq!(cookies.get("claude"), None);
}

#[test]
fn api_key_display_mask_is_utf8_safe() {
    let mut keys = ApiKeys::default();
    keys.set("openrouter", "🔑🔒漢字abcdefgh🔐", Some("unicode"));

    let display = keys.get_all_for_display();

    assert_eq!(display.len(), 1);
    assert_eq!(display[0].masked_key, "🔑🔒漢字...fgh🔐");
}

#[test]
fn test_start_at_login_command_uses_only_the_executable_path() {
    let path = std::path::PathBuf::from(r"C:\Program Files\CodexBar\codexbar-desktop-tauri.exe");
    let command = Settings::start_at_login_command(&path);
    assert_eq!(
        command,
        "\"C:\\Program Files\\CodexBar\\codexbar-desktop-tauri.exe\""
    );
    assert!(!command.contains("menubar"));
}

#[test]
fn test_start_at_login_prefers_desktop_sibling_when_called_from_cli() {
    let temp = tempfile::tempdir().expect("temp dir");
    let cli_path = temp.path().join("codexbar-cli.exe");
    let desktop_path = temp.path().join("codexbar.exe");
    std::fs::write(&cli_path, b"cli").expect("write cli");
    std::fs::write(&desktop_path, b"desktop").expect("write desktop");

    let command = Settings::start_at_login_command(&cli_path);

    assert_eq!(command, format!("\"{}\"", desktop_path.display()));
}

#[test]
fn test_start_at_login_keeps_current_exe_when_desktop_sibling_missing() {
    let temp = tempfile::tempdir().expect("temp dir");
    let cli_path = temp.path().join("codexbar-cli.exe");
    std::fs::write(&cli_path, b"cli").expect("write cli");

    let command = Settings::start_at_login_command(&cli_path);

    assert_eq!(command, format!("\"{}\"", cli_path.display()));
}

#[test]
fn test_start_at_login_repairs_stale_cli_command_after_update() {
    let temp = tempfile::tempdir().expect("temp dir");
    let cli_path = temp.path().join("codexbar-cli.exe");
    let desktop_path = temp.path().join("codexbar.exe");
    std::fs::write(&cli_path, b"cli").expect("write cli");
    std::fs::write(&desktop_path, b"desktop").expect("write desktop");
    let stale_command = format!("\"{}\"", cli_path.display());

    assert!(Settings::start_at_login_command_needs_repair(
        &stale_command,
        &desktop_path
    ));
}

#[test]
fn test_start_at_login_keeps_current_desktop_command_after_update() {
    let temp = tempfile::tempdir().expect("temp dir");
    let desktop_path = temp.path().join("codexbar.exe");
    std::fs::write(&desktop_path, b"desktop").expect("write desktop");
    let current_command = format!("\"{}\"", desktop_path.display());

    assert!(!Settings::start_at_login_command_needs_repair(
        &current_command,
        &desktop_path
    ));
}

#[test]
fn test_start_at_login_repairs_legacy_desktop_command_after_update() {
    let temp = tempfile::tempdir().expect("temp dir");
    let desktop_path = temp.path().join("codexbar.exe");
    let legacy_desktop_path = temp.path().join("codexbar-desktop.exe");
    std::fs::write(&desktop_path, b"desktop").expect("write desktop");
    std::fs::write(&legacy_desktop_path, b"legacy desktop").expect("write legacy desktop");
    let stale_command = format!("\"{}\"", legacy_desktop_path.display());

    assert!(Settings::start_at_login_command_needs_repair(
        &stale_command,
        &legacy_desktop_path
    ));
}

#[test]
fn test_language_defaults_to_english() {
    let settings = Settings::default();
    assert_eq!(settings.ui_language, Language::English);
}

#[test]
fn test_language_all_variants_available() {
    let languages = Language::all();
    assert_eq!(languages.len(), 8);
    assert!(languages.contains(&Language::English));
    assert!(languages.contains(&Language::Chinese));
    assert!(languages.contains(&Language::ChineseTraditional));
    assert!(languages.contains(&Language::Japanese));
    assert!(languages.contains(&Language::Korean));
    assert!(languages.contains(&Language::Spanish));
    assert!(languages.contains(&Language::Russian));
    assert!(languages.contains(&Language::Turkish));
}

#[test]
fn test_language_display_names() {
    assert_eq!(Language::English.display_name(), "English");
    assert_eq!(Language::Chinese.display_name(), "中文");
    assert_eq!(Language::ChineseTraditional.display_name(), "繁體中文");
    assert_eq!(Language::Japanese.display_name(), "日本語");
    assert_eq!(Language::Russian.display_name(), "Русский");
    assert_eq!(Language::Turkish.display_name(), "Türkçe");
}

#[test]
fn test_language_resolves_russian_aliases() {
    assert_eq!(Language::resolve("russian"), Some(Language::Russian));
    assert_eq!(Language::resolve("ru-RU"), Some(Language::Russian));
    assert_eq!(Language::resolve("Русский"), Some(Language::Russian));
}

#[test]
fn test_language_resolves_turkish_aliases() {
    assert_eq!(Language::resolve("turkish"), Some(Language::Turkish));
    assert_eq!(Language::resolve("tr-TR"), Some(Language::Turkish));
    assert_eq!(Language::resolve("Türkçe"), Some(Language::Turkish));
    assert_eq!(Language::resolve("turkce"), Some(Language::Turkish));
}

#[test]
fn test_settings_load_missing_language_field_defaults_to_english() {
    // Simulate loading legacy settings JSON without ui_language field
    let legacy_json = r#"{
            "enabled_providers": ["claude", "codex"],
            "refresh_interval_secs": 300,
            "start_minimized": false,
            "ui_language": "english"
        }"#;

    let settings: Result<Settings, _> = serde_json::from_str(legacy_json);
    assert!(settings.is_ok());
    let settings = settings.unwrap();
    assert_eq!(settings.ui_language, Language::English);
}

#[test]
fn test_settings_roundtrip_with_language() {
    use std::io::Write;
    use tempfile::NamedTempFile;

    // Create settings with Chinese language
    let settings = Settings {
        ui_language: Language::Chinese,
        ..Settings::default()
    };

    // Save to a temp file
    let mut temp_file = NamedTempFile::new().expect("Failed to create temp file");
    let json = serde_json::to_string_pretty(&settings).expect("Failed to serialize settings");
    temp_file
        .write_all(json.as_bytes())
        .expect("Failed to write settings");
    let path = temp_file.path().to_path_buf();

    // Read back and verify
    let content = std::fs::read_to_string(&path).expect("Failed to read settings");
    let loaded: Settings = serde_json::from_str(&content).expect("Failed to deserialize settings");

    assert_eq!(loaded.ui_language, Language::Chinese);
}

#[test]
fn test_settings_with_utf8_bom_parses_perprovider_tray_mode() {
    let json = "\u{feff}{\n            \"enabled_providers\": [\"claude\", \"codex\"],\n            \"refresh_interval_secs\": 300,\n            \"tray_icon_mode\": \"perprovider\"\n        }";

    let settings: Settings = serde_json::from_str(json.trim_start_matches('\u{feff}')).unwrap();

    assert_eq!(settings.tray_icon_mode, TrayIconMode::PerProvider);
}

#[test]
fn test_language_serde_serialization() {
    // Test that Language serializes to lowercase string
    let english = Language::English;
    let chinese = Language::Chinese;
    let chinese_traditional = Language::ChineseTraditional;

    let english_json = serde_json::to_string(&english).unwrap();
    let chinese_json = serde_json::to_string(&chinese).unwrap();
    let chinese_traditional_json = serde_json::to_string(&chinese_traditional).unwrap();

    assert_eq!(english_json, "\"english\"");
    assert_eq!(chinese_json, "\"chinese\"");
    assert_eq!(chinese_traditional_json, "\"chinesetraditional\"");
}

#[test]
fn test_language_serde_deserialization() {
    // Test that lowercase strings deserialize correctly
    let english: Language = serde_json::from_str("\"english\"").unwrap();
    let chinese: Language = serde_json::from_str("\"chinese\"").unwrap();
    let chinese_traditional: Language = serde_json::from_str("\"chinesetraditional\"").unwrap();

    assert_eq!(english, Language::English);
    assert_eq!(chinese, Language::Chinese);
    assert_eq!(chinese_traditional, Language::ChineseTraditional);
}

#[test]
fn test_language_resolves_traditional_chinese_aliases() {
    assert_eq!(
        Language::resolve("chinesetraditional"),
        Some(Language::ChineseTraditional)
    );
    assert_eq!(
        Language::resolve("zh-tw"),
        Some(Language::ChineseTraditional)
    );
    assert_eq!(
        Language::resolve("zh-hant-tw"),
        Some(Language::ChineseTraditional)
    );
    assert_eq!(
        Language::resolve("繁體中文"),
        Some(Language::ChineseTraditional)
    );
}

#[test]
fn test_theme_defaults_to_auto() {
    let settings = Settings::default();
    assert_eq!(settings.theme, ThemePreference::Auto);
}

#[test]
fn test_theme_all_variants_available() {
    let themes = ThemePreference::all();
    assert_eq!(themes.len(), 3);
    assert!(themes.contains(&ThemePreference::Auto));
    assert!(themes.contains(&ThemePreference::Light));
    assert!(themes.contains(&ThemePreference::Dark));
}

#[test]
fn test_theme_serde_roundtrip() {
    for variant in [
        ThemePreference::Auto,
        ThemePreference::Light,
        ThemePreference::Dark,
    ] {
        let encoded = serde_json::to_string(&variant).unwrap();
        let decoded: ThemePreference = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, variant);
    }
    assert_eq!(
        serde_json::to_string(&ThemePreference::Light).unwrap(),
        "\"light\""
    );
    assert_eq!(
        serde_json::to_string(&ThemePreference::Dark).unwrap(),
        "\"dark\""
    );
    assert_eq!(
        serde_json::to_string(&ThemePreference::Auto).unwrap(),
        "\"auto\""
    );
}

#[test]
fn test_settings_missing_theme_defaults_to_auto() {
    // Legacy settings JSON without the theme field should still parse.
    let legacy_json = r#"{
            "enabled_providers": ["claude", "codex"],
            "refresh_interval_secs": 300,
            "ui_language": "english"
        }"#;

    let settings: Settings = serde_json::from_str(legacy_json).unwrap();
    assert_eq!(settings.theme, ThemePreference::Auto);
}

#[test]
fn test_settings_roundtrip_with_theme() {
    let settings = Settings {
        theme: ThemePreference::Dark,
        ..Settings::default()
    };
    let json = serde_json::to_string(&settings).unwrap();
    let loaded: Settings = serde_json::from_str(&json).unwrap();
    assert_eq!(loaded.theme, ThemePreference::Dark);
}

// ── Phase 3: provider_configs migration tests ───────────────────────

/// Loading a legacy `settings.json` (with flat per-provider fields)
/// must populate `provider_configs` and surface every value through the
/// per-provider accessors.
#[test]
fn test_legacy_per_provider_fields_migrate_into_provider_configs() {
    // NOTE: placeholder values only — no real cookies/tokens.
    let legacy_json = r#"{
            "enabled_providers": ["claude", "codex"],
            "refresh_interval_secs": 300,
            "codex_cookie_source": "manual",
            "claude_cookie_source": "browser",
            "cursor_cookie_source": "manual",
            "alibaba_cookie_source": "manual",
            "alibaba_cookie_header": "ali=PLACEHOLDER",
            "alibaba_api_region": "cn",
            "zai_api_region": "cn",
            "minimax_api_region": "cn",
            "minimax_api_token": "TOK_PLACEHOLDER",
            "claude_usage_source": "ccusage",
            "codex_usage_source": "manual",
            "codex_openai_web_extras": false,
            "codex_historical_tracking": true,
            "claude_avoid_keychain_prompts": true,
            "opencode_workspace_id": "ws_placeholder",
            "jetbrains_ide_base_path": "C:/JB"
        }"#;

    let settings: Settings = serde_json::from_str(legacy_json).unwrap();

    // Cookie sources
    assert_eq!(settings.cookie_source(ProviderId::Codex), "manual");
    assert_eq!(settings.cookie_source(ProviderId::Claude), "browser");
    assert_eq!(settings.cookie_source(ProviderId::Cursor), "manual");
    assert_eq!(settings.cookie_source(ProviderId::Alibaba), "manual");
    // Untouched providers fall through to the default "manual" to avoid
    // background browser-cookie reads unless the user opts into Automatic.
    assert_eq!(settings.cookie_source(ProviderId::Amp), "manual");

    // Manual cookie headers + api regions
    assert_eq!(
        settings.manual_cookie_header(ProviderId::Alibaba),
        "ali=PLACEHOLDER"
    );
    assert_eq!(settings.api_region(ProviderId::Alibaba), "cn");
    assert_eq!(settings.api_region(ProviderId::Zai), "cn");
    assert_eq!(settings.api_region(ProviderId::MiniMax), "cn");

    // Usage sources
    assert_eq!(settings.usage_source(ProviderId::Claude), "ccusage");
    assert_eq!(settings.usage_source(ProviderId::Codex), "manual");

    // Codex booleans
    assert!(!settings.openai_web_extras(ProviderId::Codex));
    assert!(settings.historical_tracking(ProviderId::Codex));

    // Claude per-provider boolean
    assert!(settings.avoid_keychain_prompts(ProviderId::Claude));

    // Misc per-provider strings
    assert_eq!(
        settings.workspace_id(ProviderId::OpenCode),
        "ws_placeholder"
    );
    assert_eq!(settings.api_token(ProviderId::MiniMax), "TOK_PLACEHOLDER");
    assert_eq!(settings.ide_base_path(ProviderId::JetBrains), "C:/JB");

    // Legacy field-name aliases agree with typed accessors.
    assert_eq!(settings.codex_cookie_source(), "manual");
    assert_eq!(settings.alibaba_api_region(), "cn");
    assert!(settings.codex_historical_tracking());
    assert!(!settings.codex_openai_web_extras());
    assert!(settings.claude_avoid_keychain_prompts());
}

/// Round-trip: build a `Settings` programmatically via the new map +
/// accessors, serialize, parse back, and assert equality of every
/// per-provider field.
#[test]
fn test_provider_configs_roundtrip() {
    let mut settings = Settings::default();
    settings.set_cookie_source(ProviderId::Codex, "manual");
    settings.set_cookie_source(ProviderId::Claude, "browser");
    settings.set_usage_source(ProviderId::Claude, "ccusage");
    settings.set_api_region(ProviderId::Alibaba, "cn");
    settings.set_api_region(ProviderId::Zai, "cn");
    settings.set_manual_cookie_header(ProviderId::Amp, "amp=PLACEHOLDER");
    settings.set_api_token(ProviderId::MiniMax, "TOK_PLACEHOLDER");
    settings.set_workspace_id(ProviderId::OpenCode, "ws_placeholder");
    settings.set_ide_base_path(ProviderId::JetBrains, "C:/JB");
    settings.set_openai_web_extras(ProviderId::Codex, false);
    settings.set_historical_tracking(ProviderId::Codex, true);
    settings.set_avoid_keychain_prompts(ProviderId::Claude, true);

    let json = serde_json::to_string(&settings).unwrap();
    // The legacy flat fields must NOT appear in serialized output.
    assert!(!json.contains("\"codex_cookie_source\""), "json: {json}");
    assert!(!json.contains("\"alibaba_api_region\""), "json: {json}");
    assert!(
        !json.contains("\"claude_avoid_keychain_prompts\""),
        "json: {json}"
    );
    assert!(json.contains("\"provider_configs\""), "json: {json}");

    let loaded: Settings = serde_json::from_str(&json).unwrap();
    assert_eq!(loaded.cookie_source(ProviderId::Codex), "manual");
    assert_eq!(loaded.cookie_source(ProviderId::Claude), "browser");
    assert_eq!(loaded.usage_source(ProviderId::Claude), "ccusage");
    assert_eq!(loaded.api_region(ProviderId::Alibaba), "cn");
    assert_eq!(loaded.api_region(ProviderId::Zai), "cn");
    assert_eq!(
        loaded.manual_cookie_header(ProviderId::Amp),
        "amp=PLACEHOLDER"
    );
    assert_eq!(loaded.api_token(ProviderId::MiniMax), "TOK_PLACEHOLDER");
    assert_eq!(loaded.workspace_id(ProviderId::OpenCode), "ws_placeholder");
    assert_eq!(loaded.ide_base_path(ProviderId::JetBrains), "C:/JB");
    assert!(!loaded.openai_web_extras(ProviderId::Codex));
    assert!(loaded.historical_tracking(ProviderId::Codex));
    assert!(loaded.avoid_keychain_prompts(ProviderId::Claude));
    assert_eq!(
        loaded.provider_configs.get(&ProviderId::Codex),
        settings.provider_configs.get(&ProviderId::Codex)
    );
}

/// New-format files (no legacy flat fields, only `provider_configs`)
/// must load identically.
#[test]
fn test_new_format_provider_configs_only() {
    let json = r#"{
            "enabled_providers": ["claude"],
            "refresh_interval_secs": 300,
            "provider_configs": {
                "codex": { "cookie_source": "manual", "openai_web_extras": false },
                "alibaba": { "api_region": "cn", "manual_cookie_header": "ali=PLACEHOLDER" }
            }
        }"#;

    let settings: Settings = serde_json::from_str(json).unwrap();
    assert_eq!(settings.cookie_source(ProviderId::Codex), "manual");
    assert!(!settings.openai_web_extras(ProviderId::Codex));
    assert_eq!(settings.api_region(ProviderId::Alibaba), "cn");
    assert_eq!(
        settings.manual_cookie_header(ProviderId::Alibaba),
        "ali=PLACEHOLDER"
    );
    // Untouched providers still get their defaults.
    assert_eq!(settings.cookie_source(ProviderId::Claude), "manual");
    assert_eq!(settings.api_region(ProviderId::Zai), "global");
}

/// Default `Settings` should serialize WITHOUT a `provider_configs`
/// field (empty map skipped).
#[test]
fn test_default_settings_skip_empty_provider_configs() {
    let settings = Settings::default();
    let json = serde_json::to_string(&settings).unwrap();
    assert!(
        !json.contains("\"provider_configs\""),
        "empty map should be skipped, json: {json}"
    );
}

/// Per-provider defaults are applied even when the entry is absent.
#[test]
fn test_per_provider_defaults_applied() {
    let settings = Settings::default();
    assert_eq!(settings.cookie_source(ProviderId::Codex), "manual");
    assert_eq!(settings.usage_source(ProviderId::Codex), "auto");
    assert_eq!(settings.api_region(ProviderId::Alibaba), "singapore");
    assert_eq!(settings.api_region(ProviderId::Zai), "global");
    assert_eq!(settings.api_region(ProviderId::MiniMax), "global");
    assert!(settings.openai_web_extras(ProviderId::Codex));
    assert!(!settings.historical_tracking(ProviderId::Codex));
    assert!(!settings.avoid_keychain_prompts(ProviderId::Claude));
}

#[test]
fn codex_spark_usage_visibility_defaults_to_visible_and_roundtrips() {
    let mut settings = Settings::default();
    assert!(settings.codex_spark_usage_visible());

    settings.set_codex_spark_usage_visible(false);
    let serialized = serde_json::to_string(&settings).unwrap();
    let loaded: Settings = serde_json::from_str(&serialized).unwrap();

    assert!(!loaded.codex_spark_usage_visible());
}
