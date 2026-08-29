//! Claude provider implementation

mod admin_api;
mod cli_reset;
mod oauth;
mod scoped_weekly;
mod web_api;

use async_trait::async_trait;
use chrono::Utc;
use regex_lite::Regex;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
use std::process::{Command as StdCommand, Stdio};
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::cli::tty_runner::{TtyCommandOptions, TtyCommandRunner};
use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};

use admin_api::ClaudeAdminApiFetcher;
#[cfg(test)]
use cli_reset::parse_claude_reset_date_in_system_zone;
use cli_reset::{
    extract_cli_scoped_weekly_limits, normalized_for_label_search, parse_claude_reset_date,
    parse_percent_line, starts_next_usage_section,
};

// ── Upstream 0.50.1 #2516: CLI usage-result cache ────────────────────────────
//
// When token rotation revokes OAuth access, the auto path falls back to the
// CLI. To avoid hammering the CLI probe on every poll, cache the last
// successful CLI result for 15 minutes. The cache is only consulted when
// OAuth returned `OAuthRevoked` (revoked, not merely expired) so normal
// refresh cycles are unaffected.
const CLI_RESULT_CACHE_TTL: Duration = Duration::from_secs(15 * 60);

struct CachedCliResult {
    result: ProviderFetchResult,
    cached_at: Instant,
}

static CLI_RESULT_CACHE: LazyLock<Mutex<Option<CachedCliResult>>> =
    LazyLock::new(|| Mutex::new(None));

/// Store a successful CLI fetch result in the 15-minute cache.
fn cache_cli_result(result: ProviderFetchResult) {
    if let Ok(mut guard) = CLI_RESULT_CACHE.lock() {
        *guard = Some(CachedCliResult {
            result,
            cached_at: Instant::now(),
        });
    }
}

/// Return a cached CLI result if it is still within the TTL. Used when
/// revoked OAuth prevents a live fetch and the CLI should not be re-probed.
fn cached_cli_result() -> Option<ProviderFetchResult> {
    let Ok(guard) = CLI_RESULT_CACHE.lock() else {
        return None;
    };
    guard
        .as_ref()
        .filter(|entry| entry.cached_at.elapsed() <= CLI_RESULT_CACHE_TTL)
        .map(|entry| entry.result.clone())
}

/// Whether the OAuth source failed with a revocation (not just expiry).
/// Revoked tokens should reuse the working CLI fallback; expired/missing
/// tokens should NOT block the normal refresh path.
fn is_oauth_revoked_error(error: &ProviderError) -> bool {
    matches!(error, ProviderError::OAuthRevoked(_))
}
pub use oauth::ClaudeOAuthFetcher;
pub use web_api::ClaudeWebApiFetcher;

/// Whether the user explicitly consented to reading (and refreshing) Claude
/// Code's own credentials. Upstream #2634/#2745: without consent the
/// file/keyring sources stay closed and refreshed tokens are never rotated
/// into Claude Code's storage; Auto then falls back to labeled
/// reduced-fidelity CLI usage.
pub(crate) fn claude_code_consent() -> bool {
    crate::settings::Settings::load().claude_allow_reading_claude_code_credentials
}

/// Claude provider implementation
pub struct ClaudeProvider {
    metadata: ProviderMetadata,
    web_fetcher: ClaudeWebApiFetcher,
    oauth_fetcher: ClaudeOAuthFetcher,
    admin_fetcher: ClaudeAdminApiFetcher,
}

impl ClaudeProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Claude,
                display_name: "Claude",
                session_label: "Session (5h)",
                weekly_label: "Weekly",
                supports_opus: true,
                supports_credits: true,
                default_enabled: true,
                is_primary: true,
                dashboard_url: Some("https://claude.ai/settings/usage"),
                status_page_url: Some("https://status.claude.com/"),
            },
            web_fetcher: ClaudeWebApiFetcher::new(),
            oauth_fetcher: ClaudeOAuthFetcher::new(),
            admin_fetcher: ClaudeAdminApiFetcher::new(),
        }
    }
}

impl Default for ClaudeProvider {
    fn default() -> Self {
        Self::new()
    }
}

fn claude_plan_label(tier: &str) -> String {
    let normalized = tier.to_lowercase();
    if normalized.contains("claude_max_5x") || normalized.contains("claude_max_5") {
        "Claude Max 5x".to_string()
    } else if normalized.contains("claude_max_20x") || normalized.contains("claude_max_20") {
        "Claude Max 20x".to_string()
    } else {
        match normalized.as_str() {
            "free" => "Claude Free".to_string(),
            "pro" | "claude_pro" => "Claude Pro".to_string(),
            "max" => "Claude Max".to_string(),
            "team" => "Claude Team".to_string(),
            "enterprise" => "Claude Enterprise".to_string(),
            _ => format!("Claude ({})", tier),
        }
    }
}

const CLAUDE_PROBE_SESSION_ID_FILE: &str = ".codexbar-session-id";

fn claude_usage_probe_dir() -> Result<std::path::PathBuf, ProviderError> {
    let base = dirs::data_local_dir()
        .or_else(dirs::home_dir)
        .ok_or_else(|| {
            ProviderError::Other("Could not resolve a local data directory".to_string())
        })?;
    let dir = base.join("CodexBar").join("claude-usage-probe");
    std::fs::create_dir_all(&dir).map_err(|e| {
        ProviderError::Other(format!(
            "Failed to prepare Claude CLI probe directory: {}",
            e
        ))
    })?;
    Ok(dir)
}

/// Persist and reuse one probe session id so repeated `/usage` PTY launches do
/// not register a fresh empty Claude account session each refresh (upstream #2263).
fn load_or_create_probe_session_id(probe_dir: &std::path::Path) -> String {
    let path = probe_dir.join(CLAUDE_PROBE_SESSION_ID_FILE);
    if let Ok(raw) = std::fs::read_to_string(&path) {
        let trimmed = raw.trim();
        if uuid::Uuid::parse_str(trimmed).is_ok() {
            return trimmed.to_ascii_lowercase();
        }
    }
    let id = uuid::Uuid::new_v4().to_string().to_ascii_lowercase();
    if let Err(err) = std::fs::write(&path, &id) {
        tracing::debug!(error = %err, "failed to persist Claude probe session id");
    }
    id
}

/// Claude treats `--session-id` as create-only when a local transcript JSONL
/// already exists for that id. Clear probe-dir jsonl leftovers before reuse.
fn cleanup_probe_session_jsonl(probe_dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(probe_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            let _ = std::fs::remove_file(&path);
        }
    }
}

fn claude_probe_launch_args(session_id: &str) -> Vec<String> {
    vec![
        "--setting-sources".to_string(),
        "user".to_string(),
        "--allowed-tools".to_string(),
        String::new(),
        "--session-id".to_string(),
        session_id.to_string(),
    ]
}

struct ClaudePtyProbeOptions {
    script: &'static str,
    timeout_secs: f64,
    idle_timeout_secs: Option<f64>,
    initial_delay_secs: f64,
    script_char_delay_secs: f64,
    script_line_delay_secs: f64,
    send_on_substring: Option<(&'static str, &'static str)>,
}

async fn run_claude_usage_pty_probe(
    claude_path: std::path::PathBuf,
    working_directory: std::path::PathBuf,
) -> Result<String, ProviderError> {
    run_claude_pty_probe(
        claude_path,
        working_directory,
        ClaudePtyProbeOptions {
            script: "/usage",
            timeout_secs: 20.0,
            idle_timeout_secs: Some(6.0),
            initial_delay_secs: 3.0,
            script_char_delay_secs: 0.04,
            script_line_delay_secs: 0.0,
            send_on_substring: None,
        },
    )
    .await
}

async fn run_claude_trust_preflight(
    claude_path: std::path::PathBuf,
    working_directory: std::path::PathBuf,
) -> Result<String, ProviderError> {
    run_claude_pty_probe(
        claude_path,
        working_directory,
        ClaudePtyProbeOptions {
            script: "",
            timeout_secs: 15.0,
            idle_timeout_secs: Some(4.0),
            initial_delay_secs: 0.6,
            script_char_delay_secs: 0.0,
            script_line_delay_secs: 0.0,
            send_on_substring: Some(("Enter", "\n/exit\n")),
        },
    )
    .await
}

fn resolve_claude_cli_path() -> Result<std::path::PathBuf, ProviderError> {
    which_claude().ok_or_else(|| {
        ProviderError::NotInstalled(
            "Claude CLI not found. Install from https://docs.claude.ai/claude-code".to_string(),
        )
    })
}

async fn fetch_claude_cli_usage_text(
    claude_path: std::path::PathBuf,
) -> Result<String, ProviderError> {
    let probe_dir = claude_usage_probe_dir()?;
    let combined = run_claude_usage_pty_probe(claude_path.clone(), probe_dir.clone()).await?;

    rerun_claude_usage_after_trust_prompt(claude_path, probe_dir, combined).await
}

async fn rerun_claude_usage_after_trust_prompt(
    claude_path: std::path::PathBuf,
    probe_dir: std::path::PathBuf,
    combined: String,
) -> Result<String, ProviderError> {
    if !is_workspace_trust_prompt(&strip_ansi(&combined).to_lowercase()) {
        return Ok(combined);
    }

    run_claude_trust_preflight(claude_path.clone(), probe_dir.clone()).await?;
    run_claude_usage_pty_probe(claude_path, probe_dir).await
}

fn claude_cli_error_from_output(output: &str) -> Option<ProviderError> {
    let lowered = output.to_lowercase();
    claude_cli_auth_error(&lowered).or_else(|| claude_cli_environment_error(&lowered))
}

fn claude_cli_auth_error(lowered: &str) -> Option<ProviderError> {
    if claude_output_requires_login(lowered) {
        return Some(ProviderError::AuthRequired);
    }
    if lowered.contains("token expired") || lowered.contains("token_expired") {
        return Some(ProviderError::OAuthExpired(
            "Token expired. Run `claude login` to refresh.".to_string(),
        ));
    }
    if lowered.contains("authentication_error") {
        return Some(ProviderError::OAuth(
            "Authentication error. Run `claude login`.".to_string(),
        ));
    }

    None
}

fn claude_output_requires_login(lowered: &str) -> bool {
    lowered.contains("not logged in") || lowered.contains("login required")
}

fn claude_cli_environment_error(lowered: &str) -> Option<ProviderError> {
    if lowered.contains("requires git-bash") {
        return Some(ProviderError::Other(
            "Claude CLI requires Git Bash on Windows. Install Git for Windows or set \
             CLAUDE_CODE_GIT_BASH_PATH to your bash.exe path."
                .to_string(),
        ));
    }
    if lowered.contains("running scripts is disabled") {
        return Some(ProviderError::Other(
            "Claude CLI could not start because PowerShell script execution is disabled. \
             Use claude.cmd or adjust the execution policy."
                .to_string(),
        ));
    }
    if lowered.contains("cannot run a document in the middle of a pipeline") {
        return Some(ProviderError::Other(
            "Claude CLI resolved to a Unix shell script on Windows. Reinstall Claude Code or \
             ensure claude.cmd is first on PATH."
                .to_string(),
        ));
    }

    None
}

/// Environment overrides for passive Claude CLI PTY probes.
fn claude_passive_probe_env(
    mut base: std::collections::HashMap<String, String>,
) -> std::collections::HashMap<String, String> {
    // Passive status/usage probes must not mutate or update the user's Claude CLI installation.
    base.insert("NO_COLOR".to_string(), "1".to_string());
    base.insert("DISABLE_AUTOUPDATER".to_string(), "1".to_string());
    base
}

async fn run_claude_pty_probe(
    claude_path: std::path::PathBuf,
    working_directory: std::path::PathBuf,
    probe: ClaudePtyProbeOptions,
) -> Result<String, ProviderError> {
    tokio::task::spawn_blocking(move || {
        cleanup_probe_session_jsonl(&working_directory);
        let session_id = load_or_create_probe_session_id(&working_directory);
        let env = claude_passive_probe_env(TtyCommandRunner::enriched_environment());

        let mut options = TtyCommandOptions::new()
            .with_timeout(probe.timeout_secs)
            .with_initial_delay(probe.initial_delay_secs)
            .with_script_char_delay(probe.script_char_delay_secs)
            .with_script_line_delay(probe.script_line_delay_secs)
            .with_working_directory(working_directory)
            .with_extra_args(claude_probe_launch_args(&session_id));
        if let Some(idle) = probe.idle_timeout_secs {
            options = options.with_idle_timeout(idle);
        }
        if let Some((trigger, keys)) = probe.send_on_substring {
            options = options.with_send_on_substring(trigger, keys);
        }
        options.env = env;

        TtyCommandRunner::new()
            .run(&claude_path.to_string_lossy(), probe.script, options)
            .map(|result| result.text)
    })
    .await
    .map_err(|e| ProviderError::Other(format!("Claude CLI probe failed: {}", e)))?
    .map_err(|e| match e {
        crate::cli::tty_runner::TtyCommandError::TimedOut => ProviderError::Timeout,
        other => ProviderError::Other(format!("Claude CLI failed: {}", other)),
    })
}

#[async_trait]
impl Provider for ClaudeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Claude
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto => self.fetch_via_auto(ctx).await,
            SourceMode::OAuth => self.fetch_via_oauth(ctx).await,
            SourceMode::Web => self.fetch_via_web(ctx).await,
            SourceMode::Cli => match self.fetch_via_cli(ctx).await {
                Ok(result) => Ok(result),
                Err(error) if should_fallback_from_claude_cli_error(&error) => {
                    tracing::debug!(
                        error = %error,
                        "Claude CLI usage probe failed with a fallback-safe error; trying OAuth"
                    );
                    self.fetch_via_oauth(ctx).await
                }
                Err(error) => Err(error),
            },
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![
            SourceMode::Auto,
            SourceMode::OAuth,
            SourceMode::Web,
            SourceMode::Cli,
        ]
    }

    fn supports_oauth(&self) -> bool {
        true
    }

    fn supports_web(&self) -> bool {
        true
    }

    fn supports_cli(&self) -> bool {
        true
    }

    fn detect_version(&self) -> Option<String> {
        detect_claude_version()
    }
    /// Claude's CLI-presence probe (`resolve_claude_cli_path`) raises
    /// `NotInstalled` when the `claude` binary itself is missing — an
    /// installation gap, not a credential problem — so it surfaces as an
    /// offline local runtime (matching the pre-backend classifier's
    /// treatment of CLI-presence failures). Message-scoped so any future
    /// credential-flavored `NotInstalled` keeps the default mapping.
    fn error_state_kind(&self, error: &ProviderError) -> crate::core::ProviderStateKind {
        match error {
            ProviderError::NotInstalled(msg) if msg.contains("CLI not found") => {
                crate::core::ProviderStateKind::LocalRuntimeOffline
            }
            _ => error.state_kind(),
        }
    }
}

impl ClaudeProvider {
    async fn fetch_via_auto(
        &self,
        ctx: &FetchContext,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let mut failures = Vec::new();

        if let Some(result) = self.try_auto_admin_api(ctx, &mut failures).await {
            return Ok(result);
        }

        if let Some(result) =
            record_auto_source(&mut failures, "Web", self.fetch_via_web(ctx).await)
        {
            return Ok(result);
        }

        // Upstream 0.50.1 #2516: track whether OAuth failed with a revocation.
        let oauth_result = self.fetch_via_oauth(ctx).await;
        let oauth_revoked = oauth_result
            .as_ref()
            .err()
            .is_some_and(is_oauth_revoked_error);
        if let Some(result) = record_auto_source(&mut failures, "OAuth", oauth_result) {
            return Ok(result);
        }

        // When OAuth was revoked (not just expired), reuse a cached CLI result
        // if still within the 15-minute TTL to avoid re-probing the CLI.
        if oauth_revoked && let Some(cached) = cached_cli_result() {
            tracing::debug!("Claude OAuth revoked; returning cached CLI result (15-min cache)");
            return Ok(cached);
        }

        if let Some(mut result) =
            record_auto_source(&mut failures, "CLI", self.fetch_via_cli(ctx).await)
        {
            // Without consent for reading Claude Code credentials, label the
            // CLI fallback as reduced fidelity.
            if !claude_code_consent() {
                result.source_label = "cli (reduced fidelity)".to_string();
            }
            // Cache the CLI result when OAuth was revoked so subsequent polls
            // within the TTL reuse it without re-probing.
            if oauth_revoked {
                cache_cli_result(result.clone());
            }
            return Ok(result);
        }

        // Upstream 0.50.1 #2516: when all live sources fail, keep the
        // last-known quota visible (stale) instead of blanking the UI.
        if let Some(cached) = cached_cli_result() {
            tracing::debug!("All Claude live sources failed; returning stale cached CLI result");
            return Ok(cached);
        }

        Err(claude_auto_fetch_error(failures))
    }

    async fn try_auto_admin_api(
        &self,
        ctx: &FetchContext,
        failures: &mut Vec<(&'static str, ProviderError)>,
    ) -> Option<ProviderFetchResult> {
        self.admin_fetcher
            .has_credentials(ctx)
            .then_some(async { self.fetch_via_admin_api(ctx).await })?
            .await
            .map_err(|error| failures.push(("Admin API", error)))
            .ok()
    }

    async fn fetch_via_oauth(
        &self,
        ctx: &FetchContext,
    ) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Attempting OAuth fetch for Claude");
        if let Some(token) = ctx
            .api_key
            .as_deref()
            .filter(|token| !token.trim().is_empty())
        {
            return self.oauth_fetcher.fetch_with_access_token(token).await;
        }
        self.oauth_fetcher.fetch().await
    }

    async fn fetch_via_admin_api(
        &self,
        ctx: &FetchContext,
    ) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Attempting Admin API fetch for Claude");
        self.admin_fetcher.fetch(ctx).await
    }

    async fn fetch_via_web(
        &self,
        ctx: &FetchContext,
    ) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Attempting Web API fetch for Claude");

        // Check for manual cookie header first
        if let Some(ref cookie_header) = ctx.manual_cookie_header {
            tracing::debug!("Using manual cookie header");
            return self
                .web_fetcher
                .fetch_with_cookie_header(cookie_header)
                .await;
        }

        // Otherwise, try to extract cookies from browser
        self.web_fetcher.fetch_with_cookies().await
    }

    async fn fetch_via_cli(
        &self,
        _ctx: &FetchContext,
    ) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Attempting CLI probe for Claude");

        let claude_path = resolve_claude_cli_path()?;
        let combined = fetch_claude_cli_usage_text(claude_path).await?;

        if let Some(error) = claude_cli_error_from_output(&combined) {
            return Err(error);
        }

        self.parse_cli_output(&combined)
    }

    /// Parse Claude CLI /usage output
    fn parse_cli_output(&self, output: &str) -> Result<ProviderFetchResult, ProviderError> {
        let clean = strip_ansi(output);
        let clean_lower = clean.to_lowercase();

        if clean.trim().is_empty() {
            return Err(ProviderError::Parse(
                "Empty output from Claude CLI".to_string(),
            ));
        }

        if is_non_interactive_slash_command_response(&clean_lower) {
            return Err(ProviderError::Other(
                "Claude CLI treated /usage as a normal prompt instead of opening the interactive usage screen. Use Auto, OAuth, or Web mode for Claude usage.".to_string(),
            ));
        }

        if is_cli_activity_stats_response(&clean_lower) {
            return Err(ProviderError::Other(
                "Claude CLI /usage opened, but this Claude version returned local activity stats instead of plan limit percentages. Use Auto, OAuth, or Web mode for Claude limits.".to_string(),
            ));
        }

        // Parse session percent: "X% used" or "X% left"
        let mut session_percent: Option<f64> = None;
        let mut weekly_percent: Option<f64> = None;

        // Look for "Current session" section
        if let Some(session_pct) = extract_percent_near_label(&clean, "current session") {
            session_percent = Some(session_pct);
        }

        // Look for "Current week" section
        if let Some(weekly_pct) = extract_percent_near_label(&clean, "current week (all models)")
            .or_else(|| extract_percent_near_label(&clean, "current week"))
        {
            weekly_percent = Some(weekly_pct);
        }

        // Fallback: collect all percentages in order
        if session_percent.is_none() {
            let all_percents = extract_all_percents(&clean);
            if !all_percents.is_empty() {
                session_percent = Some(all_percents[0]);
            }
            if all_percents.len() > 1 && weekly_percent.is_none() {
                weekly_percent = Some(all_percents[1]);
            }
        }

        if session_percent.is_none()
            && weekly_percent.is_none()
            && !is_exhausted_short_form(&clean_lower)
        {
            return Err(ProviderError::Parse(
                "Claude CLI did not return usage data".to_string(),
            ));
        }

        // Extract identity info
        let email = extract_email(&clean);
        let login_method = extract_login_method(&clean);

        // Extract reset times
        let session_reset = extract_reset_description(&clean, "current session");
        let weekly_reset = extract_reset_description(&clean, "current week (all models)")
            .or_else(|| extract_reset_description(&clean, "current week"));
        let short_form_reset = if is_exhausted_short_form(&clean_lower) {
            extract_inline_reset_description(&clean)
        } else {
            None
        };
        let session_reset = session_reset.or(short_form_reset);
        let now = Utc::now();
        let scoped_weekly_limits = extract_cli_scoped_weekly_limits(&clean, now);

        if session_percent.is_none() && is_exhausted_short_form(&clean_lower) {
            session_percent = Some(100.0);
        }

        // Build usage snapshot
        let session_used = session_percent.unwrap_or(0.0);
        let primary = RateWindow::with_details(
            session_used,
            Some(300), // 5 hour session window
            session_reset
                .as_deref()
                .and_then(|reset| parse_claude_reset_date(reset, now, Some(300))),
            session_reset,
        );

        let mut usage = UsageSnapshot::new(primary);

        if let Some(weekly_used) = weekly_percent {
            let secondary = RateWindow::with_details(
                weekly_used,
                Some(10080), // weekly (7 * 24 * 60)
                weekly_reset
                    .as_deref()
                    .and_then(|reset| parse_claude_reset_date(reset, now, Some(10080))),
                weekly_reset,
            );
            usage = usage.with_secondary(secondary);
        }

        for limit in scoped_weekly_limits {
            usage.extra_rate_windows.push(limit);
        }

        if let Some(method) = login_method {
            usage = usage.with_login_method(&method);
        } else {
            usage = usage.with_login_method("Claude (CLI)");
        }

        if let Some(email) = email {
            usage = usage.with_email(&email);
        }

        Ok(ProviderFetchResult::new(usage, "cli"))
    }
}

fn record_auto_source(
    failures: &mut Vec<(&'static str, ProviderError)>,
    source: &'static str,
    result: Result<ProviderFetchResult, ProviderError>,
) -> Option<ProviderFetchResult> {
    result.map_err(|error| failures.push((source, error))).ok()
}

fn claude_auto_fetch_error(failures: Vec<(&'static str, ProviderError)>) -> ProviderError {
    let summary = failures
        .into_iter()
        .map(|(source, error)| format!("{source}: {error}"))
        .collect::<Vec<_>>()
        .join("; ");

    ProviderError::Other(format!(
        "Claude usage failed from all configured sources. {summary}"
    ))
}

fn should_fallback_from_claude_cli_error(error: &ProviderError) -> bool {
    match error {
        ProviderError::Parse(message) => {
            matches!(
                message.as_str(),
                "Claude CLI did not return usage data" | "Empty output from Claude CLI"
            )
        }
        ProviderError::Other(message) => {
            message.contains("returned local activity stats")
                || message.contains("treated /usage as a normal prompt")
        }
        _ => false,
    }
}

/// Try to find the claude CLI binary
fn which_claude() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        let candidates = [
            // Direct install
            dirs::data_local_dir().map(|p| p.join("Programs").join("claude").join("claude.exe")),
            // npm global (AppData\Roaming\npm)
            dirs::data_local_dir().map(|p| p.join("npm").join("claude.cmd")),
            dirs::home_dir().map(|h| {
                h.join("AppData")
                    .join("Roaming")
                    .join("npm")
                    .join("claude.cmd")
            }),
            // npm global alternate (~\.npm-global)
            dirs::home_dir().map(|h| h.join(".npm-global").join("claude.cmd")),
            // Volta managed
            dirs::data_local_dir().map(|p| {
                p.join("Volta")
                    .join("tools")
                    .join("image")
                    .join("packages")
                    .join("@anthropic-ai")
                    .join("claude-code")
                    .join("bin")
                    .join("claude.cmd")
            }),
            // fnm managed (via shim)
            dirs::data_local_dir().map(|p| p.join("fnm_multishells").join("claude.cmd")),
            // PATH lookup
            find_windows_claude_in_path(),
        ];

        candidates.into_iter().flatten().find(|p| p.exists())
    }

    #[cfg(not(windows))]
    {
        which::which("claude").ok()
    }
}

#[cfg(windows)]
fn find_windows_claude_in_path() -> Option<std::path::PathBuf> {
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut command = StdCommand::new("where");
    command
        .arg("claude")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    let output = command.output().ok()?;

    if !output.status.success() {
        return None;
    }

    let mut matches: Vec<_> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(std::path::PathBuf::from)
        .collect();

    matches.sort_by_key(|path| {
        match path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref()
        {
            Some("cmd") => 0,
            Some("bat") => 1,
            Some("exe") => 2,
            _ => 3,
        }
    });

    matches.into_iter().find(|path| path.exists())
}

/// Detect the version of the claude CLI
fn detect_claude_version() -> Option<String> {
    let claude_path = which_claude()?;

    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut cmd = std::process::Command::new(claude_path);
    cmd.args(["--version"]);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let output = cmd.output().ok()?;

    if output.status.success() {
        let version_str = String::from_utf8_lossy(&output.stdout);
        super::extract_semver(&version_str)
    } else {
        None
    }
}

/// Strip ANSI escape codes from text
fn strip_ansi(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1B' {
            // Skip CSI sequences: ESC[...letter
            if chars.peek() == Some(&'[') {
                chars.next();
                let mut final_char = None;
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next.is_ascii_alphabetic() {
                        final_char = Some(next);
                        break;
                    }
                }
                if final_char == Some('C') {
                    result.push(' ');
                }
            // Skip OSC sequences: ESC]...BEL
            } else if chars.peek() == Some(&']') {
                for next in chars.by_ref() {
                    if next == '\x07' || next == '\\' {
                        break;
                    }
                }
            }
        } else {
            result.push(c);
        }
    }

    result
}

fn is_non_interactive_slash_command_response(text: &str) -> bool {
    let mentions_usage_and_exit = text.contains("/usage") && text.contains("/exit");
    let says_entered_commands =
        text.contains("i see you've entered") || text.contains("you've entered two slash commands");
    let says_no_slash_command = text.contains("available custom slash commands")
        && text.contains("don't see these commands");
    let says_usage_is_cli_only = text
        .contains("token usage and statistics are typically displayed by the cli interface")
        || text.contains("i don't have direct access to those metrics");

    mentions_usage_and_exit
        && (says_entered_commands || says_no_slash_command || says_usage_is_cli_only)
}

fn is_workspace_trust_prompt(text: &str) -> bool {
    text.contains("quick safety check")
        && text.contains("trust this folder")
        && text.contains("yes, i trust this folder")
}

fn is_cli_activity_stats_response(text: &str) -> bool {
    let has_activity_overview = text.contains("favorite model:") || text.contains("total tokens:");
    let has_session_cost_summary =
        text.contains("total duration") && text.contains("usage:") && text.contains("cache read");

    has_activity_overview || has_session_cost_summary
}

/// Extract percentage near a label (e.g., "Current session")
/// Returns the percentage as "used" (not remaining)
fn extract_percent_near_label(text: &str, label: &str) -> Option<f64> {
    let label_normalized = normalized_for_label_search(label);
    let lines: Vec<&str> = text.lines().collect();

    // Find the line containing the label
    for (idx, line) in lines.iter().enumerate() {
        if normalized_for_label_search(line).contains(&label_normalized) {
            // Look in the next few lines for a percentage
            for (offset, next_line) in lines.iter().skip(idx).take(12).enumerate() {
                if offset > 0 && starts_next_usage_section(next_line, &label_normalized) {
                    break;
                }
                if let Some(pct) = parse_percent_line(next_line) {
                    return Some(pct);
                }
            }
        }
    }

    None
}

/// Extract all percentages from text in order
fn extract_all_percents(text: &str) -> Vec<f64> {
    let re = match Regex::new(
        r"(\d{1,3}(?:\.\d+)?)\s*%\s*(used|spent|consumed|left|remaining|available)",
    ) {
        Ok(r) => r,
        Err(_) => return vec![],
    };

    let mut results = Vec::new();
    let lower = text.to_lowercase();

    for caps in re.captures_iter(&lower) {
        if let (Some(val_match), Some(kind_match)) = (caps.get(1), caps.get(2))
            && let Ok(val) = val_match.as_str().parse::<f64>()
        {
            let kind = kind_match.as_str();
            let used = if matches!(kind, "left" | "remaining" | "available") {
                (100.0 - val).max(0.0)
            } else {
                val.min(100.0)
            };
            results.push(used);
        }
    }

    results
}

fn is_exhausted_short_form(clean_lower: &str) -> bool {
    clean_lower.contains("out of extra usage") || clean_lower.contains("hit your limit")
}

/// Extract email address from text
fn extract_email(text: &str) -> Option<String> {
    // Try explicit patterns first
    let patterns = [
        r"Account:\s*([^\s@]+@[^\s@]+\.[^\s]+)",
        r"Email:\s*([^\s@]+@[^\s@]+\.[^\s]+)",
        r"([A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,})",
    ];

    for pattern in patterns {
        if let Ok(re) = Regex::new(pattern)
            && let Some(caps) = re.captures(text)
            && let Some(m) = caps.get(1)
        {
            return Some(m.as_str().trim().to_string());
        }
    }

    None
}

/// Extract login method / plan name from text
fn extract_login_method(text: &str) -> Option<String> {
    // Look for explicit "Login method:" line
    if let Ok(re) = Regex::new(r"(?i)login\s+method:\s*(.+)")
        && let Some(caps) = re.captures(text)
        && let Some(m) = caps.get(1)
    {
        let method = m.as_str().trim();
        if !method.is_empty() {
            return Some(clean_plan_name(method));
        }
    }

    // Look for "Claude <plan>" patterns
    if let Ok(re) = Regex::new(r"(?i)(claude\s+(?:max|pro|ultra|team|free)[a-z0-9\s._-]*)")
        && let Some(caps) = re.captures(text)
        && let Some(m) = caps.get(1)
    {
        let plan = m.as_str().trim();
        if !plan.to_lowercase().contains("code") {
            return Some(clean_plan_name(plan));
        }
    }

    None
}

/// Extract reset description near a label
fn extract_reset_description(text: &str, label: &str) -> Option<String> {
    let label_normalized = normalized_for_label_search(label);
    let lines: Vec<&str> = text.lines().collect();

    for (idx, line) in lines.iter().enumerate() {
        if normalized_for_label_search(line).contains(&label_normalized) {
            // Look in the next few lines for "Resets"
            for (offset, next_line) in lines.iter().skip(idx).take(14).enumerate() {
                if offset > 0 && starts_next_usage_section(next_line, &label_normalized) {
                    break;
                }
                let lower = next_line.to_lowercase();
                if lower.contains("resets") {
                    // Extract the reset info
                    if let Some(pos) = lower.find("resets") {
                        let reset_part = &next_line[pos..];
                        return Some(reset_part.trim().to_string());
                    }
                }
            }
        }
    }

    None
}

/// Extract a "resets ..." suffix from a short single-line status.
fn extract_inline_reset_description(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let pos = lower.find("resets")?;
    Some(text[pos..].trim().to_string())
}

/// Clean up a plan name by removing ANSI codes and extra whitespace
fn clean_plan_name(text: &str) -> String {
    let cleaned = strip_ansi(text);
    // Remove bracketed codes like [22m
    let re = Regex::new(r"\[\d+m").unwrap_or_else(|_| Regex::new(".^").unwrap());
    let result = re.replace_all(&cleaned, "");
    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn passive_probe_env_disables_autoupdater_and_color() {
        let env = claude_passive_probe_env(HashMap::new());
        assert_eq!(
            env.get("DISABLE_AUTOUPDATER").map(String::as_str),
            Some("1")
        );
        assert_eq!(env.get("NO_COLOR").map(String::as_str), Some("1"));
    }

    #[test]
    fn probe_session_id_is_reused_from_probe_directory() {
        let dir = tempfile::tempdir().unwrap();
        let first = load_or_create_probe_session_id(dir.path());
        let second = load_or_create_probe_session_id(dir.path());
        assert_eq!(first, second);
        assert!(uuid::Uuid::parse_str(&first).is_ok());
        let args = claude_probe_launch_args(&first);
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--session-id" && w[1] == first)
        );
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--allowed-tools" && w[1].is_empty())
        );
    }

    #[test]
    fn probe_session_jsonl_cleanup_removes_transcript_files() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("session.jsonl");
        std::fs::write(&jsonl, "{}").unwrap();
        std::fs::write(dir.path().join("keep.txt"), "x").unwrap();
        cleanup_probe_session_jsonl(dir.path());
        assert!(!jsonl.exists());
        assert!(dir.path().join("keep.txt").exists());
    }

    #[test]
    fn parses_current_cli_usage_screen() {
        let provider = ClaudeProvider::new();
        let output = r#"
Status   Config   Usage

  Current session
  ██████████████████████████████████████████████████ 100% used
  Resets 12pm (America/Bogota)

  Current week (all models)
  ████████████████████████▌                          49% used
  Resets Apr 3, 2pm (America/Bogota)

  Extra usage
  ██▍                                                4% used
  $3.31 / $70.00 spent · Resets Apr 1 (America/Bogota)
"#;

        let result = provider.parse_cli_output(output).expect("should parse");

        assert_eq!(result.source_label, "cli");
        assert_eq!(result.usage.primary.used_percent, 100.0);
        assert_eq!(
            result.usage.primary.reset_description.as_deref(),
            Some("Resets 12pm (America/Bogota)")
        );

        let weekly = result
            .usage
            .secondary
            .expect("weekly usage should be present");
        assert_eq!(weekly.used_percent, 49.0);
        assert_eq!(
            weekly.reset_description.as_deref(),
            Some("Resets Apr 3, 2pm (America/Bogota)")
        );
    }

    #[test]
    fn parses_exhausted_short_form_as_full_session_usage() {
        let provider = ClaudeProvider::new();
        let output = "You're out of extra usage · resets 12pm (America/Bogota)";

        let result = provider.parse_cli_output(output).expect("should parse");

        assert_eq!(result.usage.primary.used_percent, 100.0);
        assert_eq!(
            result.usage.primary.reset_description.as_deref(),
            Some("resets 12pm (America/Bogota)")
        );
    }

    #[test]
    fn parses_hit_limit_short_form_as_full_session_usage() {
        let provider = ClaudeProvider::new();
        let output = "You've hit your limit \u{00b7} resets 3:20pm (Asia/Shanghai)";

        let result = provider.parse_cli_output(output).expect("should parse");

        assert_eq!(result.usage.primary.used_percent, 100.0);
        assert_eq!(
            result.usage.primary.reset_description.as_deref(),
            Some("resets 3:20pm (Asia/Shanghai)")
        );
    }

    #[test]
    fn parses_remaining_available_and_decimal_percentages() {
        let provider = ClaudeProvider::new();
        let output = r#"
Status   Config   Usage

  Current session
  12.5% remaining
  Resets 8pm

  Current week (all models)
  4% available
  Resets Apr 4, 2pm

  Current week (Sonnet only)
  1% consumed
"#;

        let result = provider.parse_cli_output(output).expect("should parse");

        assert_eq!(result.usage.primary.used_percent, 87.5);
        assert_eq!(
            result.usage.primary.reset_description.as_deref(),
            Some("Resets 8pm")
        );

        let weekly = result
            .usage
            .secondary
            .expect("weekly usage should be present");
        assert_eq!(weekly.used_percent, 96.0);
        assert_eq!(
            weekly.reset_description.as_deref(),
            Some("Resets Apr 4, 2pm")
        );

        let sonnet = result
            .usage
            .extra_rate_windows
            .iter()
            .find(|window| window.id == "claude-weekly-scoped-sonnet")
            .expect("sonnet usage should be present");
        assert_eq!(sonnet.window.used_percent, 1.0);
    }

    #[test]
    fn parses_all_cli_model_scoped_weekly_limits() {
        let provider = ClaudeProvider::new();
        let output = r#"
Current session
10% used
Resets 12pm (America/Bogota)

Current week (all models)
20% used
Resets Apr 3, 2pm (America/Bogota)

Current week (Sonnet only)
30% used
Resets Apr 4, 2pm (America/Bogota)

Current week (Opus only)
40% used
Resets Apr 5, 2pm (America/Bogota)
"#;

        let result = provider.parse_cli_output(output).expect("should parse");

        assert_eq!(result.usage.extra_rate_windows.len(), 2);
        assert_eq!(
            result.usage.extra_rate_windows[0].id,
            "claude-weekly-scoped-sonnet"
        );
        assert_eq!(result.usage.extra_rate_windows[0].title, "Sonnet only");
        assert_eq!(result.usage.extra_rate_windows[0].window.used_percent, 30.0);
        assert_eq!(
            result.usage.extra_rate_windows[1].id,
            "claude-weekly-scoped-opus"
        );
        assert!(result.usage.model_specific.is_none());
    }

    #[test]
    fn scoped_weekly_parser_handles_non_ascii_labels_and_reset_prefixes() {
        let now = "2026-04-02T18:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let limits = extract_cli_scoped_weekly_limits(
            "Current week (A€€)\n10% used\nİResets Apr 3 at 2pm (America/Bogota)",
            now,
        );

        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].title, "A€€");
        assert_eq!(
            limits[0].window.resets_at,
            Some("2026-04-03T19:00:00Z".parse().unwrap())
        );
    }

    #[test]
    fn resolves_cli_reset_occurrences_in_the_reported_timezone() {
        let now = "2026-04-02T18:00:00Z".parse::<DateTime<Utc>>().unwrap();

        assert_eq!(
            parse_claude_reset_date("Resets Apr 3, 2027, 2pm (America/Bogota)", now, None),
            Some("2027-04-03T19:00:00Z".parse().unwrap())
        );
        assert_eq!(
            parse_claude_reset_date("Resets Apr 3, 2pm (America/Bogota)", now, None),
            Some("2026-04-03T19:00:00Z".parse().unwrap())
        );
        assert_eq!(
            parse_claude_reset_date("Resets 12pm (America/Bogota)", now, None),
            Some("2026-04-03T17:00:00Z".parse().unwrap())
        );
        assert_eq!(
            parse_claude_reset_date("ResetsApr3at2pm(America/Bogota)", now, None),
            Some("2026-04-03T19:00:00Z".parse().unwrap())
        );
    }

    #[test]
    fn timezone_less_resets_use_the_supplied_system_zone() {
        let now = "2026-03-07T18:00:00Z".parse::<DateTime<Utc>>().unwrap();

        assert_eq!(
            parse_claude_reset_date_in_system_zone(
                "Resets Mar 8 at 3:30am",
                now,
                None,
                "America/New_York".parse().unwrap(),
            ),
            Some("2026-03-08T07:30:00Z".parse().unwrap())
        );
        assert_eq!(
            parse_claude_reset_date_in_system_zone(
                "Resets Mar 8 at 3:30am (America/Los_Angeles)",
                now,
                None,
                "America/New_York".parse().unwrap(),
            ),
            Some("2026-03-08T10:30:00Z".parse().unwrap())
        );
    }

    #[test]
    fn parses_compact_usage_screen() {
        let provider = ClaudeProvider::new();
        let output = r#"
Settings:StatusConfigUsage(tabtocycle)
Loadingusagedata...
Currentsession
6%used
Resets4:29am(Asia/Calcutta)
Currentweek(allmodels)
4%used
ResetsFeb12at1:29pm(Asia/Calcutta)
Currentweek(Sonnetonly)
1%used
ResetsFeb12at1:29pm(Asia/Calcutta)
"#;

        let result = provider.parse_cli_output(output).expect("should parse");

        assert_eq!(result.usage.primary.used_percent, 6.0);
        assert_eq!(
            result.usage.primary.reset_description.as_deref(),
            Some("Resets4:29am(Asia/Calcutta)")
        );
        assert_eq!(
            result
                .usage
                .secondary
                .expect("weekly usage should be present")
                .used_percent,
            4.0
        );
        let sonnet = result
            .usage
            .extra_rate_windows
            .iter()
            .find(|window| window.id == "claude-weekly-scoped-sonnet")
            .expect("sonnet usage should be present");
        assert_eq!(result.usage.extra_rate_windows.len(), 1);
        assert_eq!(sonnet.title, "Sonnet only");
        assert_eq!(sonnet.window.used_percent, 1.0);
    }

    #[test]
    fn does_not_promote_weekly_reset_to_session() {
        let provider = ClaudeProvider::new();
        let output = r#"
Current session
17% used
Current week (all models)
4% used
Resets Dec 24 at 3:59pm (Europe/Paris)
"#;

        let result = provider.parse_cli_output(output).expect("should parse");

        assert_eq!(result.usage.primary.used_percent, 17.0);
        assert_eq!(result.usage.primary.reset_description, None);
        assert_eq!(
            result
                .usage
                .secondary
                .expect("weekly usage should be present")
                .reset_description
                .as_deref(),
            Some("Resets Dec 24 at 3:59pm (Europe/Paris)")
        );
    }

    #[test]
    fn rejects_cli_output_without_usage_markers() {
        let provider = ClaudeProvider::new();
        let output = "Claude Code on Windows requires git-bash.";

        let err = provider
            .parse_cli_output(output)
            .expect_err("should reject non-usage output");

        assert!(matches!(err, ProviderError::Parse(_)));
        assert_eq!(
            err.to_string(),
            "Parse error: Claude CLI did not return usage data"
        );
    }

    #[test]
    fn cli_parse_usage_error_can_fallback_to_oauth() {
        let err = ProviderError::Parse("Claude CLI did not return usage data".to_string());

        assert!(should_fallback_from_claude_cli_error(&err));
    }

    #[test]
    fn cli_auth_error_does_not_fallback_to_oauth() {
        assert!(!should_fallback_from_claude_cli_error(
            &ProviderError::AuthRequired
        ));
    }

    #[test]
    fn auto_fetch_error_keeps_all_source_failures() {
        let err = claude_auto_fetch_error(vec![
            ("OAuth", ProviderError::OAuth("token expired".to_string())),
            ("Web", ProviderError::NoCookies),
            (
                "CLI",
                ProviderError::Parse("Empty output from Claude CLI".to_string()),
            ),
        ]);

        assert_eq!(
            err.to_string(),
            "Claude usage failed from all configured sources. OAuth: OAuth error: token expired; Web: No cookies available for web API; CLI: Parse error: Empty output from Claude CLI"
        );
    }

    #[test]
    fn rejects_claude_2_1_non_interactive_slash_response() {
        let provider = ClaudeProvider::new();
        let output = r#"
I see you've entered `/usage` and `/exit`.

**Usage**: Token usage and statistics are typically displayed by the CLI interface itself. I don't have direct access to those metrics through my available tools.

**Exit**: I'll end the session here. Goodbye!
"#;

        let err = provider
            .parse_cli_output(output)
            .expect_err("should reject non-interactive slash command response");

        assert!(matches!(err, ProviderError::Other(_)));
        assert_eq!(
            err.to_string(),
            "Claude CLI treated /usage as a normal prompt instead of opening the interactive usage screen. Use Auto, OAuth, or Web mode for Claude usage."
        );
    }

    #[test]
    fn rejects_legacy_non_interactive_slash_response() {
        let provider = ClaudeProvider::new();
        let output = r#"
I see you've entered two slash commands:

1. `/usage` - This appears to be a request to check usage information
2. `/exit` - This appears to be a request to exit

However, looking at the available custom slash commands, I don't see these commands defined.
"#;

        let err = provider
            .parse_cli_output(output)
            .expect_err("should reject non-interactive slash command response");

        assert!(matches!(err, ProviderError::Other(_)));
    }

    #[test]
    fn rejects_cli_activity_stats_without_plan_limits() {
        let provider = ClaudeProvider::new();
        let output = r#"
❯ /usage

Status   Config   Usage   Stats

Overview  Models

Favorite model: glm-4.6        Total tokens: 263.3k
Sessions: 6                    Longest session: 18s
Active days: 2/10              Longest streak: 1 day
"#;

        let err = provider
            .parse_cli_output(output)
            .expect_err("should reject local activity stats");

        assert!(matches!(err, ProviderError::Other(_)));
        assert_eq!(
            err.to_string(),
            "Claude CLI /usage opened, but this Claude version returned local activity stats instead of plan limit percentages. Use Auto, OAuth, or Web mode for Claude limits."
        );
    }

    #[test]
    fn rejects_ansi_spaced_cli_activity_stats_without_plan_limits() {
        let provider = ClaudeProvider::new();
        let output = "\x1b[2CTotal\x1b[1Ccost:\x1b[12C$0.0000\n\
                      \x1b[2CTotal\x1b[1Cduration\x1b[1C(API):\x1b[2C0s\n\
                      \x1b[2CUsage:\x1b[17C0\x1b[1Cinput,\x1b[1C0\x1b[1Coutput,\x1b[1C0\x1b[1Ccache\x1b[1Cread";

        let err = provider
            .parse_cli_output(output)
            .expect_err("should reject ANSI-spaced local activity stats");

        assert!(matches!(err, ProviderError::Other(_)));
    }

    // ── Upstream 0.50.1 #2516: revoked vs missing OAuth ────────────────────────

    #[test]
    fn oauth_revoked_error_is_detected() {
        assert!(is_oauth_revoked_error(&ProviderError::OAuthRevoked(
            "revoked".to_string()
        )));
        assert!(!is_oauth_revoked_error(&ProviderError::OAuth(
            "expired".to_string()
        )));
        assert!(!is_oauth_revoked_error(&ProviderError::AuthRequired));
    }

    #[test]
    fn cli_result_cache_round_trips() {
        let result = ProviderFetchResult::new(UsageSnapshot::new(RateWindow::new(42.0)), "cli");
        cache_cli_result(result.clone());
        let cached = cached_cli_result().expect("cached result within TTL");
        assert!((cached.usage.primary.used_percent - 42.0).abs() < 0.01);
        assert_eq!(cached.source_label, "cli");
    }
    #[test]
    fn cli_presence_maps_to_local_runtime_offline() {
        assert_eq!(
            ClaudeProvider::new().error_state_kind(&ProviderError::NotInstalled(
                "Claude CLI not found. Install from https://docs.claude.ai/claude-code".to_string(),
            )),
            crate::core::ProviderStateKind::LocalRuntimeOffline
        );
        // Other error kinds keep their default classification.
        assert_eq!(
            ClaudeProvider::new().error_state_kind(&ProviderError::AuthRequired),
            crate::core::ProviderStateKind::NeedsAuthentication
        );
    }
}
