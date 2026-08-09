//! Claude OAuth implementation
//!
//! Loads OAuth credentials from Claude CLI and fetches usage from the API.

use chrono::{DateTime, Utc};
use reqwest::Client;
use reqwest::header::{HeaderValue, RETRY_AFTER};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::core::{NamedRateWindow, ProviderError, ProviderFetchResult, RateWindow, UsageSnapshot};

mod credentials_store;
mod refresh;

/// OAuth credentials from Claude CLI
#[derive(Debug, Clone)]
pub struct ClaudeOAuthCredentials {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub scopes: Vec<String>,
    pub rate_limit_tier: Option<String>,
}

impl ClaudeOAuthCredentials {
    /// Check if the token is expired
    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            // Consider expired if within 5 minutes of expiry
            expires_at <= Utc::now() + chrono::Duration::minutes(5)
        } else {
            // No expiry info = don't assume expired, try it
            false
        }
    }

    /// Check if the credentials have a specific scope
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

/// OAuth usage response from Claude API
#[derive(Debug, Deserialize)]
pub struct OAuthUsageResponse {
    #[serde(rename = "fiveHour", alias = "five_hour")]
    pub five_hour: Option<UsageWindow>,

    #[serde(rename = "sevenDay", alias = "seven_day")]
    pub seven_day: Option<UsageWindow>,

    #[serde(rename = "sevenDaySonnet", alias = "seven_day_sonnet")]
    pub seven_day_sonnet: Option<UsageWindow>,

    #[serde(rename = "sevenDayOpus", alias = "seven_day_opus")]
    pub seven_day_opus: Option<UsageWindow>,

    #[serde(
        rename = "sevenDayDesign",
        alias = "seven_day_design",
        alias = "seven_day_oauth_apps"
    )]
    pub seven_day_design: Option<UsageWindow>,

    #[serde(
        rename = "sevenDayRoutines",
        alias = "seven_day_routines",
        alias = "seven_day_omelette"
    )]
    pub seven_day_routines: Option<UsageWindow>,

    #[serde(rename = "extraUsage", alias = "extra_usage")]
    pub extra_usage: Option<ExtraUsage>,

    #[serde(default)]
    limits: Vec<super::scoped_weekly::ScopedWeeklyLimit>,
}

/// A usage window from the OAuth API
#[derive(Debug, Deserialize)]
pub struct UsageWindow {
    pub utilization: Option<f64>,

    #[serde(rename = "resetsAt", alias = "resets_at")]
    pub resets_at: Option<String>,
}

/// Extra usage (credits) info
#[derive(Debug, Deserialize)]
pub struct ExtraUsage {
    #[serde(rename = "isEnabled", alias = "is_enabled")]
    pub is_enabled: Option<bool>,

    #[serde(rename = "usedCredits", alias = "used_credits")]
    pub used_credits: Option<f64>,

    #[serde(rename = "monthlyLimit", alias = "monthly_limit")]
    pub monthly_limit: Option<f64>,

    pub currency: Option<String>,
}

/// Claude OAuth fetcher
pub struct ClaudeOAuthFetcher {
    client: Client,
}

static RATE_LIMIT_BACKOFF_UNTIL: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();

// ── Refresh-token backoff (upstream 0.48.0 #2650) ────────────────────────────
//
// On Windows the Claude Code credential file is readable, so the macOS
// "touch completes but the refreshed credential is unreadable" state has no
// equivalent; the matching *provably-unrecoverable-by-retry* state here is the
// refresh endpoint itself rejecting the stored refresh token with
// `invalid_grant`. Retrying the identical grant can never succeed → the
// terminal gate stays blocked *indefinitely* and only clears when the
// credential file changes (the CLI re-auth rotates the refresh token) or a
// refresh succeeds. Transient failures (network, 5xx, 403, non-grant 4xx)
// use a flat 5-minute cooldown: a retry can still heal those.
const TRANSIENT_REFRESH_BACKOFF: Duration = Duration::from_secs(5 * 60);

struct RefreshBackoffEntry {
    /// When transient cooldown expires. Terminal entries never expire on a
    /// timer; this is `None` for terminal gates.
    until: Option<Instant>,
    kind: refresh::RefreshFailureKind,
    /// The refresh token observed at failure time. A subsequent poll that
    /// sees a different refresh token (CLI re-auth) clears a terminal gate.
    fingerprint: Option<String>,
}

static REFRESH_BACKOFF: LazyLock<
    Mutex<HashMap<credentials_store::CredentialSource, RefreshBackoffEntry>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Returns the active backoff kind for `source`, or `None` if it has expired
/// or been cleared by a credential change. `current_refresh_token` is the
/// token the caller is about to retry with; a terminal gate whose stored
/// fingerprint differs from it is cleared (the grant changed → retry allowed).
fn active_refresh_backoff(
    source: &credentials_store::CredentialSource,
    now: Instant,
    current_refresh_token: Option<&str>,
) -> Option<refresh::RefreshFailureKind> {
    let mut guard = REFRESH_BACKOFF.lock().ok()?;
    let entry = guard.get(source)?;
    match entry.kind {
        refresh::RefreshFailureKind::Terminal => {
            // Indefinite gate: only a credential change (different refresh
            // token) or an explicit success clears it.
            if let Some(fp) = &entry.fingerprint
                && current_refresh_token != Some(fp.as_str())
            {
                guard.remove(source);
                return None;
            }
            Some(entry.kind)
        }
        refresh::RefreshFailureKind::Transient => {
            if entry.until.is_some_and(|until| until <= now) {
                guard.remove(source);
                return None;
            }
            Some(entry.kind)
        }
    }
}

fn record_refresh_backoff(
    source: &credentials_store::CredentialSource,
    kind: refresh::RefreshFailureKind,
    now: Instant,
    current_refresh_token: Option<&str>,
) {
    let (until, fingerprint) = match kind {
        refresh::RefreshFailureKind::Terminal => (
            // Terminal gates do not expire on a timer.
            None,
            current_refresh_token.map(str::to_string),
        ),
        refresh::RefreshFailureKind::Transient => (Some(now + TRANSIENT_REFRESH_BACKOFF), None),
    };
    if let Ok(mut guard) = REFRESH_BACKOFF.lock() {
        guard.insert(
            source.clone(),
            RefreshBackoffEntry {
                until,
                kind,
                fingerprint,
            },
        );
    }
}

fn clear_refresh_backoff(source: &credentials_store::CredentialSource) {
    if let Ok(mut guard) = REFRESH_BACKOFF.lock() {
        guard.remove(source);
    }
}

/// User-facing message when refresh outcome is *terminal*: the stored refresh
/// token was rejected, so no amount of retrying refreshes the session. No
/// "then retry" tail — upstream dropped the same advice because refreshing
/// Claude Code's own credential store cannot heal this state.
fn terminal_refresh_message() -> String {
    "Claude OAuth session expired and its stored refresh token was rejected by the \
     server. Run `claude login` to re-authenticate."
        .to_string()
}

/// User-facing message while a transient refresh failure is cooling down.
fn refresh_cooldown_message() -> String {
    "Claude OAuth token expired and token refresh is cooling down after a failed \
     attempt. Please retry shortly, or run `claude login`."
        .to_string()
}

impl ClaudeOAuthFetcher {
    const USAGE_URL: &'static str = "https://api.anthropic.com/api/oauth/usage";
    const DEFAULT_RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(5 * 60);

    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// Load credentials and fetch usage, transparently refreshing an expired
    /// OAuth token first (like the Claude CLI does) so the panel stays green
    /// without the user having to re-run `claude`.
    pub async fn fetch(&self) -> Result<ProviderFetchResult, ProviderError> {
        let (credentials, source) = credentials_store::load_credentials()?;
        let (credentials, refresh_outcome) =
            self.ensure_fresh_credentials(credentials, source).await;
        // Still-expired credentials with a terminal/gated refresh state get the
        // honest message instead of a generic "expired" error (or another
        // doomed API call).
        if credentials.is_expired()
            && let Some(message) = refresh_outcome
        {
            return Err(ProviderError::OAuth(message));
        }
        self.fetch_with_credentials(credentials).await
    }

    /// Fetch usage with an explicit OAuth access token.
    pub async fn fetch_with_access_token(
        &self,
        access_token: &str,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let access_token = access_token.trim();
        if access_token.is_empty() {
            return Err(ProviderError::OAuth(
                "Claude OAuth access token is empty.".to_string(),
            ));
        }

        let credentials = ClaudeOAuthCredentials {
            access_token: access_token.to_string(),
            refresh_token: None,
            expires_at: None,
            scopes: vec!["user:profile".to_string()],
            rate_limit_tier: None,
        };

        self.fetch_with_credentials(credentials).await
    }

    async fn fetch_with_credentials(
        &self,
        credentials: ClaudeOAuthCredentials,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let usage_response = self.fetch_usage(&credentials).await?;
        let usage = self.build_usage_snapshot(&usage_response, &credentials);
        Ok(ProviderFetchResult::new(usage, "oauth"))
    }

    /// If the token is expired (or about to expire), refresh it using the
    /// refresh token and persist the new token back to `.credentials.json`.
    /// Best-effort: on any failure the original credentials are returned so the
    /// caller falls back to the existing "expired" handling. The second return
    /// value carries a user-facing message when the refresh outcome is gated
    /// (cooldown) or terminal (#2650) and the credentials remain expired.
    async fn ensure_fresh_credentials(
        &self,
        mut credentials: ClaudeOAuthCredentials,
        source: credentials_store::CredentialSource,
    ) -> (ClaudeOAuthCredentials, Option<String>) {
        // Prefer an in-memory refreshed token if it is fresher than what we just
        // read from disk (covers a prior persist that failed to write). Scoped
        // to this credential's own source so a refresh cached for one source
        // (e.g. the credentials file) never shadows another (e.g. an
        // environment-provided token).
        if let Some(cached) = credentials_store::cached_refreshed_if_fresher(&source, &credentials)
        {
            credentials = cached;
        }

        if !credentials.is_expired() {
            return (credentials, None);
        }

        // The credentials file is shared with the Claude Code CLI, which also
        // refreshes it. Re-read right before hitting the network: if the CLI (or
        // a concurrent poll) already refreshed the on-disk token, adopt it rather
        // than rotating a second refresh token against the same account.
        if let Ok((disk, disk_source)) = credentials_store::load_credentials() {
            if !disk.is_expired() {
                credentials_store::store_refreshed(&disk_source, &disk);
                return (disk, None);
            }
            credentials = disk;
        }

        let Some(refresh_token) = credentials.refresh_token.clone() else {
            // Environment-provided tokens have no refresh token; nothing to do.
            return (credentials, None);
        };

        // Skip a poll-cadence retry that is still cooling down (#2650): a
        // terminal rejection would replay the identical rejected grant, and a
        // transient failure should not hammer the endpoint every poll.
        let now = Instant::now();
        if let Some(kind) = active_refresh_backoff(&source, now, Some(refresh_token.as_str())) {
            let message = match kind {
                refresh::RefreshFailureKind::Terminal => terminal_refresh_message(),
                refresh::RefreshFailureKind::Transient => refresh_cooldown_message(),
            };
            return (credentials, Some(message));
        }

        match refresh::refresh_access_token(&self.client, &refresh_token, &credentials).await {
            Ok(refreshed) => {
                clear_refresh_backoff(&source);
                credentials_store::store_refreshed(&source, &refreshed);
                if let Err(err) = credentials_store::persist_refreshed_credentials(&refreshed) {
                    tracing::debug!("Claude OAuth token refreshed but could not persist: {err}");
                }
                tracing::debug!("Refreshed expired Claude OAuth token");
                (refreshed, None)
            }
            Err(failure) => {
                tracing::debug!("Claude OAuth token refresh failed: {}", failure.message);
                let message = match failure.kind {
                    refresh::RefreshFailureKind::Terminal => Some(terminal_refresh_message()),
                    refresh::RefreshFailureKind::Transient => Some(refresh_cooldown_message()),
                };
                record_refresh_backoff(&source, failure.kind, now, Some(refresh_token.as_str()));
                (credentials, message)
            }
        }
    }

    /// Fetch usage data using OAuth credentials
    pub async fn fetch_usage(
        &self,
        credentials: &ClaudeOAuthCredentials,
    ) -> Result<OAuthUsageResponse, ProviderError> {
        if credentials.is_expired() {
            return Err(ProviderError::OAuth(
                "OAuth token expired. Run `claude` to refresh.".to_string(),
            ));
        }

        // Check for required scope
        if !credentials.scopes.is_empty() && !credentials.has_scope("user:profile") {
            return Err(ProviderError::OAuth(format!(
                "OAuth token missing 'user:profile' scope (has: {}). Run `claude setup-token` to regenerate.",
                credentials.scopes.join(", ")
            )));
        }

        if let Some(remaining) = Self::rate_limit_backoff_remaining() {
            return Err(Self::rate_limited_error(remaining));
        }

        let response = self
            .client
            .get(Self::USAGE_URL)
            .header(
                "Authorization",
                format!("Bearer {}", credentials.access_token),
            )
            .header("Accept", "application/json")
            .header("anthropic-beta", "oauth-2025-04-20")
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let retry_after = Self::retry_after_duration(response.headers().get(RETRY_AFTER));
            let body = response.text().await.unwrap_or_default();

            if status.as_u16() == 401 {
                return Err(ProviderError::OAuth(
                    "OAuth token invalid or expired. Run `claude` to re-authenticate.".to_string(),
                ));
            }

            if status.as_u16() == 403 && body.contains("user:profile") {
                return Err(ProviderError::OAuth(
                    "OAuth token does not meet scope requirement 'user:profile'. Run `claude setup-token` to regenerate.".to_string(),
                ));
            }

            if status.as_u16() == 429 {
                Self::record_rate_limit(retry_after);
                return Err(Self::rate_limited_error(retry_after));
            }

            return Err(ProviderError::OAuth(format!(
                "API error {}: {}",
                status,
                body.chars().take(200).collect::<String>()
            )));
        }

        let usage: OAuthUsageResponse = response
            .json()
            .await
            .map_err(|e| ProviderError::Parse(format!("Failed to parse OAuth response: {}", e)))?;

        Self::clear_rate_limit();
        Ok(usage)
    }

    fn rate_limit_gate() -> &'static Mutex<Option<Instant>> {
        RATE_LIMIT_BACKOFF_UNTIL.get_or_init(|| Mutex::new(None))
    }

    fn rate_limit_backoff_remaining() -> Option<Duration> {
        let mut guard = Self::rate_limit_gate().lock().ok()?;
        let until = (*guard)?;
        let now = Instant::now();
        if until <= now {
            *guard = None;
            None
        } else {
            Some(until.saturating_duration_since(now))
        }
    }

    fn record_rate_limit(duration: Duration) {
        if let Ok(mut guard) = Self::rate_limit_gate().lock() {
            *guard = Some(Instant::now() + duration);
        }
    }

    fn clear_rate_limit() {
        if let Ok(mut guard) = Self::rate_limit_gate().lock() {
            *guard = None;
        }
    }

    fn retry_after_duration(value: Option<&HeaderValue>) -> Duration {
        let Some(value) = value.and_then(|value| value.to_str().ok()) else {
            return Self::DEFAULT_RATE_LIMIT_BACKOFF;
        };

        if let Ok(seconds) = value.trim().parse::<u64>() {
            return Duration::from_secs(seconds);
        }

        if let Ok(date) = DateTime::parse_from_rfc2822(value.trim()) {
            let now = Utc::now();
            let date = date.with_timezone(&Utc);
            if date > now {
                return (date - now)
                    .to_std()
                    .unwrap_or(Self::DEFAULT_RATE_LIMIT_BACKOFF);
            }
        }

        Self::DEFAULT_RATE_LIMIT_BACKOFF
    }

    fn rate_limited_error(duration: Duration) -> ProviderError {
        ProviderError::OAuth(format!(
            "Claude OAuth usage endpoint is rate limited. Retrying in about {}s; credentials were preserved.",
            duration.as_secs().max(1)
        ))
    }

    /// Build UsageSnapshot from OAuth response
    fn build_usage_snapshot(
        &self,
        response: &OAuthUsageResponse,
        credentials: &ClaudeOAuthCredentials,
    ) -> UsageSnapshot {
        let show_routines = crate::settings::Settings::load().claude_daily_routines_usage_visible;
        self.build_usage_snapshot_with_options(response, credentials, show_routines)
    }

    fn build_usage_snapshot_with_options(
        &self,
        response: &OAuthUsageResponse,
        credentials: &ClaudeOAuthCredentials,
        show_routines: bool,
    ) -> UsageSnapshot {
        // Primary: 5-hour session window
        let primary = response
            .five_hour
            .as_ref()
            .and_then(|w| Self::to_rate_window(w, Some(300)))
            .unwrap_or_else(|| RateWindow::new(0.0));

        let mut usage = UsageSnapshot::new(primary);

        // Secondary: prefer limits[] weekly_all over legacy seven_day (avoids
        // phantom 100% when Anthropic leaves seven_day.utilization stale).
        if let Some(weekly) =
            super::scoped_weekly::weekly_all_window(&response.limits).or_else(|| {
                response
                    .seven_day
                    .as_ref()
                    .and_then(|w| Self::to_rate_window(w, Some(10080)))
            })
        {
            usage = usage.with_secondary(weekly);
        }

        // Model-specific: Opus or Sonnet
        if let Some(opus) = response
            .seven_day_opus
            .as_ref()
            .and_then(|w| Self::to_rate_window(w, Some(10080)))
        {
            usage = usage.with_model_specific(opus);
        } else if let Some(sonnet) = response
            .seven_day_sonnet
            .as_ref()
            .and_then(|w| Self::to_rate_window(w, Some(10080)))
        {
            usage = usage.with_model_specific(sonnet);
        }

        // Model-scoped weekly limits first; Daily Routines last (upstream order).
        usage
            .extra_rate_windows
            .extend(super::scoped_weekly::scoped_weekly_windows(
                &response.limits,
            ));

        if show_routines
            && let Some(window) = response
                .seven_day_routines
                .as_ref()
                .and_then(|w| Self::to_rate_window(w, Some(10080)))
        {
            usage.extra_rate_windows.push(NamedRateWindow::new(
                "claude-routines",
                "Daily Routines",
                window,
            ));
        }

        // Login method from rate limit tier or default
        if let Some(tier) = &credentials.rate_limit_tier {
            usage = usage.with_login_method(super::claude_plan_label(tier));
        } else {
            usage = usage.with_login_method("Claude (OAuth)");
        }

        usage
    }

    /// Convert OAuth usage window to RateWindow
    fn to_rate_window(window: &UsageWindow, window_minutes: Option<u32>) -> Option<RateWindow> {
        let utilization = normalize_utilization(window.utilization?);

        let resets_at = window
            .resets_at
            .as_ref()
            .and_then(|s| parse_iso8601_date(s));

        let reset_description = resets_at.map(format_reset_date);

        Some(RateWindow::with_details(
            utilization,
            window_minutes,
            resets_at,
            reset_description,
        ))
    }
}

impl Default for ClaudeOAuthFetcher {
    fn default() -> Self {
        Self::new()
    }
}

fn normalize_utilization(utilization: f64) -> f64 {
    if utilization > 0.0 && utilization <= 1.0 {
        utilization * 100.0
    } else {
        utilization
    }
}

/// Parse an ISO8601 date string
fn parse_iso8601_date(s: &str) -> Option<DateTime<Utc>> {
    // Try parsing with various formats
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|| {
            // Try without timezone
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .map(|ndt| ndt.and_utc())
        })
}

/// Format a reset date for display
fn format_reset_date(date: DateTime<Utc>) -> String {
    date.format("%b %-d at %-I:%M%p").to_string()
}

#[cfg(test)]
mod tests {
    use super::{ClaudeOAuthCredentials, ClaudeOAuthFetcher, OAuthUsageResponse, UsageWindow};
    use reqwest::header::HeaderValue;
    use std::time::Duration;

    #[test]
    fn converts_fractional_utilization_to_percent() {
        let window = UsageWindow {
            utilization: Some(0.23),
            resets_at: None,
        };

        let rate = ClaudeOAuthFetcher::to_rate_window(&window, Some(300)).expect("rate window");

        assert!((rate.used_percent - 23.0).abs() < f64::EPSILON);
    }

    #[test]
    fn preserves_existing_percentage_utilization() {
        let window = UsageWindow {
            utilization: Some(23.0),
            resets_at: None,
        };

        let rate = ClaudeOAuthFetcher::to_rate_window(&window, Some(300)).expect("rate window");

        assert!((rate.used_percent - 23.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parses_current_snake_case_oauth_usage_response() {
        let response: OAuthUsageResponse = serde_json::from_str(
            r#"{
                "five_hour": {"utilization": 1.0, "resets_at": "2026-05-22T22:10:00Z"},
                "seven_day": {"utilization": 0.14, "resets_at": "2026-05-29T10:00:00Z"},
                "seven_day_oauth_apps": {"utilization": 0.0},
                "limits": [{
                    "kind": "weekly_scoped",
                    "group": "weekly",
                    "percent": 7,
                    "resets_at": "2026-05-29T10:00:00Z",
                    "scope": {"model": {"id": null, "display_name": "Fable"}},
                    "is_active": false
                }],
                "extra_usage": {"is_enabled": true, "used_credits": 0, "monthly_limit": 1000, "currency": "USD"}
            }"#,
        )
        .expect("snake_case OAuth response should parse");

        let credentials = ClaudeOAuthCredentials {
            access_token: "token".to_string(),
            refresh_token: None,
            expires_at: None,
            scopes: vec!["user:profile".to_string()],
            rate_limit_tier: Some("default_claude_ai".to_string()),
        };
        let usage = ClaudeOAuthFetcher::new().build_usage_snapshot(&response, &credentials);

        assert_eq!(usage.primary.used_percent, 100.0);
        assert!((usage.secondary.expect("weekly").used_percent - 14.0).abs() < 0.001);
        let scoped = usage
            .extra_rate_windows
            .iter()
            .find(|window| window.id == "claude-weekly-scoped-fable")
            .expect("Fable scoped weekly limit");
        assert_eq!(scoped.title, "Fable only");
        assert_eq!(scoped.window.used_percent, 7.0);
    }

    #[test]
    fn weekly_all_limit_wins_over_stale_seven_day_utilization() {
        let response: OAuthUsageResponse = serde_json::from_str(
            r#"{
                "five_hour": {"utilization": 8.0, "resets_at": "2026-07-20T04:29:59Z"},
                "seven_day": {"utilization": 1.0, "resets_at": "2026-07-26T22:59:59Z"},
                "limits": [
                    {
                        "kind": "weekly_all",
                        "group": "weekly",
                        "percent": 1,
                        "resets_at": "2026-07-26T22:59:59Z"
                    },
                    {
                        "kind": "weekly_scoped",
                        "group": "weekly",
                        "percent": 2,
                        "resets_at": "2026-07-26T22:59:59Z",
                        "scope": {"model": {"display_name": "Fable"}}
                    }
                ]
            }"#,
        )
        .expect("oauth body with weekly_all");

        let credentials = ClaudeOAuthCredentials {
            access_token: "token".to_string(),
            refresh_token: None,
            expires_at: None,
            scopes: vec![],
            rate_limit_tier: Some("default_claude_max_5x".to_string()),
        };
        let usage = ClaudeOAuthFetcher::new().build_usage_snapshot(&response, &credentials);

        assert!((usage.primary.used_percent - 8.0).abs() < f64::EPSILON);
        // seven_day.utilization 1.0 would normalize to 100%; weekly_all wins.
        assert!((usage.secondary.expect("weekly").used_percent - 1.0).abs() < f64::EPSILON);
        assert_eq!(
            usage
                .extra_rate_windows
                .iter()
                .filter(|w| w.id.starts_with("claude-weekly-scoped-"))
                .count(),
            1
        );
    }

    #[test]
    fn issue_210_reporter_shape_secondary_is_one_percent_not_one_hundred() {
        // Mirrors the reporter JSON: session 8%, fable 2%, all-models should be 1%
        // while seven_day.utilization is the stale 1.0 (would display as 100%).
        let response: OAuthUsageResponse = serde_json::from_str(
            r#"{
                "five_hour": {
                    "utilization": 8.0,
                    "resets_at": "2026-07-20T04:29:59.671218Z"
                },
                "seven_day": {
                    "utilization": 1.0,
                    "resets_at": "2026-07-26T22:59:59.671246Z"
                },
                "limits": [
                    {
                        "kind": "weekly_all",
                        "group": "weekly",
                        "percent": 1.0,
                        "resets_at": "2026-07-26T22:59:59.671595Z"
                    },
                    {
                        "kind": "weekly_scoped",
                        "group": "weekly",
                        "percent": 2.0,
                        "resets_at": "2026-07-26T22:59:59.671595Z",
                        "scope": {
                            "model": {
                                "id": "claude-fable",
                                "display_name": "Fable"
                            }
                        }
                    }
                ]
            }"#,
        )
        .expect("issue 210 body");

        let credentials = ClaudeOAuthCredentials {
            access_token: "token".to_string(),
            refresh_token: None,
            expires_at: None,
            scopes: vec![],
            rate_limit_tier: Some("default_claude_max_5x".to_string()),
        };
        let usage = ClaudeOAuthFetcher::new().build_usage_snapshot(&response, &credentials);

        assert_eq!(usage.login_method.as_deref(), Some("Claude Max 5x"));
        assert!((usage.primary.used_percent - 8.0).abs() < f64::EPSILON);
        let weekly = usage.secondary.expect("secondary weekly");
        assert!(
            (weekly.used_percent - 1.0).abs() < f64::EPSILON,
            "secondary was {}, expected 1% (not 100%)",
            weekly.used_percent
        );
        assert!((weekly.used_percent - 100.0).abs() > 1.0);
        let fable = usage
            .extra_rate_windows
            .iter()
            .find(|w| w.title.contains("Fable"))
            .expect("Fable only window");
        assert!((fable.window.used_percent - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parses_retry_after_seconds() {
        let header = HeaderValue::from_static("17");
        let duration = ClaudeOAuthFetcher::retry_after_duration(Some(&header));

        assert_eq!(duration, Duration::from_secs(17));
    }

    #[test]
    fn invalid_retry_after_uses_default_backoff() {
        let header = HeaderValue::from_static("not-a-date");
        let duration = ClaudeOAuthFetcher::retry_after_duration(Some(&header));

        assert_eq!(duration, ClaudeOAuthFetcher::DEFAULT_RATE_LIMIT_BACKOFF);
    }

    #[test]
    fn rate_limit_gate_blocks_and_clears() {
        ClaudeOAuthFetcher::clear_rate_limit();

        ClaudeOAuthFetcher::record_rate_limit(Duration::from_secs(30));
        assert!(ClaudeOAuthFetcher::rate_limit_backoff_remaining().is_some());

        ClaudeOAuthFetcher::clear_rate_limit();
        assert!(ClaudeOAuthFetcher::rate_limit_backoff_remaining().is_none());
    }

    #[test]
    fn rate_limited_error_preserves_credentials_language() {
        let error = ClaudeOAuthFetcher::rate_limited_error(Duration::from_secs(5));
        let message = error.to_string();

        assert!(message.contains("rate limited"));
        assert!(message.contains("credentials were preserved"));
    }

    #[test]
    fn oauth_extras_put_scoped_weekly_before_routines() {
        let response: OAuthUsageResponse = serde_json::from_str(
            r#"{
                "five_hour": {"utilization": 10.0},
                "seven_day_routines": {"utilization": 5.0},
                "limits": [{
                    "kind": "weekly_scoped",
                    "group": "weekly",
                    "percent": 7,
                    "resets_at": "2026-05-29T10:00:00Z",
                    "scope": {"model": {"display_name": "Fable"}}
                }]
            }"#,
        )
        .expect("oauth body");

        let credentials = ClaudeOAuthCredentials {
            access_token: "token".to_string(),
            refresh_token: None,
            expires_at: None,
            scopes: vec![],
            rate_limit_tier: None,
        };
        let usage = ClaudeOAuthFetcher::new().build_usage_snapshot(&response, &credentials);

        let ids: Vec<&str> = usage
            .extra_rate_windows
            .iter()
            .map(|w| w.id.as_str())
            .collect();
        assert_eq!(ids, vec!["claude-weekly-scoped-fable", "claude-routines"]);
    }

    #[test]
    fn oauth_extras_hide_routines_when_disabled() {
        let response: OAuthUsageResponse = serde_json::from_str(
            r#"{
                "five_hour": {"utilization": 10.0},
                "seven_day_routines": {"utilization": 5.0},
                "limits": [{
                    "kind": "weekly_scoped",
                    "group": "weekly",
                    "percent": 7,
                    "scope": {"model": {"display_name": "Fable"}}
                }]
            }"#,
        )
        .expect("oauth body");

        let credentials = ClaudeOAuthCredentials {
            access_token: "token".to_string(),
            refresh_token: None,
            expires_at: None,
            scopes: vec![],
            rate_limit_tier: None,
        };
        let usage = ClaudeOAuthFetcher::new().build_usage_snapshot_with_options(
            &response,
            &credentials,
            false,
        );

        assert!(
            usage
                .extra_rate_windows
                .iter()
                .all(|w| w.id != "claude-routines")
        );
        assert_eq!(usage.extra_rate_windows.len(), 1);
        assert_eq!(usage.extra_rate_windows[0].id, "claude-weekly-scoped-fable");
    }

    // ── Refresh-token backoff (upstream 0.48.0 #2650 mapping) ───

    fn unique_source(tag: &str) -> super::credentials_store::CredentialSource {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        super::credentials_store::CredentialSource::File(std::path::PathBuf::from(format!(
            "f3-refresh-backoff-{tag}-{nanos}.json"
        )))
    }

    #[test]
    fn terminal_refresh_rejection_stays_blocked_until_credential_changes() {
        let source = unique_source("terminal");
        let now = std::time::Instant::now();
        super::record_refresh_backoff(
            &source,
            super::refresh::RefreshFailureKind::Terminal,
            now,
            Some("dead-refresh-token"),
        );
        // Terminal gate is indefinite: still blocked far in the future as
        // long as the same refresh token is presented.
        assert_eq!(
            super::active_refresh_backoff(
                &source,
                now + Duration::from_secs(3600),
                Some("dead-refresh-token")
            ),
            Some(super::refresh::RefreshFailureKind::Terminal)
        );
        // A different refresh token (CLI re-auth rotated it) clears the gate.
        assert_eq!(
            super::active_refresh_backoff(&source, now, Some("new-refresh-token")),
            None,
            "credential change clears the terminal gate"
        );
        // Re-record with the new token; explicit clear re-allows attempts.
        super::record_refresh_backoff(
            &source,
            super::refresh::RefreshFailureKind::Terminal,
            now,
            Some("new-refresh-token"),
        );
        super::clear_refresh_backoff(&source);
        assert_eq!(
            super::active_refresh_backoff(&source, now, Some("new-refresh-token")),
            None,
            "explicit clear re-allows attempts (e.g. after re-login)"
        );
    }

    #[test]
    fn transient_refresh_failure_gets_5min_backoff() {
        let source = unique_source("transient");
        let now = std::time::Instant::now();
        super::record_refresh_backoff(
            &source,
            super::refresh::RefreshFailureKind::Transient,
            now,
            None,
        );
        assert_eq!(
            super::active_refresh_backoff(&source, now + Duration::from_secs(299), None),
            Some(super::refresh::RefreshFailureKind::Transient)
        );
        assert_eq!(
            super::active_refresh_backoff(&source, now + Duration::from_secs(301), None),
            None
        );
    }

    #[test]
    fn backoff_kinds_have_distinct_user_messages() {
        let terminal = super::terminal_refresh_message();
        assert!(terminal.contains("claude login"), "{terminal}");
        // Upstream drops the "then retry" tail for the provably-dead state.
        assert!(!terminal.contains("retry"), "{terminal}");

        let cooldown = super::refresh_cooldown_message();
        assert!(cooldown.contains("retry shortly"), "{cooldown}");
        assert!(cooldown.contains("claude login"), "{cooldown}");
    }
}
