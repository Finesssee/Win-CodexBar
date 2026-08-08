//! Codex API client for fetching usage information
//!
//! Uses OAuth tokens stored by the Codex CLI in ~/.codex/auth.json

use crate::core::{
    CostSnapshot, NamedRateWindow, ProviderError, RateWindow, RateWindowCadence, UsageSnapshot,
};
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

const DEFAULT_BASE_URL: &str = "https://chatgpt.com/backend-api";
const USAGE_PATH: &str = "/wham/usage";
const RESET_CREDITS_PATH: &str = "/wham/rate-limit-reset-credits";
const CREDENTIAL_CACHE_TTL: Duration = Duration::from_secs(5);

static CREDENTIAL_CACHE: OnceLock<Mutex<Option<CachedCodexCredentials>>> = OnceLock::new();

/// Codex API client
pub struct CodexApi {
    client: reqwest::Client,
    home_dir: PathBuf,
    /// When set, overrides CODEX_HOME / ~/.codex for auth.json + config.toml (tests).
    codex_home_override: Option<PathBuf>,
}

impl CodexApi {
    pub fn new() -> Self {
        // Build client with proper TLS settings
        let client = crate::core::credentialed_http_client_builder()
            .use_rustls_tls()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            client,
            home_dir: dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")),
            codex_home_override: None,
        }
    }

    /// Point the client at a specific Codex home directory (contains auth.json / config.toml).
    pub fn with_codex_home(mut self, codex_home: impl Into<PathBuf>) -> Self {
        self.codex_home_override = Some(codex_home.into());
        self
    }

    fn codex_dir(&self) -> PathBuf {
        if let Some(override_dir) = &self.codex_home_override {
            return override_dir.clone();
        }
        if let Ok(codex_home) = std::env::var("CODEX_HOME") {
            let trimmed = codex_home.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed);
            }
        }
        self.home_dir.join(".codex")
    }

    /// Fetch usage information from Codex API
    /// Returns (UsageSnapshot, optional CostSnapshot)
    pub async fn fetch_usage(
        &self,
    ) -> Result<(UsageSnapshot, Option<CostSnapshot>), ProviderError> {
        // Load credentials
        let creds = self.load_credentials()?;

        // Build request URL
        let base_url = self.resolve_base_url();
        let url = format!("{}{}", base_url, USAGE_PATH);

        // Build request
        let mut request = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", creds.access_token))
            .header("User-Agent", "CodexBar")
            .header("Accept", "application/json")
            .timeout(std::time::Duration::from_secs(30));

        if let Some(account_id) = &creds.account_id
            && !account_id.is_empty()
        {
            request = request.header("ChatGPT-Account-Id", account_id);
        }

        let response = request.send().await?;

        if response.status() == 401 || response.status() == 403 {
            return Err(ProviderError::AuthRequired);
        }

        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Codex API returned {}",
                response.status()
            )));
        }

        // Parse as raw JSON first for flexibility
        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;

        let (mut usage, cost) = self.build_result_from_json(&json)?;
        if let Ok(reset_credits) = self.fetch_rate_limit_reset_credits(&creds, &base_url).await
            && reset_credits.available_count > 0
        {
            let window = reset_credits_rate_window(&reset_credits, Utc::now());
            usage = usage.with_extra_rate_window("reset-credits", "Reset credits", window);
        }
        Ok((usage, cost))
    }

    async fn fetch_rate_limit_reset_credits(
        &self,
        creds: &CodexCredentials,
        base_url: &str,
    ) -> Result<ResetCredits, ProviderError> {
        let mut request = self
            .client
            .get(format!("{}{}", base_url, RESET_CREDITS_PATH))
            .header("Authorization", format!("Bearer {}", creds.access_token))
            .header("User-Agent", "CodexBar")
            .header("Accept", "application/json");
        if let Some(account_id) = &creds.account_id
            && !account_id.is_empty()
        {
            request = request.header("ChatGPT-Account-Id", account_id);
        }
        let response = request.send().await?;
        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Codex reset credits returned {}",
                response.status()
            )));
        }
        decode_reset_credits(&response.bytes().await?)
    }

    fn load_credentials(&self) -> Result<CodexCredentials, ProviderError> {
        let auth_path = self.get_auth_path();

        if !auth_path.exists() {
            return Err(ProviderError::NotInstalled(
                "Codex auth.json not found. Run `codex login` in a terminal to sign in."
                    .to_string(),
            ));
        }

        let modified = std::fs::metadata(&auth_path)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        if let Some(cached) = Self::cached_credentials(&auth_path, modified) {
            return Ok(cached);
        }

        let content = std::fs::read_to_string(&auth_path).map_err(|e| {
            ProviderError::Other(format!("Failed to read Codex credentials: {}", e))
        })?;

        let credentials = Self::parse_credentials_json(&content)?;
        Self::store_cached_credentials(auth_path, modified, credentials.clone());
        Ok(credentials)
    }

    fn parse_credentials_json(content: &str) -> Result<CodexCredentials, ProviderError> {
        let json: serde_json::Value = serde_json::from_str(content)
            .map_err(|e| ProviderError::Parse(format!("Invalid Codex credentials JSON: {}", e)))?;

        // Check for OPENAI_API_KEY first
        if let Some(api_key) = json.get("OPENAI_API_KEY").and_then(|v| v.as_str()) {
            let trimmed = api_key.trim();
            if !trimmed.is_empty() {
                return Ok(CodexCredentials {
                    access_token: trimmed.to_string(),
                    account_id: None,
                });
            }
        }

        // Otherwise, look for tokens object
        let tokens = json.get("tokens").ok_or_else(|| {
            ProviderError::Parse("Codex auth.json exists but contains no tokens.".to_string())
        })?;

        let access_token = tokens
            .get("access_token")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ProviderError::Parse("Missing access_token in Codex credentials".to_string())
            })?
            .to_string();

        let account_id = tokens
            .get("account_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        Ok(CodexCredentials {
            access_token,
            account_id,
        })
    }

    fn credential_cache() -> &'static Mutex<Option<CachedCodexCredentials>> {
        CREDENTIAL_CACHE.get_or_init(|| Mutex::new(None))
    }

    fn cached_credentials(
        path: &std::path::Path,
        modified: Option<SystemTime>,
    ) -> Option<CodexCredentials> {
        let guard = Self::credential_cache().lock().ok()?;
        let cached = guard.as_ref()?;
        if cached.path == path
            && cached.modified == modified
            && cached.loaded_at.elapsed() <= CREDENTIAL_CACHE_TTL
        {
            return Some(cached.credentials.clone());
        }
        None
    }

    fn store_cached_credentials(
        path: PathBuf,
        modified: Option<SystemTime>,
        credentials: CodexCredentials,
    ) {
        if let Ok(mut guard) = Self::credential_cache().lock() {
            *guard = Some(CachedCodexCredentials {
                path,
                modified,
                loaded_at: Instant::now(),
                credentials,
            });
        }
    }

    fn get_auth_path(&self) -> PathBuf {
        self.codex_dir().join("auth.json")
    }

    fn resolve_base_url(&self) -> String {
        let config_path = self.codex_dir().join("config.toml");

        if let Ok(content) = std::fs::read_to_string(&config_path)
            && let Some(base_url) = parse_chatgpt_base_url(&content)
        {
            let normalized = normalize_base_url(&base_url);
            // Only allow HTTPS URLs for custom base URLs to prevent token exfiltration
            if normalized.starts_with("https://")
                || normalized.starts_with("http://127.0.0.1")
                || normalized.starts_with("http://localhost")
            {
                return normalized;
            }
            tracing::warn!(
                "Ignoring insecure custom chatgpt_base_url (must be HTTPS): {}",
                normalized
            );
        }

        DEFAULT_BASE_URL.to_string()
    }

    fn build_result_from_json(
        &self,
        json: &serde_json::Value,
    ) -> Result<(UsageSnapshot, Option<CostSnapshot>), ProviderError> {
        // Extract plan type
        let plan_type = json
            .get("plan_type")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Extract rate limit info - handle multiple possible structures
        let (primary, secondary, monthly, code_review) = self.extract_rate_limits(json);

        // Build login method string
        let login_method = plan_type.as_ref().map(|pt| match pt.as_str() {
            "guest" => "Guest".to_string(),
            "free" => "ChatGPT Free".to_string(),
            "go" => "Codex Go".to_string(),
            "plus" => "ChatGPT Plus".to_string(),
            "pro" | "pro_lite" | "prolite" | "pro-lite" => {
                if pt == "pro" {
                    "ChatGPT Pro".to_string()
                } else {
                    "Pro Lite".to_string()
                }
            }
            "team" => "ChatGPT Team".to_string(),
            "business" => "ChatGPT Business".to_string(),
            "enterprise" => "ChatGPT Enterprise".to_string(),
            "education" | "edu" => "ChatGPT Education".to_string(),
            "free_workspace" | "freeWorkspace" => "Free Workspace".to_string(),
            "quorum" => "Codex Quorum".to_string(),
            "k12" => "Codex K12".to_string(),
            other => format!("ChatGPT {}", capitalize(other)),
        });

        let mut usage = UsageSnapshot::new(primary);
        if let Some(sec) = secondary {
            usage = usage.with_secondary(sec);
        }
        // F5 (upstream 0.48.0): monthly (30-day) windows go to tertiary so the
        // bridge and frontend can show a monthly reset instead of swallowing it.
        if let Some(mo) = monthly {
            usage = usage.with_tertiary(mo);
        }
        if let Some(cr) = code_review {
            usage = usage.with_model_specific(cr);
        }
        for extra in self.extract_additional_rate_limits(json) {
            usage.extra_rate_windows.push(extra);
        }
        if let Some(method) = login_method {
            usage = usage.with_login_method(method);
        }

        // Extract credits if present
        let cost = self.extract_credits(json);

        Ok((usage, cost))
    }

    fn extract_rate_limits(
        &self,
        json: &serde_json::Value,
    ) -> (
        RateWindow,
        Option<RateWindow>,
        Option<RateWindow>,
        Option<RateWindow>,
    ) {
        // Try rate_limit object
        if let Some(rate_limit) = json.get("rate_limit") {
            let primary_opt = rate_limit
                .get("primary_window")
                .and_then(|w| self.parse_window_if_present(w));

            let secondary_opt = rate_limit
                .get("secondary_window")
                .and_then(|w| self.parse_window_if_present(w));

            let code_review = rate_limit
                .get("code_review_window")
                .and_then(|w| self.parse_window_if_present(w));

            let (primary, secondary) = normalize_named_windows(primary_opt, secondary_opt);

            // F5 (upstream 0.48.0): named windows carry only session/weekly/code_review.
            // Monthly is extracted separately (from array windows) — return None here.
            return (primary, secondary, None, code_review);
        }

        // Try rate_limits array
        if let Some(rate_limits) = json.get("rate_limits").and_then(|v| v.as_array()) {
            let windows = rate_limits
                .iter()
                .filter_map(|window| self.parse_window_if_present(window))
                .collect::<Vec<_>>();
            let (primary, secondary, monthly, code_review) = normalize_array_windows(windows);
            // F5 (upstream 0.48.0): route monthly to its own tertiary lane.
            let mut usage = UsageSnapshot::new(primary);
            if let Some(sec) = secondary {
                usage = usage.with_secondary(sec);
            }
            if let Some(mo) = monthly {
                usage = usage.with_tertiary(mo);
            }
            if let Some(cr) = code_review {
                usage = usage.with_model_specific(cr);
            }
            return (
                usage.primary,
                usage.secondary,
                usage.tertiary,
                usage.model_specific,
            );
        }

        // Try direct fields
        let used_percent = json
            .get("used_percent")
            .or_else(|| json.get("usage_percent"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        (RateWindow::new(used_percent), None, None, None)
    }

    fn parse_window(&self, window: &serde_json::Value) -> RateWindow {
        let used_percent = window
            .get("used_percent")
            .or_else(|| window.get("usage_percent"))
            .and_then(json_f64)
            .unwrap_or(0.0);

        let window_minutes = window
            .get("limit_window_seconds")
            .and_then(json_i64)
            .and_then(|seconds| u32::try_from(seconds / 60).ok());

        let reset_at = window
            .get("reset_at")
            .and_then(json_i64)
            .and_then(|ts| Utc.timestamp_opt(ts, 0).single());

        RateWindow::with_details(
            used_percent,
            window_minutes,
            reset_at,
            format_reset_countdown(reset_at),
        )
    }

    fn parse_window_if_present(&self, window: &serde_json::Value) -> Option<RateWindow> {
        (!window.is_null() && !is_placeholder_window(window)).then(|| self.parse_window(window))
    }

    fn extract_additional_rate_limits(&self, json: &serde_json::Value) -> Vec<NamedRateWindow> {
        json.get("additional_rate_limits")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|entry| self.parse_additional_rate_limit(entry))
            .collect()
    }

    fn parse_additional_rate_limit(&self, entry: &serde_json::Value) -> Option<NamedRateWindow> {
        let metered_feature = entry
            .get("metered_feature")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty());
        let limit_name = entry
            .get("limit_name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty());

        let rate_limit = entry.get("rate_limit").unwrap_or(entry);
        let primary = rate_limit.get("primary_window");
        let secondary = rate_limit.get("secondary_window");
        let window = primary.or(secondary)?;
        if is_placeholder_window(window) {
            return None;
        }

        let parsed = self.parse_window(window);
        let feature = metered_feature.unwrap_or_default();
        let limit = limit_name.unwrap_or_default();
        let is_spark = feature.eq_ignore_ascii_case("codex_spark")
            || feature.eq_ignore_ascii_case("spark")
            || limit.to_ascii_lowercase().contains("spark");

        if is_spark {
            let is_weekly = secondary.is_some() && primary.is_none()
                || parsed
                    .window_minutes
                    .is_some_and(|mins| mins >= 7 * 24 * 60);
            let (id, title) = if is_weekly {
                ("codex-spark-weekly", "Codex Spark Weekly")
            } else {
                ("codex-spark", "Codex Spark 5-hour")
            };
            return Some(NamedRateWindow::new(id, title, parsed));
        }

        let label = limit_name.or(metered_feature)?;
        let slug = slugify(label);
        if slug.is_empty() {
            return None;
        }

        Some(NamedRateWindow::new(
            format!("codex-{slug}"),
            titleize_limit_label(label),
            parsed,
        ))
    }

    fn extract_credits(&self, json: &serde_json::Value) -> Option<CostSnapshot> {
        let credits = json.get("credits")?;

        let has_credits = credits
            .get("has_credits")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if !has_credits {
            return None;
        }

        let unlimited = credits
            .get("unlimited")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if unlimited {
            return None;
        }

        let balance = credits
            .get("balance")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        Some(CostSnapshot::new(balance, "USD", "Credits"))
    }

    fn build_result(
        &self,
        response: UsageResponse,
    ) -> Result<(UsageSnapshot, Option<CostSnapshot>), ProviderError> {
        let (primary, secondary) = normalize_named_windows(
            response
                .rate_limit
                .as_ref()
                .and_then(|rate_limit| rate_limit.primary_window.as_ref())
                .map(rate_window_from_snapshot),
            response
                .rate_limit
                .as_ref()
                .and_then(|rate_limit| rate_limit.secondary_window.as_ref())
                .map(rate_window_from_snapshot),
        );

        // Extract code review rate window
        let code_review = response
            .rate_limit
            .as_ref()
            .and_then(|rate_limit| rate_limit.code_review_window.as_ref())
            .map(rate_window_from_snapshot);

        // Build usage snapshot
        let login_method = response.plan_type.as_ref().map(|pt| match pt.as_str() {
            "guest" => "Guest".to_string(),
            "free" => "ChatGPT Free".to_string(),
            "go" => "ChatGPT Go".to_string(),
            "plus" => "ChatGPT Plus".to_string(),
            "pro" => "ChatGPT Pro".to_string(),
            "team" => "ChatGPT Team".to_string(),
            "business" => "ChatGPT Business".to_string(),
            "enterprise" => "ChatGPT Enterprise".to_string(),
            "education" | "edu" => "ChatGPT Education".to_string(),
            other => format!("ChatGPT {}", capitalize(other)),
        });

        let mut usage = UsageSnapshot::new(primary);
        if let Some(sec) = secondary {
            usage = usage.with_secondary(sec);
        }
        if let Some(cr) = code_review {
            usage = usage.with_model_specific(cr);
        }
        if let Some(method) = login_method {
            usage = usage.with_login_method(method);
        }

        // Build cost snapshot if credits are present
        let credit_limit = response.individual_limit.as_ref().or_else(|| {
            response
                .rate_limit
                .as_ref()
                .and_then(|rate_limit| rate_limit.individual_limit.as_ref())
        });
        let cost = response.credits.as_ref().and_then(|credits| {
            if credits.has_credits() {
                let balance = credits.balance.unwrap_or(0.0);
                if credits.unlimited() {
                    None // Unlimited credits, no need to show
                } else if let Some(limit) =
                    credit_limit.and_then(|limit| limit.to_cost_snapshot(balance))
                {
                    Some(limit)
                } else {
                    Some(CostSnapshot::new(balance, "USD", "Credits"))
                }
            } else {
                None
            }
        });

        Ok((usage, cost))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodexWindowRole {
    Session,
    Weekly,
    Monthly,
    Unknown,
}

fn codex_window_role(window: &RateWindow) -> CodexWindowRole {
    match window
        .window_minutes
        .map(RateWindowCadence::from_minutes)
        .unwrap_or(RateWindowCadence::Unknown)
    {
        RateWindowCadence::Session => CodexWindowRole::Session,
        RateWindowCadence::Monthly => CodexWindowRole::Monthly,
        RateWindowCadence::Weekly => CodexWindowRole::Weekly,
        RateWindowCadence::Unknown => CodexWindowRole::Unknown,
    }
}

/// Normalize the named `primary_window`/`secondary_window` fields by duration.
fn normalize_named_windows(
    primary: Option<RateWindow>,
    secondary: Option<RateWindow>,
) -> (RateWindow, Option<RateWindow>) {
    match (primary, secondary) {
        (None, None) => (RateWindow::no_active_session(), None),
        (Some(window), None) => {
            if codex_window_role(&window) == CodexWindowRole::Weekly {
                (RateWindow::no_active_session(), Some(window))
            } else {
                (window, None)
            }
        }
        (None, Some(window)) => {
            if codex_window_role(&window) == CodexWindowRole::Weekly {
                (RateWindow::no_active_session(), Some(window))
            } else {
                (window, None)
            }
        }
        (Some(primary), Some(secondary)) => {
            match (codex_window_role(&primary), codex_window_role(&secondary)) {
                (CodexWindowRole::Weekly, CodexWindowRole::Session) => (secondary, Some(primary)),
                (CodexWindowRole::Weekly, CodexWindowRole::Unknown) => {
                    (RateWindow::no_active_session(), Some(primary))
                }
                (CodexWindowRole::Unknown, CodexWindowRole::Session) => (secondary, Some(primary)),
                (CodexWindowRole::Session, CodexWindowRole::Weekly)
                | (CodexWindowRole::Unknown, CodexWindowRole::Weekly) => (primary, Some(secondary)),
                _ => (primary, Some(secondary)),
            }
        }
    }
}

/// Normalize an array of Codex windows without relying on the API's ordering.
/// Normalize an array of Codex windows without relying on the API's ordering.
///
/// Returns (session, weekly, monthly, code_review). F5 (upstream 0.48.0):
/// monthly (30-day) windows are routed to their own lane so surfaces can
/// display a monthly reset instead of swallowing it into the weekly label.
fn normalize_array_windows(
    windows: Vec<RateWindow>,
) -> (
    RateWindow,
    Option<RateWindow>,
    Option<RateWindow>,
    Option<RateWindow>,
) {
    if windows.is_empty() {
        return (RateWindow::no_active_session(), None, None, None);
    }

    // Preserve the old positional fallback when the API provides no role
    // metadata at all. There is no safe way to infer session vs weekly then.
    if !windows
        .iter()
        .any(|window| codex_window_role(window) != CodexWindowRole::Unknown)
    {
        let mut windows = windows.into_iter();
        return (
            windows.next().unwrap_or_else(RateWindow::no_active_session),
            windows.next(),
            windows.next(),
            windows.next(),
        );
    }

    let mut session = None;
    let mut weekly = None;
    let mut monthly = None;
    let mut remaining = Vec::new();

    for window in windows {
        match codex_window_role(&window) {
            CodexWindowRole::Session if session.is_none() => session = Some(window),
            CodexWindowRole::Weekly if weekly.is_none() => weekly = Some(window),
            CodexWindowRole::Monthly if monthly.is_none() => monthly = Some(window),
            _ => remaining.push(window),
        }
    }

    (
        session.unwrap_or_else(RateWindow::no_active_session),
        weekly,
        monthly,
        remaining.into_iter().next(),
    )
}

fn rate_window_from_snapshot(window: &WindowSnapshot) -> RateWindow {
    let reset_at = timestamp_to_datetime(window.reset_at);
    RateWindow::with_details(
        window.used_percent as f64,
        window
            .limit_window_seconds
            .and_then(|seconds| u32::try_from(seconds / 60).ok()),
        reset_at,
        format_reset_countdown(reset_at),
    )
}

impl Default for CodexApi {
    fn default() -> Self {
        Self::new()
    }
}

// --- Data structures ---

#[derive(Clone)]
struct CodexCredentials {
    access_token: String,
    account_id: Option<String>,
}

struct CachedCodexCredentials {
    path: PathBuf,
    modified: Option<SystemTime>,
    loaded_at: Instant,
    credentials: CodexCredentials,
}

#[derive(Debug, Deserialize)]
struct UsageResponse {
    plan_type: Option<String>,
    rate_limit: Option<RateLimitDetails>,
    credits: Option<CreditDetails>,
    #[serde(default, alias = "individualLimit")]
    individual_limit: Option<SpendControlLimitSnapshot>,
}

#[derive(Debug, Deserialize)]
struct RateLimitDetails {
    primary_window: Option<WindowSnapshot>,
    secondary_window: Option<WindowSnapshot>,
    code_review_window: Option<WindowSnapshot>,
    #[serde(default, alias = "individualLimit")]
    individual_limit: Option<SpendControlLimitSnapshot>,
}

#[derive(Debug, Deserialize)]
struct WindowSnapshot {
    used_percent: i32,
    reset_at: Option<i64>,
    limit_window_seconds: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CreditDetails {
    has_credits: Option<bool>,
    unlimited: Option<bool>,
    balance: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct SpendControlLimitSnapshot {
    limit: Option<f64>,
    used: Option<f64>,
    #[serde(default, alias = "remainingPercent")]
    remaining_percent: Option<f64>,
    #[serde(default, alias = "resetsAt")]
    resets_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
struct ResetCredit {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    expires_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ResetCredits {
    #[serde(default)]
    credits: Vec<ResetCredit>,
    #[serde(default)]
    available_count: u32,
}

fn decode_reset_credits(data: &[u8]) -> Result<ResetCredits, ProviderError> {
    serde_json::from_slice(data)
        .map_err(|e| ProviderError::Parse(format!("Failed to parse Codex reset credits: {e}")))
}

fn parse_credit_expiry(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn is_available_credit(credit: &ResetCredit) -> bool {
    match credit.status.as_deref() {
        None | Some("") => true,
        Some(status) => status.eq_ignore_ascii_case("available"),
    }
}

fn next_available_reset_credit_expiry(
    credits: &[ResetCredit],
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    credits
        .iter()
        .filter(|credit| is_available_credit(credit))
        .filter_map(|credit| credit.expires_at.as_deref().and_then(parse_credit_expiry))
        .filter(|expires_at| *expires_at > now)
        .min()
}

fn reset_credits_rate_window(reset: &ResetCredits, now: DateTime<Utc>) -> RateWindow {
    let description = format!(
        "{} reset credit{} available",
        reset.available_count,
        if reset.available_count == 1 { "" } else { "s" }
    );
    let mut window = RateWindow::informational(description);
    window.resets_at = next_available_reset_credit_expiry(&reset.credits, now);
    window
}

impl CreditDetails {
    // Helper to safely check has_credits
    fn has_credits(&self) -> bool {
        self.has_credits.unwrap_or(false)
    }

    fn unlimited(&self) -> bool {
        self.unlimited.unwrap_or(false)
    }
}

impl SpendControlLimitSnapshot {
    fn to_cost_snapshot(&self, balance: f64) -> Option<CostSnapshot> {
        let limit = self
            .limit
            .filter(|limit| limit.is_finite() && *limit >= 0.0)?;
        let used = self
            .used
            .filter(|used| used.is_finite() && *used >= 0.0)
            .or_else(|| {
                self.remaining_percent
                    .filter(|pct| pct.is_finite() && *pct >= 0.0)
                    .map(|remaining| limit * (1.0 - (remaining / 100.0)))
            })
            .unwrap_or_else(|| (limit - balance).max(0.0));
        let mut cost =
            CostSnapshot::new(used.clamp(0.0, limit), "USD", "Monthly credits").with_limit(limit);
        if let Some(resets_at) = timestamp_to_datetime(self.resets_at) {
            cost = cost.with_resets_at(resets_at);
        }
        Some(cost)
    }
}

// --- Helper functions ---

fn timestamp_to_datetime(timestamp: Option<i64>) -> Option<DateTime<Utc>> {
    timestamp.and_then(|ts| Utc.timestamp_opt(ts, 0).single())
}

fn json_f64(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|value| value as f64))
        .or_else(|| value.as_str()?.trim().parse::<f64>().ok())
}

fn json_i64(value: &serde_json::Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str()?.trim().parse::<i64>().ok())
}

fn is_placeholder_window(window: &serde_json::Value) -> bool {
    let has_usage = window
        .get("used_percent")
        .or_else(|| window.get("usage_percent"))
        .and_then(json_f64)
        .is_some();
    let has_duration = window
        .get("limit_window_seconds")
        .and_then(json_i64)
        .is_some();
    let has_reset = window.get("reset_at").and_then(json_i64).is_some();

    !has_usage && !has_duration && !has_reset
}

fn slugify(label: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;

    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            previous_dash = false;
        } else if !previous_dash && !slug.is_empty() {
            slug.push('-');
            previous_dash = true;
        }
    }

    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

fn titleize_limit_label(label: &str) -> String {
    label
        .split(['_', '-', ' '])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first
                    .to_uppercase()
                    .chain(chars.flat_map(char::to_lowercase))
                    .collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

fn format_reset_countdown(reset_at: Option<DateTime<Utc>>) -> Option<String> {
    let dt = reset_at?;
    let now = Utc::now();
    if dt <= now {
        return Some("now".to_string());
    }
    let diff = dt - now;
    let total_mins = diff.num_minutes();
    let hours = diff.num_hours();
    let mins = total_mins % 60;
    if hours >= 24 {
        let days = hours / 24;
        let rem_h = hours % 24;
        if rem_h == 0 {
            Some(format!("{}d", days))
        } else {
            Some(format!("{}d {}h", days, rem_h))
        }
    } else if hours > 0 {
        if mins == 0 {
            Some(format!("{}h", hours))
        } else {
            Some(format!("{}h {}m", hours, mins))
        }
    } else {
        Some(format!("{}m", mins))
    }
}

fn parse_chatgpt_base_url(config_content: &str) -> Option<String> {
    for line in config_content.lines() {
        // Skip comments
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        // Look for chatgpt_base_url = "..."
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            if key == "chatgpt_base_url" {
                let mut value = value.trim();
                // Remove quotes
                if (value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\''))
                {
                    value = &value[1..value.len() - 1];
                }
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

fn normalize_base_url(url: &str) -> String {
    let mut trimmed = url.trim().to_string();
    if trimmed.is_empty() {
        return DEFAULT_BASE_URL.to_string();
    }

    // Remove trailing slashes
    while trimmed.ends_with('/') {
        trimmed.pop();
    }

    // Add /backend-api if needed
    if (trimmed.starts_with("https://chatgpt.com")
        || trimmed.starts_with("https://chat.openai.com"))
        && !trimmed.contains("/backend-api")
    {
        trimmed.push_str("/backend-api");
    }

    trimmed
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_codex_credentials_without_retaining_refresh_token() {
        let credentials = CodexApi::parse_credentials_json(
            r#"{
                "tokens": {
                    "access_token": "access",
                    "refresh_token": "refresh",
                    "account_id": "acct_123"
                }
            }"#,
        )
        .expect("credentials");

        assert_eq!(credentials.access_token, "access");
        assert_eq!(credentials.account_id.as_deref(), Some("acct_123"));
    }

    #[test]
    fn decodes_reset_credits() {
        let credits = decode_reset_credits(
            br#"{"available_count":2,"credits":[{"id":"a","status":"available","expires_at":"2026-08-01T12:00:00Z"}]}"#,
        )
        .expect("reset credits");
        assert_eq!(credits.available_count, 2);
        assert_eq!(credits.credits.len(), 1);
        assert_eq!(credits.credits[0].status.as_deref(), Some("available"));
        assert_eq!(
            credits.credits[0].expires_at.as_deref(),
            Some("2026-08-01T12:00:00Z")
        );
    }

    #[test]
    fn next_expiry_picks_soonest_available() {
        let now = DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let credits = vec![
            ResetCredit {
                status: Some("available".into()),
                expires_at: Some("2026-07-10T00:00:00Z".into()),
            },
            ResetCredit {
                status: Some("available".into()),
                expires_at: Some("2026-07-05T00:00:00Z".into()),
            },
            ResetCredit {
                status: Some("available".into()),
                expires_at: Some("2026-07-20T00:00:00Z".into()),
            },
        ];
        let expiry = next_available_reset_credit_expiry(&credits, now).expect("expiry");
        assert_eq!(
            expiry,
            DateTime::parse_from_rfc3339("2026-07-05T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    #[test]
    fn next_expiry_skips_past_and_non_available() {
        let now = DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let credits = vec![
            ResetCredit {
                status: Some("available".into()),
                expires_at: Some("2026-06-01T00:00:00Z".into()),
            },
            ResetCredit {
                status: Some("used".into()),
                expires_at: Some("2026-07-03T00:00:00Z".into()),
            },
            ResetCredit {
                status: Some("AVAILABLE".into()),
                expires_at: Some("2026-07-08T00:00:00Z".into()),
            },
            ResetCredit {
                status: None,
                expires_at: Some("2026-07-09T00:00:00Z".into()),
            },
        ];
        let expiry = next_available_reset_credit_expiry(&credits, now).expect("expiry");
        assert_eq!(
            expiry,
            DateTime::parse_from_rfc3339("2026-07-08T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    #[test]
    fn reset_credits_window_sets_informational_and_expiry() {
        let now = DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let reset = ResetCredits {
            available_count: 2,
            credits: vec![
                ResetCredit {
                    status: Some("available".into()),
                    expires_at: Some("2026-07-15T12:00:00Z".into()),
                },
                ResetCredit {
                    status: Some("available".into()),
                    expires_at: Some("2026-07-10T12:00:00Z".into()),
                },
            ],
        };
        let window = reset_credits_rate_window(&reset, now);
        assert!(window.is_informational);
        assert_eq!(
            window.reset_description.as_deref(),
            Some("2 reset credits available")
        );
        assert_eq!(
            window.resets_at,
            Some(
                DateTime::parse_from_rfc3339("2026-07-10T12:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc)
            )
        );
    }

    #[test]
    fn reset_credits_window_count_only_without_expiry() {
        let now = DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let reset = ResetCredits {
            available_count: 1,
            credits: vec![],
        };
        let window = reset_credits_rate_window(&reset, now);
        assert!(window.is_informational);
        assert_eq!(
            window.reset_description.as_deref(),
            Some("1 reset credit available")
        );
        assert!(window.resets_at.is_none());
    }

    fn write_codex_home(base_url: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temp codex home");
        std::fs::write(
            dir.path().join("auth.json"),
            r#"{"tokens":{"access_token":"test-token","account_id":"acct_test"}}"#,
        )
        .expect("auth.json");
        std::fs::write(
            dir.path().join("config.toml"),
            format!("chatgpt_base_url = \"{base_url}\""),
        )
        .expect("config.toml");
        dir
    }

    #[tokio::test]
    async fn fetch_usage_attaches_reset_credits_from_http() {
        let mut server = mockito::Server::new_async().await;
        let soonest = (Utc::now() + chrono::Duration::days(5)).to_rfc3339();
        let later = (Utc::now() + chrono::Duration::days(12)).to_rfc3339();

        let usage_mock = server
            .mock("GET", "/wham/usage")
            .match_header("authorization", "Bearer test-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"plan_type":"plus","rate_limit":{"primary_window":{"used_percent":10,"limit_window_seconds":18000}}}"#,
            )
            .create_async()
            .await;

        let reset_body = format!(
            r#"{{"available_count":2,"credits":[
                {{"status":"available","expires_at":"{later}"}},
                {{"status":"available","expires_at":"{soonest}"}}
            ]}}"#
        );
        let reset_mock = server
            .mock("GET", "/wham/rate-limit-reset-credits")
            .match_header("authorization", "Bearer test-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(reset_body)
            .create_async()
            .await;

        let home = write_codex_home(&server.url());
        let api = CodexApi::new().with_codex_home(home.path());
        let (usage, _) = api.fetch_usage().await.expect("fetch_usage");

        usage_mock.assert_async().await;
        reset_mock.assert_async().await;

        let extra = usage
            .extra_rate_windows
            .iter()
            .find(|w| w.id == "reset-credits")
            .expect("reset-credits window attached");
        assert_eq!(extra.title, "Reset credits");
        assert!(extra.window.is_informational);
        assert_eq!(
            extra.window.reset_description.as_deref(),
            Some("2 reset credits available")
        );
        let expected = DateTime::parse_from_rfc3339(&soonest)
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(extra.window.resets_at, Some(expected));
    }

    #[tokio::test]
    async fn fetch_usage_skips_reset_credits_when_available_count_zero() {
        let mut server = mockito::Server::new_async().await;

        let usage_mock = server
            .mock("GET", "/wham/usage")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"plan_type":"plus","rate_limit":{"primary_window":{"used_percent":10,"limit_window_seconds":18000}}}"#,
            )
            .create_async()
            .await;

        let reset_mock = server
            .mock("GET", "/wham/rate-limit-reset-credits")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"available_count":0,"credits":[]}"#)
            .create_async()
            .await;

        let home = write_codex_home(&server.url());
        let api = CodexApi::new().with_codex_home(home.path());
        let (usage, _) = api.fetch_usage().await.expect("fetch_usage");

        usage_mock.assert_async().await;
        reset_mock.assert_async().await;

        assert!(
            usage
                .extra_rate_windows
                .iter()
                .all(|w| w.id != "reset-credits"),
            "available_count=0 must not attach reset-credits"
        );
    }

    #[test]
    fn keeps_weekly_window_in_secondary_when_session_is_absent() {
        let api = CodexApi::new();
        let (usage, _) = api
            .build_result_from_json(&json!({
                "rate_limit": {
                    "secondary_window": {
                        "used_percent": 25,
                        "limit_window_seconds": 604800,
                        "reset_at": 1783036800
                    }
                }
            }))
            .expect("codex usage");

        assert!(usage.primary.is_informational);
        assert_eq!(usage.primary.window_minutes, Some(300));
        assert_eq!(
            usage.primary.reset_description.as_deref(),
            Some("No active 5h session")
        );

        let weekly = usage.secondary.expect("weekly window");
        assert!(!weekly.is_informational);
        assert_eq!(weekly.used_percent, 25.0);
        assert_eq!(weekly.window_minutes, Some(10080));
    }

    #[test]
    fn identifies_rate_limit_array_windows_by_duration() {
        let api = CodexApi::new();
        let (usage, _) = api
            .build_result_from_json(&json!({
                "rate_limits": [
                    {
                        "used_percent": 25,
                        "limit_window_seconds": 604800,
                        "reset_at": 1783036800
                    },
                    {
                        "used_percent": 10,
                        "limit_window_seconds": 18000,
                        "reset_at": 1783018800
                    }
                ]
            }))
            .expect("codex usage");

        assert!(!usage.primary.is_informational);
        assert_eq!(usage.primary.used_percent, 10.0);
        assert_eq!(usage.primary.window_minutes, Some(300));

        let weekly = usage.secondary.expect("weekly window");
        assert_eq!(weekly.used_percent, 25.0);
        assert_eq!(weekly.window_minutes, Some(10080));
    }

    #[test]
    fn identifies_weekly_only_rate_limit_array_without_a_session() {
        let api = CodexApi::new();
        let (usage, _) = api
            .build_result_from_json(&json!({
                "rate_limits": [{
                    "used_percent": 25,
                    "limit_window_seconds": 604800,
                    "reset_at": 1783036800
                }]
            }))
            .expect("codex usage");

        assert!(usage.primary.is_informational);
        assert_eq!(usage.secondary.expect("weekly window").used_percent, 25.0);
    }

    #[test]
    fn maps_codex_spark_additional_rate_limits() {
        let api = CodexApi::new();
        let (usage, _) = api
            .build_result_from_json(&json!({
                "plan_type": "pro",
                "rate_limit": {
                    "primary_window": { "used_percent": 20, "limit_window_seconds": 18000 },
                    "secondary_window": { "used_percent": 40, "limit_window_seconds": 604800 }
                },
                "additional_rate_limits": [
                    {
                        "limit_name": "Codex Spark",
                        "metered_feature": "codex_spark",
                        "rate_limit": {
                            "primary_window": { "used_percent": "17", "limit_window_seconds": 18000 }
                        }
                    },
                    {
                        "limit_name": "Codex Spark Weekly",
                        "metered_feature": "codex_spark",
                        "rate_limit": {
                            "secondary_window": { "used_percent": 62, "limit_window_seconds": 604800 }
                        }
                    }
                ]
            }))
            .expect("codex usage");

        assert_eq!(usage.extra_rate_windows.len(), 2);
        assert_eq!(usage.extra_rate_windows[0].id, "codex-spark");
        assert_eq!(usage.extra_rate_windows[0].title, "Codex Spark 5-hour");
        assert_eq!(usage.extra_rate_windows[0].window.used_percent, 17.0);
        assert_eq!(usage.extra_rate_windows[1].id, "codex-spark-weekly");
        assert_eq!(usage.extra_rate_windows[1].title, "Codex Spark Weekly");
        assert_eq!(usage.extra_rate_windows[1].window.used_percent, 62.0);
    }

    #[test]
    fn ignores_placeholder_additional_rate_limits() {
        let api = CodexApi::new();
        let (usage, _) = api
            .build_result_from_json(&json!({
                "rate_limit": {
                    "primary_window": { "used_percent": 0, "limit_window_seconds": 18000 }
                },
                "additional_rate_limits": [
                    {
                        "limit_name": "placeholder",
                        "metered_feature": "placeholder",
                        "rate_limit": { "primary_window": {} }
                    }
                ]
            }))
            .expect("codex usage");

        assert!(usage.extra_rate_windows.is_empty());
    }

    #[test]
    fn maps_top_level_individual_credit_limit_to_cost_snapshot() {
        let api = CodexApi::new();
        let (_, cost) = api
            .build_result(UsageResponse {
                plan_type: None,
                rate_limit: None,
                credits: Some(CreditDetails {
                    has_credits: Some(true),
                    unlimited: Some(false),
                    balance: Some(7.5),
                }),
                individual_limit: Some(SpendControlLimitSnapshot {
                    limit: Some(20.0),
                    used: Some(12.5),
                    remaining_percent: None,
                    resets_at: Some(1783036800),
                }),
            })
            .expect("codex result");
        let cost = cost.expect("cost");
        assert_eq!(cost.used, 12.5);
        assert_eq!(cost.limit, Some(20.0));
        assert!(cost.resets_at.is_some());
    }

    #[test]
    fn maps_nested_individual_credit_limit_to_cost_snapshot() {
        let api = CodexApi::new();
        let (_, cost) = api
            .build_result(UsageResponse {
                plan_type: None,
                rate_limit: Some(RateLimitDetails {
                    primary_window: None,
                    secondary_window: None,
                    code_review_window: None,
                    individual_limit: Some(SpendControlLimitSnapshot {
                        limit: Some(100.0),
                        used: None,
                        remaining_percent: Some(60.0),
                        resets_at: None,
                    }),
                }),
                credits: Some(CreditDetails {
                    has_credits: Some(true),
                    unlimited: Some(false),
                    balance: Some(60.0),
                }),
                individual_limit: None,
            })
            .expect("codex result");
        let cost = cost.expect("cost");
        assert_eq!(cost.used, 40.0);
        assert_eq!(cost.limit, Some(100.0));
    }

    fn win(minutes: u32, used: f64) -> RateWindow {
        RateWindow::with_details(used, Some(minutes), None, None)
    }

    #[test]
    fn f5_normalize_array_routes_session_weekly_monthly_to_lanes() {
        // 5h session + weekly + monthly → (session, weekly, monthly, None)
        let windows = vec![win(300, 10.0), win(10_080, 20.0), win(43_200, 30.0)];
        let (primary, secondary, tertiary, code_review) = normalize_array_windows(windows);
        assert_eq!(primary.window_minutes, Some(300));
        assert_eq!(secondary.unwrap().window_minutes, Some(10_080));
        assert_eq!(tertiary.unwrap().window_minutes, Some(43_200));
        assert!(code_review.is_none());
    }

    #[test]
    fn f5_normalize_array_monthly_routes_to_tertiary_not_secondary() {
        // Monthly must go to tertiary, NOT secondary — so #268's weekly math
        // and "Weekly" label stay untouched.
        let windows = vec![win(43_200, 50.0), win(10_080, 20.0)];
        let (primary, secondary, tertiary, _) = normalize_array_windows(windows);
        assert_eq!(primary.window_minutes, Some(300)); // no session → placeholder
        assert_eq!(secondary.unwrap().window_minutes, Some(10_080));
        assert_eq!(tertiary.unwrap().window_minutes, Some(43_200));
    }

    #[test]
    fn f5_normalize_array_empty_returns_placeholder_primary() {
        let (primary, secondary, tertiary, code_review) = normalize_array_windows(vec![]);
        assert!(primary.is_informational);
        assert!(secondary.is_none());
        assert!(tertiary.is_none());
        assert!(code_review.is_none());
    }

    #[test]
    fn f5_normalize_array_unknown_windows_fall_to_code_review() {
        // Windows with unrecognized durations (not 300/10080/43200) go to the
        // remaining/code_review bucket.
        let windows = vec![win(300, 10.0), win(999, 5.0)];
        let (primary, secondary, tertiary, code_review) = normalize_array_windows(windows);
        assert_eq!(primary.window_minutes, Some(300));
        assert!(secondary.is_none());
        assert!(tertiary.is_none());
        assert_eq!(code_review.unwrap().window_minutes, Some(999));
    }
}
