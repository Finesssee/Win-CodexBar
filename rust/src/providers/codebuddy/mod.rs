//! CodeBuddy (Tencent) provider — soft-fork addition.
//!
//! Fetches account credit packages from the China billing API:
//! `POST https://www.codebuddy.cn/billing/meter/get-user-resource`
//!
//! Auth sources (priority):
//! 1. Manual Cookie header (Settings / token accounts)
//! 2. `~/.codebuddy/cb_cookie.txt` (same file as codebuddy-statusline)
//! 3. Browser cookies for `codebuddy.cn` / `www.codebuddy.cn`
//!
//! Auto mode falls back to the shared `~/.codebuddy/cb_credits.json` cache on
//! *transient* web failures only (network/429/5xx/WAF-HTML). Expired or invalid
//! credentials (HTTP 401/403, auth-flavoured API responses) propagate as
//! `AuthRequired` so the user is prompted to re-authenticate instead of
//! silently reading stale credit data.
//!
//! The cache stores typed numeric totals (`CreditTotals`) plus a SHA-256
//! fingerprint of the cookie that produced them (`accountHash`). A cache
//! belonging to a different account is rejected, so one account never sees
//! another account's stale credits. Caches written by external statusline
//! helpers (no fingerprint) are still accepted.
//!
//! EdgeOne note: UA must look like Chrome without `Edg/` or the WAF returns 401.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::{Client, StatusCode, Url};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};

const CN_API: &str = "https://www.codebuddy.cn/billing/meter/get-user-resource";
const CN_ORIGIN: &str = "https://www.codebuddy.cn";
const CN_REFERER: &str = "https://www.codebuddy.cn/profile/plans-usage";
const PRODUCT_CODE: &str = "p_tcaca";
/// Chrome UA without Edg/ — Edge UA is rejected by Tencent EdgeOne on this path.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.0.0 Safari/537.36";

/// Default package codes covering main credits + personal subscription packs.
/// Override via env `CB_PACKAGE_CODES` (JSON array string) when the account differs.
const DEFAULT_PACKAGE_CODES: &[&str] = &[
    "TCACA_code_007_nzdH5h4Nl0",
    "TCACA_code_029_6wCGEWquYy",
    "TCACA_code_030_BjSt89qTvr",
    "TCACA_code_008_cfWoLwvjU4",
    "TCACA_code_002_AkiJS3ZHF5",
    "TCACA_code_023_4xbGhMrE6q",
    "TCACA_code_026_BaESVICNoi",
    "TCACA_code_027_0FCGVA6vSa",
];

/// Upper bounds for env overrides — a misconfigured/huge override should make
/// the request smaller and obvious, not larger and weird.
const MAX_PACKAGE_CODES: usize = 64;
const MAX_PACKAGE_CODE_LEN: usize = 128;
/// Billing payloads are a few KB; anything larger is a hostile/broken endpoint.
const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;
/// The shared cache is a small JSON object; refuse reading anything larger.
const MAX_CACHE_BYTES: u64 = 1024 * 1024;

pub struct CodeBuddyProvider {
    metadata: ProviderMetadata,
    client: Client,
    /// Billing API endpoint; override via `CB_API_URL` (validated).
    api_url: String,
}

impl CodeBuddyProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::CodeBuddy,
                display_name: "CodeBuddy",
                session_label: "Credits",
                weekly_label: "Packages",
                supports_opus: false,
                supports_credits: true,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://www.codebuddy.cn/profile/plans-usage"),
                status_page_url: None,
            },
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .unwrap_or_else(|_| Client::new()),
            api_url: configured_api_url(),
        }
    }

    async fn fetch_web(&self, cookie: &str) -> Result<ProviderFetchResult, FetchFailure> {
        // One retry for transient proxy/WAF/network flakes (common on EdgeOne).
        let mut last_failure: Option<FetchFailure> = None;
        for attempt in 0..2u8 {
            match self.fetch_web_once(cookie).await {
                Ok((snapshot, totals)) => {
                    // Persist typed totals so Auto can soft-fail on the next
                    // transient flake. Failures here never mask the fetch.
                    if let Some(path) = credits_cache_path() {
                        let fingerprint = cookie_fingerprint(cookie);
                        if let Err(err) = write_credits_cache(&path, &totals, Some(&fingerprint)) {
                            tracing::debug!(%err, "CodeBuddy: failed to write local credits cache");
                        }
                    }
                    return Ok(ProviderFetchResult::new(snapshot, "web"));
                }
                Err(fail) => {
                    if !fail.is_transient() || attempt == 1 {
                        return Err(fail);
                    }
                    last_failure = Some(fail);
                    tokio::time::sleep(std::time::Duration::from_millis(350)).await;
                }
            }
        }
        Err(
            last_failure.unwrap_or(FetchFailure::Permanent(ProviderError::Other(
                "CodeBuddy fetch failed".into(),
            ))),
        )
    }

    async fn fetch_web_once(
        &self,
        cookie: &str,
    ) -> Result<(UsageSnapshot, CreditTotals), FetchFailure> {
        let body = request_body();
        let response = self
            .client
            .post(&self.api_url)
            .header("Cookie", cookie)
            .header("Accept", "application/json, text/plain, */*")
            .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.7")
            .header("Content-Type", "application/json")
            .header("Origin", CN_ORIGIN)
            .header("Referer", CN_REFERER)
            .header("User-Agent", USER_AGENT)
            .header("x-client-platform", "web")
            .header("sec-fetch-dest", "empty")
            .header("sec-fetch-mode", "cors")
            .header("sec-fetch-site", "same-origin")
            .json(&body)
            .send()
            .await
            .map_err(|e| FetchFailure::Transient(ProviderError::Network(e)))?;

        if let Some(fail) = failure_for_status(response.status()) {
            return Err(fail);
        }
        if response
            .content_length()
            .is_some_and(|len| len > MAX_RESPONSE_BYTES)
        {
            return Err(FetchFailure::Permanent(response_too_large()));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| FetchFailure::Transient(ProviderError::Network(e)))?;
        if bytes.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(FetchFailure::Permanent(response_too_large()));
        }
        // EdgeOne sometimes returns HTML 200/401 pages instead of JSON.
        if looks_like_html(&bytes) {
            return Err(FetchFailure::Transient(ProviderError::Other(
                "CodeBuddy WAF/HTML response (retry later)".into(),
            )));
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|e| {
            FetchFailure::Transient(ProviderError::Parse(format!(
                "Failed to parse CodeBuddy response: {e}"
            )))
        })?;
        // Shape/auth problems in a well-formed payload won't heal on retry.
        let totals = totals_from_api_payload(&value).map_err(FetchFailure::Permanent)?;
        Ok((snapshot_from_totals(&totals), totals))
    }

    fn fetch_local_cache(
        &self,
        fingerprint: Option<&str>,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let path = credits_cache_path().ok_or_else(|| {
            ProviderError::Other("Could not resolve ~/.codebuddy/cb_credits.json".into())
        })?;
        let value = read_credits_cache(&path)?;
        // Isolation: never serve one account's stale credits to another.
        // Fingerprint-free caches (external statusline helpers) are accepted.
        if let (Some(expected), Some(cached)) = (
            fingerprint,
            value.get("accountHash").and_then(|v| v.as_str()),
        ) && cached != expected
        {
            return Err(ProviderError::Other(
                "CodeBuddy cache belongs to a different account".into(),
            ));
        }
        let totals = totals_from_cache_json(&value)?;
        Ok(ProviderFetchResult::new(
            snapshot_from_totals(&totals),
            "cli",
        ))
    }

    fn resolve_cookie(&self, ctx: &FetchContext) -> Result<String, ProviderError> {
        if let Some(normalized) = ctx
            .manual_cookie_header
            .as_deref()
            .and_then(normalize_cookie_header)
        {
            return Ok(normalized);
        }
        if let Some(from_file) = read_cookie_file() {
            return Ok(from_file);
        }
        let header =
            crate::providers::browser_cookie_header(&["codebuddy.cn", "www.codebuddy.cn"])?;
        normalize_cookie_header(&header).ok_or(ProviderError::NoCookies)
    }
}

impl Default for CodeBuddyProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// Typed credit totals extracted from the API payload or the on-disk cache.
/// The display label and the cache file are both *derived from* this value —
/// never the other way around.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CreditTotals {
    total: f64,
    used: f64,
    remaining: f64,
    reset: Option<DateTime<Utc>>,
}

/// Typed classification of a web-fetch failure: transient failures may be
/// retried and may soft-fail to the local cache in Auto mode; permanent
/// failures (auth, shape, hostile input) propagate immediately.
#[derive(Debug)]
enum FetchFailure {
    Transient(ProviderError),
    Permanent(ProviderError),
}

impl FetchFailure {
    fn is_transient(&self) -> bool {
        matches!(self, FetchFailure::Transient(_))
    }

    fn into_error(self) -> ProviderError {
        match self {
            FetchFailure::Transient(err) | FetchFailure::Permanent(err) => err,
        }
    }
}

/// Auto mode may soft-fail to the cache for *transient* failures only.
/// `AuthRequired` must always surface so the user re-authenticates.
fn cache_fallback_allowed(source_mode: SourceMode, failure: &FetchFailure) -> bool {
    matches!(source_mode, SourceMode::Auto) && failure.is_transient()
}

/// Classify an HTTP status without any string matching.
fn failure_for_status(status: StatusCode) -> Option<FetchFailure> {
    if status.is_success() {
        return None;
    }
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Some(FetchFailure::Permanent(ProviderError::AuthRequired));
    }
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        return Some(FetchFailure::Transient(ProviderError::Other(format!(
            "CodeBuddy temporary HTTP {status}"
        ))));
    }
    Some(FetchFailure::Permanent(ProviderError::Other(format!(
        "CodeBuddy get-user-resource returned HTTP {status}"
    ))))
}

fn response_too_large() -> ProviderError {
    ProviderError::Other(format!(
        "CodeBuddy response exceeds {MAX_RESPONSE_BYTES} bytes"
    ))
}

fn looks_like_html(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .find(|b| !b.is_ascii_whitespace())
        .is_some_and(|b| *b == b'<')
}

/// Resolve the billing endpoint: `CB_API_URL` override, validated, else the
/// default CN API. `http:` is accepted only for loopback hosts (local mocks).
fn configured_api_url() -> String {
    match std::env::var("CB_API_URL") {
        Ok(raw) => match validate_api_url(&raw) {
            Ok(url) => url,
            Err(reason) => {
                tracing::warn!(%reason, "CodeBuddy: ignoring invalid CB_API_URL override");
                CN_API.to_string()
            }
        },
        Err(_) => CN_API.to_string(),
    }
}

fn validate_api_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("empty URL".into());
    }
    let url = Url::parse(trimmed).map_err(|e| format!("invalid URL: {e}"))?;
    let allowed = match url.scheme() {
        "https" => true,
        // Url::host_str() is None for IPv6 literals (Host::Ipv6); ::1 is the
        // only IPv6 loopback address, so check the parsed authority directly.
        "http" => {
            url.host_str().is_some_and(is_loopback_host) || url.authority().starts_with("[::1]")
        }
        _ => false,
    };
    if allowed {
        Ok(url.to_string())
    } else {
        Err(format!(
            "endpoint must be https (http only for loopback): {trimmed}"
        ))
    }
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

fn request_body() -> Value {
    json!({
        "PageNumber": 1,
        "PageSize": 200,
        "ProductCode": PRODUCT_CODE,
        "Status": [0, 3],
        "OnlyValidPeriod": true,
        "PackageCodes": package_codes(),
    })
}

fn package_codes() -> Vec<String> {
    if let Ok(raw) = std::env::var("CB_PACKAGE_CODES") {
        let parsed = match serde_json::from_str::<Value>(&raw) {
            Ok(Value::Array(items)) => validated_package_codes(&items),
            _ => Vec::new(),
        };
        if parsed.is_empty() {
            tracing::warn!("CodeBuddy: CB_PACKAGE_CODES had no usable entries; using defaults");
        } else {
            return parsed;
        }
    }
    DEFAULT_PACKAGE_CODES
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

/// Filter env-supplied package codes: strings only, trimmed, bounded length,
/// no control characters, capped count.
fn validated_package_codes(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .filter_map(|v| v.as_str())
        .map(str::trim)
        .filter(|s| {
            !s.is_empty() && s.len() <= MAX_PACKAGE_CODE_LEN && !s.chars().any(|c| c.is_control())
        })
        .take(MAX_PACKAGE_CODES)
        .map(str::to_string)
        .collect()
}

fn codebuddy_home() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("CODEBUDDY_HOME") {
        let p = PathBuf::from(home);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    dirs::home_dir().map(|h| h.join(".codebuddy"))
}

fn cookie_file_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CB_COOKIE_FILE") {
        let path = PathBuf::from(p);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    codebuddy_home().map(|h| h.join("cb_cookie.txt"))
}

fn credits_cache_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CB_CREDITS_FILE") {
        let path = PathBuf::from(p);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    codebuddy_home().map(|h| h.join("cb_credits.json"))
}

fn read_cookie_file() -> Option<String> {
    let path = cookie_file_path()?;
    let raw = std::fs::read_to_string(path).ok()?;
    normalize_cookie_header(&raw)
}

fn normalize_cookie_header(raw: &str) -> Option<String> {
    // Strip BOM, caret-escapes from Windows "Copy as cURL", and Cookie: prefix.
    let mut header = raw.trim().trim_start_matches('\u{feff}').replace('^', "");
    header = header.trim().to_string();
    let lower = header.to_ascii_lowercase();
    if lower.starts_with("cookie:") {
        header = header["cookie:".len()..].trim().to_string();
    }
    let pairs = header
        .split(';')
        .filter_map(|chunk| {
            let (name, value) = chunk.trim().split_once('=')?;
            let name = name.trim();
            let value = value.trim();
            (!name.is_empty() && !value.is_empty()).then(|| format!("{name}={value}"))
        })
        .collect::<Vec<_>>();
    (!pairs.is_empty()).then(|| pairs.join("; "))
}

/// First 8 bytes of SHA-256(cookie), hex — 16 chars, no cookie material.
/// Used as an account discriminator for the local credits cache; never logged.
fn cookie_fingerprint(cookie: &str) -> String {
    let digest = Sha256::digest(cookie.as_bytes());
    let mut out = String::with_capacity(16);
    for byte in &digest[..8] {
        use std::fmt::Write as _;
        // Formatting a fingerprint char can only fail on IO errors, which
        // String writes never raise; discarding is correct.
        let _written_hex = write!(out, "{byte:02x}");
    }
    out
}

/// Extract typed credit totals from a `get-user-resource` payload.
fn totals_from_api_payload(value: &Value) -> Result<CreditTotals, ProviderError> {
    let code = value.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code != 0 {
        let msg = value
            .get("msg")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        if msg.to_ascii_lowercase().contains("auth")
            || msg.contains("登录")
            || msg.contains("未登录")
        {
            return Err(ProviderError::AuthRequired);
        }
        return Err(ProviderError::Other(format!(
            "CodeBuddy API code={code}: {msg}"
        )));
    }

    let accounts = value
        .pointer("/data/Response/Data/Accounts")
        .and_then(|a| a.as_array())
        .ok_or_else(|| {
            ProviderError::Parse(
                "CodeBuddy response missing data.Response.Data.Accounts (check PackageCodes)"
                    .into(),
            )
        })?;

    if accounts.is_empty() {
        return Err(ProviderError::Parse(
            "CodeBuddy returned zero packages — set CB_PACKAGE_CODES from browser cURL body".into(),
        ));
    }

    let mut total = 0.0_f64;
    let mut used = 0.0_f64;
    let mut remaining = 0.0_f64;
    let mut reset: Option<DateTime<Utc>> = None;

    for account in accounts {
        total += precise_or_field(account, "CapacitySizePrecise", "CapacitySize").unwrap_or(0.0);
        used += precise_or_field(account, "CapacityUsedPrecise", "CapacityUsed").unwrap_or(0.0);
        remaining +=
            precise_or_field(account, "CapacityRemainPrecise", "CapacityRemain").unwrap_or(0.0);
        if let Some(reset_at) = expire_time(account) {
            reset = Some(reset.map_or(reset_at, |prev| prev.min(reset_at)));
        }
    }

    CreditTotals::normalized(total, used, remaining, reset)
}

impl CreditTotals {
    /// Reconcile partial sums: prefer summed remaining, recompute used when it
    /// is missing, and clamp every field to non-negative.
    fn normalized(
        mut total: f64,
        mut used: f64,
        mut remaining: f64,
        reset: Option<DateTime<Utc>>,
    ) -> Result<CreditTotals, ProviderError> {
        if !(total.is_finite() && used.is_finite() && remaining.is_finite()) {
            return Err(ProviderError::Parse(
                "CodeBuddy payload contains non-finite credit values".into(),
            ));
        }
        if remaining > 0.0 && total > 0.0 && used <= 0.0 {
            used = (total - remaining).max(0.0);
        }
        if used < 0.0 {
            used = 0.0;
        }
        if total < 0.0 {
            total = 0.0;
        }
        if remaining < 0.0 {
            remaining = (total - used).max(0.0);
        }
        Ok(CreditTotals {
            total,
            used,
            remaining,
            reset,
        })
    }
}

/// Build the tray snapshot from typed totals — the label is derived, never
/// the other way around.
fn snapshot_from_totals(totals: &CreditTotals) -> UsageSnapshot {
    let used_percent = if totals.total > 0.0 {
        (totals.used / totals.total * 100.0).clamp(0.0, 100.0)
    } else if totals.used > 0.0 {
        100.0
    } else {
        0.0
    };
    // Keep this short — tray card puts it on the right of the metric row
    // (`.menu-metric__reset { white-space: nowrap }`), so long copy overflows.
    let description = Some(format_credits_short(totals.remaining, totals.total));
    UsageSnapshot::new(RateWindow::with_details(
        used_percent,
        None,
        totals.reset,
        description,
    ))
    .with_login_method("CodeBuddy CN")
}

/// Serialize the shared cache from typed totals. Values are preserved exactly
/// (no display formatting); `resetsAt` and `accountHash` are optional.
fn cache_json_from_totals(totals: &CreditTotals, fingerprint: Option<&str>) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("total".into(), json!(totals.total));
    obj.insert("used".into(), json!(totals.used));
    obj.insert("remaining".into(), json!(totals.remaining));
    obj.insert("source".into(), json!("api"));
    obj.insert("updatedAt".into(), json!(Utc::now().to_rfc3339()));
    if let Some(reset) = totals.reset {
        obj.insert("resetsAt".into(), json!(reset.to_rfc3339()));
    }
    if let Some(fingerprint) = fingerprint {
        obj.insert("accountHash".into(), json!(fingerprint));
    }
    Value::Object(obj)
}

/// Parse and validate the shared cache into typed totals.
fn totals_from_cache_json(value: &Value) -> Result<CreditTotals, ProviderError> {
    let total = number_field(value, &["total"])
        .ok_or_else(|| ProviderError::Parse("cb_credits.json missing total".into()))?;
    if !total.is_finite() || total < 0.0 {
        return Err(ProviderError::Parse(format!(
            "cb_credits.json invalid total: {total}"
        )));
    }
    let used = number_field(value, &["used"]).unwrap_or(0.0).max(0.0);
    let remaining = number_field(value, &["remaining"])
        .map(|r| r.max(0.0))
        .unwrap_or_else(|| (total - used).max(0.0));
    // Prefer real package expiry from cache if present; do not treat updatedAt
    // as a quota reset (that previously looked like a bogus reset countdown).
    let reset = value
        .get("resetsAt")
        .and_then(|v| v.as_str())
        .and_then(parse_datetime);
    Ok(CreditTotals {
        total,
        used,
        remaining,
        reset,
    })
}

fn write_credits_cache(
    path: &Path,
    totals: &CreditTotals,
    fingerprint: Option<&str>,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = cache_json_from_totals(totals, fingerprint);
    std::fs::write(
        path,
        serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

fn read_credits_cache(path: &Path) -> Result<Value, ProviderError> {
    let metadata = std::fs::metadata(path).map_err(|_| {
        ProviderError::Other(format!(
            "No CodeBuddy cookie and no local cache at {}",
            path.display()
        ))
    })?;
    if metadata.len() > MAX_CACHE_BYTES {
        return Err(ProviderError::Other(format!(
            "CodeBuddy cache at {} is too large ({} bytes)",
            path.display(),
            metadata.len()
        )));
    }
    let raw = std::fs::read_to_string(path)
        .map_err(|e| ProviderError::Other(format!("Failed to read CodeBuddy cache: {e}")))?;
    serde_json::from_str(&raw).map_err(|e| ProviderError::Parse(format!("Invalid cache: {e}")))
}

/// Compact credit label for the tray metric row: `1,234 / 5,678 left`.
fn format_credits_short(remaining: f64, total: f64) -> String {
    format!(
        "{} / {} left",
        format_credit_number(remaining),
        format_credit_number(total)
    )
}

fn format_credit_number(value: f64) -> String {
    if !value.is_finite() {
        return "0".into();
    }
    let rounded = (value * 100.0).round() / 100.0;
    if (rounded - rounded.round()).abs() < 0.001 {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "value just rounded and verified integral within 0.001, so the i64 cast loses nothing"
        )]
        format_int_with_commas(rounded.round() as i64)
    } else {
        let s = format!("{rounded:.2}");
        let trimmed = s.trim_end_matches('0').trim_end_matches('.');
        // Insert commas into the integer part only.
        if let Some((whole, frac)) = trimmed.split_once('.') {
            format!(
                "{}.{}",
                format_int_with_commas(whole.parse().unwrap_or(0)),
                frac
            )
        } else {
            format_int_with_commas(trimmed.parse().unwrap_or(0))
        }
    }
}

fn format_int_with_commas(value: i64) -> String {
    let negative = value < 0;
    let digits = value.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    let mut s: String = out.chars().rev().collect();
    if negative {
        s.insert(0, '-');
    }
    s
}

fn precise_or_field(account: &Value, precise_key: &str, fallback_key: &str) -> Option<f64> {
    number_field(account, &[precise_key]).or_else(|| number_field(account, &[fallback_key]))
}

fn expire_time(account: &Value) -> Option<DateTime<Utc>> {
    // Common field names observed on Tencent package objects.
    [
        "ExpireTime",
        "expireTime",
        "ExpireTimeStamp",
        "EndTime",
        "endTime",
        "ValidEndTime",
    ]
    .iter()
    .find_map(|key| account.get(key).and_then(datetime_from_value))
}

/// Accept a datetime as either an epoch number/float-string or a date string.
fn datetime_from_value(value: &Value) -> Option<DateTime<Utc>> {
    if let Some(s) = value.as_str() {
        return parse_datetime(s);
    }
    value.as_f64().and_then(epoch_seconds_or_millis)
}

fn parse_datetime(raw: &str) -> Option<DateTime<Utc>> {
    if let Ok(number) = raw.parse::<f64>() {
        return epoch_seconds_or_millis(number);
    }
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|| {
            // "2026-08-07 12:00:00" style
            chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|ndt| ndt.and_utc())
        })
}

fn epoch_seconds_or_millis(number: f64) -> Option<DateTime<Utc>> {
    let seconds = if number > 10_000_000_000.0 {
        number / 1000.0
    } else {
        number
    };
    #[allow(
        clippy::cast_possible_truncation,
        reason = "epoch seconds far exceed i64 range yield None from from_timestamp anyway; truncation only for absurd inputs that fail validation next"
    )]
    DateTime::<Utc>::from_timestamp(seconds as i64, 0)
}

/// Multi-key numeric lookup; missing keys must not short-circuit later keys.
fn number_field(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| match value.get(*key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    })
}

#[async_trait]
impl Provider for CodeBuddyProvider {
    fn id(&self) -> ProviderId {
        ProviderId::CodeBuddy
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto | SourceMode::Web => match self.resolve_cookie(ctx) {
                Ok(cookie) => {
                    let fingerprint = cookie_fingerprint(&cookie);
                    match self.fetch_web(&cookie).await {
                        Ok(result) => Ok(result),
                        Err(fail) => {
                            // Transient flakes in Auto may soft-fail to the
                            // local cache; auth/permanent failures propagate.
                            if !cache_fallback_allowed(ctx.source_mode, &fail) {
                                return Err(fail.into_error());
                            }
                            match self.fetch_local_cache(Some(&fingerprint)) {
                                Ok(result) => Ok(result),
                                Err(_) => Err(fail.into_error()),
                            }
                        }
                    }
                }
                Err(err) if matches!(ctx.source_mode, SourceMode::Auto) => {
                    // No credentials at all: Auto may soft-fail to the shared
                    // cache (written by us or by statusline helpers).
                    self.fetch_local_cache(None).or(Err(err))
                }
                Err(err) => Err(err),
            },
            SourceMode::Cli => self.fetch_local_cache(None),
            SourceMode::OAuth => Err(ProviderError::UnsupportedSource(ctx.source_mode)),
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::Web, SourceMode::Cli]
    }

    fn supports_web(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api_payload() -> Value {
        serde_json::json!({
            "code": 0,
            "msg": "ok",
            "data": {
                "Response": {
                    "Data": {
                        "Accounts": [
                            {
                                "CapacitySizePrecise": "2000",
                                "CapacityUsedPrecise": "100",
                                "CapacityRemainPrecise": "1900",
                                "ExpireTime": "2026-09-01T00:00:00Z"
                            },
                            {
                                "CapacitySize": 1100,
                                "CapacityUsed": 11,
                                "CapacityRemain": 1089
                            }
                        ]
                    }
                }
            }
        })
    }

    #[test]
    fn parses_get_user_resource_payload_into_typed_totals() {
        let totals = totals_from_api_payload(&api_payload()).unwrap();
        assert_eq!(totals.total, 3100.0);
        assert_eq!(totals.used, 111.0);
        assert_eq!(totals.remaining, 2989.0);
        assert!(totals.reset.is_some());

        let snapshot = snapshot_from_totals(&totals);
        assert!((snapshot.primary.used_percent - (111.0 / 3100.0 * 100.0)).abs() < 0.01);
        assert_eq!(
            snapshot.primary.reset_description.as_deref(),
            Some("2,989 / 3,100 left")
        );
        assert!(snapshot.primary.resets_at.is_some());
    }

    #[test]
    fn auth_flavoured_payload_maps_to_auth_required() {
        for msg in ["未登录", "登录已过期", "auth token expired"] {
            let payload = serde_json::json!({ "code": 14001, "msg": msg });
            let err = totals_from_api_payload(&payload).unwrap_err();
            assert!(
                matches!(err, ProviderError::AuthRequired),
                "expected AuthRequired for msg={msg:?}, got {err}"
            );
        }
    }

    #[test]
    fn empty_accounts_errors_with_hint() {
        let payload = serde_json::json!({
            "code": 0,
            "data": { "Response": { "Data": { "Accounts": [] } } }
        });
        let err = totals_from_api_payload(&payload).unwrap_err();
        assert!(format!("{err}").contains("PackageCodes") || format!("{err}").contains("package"));
    }

    #[test]
    fn non_zero_code_errors() {
        let payload = serde_json::json!({ "code": 14001, "msg": "quote exceeded" });
        let err = totals_from_api_payload(&payload).unwrap_err();
        assert!(matches!(err, ProviderError::Other(_)), "got {err}");
    }

    #[test]
    fn payloads_with_non_finite_values_are_rejected() {
        // serde_json cannot carry NaN/inf, but an absurd +/-1e308 pair can
        // still overflow the sum to inf — that must be rejected, not cached.
        let payload = serde_json::json!({
            "code": 0,
            "data": { "Response": { "Data": { "Accounts": [
                { "CapacitySize": 1e308, "CapacityUsed": 1e308, "CapacityRemain": 1e308 },
                { "CapacitySize": 1e308, "CapacityUsed": 0, "CapacityRemain": 0 }
            ] } } }
        });
        assert!(totals_from_api_payload(&payload).is_err());
    }

    #[test]
    fn formats_compact_credit_labels() {
        assert_eq!(format_credits_short(1989.0, 3100.0), "1,989 / 3,100 left");
        assert_eq!(format_credits_short(12.5, 100.0), "12.5 / 100 left");
    }

    #[test]
    fn normalizes_cookie_with_caret_escapes() {
        assert_eq!(
            normalize_cookie_header("Cookie: a=1^|2; b=3").as_deref(),
            Some("a=1|2; b=3")
        );
        assert_eq!(normalize_cookie_header("  "), None);
        assert_eq!(normalize_cookie_header("Cookie:"), None);
    }

    #[test]
    fn cache_round_trip_preserves_typed_totals_exactly() {
        let totals = CreditTotals {
            total: 3100.5,
            used: 111.25,
            remaining: 2989.25,
            reset: Some(
                DateTime::parse_from_rfc3339("2026-09-01T08:30:00Z")
                    .unwrap()
                    .into(),
            ),
        };
        let json = cache_json_from_totals(&totals, Some("0123456789abcdef"));
        let parsed = totals_from_cache_json(&json).unwrap();
        assert_eq!(parsed, totals);
        // The cache carries the fingerprint verbatim.
        assert_eq!(
            json.get("accountHash").and_then(|v| v.as_str()),
            Some("0123456789abcdef")
        );
    }

    #[test]
    fn cache_file_round_trip_via_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cb_credits.json");
        let totals = CreditTotals {
            total: 2000.0,
            used: 100.25,
            remaining: 1899.75,
            reset: None,
        };
        write_credits_cache(&path, &totals, Some("feedbeefcafe0001")).unwrap();

        let value = read_credits_cache(&path).unwrap();
        let parsed = totals_from_cache_json(&value).unwrap();
        assert_eq!(parsed, totals);

        // Raw file exposes typed JSON numbers, not display text.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"total\": 2000.0"));
        assert!(raw.contains("feedbeefcafe0001"));
    }

    #[test]
    fn hostile_cache_inputs_are_rejected() {
        // Missing total.
        let err = totals_from_cache_json(&serde_json::json!({"used": 1})).unwrap_err();
        assert!(format!("{err}").contains("missing total"));
        // Negative total.
        assert!(totals_from_cache_json(&serde_json::json!({"total": -5})).is_err());
        // Non-JSON is a Parse error at the read layer.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, b"{not json").unwrap();
        assert!(matches!(
            read_credits_cache(&path),
            Err(ProviderError::Parse(_))
        ));
        // Oversized cache is refused before parsing.
        let big = dir.path().join("big.json");
        #[allow(
            clippy::cast_possible_truncation,
            reason = "MAX_CACHE_BYTES is 1 MiB; +1 stays far inside usize on any supported target"
        )]
        std::fs::write(&big, vec![b' '; (MAX_CACHE_BYTES + 1) as usize]).unwrap();
        assert!(read_credits_cache(&big).is_err());
    }

    #[test]
    fn validated_package_codes_filters_and_caps() {
        let long_code = "x".repeat(MAX_PACKAGE_CODE_LEN + 1);
        let items = vec![
            json!("TCACA_code_001_ok"),
            json!("   "),
            json!(""),
            json!(42),
            json!("aB\u{0007}c"),
            json!(long_code),
            json!("  TCACA_code_002_trimmed  "),
        ];
        let codes = validated_package_codes(&items);
        assert_eq!(
            codes,
            vec![
                "TCACA_code_001_ok".to_string(),
                "TCACA_code_002_trimmed".to_string(),
            ]
        );

        // Cap: more than MAX_PACKAGE_CODES valid entries are truncated.
        let many: Vec<Value> = (0..MAX_PACKAGE_CODES + 10)
            .map(|i| json!(format!("code_{i}")))
            .collect();
        assert_eq!(validated_package_codes(&many).len(), MAX_PACKAGE_CODES);
    }

    #[test]
    fn validate_api_url_rules() {
        assert_eq!(
            validate_api_url("https://www.codebuddy.cn/x").unwrap(),
            "https://www.codebuddy.cn/x"
        );
        assert!(validate_api_url("http://127.0.0.1:8080/x").is_ok());
        assert!(validate_api_url("http://localhost:8080/x").is_ok());
        assert!(validate_api_url("http://[::1]:8080/x").is_ok());
        assert!(validate_api_url("http://example.com/x").is_err());
        assert!(validate_api_url("ftp://example.com/x").is_err());
        assert!(validate_api_url("not a url").is_err());
        assert!(validate_api_url("   ").is_err());
    }

    #[test]
    fn failure_classification_for_http_statuses() {
        assert!(failure_for_status(StatusCode::OK).is_none());

        for status in [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN] {
            let fail = failure_for_status(status).unwrap();
            assert!(!fail.is_transient(), "{status} must be permanent");
            assert!(matches!(fail.into_error(), ProviderError::AuthRequired));
        }
        for status in [
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            assert!(
                failure_for_status(status).unwrap().is_transient(),
                "{status} must be transient"
            );
        }
        // Other failures (400 etc.) are permanent but not auth errors.
        let fail = failure_for_status(StatusCode::BAD_REQUEST).unwrap();
        assert!(!fail.is_transient());
        assert!(matches!(fail.into_error(), ProviderError::Other(_)));
    }

    #[test]
    fn cache_fallback_requires_auto_mode_and_transient_failure() {
        let transient = FetchFailure::Transient(ProviderError::Other("flake".into()));
        let permanent_auth = FetchFailure::Permanent(ProviderError::AuthRequired);
        let transient_ref = &transient;
        let auth_ref = &permanent_auth;

        // Auth never masks behind a stale cache.
        assert!(!cache_fallback_allowed(SourceMode::Auto, auth_ref));
        // Transient failures may fall back in Auto only.
        assert!(cache_fallback_allowed(SourceMode::Auto, transient_ref));
        assert!(!cache_fallback_allowed(SourceMode::Web, transient_ref));
        assert!(!cache_fallback_allowed(SourceMode::Cli, transient_ref));
    }

    #[test]
    fn cookie_fingerprint_is_stable_per_account_and_secret_free() {
        let fp_a = cookie_fingerprint("session=aaa; uid=1");
        let fp_b = cookie_fingerprint("session=bbb; uid=2");
        assert_eq!(fp_a, cookie_fingerprint("session=aaa; uid=1"));
        assert_ne!(fp_a, fp_b);
        assert_eq!(fp_a.len(), 16);
        assert!(fp_a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn parse_datetime_accepts_rfc3339_naive_and_epochs() {
        assert!(parse_datetime("2026-09-01T00:00:00Z").is_some());
        assert!(parse_datetime("2026-09-01 12:30:00").is_some());
        assert_eq!(
            parse_datetime("1767225600").unwrap().to_rfc3339(),
            DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                .unwrap()
                .to_rfc3339()
        );
        // Millis precision is divided down to seconds.
        assert_eq!(
            parse_datetime("1767225600000"),
            parse_datetime("1767225600")
        );
        assert!(parse_datetime("garbage").is_none());
    }

    #[test]
    fn number_field_does_not_short_circuit_on_missing_keys() {
        let obj = serde_json::json!({"b": "2.5"});
        assert_eq!(number_field(&obj, &["missing", "b"]), Some(2.5));
        assert_eq!(number_field(&obj, &["missing", "other"]), None);
    }

    fn provider_at(url: &str) -> CodeBuddyProvider {
        let mut provider = CodeBuddyProvider::new();
        provider.api_url = url.to_string();
        provider
    }

    #[tokio::test]
    async fn transient_failures_are_retried_exactly_once() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/billing/meter/get-user-resource")
            .with_status(500)
            .expect(2) // one initial attempt + one retry
            .create_async()
            .await;
        let provider = provider_at(&format!("{}/billing/meter/get-user-resource", server.url()));
        let fail = provider.fetch_web("a=1").await.unwrap_err();
        assert!(fail.is_transient());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn auth_required_is_permanent_and_never_retried() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/billing/meter/get-user-resource")
            .with_status(401)
            .expect(1) // auth failures must not be retried
            .create_async()
            .await;
        let provider = provider_at(&format!("{}/billing/meter/get-user-resource", server.url()));
        let fail = provider.fetch_web("a=1").await.unwrap_err();
        assert!(!fail.is_transient());
        assert!(matches!(fail.into_error(), ProviderError::AuthRequired));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn waf_html_body_is_treated_as_transient() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/billing/meter/get-user-resource")
            .with_status(200)
            .with_header("content-type", "text/html")
            .with_body("<html><body>edgeone block</body></html>")
            .expect(2)
            .create_async()
            .await;
        let provider = provider_at(&format!("{}/billing/meter/get-user-resource", server.url()));
        let fail = provider.fetch_web("a=1").await.unwrap_err();
        assert!(fail.is_transient());
        mock.assert_async().await;
    }

    /// End-to-end Auto behaviour with a real on-disk cache:
    /// success persists typed totals; transient failure falls back to them;
    /// auth failure surfaces even with a valid cache; foreign cache rejected.
    #[tokio::test]
    async fn auto_mode_cache_semantics_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("cb_credits.json");
        // SAFETY: single test using this env var; restored at the end.
        unsafe {
            std::env::set_var("CB_CREDITS_FILE", &cache_path);
        }

        let payload_body = r#"{"code":0,"msg":"ok","data":{"Response":{"Data":{"Accounts":[{"CapacitySize":2000,"CapacityUsed":100,"CapacityRemain":1900,"ExpireTime":"2026-09-01T00:00:00Z"}]}}}}"#;
        let ctx = |mode: SourceMode| FetchContext {
            source_mode: mode,
            manual_cookie_header: Some("session=abc; uid=42".to_string()),
            ..Default::default()
        };

        // Phase A: web success persists typed totals + fingerprint, source=web.
        {
            let mut server = mockito::Server::new_async().await;
            let mock = server
                .mock("POST", "/billing/meter/get-user-resource")
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(payload_body)
                .expect(1)
                .create_async()
                .await;
            let provider =
                provider_at(&format!("{}/billing/meter/get-user-resource", server.url()));
            let result = provider.fetch_usage(&ctx(SourceMode::Auto)).await.unwrap();
            assert_eq!(result.source_label, "web");
            mock.assert_async().await;

            let file = std::fs::read_to_string(&cache_path).unwrap();
            let json: Value = serde_json::from_str(&file).unwrap();
            assert_eq!(json.get("total").and_then(|v| v.as_f64()), Some(2000.0));
            assert_eq!(json.get("used").and_then(|v| v.as_f64()), Some(100.0));
            assert_eq!(json.get("remaining").and_then(|v| v.as_f64()), Some(1900.0));
            assert_eq!(
                json.get("accountHash").and_then(|v| v.as_str()),
                Some(cookie_fingerprint("session=abc; uid=42").as_str())
            );
            assert!(json.get("resetsAt").and_then(|v| v.as_str()).is_some());
        }

        // Phase B: auth failure must surface — never masked by the valid cache.
        {
            let mut server = mockito::Server::new_async().await;
            let mock = server
                .mock("POST", "/billing/meter/get-user-resource")
                .with_status(401)
                .expect(1)
                .create_async()
                .await;
            let provider =
                provider_at(&format!("{}/billing/meter/get-user-resource", server.url()));
            let err = provider
                .fetch_usage(&ctx(SourceMode::Auto))
                .await
                .unwrap_err();
            assert!(matches!(err, ProviderError::AuthRequired), "got {err}");
            mock.assert_async().await;
        }

        // Phase C: transient failure in Auto falls back to the cache (source=cli).
        {
            let mut server = mockito::Server::new_async().await;
            let mock = server
                .mock("POST", "/billing/meter/get-user-resource")
                .with_status(500)
                .expect(2)
                .create_async()
                .await;
            let provider =
                provider_at(&format!("{}/billing/meter/get-user-resource", server.url()));
            let result = provider.fetch_usage(&ctx(SourceMode::Auto)).await.unwrap();
            assert_eq!(result.source_label, "cli");
            assert_eq!(
                result.usage.primary.reset_description.as_deref(),
                Some("1,900 / 2,000 left")
            );
            assert!(
                (result.usage.primary.used_percent - 5.0).abs() < 0.01,
                "used_percent={}",
                result.usage.primary.used_percent
            );
            mock.assert_async().await;
        }

        // Phase D: a cache belonging to another account is rejected.
        {
            let mut json: Value =
                serde_json::from_str(&std::fs::read_to_string(&cache_path).unwrap()).unwrap();
            json["accountHash"] = json!("deadbeefdeadbeef");
            std::fs::write(&cache_path, serde_json::to_string_pretty(&json).unwrap()).unwrap();

            let mut server = mockito::Server::new_async().await;
            server
                .mock("POST", "/billing/meter/get-user-resource")
                .with_status(500)
                .create_async()
                .await;
            let provider =
                provider_at(&format!("{}/billing/meter/get-user-resource", server.url()));
            assert!(provider.fetch_usage(&ctx(SourceMode::Auto)).await.is_err());

            // Web mode with a transient failure must not fall back at all.
            let err = provider
                .fetch_usage(&ctx(SourceMode::Web))
                .await
                .unwrap_err();
            assert!(matches!(err, ProviderError::Other(_)), "got {err}");
        }

        // SAFETY: this test set CB_CREDITS_FILE at its start under the same
        // single-test ownership; removing it restores the shared environment.
        unsafe {
            std::env::remove_var("CB_CREDITS_FILE");
        }
    }
}
