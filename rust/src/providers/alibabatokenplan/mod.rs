//! Alibaba Token Plan provider implementation.
//!
//! Fetches Bailian / Model Studio token-plan credits from the same commerce
//! endpoints used by the upstream macOS provider. Authentication uses browser
//! cookies or a manually pasted cookie header.
//!
//! Team path: `GetSubscriptionSummary` / BssOpenAPI-V3 (+ optional sec_token).
//! Personal/Solo path: OneConsole personal token-plan APIs (+ best-effort sec_token).

mod cli;
mod personal;
mod region;

pub use region::AlibabaTokenPlanRegion;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use regex_lite::Regex;
use serde_json::Value;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};
use crate::providers::browser_cookie_header;

use region::AlibabaTokenPlanRegion as Region;

const DEFAULT_DASHBOARD_URL: &str =
    "https://bailian.console.aliyun.com/cn-beijing?tab=plan#/efm/subscription/token-plan";
pub(super) const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";
pub(super) const LANGUAGE: &str = "en-US";
pub(super) const PERSONAL_CONSOLE_PRODUCT: &str = "sfm_bailian";
pub(super) const PERSONAL_USAGE_API: &str = "zeldaHttp.apikeyMgr./tokenplan/personal/api/v2/usage";
pub(super) const PERSONAL_SUBSCRIPTION_API: &str =
    "zeldaHttp.apikeyMgr./tokenplan/personal/api/v2/subscription";
pub(super) const PERSONAL_QUOTA_CONFIG_API: &str =
    "zeldaHttp.apikeyMgr./tokenplan/personal/api/v2/quota-config";

const FIVE_HOUR_MINUTES: u32 = 5 * 60;
const WEEKLY_MINUTES: u32 = 7 * 24 * 60;
const LEGACY_MINUTES: u32 = 30 * 24 * 60;

pub struct AlibabaTokenPlanProvider {
    metadata: ProviderMetadata,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct TokenPlanSnapshot {
    pub(super) plan_name: Option<String>,
    pub(super) used_quota: Option<f64>,
    pub(super) total_quota: Option<f64>,
    pub(super) remaining_quota: Option<f64>,
    pub(super) resets_at: Option<DateTime<Utc>>,
    pub(super) five_hour_used_percent: Option<f64>,
    pub(super) five_hour_total_quota: Option<f64>,
    pub(super) five_hour_resets_at: Option<DateTime<Utc>>,
    pub(super) weekly_used_percent: Option<f64>,
    pub(super) weekly_total_quota: Option<f64>,
    pub(super) weekly_resets_at: Option<DateTime<Utc>>,
}

impl AlibabaTokenPlanProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::AlibabaTokenPlan,
                display_name: "Alibaba Token Plan",
                session_label: "Credits",
                weekly_label: "Usage",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some(DEFAULT_DASHBOARD_URL),
                status_page_url: Some("https://status.aliyun.com"),
            },
        }
    }

    fn resolve_region(ctx: &FetchContext) -> Region {
        Region::from_settings_value(ctx.api_region.as_deref())
    }

    async fn fetch_via_cli(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProviderError> {
        let snapshot = cli::fetch_cli_usage(Self::resolve_region(ctx)).await?;
        Self::snapshot_to_usage(snapshot)
    }

    async fn fetch_via_web(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProviderError> {
        let region = Self::resolve_region(ctx);
        let cookie_header = Self::resolve_cookie_header(ctx, region)?;
        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(ctx.web_timeout.max(1)))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        let snapshot = if region.uses_personal_api() {
            let sec_token = Self::resolve_sec_token(&client, &cookie_header, region, ctx).await;
            personal::fetch_personal_usage(
                &client,
                &cookie_header,
                region,
                sec_token.as_deref(),
                ctx,
            )
            .await?
        } else {
            self.fetch_team_usage(&client, &cookie_header, region, ctx)
                .await?
        };
        Self::snapshot_to_usage(snapshot)
    }

    async fn fetch_team_usage(
        &self,
        client: &reqwest::Client,
        cookie_header: &str,
        region: Region,
        ctx: &FetchContext,
    ) -> Result<TokenPlanSnapshot, ProviderError> {
        let sec_token = Self::resolve_sec_token(client, cookie_header, region, ctx).await;
        let mut form = vec![
            ("product", "BssOpenAPI-V3".to_string()),
            ("action", "GetSubscriptionSummary".to_string()),
            ("params", Self::team_request_params(region)),
            ("region", region.current_region_id().to_string()),
        ];
        if let Some(token) = sec_token
            .as_deref()
            .filter(|token| !token.trim().is_empty())
        {
            form.push(("sec_token", token.to_string()));
        }
        let mut request = client
            .post(Self::team_quota_url(region))
            .header("Cookie", cookie_header)
            .header("Accept", "*/*")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Origin", region.gateway_base_url())
            .header("Referer", region.dashboard_url())
            .header("User-Agent", USER_AGENT)
            .header("X-Requested-With", "XMLHttpRequest")
            .form(&form);

        if let Some(csrf) = cookie_value("login_aliyunid_csrf", cookie_header)
            .or_else(|| cookie_value("csrf", cookie_header))
        {
            request = request
                .header("x-xsrf-token", csrf.clone())
                .header("x-csrf-token", csrf);
        }

        let response = request.send().await?;
        let status = response.status();
        let body = response.bytes().await?;
        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                return Err(ProviderError::AuthRequired);
            }
            return Err(ProviderError::Other(format!(
                "Alibaba Token Plan API error: HTTP {status}"
            )));
        }

        Self::parse_usage_snapshot(&body)
    }

    fn resolve_cookie_header(ctx: &FetchContext, region: Region) -> Result<String, ProviderError> {
        if let Some(raw) = ctx
            .manual_cookie_header
            .as_deref()
            .and_then(normalize_cookie_header)
        {
            return Ok(raw);
        }
        for env_name in [
            "ALIBABA_TOKEN_PLAN_COOKIE",
            "ALIBABA_TOKEN_PLAN_COOKIE_HEADER",
            "BAILIAN_TOKEN_PLAN_COOKIE",
        ] {
            if let Ok(raw) = std::env::var(env_name)
                && let Some(header) = normalize_cookie_header(&raw)
            {
                return Ok(header);
            }
        }
        browser_cookie_header(region.cookie_domains())
            .and_then(|header| normalize_cookie_header(&header).ok_or(ProviderError::NoCookies))
    }

    async fn resolve_sec_token(
        client: &reqwest::Client,
        cookie_header: &str,
        region: Region,
        ctx: &FetchContext,
    ) -> Option<String> {
        let response = client
            .get(region.dashboard_url())
            .timeout(std::time::Duration::from_secs(ctx.web_timeout.clamp(1, 10)))
            .header("Cookie", cookie_header)
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .header("Referer", dashboard_referer(region))
            .header("Sec-Fetch-Site", "same-origin")
            .header("Sec-Fetch-Mode", "navigate")
            .header("Sec-Fetch-Dest", "document")
            .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
            .header("User-Agent", USER_AGENT)
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return cookie_value("sec_token", cookie_header);
        }
        let text = response.text().await.ok()?;
        extract_sec_token(&text).or_else(|| cookie_value("sec_token", cookie_header))
    }

    fn team_quota_url(region: Region) -> String {
        format!(
            "{}/data/api.json?action=GetSubscriptionSummary&product=BssOpenAPI-V3&_tag=",
            region.gateway_base_url()
        )
    }

    fn team_request_params(region: Region) -> String {
        serde_json::json!({
            "ProductCode": region.product_code(),
        })
        .to_string()
    }

    fn parse_usage_snapshot(data: &[u8]) -> Result<TokenPlanSnapshot, ProviderError> {
        if data.is_empty() {
            return Err(ProviderError::Parse(
                "Empty Alibaba Token Plan response".into(),
            ));
        }
        let value: Value = serde_json::from_slice(data).map_err(|_| {
            if is_likely_login_html(data) {
                ProviderError::AuthRequired
            } else {
                ProviderError::Parse("Invalid Alibaba Token Plan JSON response".into())
            }
        })?;
        let expanded = expand_json_strings(value);
        throw_if_error_payload(&expanded)?;

        let instance = find_token_plan_instance(&expanded);
        let plan_name = instance
            .as_ref()
            .and_then(find_plan_name)
            .or_else(|| find_plan_name(&expanded));
        let quota_source = instance
            .as_ref()
            .and_then(find_quota_info)
            .or_else(|| find_quota_info(&expanded));
        let used = quota_source
            .as_ref()
            .and_then(|v| first_f64(v, USED_QUOTA_KEYS));
        let total = quota_source
            .as_ref()
            .and_then(|v| first_f64(v, TOTAL_QUOTA_KEYS));
        let remaining = quota_source
            .as_ref()
            .and_then(|v| first_f64(v, REMAINING_QUOTA_KEYS));
        let resets_at = instance
            .as_ref()
            .and_then(find_reset_date)
            .or_else(|| find_reset_date(&expanded));

        if plan_name.is_none() && total.is_none() && used.is_none() && remaining.is_none() {
            return Err(ProviderError::Parse(format!(
                "Missing Alibaba Token Plan data ({})",
                payload_diagnostics(&expanded)
            )));
        }

        Ok(TokenPlanSnapshot {
            plan_name,
            used_quota: used,
            total_quota: total,
            remaining_quota: remaining,
            resets_at,
            five_hour_used_percent: None,
            five_hour_total_quota: None,
            five_hour_resets_at: None,
            weekly_used_percent: None,
            weekly_total_quota: None,
            weekly_resets_at: None,
        })
    }

    pub(super) fn snapshot_to_usage(
        snapshot: TokenPlanSnapshot,
    ) -> Result<UsageSnapshot, ProviderError> {
        let five_hour = snapshot.five_hour_used_percent.map(|percent| {
            RateWindow::with_details(
                percent,
                Some(FIVE_HOUR_MINUTES),
                snapshot.five_hour_resets_at,
                quota_detail_percent(percent, snapshot.five_hour_total_quota),
            )
        });
        let legacy = used_percent(
            snapshot.used_quota,
            snapshot.total_quota,
            snapshot.remaining_quota,
        )
        .map(|percent| {
            RateWindow::with_details(
                percent,
                RateWindow::monthly_window_minutes(snapshot.resets_at).or(Some(LEGACY_MINUTES)),
                snapshot.resets_at,
                quota_detail(
                    snapshot.used_quota,
                    snapshot.total_quota,
                    snapshot.remaining_quota,
                ),
            )
        });
        let weekly = snapshot.weekly_used_percent.map(|percent| {
            RateWindow::with_details(
                percent,
                Some(WEEKLY_MINUTES),
                snapshot.weekly_resets_at,
                quota_detail_percent(percent, snapshot.weekly_total_quota),
            )
        });
        // Prefer the 5-hour window, then the Team/legacy credit envelope. Personal/Solo
        // payloads sometimes expose only `per1WeekPercentage`; promote that window to
        // primary instead of failing the whole fetch.
        let (primary, secondary) = match (five_hour.or(legacy), weekly) {
            (Some(primary), secondary) => (primary, secondary),
            (None, Some(weekly)) => (weekly, None),
            (None, None) => {
                return Err(ProviderError::Parse(
                    "Alibaba Token Plan quota totals missing".into(),
                ));
            }
        };
        let mut usage = UsageSnapshot::new(primary);
        if let Some(secondary) = secondary {
            usage = usage.with_secondary(secondary);
        }

        if let Some(plan) = snapshot.plan_name.filter(|plan| !plan.trim().is_empty()) {
            usage = usage.with_login_method(plan);
        }
        Ok(usage)
    }
}

impl Default for AlibabaTokenPlanProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for AlibabaTokenPlanProvider {
    fn id(&self) -> ProviderId {
        ProviderId::AlibabaTokenPlan
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto if ctx.auto_prefer_web => match self.fetch_via_web(ctx).await {
                Ok(usage) => Ok(ProviderFetchResult::new(usage, "web")),
                Err(_) => {
                    let usage = self.fetch_via_cli(ctx).await?;
                    Ok(ProviderFetchResult::new(usage, "cli"))
                }
            },
            SourceMode::Auto => match self.fetch_via_cli(ctx).await {
                Ok(usage) => Ok(ProviderFetchResult::new(usage, "cli")),
                Err(_) => {
                    let usage = self.fetch_via_web(ctx).await?;
                    Ok(ProviderFetchResult::new(usage, "web"))
                }
            },
            SourceMode::Cli => {
                let usage = self.fetch_via_cli(ctx).await?;
                Ok(ProviderFetchResult::new(usage, "cli"))
            }
            SourceMode::Web => {
                let usage = self.fetch_via_web(ctx).await?;
                Ok(ProviderFetchResult::new(usage, "web"))
            }
            SourceMode::OAuth => Err(ProviderError::UnsupportedSource(ctx.source_mode)),
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::Cli, SourceMode::Web]
    }

    fn error_state_kind(&self, error: &ProviderError) -> crate::core::ProviderStateKind {
        match error {
            ProviderError::NotInstalled(message) if message.contains("Bailian CLI 'bl'") => {
                crate::core::ProviderStateKind::LocalRuntimeOffline
            }
            _ => error.state_kind(),
        }
    }

    fn supports_web(&self) -> bool {
        true
    }

    fn supports_cli(&self) -> bool {
        true
    }
}

pub(super) fn throw_if_error_payload(value: &Value) -> Result<(), ProviderError> {
    if let Some(status) = find_first_i64(value, &["statusCode", "status_code", "code"])
        && status != 0
        && status != 200
    {
        if status == 401 || status == 403 {
            return Err(ProviderError::AuthRequired);
        }
        let message = find_first_string(value, &["statusMessage", "status_msg", "message", "msg"])
            .unwrap_or_else(|| format!("status code {status}"));
        return Err(ProviderError::Other(format!(
            "Alibaba Token Plan API error: {message}"
        )));
    }

    if let Some(frame) = find_failing_success_frame(value) {
        let code = find_first_string(frame, &["errorCode", "Code", "code"]);
        let message = find_first_string(
            frame,
            &["errorMsg", "message", "msg", "Message", "errorMessage"],
        )
        .or_else(|| code.clone())
        .unwrap_or_else(|| "request failed".to_string());
        let lower = message.to_lowercase();
        if lower.contains("needlogin")
            || lower.contains("login")
            || lower.contains("log in")
            || lower.contains("unauthorized")
        {
            return Err(ProviderError::AuthRequired);
        }
        return Err(ProviderError::Other(format!(
            "Alibaba Token Plan API error: {message}"
        )));
    }

    let code = find_first_string(value, &["code", "status", "statusCode"])
        .unwrap_or_default()
        .to_lowercase();
    let message = find_first_string(value, &["message", "msg", "statusMessage"])
        .unwrap_or_default()
        .to_lowercase();
    if code.contains("needlogin")
        || code.contains("login")
        || message.contains("log in")
        || message.contains("login")
    {
        return Err(ProviderError::AuthRequired);
    }
    Ok(())
}

fn find_failing_success_frame(value: &Value) -> Option<&Value> {
    match value {
        Value::Object(map) => {
            let failed_here = ["success", "Success"]
                .iter()
                .filter_map(|key| map.get(*key))
                .filter_map(|value| parse_bool(Some(value)))
                .any(|success| !success);
            if failed_here {
                return Some(value);
            }
            map.values().find_map(find_failing_success_frame)
        }
        Value::Array(values) => values.iter().find_map(find_failing_success_frame),
        _ => None,
    }
}

fn normalize_cookie_header(raw: &str) -> Option<String> {
    let mut header = raw.trim();
    if header
        .get(.."cookie:".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("cookie:"))
    {
        header = header["cookie:".len()..].trim();
    }
    (!header.is_empty() && header.contains('=')).then(|| header.to_string())
}

pub(super) fn cookie_value(name: &str, cookie_header: &str) -> Option<String> {
    cookie_header.split(';').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key.trim() == name)
            .then(|| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn dashboard_referer(region: Region) -> String {
    format!("{}/", region.gateway_base_url().trim_end_matches('/'))
}

fn extract_sec_token(html: &str) -> Option<String> {
    for pattern in [
        r#""secToken"\s*:\s*"([^"]+)""#,
        r#""sec_token"\s*:\s*"([^"]+)""#,
        r#"secToken['"]?\s*[:=]\s*['"]([^'"]+)['"]"#,
        r#"sec_token['"]?\s*[:=]\s*['"]([^'"]+)['"]"#,
        r#"SEC_TOKEN['"]?\s*[:=]\s*['"]([^'"]+)['"]"#,
    ] {
        let Ok(regex) = Regex::new(pattern) else {
            continue;
        };
        if let Some(value) = regex
            .captures(html)
            .and_then(|captures| captures.get(1))
            .map(|m| m.as_str().trim().to_string())
            .filter(|value| !value.is_empty())
        {
            return Some(value);
        }
    }
    None
}

pub(super) fn is_likely_login_html(data: &[u8]) -> bool {
    let text = String::from_utf8_lossy(data).to_lowercase();
    text.contains("<html")
        && (text.contains("login") || text.contains("sign in") || text.contains("signin"))
}

pub(super) fn expand_json_strings(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(expand_json_strings).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| (key, expand_json_strings(value)))
                .collect(),
        ),
        Value::String(text) => serde_json::from_str::<Value>(&text)
            .ok()
            .filter(|nested| nested.is_object() || nested.is_array())
            .map(expand_json_strings)
            .unwrap_or(Value::String(text)),
        other => other,
    }
}

pub(super) fn percentage_points(ratio: Option<f64>) -> Option<f64> {
    let ratio = ratio.filter(|v| v.is_finite())?;
    Some((ratio.clamp(0.0, 1.0) * 100.0).clamp(0.0, 100.0))
}

pub(super) fn number_field(value: &Value, key: &str) -> Option<f64> {
    value.as_object().and_then(|map| parse_f64(map.get(key)))
}

pub(super) fn date_field(value: &Value, key: &str) -> Option<DateTime<Utc>> {
    value.as_object().and_then(|map| parse_date(map.get(key)))
}

pub(super) fn find_object_containing_any_of(value: &Value, keys: &[&str]) -> Option<Value> {
    match value {
        Value::Object(map) => {
            if keys.iter().any(|key| map.contains_key(*key)) {
                return Some(Value::Object(map.clone()));
            }
            map.values()
                .find_map(|nested| find_object_containing_any_of(nested, keys))
        }
        Value::Array(values) => values
            .iter()
            .find_map(|nested| find_object_containing_any_of(nested, keys)),
        _ => None,
    }
}

const PLAN_NAME_KEYS: &[&str] = &[
    "planName",
    "plan_name",
    "packageName",
    "package_name",
    "commodityName",
    "commodity_name",
    "instanceName",
    "instance_name",
    "displayName",
    "display_name",
    "name",
    "title",
    "planType",
    "plan_type",
    "ProductName",
    "productName",
];
const USED_QUOTA_KEYS: &[&str] = &[
    "usedQuota",
    "used_quota",
    "usedCredits",
    "usedCredit",
    "consumedCredits",
    "usage",
    "used",
    "usedAmount",
    "consumeAmount",
    "usedValue",
    "UsedValue",
    "consumedValue",
    "ConsumedValue",
];
const TOTAL_QUOTA_KEYS: &[&str] = &[
    "totalQuota",
    "total_quota",
    "totalCredits",
    "totalCredit",
    "quota",
    "creditLimit",
    "creditsTotal",
    "monthlyTotalQuota",
    "amount",
    "totalValue",
    "TotalValue",
    "totalCount",
    "TotalCount",
    "subscriptionTotalNumber",
    "SubscriptionTotalNumber",
];
const REMAINING_QUOTA_KEYS: &[&str] = &[
    "remainingQuota",
    "remainQuota",
    "remainingCredits",
    "remainingCredit",
    "availableCredits",
    "balance",
    "remaining",
    "availableAmount",
    "remainAmount",
    "totalSurplusValue",
    "TotalSurplusValue",
    "surplusValue",
    "SurplusValue",
];
const RESET_DATE_KEYS: &[&str] = &[
    "nextRefreshTime",
    "resetTime",
    "periodEndTime",
    "billingCycleEnd",
    "billCycleEndTime",
    "expireTime",
    "expirationTime",
    "endTime",
    "validEndTime",
    "instanceEndTime",
    "nearestExpireDate",
    "NearestExpireDate",
];

fn find_token_plan_instance(value: &Value) -> Option<Value> {
    find_first_object(
        value,
        &[
            "tokenPlanInstanceInfo",
            "token_plan_instance_info",
            "instanceInfo",
            "instance_info",
        ],
    )
    .or_else(|| {
        find_first_array(
            value,
            &[
                "tokenPlanInstanceInfos",
                "token_plan_instance_infos",
                "instanceInfos",
                "instances",
                "Data",
                "data",
                "successResponse",
            ],
        )
        .and_then(|values| {
            values
                .into_iter()
                .filter(Value::is_object)
                .max_by_key(active_signal_score)
        })
    })
}

fn find_plan_name(value: &Value) -> Option<String> {
    first_string(value, PLAN_NAME_KEYS).or_else(|| find_first_string(value, PLAN_NAME_KEYS))
}

fn find_quota_info(value: &Value) -> Option<Value> {
    find_first_object(
        value,
        &[
            "quotaInfo",
            "quota_info",
            "tokenPlanQuotaInfo",
            "token_plan_quota_info",
        ],
    )
    .or_else(|| {
        find_first_object_with_any_key(
            value,
            &[USED_QUOTA_KEYS, TOTAL_QUOTA_KEYS, REMAINING_QUOTA_KEYS].concat(),
        )
    })
}

fn find_reset_date(value: &Value) -> Option<DateTime<Utc>> {
    first_date(value, RESET_DATE_KEYS).or_else(|| find_first_date(value, RESET_DATE_KEYS))
}

fn find_first_object(value: &Value, keys: &[&str]) -> Option<Value> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(nested) = map.get(*key).filter(|v| v.is_object()) {
                    return Some(nested.clone());
                }
            }
            map.values()
                .find_map(|nested| find_first_object(nested, keys))
        }
        Value::Array(values) => values
            .iter()
            .find_map(|nested| find_first_object(nested, keys)),
        _ => None,
    }
}

fn find_first_object_with_any_key(value: &Value, keys: &[&str]) -> Option<Value> {
    match value {
        Value::Object(map) => {
            if keys.iter().any(|key| map.contains_key(*key)) {
                return Some(value.clone());
            }
            map.values()
                .find_map(|nested| find_first_object_with_any_key(nested, keys))
        }
        Value::Array(values) => values
            .iter()
            .find_map(|nested| find_first_object_with_any_key(nested, keys)),
        _ => None,
    }
}

fn find_first_array(value: &Value, keys: &[&str]) -> Option<Vec<Value>> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(values) = map.get(*key).and_then(Value::as_array) {
                    return Some(values.clone());
                }
            }
            map.values()
                .find_map(|nested| find_first_array(nested, keys))
        }
        Value::Array(values) => values
            .iter()
            .find_map(|nested| find_first_array(nested, keys)),
        _ => None,
    }
}

fn first_string(value: &Value, keys: &[&str]) -> Option<String> {
    let map = value.as_object()?;
    keys.iter().find_map(|key| parse_string(map.get(*key)))
}

pub(super) fn find_first_string(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => first_string(value, keys).or_else(|| {
            map.values()
                .find_map(|nested| find_first_string(nested, keys))
        }),
        Value::Array(values) => values
            .iter()
            .find_map(|nested| find_first_string(nested, keys)),
        _ => None,
    }
}

fn first_f64(value: &Value, keys: &[&str]) -> Option<f64> {
    let map = value.as_object()?;
    keys.iter().find_map(|key| parse_f64(map.get(*key)))
}

fn find_first_i64(value: &Value, keys: &[&str]) -> Option<i64> {
    match value {
        Value::Object(map) => keys
            .iter()
            .find_map(|key| parse_i64(map.get(*key)))
            .or_else(|| map.values().find_map(|nested| find_first_i64(nested, keys))),
        Value::Array(values) => values
            .iter()
            .find_map(|nested| find_first_i64(nested, keys)),
        _ => None,
    }
}

fn first_date(value: &Value, keys: &[&str]) -> Option<DateTime<Utc>> {
    let map = value.as_object()?;
    keys.iter().find_map(|key| parse_date(map.get(*key)))
}

fn find_first_date(value: &Value, keys: &[&str]) -> Option<DateTime<Utc>> {
    match value {
        Value::Object(map) => first_date(value, keys).or_else(|| {
            map.values()
                .find_map(|nested| find_first_date(nested, keys))
        }),
        Value::Array(values) => values
            .iter()
            .find_map(|nested| find_first_date(nested, keys)),
        _ => None,
    }
}

fn parse_string(value: Option<&Value>) -> Option<String> {
    value?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn parse_f64(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().replace(',', "").parse().ok(),
        _ => None,
    }
}

fn parse_i64(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(number) => number.as_i64().or_else(|| {
            // Quota/timestamp JSON floats are whole numbers; the fractional
            // part is rounding noise from the upstream API.
            let v = number.as_f64()?;
            #[expect(clippy::cast_possible_truncation, reason = "quota/timestamp JSON floats are whole numbers; fractional part is rounding noise")]
            let whole = v as i64;
            Some(whole)
        }),
        Value::String(text) => text.trim().replace(',', "").parse().ok(),
        _ => None,
    }
}

fn parse_bool(value: Option<&Value>) -> Option<bool> {
    match value? {
        Value::Bool(flag) => Some(*flag),
        Value::Number(number) => number.as_i64().map(|v| v != 0),
        Value::String(text) => match text.trim().to_lowercase().as_str() {
            "true" | "1" | "yes" | "active" | "valid" | "normal" => Some(true),
            "false" | "0" | "no" | "inactive" | "invalid" | "expired" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn parse_date(value: Option<&Value>) -> Option<DateTime<Utc>> {
    if let Some(raw) = parse_i64(value) {
        if raw > 1_000_000_000_000 {
            return Utc.timestamp_opt(raw / 1000, 0).single();
        }
        if raw > 1_000_000_000 {
            return Utc.timestamp_opt(raw, 0).single();
        }
    }
    let text = parse_string(value)?;
    if let Ok(date) = DateTime::parse_from_rfc3339(&text) {
        return Some(date.with_timezone(&Utc));
    }
    if let Ok(date) = NaiveDate::parse_from_str(&text, "%Y-%m-%d")
        && let Some(date_time) = date.and_hms_opt(0, 0, 0)
    {
        return Some(date_time.and_utc());
    }
    for format in ["%Y-%m-%d %H:%M", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(date) = NaiveDateTime::parse_from_str(&text, format) {
            return Some(date.and_utc());
        }
    }
    None
}

fn active_signal_score(value: &Value) -> i32 {
    let status = first_string(value, &["status", "instanceStatus", "state"])
        .unwrap_or_default()
        .to_uppercase();
    if ["VALID", "ACTIVE", "NORMAL"].contains(&status.as_str()) {
        return 3;
    }
    if [
        "EXPIRED",
        "INVALID",
        "INACTIVE",
        "DISABLED",
        "TERMINATED",
        "STOPPED",
    ]
    .contains(&status.as_str())
    {
        return -1;
    }
    parse_bool(
        value
            .as_object()
            .and_then(|map| map.get("isActive").or_else(|| map.get("active"))),
    )
    .map(|active| if active { 3 } else { -1 })
    .unwrap_or(0)
}

fn used_percent(used: Option<f64>, total: Option<f64>, remaining: Option<f64>) -> Option<f64> {
    let total = total.filter(|total| *total > 0.0)?;
    let used = used.or_else(|| remaining.map(|remaining| total - remaining))?;
    Some((used.clamp(0.0, total) / total * 100.0).clamp(0.0, 100.0))
}

fn quota_detail(used: Option<f64>, total: Option<f64>, remaining: Option<f64>) -> Option<String> {
    if let (Some(used), Some(total)) = (used, total.filter(|total| *total > 0.0)) {
        return Some(format!(
            "{} / {} credits used",
            format_quota(used),
            format_quota(total)
        ));
    }
    if let (Some(remaining), Some(total)) = (remaining, total.filter(|total| *total > 0.0)) {
        return Some(format!(
            "{} / {} credits left",
            format_quota(remaining),
            format_quota(total)
        ));
    }
    remaining.map(|remaining| format!("{} credits left", format_quota(remaining)))
}

fn quota_detail_percent(used_percent: f64, total: Option<f64>) -> Option<String> {
    let total = total.filter(|total| *total > 0.0)?;
    let used = total * used_percent / 100.0;
    Some(format!(
        "{} / {} credits used",
        format_quota(used),
        format_quota(total)
    ))
}

fn format_quota(value: f64) -> String {
    if (value.round() - value).abs() < f64::EPSILON {
        // Guarded above: value is within EPSILON of a whole number, so the
        // fractional part is zero.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "whole-number guard above; fractional part is zero"
        )]
        let whole = value.round() as i64;
        format_count(whole)
    } else {
        let formatted = format!("{value:.2}");
        let trimmed = formatted.trim_end_matches('0').trim_end_matches('.');
        format_count_decimal(trimmed)
    }
}

fn format_count(value: i64) -> String {
    let raw = value.to_string();
    let mut output = String::with_capacity(raw.len() + raw.len() / 3);
    for (idx, ch) in raw.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 {
            output.push(',');
        }
        output.push(ch);
    }
    output.chars().rev().collect()
}

fn format_count_decimal(raw: &str) -> String {
    let (whole, fraction) = raw.split_once('.').unwrap_or((raw, ""));
    if fraction.is_empty() {
        format_count(whole.parse().unwrap_or(0))
    } else {
        format!("{}.{}", format_count(whole.parse().unwrap_or(0)), fraction)
    }
}

fn payload_diagnostics(value: &Value) -> String {
    let top_keys = value
        .as_object()
        .map(|map| {
            let mut keys = map.keys().cloned().collect::<Vec<_>>();
            keys.sort();
            keys.join(",")
        })
        .unwrap_or_default();
    format!("topKeys={top_keys}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_bailian_cli_is_local_runtime_offline() {
        let provider = AlibabaTokenPlanProvider::new();
        let error = ProviderError::NotInstalled(
            "Bailian CLI 'bl' is not installed or not on PATH.".to_string(),
        );
        assert_eq!(
            provider.error_state_kind(&error),
            crate::core::ProviderStateKind::LocalRuntimeOffline
        );
    }
    #[test]
    fn parses_token_plan_instance_payload() {
        let payload = serde_json::json!({
            "data": {
                "tokenPlanInstanceInfo": {
                    "commodityName": "Token Plan Pro",
                    "quotaInfo": {
                        "usedQuota": "1250",
                        "totalQuota": "5000"
                    },
                    "nextRefreshTime": 1780763009000_i64
                }
            }
        });
        let snapshot =
            AlibabaTokenPlanProvider::parse_usage_snapshot(payload.to_string().as_bytes()).unwrap();
        assert_eq!(snapshot.plan_name.as_deref(), Some("Token Plan Pro"));
        assert_eq!(snapshot.used_quota, Some(1250.0));
        assert_eq!(snapshot.total_quota, Some(5000.0));

        let usage = AlibabaTokenPlanProvider::snapshot_to_usage(snapshot).unwrap();
        assert_eq!(usage.primary.used_percent, 25.0);
        assert_eq!(
            usage.primary.reset_description.as_deref(),
            Some("1,250 / 5,000 credits used")
        );
        assert_eq!(usage.login_method.as_deref(), Some("Token Plan Pro"));
    }

    #[test]
    fn expands_nested_string_payloads_and_uses_remaining_quota() {
        let nested = serde_json::json!({
            "successResponse": serde_json::json!({
                "instances": [
                    {"status": "EXPIRED", "quota": 1000, "remaining": 1000},
                    {"status": "ACTIVE", "packageName": "Team", "quota": 1000, "remaining": 250}
                ]
            }).to_string()
        });
        let snapshot =
            AlibabaTokenPlanProvider::parse_usage_snapshot(nested.to_string().as_bytes()).unwrap();
        assert_eq!(snapshot.plan_name.as_deref(), Some("Team"));
        assert_eq!(
            used_percent(
                snapshot.used_quota,
                snapshot.total_quota,
                snapshot.remaining_quota
            ),
            Some(75.0)
        );
    }

    #[test]
    fn parses_new_subscription_summary_payload() {
        let payload = serde_json::json!({
            "success": true,
            "Data": {
                "ProductName": "Token Plan Team",
                "TotalValue": "1000000",
                "TotalSurplusValue": "250000",
                "NearestExpireDate": "2026-06-30"
            }
        });
        let snapshot =
            AlibabaTokenPlanProvider::parse_usage_snapshot(payload.to_string().as_bytes()).unwrap();
        assert_eq!(snapshot.plan_name.as_deref(), Some("Token Plan Team"));
        assert_eq!(snapshot.total_quota, Some(1_000_000.0));
        assert_eq!(snapshot.remaining_quota, Some(250_000.0));
        assert_eq!(
            used_percent(
                snapshot.used_quota,
                snapshot.total_quota,
                snapshot.remaining_quota
            ),
            Some(75.0)
        );
        assert!(snapshot.resets_at.is_some());
    }

    #[test]
    fn detects_login_payloads() {
        let err = AlibabaTokenPlanProvider::parse_usage_snapshot(
            br#"{"code":"NeedLogin","message":"please login"}"#,
        )
        .unwrap_err();
        assert!(matches!(err, ProviderError::AuthRequired));
    }

    #[test]
    fn extracts_sec_token_from_html_or_cookie() {
        assert_eq!(
            extract_sec_token(r#"<script>{"secToken":"abc123"}</script>"#).as_deref(),
            Some("abc123")
        );
        assert_eq!(
            extract_sec_token(
                r#"<script>window.ALIYUN_CONSOLE_CONFIG = { SEC_TOKEN: "upper123" };</script>"#
            )
            .as_deref(),
            Some("upper123")
        );
        assert_eq!(
            cookie_value("sec_token", "foo=bar; sec_token=xyz"),
            Some("xyz".to_string())
        );
    }

    #[test]
    fn sec_token_shell_referer_uses_same_origin_root() {
        assert_eq!(
            dashboard_referer(Region::CnPersonal),
            "https://bailian.console.aliyun.com/"
        );
        assert_eq!(
            dashboard_referer(Region::IntlPersonal),
            "https://modelstudio.console.alibabacloud.com/"
        );
    }

    #[test]
    fn default_region_cn_team_urls_match_legacy() {
        let region = Region::Cn;
        assert_eq!(
            AlibabaTokenPlanProvider::team_quota_url(region),
            "https://bailian.console.aliyun.com/data/api.json?action=GetSubscriptionSummary&product=BssOpenAPI-V3&_tag="
        );
        assert_eq!(
            AlibabaTokenPlanProvider::team_request_params(region),
            serde_json::json!({"ProductCode": "sfm_tokenplanteams_dp_cn"}).to_string()
        );
        assert_eq!(region.current_region_id(), "cn-beijing");
    }
}
