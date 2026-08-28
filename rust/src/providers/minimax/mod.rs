//! MiniMax provider implementation
//!
//! Fetches usage data from MiniMax AI API
//! MiniMax stores API keys locally or in environment

mod coding_plan;
mod coding_plan_html;
mod local_storage;
mod token_plan;

// Re-exports for local storage import
#[allow(
    unused_imports,
    reason = "imports needed for future MiniMax provider wiring"
)]
pub use local_storage::{ImportError, MiniMaxLocalStorageImporter, MiniMaxSession};

use async_trait::async_trait;
use chrono::{DateTime, Duration, TimeZone, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::core::{
    CostSnapshot, FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId,
    ProviderMetadata, RateWindow, SourceMode, UsageSnapshot,
};

const CODING_PLAN_PATH: &str = "/user-center/payment/coding-plan";
const CODING_PLAN_QUERY: &str = "cycle_type=3";

#[derive(Debug, Deserialize)]
struct MiniMaxBillingHistoryPayload {
    #[serde(default)]
    base_resp: Option<MiniMaxBaseResponse>,
    #[serde(default)]
    charge_records: Vec<MiniMaxBillingRecord>,
}

#[derive(Debug, Deserialize)]
struct MiniMaxBaseResponse {
    status_code: Option<i64>,
    status_msg: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MiniMaxBillingRecord {
    consume_token: Option<serde_json::Value>,
    consume_input_token: Option<serde_json::Value>,
    consume_output_token: Option<serde_json::Value>,
    consume_cash: Option<serde_json::Value>,
    consume_cash_after_voucher: Option<serde_json::Value>,
    created_at: Option<serde_json::Value>,
    ymd: Option<String>,
    consume_time: Option<String>,
    method: Option<String>,
    model: Option<String>,
    result: Option<serde_json::Value>,
    status: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
struct MiniMaxBillingSummary {
    today_tokens: i64,
    last_30_days_tokens: i64,
    today_cash: Option<f64>,
    last_30_days_cash: Option<f64>,
    top_methods: Vec<MiniMaxBillingBreakdown>,
    top_models: Vec<MiniMaxBillingBreakdown>,
}

#[derive(Debug, Clone)]
struct MiniMaxBillingBreakdown {
    name: String,
    tokens: i64,
    cash: Option<f64>,
}

/// MiniMax API region
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MiniMaxRegion {
    Global,
    ChinaMainland,
}

impl MiniMaxRegion {
    pub fn from_settings_value(value: Option<&str>) -> Self {
        match value.unwrap_or_default().trim().to_lowercase().as_str() {
            "cn" | "china" | "china-mainland" | "china_mainland" | "mainland" => {
                Self::ChinaMainland
            }
            _ => Self::Global,
        }
    }

    pub fn settings_value(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::ChinaMainland => "cn",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Global => "Global (platform.minimax.io)",
            Self::ChinaMainland => "China mainland (platform.minimaxi.com)",
        }
    }

    pub fn base_url(self) -> &'static str {
        match self {
            Self::Global => "https://platform.minimax.io",
            Self::ChinaMainland => "https://platform.minimaxi.com",
        }
    }

    pub fn api_base_url(self) -> &'static str {
        match self {
            Self::Global => "https://api.minimax.io",
            Self::ChinaMainland => "https://api.minimaxi.com",
        }
    }

    pub fn cookie_domain(self) -> &'static str {
        match self {
            Self::Global => "platform.minimax.io",
            Self::ChinaMainland => "platform.minimaxi.com",
        }
    }

    /// WWW host for the remains-API fallback (apex domain for cookie search).
    pub fn www_base_url(self) -> &'static str {
        match self {
            Self::Global => "https://www.minimax.io",
            Self::ChinaMainland => "https://www.minimaxi.com",
        }
    }

    /// Cookie search domains — apex so `cookies.rs::domain_matches` suffix-matches
    /// `platform.`/`www.` subdomains and parent-scoped session cookies.
    pub fn cookie_search_domains(self) -> [&'static str; 1] {
        match self {
            Self::Global => ["minimax.io"],
            Self::ChinaMainland => ["minimaxi.com"],
        }
    }

    /// Remains-API URL (platform host) — the coding-plan quota endpoint.
    pub fn coding_plan_remains_url(self) -> String {
        format!(
            "{}/v1/api/openplatform/coding_plan/remains",
            self.base_url()
        )
    }

    pub fn www_remains_url(self) -> String {
        format!(
            "{}/v1/api/openplatform/coding_plan/remains",
            self.www_base_url()
        )
    }

    pub fn coding_plan_url(self) -> String {
        format!(
            "{}{}?{}",
            self.base_url(),
            CODING_PLAN_PATH,
            CODING_PLAN_QUERY
        )
    }

    fn billing_history_url(self) -> String {
        format!("{}/account/amount", self.base_url())
    }
}

/// MiniMax provider
pub struct MiniMaxProvider {
    metadata: ProviderMetadata,
}

impl MiniMaxProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::MiniMax,
                display_name: "MiniMax",
                session_label: "Usage",
                weekly_label: "Monthly",
                supports_opus: false,
                supports_credits: true,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some(
                    "https://platform.minimax.io/user-center/payment/coding-plan?cycle_type=3",
                ),
                status_page_url: None,
            },
        }
    }

    pub fn region_from_settings(value: Option<&str>) -> MiniMaxRegion {
        MiniMaxRegion::from_settings_value(value)
    }

    pub fn dashboard_url_for_region(value: Option<&str>) -> String {
        Self::region_from_settings(value).coding_plan_url()
    }

    pub fn cookie_domain_for_region(value: Option<&str>) -> &'static str {
        Self::region_from_settings(value).cookie_domain()
    }

    /// Get MiniMax config directory
    fn get_minimax_config_path() -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        {
            dirs::config_dir().map(|p| p.join("minimax"))
        }
        #[cfg(not(target_os = "windows"))]
        {
            dirs::home_dir().map(|p| p.join(".minimax"))
        }
    }

    /// Read MiniMax API key
    async fn read_api_key(&self) -> Result<(String, String), ProviderError> {
        // Check environment variables first
        if let (Ok(group_id), Ok(api_key)) = (
            std::env::var("MINIMAX_GROUP_ID"),
            std::env::var("MINIMAX_API_KEY"),
        ) {
            return Ok((group_id, api_key));
        }

        // Check config file
        let config_path = Self::get_minimax_config_path()
            .ok_or_else(|| ProviderError::NotInstalled("MiniMax config not found".to_string()))?;

        let config_file = config_path.join("config.json");
        if config_file.exists() {
            let content = tokio::fs::read_to_string(&config_file)
                .await
                .map_err(|e| ProviderError::Other(e.to_string()))?;

            let json: serde_json::Value =
                serde_json::from_str(&content).map_err(|e| ProviderError::Parse(e.to_string()))?;

            let group_id = json
                .get("group_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let api_key = json
                .get("api_key")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            if let (Some(gid), Some(key)) = (group_id, api_key) {
                return Ok((gid, key));
            }
        }

        Err(ProviderError::AuthRequired)
    }

    /// Fetch usage via MiniMax API with region fallback
    async fn fetch_via_web(
        &self,
        region: MiniMaxRegion,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let (group_id, api_key) = self.read_api_key().await?;

        match self.fetch_from_region(&group_id, &api_key, region).await {
            Ok(result) => Ok(result),
            Err(ProviderError::AuthRequired) if region == MiniMaxRegion::Global => {
                self.fetch_from_region(&group_id, &api_key, MiniMaxRegion::ChinaMainland)
                    .await
            }
            Err(e) => Err(e),
        }
    }

    /// Fetch from a specific region endpoint
    async fn fetch_from_region(
        &self,
        group_id: &str,
        api_key: &str,
        region: MiniMaxRegion,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        let base_url = region.api_base_url();
        let resp = client
            .get(format!(
                "{}/v1/billing/usage?group_id={}",
                base_url, group_id
            ))
            .header("Authorization", format!("Bearer {}", api_key))
            .header("MM-API-Source", "CodexBar")
            .send()
            .await?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED
            || resp.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(ProviderError::AuthRequired);
        }

        if !resp.status().is_success() {
            return Err(ProviderError::Other(format!(
                "MiniMax API returned status {}",
                resp.status()
            )));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;

        let usage = self.parse_usage_response(&json)?;
        Ok(self.result_with_optional_billing(usage, "api", &json))
    }

    fn parse_usage_response(
        &self,
        json: &serde_json::Value,
    ) -> Result<UsageSnapshot, ProviderError> {
        // Parse MiniMax billing response
        let base_resp = json.get("base_resp");
        if let Some(base) = base_resp {
            let status_code = base
                .get("status_code")
                .and_then(|v| v.as_i64())
                .unwrap_or(-1);
            if status_code != 0 {
                return Err(ProviderError::Parse(
                    base.get("status_msg")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown error")
                        .to_string(),
                ));
            }
        }

        let used_credits = json
            .get("used_amount")
            .or_else(|| json.get("total_amount"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let credit_limit = json
            .get("total_quota")
            .or_else(|| json.get("quota"))
            .and_then(|v| v.as_f64())
            .unwrap_or(100.0);

        let used_percent = if credit_limit > 0.0 {
            (used_credits / credit_limit) * 100.0
        } else {
            0.0
        };

        let plan = json
            .get("plan_name")
            .or_else(|| json.get("current_plan_title"))
            .or_else(|| json.get("current_subscribe_title"))
            .or_else(|| json.get("combo_title"))
            .or_else(|| json.pointer("/current_combo_card/title"))
            .or_else(|| json.get("plan_type"))
            .or_else(|| json.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or("MiniMax");

        let usage = UsageSnapshot::new(RateWindow::new(used_percent)).with_login_method(plan);

        Ok(usage)
    }

    /// Fetch the billing history summary (best-effort enrichment; never fatal).
    async fn fetch_billing_summary(
        &self,
        cookie_header: &str,
        region: MiniMaxRegion,
    ) -> Result<MiniMaxBillingSummary, ProviderError> {
        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        let response = client
            .get(region.billing_history_url())
            .query(&[("page", "1"), ("limit", "100"), ("aggregate", "false")])
            .header("Cookie", cookie_header)
            .header("Accept", "application/json, text/plain, */*")
            .header("X-Requested-With", "XMLHttpRequest")
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            || response.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(ProviderError::AuthRequired);
        }
        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "MiniMax billing returned status {}",
                response.status()
            )));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ProviderError::Parse(format!("Failed to parse MiniMax billing: {e}")))?;
        parse_billing_summary(&json)
    }

    /// User-Agent string for coding-plan web requests (matches the upstream
    /// Chrome UA; kept local to minimax so we don't couple to opencodego).
    const WEB_USER_AGENT: &'static str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

    /// Fetch quota from the coding-plan page with a remains-API fallback.
    /// Mirrors upstream `MiniMaxUsageFetcher` cookie/web flow (issue #246).
    async fn fetch_coding_plan_with_cookie(
        &self,
        cookie_header: &str,
        region: MiniMaxRegion,
    ) -> Result<UsageSnapshot, ProviderError> {
        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        let coding_url = region.coding_plan_url();
        let base = region.base_url();
        let response = client
            .get(&coding_url)
            .header("Cookie", cookie_header)
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .header("User-Agent", Self::WEB_USER_AGENT)
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Origin", base)
            .header(
                "Referer",
                &format!("{base}/user-center/payment/coding-plan"),
            )
            .send()
            .await?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ProviderError::AuthRequired);
        }
        if !status.is_success() {
            return Err(ProviderError::Other(format!(
                "MiniMax coding plan returned status {status}"
            )));
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();
        let now = Utc::now();

        // 200 with JSON content-type → parse_coding_plan_value
        if content_type.contains("application/json") {
            let json: serde_json::Value = response.json().await.map_err(|e| {
                ProviderError::Parse(format!("Failed to parse coding plan JSON: {e}"))
            })?;
            match coding_plan::parse_coding_plan_value(&json, now) {
                Ok(snapshot) => return coding_plan_html::to_usage_snapshot(&snapshot, now),
                Err(ProviderError::Parse(msg)) => {
                    tracing::debug!(
                        "MiniMax coding plan JSON parse failed: {msg}; trying remains API"
                    );
                }
                Err(e) => return Err(e),
            }
            // JSON Parse-error fall-through → remains fallback (response consumed)
            return self
                .fetch_coding_plan_remains_fallback(cookie_header, region, now)
                .await;
        }

        // 200 other → read text → parse_coding_plan_html
        let html = response
            .text()
            .await
            .map_err(|e| ProviderError::Parse(format!("Failed to read coding plan HTML: {e}")))?;
        match coding_plan_html::parse_coding_plan_html(&html, now) {
            Ok(snapshot) => match &snapshot {
                // Empty-services → remains fallback
                coding_plan::MiniMaxCodingPlanSnapshot::Services(rows) if rows.is_empty() => {}
                _ => return coding_plan_html::to_usage_snapshot(&snapshot, now),
            },
            Err(ProviderError::Parse(msg)) => {
                tracing::debug!("MiniMax coding plan HTML parse failed: {msg}; trying remains API");
            }
            Err(e) => return Err(e),
        }
        // remains fallback (response already consumed by .text())
        self.fetch_coding_plan_remains_fallback(cookie_header, region, now)
            .await
    }

    /// Remains-API fallback: try the platform-host remains URL, then the www-host
    /// remains URL. Move to the second URL only on HTTP 404/405, network error, or
    /// parse failure; AuthRequired and other statuses stop the chain.
    async fn fetch_coding_plan_remains_fallback(
        &self,
        cookie_header: &str,
        region: MiniMaxRegion,
        now: DateTime<Utc>,
    ) -> Result<UsageSnapshot, ProviderError> {
        let urls = [region.coding_plan_remains_url(), region.www_remains_url()];
        let mut last_err: Option<ProviderError> = None;
        for url in &urls {
            match self
                .fetch_remains_once(cookie_header, url, region, now)
                .await
            {
                Ok(snapshot) => return coding_plan_html::to_usage_snapshot(&snapshot, now),
                Err(err) => {
                    let should_try_next =
                        matches!(err, ProviderError::Parse(_) | ProviderError::Network(_));
                    last_err = Some(err);
                    if !should_try_next {
                        break;
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| ProviderError::Parse("Missing MiniMax remains URL.".into())))
    }

    /// One remains-API request, returning the parsed snapshot.
    async fn fetch_remains_once(
        &self,
        cookie_header: &str,
        url: &str,
        region: MiniMaxRegion,
        now: DateTime<Utc>,
    ) -> Result<coding_plan::MiniMaxCodingPlanSnapshot, ProviderError> {
        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        let base = region.base_url();
        let response = client
            .get(url)
            .header("Cookie", cookie_header)
            .header("Accept", "application/json, text/plain, */*")
            .header("X-Requested-With", "XMLHttpRequest")
            .header("User-Agent", Self::WEB_USER_AGENT)
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Origin", base)
            .header(
                "Referer",
                &format!("{base}/user-center/payment/coding-plan"),
            )
            .send()
            .await?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ProviderError::AuthRequired);
        }
        if !status.is_success() {
            let msg = format!("MiniMax remains returned status {status}");
            // 404/405 → try next URL; other → stop
            if status == reqwest::StatusCode::NOT_FOUND
                || status == reqwest::StatusCode::METHOD_NOT_ALLOWED
            {
                return Err(ProviderError::Parse(msg));
            }
            return Err(ProviderError::Other(msg));
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();
        if content_type.contains("application/json") {
            let json: serde_json::Value = response
                .json()
                .await
                .map_err(|e| ProviderError::Parse(format!("Failed to parse remains JSON: {e}")))?;
            return coding_plan::parse_coding_plan_value(&json, now);
        }

        // Non-JSON remains response → treat as HTML
        let html = response
            .text()
            .await
            .map_err(|e| ProviderError::Parse(format!("Failed to read remains text: {e}")))?;
        coding_plan_html::parse_coding_plan_html(&html, now)
    }

    /// Full cookie/web fetch: quota from coding plan + best-effort billing.
    async fn fetch_with_cookie(
        &self,
        cookie_header: &str,
        region: MiniMaxRegion,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let snapshot = match self
            .fetch_coding_plan_with_cookie(cookie_header, region)
            .await
        {
            Ok(snapshot) => snapshot,
            Err(err) if coding_plan::is_token_plan_without_coding_plan(&err) => {
                // Token Plan accounts have no coding-plan subscription: the
                // legacy remains endpoint answers base_resp 2062 ("no active
                // token plan subscription"). Fall back to the console
                // token-plan endpoints (issue #254); when they carry nothing,
                // surface the original coding-plan error unchanged.
                match token_plan::fetch_token_plan_with_cookie(self, cookie_header, region).await {
                    Ok(result) => return Ok(result),
                    Err(token_plan_err @ ProviderError::AuthRequired) => {
                        return Err(token_plan_err);
                    }
                    Err(token_plan_err) => {
                        tracing::debug!(
                            "MiniMax token-plan fallback unavailable: {token_plan_err}"
                        );
                        return Err(err);
                    }
                }
            }
            Err(err) => return Err(err),
        };
        let mut result = ProviderFetchResult::new(snapshot, "web");

        // Best-effort billing enrichment — never kills the fetch.
        if let Ok(summary) = self.fetch_billing_summary(cookie_header, region).await {
            result = attach_billing_summary(result, summary);
        } else {
            tracing::warn!("MiniMax billing history unavailable; quota from coding plan only");
        }
        Ok(result)
    }

    /// Resolve the web cookie: manual cookie takes priority, then browser cookies.
    fn resolve_web_cookie(
        &self,
        ctx: &FetchContext,
        region: MiniMaxRegion,
    ) -> Result<Option<String>, ProviderError> {
        if let Some(cookie) = ctx.manual_cookie_header.as_deref()
            && !cookie.trim().is_empty()
        {
            return Ok(Some(cookie.to_string()));
        }
        match crate::providers::browser_cookie_header(&region.cookie_search_domains()) {
            Ok(header) => Ok(Some(header)),
            Err(ProviderError::NoCookies) => Ok(None),
            Err(e) => Err(e),
        }
    }
    fn result_with_optional_billing(
        &self,
        usage: UsageSnapshot,
        source_label: &str,
        json: &serde_json::Value,
    ) -> ProviderFetchResult {
        let Ok(summary) = parse_billing_summary(json) else {
            return ProviderFetchResult::new(usage, source_label);
        };
        attach_billing_summary(ProviderFetchResult::new(usage, source_label), summary)
    }

    /// Probe for MiniMax installation (credentials check)
    async fn probe_cli(&self) -> Result<UsageSnapshot, ProviderError> {
        // Check if API key is configured
        let has_env_vars = std::env::var("MINIMAX_API_KEY").is_ok();
        let has_config = Self::get_minimax_config_path()
            .map(|p| p.join("config.json").exists())
            .unwrap_or(false);

        if has_env_vars || has_config {
            let usage =
                UsageSnapshot::new(RateWindow::new(0.0)).with_login_method("MiniMax (configured)");
            Ok(usage)
        } else {
            Err(ProviderError::NotInstalled(
                "MiniMax API not configured. Set MINIMAX_API_KEY and MINIMAX_GROUP_ID environment variables".to_string()
            ))
        }
    }
}

fn parse_billing_summary(json: &serde_json::Value) -> Result<MiniMaxBillingSummary, ProviderError> {
    let payload: MiniMaxBillingHistoryPayload = serde_json::from_value(json.clone())
        .map_err(|e| ProviderError::Parse(format!("Failed to parse MiniMax billing: {e}")))?;
    if let Some(base) = payload.base_resp
        && let Some(status) = base.status_code
        && status != 0
    {
        return Err(ProviderError::Other(
            base.status_msg
                .unwrap_or_else(|| format!("MiniMax billing status {status}")),
        ));
    }
    if payload.charge_records.is_empty() {
        return Err(ProviderError::Parse(
            "MiniMax billing records not present".to_string(),
        ));
    }
    Ok(aggregate_billing(&payload.charge_records, Utc::now()))
}

fn aggregate_billing(
    records: &[MiniMaxBillingRecord],
    now: DateTime<Utc>,
) -> MiniMaxBillingSummary {
    let today_start = now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc();
    let window_start = today_start - Duration::days(29);
    let mut today_tokens = 0;
    let mut last_30_days_tokens = 0;
    let mut today_cash = 0.0;
    let mut today_has_cash = false;
    let mut last_30_days_cash = 0.0;
    let mut last_30_has_cash = false;
    let mut method_totals: HashMap<String, (i64, f64, bool)> = HashMap::new();
    let mut model_totals: HashMap<String, (i64, f64, bool)> = HashMap::new();

    for record in records {
        if !billing_record_succeeded(record) {
            continue;
        }
        let Some(date) = record_date(record) else {
            continue;
        };
        if date < window_start || date > now {
            continue;
        }
        let tokens = record_token_count(record);
        let cash = record_cash(record);
        last_30_days_tokens += tokens;
        if let Some(cash) = cash {
            last_30_days_cash += cash;
            last_30_has_cash = true;
        }
        if date >= today_start {
            today_tokens += tokens;
            if let Some(cash) = cash {
                today_cash += cash;
                today_has_cash = true;
            }
        }
        add_breakdown(&mut method_totals, record.method.as_deref(), tokens, cash);
        add_breakdown(&mut model_totals, record.model.as_deref(), tokens, cash);
    }

    MiniMaxBillingSummary {
        today_tokens,
        last_30_days_tokens,
        today_cash: today_has_cash.then_some(today_cash),
        last_30_days_cash: last_30_has_cash.then_some(last_30_days_cash),
        top_methods: top_breakdowns(method_totals),
        top_models: top_breakdowns(model_totals),
    }
}

fn billing_record_succeeded(record: &MiniMaxBillingRecord) -> bool {
    let status =
        scalar_string(record.result.as_ref()).or_else(|| scalar_string(record.status.as_ref()));
    match status
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        None => true,
        Some(value) => value.eq_ignore_ascii_case("success"),
    }
}

fn result_from_billing_summary(
    summary: MiniMaxBillingSummary,
    source_label: &str,
) -> ProviderFetchResult {
    let usage = UsageSnapshot::new(RateWindow::with_details(
        0.0,
        None,
        None,
        Some(format!(
            "{} tokens today",
            format_count(summary.today_tokens)
        )),
    ))
    .with_secondary(RateWindow::with_details(
        0.0,
        None,
        None,
        Some(format!(
            "{} tokens over last 30 days",
            format_count(summary.last_30_days_tokens)
        )),
    ))
    .with_login_method("MiniMax billing");
    attach_billing_summary(ProviderFetchResult::new(usage, source_label), summary)
}

fn attach_billing_summary(
    mut result: ProviderFetchResult,
    summary: MiniMaxBillingSummary,
) -> ProviderFetchResult {
    result.usage = result.usage.with_extra_rate_window(
        "billing-tokens-today",
        "Tokens today",
        RateWindow::with_details(0.0, None, None, Some(format_count(summary.today_tokens))),
    );
    result.usage = result.usage.with_extra_rate_window(
        "billing-tokens-30d",
        "Tokens (30 days)",
        RateWindow::with_details(
            0.0,
            None,
            None,
            Some(format_count(summary.last_30_days_tokens)),
        ),
    );
    if let Some(cash) = summary.today_cash {
        result.usage = result.usage.with_extra_rate_window(
            "billing-cash-today",
            "Spend today",
            RateWindow::with_details(0.0, None, None, Some(format!("${cash:.2}"))),
        );
    }
    if let Some(cash) = summary.last_30_days_cash {
        result.usage = result.usage.with_extra_rate_window(
            "billing-cash-30d",
            "Spend (30 days)",
            RateWindow::with_details(0.0, None, None, Some(format!("${cash:.2}"))),
        );
        result.cost = Some(CostSnapshot::new(cash, "USD", "Last 30 days"));
    }
    for (idx, item) in summary.top_methods.iter().enumerate() {
        result.usage = result.usage.with_extra_rate_window(
            format!("billing-method-{idx}"),
            format!("Method: {}", item.name),
            RateWindow::with_details(0.0, None, None, Some(breakdown_description(item))),
        );
    }
    for (idx, item) in summary.top_models.iter().enumerate() {
        result.usage = result.usage.with_extra_rate_window(
            format!("billing-model-{idx}"),
            format!("Model: {}", item.name),
            RateWindow::with_details(0.0, None, None, Some(breakdown_description(item))),
        );
    }
    result
}

fn add_breakdown(
    totals: &mut HashMap<String, (i64, f64, bool)>,
    raw_name: Option<&str>,
    tokens: i64,
    cash: Option<f64>,
) {
    let Some(name) = raw_name.map(str::trim).filter(|s| !s.is_empty()) else {
        return;
    };
    let total = totals.entry(name.to_string()).or_default();
    total.0 += tokens;
    if let Some(cash) = cash {
        total.1 += cash;
        total.2 = true;
    }
}

fn top_breakdowns(totals: HashMap<String, (i64, f64, bool)>) -> Vec<MiniMaxBillingBreakdown> {
    let mut items: Vec<_> = totals
        .into_iter()
        .map(|(name, (tokens, cash, has_cash))| MiniMaxBillingBreakdown {
            name,
            tokens,
            cash: has_cash.then_some(cash),
        })
        .collect();
    items.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.name.cmp(&b.name)));
    items.truncate(3);
    items
}

fn breakdown_description(item: &MiniMaxBillingBreakdown) -> String {
    match item.cash {
        Some(cash) => format!("{} tokens / ${cash:.2}", format_count(item.tokens)),
        None => format!("{} tokens", format_count(item.tokens)),
    }
}

fn record_token_count(record: &MiniMaxBillingRecord) -> i64 {
    let direct = value_i64(record.consume_token.as_ref()).unwrap_or(0);
    if direct > 0 {
        return direct;
    }
    value_i64(record.consume_input_token.as_ref()).unwrap_or(0)
        + value_i64(record.consume_output_token.as_ref()).unwrap_or(0)
}

fn record_cash(record: &MiniMaxBillingRecord) -> Option<f64> {
    value_f64(record.consume_cash_after_voucher.as_ref())
        .or_else(|| value_f64(record.consume_cash.as_ref()))
}

fn record_date(record: &MiniMaxBillingRecord) -> Option<DateTime<Utc>> {
    if let Some(created_at) = value_i64(record.created_at.as_ref()) {
        let seconds = if created_at > 1_000_000_000_000 {
            created_at / 1000
        } else {
            created_at
        };
        return Utc.timestamp_opt(seconds, 0).single();
    }
    if let Some(ymd) = record.ymd.as_deref() {
        for format in ["%Y-%m-%d", "%Y%m%d", "%Y/%m/%d"] {
            if let Ok(date) = chrono::NaiveDate::parse_from_str(ymd.trim(), format) {
                return Some(date.and_hms_opt(0, 0, 0)?.and_utc());
            }
        }
    }
    if let Some(text) = record.consume_time.as_deref() {
        for format in ["%Y-%m-%d %H:%M:%S", "%Y/%m/%d %H:%M:%S"] {
            if let Ok(date) = chrono::NaiveDateTime::parse_from_str(text.trim(), format) {
                return Some(date.and_utc());
            }
        }
        if let Ok(date) = DateTime::parse_from_rfc3339(text.trim()) {
            return Some(date.with_timezone(&Utc));
        }
    }
    None
}

fn value_i64(value: Option<&serde_json::Value>) -> Option<i64> {
    match value? {
        serde_json::Value::Number(number) => number.as_i64(),
        serde_json::Value::String(text) => text.trim().replace(',', "").parse().ok(),
        _ => None,
    }
}

fn value_f64(value: Option<&serde_json::Value>) -> Option<f64> {
    match value? {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(text) => text.trim().replace(',', "").parse().ok(),
        _ => None,
    }
}

fn scalar_string(value: Option<&serde_json::Value>) -> Option<String> {
    match value? {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn format_count(value: i64) -> String {
    let raw = value.to_string();
    let mut out = String::with_capacity(raw.len() + raw.len() / 3);
    for (idx, ch) in raw.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

impl Default for MiniMaxProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for MiniMaxProvider {
    fn id(&self) -> ProviderId {
        ProviderId::MiniMax
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Fetching MiniMax usage");
        let region = MiniMaxRegion::from_settings_value(ctx.api_region.as_deref());

        match ctx.source_mode {
            SourceMode::Auto => {
                // Manual cookie → try fetch_with_cookie first.
                if let Some(cookie_header) = ctx.manual_cookie_header.as_deref()
                    && !cookie_header.trim().is_empty()
                    && let Ok(result) = self.fetch_with_cookie(cookie_header, region).await
                {
                    return Ok(result);
                }
                // Browser cookie (no manual) → same fetch_with_cookie.
                if let Ok(Some(cookie)) = self.resolve_web_cookie(ctx, region)
                    && let Ok(result) = self.fetch_with_cookie(&cookie, region).await
                {
                    return Ok(result);
                }
                // Fall through to API keys.
                if let Ok(result) = self.fetch_via_web(region).await {
                    return Ok(result);
                }
                let usage = self.probe_cli().await?;
                Ok(ProviderFetchResult::new(usage, "cli"))
            }
            SourceMode::Web => match self.resolve_web_cookie(ctx, region)? {
                Some(cookie) => self.fetch_with_cookie(&cookie, region).await,
                None => self.fetch_via_web(region).await,
            },
            SourceMode::Cli => {
                let usage = self.probe_cli().await?;
                Ok(ProviderFetchResult::new(usage, "cli"))
            }
            SourceMode::OAuth => Err(ProviderError::UnsupportedSource(SourceMode::OAuth)),
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::Web, SourceMode::Cli]
    }

    fn supports_web(&self) -> bool {
        true
    }

    fn supports_cli(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimax_region_defaults_to_global_io_urls() {
        let region = MiniMaxRegion::from_settings_value(None);
        assert_eq!(region, MiniMaxRegion::Global);
        assert_eq!(region.settings_value(), "global");
        assert_eq!(region.cookie_domain(), "platform.minimax.io");
        assert_eq!(
            region.coding_plan_url(),
            "https://platform.minimax.io/user-center/payment/coding-plan?cycle_type=3"
        );
        assert_eq!(
            MiniMaxProvider::dashboard_url_for_region(None),
            "https://platform.minimax.io/user-center/payment/coding-plan?cycle_type=3"
        );
    }

    #[test]
    fn minimax_region_accepts_legacy_china_value() {
        for value in ["cn", "china", "china-mainland", "china_mainland"] {
            let region = MiniMaxRegion::from_settings_value(Some(value));
            assert_eq!(region, MiniMaxRegion::ChinaMainland);
            assert_eq!(region.settings_value(), "cn");
            assert_eq!(region.cookie_domain(), "platform.minimaxi.com");
            assert_eq!(
                region.coding_plan_url(),
                "https://platform.minimaxi.com/user-center/payment/coding-plan?cycle_type=3"
            );
        }
    }

    #[test]
    fn minimax_region_cookie_search_domains_and_www_base_url() {
        let global = MiniMaxRegion::Global;
        assert_eq!(global.www_base_url(), "https://www.minimax.io");
        assert_eq!(global.cookie_search_domains(), ["minimax.io"]);
        assert_eq!(
            global.coding_plan_remains_url(),
            "https://platform.minimax.io/v1/api/openplatform/coding_plan/remains"
        );
        assert_eq!(
            global.www_remains_url(),
            "https://www.minimax.io/v1/api/openplatform/coding_plan/remains"
        );

        let cn = MiniMaxRegion::ChinaMainland;
        assert_eq!(cn.www_base_url(), "https://www.minimaxi.com");
        assert_eq!(cn.cookie_search_domains(), ["minimaxi.com"]);
        assert_eq!(
            cn.coding_plan_remains_url(),
            "https://platform.minimaxi.com/v1/api/openplatform/coding_plan/remains"
        );
        assert_eq!(
            cn.www_remains_url(),
            "https://www.minimaxi.com/v1/api/openplatform/coding_plan/remains"
        );
    }

    #[test]
    fn aggregates_billing_history_records() {
        let records = vec![
            MiniMaxBillingRecord {
                consume_token: Some(serde_json::json!(1200)),
                consume_input_token: None,
                consume_output_token: None,
                consume_cash: Some(serde_json::json!("0.42")),
                consume_cash_after_voucher: None,
                created_at: None,
                ymd: Some("2026-12-16".to_string()),
                consume_time: None,
                method: Some("chat".to_string()),
                model: Some("abab6.5".to_string()),
                result: Some(serde_json::json!("SUCCESS")),
                status: None,
            },
            MiniMaxBillingRecord {
                consume_token: None,
                consume_input_token: Some(serde_json::json!(300)),
                consume_output_token: Some(serde_json::json!(500)),
                consume_cash: None,
                consume_cash_after_voucher: Some(serde_json::json!(0.21)),
                created_at: None,
                ymd: Some("2026-12-15".to_string()),
                consume_time: None,
                method: Some("completion".to_string()),
                model: Some("abab6.5".to_string()),
                result: None,
                status: None,
            },
        ];
        let now = Utc.with_ymd_and_hms(2026, 12, 16, 12, 0, 0).unwrap();
        let summary = aggregate_billing(&records, now);
        assert_eq!(summary.last_30_days_tokens, 2000);
        assert!((summary.last_30_days_cash.unwrap() - 0.63).abs() < 0.001);
        assert_eq!(summary.top_models[0].name, "abab6.5");
        assert_eq!(summary.top_models[0].tokens, 2000);
    }

    #[test]
    fn parses_billing_payload_into_result_extras() {
        let json = serde_json::json!({
            "base_resp": { "status_code": 0 },
            "charge_records": [{
                "consume_token": "2500",
                "consume_cash_after_voucher": "1.25",
                "ymd": Utc::now().format("%Y-%m-%d").to_string(),
                "method": "chat",
                "model": "abab6.5"
            }]
        });
        let summary = parse_billing_summary(&json).unwrap();
        let result = result_from_billing_summary(summary, "web-billing");
        assert!(result.cost.is_some());
        assert!(
            result
                .usage
                .extra_rate_windows
                .iter()
                .any(|window| window.id == "billing-tokens-30d")
        );
    }

    #[test]
    fn parses_plan_title_from_coding_plan_fields() {
        let provider = MiniMaxProvider::new();
        for (field, expected) in [
            ("plan_name", "MiniMax Star"),
            ("current_plan_title", "Coding Plan Pro"),
            ("current_subscribe_title", "Max"),
            ("combo_title", "Combo Star"),
        ] {
            let snapshot = provider
                .parse_usage_response(&serde_json::json!({
                    "base_resp": { "status_code": 0 },
                    field: expected,
                    "used_amount": 0,
                    "total_quota": 100
                }))
                .unwrap();
            assert_eq!(snapshot.login_method.as_deref(), Some(expected));
        }

        let snapshot = provider
            .parse_usage_response(&serde_json::json!({
                "base_resp": { "status_code": 0 },
                "current_combo_card": { "title": "Card Title" },
                "used_amount": 0,
                "total_quota": 100
            }))
            .unwrap();
        assert_eq!(snapshot.login_method.as_deref(), Some("Card Title"));
    }

    #[test]
    fn filters_failed_billing_records() {
        let records = vec![
            MiniMaxBillingRecord {
                consume_token: Some(serde_json::json!(1000)),
                consume_input_token: None,
                consume_output_token: None,
                consume_cash: None,
                consume_cash_after_voucher: None,
                created_at: None,
                ymd: Some("2026-05-17".to_string()),
                consume_time: None,
                method: Some("chat".to_string()),
                model: Some("MiniMax-M1".to_string()),
                result: Some(serde_json::json!("SUCCESS")),
                status: None,
            },
            MiniMaxBillingRecord {
                consume_token: Some(serde_json::json!(2000)),
                consume_input_token: None,
                consume_output_token: None,
                consume_cash: None,
                consume_cash_after_voucher: None,
                created_at: None,
                ymd: Some("2026-05-17".to_string()),
                consume_time: None,
                method: Some("chat".to_string()),
                model: Some("MiniMax-M1".to_string()),
                result: Some(serde_json::json!("FAILED")),
                status: None,
            },
            MiniMaxBillingRecord {
                consume_token: Some(serde_json::json!(3000)),
                consume_input_token: None,
                consume_output_token: None,
                consume_cash: None,
                consume_cash_after_voucher: None,
                created_at: None,
                ymd: Some("2026-05-17".to_string()),
                consume_time: None,
                method: Some("audio".to_string()),
                model: Some("speech".to_string()),
                result: None,
                status: None,
            },
            MiniMaxBillingRecord {
                consume_token: Some(serde_json::json!(4000)),
                consume_input_token: None,
                consume_output_token: None,
                consume_cash: None,
                consume_cash_after_voucher: None,
                created_at: None,
                ymd: Some("2026-05-17".to_string()),
                consume_time: None,
                method: Some("video".to_string()),
                model: Some("video".to_string()),
                result: None,
                status: Some(serde_json::json!(0)),
            },
        ];
        let now = Utc.with_ymd_and_hms(2026, 5, 17, 12, 0, 0).unwrap();
        let summary = aggregate_billing(&records, now);
        assert_eq!(summary.today_tokens, 4000);
        assert_eq!(summary.last_30_days_tokens, 4000);
        assert_eq!(summary.top_methods.len(), 2);
        assert_eq!(summary.top_methods[0].name, "audio");
        assert_eq!(summary.top_methods[0].tokens, 3000);
        assert_eq!(summary.top_methods[1].name, "chat");
        assert_eq!(summary.top_methods[1].tokens, 1000);
    }
}
