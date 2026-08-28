//! Grok provider implementation.
//!
//! Uses the grok.com billing gRPC-web endpoint via either browser cookies or
//! `~/.grok/auth.json` produced by `grok login`.

pub mod local_sessions;

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use reqwest::Client;
use serde_json::Value;
use std::path::PathBuf;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};

const BILLING_ENDPOINT: &str = "https://grok.com/grok_api_v2.GrokBuildBilling/GetGrokCreditsConfig";
const CLI_SETTINGS_ENDPOINT: &str = "https://cli-chat-proxy.grok.com/v1/settings";

pub struct GrokProvider {
    metadata: ProviderMetadata,
    client: Client,
}

impl GrokProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Grok,
                display_name: "Grok",
                session_label: "Monthly",
                weekly_label: "On-demand",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://grok.com/?_s=usage"),
                status_page_url: Some("https://status.x.ai"),
            },
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    fn auth_file_path() -> Option<PathBuf> {
        if let Ok(home) = std::env::var("GROK_HOME")
            && !home.trim().is_empty()
        {
            return Some(PathBuf::from(home).join("auth.json"));
        }
        dirs::home_dir().map(|home| home.join(".grok").join("auth.json"))
    }

    fn load_credentials(kind: GrokAuthKind) -> Result<GrokCredentials, ProviderError> {
        let path = Self::auth_file_path()
            .ok_or_else(|| ProviderError::NotInstalled("Grok auth path not found".to_string()))?;
        let text = std::fs::read_to_string(&path).map_err(|_| {
            ProviderError::NotInstalled("Grok auth.json not found. Run `grok login`.".to_string())
        })?;
        GrokCredentials::parse_for_kind(&text, kind)
    }

    async fn fetch_with_auth(
        &self,
        credentials: &GrokCredentials,
        kind: GrokAuthKind,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let billing = self
            .fetch_billing(Some(format!("Bearer {}", credentials.access_token)), None)
            .await?;
        let plan = if kind == GrokAuthKind::Cli {
            self.fetch_cli_subscription_tier(credentials).await
        } else {
            None
        }
        .or_else(|| credentials.login_method());
        Ok(result_from_billing(
            billing,
            if kind == GrokAuthKind::Cli {
                "grok-cli"
            } else {
                "grok-oauth"
            },
            credentials.email.clone(),
            credentials.team_id.clone(),
            plan,
        ))
    }

    async fn fetch_cli_subscription_tier(&self, credentials: &GrokCredentials) -> Option<String> {
        let response = self
            .client
            .get(CLI_SETTINGS_ENDPOINT)
            .timeout(std::time::Duration::from_secs(2))
            .header(
                "Authorization",
                format!("Bearer {}", credentials.access_token),
            )
            .header("x-xai-token-auth", "xai-grok-cli")
            .header("Accept", "application/json")
            .header("User-Agent", "CodexBar")
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let value: Value = response.json().await.ok()?;
        grok_plan_display_name(
            value
                .get("subscription_tier_display")
                .and_then(Value::as_str),
        )
    }

    async fn fetch_with_cookie(
        &self,
        cookie_header: &str,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let billing = self
            .fetch_billing(None, Some(cookie_header.to_string()))
            .await?;
        // Upstream 0.52 (#2991): the browser billing response does not carry
        // the paid SuperGrok tier. If the local Grok principal is available,
        // use its settings endpoint only as identity enrichment, never as a
        // replacement for the validated browser usage result.
        let plan = match Self::load_credentials(GrokAuthKind::Cli) {
            Ok(credentials) => self.fetch_cli_subscription_tier(&credentials).await,
            Err(_) => None,
        };
        Ok(result_from_billing(
            billing,
            "grok-browser",
            None,
            None,
            plan,
        ))
    }

    /// Cookie refresh path (upstream #2458):
    /// 1. Try last validated cached cookie header (background reuse)
    /// 2. On miss/auth failure: re-import browser cookies, validate, cache
    async fn fetch_with_cookie_refresh(&self) -> Result<ProviderFetchResult, ProviderError> {
        use crate::browser::cookie_cache::CookieHeaderCache;

        if let Some(cached) = CookieHeaderCache::load(ProviderId::Grok) {
            match self.fetch_with_cookie(&cached.cookie_header).await {
                Ok(result) => return Ok(result),
                Err(err) if is_cookie_authentication_failure(&err) => {
                    CookieHeaderCache::clear(ProviderId::Grok);
                }
                Err(err) => return Err(err),
            }
        }

        let cookie_header = crate::providers::browser_cookie_header(&["grok.com"])?;
        let result = self.fetch_with_cookie(&cookie_header).await?;
        // Best-effort cache write: failing to persist the cookie only costs a
        // re-read from the browser on the next fetch.
        let _cached = CookieHeaderCache::store(ProviderId::Grok, &cookie_header, "browser");
        Ok(result)
    }

    async fn fetch_billing(
        &self,
        authorization: Option<String>,
        cookie_header: Option<String>,
    ) -> Result<GrokBillingSnapshot, ProviderError> {
        let mut request = self
            .client
            .post(BILLING_ENDPOINT)
            .body(vec![0, 0, 0, 0, 0])
            .header("Origin", "https://grok.com")
            .header("Referer", "https://grok.com/?_s=usage")
            .header("Accept", "*/*")
            .header("Content-Type", "application/grpc-web+proto")
            .header("x-grpc-web", "1")
            .header("x-user-agent", "connect-es/2.1.1")
            .header("User-Agent", "CodexBar");
        if let Some(auth) = authorization {
            request = request.header("Authorization", auth);
        }
        if let Some(cookie) = cookie_header {
            request = request.header("Cookie", cookie);
        }

        let response = request.send().await?;
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.bytes().await?;
        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                return Err(ProviderError::AuthRequired);
            }
            return Err(ProviderError::Other(format!(
                "Grok web billing returned status {status}"
            )));
        }
        validate_grpc_headers(&headers)?;
        parse_grpc_web_response(&bytes)
    }

    fn detect_cli_version() -> Option<String> {
        let mut command = std::process::Command::new("grok");
        command.arg("--version");
        hide_windows_console(&mut command);
        let output = command.output().ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        let trimmed = text
            .lines()
            .next()?
            .trim()
            .strip_prefix("grok ")
            .unwrap_or(text.trim());
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }
}

#[cfg(windows)]
fn hide_windows_console(command: &mut std::process::Command) {
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_windows_console(_command: &mut std::process::Command) {}

impl Default for GrokProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for GrokProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Grok
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto => {
                if let Some(token) = ctx.api_key.as_deref() {
                    let credentials = GrokCredentials::from_bearer(token);
                    return self
                        .fetch_with_auth(&credentials, GrokAuthKind::OAuth)
                        .await;
                }
                if let Some(cookie_header) = &ctx.manual_cookie_header {
                    return self.fetch_with_cookie(cookie_header).await;
                }
                for kind in [GrokAuthKind::Cli, GrokAuthKind::OAuth] {
                    if let Ok(credentials) = Self::load_credentials(kind) {
                        match self.fetch_with_auth(&credentials, kind).await {
                            Ok(result) => return Ok(result),
                            Err(ProviderError::AuthRequired) => {}
                            Err(error) => {
                                tracing::debug!("Grok login path failed in Auto: {error}")
                            }
                        }
                    }
                }
                self.fetch_with_cookie_refresh().await
            }
            SourceMode::Web => {
                if let Some(cookie_header) = &ctx.manual_cookie_header {
                    return self.fetch_with_cookie(cookie_header).await;
                }
                self.fetch_with_cookie_refresh().await
            }
            SourceMode::Cli => {
                let credentials = Self::load_credentials(GrokAuthKind::Cli)?;
                self.fetch_with_auth(&credentials, GrokAuthKind::Cli).await
            }
            SourceMode::OAuth => {
                let credentials = if let Some(token) = ctx.api_key.as_deref() {
                    GrokCredentials::from_bearer(token)
                } else {
                    Self::load_credentials(GrokAuthKind::OAuth)?
                };
                self.fetch_with_auth(&credentials, GrokAuthKind::OAuth)
                    .await
            }
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![
            SourceMode::Auto,
            SourceMode::Cli,
            SourceMode::OAuth,
            SourceMode::Web,
        ]
    }

    fn supports_web(&self) -> bool {
        true
    }

    fn supports_cli(&self) -> bool {
        true
    }

    fn detect_version(&self) -> Option<String> {
        Self::detect_cli_version()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GrokAuthKind {
    Cli,
    OAuth,
}

#[derive(Debug, Clone)]
struct GrokCredentials {
    access_token: String,
    auth_mode: Option<String>,
    email: Option<String>,
    team_id: Option<String>,
    expires_at: Option<DateTime<Utc>>,
}

impl GrokCredentials {
    fn from_bearer(token: &str) -> Self {
        Self {
            access_token: token.trim().to_string(),
            auth_mode: Some("oidc".into()),
            email: None,
            team_id: None,
            expires_at: None,
        }
    }

    fn parse_for_kind(text: &str, kind: GrokAuthKind) -> Result<Self, ProviderError> {
        let root: Value = serde_json::from_str(text)
            .map_err(|e| ProviderError::Parse(format!("Failed to decode Grok auth.json: {e}")))?;
        let map = root
            .as_object()
            .ok_or_else(|| ProviderError::Parse("Invalid Grok auth.json".to_string()))?;
        let selected = map.iter().find(|(scope, entry)| {
            let has_key = entry
                .get("key")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty());
            if !has_key {
                return false;
            }
            let is_oauth = scope.starts_with("https://auth.x.ai::")
                || entry
                    .get("auth_mode")
                    .and_then(Value::as_str)
                    .is_some_and(|mode| mode.eq_ignore_ascii_case("oidc"));
            match kind {
                GrokAuthKind::Cli => !is_oauth,
                GrokAuthKind::OAuth => is_oauth,
            }
        });
        let (_, entry) = selected.ok_or(ProviderError::AuthRequired)?;
        let access_token = entry
            .get("key")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or(ProviderError::AuthRequired)?
            .to_string();
        let expires_at = entry
            .get("expires_at")
            .and_then(Value::as_str)
            .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
            .map(|dt| dt.with_timezone(&Utc));
        if expires_at.is_some_and(|dt| dt <= Utc::now()) {
            return Err(ProviderError::AuthRequired);
        }
        Ok(Self {
            access_token,
            auth_mode: text_field(entry, "auth_mode"),
            email: text_field(entry, "email"),
            team_id: text_field(entry, "team_id"),
            expires_at,
        })
    }

    fn login_method(&self) -> Option<String> {
        match self.auth_mode.as_deref().map(str::to_lowercase).as_deref() {
            Some("oidc") => Some("SuperGrok".to_string()),
            Some("session") => Some("session".to_string()),
            Some(other) => Some(other.to_string()),
            None if self.expires_at.is_some() => Some("Grok".to_string()),
            None => None,
        }
    }
}

fn grok_plan_display_name(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        return None;
    }
    let compact: String = trimmed
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphabetic())
        .collect();
    Some(match compact.as_str() {
        "supergrokheavy" | "heavy" => "SuperGrok Heavy".to_string(),
        "supergrok" => "SuperGrok".to_string(),
        _ => trimmed.to_string(),
    })
}

fn text_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

#[derive(Debug, Clone, Copy)]
struct GrokBillingSnapshot {
    used_percent: Option<f64>,
    resets_at: Option<DateTime<Utc>>,
}

fn result_from_billing(
    billing: GrokBillingSnapshot,
    source_label: &str,
    email: Option<String>,
    team_id: Option<String>,
    login_method: Option<String>,
) -> ProviderFetchResult {
    // Upstream #2431 / #2566: do not infer windowMinutes from time-until-reset.
    // A monthly quota near its reset would otherwise be misclassified as weekly.
    let primary = match billing.used_percent {
        Some(used_percent) => RateWindow::with_details(used_percent, None, billing.resets_at, None),
        None => {
            let mut window = RateWindow::informational("Usage unavailable");
            window.resets_at = billing.resets_at;
            window
        }
    };
    let mut usage = UsageSnapshot::new(primary);
    usage.account_email = email;
    usage.account_organization = team_id;
    usage.login_method = login_method;
    ProviderFetchResult::new(usage, source_label)
}

fn validate_grpc_headers(headers: &reqwest::header::HeaderMap) -> Result<(), ProviderError> {
    if let Some(status) = headers
        .get("grpc-status")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u16>().ok())
        && status != 0
    {
        if status == 16 {
            return Err(ProviderError::AuthRequired);
        }
        return Err(ProviderError::Other(format!(
            "Grok RPC failed with status {status}"
        )));
    }
    Ok(())
}

/// Whether a cookie-path error should invalidate the cached browser session.
fn is_cookie_authentication_failure(err: &ProviderError) -> bool {
    matches!(err, ProviderError::AuthRequired)
}

/// Decide the next cookie-refresh step given cache presence and last error.
/// Pure helper for unit tests of the #2458 refresh flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CookieRefreshAction {
    UseCached,
    ReimportBrowser,
    GiveUp,
}

fn cookie_refresh_action(
    has_cached_header: bool,
    last_error: Option<&ProviderError>,
) -> CookieRefreshAction {
    match last_error {
        None if has_cached_header => CookieRefreshAction::UseCached,
        None => CookieRefreshAction::ReimportBrowser,
        Some(err) if is_cookie_authentication_failure(err) => CookieRefreshAction::ReimportBrowser,
        Some(ProviderError::NoCookies) => CookieRefreshAction::ReimportBrowser,
        Some(_) => CookieRefreshAction::GiveUp,
    }
}

fn parse_grpc_web_response(data: &[u8]) -> Result<GrokBillingSnapshot, ProviderError> {
    let frames = grpc_web_data_frames(data);
    if frames.is_empty() {
        return Err(ProviderError::Parse(
            "Grok web billing returned no payload".to_string(),
        ));
    }
    let mut scan = ProtoScan::default();
    for frame in frames {
        scan.scan_message(&frame, &mut Vec::new(), 0);
    }
    let used_percent = scan
        .fixed32
        .iter()
        .filter(|field| {
            field.path.last() == Some(&1)
                && field.value.is_finite()
                && field.value >= 0.0
                && field.value <= 100.0
        })
        .min_by(|a, b| {
            a.path
                .len()
                .cmp(&b.path.len())
                .then_with(|| a.order.cmp(&b.order))
        })
        .map(|field| field.value as f64);

    let resets_at = scan
        .varints
        .iter()
        .filter_map(|field| {
            // Varint timestamps are Unix seconds inside the range checked below.
            #[allow(
                clippy::cast_possible_wrap,
                reason = "varint timestamps are bounded to the Unix-seconds range checked below"
            )]
            let seconds = field.value as i64;
            (1_700_000_000..=2_100_000_000)
                .contains(&field.value)
                .then(|| Utc.timestamp_opt(seconds, 0).single())
                .flatten()
        })
        .filter(|dt| *dt > Utc::now())
        .min();
    Ok(GrokBillingSnapshot {
        used_percent,
        resets_at,
    })
}

fn grpc_web_data_frames(data: &[u8]) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    let mut index = 0;
    while index + 5 <= data.len() {
        let flags = data[index];
        let len = ((data[index + 1] as usize) << 24)
            | ((data[index + 2] as usize) << 16)
            | ((data[index + 3] as usize) << 8)
            | (data[index + 4] as usize);
        let start = index + 5;
        let end = start.saturating_add(len);
        if end > data.len() {
            break;
        }
        if flags & 0x80 == 0 {
            frames.push(data[start..end].to_vec());
        }
        index = end;
    }
    frames
}

#[derive(Default)]
struct ProtoScan {
    fixed32: Vec<Fixed32Field>,
    varints: Vec<VarintField>,
    order: usize,
}

struct Fixed32Field {
    path: Vec<u64>,
    value: f32,
    order: usize,
}

struct VarintField {
    value: u64,
}

impl ProtoScan {
    fn scan_message(&mut self, data: &[u8], path: &mut Vec<u64>, depth: usize) {
        if depth > 8 {
            return;
        }
        let mut i = 0;
        while i < data.len() {
            let Some((field, wire, next)) = read_key(data, i) else {
                break;
            };
            i = next;
            path.push(field);
            let Some(next) = self.scan_field(data, i, path, depth, wire) else {
                path.pop();
                break;
            };
            i = next;
            path.pop();
        }
    }

    fn scan_field(
        &mut self,
        data: &[u8],
        i: usize,
        path: &mut Vec<u64>,
        depth: usize,
        wire: u64,
    ) -> Option<usize> {
        match wire {
            0 => self.scan_varint(data, i),
            2 => self.scan_length_delimited(data, i, path, depth),
            5 => self.scan_fixed32(data, i, path),
            1 => Some(i.saturating_add(8)),
            _ => None,
        }
    }

    fn scan_varint(&mut self, data: &[u8], i: usize) -> Option<usize> {
        let (value, next) = read_varint(data, i)?;
        self.varints.push(VarintField { value });
        Some(next)
    }

    fn scan_length_delimited(
        &mut self,
        data: &[u8],
        i: usize,
        path: &mut Vec<u64>,
        depth: usize,
    ) -> Option<usize> {
        let (len, next) = read_varint(data, i)?;
        let start = next;
        // Varint field lengths are bounded by the containing message buffer.
        #[allow(
            clippy::cast_possible_truncation,
            reason = "varint field lengths are bounded by the containing message buffer"
        )]
        let len_usize = len as usize;
        let end = start.saturating_add(len_usize);
        if end <= data.len() {
            self.scan_message(&data[start..end], path, depth + 1);
            Some(end)
        } else {
            None
        }
    }

    fn scan_fixed32(&mut self, data: &[u8], i: usize, path: &[u64]) -> Option<usize> {
        if i + 4 > data.len() {
            return None;
        }
        let bytes = [data[i], data[i + 1], data[i + 2], data[i + 3]];
        self.fixed32.push(Fixed32Field {
            path: path.to_vec(),
            value: f32::from_le_bytes(bytes),
            order: self.order,
        });
        self.order += 1;
        Some(i + 4)
    }
}

fn read_key(data: &[u8], i: usize) -> Option<(u64, u64, usize)> {
    let (key, next) = read_varint(data, i)?;
    Some((key >> 3, key & 0x07, next))
}

fn read_varint(data: &[u8], mut i: usize) -> Option<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0;
    while i < data.len() && shift < 64 {
        let b = data[i];
        i += 1;
        value |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some((value, i));
        }
        shift += 7;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grok_plan_prefers_subscription_tier_display_names() {
        assert_eq!(
            grok_plan_display_name(Some("SuperGrok Heavy")),
            Some("SuperGrok Heavy".to_string())
        );
        assert_eq!(
            grok_plan_display_name(Some("heavy")),
            Some("SuperGrok Heavy".to_string())
        );
        assert_eq!(
            grok_plan_display_name(Some("SuperGrok")),
            Some("SuperGrok".to_string())
        );
        assert_eq!(
            grok_plan_display_name(Some(" custom ")),
            Some("custom".to_string())
        );
    }

    #[test]
    fn parses_auth_file_prefer_oidc() {
        let auth = r#"{
          "https://accounts.x.ai/sign-in": {"key": "legacy"},
          "https://auth.x.ai::abc": {"key": "oidc", "auth_mode": "oidc", "email": "u@example.com"}
        }"#;
        let parsed = GrokCredentials::parse_for_kind(auth, GrokAuthKind::OAuth).unwrap();
        assert_eq!(parsed.access_token, "oidc");
        assert_eq!(parsed.login_method().as_deref(), Some("SuperGrok"));
    }

    #[test]
    fn cli_and_oauth_select_distinct_auth_entries() {
        let auth = r#"{
          "https://accounts.x.ai/sign-in": {"key": "cli-token", "auth_mode": "session"},
          "https://auth.x.ai::abc": {"key": "oauth-token", "auth_mode": "oidc"}
        }"#;
        assert_eq!(
            GrokCredentials::parse_for_kind(auth, GrokAuthKind::Cli)
                .unwrap()
                .access_token,
            "cli-token"
        );
        assert_eq!(
            GrokCredentials::parse_for_kind(auth, GrokAuthKind::OAuth)
                .unwrap()
                .access_token,
            "oauth-token"
        );
    }
    #[test]
    fn splits_grpc_web_data_frames() {
        let data = [0, 0, 0, 0, 2, 1, 2, 0x80, 0, 0, 0, 1, b'x'];
        assert_eq!(grpc_web_data_frames(&data), vec![vec![1, 2]]);
    }

    #[test]
    fn cookie_refresh_uses_cache_when_present() {
        assert_eq!(
            cookie_refresh_action(true, None),
            CookieRefreshAction::UseCached
        );
    }

    #[test]
    fn cookie_refresh_reimports_on_auth_failure() {
        assert_eq!(
            cookie_refresh_action(true, Some(&ProviderError::AuthRequired)),
            CookieRefreshAction::ReimportBrowser
        );
        assert_eq!(
            cookie_refresh_action(false, None),
            CookieRefreshAction::ReimportBrowser
        );
    }

    #[test]
    fn cookie_refresh_gives_up_on_non_auth_errors() {
        assert_eq!(
            cookie_refresh_action(true, Some(&ProviderError::Other("network down".into()))),
            CookieRefreshAction::GiveUp
        );
    }

    #[test]
    fn is_cookie_auth_failure_only_auth_required() {
        assert!(is_cookie_authentication_failure(
            &ProviderError::AuthRequired
        ));
        assert!(!is_cookie_authentication_failure(&ProviderError::NoCookies));
    }

    #[test]
    fn billing_snapshot_leaves_window_minutes_unset() {
        // A monthly quota with six days left must not be reported as weekly.
        let resets = Utc::now() + chrono::Duration::days(6);
        let result = result_from_billing(
            GrokBillingSnapshot {
                used_percent: Some(12.0),
                resets_at: Some(resets),
            },
            "web",
            None,
            None,
            Some("SuperGrok".into()),
        );
        assert_eq!(result.usage.primary.window_minutes, None);
        assert_eq!(result.usage.primary.resets_at, Some(resets));
        assert_eq!(result.usage.login_method.as_deref(), Some("SuperGrok"));
    }

    #[test]
    fn period_only_billing_is_informational_not_zero_usage() {
        let resets = Utc::now() + chrono::Duration::days(6);
        let result = result_from_billing(
            GrokBillingSnapshot {
                used_percent: None,
                resets_at: Some(resets),
            },
            "cli",
            Some("user@example.com".into()),
            None,
            Some("SuperGrok Heavy".into()),
        );

        assert!(result.usage.primary.is_informational);
        assert_eq!(result.usage.primary.resets_at, Some(resets));
        assert_eq!(
            result.usage.account_email.as_deref(),
            Some("user@example.com")
        );
        assert_eq!(
            result.usage.login_method.as_deref(),
            Some("SuperGrok Heavy")
        );
    }
}
