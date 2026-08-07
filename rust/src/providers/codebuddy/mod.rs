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
//! Optional CLI fallback: read normalized `~/.codebuddy/cb_credits.json`
//! when web fetch is unavailable.
//!
//! EdgeOne note: UA must look like Chrome without `Edg/` or the WAF returns 401.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde_json::{json, Value};
use std::path::PathBuf;

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

pub struct CodeBuddyProvider {
    metadata: ProviderMetadata,
    client: Client,
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
        }
    }

    async fn fetch_web(&self, cookie_header: &str) -> Result<ProviderFetchResult, ProviderError> {
        let cookie = normalize_cookie_header(cookie_header).ok_or(ProviderError::NoCookies)?;
        // One retry for transient proxy/WAF/network flakes (common on EdgeOne).
        let mut last_err = None;
        for attempt in 0..2u8 {
            match self.fetch_web_once(&cookie).await {
                Ok(result) => {
                    // Persist a short cache so Auto can soft-fail on the next flake.
                    if let Err(err) = write_credits_cache_from_snapshot(&result.usage) {
                        tracing::debug!(?err, "CodeBuddy: failed to write local credits cache");
                    }
                    return Ok(result);
                }
                Err(err) => {
                    let retryable = is_retryable_error(&err);
                    last_err = Some(err);
                    if !retryable || attempt == 1 {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(350)).await;
                }
            }
        }
        Err(last_err.unwrap_or(ProviderError::Other("CodeBuddy fetch failed".into())))
    }

    async fn fetch_web_once(&self, cookie: &str) -> Result<ProviderFetchResult, ProviderError> {
        let body = request_body();
        let response = self
            .client
            .post(CN_API)
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
            .map_err(ProviderError::from)?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ProviderError::AuthRequired);
        }
        if status.as_u16() == 429
            || status.is_server_error()
            || status == reqwest::StatusCode::BAD_GATEWAY
        {
            return Err(ProviderError::Other(format!(
                "CodeBuddy temporary HTTP {status}"
            )));
        }
        if !status.is_success() {
            return Err(ProviderError::Other(format!(
                "CodeBuddy get-user-resource returned HTTP {status}"
            )));
        }

        let bytes = response.bytes().await.map_err(ProviderError::from)?;
        // EdgeOne sometimes returns HTML 200/401 pages instead of JSON.
        if bytes
            .iter()
            .find(|b| !b.is_ascii_whitespace())
            .is_some_and(|b| *b == b'<')
        {
            return Err(ProviderError::Other(
                "CodeBuddy WAF/HTML response (retry later)".into(),
            ));
        }

        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Parse(format!("Failed to parse CodeBuddy response: {e}")))?;

        let snapshot = snapshot_from_api_payload(&value)?;
        Ok(ProviderFetchResult::new(snapshot, "web"))
    }

    fn fetch_local_cache(&self) -> Result<ProviderFetchResult, ProviderError> {
        let path = credits_cache_path().ok_or_else(|| {
            ProviderError::Other("Could not resolve ~/.codebuddy/cb_credits.json".into())
        })?;
        let raw = std::fs::read_to_string(&path).map_err(|_| {
            ProviderError::Other(format!(
                "No CodeBuddy cookie and no local cache at {}",
                path.display()
            ))
        })?;
        let value: Value = serde_json::from_str(&raw)
            .map_err(|e| ProviderError::Parse(format!("Invalid cb_credits.json: {e}")))?;
        let snapshot = snapshot_from_cache_payload(&value)?;
        Ok(ProviderFetchResult::new(snapshot, "cli"))
    }

    fn resolve_cookie(&self, ctx: &FetchContext) -> Result<String, ProviderError> {
        if let Some(cookie) = ctx.manual_cookie_header.as_deref() {
            if let Some(normalized) = normalize_cookie_header(cookie) {
                return Ok(normalized);
            }
        }
        if let Some(from_file) = read_cookie_file() {
            return Ok(from_file);
        }
        let header = crate::providers::browser_cookie_header(&[
            "codebuddy.cn",
            "www.codebuddy.cn",
        ])?;
        normalize_cookie_header(&header).ok_or(ProviderError::NoCookies)
    }
}

impl Default for CodeBuddyProvider {
    fn default() -> Self {
        Self::new()
    }
}

fn request_body() -> Value {
    let codes: Vec<String> = package_codes();
    json!({
        "PageNumber": 1,
        "PageSize": 200,
        "ProductCode": PRODUCT_CODE,
        "Status": [0, 3],
        "OnlyValidPeriod": true,
        "PackageCodes": codes,
    })
}

fn package_codes() -> Vec<String> {
    if let Ok(raw) = std::env::var("CB_PACKAGE_CODES") {
        if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&raw) {
            let parsed: Vec<String> = items
                .into_iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .filter(|s| !s.is_empty())
                .collect();
            if !parsed.is_empty() {
                return parsed;
            }
        }
    }
    DEFAULT_PACKAGE_CODES
        .iter()
        .map(|s| (*s).to_string())
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

fn snapshot_from_api_payload(value: &Value) -> Result<UsageSnapshot, ProviderError> {
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
    let mut earliest_reset: Option<DateTime<Utc>> = None;

    for account in accounts {
        total += precise_or_field(account, "CapacitySizePrecise", "CapacitySize").unwrap_or(0.0);
        used += precise_or_field(account, "CapacityUsedPrecise", "CapacityUsed").unwrap_or(0.0);
        remaining +=
            precise_or_field(account, "CapacityRemainPrecise", "CapacityRemain").unwrap_or(0.0);
        if let Some(reset) = expire_time(account) {
            earliest_reset = Some(match earliest_reset {
                Some(prev) => prev.min(reset),
                None => reset,
            });
        }
    }

    // Prefer summed remaining when present; recompute used if inconsistent.
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

    let used_percent = if total > 0.0 {
        (used / total * 100.0).clamp(0.0, 100.0)
    } else if used > 0.0 {
        100.0
    } else {
        0.0
    };

    // Keep this short — tray card puts it on the right of the metric row
    // (`.menu-metric__reset { white-space: nowrap }`), so long copy overflows.
    let description = Some(format_credits_short(remaining, total));

    Ok(UsageSnapshot::new(RateWindow::with_details(
        used_percent,
        None,
        earliest_reset,
        description,
    ))
    .with_login_method("CodeBuddy CN"))
}

fn snapshot_from_cache_payload(value: &Value) -> Result<UsageSnapshot, ProviderError> {
    let total = number_field(value, &["total"]).ok_or_else(|| {
        ProviderError::Parse("cb_credits.json missing total".into())
    })?;
    let used = number_field(value, &["used"]).unwrap_or(0.0);
    let remaining = number_field(value, &["remaining"]).unwrap_or_else(|| (total - used).max(0.0));
    let used_percent = if total > 0.0 {
        (used / total * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    // Compact: "1,234 / 5,678 left" — avoid "(local cache)" which overflows tray.
    let description = Some(format_credits_short(remaining, total));
    // Prefer real package expiry from cache if present; do not treat updatedAt
    // as a quota reset (that previously looked like a bogus reset countdown).
    let reset = value
        .get("resetsAt")
        .and_then(|v| v.as_str())
        .and_then(parse_datetime);
    Ok(UsageSnapshot::new(RateWindow::with_details(
        used_percent,
        None,
        reset,
        description,
    ))
    .with_login_method("CodeBuddy CN"))
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

fn write_credits_cache_from_snapshot(usage: &UsageSnapshot) -> Result<(), String> {
    let path = credits_cache_path().ok_or_else(|| "no cache path".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Recover remaining/total from description when possible is fragile; store
    // used_percent + free-form fields derived from description text is worse.
    // Instead re-parse from the short description format "X / Y left".
    let (remaining, total) = parse_short_credits_desc(
        usage
            .primary
            .reset_description
            .as_deref()
            .unwrap_or_default(),
    )
    .unwrap_or((0.0, 0.0));
    let used = if total > 0.0 {
        (total - remaining).max(0.0)
    } else {
        0.0
    };
    let mut obj = serde_json::Map::new();
    obj.insert("total".into(), json!(round2(total)));
    obj.insert("used".into(), json!(round2(used)));
    obj.insert("remaining".into(), json!(round2(remaining)));
    obj.insert("source".into(), json!("api"));
    obj.insert("updatedAt".into(), json!(Utc::now().to_rfc3339()));
    if let Some(resets) = usage.primary.resets_at {
        obj.insert("resetsAt".into(), json!(resets.to_rfc3339()));
    }
    std::fs::write(
        path,
        serde_json::to_string_pretty(&Value::Object(obj)).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

fn parse_short_credits_desc(desc: &str) -> Option<(f64, f64)> {
    // "1,234.5 / 5,678 left"
    let left = desc.split(" left").next()?.trim();
    let (rem_s, tot_s) = left.split_once('/')?;
    let remaining = rem_s.trim().replace(',', "").parse::<f64>().ok()?;
    let total = tot_s.trim().replace(',', "").parse::<f64>().ok()?;
    Some((remaining, total))
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn is_retryable_error(err: &ProviderError) -> bool {
    match err {
        ProviderError::Network(_) => true,
        ProviderError::Other(msg) => {
            let lower = msg.to_ascii_lowercase();
            lower.contains("temporary")
                || lower.contains("waf")
                || lower.contains("timeout")
                || lower.contains("timed out")
                || lower.contains("connection")
                || lower.contains("http 429")
                || lower.contains("http 5")
                || lower.contains("502")
                || lower.contains("503")
                || lower.contains("504")
        }
        ProviderError::Parse(msg) => {
            // Transient HTML/WAF sometimes fails JSON parse mid-body.
            let lower = msg.to_ascii_lowercase();
            lower.contains("html") || lower.contains("eof") || lower.contains("expected")
        }
        _ => false,
    }
}

fn precise_or_field(account: &Value, precise_key: &str, fallback_key: &str) -> Option<f64> {
    number_field(account, &[precise_key]).or_else(|| number_field(account, &[fallback_key]))
}

fn expire_time(account: &Value) -> Option<DateTime<Utc>> {
    // Common field names observed on Tencent package objects.
    for key in [
        "ExpireTime",
        "expireTime",
        "ExpireTimeStamp",
        "EndTime",
        "endTime",
        "ValidEndTime",
    ] {
        if let Some(v) = account.get(key) {
            if let Some(s) = v.as_str() {
                if let Some(dt) = parse_datetime(s) {
                    return Some(dt);
                }
            }
            if let Some(n) = v.as_f64() {
                let seconds = if n > 10_000_000_000.0 { n / 1000.0 } else { n };
                if let Some(dt) = DateTime::<Utc>::from_timestamp(seconds as i64, 0) {
                    return Some(dt);
                }
            }
        }
    }
    None
}

fn number_field(value: &Value, keys: &[&str]) -> Option<f64> {
    for key in keys {
        match value.get(*key)? {
            Value::Number(n) => return n.as_f64(),
            Value::String(s) => {
                if let Ok(n) = s.trim().parse::<f64>() {
                    return Some(n);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_datetime(raw: &str) -> Option<DateTime<Utc>> {
    if let Ok(number) = raw.parse::<f64>() {
        let seconds = if number > 10_000_000_000.0 {
            number / 1000.0
        } else {
            number
        };
        return DateTime::<Utc>::from_timestamp(seconds as i64, 0);
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
                Ok(cookie) => match self.fetch_web(&cookie).await {
                    Ok(result) => Ok(result),
                    Err(err) if matches!(ctx.source_mode, SourceMode::Auto) => {
                        // Fall back to local normalized cache from statusline watch.
                        self.fetch_local_cache().or(Err(err))
                    }
                    Err(err) => Err(err),
                },
                Err(err) if matches!(ctx.source_mode, SourceMode::Auto) => {
                    self.fetch_local_cache().or(Err(err))
                }
                Err(err) => Err(err),
            },
            SourceMode::Cli => self.fetch_local_cache(),
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

    #[test]
    fn parses_get_user_resource_payload() {
        let payload = serde_json::json!({
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
        });
        let snapshot = snapshot_from_api_payload(&payload).unwrap();
        // used 111 / total 3100 ≈ 3.58%
        assert!((snapshot.primary.used_percent - (111.0 / 3100.0 * 100.0)).abs() < 0.01);
        assert_eq!(
            snapshot.primary.reset_description.as_deref(),
            Some("2,989 / 3,100 left")
        );
        assert!(snapshot.primary.resets_at.is_some());
    }

    #[test]
    fn formats_compact_credit_labels() {
        assert_eq!(format_credits_short(1989.0, 3100.0), "1,989 / 3,100 left");
        assert_eq!(format_credits_short(12.5, 100.0), "12.5 / 100 left");
    }

    #[test]
    fn empty_accounts_errors_with_hint() {
        let payload = serde_json::json!({
            "code": 0,
            "data": { "Response": { "Data": { "Accounts": [] } } }
        });
        let err = snapshot_from_api_payload(&payload).unwrap_err();
        assert!(format!("{err}").contains("PackageCodes") || format!("{err}").contains("package"));
    }

    #[test]
    fn non_zero_code_errors() {
        let payload = serde_json::json!({ "code": 14001, "msg": "quote exceeded" });
        assert!(snapshot_from_api_payload(&payload).is_err());
    }

    #[test]
    fn parses_local_cache() {
        let payload = serde_json::json!({
            "total": 3100,
            "used": 111,
            "remaining": 2989,
            "source": "api",
            "updatedAt": "2026-08-07T00:00:00Z"
        });
        let snapshot = snapshot_from_cache_payload(&payload).unwrap();
        assert!((snapshot.primary.used_percent - (111.0 / 3100.0 * 100.0)).abs() < 0.01);
    }

    #[test]
    fn normalizes_cookie_with_caret_escapes() {
        assert_eq!(
            normalize_cookie_header("Cookie: a=1^|2; b=3").as_deref(),
            Some("a=1|2; b=3")
        );
    }
}
