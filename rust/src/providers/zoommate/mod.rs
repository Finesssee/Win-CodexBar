//! ZoomMate provider (upstream 0.46.0).
//!
//! Auth: manual Cookie header **or** DevTools cURL of credits/status (must include
//! `Authorization: Bearer …`), else browser cookies → mint bearer via login bootstrap.
//! Bearer JWTs are cached in-process by cookie fingerprint until `exp - 60s`.
//!
//! Optional credits/history (Today KPI) is intentionally skipped for the Windows v1
//! port — primary credits/status is sufficient; history can land later without
//! blocking the snapshot.

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::core::curl_capture;
use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};
use crate::providers::browser_cookie_header;

const API_HOSTS: &[&str] = &["ai.zoom.us", "zoommate.zoom.us"];
const COOKIE_DOMAINS: &[&str] = &["ai.zoom.us", "zoommate.zoom.us", "zoom.us"];
const CREDITS_STATUS_PATH: &str = "/ai-computer/api/v1/credits/status";
const ORIGIN_REFERER: &str = "https://zoommate.zoom.us";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";
const BEARER_REFRESH_SKEW_SECS: u64 = 60;

const FORWARDED_MANUAL_HEADERS: &[(&str, &str)] = &[
    ("authorization", "Authorization"),
    ("cookie", "Cookie"),
    ("origin", "Origin"),
    ("referer", "Referer"),
    ("user-agent", "User-Agent"),
    ("accept", "Accept"),
    ("accept-language", "Accept-Language"),
];

// ── Response shapes ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default, Clone)]
struct CreditsStatusEnvelope {
    data: Option<CreditsDataBox>,
    #[serde(default)]
    status_code: Option<i64>,
    #[serde(default)]
    error_message: Option<String>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct CreditsDataBox {
    credit_status: Option<CreditStatus>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub(crate) struct CreditStatus {
    budget_cap: Option<f64>,
    used_credit: Option<f64>,
    remaining_credit: Option<f64>,
    overage_credit: Option<f64>,
    allow_overage: Option<bool>,
    cycle_start_date: Option<i64>,
    cycle_end_date: Option<i64>,
    is_quota_available: Option<bool>,
    is_unlimited: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
struct LoginBootstrapEnvelope {
    success: Option<bool>,
    data: Option<LoginDataBox>,
}

#[derive(Debug, Deserialize, Default)]
struct LoginDataBox {
    nak: Option<String>,
    user_profile: Option<LoginUserProfile>,
}

#[derive(Debug, Deserialize, Default)]
struct LoginUserProfile {
    email: Option<String>,
}

// ── Request context ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct RequestContext {
    authorization: String,
    headers: HashMap<String, String>,
    /// Host → Cookie header. Empty map / missing host → no Cookie sent for that host.
    cookie_by_host: HashMap<String, String>,
    preferred_host: Option<String>,
    account_email: Option<String>,
    /// Present only on cookie-mint path so 401 can invalidate the bearer cache entry.
    cache_key: Option<String>,
}

#[derive(Debug, Clone)]
struct MintedToken {
    bearer_token: String,
    account_email: Option<String>,
}

// ── In-process bearer cache ──────────────────────────────────────────────────

struct BearerEntry {
    token: String,
    account_email: Option<String>,
    /// UNIX seconds of JWT `exp`.
    exp_unix: u64,
}

fn bearer_cache() -> &'static Mutex<HashMap<String, BearerEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<String, BearerEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cookie_fingerprint(cookie_by_host: &HashMap<String, String>) -> String {
    let mut pairs: Vec<_> = cookie_by_host.iter().collect();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    let canonical = pairs
        .into_iter()
        .map(|(h, c)| format!("{h}={c}"))
        .collect::<Vec<_>>()
        .join("\n");
    let digest = Sha256::digest(canonical.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn cache_valid_entry(key: &str) -> Option<MintedToken> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let mut guard = bearer_cache().lock().ok()?;
    let entry = guard.get(key)?;
    if entry.exp_unix.saturating_sub(BEARER_REFRESH_SKEW_SECS) <= now {
        guard.remove(key);
        return None;
    }
    Some(MintedToken {
        bearer_token: entry.token.clone(),
        account_email: entry.account_email.clone(),
    })
}

fn cache_store(key: String, minted: &MintedToken, exp_unix: u64) {
    if let Ok(mut guard) = bearer_cache().lock() {
        guard.insert(
            key,
            BearerEntry {
                token: minted.bearer_token.clone(),
                account_email: minted.account_email.clone(),
                exp_unix,
            },
        );
    }
}

fn cache_invalidate(key: &str) {
    if let Ok(mut guard) = bearer_cache().lock() {
        guard.remove(key);
    }
}

// ── Provider ─────────────────────────────────────────────────────────────────

pub struct ZoomMateProvider {
    metadata: ProviderMetadata,
    client: Client,
}

impl ZoomMateProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::ZoomMate,
                display_name: "ZoomMate",
                session_label: "Credits",
                weekly_label: "Credits",
                supports_opus: false,
                supports_credits: true,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://zoommate.zoom.us/#/?settings=credit-usage"),
                status_page_url: Some("https://www.zoomstatus.com/"),
            },
            client: crate::core::credentialed_http_client_builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    async fn fetch_via_web(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProviderError> {
        let request_context = self.resolve_request_context(ctx).await?;
        match self
            .fetch_credits_status(&request_context, ctx.web_timeout)
            .await
        {
            Ok(snap) => Ok(snap),
            Err(ProviderError::AuthRequired) => {
                if let Some(key) = request_context.cache_key.as_deref() {
                    cache_invalidate(key);
                }
                Err(ProviderError::AuthRequired)
            }
            Err(e) => Err(e),
        }
    }

    async fn resolve_request_context(
        &self,
        ctx: &FetchContext,
    ) -> Result<RequestContext, ProviderError> {
        // 1) Manual paste: cURL capture (bearer) or bare Cookie header.
        if let Some(raw) = ctx.manual_cookie_header.as_deref() {
            let raw = raw.trim();
            if !raw.is_empty() {
                if let Some(ctx) = request_context_from_manual(raw) {
                    return Ok(ctx);
                }
                // Bare cookie header → mint bearer.
                if !curl_capture::looks_like_curl_capture(raw) {
                    let mut cookie_by_host = HashMap::new();
                    for host in API_HOSTS {
                        cookie_by_host.insert((*host).to_string(), raw.to_string());
                    }
                    return self
                        .request_context_from_cookies(cookie_by_host, ctx.web_timeout)
                        .await;
                }
                return Err(ProviderError::Other(
                    "Paste a cURL capture of the HTTPS ZoomMate credits/status request \
                     (from ai.zoom.us or zoommate.zoom.us) including Authorization: Bearer …, \
                     or paste a Cookie header."
                        .into(),
                ));
            }
        }

        // 2) Browser cookies → mint bearer.
        let cookie_header = browser_cookie_header(COOKIE_DOMAINS)?;
        let trimmed = cookie_header.trim();
        if trimmed.is_empty() {
            return Err(ProviderError::NoCookies);
        }
        let mut cookie_by_host = HashMap::new();
        for host in API_HOSTS {
            cookie_by_host.insert((*host).to_string(), trimmed.to_string());
        }
        self.request_context_from_cookies(cookie_by_host, ctx.web_timeout)
            .await
    }

    async fn request_context_from_cookies(
        &self,
        cookie_by_host: HashMap<String, String>,
        timeout_secs: u64,
    ) -> Result<RequestContext, ProviderError> {
        let cache_key = cookie_fingerprint(&cookie_by_host);
        let minted = if let Some(entry) = cache_valid_entry(&cache_key) {
            entry
        } else {
            let minted = self
                .mint_bearer_token(&cookie_by_host, timeout_secs)
                .await?;
            if let Some(exp) = jwt_exp_unix(&minted.bearer_token) {
                cache_store(cache_key.clone(), &minted, exp);
            }
            minted
        };
        Ok(RequestContext {
            authorization: bearer_header_value(&minted.bearer_token),
            headers: HashMap::new(),
            cookie_by_host,
            preferred_host: None,
            account_email: minted.account_email,
            cache_key: Some(cache_key),
        })
    }

    async fn mint_bearer_token(
        &self,
        cookie_by_host: &HashMap<String, String>,
        timeout_secs: u64,
    ) -> Result<MintedToken, ProviderError> {
        with_api_host_failover(None, |host| async move {
            self.mint_bearer_token_on_host(cookie_by_host, host, timeout_secs)
                .await
        })
        .await
    }

    async fn mint_bearer_token_on_host(
        &self,
        cookie_by_host: &HashMap<String, String>,
        host: &str,
        timeout_secs: u64,
    ) -> Result<MintedToken, ProviderError> {
        let url =
            format!("https://{host}/ai-computer/api/v1/login/?continue=https://zoommate.zoom.us/");
        let mut req = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(timeout_secs.max(1)))
            .header("Accept", "application/json, text/plain, */*")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("User-Agent", USER_AGENT)
            .header("Origin", ORIGIN_REFERER)
            .header("Referer", ORIGIN_REFERER)
            .header("Sec-Fetch-Dest", "empty")
            .header("Sec-Fetch-Mode", "cors")
            .header("Sec-Fetch-Site", "same-site");
        if let Some(cookie) = cookie_by_host
            .get(host)
            .or_else(|| cookie_by_host.values().next())
        {
            req = req.header("Cookie", cookie);
        }
        let resp = req.send().await?;
        let status = resp.status();
        let body = resp.bytes().await?;
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ProviderError::AuthRequired);
        }
        if !status.is_success() {
            return Err(ProviderError::Other(format!(
                "ZoomMate login bootstrap error: HTTP {status}"
            )));
        }
        let envelope: LoginBootstrapEnvelope = serde_json::from_slice(&body).map_err(|e| {
            ProviderError::Parse(format!("ZoomMate login bootstrap parse failed: {e}"))
        })?;
        let nak = envelope
            .data
            .as_ref()
            .and_then(|d| d.nak.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ProviderError::Parse("Missing nak in ZoomMate login bootstrap response.".into())
            })?;
        let email = envelope
            .data
            .as_ref()
            .and_then(|d| d.user_profile.as_ref())
            .and_then(|p| p.email.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        Ok(MintedToken {
            bearer_token: nak.to_string(),
            account_email: email,
        })
    }

    async fn fetch_credits_status(
        &self,
        ctx: &RequestContext,
        timeout_secs: u64,
    ) -> Result<UsageSnapshot, ProviderError> {
        let preferred = ctx.preferred_host.clone();
        let authorization = ctx.authorization.clone();
        let headers = ctx.headers.clone();
        let cookie_by_host = ctx.cookie_by_host.clone();
        let account_email = ctx.account_email.clone();

        with_api_host_failover(preferred.as_deref(), |host| {
            let authorization = authorization.clone();
            let headers = headers.clone();
            let cookie_by_host = cookie_by_host.clone();
            let account_email = account_email.clone();
            async move {
                self.fetch_credits_status_on_host(
                    host,
                    &authorization,
                    &headers,
                    &cookie_by_host,
                    account_email.as_deref(),
                    timeout_secs,
                )
                .await
            }
        })
        .await
    }

    async fn fetch_credits_status_on_host(
        &self,
        host: &str,
        authorization: &str,
        headers: &HashMap<String, String>,
        cookie_by_host: &HashMap<String, String>,
        account_email: Option<&str>,
        timeout_secs: u64,
    ) -> Result<UsageSnapshot, ProviderError> {
        let url = format!("https://{host}{CREDITS_STATUS_PATH}");
        let mut req = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(timeout_secs.max(1)))
            .header("Accept", "application/json, text/plain, */*")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("User-Agent", USER_AGENT)
            .header("Sec-Fetch-Dest", "empty")
            .header("Sec-Fetch-Mode", "cors")
            .header("Sec-Fetch-Site", "same-site");

        // Captured headers first (except Origin/Referer/Authorization which we pin).
        for (name, value) in headers {
            if name.eq_ignore_ascii_case("origin")
                || name.eq_ignore_ascii_case("referer")
                || name.eq_ignore_ascii_case("authorization")
                || name.eq_ignore_ascii_case("cookie")
            {
                continue;
            }
            req = req.header(name.as_str(), value.as_str());
        }
        if let Some(cookie) = cookie_by_host
            .get(host)
            .or_else(|| cookie_by_host.values().next())
        {
            req = req.header("Cookie", cookie);
        }
        // Fixed Origin/Referer so captured values never widen the first-party boundary.
        req = req
            .header("Authorization", authorization)
            .header("Origin", ORIGIN_REFERER)
            .header("Referer", ORIGIN_REFERER);

        let resp = req.send().await?;
        let status = resp.status();
        let body = resp.bytes().await?;
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ProviderError::AuthRequired);
        }
        if !status.is_success() {
            return Err(ProviderError::Other(format!(
                "ZoomMate API error: HTTP {status}"
            )));
        }

        let envelope: CreditsStatusEnvelope = serde_json::from_slice(&body).map_err(|e| {
            ProviderError::Parse(format!("ZoomMate credits/status parse failed: {e}"))
        })?;
        let credit_status = envelope
            .data
            .and_then(|d| d.credit_status)
            .ok_or_else(|| ProviderError::Parse("Missing credit_status object.".into()))?;

        Ok(snapshot_from_credit_status(
            &credit_status,
            account_email,
            Utc::now(),
        ))
    }
}

impl Default for ZoomMateProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for ZoomMateProvider {
    fn id(&self) -> ProviderId {
        ProviderId::ZoomMate
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto | SourceMode::Web => {
                let usage = self.fetch_via_web(ctx).await?;
                Ok(ProviderFetchResult::new(usage, "web"))
            }
            SourceMode::OAuth | SourceMode::Cli => {
                Err(ProviderError::UnsupportedSource(ctx.source_mode))
            }
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::Web]
    }

    fn supports_web(&self) -> bool {
        true
    }
}

// ── Pure helpers ─────────────────────────────────────────────────────────────

fn bearer_header_value(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() >= 7 && trimmed[..7].eq_ignore_ascii_case("bearer ") {
        trimmed.to_string()
    } else {
        format!("Bearer {trimmed}")
    }
}

fn jwt_exp_unix(token: &str) -> Option<u64> {
    let raw = bearer_header_value(token);
    let token = raw.strip_prefix("Bearer ").unwrap_or(raw.as_str());
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload_b64 = parts.next()?;
    let json = base64url_decode(payload_b64)?;
    let value: serde_json::Value = serde_json::from_slice(&json).ok()?;
    let exp = value.get("exp")?.as_u64().or_else(|| {
        value
            .get("exp")?
            .as_f64()
            .filter(|f| f.is_finite() && *f > 0.0)
            .map(|f| f as u64)
    })?;
    (exp > 0).then_some(exp)
}

fn base64url_decode(value: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let mut s = value.replace('-', "+").replace('_', "/");
    let pad = (4 - s.len() % 4) % 4;
    s.push_str(&"=".repeat(pad));
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

fn date_from_millis(raw: Option<i64>) -> Option<DateTime<Utc>> {
    let ms = raw.filter(|&v| v > 0)?;
    Utc.timestamp_millis_opt(ms).single()
}

fn hosts_preferred(preferred: Option<&str>) -> Vec<&'static str> {
    let preferred = preferred.map(|h| h.to_ascii_lowercase());
    match preferred.as_deref() {
        Some(h) if API_HOSTS.iter().any(|x| x.eq_ignore_ascii_case(h)) => {
            let mut out = vec![
                API_HOSTS
                    .iter()
                    .copied()
                    .find(|x| x.eq_ignore_ascii_case(h))
                    .unwrap_or(API_HOSTS[0]),
            ];
            for host in API_HOSTS {
                if !host.eq_ignore_ascii_case(h) {
                    out.push(*host);
                }
            }
            out
        }
        _ => API_HOSTS.to_vec(),
    }
}

/// Runs `operation` per host. Auth (401/403 → AuthRequired) and parse errors do
/// **not** fail over; other errors try the next host.
async fn with_api_host_failover<T, F, Fut>(
    preferred: Option<&str>,
    mut operation: F,
) -> Result<T, ProviderError>
where
    F: FnMut(&'static str) -> Fut,
    Fut: std::future::Future<Output = Result<T, ProviderError>>,
{
    let hosts = hosts_preferred(preferred);
    let mut last_err: Option<ProviderError> = None;
    for (index, host) in hosts.iter().enumerate() {
        match operation(host).await {
            Ok(v) => return Ok(v),
            Err(ProviderError::AuthRequired) => return Err(ProviderError::AuthRequired),
            Err(ProviderError::Parse(msg)) => return Err(ProviderError::Parse(msg)),
            Err(e) => {
                last_err = Some(e);
                if index + 1 < hosts.len() {
                    tracing::info!("ZoomMate API host unavailable; retrying on the alternate host");
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| ProviderError::Other("No ZoomMate API host succeeded.".into())))
}

/// cURL validation: https, host ∈ {ai.zoom.us, zoommate.zoom.us}, path exactly
/// `/ai-computer/api/v1/credits/status`, no query/fragment, authorization required.
fn is_allowed_capture_url(url: &reqwest::Url) -> bool {
    let host = match url.host_str() {
        Some(h) => h.to_ascii_lowercase(),
        None => return false,
    };
    url.scheme().eq_ignore_ascii_case("https")
        && API_HOSTS.iter().any(|h| host == *h)
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == CREDITS_STATUS_PATH
        && url.query().is_none()
        && url.fragment().is_none()
}

fn request_context_from_manual(raw: &str) -> Option<RequestContext> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    // Prefer full cURL capture with Authorization.
    if curl_capture::looks_like_curl_capture(raw) {
        let capture_url = curl_capture::request_url(raw)?;
        let parsed = reqwest::Url::parse(&capture_url).ok()?;
        if !is_allowed_capture_url(&parsed) {
            return None;
        }
        let capture_host = parsed.host_str()?.to_ascii_lowercase();
        let fields = curl_capture::header_fields(raw);
        let authorization = curl_capture::header_value("Authorization", &fields)?;
        if authorization.trim().is_empty() {
            return None;
        }
        let mut headers =
            curl_capture::forwarded_headers_from_pairs(&fields, FORWARDED_MANUAL_HEADERS);
        headers.remove("Authorization");
        let cookie = headers.remove("Cookie");
        let mut cookie_by_host = HashMap::new();
        if let Some(c) = cookie {
            cookie_by_host.insert(capture_host.clone(), c);
        }
        // Pin Origin/Referer at send time; drop any captured values.
        headers.remove("Origin");
        headers.remove("Referer");
        return Some(RequestContext {
            authorization: bearer_header_value(&authorization),
            headers,
            cookie_by_host,
            preferred_host: Some(capture_host),
            account_email: None,
            cache_key: None,
        });
    }

    None
}

pub(crate) fn snapshot_from_credit_status(
    status: &CreditStatus,
    account_email: Option<&str>,
    now: DateTime<Utc>,
) -> UsageSnapshot {
    let budget_cap = status.budget_cap.unwrap_or(0.0);
    let used_credit = status.used_credit.unwrap_or(0.0);
    let is_unlimited = status.is_unlimited.unwrap_or(false);

    let used_percent =
        if is_unlimited || budget_cap <= 0.0 || !budget_cap.is_finite() || !used_credit.is_finite()
        {
            0.0
        } else {
            ((used_credit / budget_cap) * 100.0).clamp(0.0, 100.0)
        };

    let cycle_start = date_from_millis(status.cycle_start_date);
    let cycle_end = date_from_millis(status.cycle_end_date);

    let resets_at = if is_unlimited || budget_cap <= 0.0 {
        None
    } else {
        cycle_end
    };

    let window_minutes = match (cycle_start, cycle_end) {
        (Some(start), Some(end)) if end > start => {
            let mins = (end - start).num_minutes();
            if mins > 0 {
                Some(mins.min(u32::MAX as i64) as u32)
            } else {
                None
            }
        }
        _ => None,
    };

    let primary = RateWindow::with_details(
        used_percent,
        window_minutes,
        resets_at,
        Some("Credits".into()),
    );

    let mut snap = UsageSnapshot::new(primary);
    snap.updated_at = now;
    if let Some(email) = account_email.map(str::trim).filter(|s| !s.is_empty()) {
        snap = snap.with_email(email).with_login_method("Cookie");
    }
    // Silence unused-field warnings for fields we intentionally decode for forward compat.
    let _ = (
        status.remaining_credit,
        status.overage_credit,
        status.allow_overage,
        status.is_quota_available,
    );
    snap
}

// Classify failover errors for pure unit tests.
#[cfg(test)]
pub(crate) fn should_failover(err: &ProviderError) -> bool {
    !matches!(err, ProviderError::AuthRequired | ProviderError::Parse(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_status_json() -> &'static str {
        r#"{
          "data": {
            "credit_status": {
              "budget_cap": 1000.0,
              "used_credit": 250.0,
              "remaining_credit": 750.0,
              "overage_credit": 0.0,
              "allow_overage": false,
              "cycle_start_date": 1722470400000,
              "cycle_end_date": 1725148800000,
              "is_quota_available": true,
              "is_unlimited": false
            }
          },
          "status_code": 200
        }"#
    }

    #[test]
    fn parses_credits_status_fixture() {
        let envelope: CreditsStatusEnvelope =
            serde_json::from_str(sample_status_json()).expect("fixture parses");
        let status = envelope.data.unwrap().credit_status.unwrap();
        let snap = snapshot_from_credit_status(&status, Some("user@zoom.us"), Utc::now());
        assert!((snap.primary.used_percent - 25.0).abs() < 0.01);
        assert_eq!(snap.primary.reset_description.as_deref(), Some("Credits"));
        assert!(snap.primary.resets_at.is_some());
        // 31 days ≈ 44640 minutes (2024-08-01 → 2024-09-01)
        assert_eq!(snap.primary.window_minutes, Some(44640));
        assert_eq!(snap.account_email.as_deref(), Some("user@zoom.us"));
        assert_eq!(snap.login_method.as_deref(), Some("Cookie"));
    }

    #[test]
    fn unlimited_or_zero_cap_yields_zero_percent() {
        let unlimited = CreditStatus {
            budget_cap: Some(100.0),
            used_credit: Some(50.0),
            is_unlimited: Some(true),
            ..Default::default()
        };
        let snap = snapshot_from_credit_status(&unlimited, None, Utc::now());
        assert_eq!(snap.primary.used_percent, 0.0);
        assert!(snap.primary.resets_at.is_none());

        let zero_cap = CreditStatus {
            budget_cap: Some(0.0),
            used_credit: Some(10.0),
            is_unlimited: Some(false),
            ..Default::default()
        };
        let snap = snapshot_from_credit_status(&zero_cap, None, Utc::now());
        assert_eq!(snap.primary.used_percent, 0.0);
    }

    #[test]
    fn clamps_used_percent_to_100() {
        let status = CreditStatus {
            budget_cap: Some(100.0),
            used_credit: Some(150.0),
            is_unlimited: Some(false),
            cycle_end_date: Some(1_900_000_000_000),
            ..Default::default()
        };
        let snap = snapshot_from_credit_status(&status, None, Utc::now());
        assert_eq!(snap.primary.used_percent, 100.0);
    }

    #[test]
    fn manual_curl_capture_requires_allowed_url_and_authorization() {
        let good = "curl 'https://ai.zoom.us/ai-computer/api/v1/credits/status' \
            -H 'Authorization: Bearer tok-abc' -H 'Cookie: session=xyz'";
        let ctx = request_context_from_manual(good).expect("valid capture");
        assert_eq!(ctx.authorization, "Bearer tok-abc");
        assert_eq!(ctx.preferred_host.as_deref(), Some("ai.zoom.us"));
        assert_eq!(
            ctx.cookie_by_host.get("ai.zoom.us").map(String::as_str),
            Some("session=xyz")
        );

        // Wrong path
        assert!(
            request_context_from_manual(
                "curl 'https://ai.zoom.us/ai-computer/api/v1/other' -H 'Authorization: Bearer x'"
            )
            .is_none()
        );
        // Query rejected
        assert!(
            request_context_from_manual(
                "curl 'https://ai.zoom.us/ai-computer/api/v1/credits/status?x=1' \
                 -H 'Authorization: Bearer x'"
            )
            .is_none()
        );
        // Missing auth
        assert!(
            request_context_from_manual(
                "curl 'https://ai.zoom.us/ai-computer/api/v1/credits/status' -H 'Cookie: a=b'"
            )
            .is_none()
        );
        // Bad host
        assert!(
            request_context_from_manual(
                "curl 'https://evil.example/ai-computer/api/v1/credits/status' \
                 -H 'Authorization: Bearer x'"
            )
            .is_none()
        );
    }

    #[test]
    fn hosts_preferred_promotes_capture_host() {
        assert_eq!(
            hosts_preferred(Some("zoommate.zoom.us")),
            vec!["zoommate.zoom.us", "ai.zoom.us"]
        );
        assert_eq!(
            hosts_preferred(None),
            vec!["ai.zoom.us", "zoommate.zoom.us"]
        );
    }

    #[test]
    fn failover_skips_auth_and_parse() {
        assert!(!should_failover(&ProviderError::AuthRequired));
        assert!(!should_failover(&ProviderError::Parse("x".into())));
        assert!(should_failover(&ProviderError::Other("HTTP 500".into())));
        assert!(should_failover(&ProviderError::NoCookies));
    }

    #[test]
    fn bearer_header_normalizes_prefix() {
        assert_eq!(bearer_header_value("tok"), "Bearer tok");
        assert_eq!(bearer_header_value("Bearer tok"), "Bearer tok");
        assert_eq!(bearer_header_value("bearer tok"), "bearer tok");
    }

    #[test]
    fn jwt_exp_reads_payload() {
        // header.payload.sig — payload = {"exp": 2000000000}
        let payload = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            br#"{"exp":2000000000}"#,
        );
        let token = format!("aaa.{payload}.sig");
        assert_eq!(jwt_exp_unix(&token), Some(2_000_000_000));
        assert_eq!(jwt_exp_unix("not-a-jwt"), None);
    }

    #[test]
    fn cookie_fingerprint_is_stable() {
        let mut a = HashMap::new();
        a.insert("ai.zoom.us".into(), "c=1".into());
        a.insert("zoommate.zoom.us".into(), "c=2".into());
        let mut b = HashMap::new();
        b.insert("zoommate.zoom.us".into(), "c=2".into());
        b.insert("ai.zoom.us".into(), "c=1".into());
        assert_eq!(cookie_fingerprint(&a), cookie_fingerprint(&b));
        assert_eq!(cookie_fingerprint(&a).len(), 64);
    }
}
