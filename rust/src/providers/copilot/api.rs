//! GitHub Copilot API client for fetching usage information.
//!
//! Uses a GitHub OAuth token for GitHub's Copilot usage endpoint. The primary
//! path is app-managed device OAuth/token accounts; legacy API key and Windows
//! Credential Manager tokens remain supported as fallbacks.

use crate::core::{NamedRateWindow, ProviderError, RateWindow, UsageSnapshot};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const DEFAULT_GITHUB_HOST: &str = "github.com";
const COPILOT_USAGE_PATH: &str = "/copilot_internal/user";
const GITHUB_USER_PATH: &str = "/user";

// Credential Manager targets to try
const CREDENTIAL_TARGETS: &[&str] = &[
    "codexbar-copilot",       // Our own storage
    "git:https://github.com", // GitHub CLI / Git Credential Manager
    "github.com",             // Alternative format
];

/// Basic GitHub identity for labeling OAuth token accounts.
#[derive(Debug, Clone, Deserialize)]
pub struct GitHubIdentity {
    pub login: String,
    pub id: Option<u64>,
    pub name: Option<String>,
}

/// Copilot API client.
pub struct CopilotApi {
    client: reqwest::Client,
}

impl CopilotApi {
    pub fn new() -> Self {
        let client = crate::core::credentialed_http_client_builder()
            .use_rustls_tls()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self { client }
    }

    /// Fetch usage information from the default GitHub host.
    pub async fn fetch_usage(&self, api_key: Option<&str>) -> Result<UsageSnapshot, ProviderError> {
        self.fetch_usage_for_host(api_key, None).await
    }

    /// Fetch usage information from Copilot API, optionally targeting an
    /// enterprise GitHub host. `github.com` maps to `api.github.com`; an
    /// enterprise host maps to `api.<host>` unless it already starts with
    /// `api.`.
    pub async fn fetch_usage_for_host(
        &self,
        api_key: Option<&str>,
        github_host: Option<&str>,
    ) -> Result<UsageSnapshot, ProviderError> {
        let token = self.load_token(api_key, github_host)?;
        self.fetch_usage_with_token(&token, github_host).await
    }

    /// Fetch usage with an already-resolved OAuth token.
    pub async fn fetch_usage_with_token(
        &self,
        token: &str,
        github_host: Option<&str>,
    ) -> Result<UsageSnapshot, ProviderError> {
        let api_url = copilot_usage_url(github_host);
        let response = self
            .client
            .get(api_url)
            .header("Authorization", format!("token {}", token.trim()))
            .header("Accept", "application/json")
            .header("Editor-Version", "vscode/1.96.2")
            .header("Editor-Plugin-Version", "copilot-chat/0.26.7")
            .header("User-Agent", "GitHubCopilotChat/0.26.7")
            .header("X-Github-Api-Version", "2025-04-01")
            .send()
            .await
            .map_err(|e| ProviderError::Other(format!("Request failed: {}", e)))?;

        if response.status() == 401 || response.status() == 403 {
            return Err(ProviderError::AuthRequired);
        }

        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "GitHub Copilot usage endpoint returned {}",
                response.status()
            )));
        }

        let usage_response: CopilotUsageResponse = response
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;

        snapshot_from_response(usage_response)
    }

    /// Fetch GitHub identity for labeling a stored device-OAuth token.
    pub async fn fetch_identity_with_token(
        &self,
        token: &str,
        github_host: Option<&str>,
    ) -> Result<GitHubIdentity, ProviderError> {
        let url = github_api_url(github_host, GITHUB_USER_PATH);
        let response = self
            .client
            .get(url)
            .header("Authorization", format!("token {}", token.trim()))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "Win-CodexBar")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|e| ProviderError::Other(format!("Request failed: {}", e)))?;

        if response.status() == 401 || response.status() == 403 {
            return Err(ProviderError::AuthRequired);
        }

        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "GitHub identity endpoint returned {}",
                response.status()
            )));
        }

        response
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))
    }

    fn load_token(
        &self,
        api_key: Option<&str>,
        github_host: Option<&str>,
    ) -> Result<String, ProviderError> {
        if let Some(key) = normalize_token(api_key) {
            tracing::debug!("Using Copilot token from settings or active token account");
            return Ok(key);
        }

        if let Some(token) = load_gh_cli_token(github_host) {
            tracing::debug!("Using Copilot token from GitHub CLI auth");
            return Ok(token);
        }

        for target in CREDENTIAL_TARGETS {
            if let Some(token) = self.try_load_credential(target)
                && let Some(actual_token) = normalize_token(Some(&token))
            {
                tracing::debug!("Found Copilot token in credential target: {}", target);
                return Ok(actual_token);
            }
        }

        Err(ProviderError::NotInstalled(
            "GitHub Copilot token not found. Sign in with GitHub from Copilot settings, run 'gh auth login', or add a legacy GitHub token.".to_string(),
        ))
    }

    #[cfg(target_os = "windows")]
    fn try_load_credential(&self, target: &str) -> Option<String> {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Security::Credentials::{
            CRED_TYPE_GENERIC, CREDENTIALW, CredFree, CredReadW,
        };
        use windows::core::PCWSTR;

        let target_wide: Vec<u16> = OsStr::new(target)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let mut credential: *mut CREDENTIALW = std::ptr::null_mut();

        let result = unsafe {
            CredReadW(
                PCWSTR(target_wide.as_ptr()),
                CRED_TYPE_GENERIC,
                0,
                &mut credential,
            )
        };

        if result.is_err() {
            return None;
        }

        let token = unsafe {
            let cred = &*credential;
            if cred.CredentialBlobSize == 0 || cred.CredentialBlob.is_null() {
                CredFree(credential as *mut std::ffi::c_void);
                return None;
            }

            let blob =
                std::slice::from_raw_parts(cred.CredentialBlob, cred.CredentialBlobSize as usize);

            let token = String::from_utf8_lossy(blob).to_string();
            CredFree(credential as *mut std::ffi::c_void);
            token
        };

        let trimmed = token.trim();
        if !trimmed.is_empty() {
            Some(trimmed.to_string())
        } else {
            None
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn try_load_credential(&self, _target: &str) -> Option<String> {
        None
    }
}

impl Default for CopilotApi {
    fn default() -> Self {
        Self::new()
    }
}

// --- API Response Types ---

#[derive(Debug, Deserialize)]
struct CopilotUsageResponse {
    #[serde(default)]
    quota_snapshots: QuotaSnapshots,
    #[serde(default)]
    monthly_quotas: QuotaCounts,
    #[serde(default)]
    limited_user_quotas: QuotaCounts,
    #[serde(default = "unknown_plan")]
    copilot_plan: String,
    #[serde(default)]
    token_based_billing: bool,
    #[serde(default)]
    quota_reset_date: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(transparent)]
struct QuotaSnapshots {
    entries: Map<String, Value>,
}

#[derive(Debug, Default, Deserialize)]
struct QuotaCounts {
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    completions: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    chat: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct QuotaSnapshot {
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    entitlement: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    remaining: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    percent_remaining: Option<f64>,
    #[serde(default)]
    quota_id: Option<String>,
    #[serde(default)]
    placeholder: bool,
    /// Absolute AI-credit consumption counter reported for token-billed seats
    /// (upstream 0.48.0 #2613: `credits_used`). Kept off the rate-window path
    /// on purpose — a counter has no quota denominator to render.
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    credits_used: Option<f64>,
}

// --- Snapshot building ---

fn snapshot_from_response(response: CopilotUsageResponse) -> Result<UsageSnapshot, ProviderError> {
    let reset = response
        .quota_reset_date
        .as_deref()
        .and_then(parse_iso_date);
    let quotas = response.usable_quotas(reset);
    let credits_used = response.credits_used_counter();

    let primary_quota = quotas.premium.clone().or_else(|| quotas.first.clone());
    if primary_quota.is_none()
        && quotas.chat.is_none()
        && quotas.completions.is_none()
        && response.token_based_billing
    {
        // Token-billed seats reporting zero-entitlement quota snapshots still
        // carry their true consumption in the absolute credits counter —
        // surface it instead of blanking the seat (upstream #2613 regression).
        if let Some(credits) = credits_used {
            let mut primary = RateWindow::informational(format_credits_used(credits));
            primary.resets_at = reset;
            return Ok(
                UsageSnapshot::new(primary).with_login_method(plan_label(&response.copilot_plan))
            );
        }
        return Err(ProviderError::Other(
            "Copilot Business token-based billing usage is unavailable from GitHub's current endpoint.".to_string(),
        ));
    }

    let primary = primary_quota
        .as_ref()
        .map(|quota| quota.to_rate_window(reset))
        .unwrap_or_else(|| RateWindow::new(0.0));

    let mut usage =
        UsageSnapshot::new(primary).with_login_method(plan_label(&response.copilot_plan));

    if let Some(chat) = quotas.chat
        && primary_quota
            .as_ref()
            .is_none_or(|primary| primary.kind != CopilotQuotaKind::Chat)
    {
        usage = usage.with_secondary(chat.to_rate_window(reset));
    }

    if let Some(completions) = quotas.completions
        && primary_quota
            .as_ref()
            .is_some_and(|primary| primary.kind != completions.kind)
    {
        usage = usage.with_extra_rate_window(
            "completions",
            "Completions",
            completions.to_rate_window(reset),
        );
    }

    usage.extra_rate_windows.extend(quotas.extra);

    // Absolute AI-credit consumption for token-billed seats, alongside the
    // windowed quotas when both exist (upstream attaches CopilotCreditsSnapshot
    // at snapshot level; locally an informational extra window keeps it on the
    // snapshot and exports — including diagnostics — without inventing a fake
    // quota denominator).
    if let Some(credits) = credits_used {
        usage = usage.with_extra_rate_window(
            "ai-credits",
            "AI credits",
            RateWindow::informational(format_credits_used(credits)),
        );
    }

    Ok(usage)
}

/// Render the absolute credits counter (whole numbers without decimals).
fn format_credits_used(credits: f64) -> String {
    let amount = if credits.fract() == 0.0 {
        format!("{credits:.0}")
    } else {
        format!("{credits:.2}")
    };
    format!("{amount} AI credits used")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopilotQuotaKind {
    Premium,
    Chat,
    Completions,
    Other,
}

/// Classify a quota entry by map key + quota_id (shared by window selection
/// and the credits-counter lookup).
fn classify_quota_kind(key: &str, quota_id: &str) -> CopilotQuotaKind {
    let key = key.to_ascii_lowercase();
    let id = quota_id.to_ascii_lowercase();
    if key.contains("chat") || id.contains("chat") {
        CopilotQuotaKind::Chat
    } else if key.contains("completion") || id.contains("completion") {
        CopilotQuotaKind::Completions
    } else if key.contains("premium") || id.contains("premium") || id.contains("interaction") {
        CopilotQuotaKind::Premium
    } else {
        CopilotQuotaKind::Other
    }
}

#[derive(Debug, Clone)]
struct UsableQuota {
    kind: CopilotQuotaKind,
    percent_remaining: f64,
    id: String,
    title: String,
}

impl UsableQuota {
    fn from_snapshot(key: &str, snapshot: QuotaSnapshot) -> Option<Self> {
        if snapshot.is_placeholder() {
            return None;
        }

        let percent_remaining = snapshot.percent_remaining.or_else(|| {
            let entitlement = snapshot.entitlement?;
            let remaining = snapshot.remaining?;
            if entitlement > 0.0 {
                Some(remaining / entitlement * 100.0)
            } else {
                None
            }
        })?;

        let quota_id = snapshot.quota_id.as_deref().unwrap_or_default();
        let key = key.to_ascii_lowercase();
        let kind = classify_quota_kind(&key, quota_id);

        Some(Self {
            kind,
            percent_remaining,
            id: quota_id_or_key(quota_id, &key),
            title: quota_title(quota_id, &key),
        })
    }

    fn from_limited(
        kind: CopilotQuotaKind,
        id: &str,
        title: &str,
        entitlement: Option<f64>,
        remaining: Option<f64>,
    ) -> Option<Self> {
        let entitlement = entitlement?;
        let remaining = remaining?;
        if entitlement <= 0.0 {
            return None;
        }

        Some(Self {
            kind,
            percent_remaining: remaining / entitlement * 100.0,
            id: id.to_string(),
            title: title.to_string(),
        })
    }

    fn to_rate_window(&self, reset: Option<DateTime<Utc>>) -> RateWindow {
        let used_percent = (100.0 - self.percent_remaining).max(0.0);
        let reset_description = (used_percent > 100.0).then(|| format!("{used_percent:.0}% used"));
        RateWindow {
            used_percent,
            window_minutes: None,
            resets_at: reset,
            reset_description,
            is_informational: false,
        }
    }
}

#[derive(Default)]
struct UsableQuotas {
    premium: Option<UsableQuota>,
    chat: Option<UsableQuota>,
    completions: Option<UsableQuota>,
    first: Option<UsableQuota>,
    extra: Vec<NamedRateWindow>,
}

impl CopilotUsageResponse {
    /// Absolute AI-credit counter for token-billed seats (upstream 0.48.0
    /// #2593/#2613): the first snapshot carrying `credits_used`, preferring
    /// premium- then chat-classified entries — the order upstream reads them
    /// in. Zero-entitlement *placeholder* snapshots still count: the absolute
    /// counter is real consumption data even when no percentage window can
    /// render (`carriesCreditsCounter`).
    fn credits_used_counter(&self) -> Option<f64> {
        let mut chat: Option<f64> = None;
        let mut first: Option<f64> = None;
        for (key, value) in &self.quota_snapshots.entries {
            let Ok(snapshot) = serde_json::from_value::<QuotaSnapshot>(value.clone()) else {
                continue;
            };
            let Some(credits) = snapshot.credits_used else {
                continue;
            };
            if !credits.is_finite() {
                continue;
            }
            match classify_quota_kind(key, snapshot.quota_id.as_deref().unwrap_or_default()) {
                CopilotQuotaKind::Premium => return Some(credits),
                CopilotQuotaKind::Chat => {
                    if chat.is_none() {
                        chat = Some(credits);
                    }
                }
                _ => {
                    if first.is_none() {
                        first = Some(credits);
                    }
                }
            }
        }
        chat.or(first)
    }

    fn usable_quotas(&self, reset: Option<DateTime<Utc>>) -> UsableQuotas {
        let mut quotas = UsableQuotas::default();

        for (key, value) in &self.quota_snapshots.entries {
            let Ok(snapshot) = serde_json::from_value::<QuotaSnapshot>(value.clone()) else {
                continue;
            };
            let Some(quota) = UsableQuota::from_snapshot(key, snapshot) else {
                continue;
            };

            match quota.kind {
                CopilotQuotaKind::Premium => {
                    if quotas.first.is_none() {
                        quotas.first = Some(quota.clone());
                    }
                    if quotas.premium.is_none() {
                        quotas.premium = Some(quota);
                    }
                }
                CopilotQuotaKind::Chat => {
                    if quotas.first.is_none() {
                        quotas.first = Some(quota.clone());
                    }
                    if quotas.chat.is_none() {
                        quotas.chat = Some(quota);
                    }
                }
                CopilotQuotaKind::Completions => {
                    if quotas.first.is_none() {
                        quotas.first = Some(quota.clone());
                    }
                    if quotas.completions.is_none() {
                        quotas.completions = Some(quota);
                    }
                }
                CopilotQuotaKind::Other => {
                    quotas.extra.push(NamedRateWindow::new(
                        quota.id.clone(),
                        quota.title.clone(),
                        quota.to_rate_window(reset),
                    ));
                }
            }
        }

        let completions = UsableQuota::from_limited(
            CopilotQuotaKind::Completions,
            "completions",
            "Completions",
            self.monthly_quotas.completions,
            self.limited_user_quotas.completions,
        );
        if quotas.completions.is_none() {
            quotas.completions = completions.clone();
        }
        if quotas.premium.is_none() {
            quotas.premium = completions;
        }

        let chat = UsableQuota::from_limited(
            CopilotQuotaKind::Chat,
            "chat",
            "Chat",
            self.monthly_quotas.chat,
            self.limited_user_quotas.chat,
        );
        if quotas.chat.is_none() {
            quotas.chat = chat;
        }

        if quotas.first.is_none() {
            quotas.first = quotas
                .premium
                .clone()
                .or_else(|| quotas.chat.clone())
                .or_else(|| quotas.completions.clone());
        }

        quotas
    }
}

impl QuotaSnapshot {
    fn is_placeholder(&self) -> bool {
        if self.placeholder {
            return true;
        }

        if self.entitlement == Some(0.0) && self.remaining == Some(0.0) {
            return true;
        }

        self.entitlement.unwrap_or_default() == 0.0
            && self.remaining.unwrap_or_default() == 0.0
            && self.percent_remaining.unwrap_or_default() == 0.0
            && self.quota_id.as_deref().unwrap_or_default().is_empty()
    }
}

// --- Helper functions ---

fn github_api_url(github_host: Option<&str>, path: &str) -> String {
    let host = normalized_api_host(github_host);
    format!("https://{host}{path}")
}

fn copilot_usage_url(github_host: Option<&str>) -> String {
    github_api_url(github_host, COPILOT_USAGE_PATH)
}

fn normalized_api_host(github_host: Option<&str>) -> String {
    let host = github_host
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .unwrap_or(DEFAULT_GITHUB_HOST)
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_ascii_lowercase();

    if host == DEFAULT_GITHUB_HOST || host == "api.github.com" {
        "api.github.com".to_string()
    } else if host.starts_with("api.") {
        host
    } else {
        format!("api.{host}")
    }
}

fn normalize_token(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        return None;
    }

    let token = if let Some((_user, pass)) = trimmed.split_once(':') {
        pass.trim()
    } else if trimmed.to_ascii_lowercase().starts_with("bearer ") {
        trimmed[7..].trim()
    } else if trimmed.to_ascii_lowercase().starts_with("token ") {
        trimmed[6..].trim()
    } else {
        trimmed
    };

    if !token.is_empty() && token.chars().all(|c| c.is_ascii_graphic()) {
        Some(token.to_string())
    } else {
        None
    }
}

fn load_gh_cli_token(github_host: Option<&str>) -> Option<String> {
    let host = github_host
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .unwrap_or(DEFAULT_GITHUB_HOST);
    let mut command = Command::new("gh");
    command.args(["auth", "token", "--hostname", host]);
    hide_windows_console(&mut command);
    let output = command.output().ok()?;

    if !output.status.success() {
        return None;
    }

    let token = String::from_utf8(output.stdout).ok()?;
    normalize_token(Some(&token))
}

#[cfg(windows)]
fn hide_windows_console(command: &mut Command) {
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_windows_console(_command: &mut Command) {}

fn parse_iso_date(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }

    if let Ok(dt) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(DateTime::from_naive_utc_and_offset(
            dt.and_hms_opt(0, 0, 0)?,
            Utc,
        ));
    }

    None
}

fn plan_label(plan: &str) -> String {
    let plan = plan.trim();
    if plan.is_empty() || plan.eq_ignore_ascii_case("unknown") {
        "GitHub Copilot".to_string()
    } else {
        format!("Copilot {}", capitalize(plan))
    }
}

fn quota_id_or_key(quota_id: &str, key: &str) -> String {
    let raw = if quota_id.is_empty() { key } else { quota_id };
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn quota_title(quota_id: &str, key: &str) -> String {
    let raw = if quota_id.is_empty() { key } else { quota_id };
    let words: Vec<String> = raw
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(capitalize)
        .collect();
    if words.is_empty() {
        "Additional Budget".to_string()
    } else {
        words.join(" ")
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

fn unknown_plan() -> String {
    "unknown".to_string()
}

fn deserialize_optional_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| match value {
        Value::Number(number) => number.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_snapshot(json: &str) -> UsageSnapshot {
        let response: CopilotUsageResponse = serde_json::from_str(json).unwrap();
        snapshot_from_response(response).unwrap()
    }

    fn parse_snapshot_result(json: &str) -> Result<UsageSnapshot, ProviderError> {
        let response: CopilotUsageResponse = serde_json::from_str(json).unwrap();
        snapshot_from_response(response)
    }

    #[test]
    fn paid_plan_parses_premium_and_chat_quotas() {
        let usage = parse_snapshot(
            r#"{
                "copilot_plan": "pro",
                "quota_reset_date": "2026-06-01",
                "quota_snapshots": {
                    "premium_interactions": {
                        "entitlement": 300,
                        "remaining": 240,
                        "percent_remaining": 80,
                        "quota_id": "premium_interactions"
                    },
                    "chat": {
                        "entitlement": 1000,
                        "remaining": 900,
                        "percent_remaining": 90,
                        "quota_id": "chat"
                    }
                }
            }"#,
        );

        assert_eq!(usage.login_method.as_deref(), Some("Copilot Pro"));
        assert!((usage.primary.used_percent - 20.0).abs() < 0.001);
        assert!((usage.secondary.unwrap().used_percent - 10.0).abs() < 0.001);
    }

    #[test]
    fn limited_user_quotas_parse_free_schema() {
        let usage = parse_snapshot(
            r#"{
                "copilot_plan": "free",
                "monthly_quotas": {
                    "completions": 2000,
                    "chat": "50"
                },
                "limited_user_quotas": {
                    "completions": "1000",
                    "chat": 10
                }
            }"#,
        );

        assert_eq!(usage.login_method.as_deref(), Some("Copilot Free"));
        assert!((usage.primary.used_percent - 50.0).abs() < 0.001);
        assert!((usage.secondary.unwrap().used_percent - 80.0).abs() < 0.001);
    }

    #[test]
    fn derives_missing_percent_and_accepts_numeric_strings() {
        let usage = parse_snapshot(
            r#"{
                "quota_snapshots": {
                    "premium_interactions": {
                        "entitlement": "100",
                        "remaining": "25",
                        "quota_id": "premium_interactions"
                    }
                }
            }"#,
        );

        assert!((usage.primary.used_percent - 75.0).abs() < 0.001);
    }

    #[test]
    fn ignores_placeholders_and_does_not_promote_chat_to_premium() {
        let usage = parse_snapshot(
            r#"{
                "quota_snapshots": {
                    "premium_interactions": {
                        "percent_remaining": 0,
                        "quota_id": ""
                    },
                    "chat": {
                        "entitlement": 100,
                        "remaining": 75,
                        "percent_remaining": 75,
                        "quota_id": "chat"
                    }
                }
            }"#,
        );

        assert!((usage.primary.used_percent - 25.0).abs() < 0.001);
        assert!(usage.secondary.is_none());
    }

    #[test]
    fn drops_business_token_billing_zero_entitlement_quotas() {
        let err = parse_snapshot_result(
            r#"{
                "copilot_plan": "business",
                "token_based_billing": true,
                "quota_snapshots": {
                    "premium_interactions": {
                        "entitlement": 0,
                        "remaining": 0,
                        "percent_remaining": 100,
                        "quota_id": "premium_interactions"
                    },
                    "chat": {
                        "entitlement": 0,
                        "remaining": 0,
                        "percent_remaining": 100,
                        "quota_id": "chat"
                    },
                    "completions": {
                        "entitlement": 0,
                        "remaining": 0,
                        "percent_remaining": 100,
                        "quota_id": "completions"
                    }
                }
            }"#,
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("token-based billing usage is unavailable")
        );
    }

    #[test]
    fn keeps_percent_only_quota_snapshots_available() {
        let usage = parse_snapshot(
            r#"{
                "copilot_plan": "business",
                "quota_snapshots": {
                    "chat": {
                        "percent_remaining": 40,
                        "quota_id": "chat"
                    }
                }
            }"#,
        );

        assert_eq!(usage.login_method.as_deref(), Some("Copilot Business"));
        assert!((usage.primary.used_percent - 60.0).abs() < 0.001);
        assert!(usage.secondary.is_none());
    }

    #[test]
    fn keeps_fully_consumed_positive_entitlement_quota() {
        let usage = parse_snapshot(
            r#"{
                "quota_snapshots": {
                    "premium_interactions": {
                        "entitlement": 500,
                        "remaining": 0,
                        "percent_remaining": 0,
                        "quota_id": "premium_interactions"
                    }
                }
            }"#,
        );

        assert!((usage.primary.used_percent - 100.0).abs() < 0.001);
    }

    #[test]
    fn keeps_additional_budget_as_extra_window() {
        let usage = parse_snapshot(
            r#"{
                "copilot_plan": "pro",
                "quota_snapshots": {
                    "premium_interactions": {
                        "entitlement": 500,
                        "remaining": 250,
                        "quota_id": "premium_interactions"
                    },
                    "additional_budget": {
                        "entitlement": 100,
                        "remaining": 25,
                        "quota_id": "additional_budget"
                    }
                }
            }"#,
        );

        assert!((usage.primary.used_percent - 50.0).abs() < 0.001);
        assert_eq!(usage.extra_rate_windows.len(), 1);
        assert_eq!(usage.extra_rate_windows[0].id, "additional-budget");
        assert_eq!(usage.extra_rate_windows[0].title, "Additional Budget");
        assert!((usage.extra_rate_windows[0].window.used_percent - 75.0).abs() < 0.001);
    }

    #[test]
    fn preserves_over_quota_percent_remaining() {
        let usage = parse_snapshot(
            r#"{
                "copilot_plan": "pro",
                "quota_snapshots": {
                    "premium_interactions": {
                        "entitlement": 500,
                        "remaining": -75,
                        "percent_remaining": -15,
                        "quota_id": "premium_interactions"
                    }
                }
            }"#,
        );

        assert_eq!(usage.login_method.as_deref(), Some("Copilot Pro"));
        assert!((usage.primary.used_percent - 115.0).abs() < 0.001);
        assert_eq!(
            usage.primary.reset_description.as_deref(),
            Some("115% used")
        );
        assert!(usage.primary.is_exhausted());
    }

    #[test]
    fn derives_over_quota_percent_from_negative_remaining() {
        let usage = parse_snapshot(
            r#"{
                "quota_snapshots": {
                    "chat": {
                        "entitlement": 500,
                        "remaining": -75,
                        "quota_id": "chat"
                    }
                }
            }"#,
        );

        assert!((usage.primary.used_percent - 115.0).abs() < 0.001);
        assert_eq!(
            usage.primary.reset_description.as_deref(),
            Some("115% used")
        );
    }

    #[test]
    fn normalizes_enterprise_hosts() {
        assert_eq!(
            normalized_api_host(Some("github.com")),
            "api.github.com".to_string()
        );
        assert_eq!(
            normalized_api_host(Some("github.example.com")),
            "api.github.example.com".to_string()
        );
        assert_eq!(
            normalized_api_host(Some("api.github.example.com")),
            "api.github.example.com".to_string()
        );
    }

    // ── A15: credits_used counter for token-billed seats (upstream #2613) ───

    #[test]
    fn decodes_credits_used_as_number_or_string() {
        let usage = parse_snapshot(
            r#"{
                "copilot_plan": "business",
                "quota_reset_date": "2026-06-01",
                "quota_snapshots": {
                    "premium_interactions": {
                        "entitlement": 300,
                        "remaining": 240,
                        "percent_remaining": 80,
                        "quota_id": "premium_interactions",
                        "credits_used": "1234.56"
                    }
                }
            }"#,
        );
        let extra = &usage.extra_rate_windows;
        assert!(
            extra.iter().any(|w| w.id == "ai-credits"
                && w.window.reset_description.as_deref() == Some("1234.56 AI credits used")),
            "{extra:?}"
        );
    }

    #[test]
    fn zero_entitlement_business_seat_surfaces_credits_counter() {
        let usage = parse_snapshot(
            r#"{
                "copilot_plan": "business",
                "token_based_billing": true,
                "quota_reset_date": "2026-06-01",
                "quota_snapshots": {
                    "premium_interactions": {
                        "entitlement": 0,
                        "remaining": 0,
                        "percent_remaining": 100,
                        "quota_id": "premium_interactions",
                        "credits_used": 1234
                    }
                }
            }"#,
        );
        // Not an error anymore: informational counter row without a fake bar.
        assert!(usage.primary.is_informational);
        assert_eq!(
            usage.primary.reset_description.as_deref(),
            Some("1234 AI credits used")
        );
        assert!(usage.primary.resets_at.is_some());
        assert_eq!(usage.login_method.as_deref(), Some("Copilot Business"));
    }

    #[test]
    fn placeholder_snapshot_still_carries_its_credits_counter() {
        // Upstream carriesCreditsCounter: a placeholder cannot become a window,
        // but its absolute counter is real consumption and must survive.
        let usage = parse_snapshot(
            r#"{
                "copilot_plan": "business",
                "token_based_billing": true,
                "quota_snapshots": {
                    "premium_interactions": {
                        "entitlement": 0,
                        "remaining": 0,
                        "percent_remaining": 0,
                        "quota_id": "",
                        "placeholder": true,
                        "credits_used": 42.5
                    }
                }
            }"#,
        );
        assert!(usage.primary.is_informational);
        assert_eq!(
            usage.primary.reset_description.as_deref(),
            Some("42.50 AI credits used")
        );
    }

    #[test]
    fn premium_credits_counter_wins_over_chat() {
        let usage = parse_snapshot(
            r#"{
                "copilot_plan": "pro",
                "quota_snapshots": {
                    "chat": {
                        "entitlement": 100,
                        "remaining": 75,
                        "percent_remaining": 75,
                        "quota_id": "chat",
                        "credits_used": 1
                    },
                    "premium_interactions": {
                        "entitlement": 300,
                        "remaining": 240,
                        "percent_remaining": 80,
                        "quota_id": "premium_interactions",
                        "credits_used": 7
                    }
                }
            }"#,
        );
        let credits_row = usage
            .extra_rate_windows
            .iter()
            .find(|w| w.id == "ai-credits")
            .expect("ai-credits row");
        assert_eq!(
            credits_row.window.reset_description.as_deref(),
            Some("7 AI credits used")
        );
        // Windows still render normally next to the counter.
        assert!(credits_row.window.is_informational);
        assert!((usage.primary.used_percent - 20.0).abs() < 0.001);
    }

    #[test]
    fn business_seat_without_credits_keeps_existing_error() {
        let err = parse_snapshot_result(
            r#"{
                "copilot_plan": "business",
                "token_based_billing": true
            }"#,
        );
        assert!(err.is_err());
    }
}
