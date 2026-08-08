//! Guard command — gate automation on one provider's remaining quota.
//!
//! Stable exit codes (sysexits-aligned):
//! - `0` safe (remaining ≥ threshold)
//! - `1` blocked (remaining below threshold)
//! - `64` usage / invalid arguments (`EX_USAGE`)
//! - `69` unavailable (`EX_UNAVAILABLE`; becomes `0` with `--fail-open`)

use clap::Args;
use serde::Serialize;
use tokio::time::{Duration, timeout};

use super::exit_codes;
use super::usage::ProviderSelection;
use crate::core::{FetchContext, ProviderId, RateWindow, SourceMode, instantiate_provider};

/// Arguments for `codexbar guard`.
#[derive(Args, Debug, Clone)]
pub struct GuardArgs {
    /// Provider to check (exactly one; required)
    #[arg(short, long)]
    pub provider: String,

    /// Minimum remaining quota required, as a percent (default 10)
    #[arg(long = "min-remaining", default_value = "10")]
    pub min_remaining: f64,

    /// Window to check: session (primary) | weekly (secondary)
    #[arg(long, default_value = "session", value_parser = ["session", "weekly"])]
    pub window: String,

    /// Overall fetch timeout in seconds, 0…86400 (default 60; 0 disables)
    #[arg(long, default_value = "60")]
    pub timeout: f64,

    /// Emit machine-readable decision JSON
    #[arg(long)]
    pub json: bool,

    /// Pretty-print decision JSON
    #[arg(long)]
    pub pretty: bool,

    /// Exit 0 instead of 69 when quota is unavailable
    #[arg(long = "fail-open")]
    pub fail_open: bool,

    /// Data source: auto, web, cli, oauth
    #[arg(long, default_value = "auto", value_parser = ["auto", "web", "cli", "oauth"])]
    pub source: String,
}

/// Window selected by the guard command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardWindow {
    Session,
    Weekly,
}

impl GuardWindow {
    pub fn as_str(self) -> &'static str {
        match self {
            GuardWindow::Session => "session",
            GuardWindow::Weekly => "weekly",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.to_lowercase().as_str() {
            "session" => Some(GuardWindow::Session),
            "weekly" => Some(GuardWindow::Weekly),
            _ => None,
        }
    }
}

/// Pure gating outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GuardDecision {
    Ok,
    Blocked,
    Unknown,
}

impl GuardDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            GuardDecision::Ok => "ok",
            GuardDecision::Blocked => "blocked",
            GuardDecision::Unknown => "unknown",
        }
    }
}

/// Why a quota check could not produce a remaining percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GuardUnavailableReason {
    AccountResolution,
    FetchFailed,
    Timeout,
    WindowUnavailable,
}

impl GuardUnavailableReason {
    pub fn as_str(self) -> &'static str {
        match self {
            GuardUnavailableReason::AccountResolution => "account-resolution",
            GuardUnavailableReason::FetchFailed => "fetch-failed",
            GuardUnavailableReason::Timeout => "timeout",
            GuardUnavailableReason::WindowUnavailable => "window-unavailable",
        }
    }
}

/// Result of attempting to read remaining quota.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GuardFetchOutcome {
    Available(f64),
    Unavailable(GuardUnavailableReason),
}

/// Pure evaluation result for exit-code mapping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuardEvaluation {
    pub decision: GuardDecision,
    pub exit_code: i32,
    pub remaining_percent: Option<f64>,
    pub unavailable_reason: Option<GuardUnavailableReason>,
}

/// Pure decision core for `codexbar guard`.
///
/// - unavailable quota → `.unknown` (exit `0` when `fail_open`, else `69`)
/// - remaining ≥ threshold → `.ok` (exit `0`)
/// - otherwise → `.blocked` (exit `1`)
pub fn evaluate_guard(
    outcome: GuardFetchOutcome,
    minimum_remaining_percent: f64,
    fail_open: bool,
) -> GuardEvaluation {
    match outcome {
        GuardFetchOutcome::Unavailable(reason) => GuardEvaluation {
            decision: GuardDecision::Unknown,
            exit_code: if fail_open {
                exit_codes::SUCCESS
            } else {
                exit_codes::UNAVAILABLE
            },
            remaining_percent: None,
            unavailable_reason: Some(reason),
        },
        GuardFetchOutcome::Available(remaining) if remaining >= minimum_remaining_percent => {
            GuardEvaluation {
                decision: GuardDecision::Ok,
                exit_code: exit_codes::SUCCESS,
                remaining_percent: Some(remaining),
                unavailable_reason: None,
            }
        }
        GuardFetchOutcome::Available(remaining) => GuardEvaluation {
            decision: GuardDecision::Blocked,
            exit_code: exit_codes::GUARD_BLOCKED,
            remaining_percent: Some(remaining),
            unavailable_reason: None,
        },
    }
}

/// Remaining headroom (`100 - used_percent`) for a rate window, or `None` when
/// the window is absent or informational (not a real quota lane).
pub fn guard_remaining_headroom(window: Option<&RateWindow>) -> Option<f64> {
    let window = window?;
    if window.is_informational {
        return None;
    }
    Some(100.0 - window.used_percent)
}

/// Validate `--min-remaining` (finite percent in `0…100`).
pub fn parse_min_remaining(value: f64) -> Result<f64, &'static str> {
    if value.is_finite() && (0.0..=100.0).contains(&value) {
        Ok(value)
    } else {
        Err("--min-remaining must be a finite percent between 0 and 100.")
    }
}

/// Validate `--timeout` (finite seconds in `0…86400`).
pub fn parse_timeout_secs(value: f64) -> Result<f64, &'static str> {
    if value.is_finite() && (0.0..=86400.0).contains(&value) {
        Ok(value)
    } else {
        Err("--timeout must be a finite number of seconds from 0 through 86400.")
    }
}

/// Resolve exactly one provider for guard.
pub fn resolve_guard_provider(raw: &str) -> Result<ProviderId, String> {
    match ProviderSelection::from_arg(Some(raw)) {
        Ok(ProviderSelection::Single(id)) => Ok(id),
        Ok(ProviderSelection::Both | ProviderSelection::All) => {
            Err("guard requires exactly one --provider.".to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

#[derive(Debug, Serialize)]
struct GuardResultPayload {
    provider: String,
    window: String,
    #[serde(rename = "remainingPercent")]
    remaining_percent: Option<f64>,
    #[serde(rename = "minimumRemainingPercent")]
    minimum_remaining_percent: f64,
    decision: String,
    #[serde(rename = "exitCode")]
    exit_code: i32,
    #[serde(rename = "unavailableReason")]
    unavailable_reason: Option<String>,
}

/// Run the guard command. Returns the process exit code.
pub async fn run(args: GuardArgs) -> i32 {
    let window = match GuardWindow::parse(&args.window) {
        Some(w) => w,
        None => {
            eprintln!("Error: --window must be session|weekly.");
            return exit_codes::USAGE_ERROR;
        }
    };

    let minimum_remaining = match parse_min_remaining(args.min_remaining) {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("Error: {}", msg);
            return exit_codes::USAGE_ERROR;
        }
    };

    let timeout_secs = match parse_timeout_secs(args.timeout) {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("Error: {}", msg);
            return exit_codes::USAGE_ERROR;
        }
    };

    let provider_id = match resolve_guard_provider(&args.provider) {
        Ok(id) => id,
        Err(msg) => {
            eprintln!("Error: {}", msg);
            return exit_codes::USAGE_ERROR;
        }
    };

    let source_mode = SourceMode::parse(&args.source).unwrap_or(SourceMode::Auto);
    let web_timeout = if timeout_secs > 0.0 {
        timeout_secs.ceil() as u64
    } else {
        60
    };

    let outcome = run_guard_fetch(timeout_secs, || async {
        fetch_guard_outcome(provider_id, window, source_mode, web_timeout).await
    })
    .await;

    let evaluation = evaluate_guard(outcome, minimum_remaining, args.fail_open);
    emit_guard_result(
        provider_id,
        window,
        minimum_remaining,
        evaluation,
        args.json,
        args.pretty,
    );
    evaluation.exit_code
}

async fn run_guard_fetch<F, Fut>(timeout_secs: f64, operation: F) -> GuardFetchOutcome
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = GuardFetchOutcome>,
{
    if timeout_secs <= 0.0 {
        return operation().await;
    }

    let duration = Duration::from_secs_f64(timeout_secs);
    match timeout(duration, operation()).await {
        Ok(outcome) => outcome,
        Err(_) => GuardFetchOutcome::Unavailable(GuardUnavailableReason::Timeout),
    }
}

async fn fetch_guard_outcome(
    provider_id: ProviderId,
    window: GuardWindow,
    source_mode: SourceMode,
    web_timeout: u64,
) -> GuardFetchOutcome {
    let provider = instantiate_provider(provider_id);

    let ctx = FetchContext {
        source_mode,
        include_credits: false,
        web_timeout,
        verbose: false,
        manual_cookie_header: None,
        api_key: None,
        workspace_id: None,
        api_region: None,
        gateway_url: None,
        auto_prefer_web: false,
        // Guard checks keep the short optional-join grace.
        requires_optional_usage_completeness: false,
    };

    match provider.fetch_usage(&ctx).await {
        Ok(result) => {
            let rate_window = match window {
                GuardWindow::Session => Some(&result.usage.primary),
                GuardWindow::Weekly => result.usage.secondary.as_ref(),
            };
            match guard_remaining_headroom(rate_window) {
                Some(remaining) => GuardFetchOutcome::Available(remaining),
                None => GuardFetchOutcome::Unavailable(GuardUnavailableReason::WindowUnavailable),
            }
        }
        Err(_) => GuardFetchOutcome::Unavailable(GuardUnavailableReason::FetchFailed),
    }
}

fn emit_guard_result(
    provider_id: ProviderId,
    window: GuardWindow,
    minimum_remaining: f64,
    evaluation: GuardEvaluation,
    json: bool,
    pretty: bool,
) {
    if json {
        let payload = GuardResultPayload {
            provider: provider_id.cli_name().to_string(),
            window: window.as_str().to_string(),
            remaining_percent: evaluation.remaining_percent,
            minimum_remaining_percent: minimum_remaining,
            decision: evaluation.decision.as_str().to_string(),
            exit_code: evaluation.exit_code,
            unavailable_reason: evaluation
                .unavailable_reason
                .map(|r| r.as_str().to_string()),
        };
        let rendered = if pretty {
            serde_json::to_string_pretty(&payload)
        } else {
            serde_json::to_string(&payload)
        };
        match rendered {
            Ok(s) => println!("{}", s),
            Err(e) => eprintln!("Error: failed to encode JSON: {}", e),
        }
        return;
    }

    println!(
        "{}",
        guard_human_line(
            provider_id,
            window,
            evaluation.remaining_percent,
            minimum_remaining,
            evaluation.decision,
            evaluation.unavailable_reason,
        )
    );
}

/// Human-readable one-line summary (non-JSON mode).
pub fn guard_human_line(
    provider_id: ProviderId,
    window: GuardWindow,
    remaining_percent: Option<f64>,
    minimum_remaining_percent: f64,
    decision: GuardDecision,
    unavailable_reason: Option<GuardUnavailableReason>,
) -> String {
    let remaining_text = remaining_percent
        .map(|v| format!("{} remaining", guard_percent_string(v)))
        .unwrap_or_else(|| "unknown".to_string());
    let verdict = match decision {
        GuardDecision::Ok => "OK",
        GuardDecision::Blocked => "BLOCKED",
        GuardDecision::Unknown => "UNKNOWN",
    };
    let reason_text = unavailable_reason
        .map(|r| format!("; {}", r.as_str()))
        .unwrap_or_default();
    format!(
        "{} {}: {} — {} (minimum {}{})",
        provider_id.cli_name(),
        window.as_str(),
        remaining_text,
        verdict,
        guard_percent_string(minimum_remaining_percent),
        reason_text
    )
}

fn guard_percent_string(value: f64) -> String {
    let rounded = value.round();
    if (value - rounded).abs() < 0.05 {
        format!("{}%", rounded as i64)
    } else {
        format!("{:.1}%", value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ample_headroom_is_ok_and_exits_zero() {
        let result = evaluate_guard(GuardFetchOutcome::Available(74.0), 10.0, false);
        assert_eq!(result.decision, GuardDecision::Ok);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.remaining_percent, Some(74.0));
    }

    #[test]
    fn insufficient_headroom_is_blocked_and_exits_one() {
        let result = evaluate_guard(GuardFetchOutcome::Available(5.0), 10.0, false);
        assert_eq!(result.decision, GuardDecision::Blocked);
        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn fetch_failure_exits_unavailable_by_default() {
        let result = evaluate_guard(
            GuardFetchOutcome::Unavailable(GuardUnavailableReason::FetchFailed),
            10.0,
            false,
        );
        assert_eq!(result.decision, GuardDecision::Unknown);
        assert_eq!(result.exit_code, 69);
        assert_eq!(
            result.unavailable_reason,
            Some(GuardUnavailableReason::FetchFailed)
        );
    }

    #[test]
    fn unknown_remaining_with_fail_open_exits_zero() {
        let result = evaluate_guard(
            GuardFetchOutcome::Unavailable(GuardUnavailableReason::FetchFailed),
            10.0,
            true,
        );
        assert_eq!(result.decision, GuardDecision::Unknown);
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn remaining_exactly_equal_to_need_is_ok() {
        let result = evaluate_guard(GuardFetchOutcome::Available(10.0), 10.0, false);
        assert_eq!(result.decision, GuardDecision::Ok);
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn real_window_reports_remaining_headroom() {
        let window = RateWindow::new(30.0);
        let remaining = guard_remaining_headroom(Some(&window));
        assert_eq!(remaining, Some(70.0));
    }

    #[test]
    fn informational_window_is_treated_as_unknown() {
        let window = RateWindow::informational("no session");
        assert_eq!(guard_remaining_headroom(Some(&window)), None);
    }

    #[test]
    fn absent_window_is_unknown() {
        assert_eq!(guard_remaining_headroom(None), None);
    }

    #[test]
    fn fully_used_real_window_has_zero_headroom() {
        let window = RateWindow::new(100.0);
        assert_eq!(guard_remaining_headroom(Some(&window)), Some(0.0));
    }

    #[test]
    fn missing_provider_rejected_via_empty_string_path() {
        let err = resolve_guard_provider("").unwrap_err();
        assert!(err.to_lowercase().contains("unknown") || err.to_lowercase().contains("provider"));
    }

    #[test]
    fn both_provider_rejected() {
        let err = resolve_guard_provider("both").unwrap_err();
        assert!(err.contains("exactly one"));
    }

    #[test]
    fn all_provider_rejected() {
        let err = resolve_guard_provider("all").unwrap_err();
        assert!(err.contains("exactly one"));
    }

    #[test]
    fn known_provider_accepted() {
        assert_eq!(
            resolve_guard_provider("claude").unwrap(),
            ProviderId::Claude
        );
        assert_eq!(resolve_guard_provider("codex").unwrap(), ProviderId::Codex);
    }

    #[test]
    fn min_remaining_bounds() {
        assert_eq!(parse_min_remaining(10.0).unwrap(), 10.0);
        assert_eq!(parse_min_remaining(0.0).unwrap(), 0.0);
        assert_eq!(parse_min_remaining(100.0).unwrap(), 100.0);
        assert!(parse_min_remaining(-1.0).is_err());
        assert!(parse_min_remaining(101.0).is_err());
        assert!(parse_min_remaining(f64::NAN).is_err());
        assert!(parse_min_remaining(f64::INFINITY).is_err());
    }

    #[test]
    fn timeout_bounds() {
        assert_eq!(parse_timeout_secs(60.0).unwrap(), 60.0);
        assert_eq!(parse_timeout_secs(0.0).unwrap(), 0.0);
        assert_eq!(parse_timeout_secs(86400.0).unwrap(), 86400.0);
        assert!(parse_timeout_secs(-1.0).is_err());
        assert!(parse_timeout_secs(86401.0).is_err());
        assert!(parse_timeout_secs(1e100).is_err());
    }

    #[test]
    fn human_line_formats_ok() {
        let line = guard_human_line(
            ProviderId::Claude,
            GuardWindow::Session,
            Some(74.0),
            10.0,
            GuardDecision::Ok,
            None,
        );
        assert_eq!(line, "claude session: 74% remaining — OK (minimum 10%)");
    }

    #[test]
    fn human_line_formats_unknown_with_reason() {
        let line = guard_human_line(
            ProviderId::Codex,
            GuardWindow::Weekly,
            None,
            10.0,
            GuardDecision::Unknown,
            Some(GuardUnavailableReason::Timeout),
        );
        assert_eq!(
            line,
            "codex weekly: unknown — UNKNOWN (minimum 10%; timeout)"
        );
    }

    #[tokio::test]
    async fn fetch_timeout_is_reported_as_unavailable() {
        let result = run_guard_fetch(0.01, || async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            GuardFetchOutcome::Available(100.0)
        })
        .await;
        assert_eq!(
            result,
            GuardFetchOutcome::Unavailable(GuardUnavailableReason::Timeout)
        );
    }
}
