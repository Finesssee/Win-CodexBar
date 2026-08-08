//! OpenCode Go provider implementation
//!
//! Separate workspace surface that shares the `opencode.ai` cookie domain with
//! the OpenCode provider. Auto prefers local SQLite usage (upstream #2316)
//! unless a workspace override scopes the fetch to web first; Web is cookie
//! scrape only; Cli is local-only.

pub(crate) mod local;

use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use std::time::Duration;
use uuid::Uuid;

use crate::core::{
    CostSnapshot, FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId,
    ProviderMetadata, RateWindow, SourceMode, UsageSnapshot,
};

const BASE_URL: &str = "https://opencode.ai";
const SERVER_URL: &str = "https://opencode.ai/_server";
const WORKSPACES_SERVER_ID: &str =
    "def39973159c7f0483d8793a822b8dbb10d067e12c65455fcb4608459ba0234f";
const BILLING_SERVER_ID: &str = "c83b78a614689c38ebee981f9b39a8b377716db85c1fd7dbab604adc02d3313d";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

// Upstream 0.48.0 #2583 (F15) optional-Zen-balance bounds.
/// `optionalZenBalanceTimeout`: outer bound of the billing lookup.
const ZEN_BALANCE_TIMEOUT: Duration = Duration::from_secs(5);
/// `optionalZenBalanceStartDelay`: the usage page gets a head start.
const ZEN_BALANCE_START_DELAY: Duration = Duration::from_millis(25);
/// `optionalZenBalanceJoinGrace`: join bound for background/UI reads.
const ZEN_BALANCE_JOIN_GRACE: Duration = Duration::from_millis(250);

pub struct OpenCodeGoProvider {
    metadata: ProviderMetadata,
    client: Client,
}

impl OpenCodeGoProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::OpenCodeGo,
                display_name: "OpenCode Go",
                session_label: "5-hour",
                weekly_label: "Weekly",
                supports_opus: true,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://opencode.ai"),
                status_page_url: None,
            },
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    fn workspace_id_from_context(workspace_id: Option<&str>) -> Option<&str> {
        workspace_id.filter(|id| !id.is_empty())
    }

    async fn fetch_workspace_id(
        client: &Client,
        cookie_header: &str,
    ) -> Result<String, ProviderError> {
        let url = format!("{}?id={}", SERVER_URL, WORKSPACES_SERVER_ID);
        let response = client
            .get(&url)
            .header("Cookie", cookie_header)
            .header("X-Server-Id", WORKSPACES_SERVER_ID)
            .header("X-Server-Instance", format!("server-fn:{}", Uuid::new_v4()))
            .header("User-Agent", USER_AGENT)
            .header("Origin", BASE_URL)
            .header("Referer", BASE_URL)
            .header(
                "Accept",
                "text/javascript, application/json;q=0.9, */*;q=0.8",
            )
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(ProviderError::AuthRequired);
            }
            return Err(ProviderError::Other(format!(
                "OpenCode workspace API returned {}",
                status
            )));
        }

        let text = response.text().await?;
        if Self::looks_signed_out(&text) {
            return Err(ProviderError::AuthRequired);
        }

        let ids = Self::parse_workspace_ids(&text);
        ids.into_iter()
            .next()
            .ok_or_else(|| ProviderError::Parse("No workspace ID found".to_string()))
    }

    async fn fetch_usage_page(
        client: &Client,
        workspace_id: &str,
        cookie_header: &str,
    ) -> Result<String, ProviderError> {
        let url = format!("{}/workspace/{}/go", BASE_URL, workspace_id);
        Self::fetch_page_text(client, &url, cookie_header, None, "usage page").await
    }

    /// GET a page with the standard browser-ish headers; `timeout` overrides
    /// the client default when the caller is inside a smaller budget.
    async fn fetch_page_text(
        client: &Client,
        url: &str,
        cookie_header: &str,
        timeout: Option<Duration>,
        what: &str,
    ) -> Result<String, ProviderError> {
        let mut request = client
            .get(url)
            .header("Cookie", cookie_header)
            .header("User-Agent", USER_AGENT)
            .header("Referer", BASE_URL)
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            );
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        let response = request.send().await?;

        let status = response.status();
        if !status.is_success() {
            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(ProviderError::AuthRequired);
            }
            return Err(ProviderError::Other(format!(
                "OpenCode Go {what} returned {status}"
            )));
        }

        let text = response.text().await?;
        if Self::looks_signed_out(&text) {
            return Err(ProviderError::AuthRequired);
        }
        Ok(text)
    }

    fn parse_usage_text(text: &str) -> Result<UsageSnapshot, ProviderError> {
        let now = Utc::now();

        let rolling = Self::extract_window(text, &["rollingUsage", "rolling_usage", "rolling"])
            .ok_or_else(|| ProviderError::Parse("Missing rolling usage window".to_string()))?;
        let weekly = Self::extract_window(text, &["weeklyUsage", "weekly_usage", "weekly"]);
        let monthly = Self::extract_window(text, &["monthlyUsage", "monthly_usage", "monthly"]);

        let primary = RateWindow::with_details(
            rolling.0,
            Some(300),
            Some(now + chrono::Duration::seconds(rolling.1)),
            None,
        );
        let mut snap = UsageSnapshot::new(primary).with_login_method("OpenCode Go");

        if let Some((pct, reset)) = weekly {
            snap = snap.with_secondary(RateWindow::with_details(
                pct,
                Some(10080),
                Some(now + chrono::Duration::seconds(reset)),
                None,
            ));
        }

        if let Some((pct, reset)) = monthly {
            let resets_at = now + chrono::Duration::seconds(reset);
            snap = snap.with_tertiary(RateWindow::with_details(
                pct,
                RateWindow::monthly_window_minutes(Some(resets_at)).or(Some(43200)),
                Some(resets_at),
                None,
            ));
        }

        if let Some(renews_at) = super::extract_renewal(text) {
            snap = snap.with_extra_rate_window(
                "renewal",
                "Renews",
                RateWindow::with_details(0.0, None, Some(renews_at), None),
            );
        }

        Ok(snap)
    }

    /// Extract `(percent, resetInSec)` for a usage block by name.
    fn extract_window(text: &str, names: &[&str]) -> Option<(f64, i64)> {
        for name in names {
            let percent_pattern = format!(
                r#"{}[^}}]*?(?:usagePercent|usedPercent|percentUsed|percent)\s*[:=]\s*([0-9]+(?:\.[0-9]+)?)"#,
                name
            );
            let reset_pattern = format!(
                r#"{}[^}}]*?(?:resetInSec|resetInSeconds|resetSeconds|resetSec)\s*[:=]\s*([0-9]+)"#,
                name
            );

            let percent = super::extract_number(&percent_pattern, text);
            if let Some(p) = percent {
                let reset = super::extract_number(&reset_pattern, text)
                    .map(|n| n as i64)
                    .unwrap_or(0);
                // Direct percent fields arrive as integer percent in the serialized payload; no fraction scaling (upstream parseSubscription parity; win-fork #247).
                return Some((p.clamp(0.0, 100.0), reset.max(0)));
            }

            // Computed used/limit (already 0..100) — do not apply fraction *100.
            let used_pattern = format!(
                r#"{}[^}}]*?(?:used|usage|consumed)\s*[:=]\s*([0-9]+(?:\.[0-9]+)?)"#,
                name
            );
            let limit_pattern = format!(
                r#"{}[^}}]*?(?:limit|total|allowance)\s*[:=]\s*([0-9]+(?:\.[0-9]+)?)"#,
                name
            );
            if let (Some(used), Some(limit)) = (
                super::extract_number(&used_pattern, text),
                super::extract_number(&limit_pattern, text),
            ) && limit > 0.0
            {
                let reset = super::extract_number(&reset_pattern, text)
                    .map(|n| n as i64)
                    .unwrap_or(0);
                let p = (used / limit) * 100.0;
                return Some((p.clamp(0.0, 100.0), reset.max(0)));
            }
        }
        None
    }

    fn parse_workspace_ids(text: &str) -> Vec<String> {
        let pattern = r#"(wrk_[A-Za-z0-9_-]+)"#;
        let re = match regex_lite::Regex::new(pattern) {
            Ok(r) => r,
            Err(_) => return vec![],
        };
        let mut seen = Vec::new();
        for caps in re.captures_iter(text) {
            if let Some(m) = caps.get(1) {
                let s = m.as_str().to_string();
                if !seen.contains(&s) {
                    seen.push(s);
                }
            }
        }
        seen
    }

    fn looks_signed_out(text: &str) -> bool {
        let lower = text.to_lowercase();
        lower.contains("auth/authorize")
            || lower.contains("\"signin\"")
            || lower.contains("please sign in")
    }

    fn parse_zen_balance(text: &str) -> Option<f64> {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(text)
            && let Some(value) = Self::find_balance_value(&json)
        {
            return Some(value);
        }
        let patterns = [
            r#"(?i)(?:current\s+balance|zen\s+balance|現在の残高)[^$]{0,80}\$\s*([0-9][0-9,]*(?:\.[0-9]+)?)"#,
            r#"(?i)(?:balance|残高)[\s\S]{0,120}?\$\s*([0-9][0-9,]*(?:\.[0-9]+)?)"#,
        ];
        patterns.iter().find_map(|pattern| {
            let re = regex_lite::Regex::new(pattern).ok()?;
            let raw = re.captures(text)?.get(1)?.as_str().replace(',', "");
            raw.parse::<f64>().ok()
        })
    }

    fn find_balance_value(value: &serde_json::Value) -> Option<f64> {
        match value {
            serde_json::Value::Object(map) => {
                for (key, value) in map {
                    let normalized: String = key
                        .to_lowercase()
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric())
                        .collect();
                    if matches!(
                        normalized.as_str(),
                        "zenbalance"
                            | "zencurrentbalance"
                            | "currentbalance"
                            | "currentbalanceusd"
                            | "balanceusd"
                            | "usdbalance"
                    ) {
                        if let Some(number) = value.as_f64() {
                            return Some(number);
                        }
                        if let Some(text) = value.as_str()
                            && let Ok(number) = text.trim().replace(',', "").parse()
                        {
                            return Some(number);
                        }
                    }
                    if let Some(found) = Self::find_balance_value(value) {
                        return Some(found);
                    }
                }
                None
            }
            serde_json::Value::Array(items) => items.iter().find_map(Self::find_balance_value),
            _ => None,
        }
    }

    // ── Zen balance (upstream 0.48.0 #2583) ─────────────────────────────────

    /// Zen dashboard page URL (upstream `zenDashboardURL`).
    fn zen_dashboard_url(workspace_id: &str) -> String {
        format!("{BASE_URL}/workspace/{workspace_id}")
    }

    /// Fetch a server-fn endpoint (same call shape as `fetch_workspace_id`:
    /// `GET {SERVER_URL}?id=…&args=…` with the server-fn headers).
    async fn fetch_server_text(
        client: &Client,
        server_id: &str,
        args: Option<&str>,
        referer: &str,
        cookie_header: &str,
        timeout: Option<Duration>,
    ) -> Result<String, ProviderError> {
        let mut url = reqwest::Url::parse(SERVER_URL)
            .map_err(|e| ProviderError::Parse(format!("Invalid OpenCode server URL: {e}")))?;
        url.query_pairs_mut().append_pair("id", server_id);
        if let Some(args) = args {
            url.query_pairs_mut().append_pair("args", args);
        }
        let mut request = client
            .get(url)
            .header("Cookie", cookie_header)
            .header("X-Server-Id", server_id)
            .header("X-Server-Instance", format!("server-fn:{}", Uuid::new_v4()))
            .header("User-Agent", USER_AGENT)
            .header("Origin", BASE_URL)
            .header("Referer", referer)
            .header(
                "Accept",
                "text/javascript, application/json;q=0.9, */*;q=0.8",
            );
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(ProviderError::AuthRequired);
            }
            return Err(ProviderError::Other(format!(
                "OpenCode Go server returned {status}"
            )));
        }
        let text = response.text().await?;
        if Self::looks_signed_out(&text) {
            return Err(ProviderError::AuthRequired);
        }
        Ok(text)
    }

    /// Upstream `fetchZenBalance`: the dashboard HTML embeds the balance for
    /// some page states; the dedicated billing server-fn report (raw 1e-8 USD
    /// units behind a customerID marker) is the fallback. Optional enrichment
    /// — every failure degrades to `None`, never to a fetch error.
    async fn fetch_zen_balance(
        client: &Client,
        workspace_id: &str,
        cookie_header: &str,
        timeout: Duration,
    ) -> Option<f64> {
        let request_timeout = timeout.min(ZEN_BALANCE_TIMEOUT);
        let referer = Self::zen_dashboard_url(workspace_id);

        let page = Self::fetch_page_text(
            client,
            &referer,
            cookie_header,
            Some(request_timeout),
            "Zen dashboard page",
        )
        .await
        .ok()?;
        if let Some(balance) = Self::parse_zen_balance(&page) {
            return Some(balance);
        }

        let args = serde_json::json!([workspace_id]).to_string();
        let billing = Self::fetch_server_text(
            client,
            BILLING_SERVER_ID,
            Some(&args),
            &referer,
            cookie_header,
            Some(request_timeout),
        )
        .await
        .ok()?;
        parse_billing_server_balance(&billing)
    }

    /// Spawn the optional Zen balance task (25 ms start delay so the usage
    /// fetch gets the head start, per upstream). Resolves the workspace id
    /// inside the task when no override is pinned.
    fn spawn_zen_balance_task(
        &self,
        cookie_header: &str,
        workspace_id_override: Option<&str>,
        web_timeout: u64,
    ) -> (tokio::task::JoinHandle<Option<f64>>, std::time::Instant) {
        let client = self.client.clone();
        let cookie_header = cookie_header.to_string();
        let workspace_id_override = workspace_id_override.map(str::to_string);
        let timeout = Duration::from_secs(web_timeout.max(1));
        let started_at = std::time::Instant::now();
        let task = tokio::spawn(async move {
            tokio::time::sleep(ZEN_BALANCE_START_DELAY).await;
            let workspace_id = match workspace_id_override {
                Some(id) => id,
                None => Self::fetch_workspace_id(&client, &cookie_header)
                    .await
                    .ok()?,
            };
            Self::fetch_zen_balance(&client, &workspace_id, &cookie_header, timeout).await
        });
        (task, started_at)
    }

    /// Join the optional Zen balance task within the policy budget. A budget
    /// expiry cancels the in-flight HTTP work instead of leaking it.
    async fn join_zen_balance(
        mut task: tokio::task::JoinHandle<Option<f64>>,
        started_at: std::time::Instant,
        requires_optional_usage_completeness: bool,
    ) -> Option<f64> {
        let budget = zen_balance_join_budget(started_at, requires_optional_usage_completeness);
        match tokio::time::timeout(budget, &mut task).await {
            Ok(Ok(balance)) => balance,
            Ok(Err(_)) | Err(_) => {
                task.abort();
                None
            }
        }
    }

    async fn fetch_with_cookies(
        &self,
        ctx: &FetchContext,
        cookie_header: &str,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let workspace_id = match Self::workspace_id_from_context(ctx.workspace_id.as_deref()) {
            Some(workspace_id) => workspace_id.to_string(),
            None => Self::fetch_workspace_id(&self.client, cookie_header).await?,
        };
        // F15 (#2583): start the optional Zen balance fetch in parallel with the
        // usage page and bound the join from task creation, so a slow balance
        // still lands in CLI/serve usage reads without stacking a second wait.
        let (zen_task, zen_started) =
            self.spawn_zen_balance_task(cookie_header, Some(&workspace_id), ctx.web_timeout);
        let page = match Self::fetch_usage_page(&self.client, &workspace_id, cookie_header).await {
            Ok(page) => page,
            Err(err) => {
                zen_task.abort();
                return Err(err);
            }
        };
        let usage = match Self::parse_usage_text(&page) {
            Ok(usage) => usage,
            Err(err) => {
                zen_task.abort();
                return Err(err);
            }
        };
        // The /go page states embed the balance for some deployments — the
        // zero-cost parse wins over the dedicated fetch when it works.
        let balance = match Self::parse_zen_balance(&page) {
            Some(balance) => {
                zen_task.abort();
                Some(balance)
            }
            None => {
                Self::join_zen_balance(
                    zen_task,
                    zen_started,
                    ctx.requires_optional_usage_completeness,
                )
                .await
            }
        };
        Ok(Self::with_zen_balance(usage, "web", balance))
    }

    /// Attach an optional Zen balance to the snapshot: informational extra
    /// window plus the cost row (existing bridge shape).
    fn with_zen_balance(
        mut usage: UsageSnapshot,
        source: &str,
        balance: Option<f64>,
    ) -> ProviderFetchResult {
        let Some(balance) = balance else {
            return ProviderFetchResult::new(usage, source);
        };
        usage = usage.with_extra_rate_window(
            "zen-balance",
            "Zen balance",
            RateWindow::with_details(0.0, None, None, Some(format!("${balance:.2}"))),
        );
        ProviderFetchResult::new(usage, source).with_cost(CostSnapshot::new(
            balance,
            "USD",
            "Zen balance",
        ))
    }
}

impl Default for OpenCodeGoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for OpenCodeGoProvider {
    fn id(&self) -> ProviderId {
        ProviderId::OpenCodeGo
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Fetching OpenCode Go usage");

        match ctx.source_mode {
            SourceMode::Auto => {
                // Local-first unless workspace/token scope asks for web first
                // (manual cookie source is already mapped to Web by the shell).
                if Self::auto_prefers_web_first(ctx) {
                    match self.fetch_web(ctx).await {
                        Ok(result) => return Ok(result),
                        Err(e) if Self::web_error_allows_local_fallback(&e) => {
                            tracing::debug!(
                                "OpenCode Go web failed in scoped Auto; trying local: {e}"
                            );
                        }
                        Err(e) => return Err(e),
                    }
                    return self.fetch_local_with_balance(ctx).await;
                }

                match self.fetch_local_with_balance(ctx).await {
                    Ok(result) => return Ok(result),
                    Err(e) => {
                        tracing::debug!("OpenCode Go local failed in Auto; trying web: {e}");
                    }
                }
                self.fetch_web(ctx).await
            }
            SourceMode::Web => self.fetch_web(ctx).await,
            SourceMode::Cli => self.fetch_local_with_balance(ctx).await,
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

impl OpenCodeGoProvider {
    /// Auto prefers web when a workspace override or active token-account scope
    /// is present (upstream `requiresScopedWebStrategy`).
    fn auto_prefers_web_first(ctx: &FetchContext) -> bool {
        if ctx
            .workspace_id
            .as_deref()
            .is_some_and(|id| !id.trim().is_empty())
        {
            return true;
        }
        // Shell sets this when a token account is active for cookie/web scope.
        ctx.auto_prefer_web
    }

    fn web_error_allows_local_fallback(err: &ProviderError) -> bool {
        matches!(
            err,
            ProviderError::AuthRequired
                | ProviderError::NoCookies
                | ProviderError::Timeout
                | ProviderError::Network(_)
                | ProviderError::Parse(_)
                | ProviderError::Other(_)
        )
    }

    /// Local SQLite snapshot plus the optional bounded Zen balance enrichment
    /// (upstream #2583 waits for the balance in usage-snapshot reads too, not
    /// just web reads; cookie absence or a slow/broken billing lookup degrades
    /// to no balance, never to an error).
    async fn fetch_local_with_balance(
        &self,
        ctx: &FetchContext,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let snap = local::fetch_local_usage(Utc::now())?;
        let mut result = snap.to_fetch_result();
        if !ctx.include_credits {
            return Ok(result);
        }
        let cookie_header = match ctx.manual_cookie_header.clone() {
            Some(header) => Some(header),
            None => crate::providers::browser_cookie_header(&["opencode.ai"]).ok(),
        };
        let Some(cookie_header) = cookie_header else {
            return Ok(result);
        };
        let (task, started) = self.spawn_zen_balance_task(
            &cookie_header,
            ctx.workspace_id.as_deref(),
            ctx.web_timeout,
        );
        if let Some(balance) =
            Self::join_zen_balance(task, started, ctx.requires_optional_usage_completeness).await
        {
            result = Self::with_zen_balance(result.usage, "local", Some(balance));
        }
        Ok(result)
    }

    async fn fetch_web(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        if let Some(cookie_header) = &ctx.manual_cookie_header {
            return self.fetch_with_cookies(ctx, cookie_header).await;
        }

        match crate::providers::browser_cookie_header(&["opencode.ai"]) {
            Ok(cookie_header) => self.fetch_with_cookies(ctx, &cookie_header).await,
            Err(ProviderError::NoCookies) => Err(ProviderError::AuthRequired),
            Err(e) => Err(e),
        }
    }
}

// ── F15 helpers ─────────────────────────────────────────────────────────────

/// The optional-balance join bound, measured from task creation (upstream
/// `optionalZenBalanceJoinTimeout`): usage-completeness reads get the remainder
/// of the 5 s optional-balance budget so a slow usage fetch cannot stack a
/// second full wait; background reads keep the short join grace.
fn zen_balance_join_budget(
    started_at: std::time::Instant,
    requires_optional_usage_completeness: bool,
) -> Duration {
    if !requires_optional_usage_completeness {
        return ZEN_BALANCE_JOIN_GRACE;
    }
    ZEN_BALANCE_TIMEOUT.saturating_sub(started_at.elapsed())
}

/// Upstream `parseBillingServerResponse`: the billing server-fn reports the
/// balance in raw 1e-8 USD units behind a customerID marker; JSON tree first,
/// RSC-streamed text fragment second.
fn parse_billing_server_balance(text: &str) -> Option<f64> {
    const BILLING_SCALE: f64 = 100_000_000.0;
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(text)
        && let Some(raw) = find_raw_billing_balance(&json)
    {
        return Some(raw / BILLING_SCALE);
    }
    let customer_re = regex_lite::Regex::new(
        r#"(?:"customerID"|customerID)\s*:\s*(?:\$R\[\d+\]\s*=\s*)?"[^"]+""#,
    )
    .ok()?;
    customer_re.find(text)?;
    let balance_re = regex_lite::Regex::new(
        r#"(?:"balance"|balance)\s*:\s*(?:\$R\[\d+\]\s*=\s*)?(-?[0-9]+(?:\.[0-9]+)?)"#,
    )
    .ok()?;
    let raw: f64 = balance_re
        .captures(text)?
        .get(1)?
        .as_str()
        .replace(',', "")
        .parse()
        .ok()?;
    Some(raw / BILLING_SCALE)
}

/// Find a `balance` value guarded by a non-empty `customerID` sibling
/// (upstream `findRawBillingBalance`). An object holding a `balance` key
/// decides terminally — no deeper search below it once the guard fails the
/// value. Booleans are excluded like upstream's `doubleValue`.
fn find_raw_billing_balance(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(balance) = map.get("balance") {
                let customer_ok = map
                    .get("customerID")
                    .and_then(|v| v.as_str())
                    .is_some_and(|id| !id.is_empty());
                if !customer_ok {
                    return None;
                }
                return billing_numeric_value(balance);
            }
            map.values().find_map(find_raw_billing_balance)
        }
        serde_json::Value::Array(items) => items.iter().find_map(find_raw_billing_balance),
        _ => None,
    }
}

fn billing_numeric_value(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().replace(',', "").parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_workspace_ids() {
        let text = r#"{ id: "wrk_abc123", name: "x" } { id: "wrk_def456" }"#;
        let ids = OpenCodeGoProvider::parse_workspace_ids(text);
        assert_eq!(
            ids,
            vec!["wrk_abc123".to_string(), "wrk_def456".to_string()]
        );
    }

    #[test]
    fn uses_context_workspace_id_before_discovery() {
        assert_eq!(
            OpenCodeGoProvider::workspace_id_from_context(Some("wrk_override")),
            Some("wrk_override")
        );
        assert_eq!(
            OpenCodeGoProvider::workspace_id_from_context(Some("")),
            None
        );
    }

    #[test]
    fn sub_one_percent_computed_used_limit_is_not_rescaled() {
        let text = r#"
            rollingUsage: { used: 1, limit: 100, resetInSec: 600 }
            weeklyUsage: { used: 1, limit: 200, resetInSec: 86400 }
        "#;
        let snap = OpenCodeGoProvider::parse_usage_text(text).unwrap();
        assert!((snap.primary.used_percent - 1.0).abs() < 0.001);
        assert!((snap.secondary.as_ref().unwrap().used_percent - 0.5).abs() < 0.001);
    }

    #[test]
    fn parses_usage_blocks() {
        let text = r#"
            rollingUsage: { usagePercent: 42.5, resetInSec: 3600 }
            weeklyUsage: { usagePercent: 13, resetInSec: 86400 }
            monthlyUsage: { usagePercent: 7, resetInSec: 2592000 }
        "#;
        let snap = OpenCodeGoProvider::parse_usage_text(text).unwrap();
        assert!((snap.primary.used_percent - 42.5).abs() < 0.001);
        let secondary = snap.secondary.expect("weekly");
        // usagePercent: 13 is a direct integer percent → 13%
        assert!((secondary.used_percent - 13.0).abs() < 0.001);
        let tertiary = snap.tertiary.expect("monthly");
        assert!((tertiary.used_percent - 7.0).abs() < 0.001);
        let expected = RateWindow::monthly_window_minutes(tertiary.resets_at).or(Some(43200));
        assert_eq!(tertiary.window_minutes, expected);
        assert!(tertiary.resets_at.is_some());
    }

    #[test]
    fn direct_percent_one_is_not_rescaled() {
        let text = r#"rollingUsage:$R[34]={status:"ok",resetInSec:13631,usagePercent:1} weeklyUsage:$R[35]={status:"ok",resetInSec:53863,usagePercent:15}"#;
        let snap = OpenCodeGoProvider::parse_usage_text(text).unwrap();
        assert!((snap.primary.used_percent - 1.0).abs() < 0.001);
        assert!((snap.secondary.as_ref().unwrap().used_percent - 15.0).abs() < 0.001);
    }

    #[test]
    fn parses_renewal_window() {
        let text = r#"
            rollingUsage: { usagePercent: 42.5, resetInSec: 3600 }
            weeklyUsage: { usagePercent: 50, resetInSec: 86400 }
            renewAt: "2026-06-01T12:00:00Z"
        "#;
        let snap = OpenCodeGoProvider::parse_usage_text(text).unwrap();
        let renewal = snap
            .extra_rate_windows
            .iter()
            .find(|window| window.id == "renewal")
            .expect("renewal window");
        assert_eq!(renewal.title, "Renews");
        assert_eq!(
            renewal.window.resets_at.unwrap().to_rfc3339(),
            "2026-06-01T12:00:00+00:00"
        );
    }

    // ── F15: bounded optional Zen balance wait (upstream #2583) ───

    #[test]
    fn zen_join_budget_grace_vs_completeness() {
        let started = std::time::Instant::now();
        // Background/UI reads keep the short join grace.
        assert_eq!(
            zen_balance_join_budget(started, false),
            Duration::from_millis(250)
        );
        // Completeness reads get the remainder of the 5 s optional-balance
        // budget measured from task creation.
        let budget = zen_balance_join_budget(started, true);
        assert!(budget <= ZEN_BALANCE_TIMEOUT, "{budget:?}");
        assert!(budget > Duration::from_secs(4), "{budget:?}");
        // An already-exhausted budget joins immediately.
        let stale = std::time::Instant::now()
            .checked_sub(Duration::from_secs(60))
            .unwrap();
        assert_eq!(zen_balance_join_budget(stale, true), Duration::ZERO);
    }

    #[tokio::test]
    async fn slow_zen_task_is_abandoned_within_grace() {
        let started = std::time::Instant::now();
        let task = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Some(42.5)
        });
        // UI grace (250 ms) never waits out a 30 s balance fetch.
        let balance = OpenCodeGoProvider::join_zen_balance(task, started, false).await;
        assert_eq!(balance, None);
    }

    #[tokio::test]
    async fn fast_zen_task_lands_in_completeness_budget() {
        let started = std::time::Instant::now();
        let task = tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Some(42.5)
        });
        let balance = OpenCodeGoProvider::join_zen_balance(task, started, true).await;
        assert_eq!(balance, Some(42.5));
    }

    #[test]
    fn billing_server_balance_needs_customer_marker() {
        // Raw 1e-8-scaled balance behind a customerID → USD.
        assert_eq!(
            parse_billing_server_balance(r#"{"balance": 1500000000, "customerID": "cus_123"}"#),
            Some(15.0)
        );
        // Same shape without the marker is not a billing payload.
        assert_eq!(
            parse_billing_server_balance(r#"{"balance": 1500000000}"#),
            None
        );
        // Non-numeric balance with marker → no result (upstream terminal guard).
        assert_eq!(
            parse_billing_server_balance(r#"{"balance": true, "customerID": "cus_123"}"#),
            None
        );
    }

    #[test]
    fn billing_server_balance_handles_strings_nesting_and_rsc_fragments() {
        // Numeric strings coerce.
        assert_eq!(
            parse_billing_server_balance(r#"{"balance": "1,000,000,000", "customerID": "cus_1"}"#),
            Some(10.0)
        );
        // Nested containers search through.
        assert_eq!(
            parse_billing_server_balance(
                r#"{"data": {"rows": [{"balance": 250000000, "customerID": "cus_2"}]}}"#
            ),
            Some(2.5)
        );
        // RSC-streamed fragment: marker plus plain balance pair.
        assert_eq!(
            parse_billing_server_balance(r#"customerID:$R[1] = "cus_9"; "balance": -500000000"#),
            Some(-5.0)
        );
        // Marker alone is not enough.
        assert_eq!(parse_billing_server_balance(r#"customerID: "cus_9""#), None);
    }
}
