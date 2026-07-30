//! Qwen Cloud personal token-plan provider (upstream 0.46.0).
//!
//! Cookie-authenticated OneConsole gateway:
//! `POST https://cs-data.qwencloud.com/data/api.json?...`
//! with `IntlBroadScopeAspnGateway` / `sfm_bailian`.

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use regex_lite::Regex;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};
use crate::providers::browser_cookie_header;

const GATEWAY_BASE_URL: &str = "https://home.qwencloud.com";
const DATA_GATEWAY_BASE_URL: &str = "https://cs-data.qwencloud.com";
const DASHBOARD_URL: &str = "https://home.qwencloud.com/billing/subscription/token-plan-individual";
const STATUS_PAGE_URL: &str = "https://status.alibabacloud.com";
const PRODUCT_CODE: &str = "sfm_tokenplansolo_public_intl";
const CONSOLE_PRODUCT: &str = "sfm_bailian";
const CONSOLE_ACTION: &str = "IntlBroadScopeAspnGateway";
const USAGE_API: &str = "zeldaHttp.apikeyMgr./tokenplan/personal/api/v2/usage";
const SUBSCRIPTION_API: &str = "zeldaHttp.apikeyMgr./tokenplan/personal/api/v2/subscription";
const QUOTA_CONFIG_API: &str = "zeldaHttp.apikeyMgr./tokenplan/personal/api/v2/quota-config";
const REGION: &str = "ap-southeast-1";
const LANGUAGE: &str = "en-US";
const USER_INFO_PATH: &str = "/tool/user/info.json";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";

const COOKIE_DOMAINS: &[&str] = &[
    "qwencloud.com",
    "home.qwencloud.com",
    "account.qwencloud.com",
    "signin.qwencloud.com",
    "www.qwencloud.com",
    "cs-data.qwencloud.com",
    "alibabacloud.com",
    "account.alibabacloud.com",
    "aliyun.com",
    "console.aliyun.com",
];

const FIVE_HOUR_MINUTES: u32 = 5 * 60;
const WEEKLY_MINUTES: u32 = 7 * 24 * 60;
const LEGACY_MINUTES: u32 = 30 * 24 * 60;

pub struct QwenCloudProvider {
    metadata: ProviderMetadata,
}

#[derive(Debug, Clone, PartialEq)]
struct QwenCloudSnapshot {
    plan_name: Option<String>,
    used_quota: Option<f64>,
    total_quota: Option<f64>,
    remaining_quota: Option<f64>,
    resets_at: Option<DateTime<Utc>>,
    five_hour_used_percent: Option<f64>,
    five_hour_total_quota: Option<f64>,
    five_hour_resets_at: Option<DateTime<Utc>>,
    weekly_used_percent: Option<f64>,
    weekly_total_quota: Option<f64>,
    weekly_resets_at: Option<DateTime<Utc>>,
}

impl QwenCloudProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::QwenCloud,
                display_name: "Qwen Cloud",
                session_label: "5-hour",
                weekly_label: "Weekly",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some(DASHBOARD_URL),
                status_page_url: Some(STATUS_PAGE_URL),
            },
        }
    }

    async fn fetch_via_web(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProviderError> {
        let cookie_header = Self::resolve_cookie_header(ctx)?;
        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(ctx.web_timeout.max(1)))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        let sec_token = Self::resolve_sec_token(&client, &cookie_header, ctx)
            .await
            .ok_or(ProviderError::AuthRequired)?;

        let usage_body = Self::post_api(
            &client,
            USAGE_API,
            Map::new(),
            &sec_token,
            &cookie_header,
            ctx,
        )
        .await?;

        let subscription_body = Self::post_api_optional(
            &client,
            SUBSCRIPTION_API,
            {
                let mut data = Map::new();
                data.insert(
                    "commodityCode".into(),
                    Value::String(PRODUCT_CODE.to_string()),
                );
                data
            },
            &sec_token,
            &cookie_header,
            ctx,
        )
        .await;

        let quota_config_body = Self::post_api_optional(
            &client,
            QUOTA_CONFIG_API,
            Map::new(),
            &sec_token,
            &cookie_header,
            ctx,
        )
        .await;

        let snapshot = Self::parse(
            &usage_body,
            subscription_body.as_deref(),
            quota_config_body.as_deref(),
        )?;
        Self::snapshot_to_usage(snapshot)
    }

    fn resolve_cookie_header(ctx: &FetchContext) -> Result<String, ProviderError> {
        if let Some(raw) = ctx
            .manual_cookie_header
            .as_deref()
            .and_then(normalize_cookie_header)
        {
            return Ok(raw);
        }
        for env_name in ["QWEN_CLOUD_COOKIE", "QWEN_CLOUD_COOKIE_HEADER"] {
            if let Ok(raw) = std::env::var(env_name)
                && let Some(header) = normalize_cookie_header(&raw)
            {
                return Ok(header);
            }
        }
        browser_cookie_header(COOKIE_DOMAINS)
            .and_then(|header| normalize_cookie_header(&header).ok_or(ProviderError::NoCookies))
    }

    async fn resolve_sec_token(
        client: &reqwest::Client,
        cookie_header: &str,
        ctx: &FetchContext,
    ) -> Option<String> {
        let timeout = std::time::Duration::from_secs(ctx.web_timeout.clamp(1, 20));

        // 1. Dashboard HTML (freshest OneConsole inject).
        if let Ok(response) = client
            .get(DASHBOARD_URL)
            .timeout(timeout)
            .header("Cookie", cookie_header)
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .header("User-Agent", USER_AGENT)
            .send()
            .await
            && response.status().is_success()
            && let Ok(text) = response.text().await
        {
            if looks_like_login_page(&text) {
                return None;
            }
            if let Some(token) = extract_sec_token(&text) {
                return Some(token);
            }
        }

        // 2. Cookie fallback.
        if let Some(token) = cookie_value("sec_token", cookie_header) {
            return Some(token);
        }

        // 3. User-info JSON endpoint.
        let user_info_url = format!("{GATEWAY_BASE_URL}{USER_INFO_PATH}");
        if let Ok(response) = client
            .get(&user_info_url)
            .timeout(timeout)
            .header("Cookie", cookie_header)
            .header("Accept", "application/json, text/plain, */*")
            .header("User-Agent", USER_AGENT)
            .send()
            .await
            && response.status().is_success()
            && let Ok(bytes) = response.bytes().await
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        {
            let expanded = expand_json_strings(value);
            if let Some(token) =
                find_first_string(&expanded, &["secToken", "sec_token", "csrfToken", "token"])
            {
                return Some(token);
            }
        }

        None
    }

    async fn post_api(
        client: &reqwest::Client,
        api: &str,
        data_parameters: Map<String, Value>,
        sec_token: &str,
        cookie_header: &str,
        ctx: &FetchContext,
    ) -> Result<Vec<u8>, ProviderError> {
        let url = api_url(api);
        let params_json = build_params_json(api, data_parameters, cookie_header);
        let form = [
            ("product", CONSOLE_PRODUCT.to_string()),
            ("action", CONSOLE_ACTION.to_string()),
            ("sec_token", sec_token.to_string()),
            ("region", REGION.to_string()),
            ("language", LANGUAGE.to_string()),
            ("params", params_json),
        ];

        let mut request = client
            .post(&url)
            .timeout(std::time::Duration::from_secs(ctx.web_timeout.max(1)))
            .header("Cookie", cookie_header)
            .header("Accept", "application/json, text/plain, */*")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Origin", GATEWAY_BASE_URL)
            .header("Referer", DASHBOARD_URL)
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
                "Qwen Cloud API error: HTTP {status}"
            )));
        }
        Ok(body.to_vec())
    }

    async fn post_api_optional(
        client: &reqwest::Client,
        api: &str,
        data_parameters: Map<String, Value>,
        sec_token: &str,
        cookie_header: &str,
        ctx: &FetchContext,
    ) -> Option<Vec<u8>> {
        Self::post_api(client, api, data_parameters, sec_token, cookie_header, ctx)
            .await
            .ok()
    }

    fn parse(
        usage_data: &[u8],
        subscription_data: Option<&[u8]>,
        quota_config_data: Option<&[u8]>,
    ) -> Result<QwenCloudSnapshot, ProviderError> {
        if usage_data.is_empty() {
            return Err(ProviderError::Parse("Empty Qwen Cloud response".into()));
        }

        let value: Value = serde_json::from_slice(usage_data).map_err(|_| {
            if is_likely_login_html(usage_data) {
                ProviderError::AuthRequired
            } else {
                ProviderError::Parse("Invalid Qwen Cloud JSON response".into())
            }
        })?;
        let expanded = expand_json_strings(value);
        throw_if_error_payload(&expanded)?;

        if let Some(snapshot) =
            parse_current_token_plan(&expanded, subscription_data, quota_config_data)
        {
            return Ok(snapshot);
        }

        parse_legacy_token_plan(&expanded)
    }

    fn snapshot_to_usage(snapshot: QwenCloudSnapshot) -> Result<UsageSnapshot, ProviderError> {
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
                Some(LEGACY_MINUTES),
                snapshot.resets_at,
                quota_detail(
                    snapshot.used_quota,
                    snapshot.total_quota,
                    snapshot.remaining_quota,
                ),
            )
        });
        let primary = five_hour
            .or(legacy)
            .ok_or_else(|| ProviderError::Parse("Qwen Cloud usage windows missing".into()))?;
        let mut usage = UsageSnapshot::new(primary);

        if let Some(weekly_percent) = snapshot.weekly_used_percent {
            usage = usage.with_secondary(RateWindow::with_details(
                weekly_percent,
                Some(WEEKLY_MINUTES),
                snapshot.weekly_resets_at,
                quota_detail_percent(weekly_percent, snapshot.weekly_total_quota),
            ));
        }

        if let Some(plan) = snapshot.plan_name.filter(|plan| !plan.trim().is_empty()) {
            usage = usage.with_login_method(plan);
        }
        Ok(usage)
    }
}

impl Default for QwenCloudProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for QwenCloudProvider {
    fn id(&self) -> ProviderId {
        ProviderId::QwenCloud
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
            SourceMode::Cli | SourceMode::OAuth => {
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

fn api_url(api: &str) -> String {
    format!(
        "{DATA_GATEWAY_BASE_URL}/data/api.json?action={CONSOLE_ACTION}&product={CONSOLE_PRODUCT}&api={api}&_v=undefined"
    )
}

fn build_params_json(
    api: &str,
    mut data_parameters: Map<String, Value>,
    cookie_header: &str,
) -> String {
    let mut cornerstone = Map::new();
    cornerstone.insert(
        "feTraceId".into(),
        Value::String(Uuid::new_v4().to_string().to_lowercase()),
    );
    cornerstone.insert("feURL".into(), Value::String(DASHBOARD_URL.to_string()));
    cornerstone.insert("protocol".into(), Value::String("V2".into()));
    cornerstone.insert("console".into(), Value::String("ONE_CONSOLE".into()));
    cornerstone.insert("productCode".into(), Value::String("p_efm".into()));
    cornerstone.insert("domain".into(), Value::String("home.qwencloud.com".into()));
    cornerstone.insert("consoleSite".into(), Value::String("QWENCLOUD".into()));
    cornerstone.insert("userNickName".into(), Value::String(String::new()));
    cornerstone.insert("userPrincipalName".into(), Value::String(String::new()));
    cornerstone.insert("xsp_lang".into(), Value::String(LANGUAGE.into()));
    if let Some(cna) = cookie_value("cna", cookie_header) {
        cornerstone.insert("X-Anonymous-Id".into(), Value::String(cna));
    }

    data_parameters.insert("cornerstoneParam".into(), Value::Object(cornerstone));

    json!({
        "Api": api,
        "V": "1.0",
        "Data": Value::Object(data_parameters),
    })
    .to_string()
}

fn parse_current_token_plan(
    expanded: &Value,
    subscription_data: Option<&[u8]>,
    quota_config_data: Option<&[u8]>,
) -> Option<QwenCloudSnapshot> {
    let usage =
        find_object_containing_any_of(expanded, &["per5HourPercentage", "per1WeekPercentage"])?;
    let five_hour = percentage_points(number_field(&usage, "per5HourPercentage"));
    let weekly = percentage_points(number_field(&usage, "per1WeekPercentage"));
    if five_hour.is_none() && weekly.is_none() {
        return None;
    }

    let plan_code = subscription_data.and_then(plan_code_from_bytes);
    let plan_name = plan_code.as_deref().map(display_plan_name);
    let quota = quota_config_data
        .zip(plan_code.as_ref())
        .and_then(|(data, code)| quota_totals_from_bytes(data, code));

    Some(QwenCloudSnapshot {
        plan_name,
        used_quota: None,
        total_quota: None,
        remaining_quota: None,
        resets_at: None,
        five_hour_used_percent: five_hour,
        five_hour_total_quota: quota.map(|q| q.0).unwrap_or(None),
        five_hour_resets_at: date_field(&usage, "per5HourResetTime"),
        weekly_used_percent: weekly,
        weekly_total_quota: quota.map(|q| q.1).unwrap_or(None),
        weekly_resets_at: date_field(&usage, "per1WeekResetTime"),
    })
}

fn parse_legacy_token_plan(expanded: &Value) -> Result<QwenCloudSnapshot, ProviderError> {
    let instance = find_token_plan_instance(expanded);
    let plan_name = instance
        .as_ref()
        .and_then(|v| find_first_string(v, PLAN_NAME_KEYS))
        .or_else(|| find_first_string(expanded, PLAN_NAME_KEYS));
    let quota_source = instance
        .as_ref()
        .and_then(find_quota_info)
        .or_else(|| find_quota_info(expanded))
        .unwrap_or_else(|| expanded.clone());
    let used = first_f64(&quota_source, USED_QUOTA_KEYS)
        .or_else(|| find_first_f64(&quota_source, USED_QUOTA_KEYS));
    let total = first_f64(&quota_source, TOTAL_QUOTA_KEYS)
        .or_else(|| find_first_f64(&quota_source, TOTAL_QUOTA_KEYS));
    let remaining = first_f64(&quota_source, REMAINING_QUOTA_KEYS)
        .or_else(|| find_first_f64(&quota_source, REMAINING_QUOTA_KEYS));
    let resets_at = instance
        .as_ref()
        .and_then(|v| {
            first_date(v, RESET_DATE_KEYS).or_else(|| find_first_date(v, RESET_DATE_KEYS))
        })
        .or_else(|| {
            first_date(expanded, RESET_DATE_KEYS)
                .or_else(|| find_first_date(expanded, RESET_DATE_KEYS))
        });

    if used_percent(used, total, remaining).is_none() {
        return Err(ProviderError::Parse(
            "Qwen Cloud has no active token-plan subscription".into(),
        ));
    }

    Ok(QwenCloudSnapshot {
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

fn plan_code_from_bytes(data: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(data).ok()?;
    let expanded = expand_json_strings(value);
    let plan = find_object_containing_any_of(
        &expanded,
        &["specCode", "spec_code", "planName", "plan_name"],
    )?;
    for key in ["specCode", "spec_code", "planName", "plan_name"] {
        if let Some(raw) = plan.get(key).and_then(Value::as_str) {
            let normalized = raw.trim().to_lowercase();
            if !normalized.is_empty() {
                return Some(normalized);
            }
        }
    }
    None
}

fn display_plan_name(plan_code: &str) -> String {
    match plan_code {
        "lite" => "Lite".into(),
        "standard" => "Standard".into(),
        "pro" => "Pro".into(),
        "max" => "Max".into(),
        other => other.to_string(),
    }
}

fn quota_totals_from_bytes(data: &[u8], plan_code: &str) -> Option<(Option<f64>, Option<f64>)> {
    let value: Value = serde_json::from_slice(data).ok()?;
    let expanded = expand_json_strings(value);
    let quota = find_first_value_for_key(&expanded, plan_code)?
        .as_object()?
        .clone();
    let five_hour = number_field(&Value::Object(quota.clone()), "five_hour")
        .or_else(|| number_field(&Value::Object(quota.clone()), "fiveHour"));
    let weekly = number_field(&Value::Object(quota), "weekly");
    if five_hour.is_none() && weekly.is_none() {
        return None;
    }
    Some((five_hour, weekly))
}

fn throw_if_error_payload(value: &Value) -> Result<(), ProviderError> {
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
            "Qwen Cloud API error: {message}"
        )));
    }

    if let Some(success) = find_first_bool(value, &["success", "Success", "successResponse"])
        && !success
    {
        let message = find_first_string(value, &["message", "msg", "Message", "errorMessage"])
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
            "Qwen Cloud API error: {message}"
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
    if code.contains("forbidden") || code == "403" || message.contains("forbidden") {
        return Err(ProviderError::AuthRequired);
    }
    Ok(())
}

fn normalize_cookie_header(raw: &str) -> Option<String> {
    let mut header = raw.trim();
    if header
        .get(.."cookie:".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("cookie:"))
    {
        header = header["cookie:".len()..].trim();
    }
    if (header.starts_with('"') && header.ends_with('"'))
        || (header.starts_with('\'') && header.ends_with('\''))
    {
        header = header[1..header.len().saturating_sub(1)].trim();
    }
    (!header.is_empty() && header.contains('=')).then(|| header.to_string())
}

fn cookie_value(name: &str, cookie_header: &str) -> Option<String> {
    cookie_header.split(';').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key.trim().eq_ignore_ascii_case(name))
            .then(|| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn extract_sec_token(html: &str) -> Option<String> {
    for pattern in [
        r#""secToken"\s*:\s*"([^"]+)""#,
        r#""sec_token"\s*:\s*"([^"]+)""#,
        r#"secToken['"]?\s*[:=]\s*['"]([^'"]+)['"]"#,
        r#"sec_token['"]?\s*[:=]\s*['"]([^'"]+)['"]"#,
        r#"csrfToken['"]?\s*[:=]\s*['"]([^'"]+)['"]"#,
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

fn looks_like_login_page(html: &str) -> bool {
    let lowered = html.to_lowercase();
    lowered.contains("passport.alibabacloud.com")
        || lowered.contains("signin.aliyun.com")
        || lowered.contains("account.alibabacloud.com/login")
        || lowered.contains("login.qwencloud.com")
        || (lowered.contains("login")
            && lowered.contains("password")
            && lowered.contains("sign in"))
}

fn is_likely_login_html(data: &[u8]) -> bool {
    let text = String::from_utf8_lossy(data).to_lowercase();
    text.contains("<html")
        && (text.contains("login") || text.contains("sign in") || text.contains("signin"))
}

fn expand_json_strings(value: Value) -> Value {
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

fn percentage_points(ratio: Option<f64>) -> Option<f64> {
    let ratio = ratio.filter(|v| v.is_finite())?;
    Some((ratio.clamp(0.0, 1.0) * 100.0).clamp(0.0, 100.0))
}

fn number_field(value: &Value, key: &str) -> Option<f64> {
    value.as_object().and_then(|map| parse_f64(map.get(key)))
}

fn date_field(value: &Value, key: &str) -> Option<DateTime<Utc>> {
    value.as_object().and_then(|map| parse_date(map.get(key)))
}

fn find_object_containing_any_of(value: &Value, keys: &[&str]) -> Option<Value> {
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

fn find_first_value_for_key(value: &Value, key: &str) -> Option<Value> {
    match value {
        Value::Object(map) => {
            if let Some(nested) = map.get(key) {
                return Some(nested.clone());
            }
            map.values()
                .find_map(|nested| find_first_value_for_key(nested, key))
        }
        Value::Array(values) => values
            .iter()
            .find_map(|nested| find_first_value_for_key(nested, key)),
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
    "InstanceCode",
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
    "CycleTotalValue",
    "cycleTotalValue",
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
    "CycleSurplusValue",
    "cycleSurplusValue",
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
    "EndTime",
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
                "EquityList",
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
        find_first_array(value, &["EquityList", "equityList"]).and_then(|values| {
            values.into_iter().find(|item| {
                item.as_object().is_some_and(|map| {
                    map.contains_key("CycleTotalValue")
                        || map.contains_key("cycleTotalValue")
                        || map.contains_key("CycleSurplusValue")
                })
            })
        })
    })
    .or_else(|| {
        let keys: Vec<&str> = USED_QUOTA_KEYS
            .iter()
            .chain(TOTAL_QUOTA_KEYS.iter())
            .chain(REMAINING_QUOTA_KEYS.iter())
            .copied()
            .collect();
        find_first_object_with_any_key(value, &keys)
    })
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
                return Some(Value::Object(map.clone()));
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
                if let Some(Value::Array(values)) = map.get(*key) {
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
    value
        .as_object()
        .and_then(|map| keys.iter().find_map(|key| parse_string(map.get(*key))))
}

fn find_first_string(value: &Value, keys: &[&str]) -> Option<String> {
    first_string(value, keys).or_else(|| match value {
        Value::Object(map) => map
            .values()
            .find_map(|nested| find_first_string(nested, keys)),
        Value::Array(values) => values
            .iter()
            .find_map(|nested| find_first_string(nested, keys)),
        _ => None,
    })
}

fn first_f64(value: &Value, keys: &[&str]) -> Option<f64> {
    value
        .as_object()
        .and_then(|map| keys.iter().find_map(|key| parse_f64(map.get(*key))))
}

fn find_first_f64(value: &Value, keys: &[&str]) -> Option<f64> {
    first_f64(value, keys).or_else(|| match value {
        Value::Object(map) => map.values().find_map(|nested| find_first_f64(nested, keys)),
        Value::Array(values) => values
            .iter()
            .find_map(|nested| find_first_f64(nested, keys)),
        _ => None,
    })
}

fn find_first_i64(value: &Value, keys: &[&str]) -> Option<i64> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(parsed) = parse_i64(map.get(*key)) {
                    return Some(parsed);
                }
            }
            map.values().find_map(|nested| find_first_i64(nested, keys))
        }
        Value::Array(values) => values
            .iter()
            .find_map(|nested| find_first_i64(nested, keys)),
        _ => None,
    }
}

fn find_first_bool(value: &Value, keys: &[&str]) -> Option<bool> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(parsed) = parse_bool(map.get(*key)) {
                    return Some(parsed);
                }
            }
            map.values()
                .find_map(|nested| find_first_bool(nested, keys))
        }
        Value::Array(values) => values
            .iter()
            .find_map(|nested| find_first_bool(nested, keys)),
        _ => None,
    }
}

fn first_date(value: &Value, keys: &[&str]) -> Option<DateTime<Utc>> {
    value
        .as_object()
        .and_then(|map| keys.iter().find_map(|key| parse_date(map.get(*key))))
}

fn find_first_date(value: &Value, keys: &[&str]) -> Option<DateTime<Utc>> {
    first_date(value, keys).or_else(|| match value {
        Value::Object(map) => map
            .values()
            .find_map(|nested| find_first_date(nested, keys)),
        Value::Array(values) => values
            .iter()
            .find_map(|nested| find_first_date(nested, keys)),
        _ => None,
    })
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
    .filter(|v| v.is_finite())
}

fn parse_i64(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|v| v as i64)),
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
    let status = first_string(value, &["status", "instanceStatus", "state", "Status"])
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
        format_count(value.round() as i64)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_current_token_plan_5h_and_weekly() {
        let inner = r#"{
          "code": 0,
          "data": {
            "per5HourPercentage": 0.03,
            "per5HourResetTime": 1700003600000,
            "per1WeekPercentage": 0.01,
            "per1WeekResetTime": 1700086400000
          },
          "success": true
        }"#;
        let payload = serde_json::json!({
            "data": {
                "DataV2": {
                    "data": inner,
                },
            },
            "httpStatusCode": 200,
        });

        let snapshot =
            QwenCloudProvider::parse(payload.to_string().as_bytes(), None, None).unwrap();
        assert_eq!(snapshot.five_hour_used_percent, Some(3.0));
        assert_eq!(
            snapshot.five_hour_resets_at,
            Some(Utc.timestamp_opt(1_700_003_600, 0).single().unwrap())
        );
        assert_eq!(snapshot.weekly_used_percent, Some(1.0));
        assert_eq!(
            snapshot.weekly_resets_at,
            Some(Utc.timestamp_opt(1_700_086_400, 0).single().unwrap())
        );

        let usage = QwenCloudProvider::snapshot_to_usage(snapshot).unwrap();
        assert_eq!(usage.primary.used_percent, 3.0);
        assert_eq!(usage.primary.window_minutes, Some(FIVE_HOUR_MINUTES));
        assert_eq!(usage.secondary.as_ref().map(|w| w.used_percent), Some(1.0));
        assert_eq!(
            usage.secondary.as_ref().and_then(|w| w.window_minutes),
            Some(WEEKLY_MINUTES)
        );
    }

    #[test]
    fn parses_personal_usage_fixture_shape() {
        let payload = serde_json::json!({
            "code": "200",
            "data": {
                "DataV2": {
                    "data": {
                        "success": true,
                        "data": {
                            "per5HourPercentage": 0.0009973083333333333,
                            "per5HourResetTime": 1784813220000_i64,
                            "per1WeekPercentage": 0.0003014725,
                            "per1WeekResetTime": 1785234900000_i64
                        }
                    },
                    "success": true,
                    "httpStatus": 200
                }
            },
            "successResponse": true
        });
        let usage = QwenCloudProvider::snapshot_to_usage(
            QwenCloudProvider::parse(payload.to_string().as_bytes(), None, None).unwrap(),
        )
        .unwrap();
        assert!((usage.primary.used_percent - 0.09973083333333333).abs() < 1e-9);
        assert_eq!(usage.primary.window_minutes, Some(300));
        assert!(usage.secondary.is_some());
        assert_eq!(
            usage.secondary.as_ref().and_then(|w| w.window_minutes),
            Some(10080)
        );
    }

    #[test]
    fn parses_nested_equity_list_legacy() {
        let payload = serde_json::json!({
            "code": "200",
            "successResponse": true,
            "data": {
                "TotalCount": 1,
                "Data": [
                    {
                        "InstanceCode": "qwen-token-plan",
                        "Status": "NORMAL",
                        "EndTime": 1_701_000_000_000_i64,
                        "EquityList": [
                            {
                                "Type": "CREDITS",
                                "CycleTotalValue": "1000",
                                "CycleSurplusValue": "875"
                            }
                        ]
                    }
                ]
            }
        });
        let snapshot =
            QwenCloudProvider::parse(payload.to_string().as_bytes(), None, None).unwrap();
        assert_eq!(snapshot.total_quota, Some(1000.0));
        assert_eq!(snapshot.remaining_quota, Some(875.0));
        let usage = QwenCloudProvider::snapshot_to_usage(snapshot).unwrap();
        assert_eq!(usage.primary.used_percent, 12.5);
        assert_eq!(usage.primary.window_minutes, Some(LEGACY_MINUTES));
    }

    #[test]
    fn parses_flat_subscription_summary_legacy() {
        let payload = serde_json::json!({
            "Success": true,
            "Data": {
                "TotalCount": 1,
                "TotalValue": 2000,
                "TotalSurplusValue": 1500
            }
        });
        let snapshot =
            QwenCloudProvider::parse(payload.to_string().as_bytes(), None, None).unwrap();
        assert_eq!(snapshot.total_quota, Some(2000.0));
        assert_eq!(snapshot.remaining_quota, Some(1500.0));
        assert_eq!(
            used_percent(
                snapshot.used_quota,
                snapshot.total_quota,
                snapshot.remaining_quota
            ),
            Some(25.0)
        );
    }

    #[test]
    fn login_payload_maps_to_auth_required() {
        let err = QwenCloudProvider::parse(
            br#"{"code":"ConsoleNeedLogin","message":"You need to log in.","successResponse":false}"#,
            None,
            None,
        )
        .unwrap_err();
        assert!(matches!(err, ProviderError::AuthRequired));
    }

    #[test]
    fn attaches_plan_name_from_subscription() {
        let usage_payload = serde_json::json!({
            "data": {
                "per5HourPercentage": 0.1,
                "per5HourResetTime": 1700003600000_i64,
                "per1WeekPercentage": 0.2,
                "per1WeekResetTime": 1700086400000_i64
            }
        });
        let subscription = serde_json::json!({
            "data": { "specCode": "pro" }
        });
        let snapshot = QwenCloudProvider::parse(
            usage_payload.to_string().as_bytes(),
            Some(subscription.to_string().as_bytes()),
            None,
        )
        .unwrap();
        assert_eq!(snapshot.plan_name.as_deref(), Some("Pro"));
        let usage = QwenCloudProvider::snapshot_to_usage(snapshot).unwrap();
        assert_eq!(usage.login_method.as_deref(), Some("Pro"));
    }

    #[test]
    fn extracts_sec_token_from_html() {
        assert_eq!(
            extract_sec_token(r#"<script>sec_token = "qwen-html-token";</script>"#).as_deref(),
            Some("qwen-html-token")
        );
        assert_eq!(
            cookie_value("login_aliyunid_csrf", "foo=bar; login_aliyunid_csrf=tok"),
            Some("tok".to_string())
        );
    }

    #[test]
    fn metadata_labels_match_upstream() {
        let provider = QwenCloudProvider::new();
        assert_eq!(provider.metadata().session_label, "5-hour");
        assert_eq!(provider.metadata().weekly_label, "Weekly");
        assert!(!provider.metadata().default_enabled);
        assert_eq!(provider.metadata().dashboard_url, Some(DASHBOARD_URL));
    }
}
