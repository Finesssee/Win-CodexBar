//! Safe provider diagnostics export.
//!
//! This intentionally reports configuration shape and fetch outcomes without
//! printing cookies, tokens, account emails, or provider response bodies.

use anyhow::Context;
use chrono::{DateTime, Utc};
use clap::Args;
use serde::Serialize;

use crate::core::{
    CostSnapshot, FetchContext, ProviderError, ProviderFetchResult, ProviderId, RateWindow,
    SourceMode, instantiate_provider,
};
use crate::settings::{ApiKeys, ManualCookies, Settings};

#[derive(Args, Debug, Clone)]
pub struct DiagnoseArgs {
    /// Provider to diagnose, or "all" for enabled providers
    #[arg(short, long, default_value = "all")]
    pub provider: String,

    /// Data source: auto, web, cli, oauth
    #[arg(long, default_value = "auto", value_parser = ["auto", "web", "cli", "oauth"])]
    pub source: String,

    /// Web fetch timeout in seconds
    #[arg(long = "web-timeout", default_value = "60")]
    pub web_timeout: u64,

    /// Pretty-print JSON output
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Debug, Serialize)]
struct ProviderDiagnosticBatchExport {
    schema_version: u8,
    timestamp: DateTime<Utc>,
    diagnostics: Vec<ProviderDiagnosticExport>,
}

#[derive(Debug, Serialize)]
struct ProviderDiagnosticExport {
    schema_version: u8,
    timestamp: DateTime<Utc>,
    provider: String,
    display_name: String,
    source: Option<String>,
    source_mode: String,
    auth: ProviderDiagnosticAuthSummary,
    usage: Option<ProviderDiagnosticUsageSummary>,
    fetch_attempts: Vec<ProviderDiagnosticFetchAttempt>,
    error: Option<ProviderDiagnosticError>,
    settings: ProviderDiagnosticSettingsSummary,
}

#[derive(Debug, Serialize)]
struct ProviderDiagnosticAuthSummary {
    configured: bool,
    modes: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ProviderDiagnosticUsageSummary {
    windows: Vec<ProviderDiagnosticRateWindow>,
    provider_cost_present: bool,
    account_present: bool,
    login_method_present: bool,
}

#[derive(Debug, Serialize)]
struct ProviderDiagnosticRateWindow {
    label: String,
    used_percent: f64,
    window_minutes: Option<u32>,
    resets_at: Option<DateTime<Utc>>,
    has_reset_description: bool,
}

#[derive(Debug, Serialize)]
struct ProviderDiagnosticFetchAttempt {
    kind: String,
    was_available: bool,
    error_category: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProviderDiagnosticError {
    category: String,
    message: String,
}

#[derive(Debug, Serialize)]
struct ProviderDiagnosticSettingsSummary {
    enabled: bool,
    source_mode: String,
    cookie_source: String,
    api_region: String,
    supported_sources: Vec<String>,
}

pub async fn run(args: DiagnoseArgs) -> anyhow::Result<()> {
    let settings = Settings::load();
    let api_keys = ApiKeys::load();
    let manual_cookies = ManualCookies::load();
    let source_mode = SourceMode::parse(&args.source).unwrap_or(SourceMode::Auto);
    let provider_ids = select_providers(&args.provider, &settings)?;

    let mut diagnostics = Vec::with_capacity(provider_ids.len());
    for provider_id in provider_ids {
        diagnostics.push(
            collect_provider_diagnostic(
                provider_id,
                &settings,
                &api_keys,
                &manual_cookies,
                source_mode,
                args.web_timeout,
            )
            .await,
        );
    }

    let export = ProviderDiagnosticBatchExport {
        schema_version: 1,
        timestamp: Utc::now(),
        diagnostics,
    };

    let json = if args.pretty {
        serde_json::to_string_pretty(&export)
    } else {
        serde_json::to_string(&export)
    }
    .context("failed to serialize diagnostics")?;

    println!("{json}");
    Ok(())
}

fn select_providers(provider: &str, settings: &Settings) -> anyhow::Result<Vec<ProviderId>> {
    if provider.eq_ignore_ascii_case("all") {
        let enabled = settings.get_enabled_provider_ids();
        if enabled.is_empty() {
            return Ok(ProviderId::all().to_vec());
        }
        return Ok(enabled);
    }

    let id = ProviderId::from_cli_name(provider)
        .ok_or_else(|| anyhow::anyhow!("Unknown provider: {}", provider))?;
    Ok(vec![id])
}

async fn collect_provider_diagnostic(
    provider_id: ProviderId,
    settings: &Settings,
    api_keys: &ApiKeys,
    manual_cookies: &ManualCookies,
    source_mode: SourceMode,
    web_timeout: u64,
) -> ProviderDiagnosticExport {
    let provider = instantiate_provider(provider_id);
    let source_mode = configured_source_mode(provider_id, settings, source_mode);
    let ctx = FetchContext {
        source_mode,
        include_credits: true,
        web_timeout,
        verbose: false,
        manual_cookie_header: manual_cookies
            .get(provider_id.cli_name())
            .map(ToOwned::to_owned),
        api_key: api_keys.get(provider_id.cli_name()).map(ToOwned::to_owned),
        workspace_id: settings
            .provider_config(provider_id)
            .and_then(|config| config.workspace_id.clone()),
        api_region: settings
            .provider_config(provider_id)
            .and_then(|config| config.api_region.clone()),
        gateway_url: settings
            .provider_config(provider_id)
            .and_then(|config| config.gateway_url.clone()),
        auto_prefer_web: false,
        // Diagnostics keep the short optional-join grace (upstream #2583 gate).
        requires_optional_usage_completeness: false,
    };

    let fetch_result = provider.fetch_usage(&ctx).await;
    let (usage, source, error, fetch_attempts) = match fetch_result {
        Ok(result) => {
            let source = Some(result.source_label.clone());
            (
                Some(usage_summary(&result)),
                source,
                None,
                vec![ProviderDiagnosticFetchAttempt {
                    kind: source_mode_name(source_mode).to_string(),
                    was_available: true,
                    error_category: None,
                }],
            )
        }
        Err(err) => {
            let category = error_category(&err);
            (
                None,
                None,
                Some(ProviderDiagnosticError {
                    category: category.to_string(),
                    message: safe_error_message(&err),
                }),
                vec![ProviderDiagnosticFetchAttempt {
                    kind: source_mode_name(source_mode).to_string(),
                    was_available: false,
                    error_category: Some(category.to_string()),
                }],
            )
        }
    };

    let auth = auth_summary(
        provider_id,
        settings,
        api_keys,
        manual_cookies,
        source.as_deref(),
    );

    ProviderDiagnosticExport {
        schema_version: 1,
        timestamp: Utc::now(),
        provider: provider_id.cli_name().to_string(),
        display_name: provider.metadata().display_name.to_string(),
        source,
        source_mode: source_mode_name(source_mode).to_string(),
        auth,
        usage,
        fetch_attempts,
        error,
        settings: settings_summary(provider_id, settings),
    }
}

fn configured_source_mode(
    provider_id: ProviderId,
    settings: &Settings,
    requested: SourceMode,
) -> SourceMode {
    if requested != SourceMode::Auto {
        return requested;
    }

    SourceMode::parse(settings.usage_source(provider_id)).unwrap_or(SourceMode::Auto)
}

fn auth_summary(
    provider_id: ProviderId,
    settings: &Settings,
    api_keys: &ApiKeys,
    manual_cookies: &ManualCookies,
    resolved_source: Option<&str>,
) -> ProviderDiagnosticAuthSummary {
    let mut modes = Vec::new();
    let cli_name = provider_id.cli_name();

    if api_keys.has_key(cli_name) || !settings.api_token(provider_id).trim().is_empty() {
        modes.push("api_key".to_string());
    }
    if manual_cookies.get(cli_name).is_some()
        || !settings.manual_cookie_header(provider_id).trim().is_empty()
    {
        modes.push("manual_cookie".to_string());
    }
    if instantiate_provider(provider_id).supports_oauth() {
        modes.push("oauth_supported".to_string());
    }
    if instantiate_provider(provider_id).supports_cli() {
        modes.push("cli_supported".to_string());
    }
    if instantiate_provider(provider_id).supports_web() {
        modes.push("web_supported".to_string());
    }

    modes.sort();
    modes.dedup();

    ProviderDiagnosticAuthSummary {
        configured: resolved_source.is_some()
            || modes
                .iter()
                .any(|mode| matches!(mode.as_str(), "api_key" | "manual_cookie")),
        modes,
    }
}

fn settings_summary(
    provider_id: ProviderId,
    settings: &Settings,
) -> ProviderDiagnosticSettingsSummary {
    let provider = instantiate_provider(provider_id);
    ProviderDiagnosticSettingsSummary {
        enabled: settings.is_provider_enabled(provider_id),
        source_mode: settings.usage_source(provider_id).to_string(),
        cookie_source: settings.cookie_source(provider_id).to_string(),
        api_region: settings.api_region(provider_id).to_string(),
        supported_sources: provider
            .available_sources()
            .into_iter()
            .map(source_mode_name)
            .map(ToOwned::to_owned)
            .collect(),
    }
}

fn usage_summary(result: &ProviderFetchResult) -> ProviderDiagnosticUsageSummary {
    let usage = &result.usage;
    let mut windows = vec![rate_window_summary("primary", &usage.primary)];
    push_optional_window(&mut windows, "secondary", usage.secondary.as_ref());
    push_optional_window(
        &mut windows,
        "model_specific",
        usage.model_specific.as_ref(),
    );
    push_optional_window(&mut windows, "tertiary", usage.tertiary.as_ref());
    for extra in &usage.extra_rate_windows {
        windows.push(rate_window_summary(&extra.id, &extra.window));
    }

    ProviderDiagnosticUsageSummary {
        windows,
        provider_cost_present: cost_present(result.cost.as_ref()),
        account_present: usage.account_email.is_some() || usage.account_organization.is_some(),
        login_method_present: usage.login_method.is_some(),
    }
}

fn push_optional_window(
    windows: &mut Vec<ProviderDiagnosticRateWindow>,
    label: &str,
    window: Option<&RateWindow>,
) {
    if let Some(window) = window {
        windows.push(rate_window_summary(label, window));
    }
}

fn rate_window_summary(label: &str, window: &RateWindow) -> ProviderDiagnosticRateWindow {
    ProviderDiagnosticRateWindow {
        label: label.to_string(),
        used_percent: window.used_percent,
        window_minutes: window.window_minutes,
        resets_at: window.resets_at,
        has_reset_description: window.reset_description.is_some(),
    }
}

fn cost_present(cost: Option<&CostSnapshot>) -> bool {
    cost.is_some()
}

fn source_mode_name(mode: SourceMode) -> &'static str {
    match mode {
        SourceMode::Auto => "auto",
        SourceMode::OAuth => "oauth",
        SourceMode::Web => "web",
        SourceMode::Cli => "cli",
    }
}

fn error_category(err: &ProviderError) -> &'static str {
    match err {
        ProviderError::AuthRequired | ProviderError::OAuth(_) | ProviderError::NoCookies => "auth",
        ProviderError::Network(_) | ProviderError::Timeout => "network",
        ProviderError::NotInstalled(_) | ProviderError::UnsupportedSource(_) => "config",
        ProviderError::Parse(_) => "parse",
        ProviderError::Other(message) => {
            let lower = message.to_lowercase();
            if lower.contains("rate limit") || lower.contains("429") {
                "api"
            } else {
                "unknown"
            }
        }
    }
}

fn safe_error_message(err: &ProviderError) -> String {
    let message = err.to_string();
    if message.len() > 240 {
        format!("{}...", message.chars().take(237).collect::<String>())
    } else {
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{RateWindow, UsageSnapshot};

    #[test]
    fn source_mode_names_are_stable() {
        assert_eq!(source_mode_name(SourceMode::Auto), "auto");
        assert_eq!(source_mode_name(SourceMode::OAuth), "oauth");
        assert_eq!(source_mode_name(SourceMode::Web), "web");
        assert_eq!(source_mode_name(SourceMode::Cli), "cli");
    }

    #[test]
    fn diagnostic_usage_summary_does_not_export_identity_values() {
        let usage = UsageSnapshot::new(RateWindow::new(42.0))
            .with_email("person@example.com")
            .with_login_method("Team");
        let result = ProviderFetchResult::new(usage, "test");

        let value = serde_json::to_value(usage_summary(&result)).unwrap();
        let text = value.to_string();

        assert!(value["account_present"].as_bool().unwrap());
        assert!(value["login_method_present"].as_bool().unwrap());
        assert!(!text.contains("person@example.com"));
        assert!(!text.contains("Team"));
    }

    #[test]
    fn provider_selection_accepts_aliases() {
        let settings = Settings::default();
        assert_eq!(
            select_providers("anthropic", &settings).unwrap(),
            vec![ProviderId::Claude]
        );
    }

    #[test]
    fn provider_selection_defaults_to_enabled_providers() {
        let settings = Settings::default();
        let selected = select_providers("all", &settings).unwrap();
        assert!(!selected.is_empty());
    }
}
