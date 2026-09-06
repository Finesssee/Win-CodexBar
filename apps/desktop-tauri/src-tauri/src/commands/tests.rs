use std::collections::HashMap;

use super::{
    NamedRateWindowSnapshot, ProviderSummary, ProviderUsageSnapshot, provider_cookie_source_lookup,
    provider_region_lookup, validate_external_url, validate_surface_target,
};
use crate::state::AppState;
use crate::surface::SurfaceMode;
use crate::surface_target::SurfaceTarget;
use codexbar::core::{
    FetchContext, ProviderAccountData, ProviderFetchResult, ProviderId, SourceMode, TokenAccount,
    instantiate_provider,
};
use codexbar::host::session::launch_block_reason;
use codexbar::settings::{ApiKeys, Language, ManualCookies, Settings};

#[test]
fn validate_surface_target_accepts_matching_target() {
    let target = validate_surface_target(
        SurfaceMode::Settings,
        SurfaceTarget::Settings {
            tab: "apiKeys".into(),
        },
    )
    .unwrap();

    assert_eq!(
        target,
        SurfaceTarget::Settings {
            tab: "apiKeys".into()
        }
    );
}

#[test]
fn validate_surface_target_rejects_mismatched_target() {
    let error = validate_surface_target(
        SurfaceMode::TrayPanel,
        SurfaceTarget::Settings {
            tab: "apiKeys".into(),
        },
    )
    .unwrap_err();

    assert!(error.contains("not valid for mode 'trayPanel'"));
}

#[test]
fn validate_surface_target_rejects_hidden_mode() {
    let error = validate_surface_target(SurfaceMode::Hidden, SurfaceTarget::Summary).unwrap_err();

    assert!(error.contains("only supports visible surfaces"));
}

#[test]
fn external_url_validation_allows_only_http_urls() {
    assert_eq!(
        validate_external_url(" https://github.com/Finesssee/Win-CodexBar ").unwrap(),
        "https://github.com/Finesssee/Win-CodexBar"
    );
    assert_eq!(
        validate_external_url("http://codexbar.app").unwrap(),
        "http://codexbar.app"
    );

    for invalid in [
        "",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "https://bad\nhost",
    ] {
        assert!(
            validate_external_url(invalid).is_err(),
            "accepted invalid URL: {invalid:?}"
        );
    }
}

#[test]
fn credential_status_labels_do_not_include_error_details() {
    assert_eq!(
        super::credential_file_status_label(codexbar::secure_file::SecureFileStatus::Missing),
        "missing"
    );
    assert_eq!(
        super::credential_file_status_label(codexbar::secure_file::SecureFileStatus::Plaintext),
        "plaintext"
    );
    assert_eq!(
        super::credential_file_status_label(codexbar::secure_file::SecureFileStatus::Protected(
            "windows-dpapi-user".to_string(),
        )),
        "protected:windows-dpapi-user"
    );
    assert_eq!(
        super::credential_file_status_label(codexbar::secure_file::SecureFileStatus::Unreadable(
            "secret path / token".to_string(),
        )),
        "unreadable"
    );
}

#[test]
fn command_inputs_reject_invalid_provider_ids_before_storage_writes() {
    assert!(super::set_api_key("not-a-provider".into(), "sk-test".into(), None).is_err());
    assert!(super::set_manual_cookie("not-a-provider".into(), "a=b".into()).is_err());
    assert!(super::remove_api_key("bad\nprovider".into()).is_err());
    assert!(super::remove_manual_cookie("".into()).is_err());
}

#[test]
fn command_inputs_reject_multiline_secrets() {
    assert!(super::set_api_key("openrouter".into(), "sk-test\nnext".into(), None).is_err());
    assert!(super::set_manual_cookie("codex".into(), "a=b\nc=d".into()).is_err());
}

#[test]
fn command_inputs_reject_unknown_cookie_source_and_region_values() {
    assert!(super::set_provider_cookie_source("codex".into(), "browser".into()).is_err());
    assert!(super::set_provider_region("zai".into(), "moon".into()).is_err());
}

#[test]
fn apply_provider_order_dedupes_and_appends_unknown_canonical() {
    // Request only "codex" and "claude" — remaining canonical ids should
    // be appended after, preserving canonical order.
    let order =
        codexbar::settings::normalize_provider_order(&["codex".to_string(), "claude".to_string()]);
    assert_eq!(order[0], "codex");
    assert_eq!(order[1], "claude");
    // Every canonical id appears exactly once.
    let mut sorted = order.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), order.len());
    // Every canonical id is present.
    let canonical = codexbar::core::ProviderId::all()
        .iter()
        .map(|p| p.cli_name().to_string())
        .collect::<Vec<_>>();
    for id in &canonical {
        assert!(order.contains(id), "missing canonical id: {id}");
    }
}

#[test]
fn apply_provider_order_ignores_unknown_ids() {
    let order = codexbar::settings::normalize_provider_order(&[
        "not-a-provider".to_string(),
        "codex".to_string(),
    ]);
    assert_eq!(order[0], "codex");
    assert!(!order.iter().any(|id| id == "not-a-provider"));
}

#[test]
fn provider_summaries_reflect_settings_order() {
    // Deprecated providers (KimiK2, CrossModel) are soft-removed from the
    // Settings catalog unless already enabled, so the default Settings
    // surface omits them (upstream #2254).
    let canonical_len = codexbar::core::ProviderId::all()
        .iter()
        .filter(|p| !p.is_deprecated())
        .count();
    let s = Settings::default();
    let summaries: Vec<ProviderSummary> = super::build_provider_summaries(&s);
    assert_eq!(summaries.len(), canonical_len);
    // Index is assigned in emission order.
    for (i, s) in summaries.iter().enumerate() {
        assert_eq!(s.order, i as u32);
    }
}

#[test]
fn provider_catalog_preserves_partial_config_order() {
    let settings = Settings {
        provider_order: codexbar::settings::normalize_provider_order(&[
            "gemini".to_string(),
            "claude".to_string(),
            "codex".to_string(),
        ]),
        ..Settings::default()
    };

    let catalog = super::provider_catalog_for(&settings);

    assert_eq!(
        catalog
            .iter()
            .take(3)
            .map(|provider| provider.id.as_str())
            .collect::<Vec<_>>(),
        vec!["gemini", "claude", "codex"]
    );
}

#[test]
fn settings_snapshot_preserves_partial_config_order_for_enabled_providers() {
    let settings = Settings {
        enabled_providers: ["gemini", "claude", "codex"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        provider_order: codexbar::settings::normalize_provider_order(&[
            "gemini".to_string(),
            "claude".to_string(),
            "codex".to_string(),
        ]),
        ..Settings::default()
    };

    let snapshot = serde_json::to_value(super::SettingsSnapshot::from(settings)).unwrap();

    assert_eq!(
        snapshot["providerOrder"]
            .as_array()
            .unwrap()
            .iter()
            .take(3)
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["gemini", "claude", "codex"],
    );
    assert_eq!(
        snapshot["enabledProviders"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["gemini", "claude", "codex"],
    );
}

#[test]
fn provider_cookie_source_lookup_roundtrips_known_providers() {
    let mut s = Settings::default();
    super::provider_cookie_source_set(&mut s, "codex", "cli-config".to_string()).unwrap();
    assert_eq!(
        provider_cookie_source_lookup(&s, "codex").as_deref(),
        Some("cli-config")
    );
    assert!(provider_cookie_source_lookup(&s, "unknown-provider").is_none());
}

#[test]
fn provider_region_lookup_roundtrips_known_providers() {
    let mut s = Settings::default();
    super::provider_region_set(&mut s, "alibaba", "china".to_string()).unwrap();
    assert_eq!(
        provider_region_lookup(&s, "alibaba").as_deref(),
        Some("china")
    );
    // Non-regional providers return None.
    assert!(provider_region_lookup(&s, "claude").is_none());
}

#[test]
fn minimax_region_lookup_normalizes_legacy_china_value() {
    let mut s = Settings::default();
    super::provider_region_set(&mut s, "minimax", "china".to_string()).unwrap();
    assert_eq!(provider_region_lookup(&s, "minimax").as_deref(), Some("cn"));
}

#[test]
fn minimax_cookie_domain_follows_selected_region() {
    let mut s = Settings::default();
    assert_eq!(
        super::provider_cookie_domain(ProviderId::MiniMax, &s),
        Some("platform.minimax.io")
    );

    s.set_api_region(ProviderId::MiniMax, "cn");
    assert_eq!(
        super::provider_cookie_domain(ProviderId::MiniMax, &s),
        Some("platform.minimaxi.com")
    );
}

#[test]
fn provider_cookie_source_set_rejects_unknown_provider() {
    let mut s = Settings::default();
    let err = super::provider_cookie_source_set(&mut s, "nope", "x".into()).unwrap_err();
    assert!(err.contains("nope"));
}

#[test]
fn fetch_context_defaults_to_manual_cookies_without_browser_import() {
    let settings = Settings::default();
    let cookies = ManualCookies::default();
    let api_keys = ApiKeys::default();
    let token_accounts = HashMap::new();

    let ctx = super::build_fetch_context(
        ProviderId::Cursor,
        &settings,
        &cookies,
        &api_keys,
        &token_accounts,
    );

    // Cursor does not support Cli; empty manual cookie remaps to Web (browser attempt).
    assert_eq!(ctx.source_mode, SourceMode::Web);
}

#[test]
fn fetch_context_cursor_cookie_off_stays_cli() {
    let mut settings = Settings::default();
    settings.set_cookie_source(ProviderId::Cursor, "off");
    let cookies = ManualCookies::default();
    let api_keys = ApiKeys::default();
    let token_accounts = HashMap::new();

    let ctx = super::build_fetch_context(
        ProviderId::Cursor,
        &settings,
        &cookies,
        &api_keys,
        &token_accounts,
    );

    // Explicit cookie-off keeps Cli (no browser scrape).
    assert_eq!(ctx.source_mode, SourceMode::Cli);
    assert!(ctx.manual_cookie_header.is_none());
}

#[test]
fn fetch_context_grok_cookie_off_preserves_auto_oauth() {
    let mut settings = Settings::default();
    settings.set_cookie_source(ProviderId::Grok, "off");
    settings.set_usage_source(ProviderId::Grok, "auto");
    let ctx = super::build_fetch_context(
        ProviderId::Grok,
        &settings,
        &ManualCookies::default(),
        &ApiKeys::default(),
        &HashMap::new(),
    );

    assert_eq!(ctx.source_mode, SourceMode::OAuth);
    assert!(ctx.manual_cookie_header.is_none());
}

#[test]
fn fetch_context_grok_cookie_off_preserves_explicit_oauth() {
    let mut settings = Settings::default();
    settings.set_cookie_source(ProviderId::Grok, "off");
    settings.set_usage_source(ProviderId::Grok, "oauth");
    let ctx = super::build_fetch_context(
        ProviderId::Grok,
        &settings,
        &ManualCookies::default(),
        &ApiKeys::default(),
        &HashMap::new(),
    );

    assert_eq!(ctx.source_mode, SourceMode::OAuth);
    assert!(ctx.manual_cookie_header.is_none());
}

#[test]
fn fetch_context_grok_cookie_off_keeps_explicit_cli() {
    let mut settings = Settings::default();
    settings.set_cookie_source(ProviderId::Grok, "off");
    settings.set_usage_source(ProviderId::Grok, "cli");
    let ctx = super::build_fetch_context(
        ProviderId::Grok,
        &settings,
        &ManualCookies::default(),
        &ApiKeys::default(),
        &HashMap::new(),
    );

    assert_eq!(ctx.source_mode, SourceMode::Cli);
    assert!(ctx.manual_cookie_header.is_none());
}

#[test]
fn fetch_context_grok_empty_manual_preserves_oauth_without_browser_import() {
    let mut settings = Settings::default();
    settings.set_cookie_source(ProviderId::Grok, "manual");
    settings.set_usage_source(ProviderId::Grok, "auto");
    let ctx = super::build_fetch_context(
        ProviderId::Grok,
        &settings,
        &ManualCookies::default(),
        &ApiKeys::default(),
        &HashMap::new(),
    );

    assert_eq!(ctx.source_mode, SourceMode::OAuth);
    assert!(ctx.manual_cookie_header.is_none());
}

#[test]
fn fetch_context_opencode_empty_manual_remaps_to_web() {
    let settings = Settings::default();
    let cookies = ManualCookies::default();
    let api_keys = ApiKeys::default();
    let token_accounts = HashMap::new();

    let ctx = super::build_fetch_context(
        ProviderId::OpenCode,
        &settings,
        &cookies,
        &api_keys,
        &token_accounts,
    );

    assert_eq!(ctx.source_mode, SourceMode::Web);
}

#[test]
fn fetch_context_claude_uses_oauth_without_manual_cookie() {
    let settings = Settings::default();
    let cookies = ManualCookies::default();
    let api_keys = ApiKeys::default();
    let token_accounts = HashMap::new();

    let ctx = super::build_fetch_context(
        ProviderId::Claude,
        &settings,
        &cookies,
        &api_keys,
        &token_accounts,
    );

    assert_eq!(ctx.source_mode, SourceMode::OAuth);
    assert!(ctx.manual_cookie_header.is_none());
}

#[test]
fn fetch_context_claude_explicit_cli_source_still_uses_cli() {
    let mut settings = Settings::default();
    settings.set_usage_source(ProviderId::Claude, "cli");
    let cookies = ManualCookies::default();
    let api_keys = ApiKeys::default();
    let token_accounts = HashMap::new();

    let ctx = super::build_fetch_context(
        ProviderId::Claude,
        &settings,
        &cookies,
        &api_keys,
        &token_accounts,
    );

    assert_eq!(ctx.source_mode, SourceMode::Cli);
    assert!(ctx.manual_cookie_header.is_none());
}

#[test]
fn fetch_context_manual_cookie_uses_web_without_browser_import() {
    let settings = Settings::default();
    let mut cookies = ManualCookies::default();
    cookies.set("cursor", "session=abc123");
    let api_keys = ApiKeys::default();
    let token_accounts = HashMap::new();

    let ctx = super::build_fetch_context(
        ProviderId::Cursor,
        &settings,
        &cookies,
        &api_keys,
        &token_accounts,
    );

    assert_eq!(ctx.source_mode, SourceMode::Web);
    assert_eq!(ctx.manual_cookie_header.as_deref(), Some("session=abc123"));
}

#[test]
fn fetch_context_api_key_provider_uses_auto_without_cookie_import() {
    let settings = Settings::default();
    let cookies = ManualCookies::default();
    let mut api_keys = ApiKeys::default();
    api_keys.set("deepseek", "sk-test", None);
    let token_accounts = HashMap::new();

    let ctx = super::build_fetch_context(
        ProviderId::DeepSeek,
        &settings,
        &cookies,
        &api_keys,
        &token_accounts,
    );

    assert_eq!(ctx.source_mode, SourceMode::Auto);
    assert!(ctx.manual_cookie_header.is_none());
    assert_eq!(ctx.api_key.as_deref(), Some("sk-test"));
}

#[test]
fn fetch_context_kimi_api_key_preserves_auto_for_web_fallback() {
    let settings = Settings::default();
    let cookies = ManualCookies::default();
    let mut api_keys = ApiKeys::default();
    api_keys.set("kimi", "sk-kimi-test", None);
    let token_accounts = HashMap::new();

    let ctx = super::build_fetch_context(
        ProviderId::Kimi,
        &settings,
        &cookies,
        &api_keys,
        &token_accounts,
    );

    assert_eq!(ctx.source_mode, SourceMode::Auto);
    assert!(ctx.manual_cookie_header.is_none());
    assert_eq!(ctx.api_key.as_deref(), Some("sk-kimi-test"));
}

#[test]
fn fetch_context_opencodego_api_key_preserves_auto_for_api_overlay() {
    let settings = Settings::default();
    let cookies = ManualCookies::default();
    let mut api_keys = ApiKeys::default();
    api_keys.set("opencodego", "go-test", None);
    let token_accounts = HashMap::new();

    let ctx = super::build_fetch_context(
        ProviderId::OpenCodeGo,
        &settings,
        &cookies,
        &api_keys,
        &token_accounts,
    );

    assert_eq!(ctx.source_mode, SourceMode::Auto);
    assert!(ctx.manual_cookie_header.is_none());
    assert_eq!(ctx.api_key.as_deref(), Some("go-test"));
}

#[test]
fn fetch_context_includes_minimax_region() {
    let mut settings = Settings::default();
    settings.set_api_region(ProviderId::MiniMax, "cn");
    let cookies = ManualCookies::default();
    let api_keys = ApiKeys::default();
    let token_accounts = HashMap::new();

    let ctx = super::build_fetch_context(
        ProviderId::MiniMax,
        &settings,
        &cookies,
        &api_keys,
        &token_accounts,
    );

    assert_eq!(ctx.api_region.as_deref(), Some("cn"));
}

#[test]
fn fetch_context_token_account_uses_web_cookie_header() {
    let settings = Settings::default();
    let cookies = ManualCookies::default();
    let api_keys = ApiKeys::default();
    let mut token_accounts = HashMap::new();
    let mut data = ProviderAccountData::new();
    data.add_account(TokenAccount::new("Work", "abc123"));
    token_accounts.insert(ProviderId::Ollama, data);

    let ctx = super::build_fetch_context(
        ProviderId::Ollama,
        &settings,
        &cookies,
        &api_keys,
        &token_accounts,
    );

    assert_eq!(ctx.source_mode, SourceMode::Web);
    assert_eq!(
        ctx.manual_cookie_header.as_deref(),
        Some("__Secure-session=abc123")
    );
}

#[test]
fn fetch_context_claude_oauth_token_account_uses_oauth() {
    let settings = Settings::default();
    let cookies = ManualCookies::default();
    let api_keys = ApiKeys::default();
    let mut token_accounts = HashMap::new();
    let mut data = ProviderAccountData::new();
    data.add_account(TokenAccount::new("Claude OAuth", "sk-ant-oat01-abc123"));
    token_accounts.insert(ProviderId::Claude, data);

    let ctx = super::build_fetch_context(
        ProviderId::Claude,
        &settings,
        &cookies,
        &api_keys,
        &token_accounts,
    );

    assert_eq!(ctx.source_mode, SourceMode::OAuth);
    assert!(ctx.manual_cookie_header.is_none());
    assert_eq!(ctx.api_key.as_deref(), Some("sk-ant-oat01-abc123"));
}

#[test]
fn fetch_context_copilot_token_account_uses_oauth_api_key() {
    let settings = Settings::default();
    let cookies = ManualCookies::default();
    let api_keys = ApiKeys::default();
    let mut token_accounts = HashMap::new();
    let mut data = ProviderAccountData::new();
    data.add_account(TokenAccount::new("GitHub", "gho_testtoken"));
    token_accounts.insert(ProviderId::Copilot, data);

    let ctx = super::build_fetch_context(
        ProviderId::Copilot,
        &settings,
        &cookies,
        &api_keys,
        &token_accounts,
    );

    assert_eq!(ctx.source_mode, SourceMode::OAuth);
    assert!(ctx.manual_cookie_header.is_none());
    assert_eq!(ctx.api_key.as_deref(), Some("gho_testtoken"));
}

#[test]
fn fetch_context_claude_session_token_account_uses_web_cookie() {
    let settings = Settings::default();
    let cookies = ManualCookies::default();
    let api_keys = ApiKeys::default();
    let mut token_accounts = HashMap::new();
    let mut data = ProviderAccountData::new();
    data.add_account(TokenAccount::new("Claude Web", "sessionKey=abc123"));
    token_accounts.insert(ProviderId::Claude, data);

    let ctx = super::build_fetch_context(
        ProviderId::Claude,
        &settings,
        &cookies,
        &api_keys,
        &token_accounts,
    );

    assert_eq!(ctx.source_mode, SourceMode::Web);
    assert_eq!(
        ctx.manual_cookie_header.as_deref(),
        Some("sessionKey=abc123")
    );
    assert!(ctx.api_key.is_none());
}

#[test]
fn fetch_context_token_account_takes_precedence_over_manual_cookie() {
    let settings = Settings::default();
    let mut cookies = ManualCookies::default();
    cookies.set("cursor", "manual=old");
    let api_keys = ApiKeys::default();
    let mut token_accounts = HashMap::new();
    let mut data = ProviderAccountData::new();
    data.add_account(TokenAccount::new("Work", "WorkosCursorSessionToken=new"));
    token_accounts.insert(ProviderId::Cursor, data);

    let ctx = super::build_fetch_context(
        ProviderId::Cursor,
        &settings,
        &cookies,
        &api_keys,
        &token_accounts,
    );

    assert_eq!(ctx.source_mode, SourceMode::Web);
    assert_eq!(
        ctx.manual_cookie_header.as_deref(),
        Some("WorkosCursorSessionToken=new")
    );
}

#[test]
fn fetch_context_openrouter_token_account_overrides_stored_api_key() {
    let settings = Settings::default();
    let cookies = ManualCookies::default();
    let mut api_keys = ApiKeys::default();
    api_keys.set("openrouter", "sk-or-v1-stored-decoy", None);
    let mut token_accounts = HashMap::new();
    let mut data = ProviderAccountData::new();
    data.add_account(TokenAccount::new("Personal", "sk-or-v1-personal"));
    data.add_account(TokenAccount::new("Work", "sk-or-v1-work"));
    data.set_active(1);
    token_accounts.insert(ProviderId::OpenRouter, data);

    let ctx = super::build_fetch_context(
        ProviderId::OpenRouter,
        &settings,
        &cookies,
        &api_keys,
        &token_accounts,
    );

    assert_eq!(ctx.source_mode, SourceMode::OAuth);
    assert!(ctx.manual_cookie_header.is_none());
    assert_eq!(ctx.api_key.as_deref(), Some("sk-or-v1-work"));
}

#[test]
fn fetch_context_openrouter_falls_back_to_stored_api_key_without_token_accounts() {
    let settings = Settings::default();
    let cookies = ManualCookies::default();
    let mut api_keys = ApiKeys::default();
    api_keys.set("openrouter", "sk-or-v1-stored", None);
    let token_accounts = HashMap::new();

    let ctx = super::build_fetch_context(
        ProviderId::OpenRouter,
        &settings,
        &cookies,
        &api_keys,
        &token_accounts,
    );

    assert_eq!(ctx.api_key.as_deref(), Some("sk-or-v1-stored"));
}

#[test]
fn provider_region_set_rejects_non_regional_provider() {
    let mut s = Settings::default();
    let err = super::provider_region_set(&mut s, "claude", "global".into()).unwrap_err();
    assert!(err.contains("claude"));
}

#[test]
fn launch_block_reason_helper_returns_none_when_not_blocked() {
    assert!(launch_block_reason(false, false).is_none());
}

#[test]
fn launch_block_reason_helper_prefers_ssh() {
    let msg = launch_block_reason(true, true).unwrap();
    assert!(msg.contains("SSH"));
}

// ── Phase 6b — provider detail pane ────────────────────────────

#[test]
fn build_provider_detail_populates_identity_urls() {
    let detail = super::build_provider_detail("claude").expect("known provider");
    assert_eq!(detail.id, "claude");
    assert_eq!(detail.display_name, "Claude");
    // Claude advertises a status page URL in its metadata.
    assert!(detail.status_page_url.is_some());
    // No snapshot yet — empty usage bars and no error.
    assert!(detail.session.is_none());
    assert!(detail.last_error.is_none());
    assert!(!detail.has_snapshot);
}

#[test]
fn build_provider_detail_rejects_unknown_provider() {
    let err = super::build_provider_detail("not-a-provider").unwrap_err();
    assert!(err.contains("not-a-provider"));
}

#[test]
fn provider_detail_roundtrips_through_serde() {
    let detail = super::build_provider_detail("codex").expect("known provider");
    let json = serde_json::to_string(&detail).expect("serialize");
    // camelCase rename survives the round-trip.
    assert!(json.contains("\"displayName\""));
    assert!(json.contains("\"hasSnapshot\""));
    assert!(json.contains("\"statusPageUrl\""));
}

#[test]
fn pace_stage_serializes_to_snake_case_string() {
    use codexbar::core::PaceStage;
    assert_eq!(super::pace_stage_str(PaceStage::OnTrack), "on_track");
    assert_eq!(
        super::pace_stage_str(PaceStage::SlightlyAhead),
        "slightly_ahead"
    );
    assert_eq!(super::pace_stage_str(PaceStage::FarAhead), "far_ahead");
    assert_eq!(
        super::pace_stage_str(PaceStage::SlightlyBehind),
        "slightly_behind"
    );
    assert_eq!(super::pace_stage_str(PaceStage::Behind), "behind");
    assert_eq!(super::pace_stage_str(PaceStage::FarBehind), "far_behind");
}

#[test]
fn provider_cache_is_fresh_inside_stale_window() {
    assert!(super::is_provider_cache_fresh(
        Some(std::time::Instant::now()),
        std::time::Duration::from_secs(30),
    ));
}

#[test]
fn provider_cache_is_stale_when_missing_timestamp() {
    assert!(!super::is_provider_cache_fresh(
        None,
        std::time::Duration::from_secs(30),
    ));
}

#[test]
fn provider_cache_is_stale_after_window() {
    assert!(!super::is_provider_cache_fresh(
        Some(std::time::Instant::now() - std::time::Duration::from_secs(31)),
        std::time::Duration::from_secs(30),
    ));
}

#[test]
fn provider_fetch_timeout_allows_slower_authenticated_providers() {
    let ctx = FetchContext {
        web_timeout: 30,
        ..FetchContext::default()
    };
    assert_eq!(
        super::provider_fetch_timeout(ProviderId::Claude, &ctx),
        std::time::Duration::from_secs(75)
    );
    assert_eq!(
        super::provider_fetch_timeout(ProviderId::Codex, &ctx),
        std::time::Duration::from_secs(75)
    );
    assert_eq!(
        super::provider_fetch_timeout(ProviderId::Copilot, &ctx),
        std::time::Duration::from_secs(75)
    );
    assert_eq!(
        super::provider_fetch_timeout(ProviderId::DeepSeek, &ctx),
        std::time::Duration::from_secs(35)
    );
}

#[test]
fn provider_fetch_timeout_respects_context_web_timeout_with_cap() {
    let ctx = FetchContext {
        web_timeout: 60,
        ..FetchContext::default()
    };
    assert_eq!(
        super::provider_fetch_timeout(ProviderId::T3Chat, &ctx),
        std::time::Duration::from_secs(65)
    );

    let ctx = FetchContext {
        web_timeout: 120,
        ..FetchContext::default()
    };
    assert_eq!(
        super::provider_fetch_timeout(ProviderId::AzureOpenAI, &ctx),
        std::time::Duration::from_secs(65)
    );
}

#[test]
fn provider_cache_upsert_replaces_existing_provider() {
    let metadata = instantiate_provider(ProviderId::Codex).metadata().clone();
    let result = ProviderFetchResult {
        usage: codexbar::core::UsageSnapshot::new(codexbar::core::RateWindow::new(10.0)),
        cost: None,
        wayfinder_usage: None,
        source_label: "CLI".to_string(),
    };
    let mut first =
        ProviderUsageSnapshot::from_fetch_result(ProviderId::Codex, &metadata, &result, None);
    let mut second = first.clone();
    first.error = Some("old".to_string());
    second.error = Some("new".to_string());

    let mut cache = vec![first];
    super::upsert_provider_cache(&mut cache, second);

    assert_eq!(cache.len(), 1);
    assert_eq!(cache[0].provider_id, "codex");
    assert_eq!(cache[0].error.as_deref(), Some("new"));
}

#[test]
fn provider_cache_prunes_disabled_providers() {
    let metadata = instantiate_provider(ProviderId::Codex).metadata().clone();
    let result = ProviderFetchResult {
        usage: codexbar::core::UsageSnapshot::new(codexbar::core::RateWindow::new(10.0)),
        cost: None,
        wayfinder_usage: None,
        source_label: "CLI".to_string(),
    };
    let codex =
        ProviderUsageSnapshot::from_fetch_result(ProviderId::Codex, &metadata, &result, None);
    let claude_meta = instantiate_provider(ProviderId::Claude).metadata().clone();
    let claude =
        ProviderUsageSnapshot::from_fetch_result(ProviderId::Claude, &claude_meta, &result, None);

    let mut cache = vec![codex, claude];
    super::prune_provider_cache_to_enabled(&mut cache, &[ProviderId::Codex]);

    assert_eq!(cache.len(), 1);
    assert_eq!(cache[0].provider_id, "codex");
}

#[test]
fn superseded_refresh_generation_is_not_current() {
    let mut state = AppState::new();
    state.provider_refresh_generation = 3;
    assert!(super::is_current_provider_refresh_generation(&state, 3));
    assert!(!super::is_current_provider_refresh_generation(&state, 2));
}

#[test]
fn hiding_codex_spark_rows_preserves_other_extra_usage() {
    let metadata = instantiate_provider(ProviderId::Codex).metadata().clone();
    let result = ProviderFetchResult {
        usage: codexbar::core::UsageSnapshot::new(codexbar::core::RateWindow::new(10.0)),
        cost: None,
        wayfinder_usage: None,
        source_label: "CLI".to_string(),
    };
    let mut snapshot =
        ProviderUsageSnapshot::from_fetch_result(ProviderId::Codex, &metadata, &result, None);
    snapshot.extra_rate_windows = vec![
        NamedRateWindowSnapshot {
            id: "codex-spark".to_string(),
            title: "Codex Spark 5-hour".to_string(),
            window: snapshot.primary.clone(),
        },
        NamedRateWindowSnapshot {
            id: "credits".to_string(),
            title: "Credits".to_string(),
            window: snapshot.primary.clone(),
        },
    ];

    super::filter_hidden_codex_spark_rows(&mut snapshot, false);

    assert_eq!(snapshot.extra_rate_windows.len(), 1);
    assert_eq!(snapshot.extra_rate_windows[0].id, "credits");
}

#[test]
fn claude_transient_auth_failure_preserves_first_last_good_snapshot() {
    let metadata = instantiate_provider(ProviderId::Claude).metadata().clone();
    let result = ProviderFetchResult {
        usage: codexbar::core::UsageSnapshot::new(codexbar::core::RateWindow::new(42.0)),
        cost: None,
        wayfinder_usage: None,
        source_label: "OAuth".to_string(),
    };
    let good =
        ProviderUsageSnapshot::from_fetch_result(ProviderId::Claude, &metadata, &result, None);
    let error = ProviderUsageSnapshot::from_error(
        ProviderId::Claude,
        &metadata,
        "Unauthorized".to_string(),
        codexbar::core::ProviderStateKind::NeedsAuthentication,
    );
    let mut state = crate::state::AppState::new();
    state.provider_cache.push(good.clone());

    let preserved = super::providers::preserve_last_good_transient_failure(
        &mut state,
        ProviderId::Claude,
        error,
    );

    assert_eq!(preserved.error, None);
    assert_eq!(preserved.primary.used_percent, 42.0);
}

#[test]
fn claude_repeated_auth_failure_surfaces_error() {
    let metadata = instantiate_provider(ProviderId::Claude).metadata().clone();
    let result = ProviderFetchResult {
        usage: codexbar::core::UsageSnapshot::new(codexbar::core::RateWindow::new(42.0)),
        cost: None,
        wayfinder_usage: None,
        source_label: "OAuth".to_string(),
    };
    let good =
        ProviderUsageSnapshot::from_fetch_result(ProviderId::Claude, &metadata, &result, None);
    let first_error = ProviderUsageSnapshot::from_error(
        ProviderId::Claude,
        &metadata,
        "Unauthorized".to_string(),
        codexbar::core::ProviderStateKind::NeedsAuthentication,
    );
    let second_error = first_error.clone();
    let mut state = crate::state::AppState::new();
    state.provider_cache.push(good);

    let _ = super::providers::preserve_last_good_transient_failure(
        &mut state,
        ProviderId::Claude,
        first_error,
    );
    let surfaced = super::providers::preserve_last_good_transient_failure(
        &mut state,
        ProviderId::Claude,
        second_error,
    );

    assert!(surfaced.error.is_some());
}

#[test]
fn claude_cli_parse_failure_keeps_last_good_every_time() {
    let metadata = instantiate_provider(ProviderId::Claude).metadata().clone();
    let result = ProviderFetchResult {
        usage: codexbar::core::UsageSnapshot::new(codexbar::core::RateWindow::new(17.0)),
        cost: None,
        wayfinder_usage: None,
        source_label: "CLI".to_string(),
    };
    let good =
        ProviderUsageSnapshot::from_fetch_result(ProviderId::Claude, &metadata, &result, None);
    let err = ProviderUsageSnapshot::from_error(
        ProviderId::Claude,
        &metadata,
        "Parse error: Empty output from Claude CLI".to_string(),
        codexbar::core::ProviderStateKind::Unknown,
    );
    let mut state = crate::state::AppState::new();
    state.provider_cache.push(good.clone());

    let first = super::providers::preserve_last_good_transient_failure(
        &mut state,
        ProviderId::Claude,
        err.clone(),
    );
    let second =
        super::providers::preserve_last_good_transient_failure(&mut state, ProviderId::Claude, err);

    assert_eq!(first.error, None);
    assert_eq!(first.primary.used_percent, 17.0);
    // Parse failures keep last-good on every refresh (upstream #2247), unlike one-shot auth.
    assert_eq!(second.error, None);
    assert_eq!(second.primary.used_percent, 17.0);
}

#[test]
fn claude_hard_credentials_missing_does_not_preserve_stale() {
    let metadata = instantiate_provider(ProviderId::Claude).metadata().clone();
    let result = ProviderFetchResult {
        usage: codexbar::core::UsageSnapshot::new(codexbar::core::RateWindow::new(17.0)),
        cost: None,
        wayfinder_usage: None,
        source_label: "OAuth".to_string(),
    };
    let good =
        ProviderUsageSnapshot::from_fetch_result(ProviderId::Claude, &metadata, &result, None);
    let err = ProviderUsageSnapshot::from_error(
        ProviderId::Claude,
        &metadata,
        "OAuth error: Claude OAuth credentials not found. Run `claude` to authenticate."
            .to_string(),
        codexbar::core::ProviderStateKind::NeedsAuthentication,
    );
    let mut state = crate::state::AppState::new();
    state.provider_cache.push(good);

    let out =
        super::providers::preserve_last_good_transient_failure(&mut state, ProviderId::Claude, err);
    assert!(out.error.is_some());
    assert_eq!(
        out.error_state,
        codexbar::core::ProviderStateKind::NeedsAuthentication,
        "hard auth failure must carry its classification on the snapshot"
    );
}

#[test]
fn claude_error_message_removes_upstream_swift_cancellation() {
    let message = super::friendly_provider_error(
        ProviderId::Claude,
        "The operation couldn't be completed. (Swift.CancellationError error 1.)",
    );

    assert!(!message.contains("Swift"));
    assert!(message.contains("Claude usage fetch was cancelled"));
    assert!(message.contains("Refresh Claude"));
}

#[test]
fn claude_error_message_explains_missing_sign_in() {
    let message = super::friendly_provider_error(
        ProviderId::Claude,
        "OAuth error: Claude OAuth credentials not found. Run `claude` to authenticate.",
    );

    assert_eq!(
        message,
        "Claude sign-in was not found. Run `claude` once to authenticate, then refresh Claude in Win-CodexBar."
    );
}

#[test]
fn non_claude_error_message_is_preserved() {
    let message = super::friendly_provider_error(
        ProviderId::Codex,
        "OAuth error: Claude OAuth credentials not found. Run `claude` to authenticate.",
    );

    assert_eq!(
        message,
        "OAuth error: Claude OAuth credentials not found. Run `claude` to authenticate."
    );
}

#[test]
fn chart_data_serde_roundtrip_preserves_fields() {
    use super::{
        DailyCostPoint, DailyTokenPoint, DailyUsageBreakdown, ProviderChartData, ServiceUsagePoint,
    };

    let original = ProviderChartData {
        provider_id: "codex".into(),
        cost_history: vec![
            DailyCostPoint {
                date: "2025-01-01".into(),
                value: Some(1.25),
            },
            DailyCostPoint {
                date: "2025-01-02".into(),
                value: Some(0.0),
            },
        ],
        credits_history: vec![DailyCostPoint {
            date: "2025-01-01".into(),
            value: Some(42.0),
        }],
        usage_breakdown: vec![DailyUsageBreakdown {
            day: "2025-01-01".into(),
            services: vec![
                ServiceUsagePoint {
                    service: "gpt-4o".into(),
                    credits_used: 10.0,
                },
                ServiceUsagePoint {
                    service: "gpt-4o-mini".into(),
                    credits_used: 3.5,
                },
            ],
            total_credits_used: 13.5,
        }],
        local_usage: None,
        tokens_history: vec![DailyTokenPoint {
            date: "2025-01-01".into(),
            tokens: 123_456,
        }],
        tokens_incomplete: true,
    };

    let json = serde_json::to_string(&original).expect("serialize");
    assert!(
        json.contains("\"providerId\":\"codex\""),
        "camelCase providerId: {json}"
    );
    assert!(json.contains("\"costHistory\""));
    assert!(json.contains("\"creditsHistory\""));
    assert!(json.contains("\"usageBreakdown\""));
    assert!(json.contains("\"localUsage\":null"));
    assert!(json.contains("\"creditsUsed\":10.0"));
    assert!(json.contains("\"totalCreditsUsed\":13.5"));
    assert!(json.contains("\"tokensHistory\""));
    assert!(json.contains("\"tokens\":123456"));
    assert!(json.contains("\"tokensIncomplete\":true"));

    let back: ProviderChartData = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.provider_id, "codex");
    assert_eq!(back.cost_history.len(), 2);
    assert_eq!(back.cost_history[0].date, "2025-01-01");
    assert_eq!(back.credits_history[0].value, Some(42.0));
    assert_eq!(back.usage_breakdown[0].services.len(), 2);
    assert_eq!(back.usage_breakdown[0].total_credits_used, 13.5);
    assert_eq!(back.tokens_history[0].tokens, 123_456);
    assert!(back.tokens_incomplete);
}

#[test]
fn chart_data_for_unknown_provider_is_empty() {
    let data =
        super::build_provider_chart_data("this-provider-definitely-does-not-exist".into(), None);
    assert_eq!(data.provider_id, "this-provider-definitely-does-not-exist");
    assert!(data.credits_history.is_empty());
    assert!(data.usage_breakdown.is_empty());
}

#[test]
fn japanese_provider_snapshot_localizes_weekly_label() {
    let metadata = instantiate_provider(ProviderId::Claude).metadata().clone();
    let usage = codexbar::core::UsageSnapshot::new(codexbar::core::RateWindow::new(10.0))
        .with_secondary(codexbar::core::RateWindow::new(20.0));
    let result = ProviderFetchResult {
        usage,
        cost: None,
        wayfinder_usage: None,
        source_label: "OAuth".to_string(),
    };

    let snapshot =
        ProviderUsageSnapshot::from_fetch_result(ProviderId::Claude, &metadata, &result, None);

    // Secondary label stays raw; localization happens at render time.
    assert_eq!(snapshot.secondary_label, Some("Weekly".to_string()));
}

#[test]
fn japanese_provider_snapshot_localizes_pace_reserve_description() {
    use chrono::{Duration, Utc};

    let metadata = instantiate_provider(ProviderId::Claude).metadata().clone();
    let now = Utc::now();
    // 7-day window, half elapsed, 40% used → 10% ahead of pace, will last to reset.
    let secondary = codexbar::core::RateWindow::with_details(
        40.0,
        Some(7 * 24 * 60),
        Some(now + Duration::minutes(7 * 24 * 60 / 2)),
        None,
    );
    let usage = codexbar::core::UsageSnapshot::new(codexbar::core::RateWindow::new(10.0))
        .with_secondary(secondary);
    let result = ProviderFetchResult {
        usage,
        cost: None,
        wayfinder_usage: None,
        source_label: "OAuth".to_string(),
    };

    let snapshot =
        ProviderUsageSnapshot::from_fetch_result(ProviderId::Claude, &metadata, &result, None);

    // Reserve data stays raw; localization happens at render time.
    let secondary = snapshot.secondary.as_ref().expect("secondary window");
    assert!(secondary.reserve_percent.is_some());
    assert!(secondary.reserve_will_last_to_reset);
    assert!(secondary.reserve_description.is_none());
}

#[test]
fn chart_data_requires_account_email_for_codex() {
    let data = super::build_provider_chart_data("codex".into(), None);
    assert_eq!(data.provider_id, "codex");
    assert!(data.credits_history.is_empty());
    assert!(data.usage_breakdown.is_empty());
}

#[test]
fn cookie_options_for_cookie_supporting_provider() {
    let opts = super::cookie_source_options_for("codex", Language::English);
    let values: Vec<_> = opts.iter().map(|o| o.value.as_str()).collect();
    assert_eq!(values, vec!["auto", "manual", "off"]);
    assert!(opts.iter().any(|o| o.label == "Automatic"));
    assert!(opts.iter().any(|o| o.label == "Manual"));
    assert!(opts.iter().any(|o| o.label == "Disabled"));
}

#[test]
fn cookie_options_empty_for_providers_without_picker() {
    assert!(super::cookie_source_options_for("anthropic", Language::English).is_empty());
    assert!(super::cookie_source_options_for("unknown", Language::English).is_empty());
}

#[test]
fn region_options_for_regional_provider() {
    let opts = super::region_options_for("alibaba");
    let values: Vec<_> = opts.iter().map(|o| o.value.as_str()).collect();
    assert_eq!(values, vec!["singapore", "us", "germany", "hongkong", "cn"]);
}

#[test]
fn alibaba_token_plan_region_options() {
    let opts = super::region_options_for("alibabatokenplan");
    let values: Vec<_> = opts.iter().map(|o| o.value.as_str()).collect();
    let labels: Vec<_> = opts.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(values, vec!["cn", "intl", "cn-personal", "intl-personal"]);
    assert_eq!(
        labels,
        vec![
            "China Team",
            "International Team",
            "China Personal/Solo",
            "International Personal/Solo"
        ]
    );
}

#[test]
fn minimax_region_options_match_upstream_hosts() {
    let opts = super::region_options_for("minimax");
    let values: Vec<_> = opts.iter().map(|o| o.value.as_str()).collect();
    let labels: Vec<_> = opts.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(values, vec!["global", "cn"]);
    assert_eq!(
        labels,
        vec![
            "Global (platform.minimax.io)",
            "China mainland (platform.minimaxi.com)"
        ]
    );
}

#[test]
fn region_options_empty_for_non_regional_provider() {
    assert!(super::region_options_for("claude").is_empty());
    assert!(super::region_options_for("codex").is_empty());
}

#[test]
fn cookie_source_option_roundtrips_serde() {
    let opt = super::CookieSourceOption {
        value: "auto".to_string(),
        label: "Automatic".to_string(),
        description: Some("Imports browser cookies.".to_string()),
    };
    let json = serde_json::to_string(&opt).unwrap();
    let back: super::CookieSourceOption = serde_json::from_str(&json).unwrap();
    assert_eq!(opt, back);
}

#[test]
fn region_option_roundtrips_serde() {
    let opt = super::RegionOption {
        value: "intl".to_string(),
        label: "International".to_string(),
    };
    let json = serde_json::to_string(&opt).unwrap();
    let back: super::RegionOption = serde_json::from_str(&json).unwrap();
    assert_eq!(opt, back);
}

// ── Phase 6d — credential detection UIs ────────────────────────

#[test]
fn open_path_rejects_empty_path() {
    let err = super::open_path(String::new()).unwrap_err();
    assert!(err.to_lowercase().contains("empty"));
}

#[test]
fn open_path_rejects_relative_path() {
    let err = super::open_path("relative/path".into()).unwrap_err();
    assert!(err.contains("absolute"));
}

#[test]
fn open_path_rejects_missing_path() {
    let missing = std::env::temp_dir()
        .join(format!("codexbar-phase6d-missing-{}", std::process::id()))
        .join("does-not-exist");
    let err = super::open_path(missing.to_string_lossy().into_owned()).unwrap_err();
    assert!(err.contains("not found"));
}

#[test]
fn external_url_validator_accepts_http_and_https() {
    assert_eq!(
        super::validate_external_url(" https://github.com/Finesssee/Win-CodexBar "),
        Ok("https://github.com/Finesssee/Win-CodexBar")
    );
    assert_eq!(
        super::validate_external_url("http://localhost:1420"),
        Ok("http://localhost:1420")
    );
}

#[test]
fn external_url_validator_rejects_non_web_and_control_urls() {
    assert!(super::validate_external_url("file:///C:/Windows/win.ini").is_err());
    assert!(super::validate_external_url("javascript:alert(1)").is_err());
    assert!(super::validate_external_url("https://example.com/\nmalicious").is_err());
}

// ── Phase 13 — E2E IPC harness ─────────────────────────────────
//
// Build the full bootstrap payload and prove that every shared
// `ProviderId` variant ends up in the provider catalog with a
// non-empty id + display name. If a new provider is added to the
// enum but never wired through the desktop catalog, this test will
// fail with `missing provider in bootstrap catalog: <id>`.

#[test]
fn bootstrap_payload_exposes_every_provider_variant() {
    let payload = super::get_bootstrap_state();

    let catalog_ids: std::collections::HashSet<String> = payload
        .providers
        .iter()
        .map(|entry| entry.id.clone())
        .collect();

    for entry in &payload.providers {
        assert!(!entry.id.is_empty(), "provider entry has empty id");
        assert!(
            !entry.display_name.is_empty(),
            "provider {} has empty display_name",
            entry.id
        );
    }

    // Deprecated providers (KimiK2, CrossModel) are soft-removed from the
    // desktop catalog unless already enabled (upstream #2254); they are
    // intentionally absent from the default bootstrap payload.
    let active: Vec<ProviderId> = ProviderId::all()
        .iter()
        .copied()
        .filter(|p| !p.is_deprecated())
        .collect();

    for provider in &active {
        let expected = provider.cli_name().to_string();
        assert!(
            catalog_ids.contains(&expected),
            "missing provider in bootstrap catalog: {expected}"
        );
    }

    assert_eq!(
        catalog_ids.len(),
        active.len(),
        "bootstrap catalog size drifted from the active (non-deprecated) providers"
    );

    // Sanity — payload must also round-trip through JSON cleanly so
    // the TypeScript bridge never sees a partially-populated record.
    let encoded = serde_json::to_string(&payload).expect("serialize bootstrap");
    assert!(encoded.contains("contractVersion"));
    assert!(encoded.contains("\"providers\""));
    assert!(encoded.contains("\"settings\""));
}
