//! Command Code provider implementation.
//!
//! Uses a browser session cookie to fetch monthly and purchased credit balances.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use regex_lite::Regex;
use reqwest::Client;
use serde_json::Value;

use crate::core::{
    CostSnapshot, FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId,
    ProviderMetadata, RateWindow, SourceMode, UsageSnapshot,
};

const COMMAND_CODE_API_BASE: &str = "https://api.commandcode.ai";
const COMMAND_CODE_CREDITS_PATH: &str = "/internal/billing/credits";
const COMMAND_CODE_SUBSCRIPTIONS_PATH: &str = "/internal/billing/subscriptions";

/// Rolling-limit window durations reported under `windowLimits` (upstream
/// `CommandCodeUsageFetcher`): five-hour and weekly rolling caps.
const FIVE_HOUR_WINDOW_MINUTES: u32 = 5 * 60;
const WEEKLY_WINDOW_MINUTES: u32 = 7 * 24 * 60;

/// Recognized better-auth session cookie names in upstream priority order
/// (upstream `CommandCodeCookieHeader.supportedSessionCookieNames`). #2706 added
/// the production `commandcode_prod_` family; the legacy `better-auth` names stay
/// as fallback. Name matching against a pasted header is case-insensitive, and a
/// bare pasted token keeps the legacy production name until a renamed production
/// cookie is proven live (same rule as upstream).
const SESSION_COOKIE_NAMES: &[&str] = &[
    "__Secure-commandcode_prod_.session_token",
    "commandcode_prod_.session_token",
    "__Host-commandcode_prod_.session_token",
    "__Host-better-auth.session_token",
    "__Secure-better-auth.session_token",
    "better-auth.session_token",
];
const LEGACY_BARE_TOKEN_COOKIE_NAME: &str = "__Secure-better-auth.session_token";

/// Static `planId` → monthly grant catalog mirroring upstream
/// `CommandCodePlanCatalog`. The credits endpoint only reports the *remaining*
/// monthly grant; the plan totals come from the public pricing page (#2706 added
/// `individual-goat` at $70/mo).
struct CommandCodePlan {
    id: &'static str,
    display_name: &'static str,
    monthly_credits_usd: f64,
}

const PLANS: &[CommandCodePlan] = &[
    CommandCodePlan {
        id: "individual-go",
        display_name: "Go",
        monthly_credits_usd: 10.0,
    },
    CommandCodePlan {
        id: "individual-goat",
        display_name: "GOAT",
        monthly_credits_usd: 70.0,
    },
    CommandCodePlan {
        id: "individual-pro",
        display_name: "Pro",
        monthly_credits_usd: 30.0,
    },
    CommandCodePlan {
        id: "individual-max",
        display_name: "Max",
        monthly_credits_usd: 150.0,
    },
    CommandCodePlan {
        id: "individual-ultra",
        display_name: "Ultra",
        monthly_credits_usd: 300.0,
    },
];

fn find_plan(plan_id: &str) -> Option<&'static CommandCodePlan> {
    let normalized = plan_id.trim().to_ascii_lowercase();
    PLANS.iter().find(|plan| plan.id == normalized)
}

pub struct CommandCodeProvider {
    metadata: ProviderMetadata,
    client: Client,
}

impl CommandCodeProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::CommandCode,
                display_name: "Command Code",
                session_label: "5-hour",
                weekly_label: "Weekly",
                supports_opus: false,
                supports_credits: true,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://commandcode.ai"),
                status_page_url: None,
            },
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    async fn fetch_web(&self, cookie_header: &str) -> Result<ProviderFetchResult, ProviderError> {
        let cookie_header =
            normalize_cookie_header(cookie_header).ok_or_else(|| ProviderError::NoCookies)?;
        let credits = self
            .get_json(
                &format!("{COMMAND_CODE_API_BASE}{COMMAND_CODE_CREDITS_PATH}"),
                &cookie_header,
            )
            .await?;
        let subscription = self
            .get_json(
                &format!("{COMMAND_CODE_API_BASE}{COMMAND_CODE_SUBSCRIPTIONS_PATH}"),
                &cookie_header,
            )
            .await
            .ok();
        result_from_payloads(&credits, subscription.as_ref())
    }

    async fn get_json(&self, url: &str, cookie_header: &str) -> Result<Value, ProviderError> {
        let response = self
            .client
            .get(url)
            .header("Cookie", cookie_header)
            .header("Accept", "application/json, text/plain, */*")
            .header("Origin", "https://commandcode.ai")
            .header("Referer", "https://commandcode.ai/")
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            || response.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(ProviderError::AuthRequired);
        }
        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Command Code API returned status {}",
                response.status()
            )));
        }
        response.json::<Value>().await.map_err(|e| {
            ProviderError::Parse(format!("Failed to parse Command Code response: {e}"))
        })
    }

    /// Cookie refresh path (upstream #2564):
    /// 1. Try last validated cached cookie header
    /// 2. On auth failure: clear cache, re-import browser cookies, validate, store
    async fn fetch_with_cookie_refresh(&self) -> Result<ProviderFetchResult, ProviderError> {
        use crate::browser::cookie_cache::CookieHeaderCache;

        if let Some(cached) = CookieHeaderCache::load(ProviderId::CommandCode) {
            match self.fetch_web(&cached.cookie_header).await {
                Ok(result) => return Ok(result),
                Err(ProviderError::AuthRequired) => {
                    CookieHeaderCache::clear(ProviderId::CommandCode);
                }
                Err(err) => return Err(err),
            }
        }

        let cookie_header = crate::providers::browser_cookie_header(&["commandcode.ai"])?;
        let result = self.fetch_web(&cookie_header).await?;
        let _ = CookieHeaderCache::store(ProviderId::CommandCode, &cookie_header, "browser");
        Ok(result)
    }
}

fn normalize_cookie_header(raw: &str) -> Option<String> {
    let extracted;
    let mut header = raw.trim();
    if let Some(cookie_header) = cookie_header_from_curl(raw) {
        extracted = cookie_header;
        header = extracted.trim();
    } else if looks_like_curl_capture(header) {
        return None;
    }
    if header
        .get(.."cookie:".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("cookie:"))
    {
        header = header["cookie:".len()..].trim();
    }
    if header.is_empty() {
        return None;
    }
    if !header.contains('=') && !header.contains(';') {
        // Bare token — assume the established production cookie name (upstream
        // #2706 keeps the legacy better-auth default until a renamed production
        // cookie is proven live).
        return Some(format!("{LEGACY_BARE_TOKEN_COOKIE_NAME}={header}"));
    }

    // A pasted header narrows to the session cookie only (upstream
    // `CommandCodeCookieHeader.override(from:)`); forwarding unrelated cookies
    // (analytics, stripe ids, session_data sidecars) is unnecessary for auth.
    let (name, token) = extract_session_cookie(header)?;
    Some(format!("{name}={token}"))
}

/// Pick the better-auth session cookie out of a `name=value; …` header by the
/// upstream priority list, case-insensitively, preserving the header's own
/// name casing and token bytes.
fn extract_session_cookie(header: &str) -> Option<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for chunk in header.split(';') {
        let Some((name, value)) = chunk.trim().split_once('=') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() || value.is_empty() {
            continue;
        }
        pairs.push((name.to_string(), value.to_string()));
    }
    for expected in SESSION_COOKIE_NAMES {
        for (name, value) in &pairs {
            if name.eq_ignore_ascii_case(expected) {
                return Some((name.clone(), value.clone()));
            }
        }
    }
    None
}

fn cookie_header_from_curl(raw: &str) -> Option<String> {
    let re =
        Regex::new(r#"(?s)(?:^|\s)(?:-H|--header)(?:\s+|=)(?:'([^']*)'|"([^"]*)"|(\S+))"#).ok()?;
    re.captures_iter(raw).find_map(|caps| {
        let field = caps
            .get(1)
            .or_else(|| caps.get(2))
            .or_else(|| caps.get(3))?
            .as_str();
        let field = unescape_shell_segment(field);
        let (name, value) = split_header(&field)?;
        name.eq_ignore_ascii_case("cookie")
            .then(|| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn looks_like_curl_capture(raw: &str) -> bool {
    let lower = raw.trim_start().to_ascii_lowercase();
    lower.starts_with("curl ") || lower.starts_with("curl.exe ")
}

fn split_header(field: &str) -> Option<(&str, &str)> {
    let colon = field.find(':')?;
    Some((field[..colon].trim(), field[colon + 1..].trim()))
}

fn unescape_shell_segment(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                output.push(next);
            }
        } else {
            output.push(ch);
        }
    }
    output
}

fn result_from_payloads(
    credits_payload: &Value,
    subscription_payload: Option<&Value>,
) -> Result<ProviderFetchResult, ProviderError> {
    let credits = credits_payload
        .get("credits")
        .ok_or_else(|| ProviderError::Parse("Command Code credits object missing".into()))?;
    let monthly_credits = number(credits.get("monthlyCredits"))
        .ok_or_else(|| ProviderError::Parse("Command Code monthlyCredits missing".into()))?;
    let purchased = number(credits.get("purchasedCredits")).unwrap_or(0.0);
    let premium = number(credits.get("premiumMonthlyCredits")).unwrap_or(0.0);
    let open_source = number(credits.get("opensourceMonthlyCredits")).unwrap_or(0.0);

    // Upstream 0.48.0 F12: rolling 5-hour/weekly limits ride alongside the
    // monthly credits, under `windowLimits` at the root or inside `credits`.
    let five_hour = limit_window(
        window_limits(credits_payload).and_then(|limits| limits.get("fiveHour")),
        FIVE_HOUR_WINDOW_MINUTES,
    );
    let weekly = limit_window(
        window_limits(credits_payload).and_then(|limits| limits.get("weekly")),
        WEEKLY_WINDOW_MINUTES,
    );

    let period_end = subscription_payload
        .and_then(|root| root.get("data"))
        .and_then(|data| data.get("currentPeriodEnd"))
        .and_then(|value| value.as_str())
        .and_then(parse_datetime);
    let plan = subscription_payload
        .and_then(|root| root.get("data"))
        .and_then(|data| data.get("planId"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .and_then(find_plan);
    let monthly_window = monthly_window(monthly_credits, purchased, plan, period_end);

    // Slot ordering mirrors upstream `toUsageSnapshot` (5-hour → weekly →
    // monthly); the local snapshot requires a primary, so an API without
    // `windowLimits` keeps the pre-F12 monthly-primary layout.
    let mut snapshot = if let Some(five) = five_hour {
        let mut snapshot = UsageSnapshot::new(five);
        if let Some(weekly) = weekly {
            snapshot = snapshot.with_secondary(weekly);
        }
        if let Some(monthly) = monthly_window {
            snapshot = snapshot.with_tertiary(monthly);
        }
        snapshot
    } else if let Some(weekly) = weekly {
        let mut snapshot = UsageSnapshot::new(weekly);
        if let Some(monthly) = monthly_window {
            snapshot = snapshot.with_tertiary(monthly);
        }
        snapshot
    } else if let Some(monthly) = monthly_window {
        UsageSnapshot::new(monthly)
    } else {
        UsageSnapshot::new(RateWindow::with_details(
            0.0,
            None,
            period_end,
            Some(format!("{monthly_credits:.2} monthly credits remaining")),
        ))
    };

    if let Some(method) = login_method(monthly_credits, purchased, plan) {
        snapshot = snapshot.with_login_method(method);
    }

    let (used, limit) = match plan {
        Some(plan) => (
            (plan.monthly_credits_usd - monthly_credits).clamp(0.0, plan.monthly_credits_usd),
            plan.monthly_credits_usd,
        ),
        None => {
            let total = premium + open_source;
            ((total - monthly_credits).max(0.0), total.max(0.0))
        }
    };
    let cost = CostSnapshot::new(used, "USD", "monthly credits").with_limit(limit);
    Ok(ProviderFetchResult::new(snapshot, "web").with_cost(cost))
}

fn window_limits(credits_payload: &Value) -> Option<&Value> {
    credits_payload.get("windowLimits").or_else(|| {
        credits_payload
            .get("credits")
            .and_then(|credits| credits.get("windowLimits"))
    })
}

/// A rolling `windowLimits.fiveHour|weekly` entry: `{cap, used, resetAt}`.
/// `cap` must be positive (number-or-string coercion) and `usedPercent` is the
/// clamped `used / cap` ratio, matching upstream `UsagePercent.displayClamped`.
fn limit_window(value: Option<&Value>, window_minutes: u32) -> Option<RateWindow> {
    let limit = value?;
    let cap = number(limit.get("cap"))?;
    if cap <= 0.0 {
        return None;
    }
    let used = number(limit.get("used")).unwrap_or(0.0);
    let used_percent = (used / cap * 100.0).clamp(0.0, 100.0);
    Some(RateWindow::with_details(
        used_percent,
        Some(window_minutes),
        coerce_reset_at(limit.get("resetAt")),
        None,
    ))
}

/// `resetAt` arrives as epoch seconds, epoch milliseconds, or an ISO-8601
/// string (upstream `CommandCodeUsageFetcher.date(from:)`).
fn coerce_reset_at(value: Option<&Value>) -> Option<DateTime<Utc>> {
    let value = value?;
    if let Some(timestamp) = number(Some(value))
        && timestamp > 0.0
    {
        let seconds = if timestamp > 10_000_000_000.0 {
            timestamp / 1000.0
        } else {
            timestamp
        };
        return DateTime::from_timestamp(seconds as i64, 0);
    }
    value.as_str().and_then(|text| parse_datetime(text.trim()))
}

/// Monthly grant window from the plan catalog (upstream `makeMonthlyWindow`):
/// catalog total − remaining, clamped to [0, total]. Without a recognized plan
/// the fallback keeps a visible-but-empty bar when credits remain, and no
/// window at all when the account holds nothing.
fn monthly_window(
    monthly_remaining: f64,
    purchased: f64,
    plan: Option<&CommandCodePlan>,
    period_end: Option<DateTime<Utc>>,
) -> Option<RateWindow> {
    if let Some(plan) = plan
        && plan.monthly_credits_usd > 0.0
    {
        let total = plan.monthly_credits_usd;
        let used = (total - monthly_remaining).clamp(0.0, total);
        return Some(RateWindow::with_details(
            (used / total * 100.0).clamp(0.0, 100.0),
            RateWindow::monthly_window_minutes(period_end),
            period_end,
            None,
        ));
    }
    if monthly_remaining > 0.0 || purchased > 0.0 {
        return Some(RateWindow::with_details(
            0.0,
            RateWindow::monthly_window_minutes(period_end),
            period_end,
            None,
        ));
    }
    None
}

/// Plan summary line, upstream shape: `GOAT · $61.50 of $70.00 · + $2.00 credits`.
fn login_method(
    monthly_remaining: f64,
    purchased: f64,
    plan: Option<&CommandCodePlan>,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(plan) = plan {
        parts.push(plan.display_name.to_string());
        let used =
            (plan.monthly_credits_usd - monthly_remaining).clamp(0.0, plan.monthly_credits_usd);
        parts.push(format!(
            "{} of {}",
            format_usd(used),
            format_usd(plan.monthly_credits_usd)
        ));
    } else if monthly_remaining > 0.0 {
        parts.push(format!("{} remaining", format_usd(monthly_remaining)));
    }
    if purchased > 0.0 {
        parts.push(format!("+ {} credits", format_usd(purchased)));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// Upstream `formatUSD`: under $100 keeps two decimals, $100+ rounds to whole.
fn format_usd(value: f64) -> String {
    if value.abs() < 100.0 {
        format!("${value:.2}")
    } else {
        format!("${value:.0}")
    }
}

fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn parse_datetime(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.with_timezone(&Utc))
}

impl Default for CommandCodeProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for CommandCodeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::CommandCode
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto | SourceMode::Web => {
                if let Some(cookie) = ctx.manual_cookie_header.as_deref() {
                    return self.fetch_web(cookie).await;
                }
                self.fetch_with_cookie_refresh().await
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const WINDOW_LIMITS_ROOT: &str =
        include_str!("../fixtures/commandcode/window-limits-root.json");
    const WINDOW_LIMITS_NESTED: &str =
        include_str!("../fixtures/commandcode/window-limits-nested.json");

    #[test]
    fn command_code_accepts_bare_session_token() {
        // Upstream #2706: bare tokens keep the legacy production cookie name.
        assert_eq!(
            normalize_cookie_header("abc123").as_deref(),
            Some("__Secure-better-auth.session_token=abc123")
        );
    }

    #[test]
    fn command_code_narrows_header_to_production_session_cookie() {
        assert_eq!(
            normalize_cookie_header(
                "__Secure-commandcode_prod_.session_token=token; __Secure-commandcode_prod_.session_data=data; stripe=ignored"
            )
            .as_deref(),
            Some("__Secure-commandcode_prod_.session_token=token")
        );
    }

    #[test]
    fn command_code_rejects_header_without_session_cookie() {
        assert_eq!(
            normalize_cookie_header("Cookie: sidebar=value; stripe_mid=mid"),
            None
        );
        assert_eq!(normalize_cookie_header("not-a-cookie; also-bad"), None);
    }

    #[test]
    fn command_code_prefers_new_production_cookie_family() {
        // Upstream priority order: commandcode_prod_ names win over the legacy
        // better-auth family when a pasted header carries both.
        assert_eq!(
            normalize_cookie_header(
                "__Secure-better-auth.session_token=legacy; commandcode_prod_.session_token=prod"
            )
            .as_deref(),
            Some("commandcode_prod_.session_token=prod")
        );
    }

    #[test]
    fn command_code_matches_session_cookie_case_insensitively() {
        assert_eq!(
            normalize_cookie_header("__SECURE-COMMANDCODE_PROD_.SESSION_TOKEN=token").as_deref(),
            Some("__SECURE-COMMANDCODE_PROD_.SESSION_TOKEN=token")
        );
        assert_eq!(
            normalize_cookie_header("__Host-better-auth.session_token=legacy").as_deref(),
            Some("__Host-better-auth.session_token=legacy")
        );
    }

    #[test]
    fn command_code_extracts_cookie_header_from_curl() {
        let curl = r#"curl 'https://commandcode.ai' -H 'User-Agent: Browser' -H 'Cookie: __Secure-commandcode_prod_.session_token=token; __Secure-commandcode_prod_.session_data=data' "#;
        assert_eq!(
            normalize_cookie_header(curl).as_deref(),
            Some("__Secure-commandcode_prod_.session_token=token")
        );
    }

    #[test]
    fn command_code_rejects_curl_without_cookie_header() {
        let curl = r#"curl 'https://commandcode.ai' -H 'User-Agent: Browser'"#;
        assert_eq!(normalize_cookie_header(curl), None);
    }

    #[test]
    fn command_code_rejects_empty_or_malformed_cookie_header() {
        assert_eq!(normalize_cookie_header("Cookie:   "), None);
    }

    #[test]
    fn command_code_result_without_windows_keeps_monthly_layout() {
        let result = result_from_payloads(
            &json!({"credits":{"monthlyCredits":25,"purchasedCredits":2,"premiumMonthlyCredits":100}}),
            None,
        )
        .unwrap();
        // No windowLimits and no recognized plan → free/unknown-plan fallback:
        // a visible-but-empty monthly bar (upstream makeMonthlyWindow fallback).
        assert_eq!(result.usage.primary.used_percent, 0.0);
        assert!(result.usage.secondary.is_none());
        assert_eq!(
            result.usage.login_method.as_deref(),
            Some("$25.00 remaining · + $2.00 credits")
        );
        let cost = result.cost.expect("monthly credits cost");
        assert_eq!(cost.used, 75.0);
        assert_eq!(cost.limit, Some(100.0));
    }

    // ── F12: windowLimits parsing (upstream fixtures, copied verbatim) ───

    #[test]
    fn window_limits_root_fixture_maps_5h_weekly_monthly_slots() {
        let credits: Value = serde_json::from_str(WINDOW_LIMITS_ROOT).unwrap();
        let result = result_from_payloads(&credits, None).unwrap();
        let usage = result.usage;

        // fiveHour: cap 3, used 0.75 → 25%, 5×60 minutes, ms-epoch reset.
        assert_eq!(usage.primary.used_percent, 25.0);
        assert_eq!(usage.primary.window_minutes, Some(300));
        assert_eq!(
            usage.primary.resets_at.map(|ts| ts.timestamp()),
            Some(1_780_000_000)
        );

        // weekly: cap 15, used 1.5 → 10%, 7×24×60 minutes.
        let weekly = usage.secondary.expect("weekly window");
        assert_eq!(weekly.used_percent, 10.0);
        assert_eq!(weekly.window_minutes, Some(10080));
        assert_eq!(
            weekly.resets_at.map(|ts| ts.timestamp()),
            Some(1_780_100_000)
        );

        // No subscription payload → unknown plan → monthly fallback bar at 0%.
        let monthly = usage.tertiary.expect("monthly window");
        assert_eq!(monthly.used_percent, 0.0);
        assert_eq!(usage.login_method.as_deref(), Some("$8.50 remaining"));
    }

    #[test]
    fn window_limits_nested_fixture_coerces_string_numbers_and_epoch_seconds() {
        let credits: Value = serde_json::from_str(WINDOW_LIMITS_NESTED).unwrap();
        let result = result_from_payloads(&credits, None).unwrap();
        let usage = result.usage;

        // "4"/"1" strings coerce; resetAt "1780200000" reads as seconds.
        assert_eq!(usage.primary.used_percent, 25.0);
        assert_eq!(
            usage.primary.resets_at.map(|ts| ts.timestamp()),
            Some(1_780_200_000)
        );
        // weekly numeric cap 20/used 4 → 20%, ms-epoch reset.
        let weekly = usage.secondary.expect("weekly window");
        assert_eq!(weekly.used_percent, 20.0);
        assert_eq!(
            weekly.resets_at.map(|ts| ts.timestamp()),
            Some(1_780_300_000)
        );
        assert_eq!(
            usage.login_method.as_deref(),
            Some("$7.25 remaining · + $2.00 credits")
        );
    }

    #[test]
    fn window_limits_ignore_nonpositive_caps() {
        let result = result_from_payloads(
            &json!({
                "credits": {"monthlyCredits": 5},
                "windowLimits": {
                    "fiveHour": {"cap": 0, "used": 1, "resetAt": 1780000000},
                    "weekly": {"cap": -3, "used": 1}
                }
            }),
            None,
        )
        .unwrap();
        // No usable rolling windows: monthly fallback becomes primary.
        assert_eq!(result.usage.primary.used_percent, 0.0);
        assert!(result.usage.secondary.is_none());
        assert!(result.usage.tertiary.is_none());
    }

    // ── F13: plan catalog (upstream #2706) ───

    #[test]
    fn plan_catalog_recognizes_all_tiers_case_insensitively() {
        assert_eq!(find_plan("individual-goat").unwrap().display_name, "GOAT");
        assert_eq!(
            find_plan("individual-goat").unwrap().monthly_credits_usd,
            70.0
        );
        assert_eq!(find_plan("Individual-ULTRA").unwrap().display_name, "Ultra");
        assert!(find_plan("team").is_none());
        assert!(find_plan("").is_none());
    }

    #[test]
    fn goat_plan_drives_monthly_window_and_login_method() {
        let subscription = json!({
            "data": {
                "planId": "individual-goat",
                "currentPeriodEnd": "2026-06-16T04:26:40.371Z"
            }
        });
        let credits: Value = serde_json::from_str(WINDOW_LIMITS_ROOT).unwrap();
        let result = result_from_payloads(&credits, Some(&subscription)).unwrap();

        // $70 grant, $8.50 remaining → 61.5 used → 87.857…%.
        let monthly = result.usage.tertiary.expect("monthly window");
        assert!((monthly.used_percent - 87.857).abs() < 0.01, "{monthly:?}");
        assert!(monthly.window_minutes.is_some(), "calendar-month minutes");
        assert_eq!(
            monthly.resets_at.map(|ts| ts.to_rfc3339()),
            Some("2026-06-16T04:26:40.371+00:00".to_string())
        );
        assert_eq!(
            result.usage.login_method.as_deref(),
            Some("GOAT · $61.50 of $70.00")
        );
        let cost = result.cost.expect("monthly credits cost");
        assert_eq!(cost.used, 61.5);
        assert_eq!(cost.limit, Some(70.0));
    }

    #[test]
    fn empty_account_has_no_monthly_window() {
        let result = result_from_payloads(
            &json!({"credits":{"monthlyCredits":0,"purchasedCredits":0}}),
            None,
        )
        .unwrap();
        assert_eq!(result.usage.primary.used_percent, 0.0);
        assert_eq!(
            result.usage.primary.reset_description.as_deref(),
            Some("0.00 monthly credits remaining")
        );
        assert!(result.usage.tertiary.is_none());
        assert!(result.usage.login_method.is_none());
    }
}
