//! `codexbar hooks` — list / enable / disable / test / watch external hook rules.

use clap::{Args, Subcommand};
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::core::{
    FetchContext, HookEvent, HookEventType, HookProviderObservation, HookProviderStatus,
    HookQuotaLaneKey, HookQuotaLaneObservation, HookQuotaWindow, HookRateLimiter, HookRunner,
    HookTransitionDetector, HooksConfig, ProviderError, ProviderId, RateWindow, SourceMode,
    instantiate_provider,
};
use crate::settings::{ApiKeys, Settings};
use crate::status::{StatusLevel, fetch_provider_status};

#[derive(Args, Debug, Clone)]
pub struct HooksArgs {
    #[command(subcommand)]
    pub command: HooksCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum HooksCommand {
    /// Print configured hook rules
    List(HooksListArgs),
    /// Enable hooks in hooks.json (master switch)
    Enable(HooksToggleArgs),
    /// Disable hooks in hooks.json (master switch)
    Disable(HooksToggleArgs),
    /// Run matching rules for a sample event
    Test(HooksTestArgs),
    /// Continuously poll providers and fire hooks on real transitions
    Watch(HooksWatchArgs),
}

#[derive(Args, Debug, Clone)]
pub struct HooksListArgs {
    /// Emit JSON
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args, Debug, Clone)]
pub struct HooksToggleArgs {
    /// Emit JSON
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args, Debug, Clone)]
pub struct HooksTestArgs {
    /// Event name (quota_low, quota_reached, quota_reset, provider_unavailable, provider_recovered, refresh_failed)
    pub event: String,
    /// Provider CLI name
    #[arg(long)]
    pub provider: String,
    /// Emit JSON
    #[arg(long)]
    pub json: bool,
    /// Pretty-print JSON
    #[arg(long)]
    pub pretty: bool,
}

/// Default poll period (seconds). Longer than serve cache TTL — watch originates
/// traffic against every enabled provider on every tick.
pub const HOOKS_WATCH_DEFAULT_INTERVAL: u64 = 300;
/// Floor for `--interval`. Rejected rather than clamped.
pub const HOOKS_WATCH_MINIMUM_INTERVAL: u64 = 60;
/// Sleep tick so Ctrl-C is noticed without waiting the full interval.
const HOOKS_WATCH_SLEEP_TICK: Duration = Duration::from_millis(200);

#[derive(Args, Debug, Clone)]
pub struct HooksWatchArgs {
    /// Poll period in seconds (default 300, minimum 60)
    #[arg(long, default_value_t = HOOKS_WATCH_DEFAULT_INTERVAL)]
    pub interval: u64,

    /// Provider CLI name(s); comma-separated or repeated. Default: enabled providers.
    #[arg(long, value_delimiter = ',')]
    pub provider: Vec<String>,

    /// Emit JSON hook events
    #[arg(long)]
    pub json: bool,

    /// Pretty-print JSON
    #[arg(long)]
    pub pretty: bool,

    /// Print fetch diagnostics
    #[arg(long)]
    pub verbose: bool,

    /// Web fetch timeout in seconds (default 60)
    #[arg(long = "web-timeout", default_value_t = 60)]
    pub web_timeout: u64,

    /// Data source: auto, web, cli, oauth
    #[arg(long, default_value = "auto", value_parser = ["auto", "web", "cli", "oauth"])]
    pub source: String,
}

#[derive(Debug, Serialize)]
struct HooksListJson {
    enabled: bool,
    settings_hooks_enabled: bool,
    path: Option<String>,
    rules: Vec<HookRuleListItem>,
}

#[derive(Debug, Serialize)]
struct HookRuleListItem {
    enabled: bool,
    event: Option<String>,
    events: Vec<String>,
    provider: Option<String>,
    executable: String,
    arguments: Vec<String>,
    timeout_secs: u64,
}

#[derive(Debug, Serialize)]
struct HookTestResult {
    executable: String,
    event: String,
    provider: String,
    ok: bool,
    error: Option<String>,
}

pub async fn run(args: HooksArgs) -> anyhow::Result<()> {
    match args.command {
        HooksCommand::List(a) => run_list(a),
        HooksCommand::Enable(a) => run_set_enabled(true, a),
        HooksCommand::Disable(a) => run_set_enabled(false, a),
        HooksCommand::Test(a) => run_test(a),
        HooksCommand::Watch(a) => run_watch(a).await,
    }
}

/// Continuously poll providers and dispatch hooks on real quota/status transitions.
async fn run_watch(args: HooksWatchArgs) -> anyhow::Result<()> {
    // Validate command-only args before reading config (upstream ordering).
    let interval = decode_hooks_watch_interval(args.interval)?;
    let explicit = decode_hooks_watch_providers(&args.provider)?;

    let hooks = HooksConfig::load();
    let settings = Settings::load();
    let providers = resolve_hooks_watch_providers(explicit, &settings)?;

    if !hooks.enabled {
        anyhow::bail!("Hooks are disabled. Run `codexbar hooks enable` first.");
    }
    if hooks.events.is_empty() {
        anyhow::bail!("No hook rules configured. See `codexbar hooks list`.");
    }

    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_signal = Arc::clone(&stop);
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        stop_for_signal.store(true, Ordering::SeqCst);
    });

    let mut detector = HookTransitionDetector::new();
    let rate_limiter = HookRateLimiter::default();
    let source_mode = SourceMode::parse(&args.source).unwrap_or(SourceMode::Auto);

    if !args.json {
        let names = providers
            .iter()
            .map(|p| p.cli_name())
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "Watching {} provider(s) every {}s: {}",
            providers.len(),
            interval.as_secs(),
            names
        );
        println!("Press Ctrl-C to stop.");
    }

    while !stop.load(Ordering::SeqCst) {
        for provider in &providers {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let observation = hooks_watch_observation(
                *provider,
                &settings,
                source_mode,
                args.web_timeout,
                args.verbose,
            )
            .await;

            let dispatches = detector.evaluate(&observation, &hooks);
            for dispatch in dispatches {
                report_hook_event(&dispatch.event, args.json, args.pretty)?;
                let dispatch_config = match &dispatch.rules {
                    Some(rules) => HooksConfig {
                        enabled: true,
                        events: rules.clone(),
                    },
                    None => hooks.clone(),
                };
                HookRunner::dispatch(&dispatch.event, &dispatch_config, &rate_limiter);
            }
        }

        if stop.load(Ordering::SeqCst) {
            break;
        }
        sleep_interruptibly(interval, &stop).await;
    }

    Ok(())
}

fn decode_hooks_watch_interval(raw: u64) -> anyhow::Result<Duration> {
    if raw < HOOKS_WATCH_MINIMUM_INTERVAL {
        anyhow::bail!(
            "--interval must be at least {} seconds.",
            HOOKS_WATCH_MINIMUM_INTERVAL
        );
    }
    Ok(Duration::from_secs(raw))
}

fn decode_hooks_watch_providers(names: &[String]) -> anyhow::Result<Option<Vec<ProviderId>>> {
    if names.is_empty() {
        return Ok(None);
    }
    let mut selected = Vec::new();
    for name in names {
        let id = ProviderId::from_cli_name(name)
            .ok_or_else(|| anyhow::anyhow!("Unknown provider: {name}"))?;
        if !selected.contains(&id) {
            selected.push(id);
        }
    }
    Ok(Some(selected))
}

fn resolve_hooks_watch_providers(
    explicit: Option<Vec<ProviderId>>,
    settings: &Settings,
) -> anyhow::Result<Vec<ProviderId>> {
    if let Some(list) = explicit {
        return Ok(list);
    }
    let enabled = settings.get_enabled_provider_ids();
    if enabled.is_empty() {
        anyhow::bail!("No providers are enabled.");
    }
    Ok(enabled)
}

async fn hooks_watch_observation(
    provider_id: ProviderId,
    settings: &Settings,
    source_mode: SourceMode,
    web_timeout: u64,
    verbose: bool,
) -> HookProviderObservation {
    let status = match fetch_provider_status(provider_id.cli_name()).await {
        Some(s) => map_status_level(s.level),
        None => HookProviderStatus::Unknown,
    };

    let workspace = settings.workspace_id(provider_id);
    let region = settings.api_region(provider_id);
    let gateway = settings.gateway_url(provider_id);

    let mut ctx = FetchContext {
        source_mode,
        include_credits: false,
        web_timeout,
        verbose,
        manual_cookie_header: None,
        api_key: None,
        workspace_id: (!workspace.is_empty()).then(|| workspace.to_string()),
        api_region: (!region.is_empty()).then(|| region.to_string()),
        gateway_url: (!gateway.is_empty()).then(|| gateway.to_string()),
        auto_prefer_web: false,
        // Hook watches keep the short optional-join grace.
        requires_optional_usage_completeness: false,
    };

    if ctx.api_key.is_none() {
        ctx.api_key = ApiKeys::load()
            .get(provider_id.cli_name())
            .map(|s| s.to_string());
    }

    let provider = instantiate_provider(provider_id);
    match provider.fetch_usage(&ctx).await {
        Ok(result) => {
            let usage = &result.usage;
            let account = usage.account_email.clone();
            HookProviderObservation {
                provider: provider_id.cli_name().to_string(),
                lanes: hooks_watch_lanes(provider_id, usage, settings, account.as_deref()),
                status,
                refresh_failure_status: None,
                account_display_name: account,
            }
        }
        Err(err) => HookProviderObservation {
            provider: provider_id.cli_name().to_string(),
            lanes: Vec::new(),
            status,
            refresh_failure_status: Some(hook_refresh_failure_status(&err)),
            account_display_name: None,
        },
    }
}

fn hooks_watch_lanes(
    provider_id: ProviderId,
    usage: &crate::core::UsageSnapshot,
    settings: &Settings,
    account: Option<&str>,
) -> Vec<HookQuotaLaneObservation> {
    let mut lanes = Vec::new();
    let pairs: [(HookQuotaWindow, Option<&RateWindow>); 2] = [
        (HookQuotaWindow::Session, Some(&usage.primary)),
        (HookQuotaWindow::Weekly, usage.secondary.as_ref()),
    ];
    for (window, rate_window) in pairs {
        let Some(rate_window) = rate_window else {
            continue;
        };
        if rate_window.is_informational {
            continue;
        }
        let thresholds = settings.usage_thresholds(provider_id, window.as_str());
        // Settings store *used* percentages; detector math uses used fractions.
        let fallback = [thresholds.high / 100.0, thresholds.critical / 100.0];
        lanes.push(HookQuotaLaneObservation {
            key: HookQuotaLaneKey::new(
                provider_id.cli_name(),
                window,
                account.map(str::to_string),
                None,
            ),
            label: window.display_name().to_string(),
            rate_window: Some(rate_window.clone()),
            fallback_thresholds: fallback.to_vec(),
            account_display_name: account.map(str::to_string),
        });
    }
    lanes
}

fn map_status_level(level: StatusLevel) -> HookProviderStatus {
    match level {
        StatusLevel::Operational => HookProviderStatus::None,
        StatusLevel::Degraded => HookProviderStatus::Minor,
        StatusLevel::Partial => HookProviderStatus::Major,
        StatusLevel::Major => HookProviderStatus::Critical,
        StatusLevel::Unknown => HookProviderStatus::Unknown,
    }
}

/// Coarse, non-secret category for a refresh failure. Never forwards raw errors.
fn hook_refresh_failure_status(error: &ProviderError) -> String {
    match error {
        ProviderError::AuthRequired | ProviderError::NoCookies | ProviderError::OAuth(_) => {
            "auth_required".into()
        }
        ProviderError::Timeout => "timeout".into(),
        ProviderError::Network(err) => {
            if err.is_timeout() {
                "timeout".into()
            } else if err.is_connect() {
                "offline".into()
            } else {
                "network_error".into()
            }
        }
        ProviderError::NotInstalled(_) => "error".into(),
        ProviderError::Parse(_) | ProviderError::UnsupportedSource(_) | ProviderError::Other(_) => {
            "error".into()
        }
    }
}

async fn sleep_interruptibly(interval: Duration, stop: &AtomicBool) {
    let mut remaining = interval;
    while remaining > Duration::ZERO && !stop.load(Ordering::SeqCst) {
        let tick = remaining.min(HOOKS_WATCH_SLEEP_TICK);
        tokio::time::sleep(tick).await;
        remaining = remaining.saturating_sub(tick);
    }
}

fn report_hook_event(event: &HookEvent, json: bool, pretty: bool) -> anyhow::Result<()> {
    if json {
        print_json(event, pretty)?;
        return Ok(());
    }
    let mut line = format!("{} {}", event.event.as_str(), event.provider);
    if let Some(window) = &event.window {
        line.push_str(&format!(" window={window}"));
    }
    if let Some(usage) = event.usage_percent {
        line.push_str(&format!(" usage={:.0}%", usage * 100.0));
    }
    if let Some(status) = &event.status {
        line.push_str(&format!(" status={status}"));
    }
    println!("{line}");
    Ok(())
}

fn run_list(args: HooksListArgs) -> anyhow::Result<()> {
    let config = HooksConfig::load();
    let settings = Settings::load();
    let path = HooksConfig::path().map(|p| p.display().to_string());

    if args.json {
        let payload = HooksListJson {
            enabled: config.enabled,
            settings_hooks_enabled: settings.hooks_enabled,
            path,
            rules: config
                .events
                .iter()
                .map(|r| HookRuleListItem {
                    enabled: r.enabled,
                    event: r.event.map(|e| e.as_str().to_string()),
                    events: r.events.iter().map(|e| e.as_str().to_string()).collect(),
                    provider: r.provider.clone(),
                    executable: r.executable.display().to_string(),
                    arguments: r.arguments.clone(),
                    timeout_secs: r.timeout_secs,
                })
                .collect(),
        };
        print_json(&payload, args.pretty)?;
        return Ok(());
    }

    println!(
        "Hooks: {} (settings toggle: {})",
        if config.enabled {
            "enabled"
        } else {
            "disabled"
        },
        if settings.hooks_enabled { "on" } else { "off" }
    );
    if let Some(path) = path {
        println!("Config: {path}");
    }
    if config.events.is_empty() {
        println!("No rules configured.");
        return Ok(());
    }
    for rule in &config.events {
        let state = if rule.enabled { "on" } else { "off" };
        let event = rule
            .event
            .map(|e| e.as_str().to_string())
            .or_else(|| {
                (!rule.events.is_empty()).then(|| {
                    rule.events
                        .iter()
                        .map(|e| e.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                })
            })
            .unwrap_or_else(|| "any".into());
        let provider = rule.provider.as_deref().unwrap_or("any");
        let command = std::iter::once(rule.executable.display().to_string())
            .chain(rule.arguments.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");
        println!("[{state}] {event} provider={provider}: {command}");
    }
    Ok(())
}

fn run_set_enabled(enabled: bool, args: HooksToggleArgs) -> anyhow::Result<()> {
    let mut config = HooksConfig::load();
    config.enabled = enabled;
    let path = config.save().map_err(anyhow::Error::msg)?;
    if args.json {
        print_json(
            &serde_json::json!({
                "enabled": config.enabled,
                "path": path.display().to_string(),
            }),
            args.pretty,
        )?;
    } else {
        println!(
            "Hooks: {} ({})",
            if enabled { "enabled" } else { "disabled" },
            path.display()
        );
    }
    Ok(())
}

fn run_test(args: HooksTestArgs) -> anyhow::Result<()> {
    let event_type = parse_event(&args.event)?;
    let provider = ProviderId::from_cli_name(&args.provider).ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown provider '{}'. Use a CLI name such as codex or claude.",
            args.provider
        )
    })?;

    let event = sample_event(event_type, provider.cli_name());
    let config = HooksConfig::load();
    if !config.enabled {
        anyhow::bail!("Hooks are disabled. Run `codexbar hooks enable` first.");
    }
    let settings = Settings::load();
    if !settings.hooks_enabled {
        anyhow::bail!(
            "Hooks are disabled in Settings (hooks_enabled=false). Enable them in Advanced."
        );
    }

    let rules = config.matching_rules(&event);
    if rules.is_empty() {
        anyhow::bail!(
            "No hook rule matches {} for {}.",
            event_type.as_str(),
            provider.cli_name()
        );
    }

    let base_env = std::env::vars().collect();
    let mut results = Vec::new();
    for rule in rules {
        match HookRunner::run(rule, &event, &base_env) {
            Ok(()) => results.push(HookTestResult {
                executable: rule.executable.display().to_string(),
                event: event_type.as_str().into(),
                provider: provider.cli_name().into(),
                ok: true,
                error: None,
            }),
            Err(err) => results.push(HookTestResult {
                executable: rule.executable.display().to_string(),
                event: event_type.as_str().into(),
                provider: provider.cli_name().into(),
                ok: false,
                error: Some(err),
            }),
        }
    }

    if args.json {
        print_json(&results, args.pretty)?;
    } else {
        for r in &results {
            if r.ok {
                println!("ok  {} ({})", r.executable, r.event);
            } else {
                println!(
                    "err {} ({}) — {}",
                    r.executable,
                    r.event,
                    r.error.as_deref().unwrap_or("error")
                );
            }
        }
    }

    if results.iter().any(|r| !r.ok) {
        anyhow::bail!("one or more hooks failed");
    }
    Ok(())
}

fn sample_event(event: HookEventType, provider: &str) -> HookEvent {
    let mut e = HookEvent::new(event, provider).with_window("session");
    match event {
        HookEventType::QuotaLow => e = e.with_used_percent(85.0),
        HookEventType::QuotaReached => e = e.with_used_percent(100.0),
        HookEventType::QuotaReset => e = e.with_used_percent(5.0),
        HookEventType::ProviderUnavailable => e = e.with_status("unavailable"),
        HookEventType::ProviderRecovered => e = e.with_status("ok"),
        HookEventType::RefreshFailed => e = e.with_status("refresh_failed"),
    }
    e
}

fn parse_event(raw: &str) -> anyhow::Result<HookEventType> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "quota_low" => Ok(HookEventType::QuotaLow),
        "quota_reached" => Ok(HookEventType::QuotaReached),
        "quota_reset" => Ok(HookEventType::QuotaReset),
        "provider_unavailable" => Ok(HookEventType::ProviderUnavailable),
        "provider_recovered" => Ok(HookEventType::ProviderRecovered),
        "refresh_failed" => Ok(HookEventType::RefreshFailed),
        other => anyhow::bail!(
            "Unknown event '{other}'. Use one of: quota_low, quota_reached, quota_reset, provider_unavailable, provider_recovered, refresh_failed."
        ),
    }
}

fn print_json<T: Serialize>(value: &T, pretty: bool) -> anyhow::Result<()> {
    if pretty {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        println!("{}", serde_json::to_string(value)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_event_names() {
        assert!(matches!(
            parse_event("quota_low").unwrap(),
            HookEventType::QuotaLow
        ));
        assert!(parse_event("nope").is_err());
    }

    #[test]
    fn sample_quota_low_has_remaining() {
        let e = sample_event(HookEventType::QuotaLow, "claude");
        assert!(e.remaining_percent.unwrap() < 20.0);
        assert_eq!(e.provider, "claude");
        assert!(e.environment_variables().contains_key("CODEXBAR_PROVIDER"));
    }

    #[test]
    fn watch_interval_rejects_below_floor() {
        assert!(decode_hooks_watch_interval(59).is_err());
        assert!(decode_hooks_watch_interval(60).is_ok());
        assert_eq!(
            decode_hooks_watch_interval(300).unwrap(),
            Duration::from_secs(300)
        );
    }

    #[test]
    fn watch_providers_reject_unknown_and_dedupe() {
        assert!(decode_hooks_watch_providers(&["nosuch".into()]).is_err());
        let got = decode_hooks_watch_providers(&["codex".into(), "codex".into()])
            .unwrap()
            .unwrap();
        assert_eq!(got, vec![ProviderId::Codex]);
        assert!(decode_hooks_watch_providers(&[]).unwrap().is_none());
    }
}
