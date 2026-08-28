//! Droid (Factory) provider implementation
//!
//! Fetches usage from Factory.ai (Droid) via:
//! 1. API key bearer auth (`FACTORY_API_KEY` / encrypted store / `~/.factory/.env`)
//! 2. Browser cookies (web path)
//!
//! Auto mode tries API first when a key is resolvable, then falls back to web
//! on recoverable failures (upstream v0.43.0).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde::Deserialize;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};
use crate::providers::browser_cookie_header;
use crate::settings::ApiKeys;

/// Factory.ai host bases (bearer prefers api, cookies use app).
const FACTORY_API_BASE: &str = "https://api.factory.ai";
const FACTORY_APP_BASE: &str = "https://app.factory.ai";

const FACTORY_API_KEY_ENV: &str = "FACTORY_API_KEY";
const FACTORY_CLIENT_HEADER: &str = "web-app";

// ── Response models ──────────────────────────────────────────────────

/// Legacy / cookie subscription usage (top-level standard + premium windows).
#[derive(Debug, Deserialize)]
struct FactoryUsageResponse {
    #[serde(default)]
    standard: Option<FactoryUsageWindow>,
    #[serde(default)]
    premium: Option<FactoryUsageWindow>,
    /// Nested upstream shape: `{ "usage": { "standard": … } }`
    #[serde(default)]
    usage: Option<FactoryUsageNested>,
}

#[derive(Debug, Deserialize)]
struct FactoryUsageNested {
    #[serde(default)]
    standard: Option<FactoryTokenUsage>,
    #[serde(default)]
    premium: Option<FactoryTokenUsage>,
}

#[derive(Debug, Deserialize)]
struct FactoryUsageWindow {
    used: Option<f64>,
    allowance: Option<f64>,
}

impl FactoryUsageWindow {
    fn percent_used(&self) -> f64 {
        let used = self.used.unwrap_or(0.0);
        let allowance = self.allowance.unwrap_or(1.0);
        if allowance > 0.0 {
            ((used / allowance) * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        }
    }
}

/// Upstream token-usage fields under nested `usage`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FactoryTokenUsage {
    user_tokens: Option<i64>,
    total_allowance: Option<i64>,
    used_ratio: Option<f64>,
}

impl FactoryTokenUsage {
    fn percent_used(&self) -> f64 {
        if let Some(ratio) = self.used_ratio.filter(|r| r.is_finite()) {
            if (-0.001..=1.001).contains(&ratio) {
                return (ratio * 100.0).clamp(0.0, 100.0);
            }
            if (-0.1..=100.1).contains(&ratio) {
                return ratio.clamp(0.0, 100.0);
            }
        }
        let used = self.user_tokens.unwrap_or(0) as f64;
        let allowance = self.total_allowance.unwrap_or(0) as f64;
        if allowance > 0.0 {
            ((used / allowance) * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        }
    }
}

/// Token-rate-limits billing payload (`GET /api/billing/limits`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FactoryBillingLimitsResponse {
    #[serde(default)]
    uses_token_rate_limits_billing: bool,
    limits: Option<FactoryTokenRateLimits>,
    #[serde(default)]
    extra_usage_balance_cents: i64,
    overage_preference: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FactoryTokenRateLimits {
    standard: FactoryLimitPool,
    #[serde(default)]
    core: Option<FactoryLimitPool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FactoryLimitPool {
    five_hour: FactoryBillingWindow,
    weekly: FactoryBillingWindow,
    monthly: FactoryBillingWindow,
}

impl FactoryLimitPool {
    fn has_usage_data(&self) -> bool {
        [&self.five_hour, &self.weekly, &self.monthly]
            .iter()
            .any(|w| {
                w.used_percent > 0.0 || w.window_end.is_some() || w.seconds_remaining.is_some()
            })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FactoryBillingWindow {
    #[serde(default)]
    used_percent: f64,
    #[serde(default)]
    window_end: Option<serde_json::Value>,
    #[serde(default)]
    seconds_remaining: Option<f64>,
}

impl FactoryBillingWindow {
    fn rate_window(&self, window_minutes: Option<u32>) -> RateWindow {
        let resets_at = self
            .seconds_remaining
            .filter(|s| *s > 0.0 && s.is_finite())
            .map(|s| {
                // Seconds guarded finite and positive; the sub-millisecond
                // fraction is below the reset-time display resolution.
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "seconds guarded finite and positive; sub-millisecond fraction dropped"
                )]
                let millis = (s * 1000.0) as i64;
                Utc::now() + Duration::milliseconds(millis)
            });
        let mut window = RateWindow::with_details(
            self.used_percent.clamp(0.0, 100.0),
            window_minutes,
            resets_at,
            None,
        );
        if let Some(at) = resets_at {
            window.reset_description = Some(format!("Resets {}", at.format("%b %d at %H:%M UTC")));
        }
        window
    }
}

/// Auth response (cookie + bearer). Accepts both Win-legacy and upstream shapes.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FactoryAuthResponse {
    user: Option<FactoryUser>,
    user_profile: Option<FactoryUser>,
    organization: Option<FactoryOrganization>,
}

#[derive(Debug, Deserialize)]
struct FactoryUser {
    email: Option<String>,
    id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FactoryOrganization {
    name: Option<String>,
    tier: Option<String>,
    #[serde(rename = "planName")]
    plan_name: Option<String>,
    subscription: Option<FactorySubscription>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FactorySubscription {
    factory_tier: Option<String>,
    factory_tiers: Option<String>,
    orb_subscription: Option<FactoryOrbSubscription>,
}

#[derive(Debug, Deserialize)]
struct FactoryOrbSubscription {
    plan: Option<FactoryPlan>,
}

#[derive(Debug, Deserialize)]
struct FactoryPlan {
    name: Option<String>,
}

impl FactoryOrganization {
    fn login_method(&self) -> String {
        let tier = self
            .tier
            .as_deref()
            .or_else(|| {
                self.subscription
                    .as_ref()
                    .and_then(|s| s.factory_tier.as_deref().or(s.factory_tiers.as_deref()))
            })
            .unwrap_or("Droid");
        let plan = self.plan_name.as_deref().or_else(|| {
            self.subscription
                .as_ref()
                .and_then(|s| s.orb_subscription.as_ref())
                .and_then(|o| o.plan.as_ref())
                .and_then(|p| p.name.as_deref())
        });
        match plan.filter(|p| !p.is_empty()) {
            Some(plan) if !plan.eq_ignore_ascii_case("factory") => {
                if tier.eq_ignore_ascii_case("droid") {
                    format!("Droid ({plan})")
                } else {
                    format!("Factory {tier} - {plan}")
                }
            }
            _ => {
                if tier.eq_ignore_ascii_case("droid") {
                    "Droid".to_string()
                } else {
                    format!("Factory {tier}")
                }
            }
        }
    }
}

// ── API key resolution ───────────────────────────────────────────────

/// Clean whitespace and a single matching pair of surrounding quotes.
pub(crate) fn clean_factory_secret(raw: Option<&str>) -> Option<String> {
    let mut value = raw?.trim().to_string();
    if value.is_empty() {
        return None;
    }
    if (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
        || (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2)
    {
        value = value[1..value.len() - 1].trim().to_string();
    }
    if value.is_empty() { None } else { Some(value) }
}

/// Parse `FACTORY_API_KEY` from a Factory dotenv file body.
pub(crate) fn parse_factory_dotenv_key(contents: &str) -> Option<String> {
    for raw_line in contents.lines() {
        let mut line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("export ") {
            line = rest.trim();
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != FACTORY_API_KEY_ENV {
            continue;
        }
        // Empty assignment (`FACTORY_API_KEY=`) must not short-circuit a later real key.
        if let Some(secret) = clean_factory_secret(Some(value)) {
            return Some(secret);
        }
    }
    None
}

/// Home directory for dotenv lookup from an env map only (no process fallback).
///
/// Upstream skips dotenv when `HOME`/`USERPROFILE` are absent so tests stay hermetic.
fn factory_home_dir_from_env(env: &HashMap<String, String>) -> Option<PathBuf> {
    env.get("USERPROFILE")
        .or_else(|| env.get("HOME"))
        .and_then(|v| clean_factory_secret(Some(v)))
        .map(PathBuf::from)
}

fn factory_dotenv_path(home: &Path) -> PathBuf {
    home.join(".factory").join(".env")
}

fn api_key_from_dotenv(env: &HashMap<String, String>) -> Option<String> {
    let home = factory_home_dir_from_env(env)?;
    let path = factory_dotenv_path(&home);
    let contents = std::fs::read_to_string(path).ok()?;
    parse_factory_dotenv_key(&contents)
}

/// Resolve a Factory API key with injectable environment (for tests).
///
/// Precedence (first non-blank wins):
/// 1. Explicit / `FetchContext.api_key` (encrypted store / CLI override)
/// 2. Saved encrypted store entry (`ApiKeys`)
/// 3. `FACTORY_API_KEY` from `env`
/// 4. Read-only `%USERPROFILE%\.factory\.env` (or `HOME` when provided in `env`)
pub(crate) fn resolve_factory_api_key_from(
    explicit: Option<&str>,
    env: &HashMap<String, String>,
    saved_store: Option<&str>,
) -> Option<String> {
    if let Some(key) = clean_factory_secret(explicit) {
        return Some(key);
    }
    if let Some(key) = clean_factory_secret(saved_store) {
        return Some(key);
    }
    if let Some(key) = env
        .get(FACTORY_API_KEY_ENV)
        .and_then(|v| clean_factory_secret(Some(v)))
    {
        return Some(key);
    }
    api_key_from_dotenv(env)
}

/// Resolve a Factory API key from context, encrypted store, env, and dotenv.
pub(crate) fn resolve_factory_api_key(ctx_key: Option<&str>) -> Option<String> {
    let mut env: HashMap<String, String> = std::env::vars().collect();
    // Ensure dotenv can resolve on machines where the process map is sparse.
    if !env.contains_key("USERPROFILE")
        && !env.contains_key("HOME")
        && let Some(home) = dirs::home_dir()
    {
        env.insert("USERPROFILE".to_string(), home.display().to_string());
    }
    let saved = ApiKeys::load().get("factory").map(str::to_string);
    resolve_factory_api_key_from(ctx_key, &env, saved.as_deref())
}

/// Whether an Auto-mode API failure should fall through to the cookie web path.
///
/// Matches upstream: any non-cancellation error is recoverable in Auto; explicit
/// API mode never falls back.
pub(crate) fn factory_api_error_is_recoverable(error: &ProviderError) -> bool {
    match error {
        ProviderError::UnsupportedSource(_) => false,
        // Timeouts, network, auth, parse, missing key, 5xx-as-Other, etc.
        _ => true,
    }
}

// ── Provider ─────────────────────────────────────────────────────────

/// Droid (Factory) provider
pub struct FactoryProvider {
    metadata: ProviderMetadata,
}

impl FactoryProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Factory,
                display_name: "Droid",
                session_label: "Standard",
                weekly_label: "Premium",
                supports_opus: false,
                supports_credits: true,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://app.factory.ai"),
                status_page_url: Some("https://status.factory.ai"),
            },
        }
    }

    fn get_cookies(&self, ctx: &FetchContext) -> Result<String, ProviderError> {
        if let Some(ref manual) = ctx.manual_cookie_header {
            let trimmed = manual.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
        browser_cookie_header(&["app.factory.ai", "factory.ai", "auth.factory.ai"])
    }

    fn build_client() -> Result<reqwest::Client, ProviderError> {
        crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))
    }

    fn apply_factory_headers(req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("Origin", FACTORY_APP_BASE)
            .header("Referer", format!("{FACTORY_APP_BASE}/"))
            .header("x-factory-client", FACTORY_CLIENT_HEADER)
    }

    /// Fetch auth info with optional cookie and/or bearer token.
    async fn fetch_auth_info(
        &self,
        client: &reqwest::Client,
        base: &str,
        cookies: Option<&str>,
        bearer: Option<&str>,
    ) -> Result<FactoryAuthResponse, ProviderError> {
        let url = format!("{base}/api/app/auth/me");
        let mut req = Self::apply_factory_headers(client.get(&url));
        if let Some(c) = cookies.filter(|s| !s.is_empty()) {
            req = req.header("Cookie", c);
        }
        if let Some(token) = bearer.filter(|s| !s.is_empty()) {
            req = req.header("Authorization", format!("Bearer {token}"));
        }

        let resp = req.send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ProviderError::AuthRequired);
        }
        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(ProviderError::AuthRequired);
        }
        if !status.is_success() {
            return Err(ProviderError::Other(format!(
                "Factory auth API returned status {status}"
            )));
        }

        resp.json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))
    }

    /// Fetch legacy subscription usage.
    async fn fetch_usage_api(
        &self,
        client: &reqwest::Client,
        base: &str,
        cookies: Option<&str>,
        bearer: Option<&str>,
    ) -> Result<FactoryUsageResponse, ProviderError> {
        let url = format!("{base}/api/organization/subscription/usage?useCache=true");
        let mut req = Self::apply_factory_headers(client.get(&url));
        if let Some(c) = cookies.filter(|s| !s.is_empty()) {
            req = req.header("Cookie", c);
        }
        if let Some(token) = bearer.filter(|s| !s.is_empty()) {
            req = req.header("Authorization", format!("Bearer {token}"));
        }

        let resp = req.send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ProviderError::AuthRequired);
        }
        if !status.is_success() {
            return Err(ProviderError::Other(format!(
                "Factory usage API returned status {status}"
            )));
        }

        resp.json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))
    }

    /// Optional billing-limits probe (token-rate-limits accounts).
    async fn fetch_billing_limits(
        &self,
        client: &reqwest::Client,
        bearer: &str,
    ) -> Option<FactoryBillingLimitsResponse> {
        let url = format!("{FACTORY_API_BASE}/api/billing/limits");
        let req = Self::apply_factory_headers(client.get(&url))
            .header("Authorization", format!("Bearer {bearer}"));

        let resp = req.send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        resp.json().await.ok()
    }

    /// Fetch via bearer API key (api host first, then app host for auth/usage).
    async fn fetch_via_api(&self, api_key: &str) -> Result<UsageSnapshot, ProviderError> {
        let client = Self::build_client()?;
        tracing::debug!("Fetching Droid usage via API key");

        // Prefer token-rate-limits billing when available.
        if let Some(billing) = self.fetch_billing_limits(&client, api_key).await
            && billing.uses_token_rate_limits_billing
            && let Some(limits) = billing.limits
        {
            let auth = self
                .fetch_auth_info(&client, FACTORY_API_BASE, None, Some(api_key))
                .await
                .or(self
                    .fetch_auth_info(&client, FACTORY_APP_BASE, None, Some(api_key))
                    .await)
                .ok();
            return Ok(Self::apply_auth_info(
                snapshot_from_billing_limits(&limits, billing.overage_preference.as_deref()),
                auth,
            ));
        }

        // Auth + legacy usage: try api host, then app host. Prefer auth failures
        // from the first host over later host noise (upstream).
        let mut preferred_auth: Option<ProviderError> = None;
        let mut last_error: Option<ProviderError> = None;

        for base in [FACTORY_API_BASE, FACTORY_APP_BASE] {
            match self
                .fetch_auth_and_usage_bearer(&client, base, api_key)
                .await
            {
                Ok(snapshot) => return Ok(snapshot),
                Err(err) => {
                    if preferred_auth.is_none() && matches!(err, ProviderError::AuthRequired) {
                        preferred_auth = Some(err);
                    } else {
                        last_error = Some(err);
                    }
                }
            }
        }

        Err(preferred_auth
            .or(last_error)
            .unwrap_or(ProviderError::AuthRequired))
    }

    async fn fetch_auth_and_usage_bearer(
        &self,
        client: &reqwest::Client,
        base: &str,
        api_key: &str,
    ) -> Result<UsageSnapshot, ProviderError> {
        let auth_info = self
            .fetch_auth_info(client, base, None, Some(api_key))
            .await
            .ok();
        let usage_data = self
            .fetch_usage_api(client, base, None, Some(api_key))
            .await?;
        Ok(Self::apply_auth_info(
            Self::usage_snapshot_from_response(&usage_data),
            auth_info,
        ))
    }

    /// Fetch usage via web cookies (existing path).
    async fn fetch_via_web(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProviderError> {
        let cookies = self.get_cookies(ctx)?;
        let client = Self::build_client()?;
        let auth_info = self
            .fetch_auth_info(&client, FACTORY_APP_BASE, Some(&cookies), None)
            .await
            .ok();
        let usage_data = self
            .fetch_usage_api(&client, FACTORY_APP_BASE, Some(&cookies), None)
            .await?;

        Ok(Self::apply_auth_info(
            Self::usage_snapshot_from_response(&usage_data),
            auth_info,
        ))
    }

    fn usage_snapshot_from_response(usage_data: &FactoryUsageResponse) -> UsageSnapshot {
        // Prefer nested upstream `usage` only when it carries usable data.
        // An empty `usage: {}` must not suppress populated top-level windows.
        if let Some(nested) = &usage_data.usage
            && nested_usage_has_data(nested)
        {
            let standard_percent = nested
                .standard
                .as_ref()
                .map(FactoryTokenUsage::percent_used)
                .unwrap_or(0.0);
            let mut usage = UsageSnapshot::new(RateWindow::new(standard_percent));
            if let Some(premium) = &nested.premium {
                usage = usage.with_secondary(RateWindow::new(premium.percent_used()));
            }
            return usage;
        }

        let standard_percent = usage_data
            .standard
            .as_ref()
            .map(FactoryUsageWindow::percent_used)
            .unwrap_or(0.0);

        let mut usage = UsageSnapshot::new(RateWindow::new(standard_percent));
        if let Some(premium) = &usage_data.premium {
            usage = usage.with_secondary(RateWindow::new(premium.percent_used()));
        }

        usage
    }

    fn apply_auth_info(
        mut usage: UsageSnapshot,
        auth_info: Option<FactoryAuthResponse>,
    ) -> UsageSnapshot {
        let Some(auth) = auth_info else {
            return usage.with_login_method("Droid");
        };

        let email = auth
            .user
            .as_ref()
            .and_then(|u| u.email.clone())
            .or_else(|| auth.user_profile.as_ref().and_then(|u| u.email.clone()));
        if let Some(email) = email {
            usage = usage.with_email(email);
        }

        if let Some(org) = auth.organization {
            usage = usage.with_login_method(org.login_method());
            if let Some(org_name) = org.name {
                usage = usage.with_organization(org_name);
            }
        } else if usage.login_method.is_none() {
            usage = usage.with_login_method("Droid");
        }

        usage
    }

    async fn fetch_api_result(
        &self,
        ctx: &FetchContext,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let api_key = resolve_factory_api_key(ctx.api_key.as_deref()).ok_or_else(|| {
            ProviderError::NotInstalled(
                "Droid API key missing. Set FACTORY_API_KEY, save a key in Preferences → Providers → Droid, or add it to %USERPROFILE%\\.factory\\.env.".to_string(),
            )
        })?;
        let usage = self.fetch_via_api(&api_key).await?;
        Ok(ProviderFetchResult::new(usage, "api"))
    }

    async fn fetch_web_result(
        &self,
        ctx: &FetchContext,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let usage = self.fetch_via_web(ctx).await?;
        Ok(ProviderFetchResult::new(usage, "web"))
    }

    async fn fetch_auto(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        // Resolve once — do not double-load store/dotenv for the API branch.
        if let Some(api_key) = resolve_factory_api_key(ctx.api_key.as_deref()) {
            match self.fetch_via_api(&api_key).await {
                Ok(usage) => return Ok(ProviderFetchResult::new(usage, "api")),
                Err(err) if factory_api_error_is_recoverable(&err) => {
                    tracing::debug!(
                        "Droid API path failed ({}); falling back to web cookies",
                        crate::core::SecretRedactor::redact(&err.to_string())
                    );
                }
                Err(err) => return Err(err),
            }
        }
        self.fetch_web_result(ctx).await
    }
}

fn nested_usage_has_data(nested: &FactoryUsageNested) -> bool {
    let window_has = |w: &FactoryTokenUsage| {
        w.user_tokens.is_some()
            || w.total_allowance.is_some()
            || w.used_ratio.is_some_and(|r| r.is_finite())
    };
    nested.standard.as_ref().is_some_and(window_has)
        || nested.premium.as_ref().is_some_and(window_has)
}

fn snapshot_from_billing_limits(
    limits: &FactoryTokenRateLimits,
    overage_preference: Option<&str>,
) -> UsageSnapshot {
    let primary = limits.standard.five_hour.rate_window(Some(5 * 60));
    let secondary = limits.standard.weekly.rate_window(Some(7 * 24 * 60));
    let tertiary = limits.standard.monthly.rate_window(None);

    let mut usage = UsageSnapshot::new(primary)
        .with_secondary(secondary)
        .with_tertiary(tertiary);

    if let Some(core) = &limits.core
        && core.has_usage_data()
    {
        for (id, title, window) in [
            (
                "factory-core-5h",
                "Core 5h",
                core.five_hour.rate_window(Some(5 * 60)),
            ),
            (
                "factory-core-7d",
                "Core 7-day",
                core.weekly.rate_window(Some(7 * 24 * 60)),
            ),
            (
                "factory-core-monthly",
                "Core Monthly",
                core.monthly.rate_window(None),
            ),
        ] {
            usage = usage.with_extra_rate_window(id, title, window);
        }
    }

    let mut login = "Droid".to_string();
    if let Some(pref) = overage_preference.filter(|p| !p.is_empty()) {
        login = format!("Droid - Fallback: {pref}");
    }
    usage.with_login_method(login)
}

impl Default for FactoryProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for FactoryProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Factory
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Fetching Droid (Factory) usage");

        match ctx.source_mode {
            // Auto: API key first, then web cookies (when cookie path is allowed).
            SourceMode::Auto => self.fetch_auto(ctx).await,
            // Cli = cookie-off / no-browser path from the shell: API key only.
            // Never fall through to browser cookie scrape.
            SourceMode::Cli => self.fetch_api_result(ctx).await,
            // Win maps explicit API-token sources to OAuth (sub2api pattern).
            SourceMode::OAuth => self.fetch_api_result(ctx).await,
            SourceMode::Web => self.fetch_web_result(ctx).await,
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        // Auto (API→web), OAuth (= explicit API key), Web (cookies).
        vec![SourceMode::Auto, SourceMode::OAuth, SourceMode::Web]
    }

    fn supports_web(&self) -> bool {
        true
    }

    fn supports_cli(&self) -> bool {
        // Shell maps cookie-source "off" to Cli; Factory treats Cli as API-only.
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn cleans_quotes_and_whitespace() {
        assert_eq!(
            clean_factory_secret(Some("  \"fk-quoted\"  ")).as_deref(),
            Some("fk-quoted")
        );
        assert_eq!(
            clean_factory_secret(Some("'fk-single'")).as_deref(),
            Some("fk-single")
        );
        assert_eq!(clean_factory_secret(Some("   ")), None);
        assert_eq!(clean_factory_secret(None), None);
    }

    #[test]
    fn parses_factory_dotenv_variants() {
        assert_eq!(
            parse_factory_dotenv_key("FACTORY_API_KEY=fk-plain").as_deref(),
            Some("fk-plain")
        );
        assert_eq!(
            parse_factory_dotenv_key("export FACTORY_API_KEY='fk-single'").as_deref(),
            Some("fk-single")
        );
        assert_eq!(
            parse_factory_dotenv_key("# comment\nFACTORY_API_KEY=\"fk-double\"").as_deref(),
            Some("fk-double")
        );
        assert_eq!(parse_factory_dotenv_key("OTHER=1\n"), None);
        assert_eq!(
            parse_factory_dotenv_key(
                "export FACTORY_API_KEY='fk-quoted'\nMALFORMED\nFACTORY_API_KEY=\n"
            )
            .as_deref(),
            Some("fk-quoted")
        );
        // Empty assignment must not short-circuit a later real key.
        assert_eq!(
            parse_factory_dotenv_key("FACTORY_API_KEY=\nFACTORY_API_KEY=fk-real\n").as_deref(),
            Some("fk-real")
        );
        assert_eq!(
            parse_factory_dotenv_key("FACTORY_API_KEY=\"\"\nFACTORY_API_KEY='fk-later'\n")
                .as_deref(),
            Some("fk-later")
        );
    }

    #[test]
    fn key_resolution_honors_explicit_env_dotenv_precedence() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(home.path().join(".factory")).unwrap();
        fs::write(
            home.path().join(".factory").join(".env"),
            "FACTORY_API_KEY=fk-dotenv\n",
        )
        .unwrap();

        let env_with_key = HashMap::from([
            (
                FACTORY_API_KEY_ENV.to_string(),
                "  \"fk-env\"  ".to_string(),
            ),
            ("USERPROFILE".to_string(), home.path().display().to_string()),
        ]);
        let env_dotenv_only =
            HashMap::from([("USERPROFILE".to_string(), home.path().display().to_string())]);
        let env_home_fallback =
            HashMap::from([("HOME".to_string(), home.path().display().to_string())]);

        assert_eq!(
            resolve_factory_api_key_from(Some(" 'fk-saved' "), &env_with_key, None).as_deref(),
            Some("fk-saved")
        );
        assert_eq!(
            resolve_factory_api_key_from(None, &env_with_key, Some(" fk-store ")).as_deref(),
            Some("fk-store")
        );
        assert_eq!(
            resolve_factory_api_key_from(None, &env_with_key, None).as_deref(),
            Some("fk-env")
        );
        assert_eq!(
            resolve_factory_api_key_from(None, &env_dotenv_only, None).as_deref(),
            Some("fk-dotenv")
        );
        assert_eq!(
            resolve_factory_api_key_from(None, &env_home_fallback, None).as_deref(),
            Some("fk-dotenv")
        );
        assert_eq!(
            resolve_factory_api_key_from(None, &HashMap::new(), None),
            None
        );
    }

    #[test]
    fn parses_billing_limits_fixture_json() {
        let body = r#"{
          "usesTokenRateLimitsBilling": true,
          "limits": {
            "standard": {
              "fiveHour": { "usedPercent": 12, "secondsRemaining": 3600 },
              "weekly": { "usedPercent": 34, "secondsRemaining": 86400 },
              "monthly": { "usedPercent": 56, "secondsRemaining": 604800 }
            }
          },
          "extraUsageBalanceCents": 0,
          "extraUsageAllowed": false,
          "tokenRateLimitsRolloutEligible": true
        }"#;
        let parsed: FactoryBillingLimitsResponse = serde_json::from_str(body).unwrap();
        assert!(parsed.uses_token_rate_limits_billing);
        let limits = parsed.limits.unwrap();
        let snap = snapshot_from_billing_limits(&limits, None);
        assert!((snap.primary.used_percent - 12.0).abs() < f64::EPSILON);
        assert!((snap.secondary.unwrap().used_percent - 34.0).abs() < f64::EPSILON);
        assert!((snap.tertiary.unwrap().used_percent - 56.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parses_legacy_usage_fixture_json() {
        let body = r#"{
          "standard": { "used": 25.0, "allowance": 100.0 },
          "premium": { "used": 10.0, "allowance": 50.0 }
        }"#;
        let parsed: FactoryUsageResponse = serde_json::from_str(body).unwrap();
        let snap = FactoryProvider::usage_snapshot_from_response(&parsed);
        assert!((snap.primary.used_percent - 25.0).abs() < f64::EPSILON);
        assert!((snap.secondary.unwrap().used_percent - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parses_nested_usage_fixture_json() {
        let body = r#"{
          "usage": {
            "standard": { "userTokens": 1200, "totalAllowance": 4000, "usedRatio": 0.3 },
            "premium": { "userTokens": 100, "totalAllowance": 1000, "usedRatio": 0.1 }
          }
        }"#;
        let parsed: FactoryUsageResponse = serde_json::from_str(body).unwrap();
        let snap = FactoryProvider::usage_snapshot_from_response(&parsed);
        assert!((snap.primary.used_percent - 30.0).abs() < f64::EPSILON);
        assert!((snap.secondary.unwrap().used_percent - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn empty_nested_usage_falls_through_to_top_level() {
        let body = r#"{
          "usage": {},
          "standard": { "used": 40.0, "allowance": 100.0 },
          "premium": { "used": 5.0, "allowance": 50.0 }
        }"#;
        let parsed: FactoryUsageResponse = serde_json::from_str(body).unwrap();
        let snap = FactoryProvider::usage_snapshot_from_response(&parsed);
        assert!((snap.primary.used_percent - 40.0).abs() < f64::EPSILON);
        assert!((snap.secondary.unwrap().used_percent - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn available_sources_do_not_advertise_cli() {
        let sources = FactoryProvider::new().available_sources();
        assert!(!sources.contains(&SourceMode::Cli));
        assert!(sources.contains(&SourceMode::Auto));
        assert!(sources.contains(&SourceMode::OAuth));
        assert!(sources.contains(&SourceMode::Web));
    }

    #[test]
    fn parses_auth_fixture_json() {
        let body = r#"{
          "organization": {
            "id": "org_1",
            "name": "Acme",
            "subscription": {
              "factoryTier": "team",
              "orbSubscription": {
                "plan": { "name": "Team", "id": "plan_1" },
                "status": "active"
              }
            }
          },
          "userProfile": { "id": "u1", "email": "user@example.com" }
        }"#;
        let auth: FactoryAuthResponse = serde_json::from_str(body).unwrap();
        let snap =
            FactoryProvider::apply_auth_info(UsageSnapshot::new(RateWindow::new(0.0)), Some(auth));
        assert_eq!(snap.account_email.as_deref(), Some("user@example.com"));
        assert_eq!(snap.account_organization.as_deref(), Some("Acme"));
        assert!(
            snap.login_method
                .as_deref()
                .is_some_and(|m| m.contains("team") || m.contains("Team"))
        );
    }

    #[test]
    fn auto_api_errors_are_recoverable() {
        assert!(factory_api_error_is_recoverable(
            &ProviderError::AuthRequired
        ));
        assert!(factory_api_error_is_recoverable(&ProviderError::Timeout));
        assert!(factory_api_error_is_recoverable(&ProviderError::Parse(
            "bad json".into()
        )));
        assert!(factory_api_error_is_recoverable(&ProviderError::Other(
            "HTTP 500".into()
        )));
        assert!(factory_api_error_is_recoverable(
            &ProviderError::NotInstalled("missing".into())
        ));
        assert!(!factory_api_error_is_recoverable(
            &ProviderError::UnsupportedSource(SourceMode::Web)
        ));
    }

    #[test]
    fn available_sources_include_auto_api_and_web() {
        let sources = FactoryProvider::new().available_sources();
        assert!(sources.contains(&SourceMode::Auto));
        assert!(sources.contains(&SourceMode::OAuth)); // explicit API
        assert!(sources.contains(&SourceMode::Web));
    }

    #[test]
    fn secret_redactor_covers_factory_keys() {
        let redacted = crate::core::SecretRedactor::redact("Factory key fk-test-key-abcdef");
        assert!(
            !redacted.contains("fk-test-key"),
            "factory key must not appear: {redacted}"
        );
    }
}
