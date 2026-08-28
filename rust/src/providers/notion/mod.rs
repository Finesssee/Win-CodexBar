//! Notion AI provider implementation (upstream CodexBar 0.47.0 / #2552).
//!
//! Cookie-authenticated workspace allowance tracking via:
//! - `POST https://app.notion.com/api/v3/getSpaces`
//! - `POST https://app.notion.com/api/v3/getCreditRateLimitStatus` body `{"spaceId":...}`
//!
//! Reports a rolling (session-shaped) primary window and a billing-period secondary
//! window. Billing length uses the shared monthly sentinel so calendar-month pace
//! can resolve the real cycle ending at `periodEndMs`.

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use reqwest::Client;
use serde_json::Value;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};
use crate::providers::browser_cookie_header;

const BASE_URL: &str = "https://app.notion.com";
const GET_SPACES_URL: &str = "https://app.notion.com/api/v3/getSpaces";
const RATE_LIMIT_URL: &str = "https://app.notion.com/api/v3/getCreditRateLimitStatus";
const DASHBOARD_URL: &str = "https://app.notion.com/";
const STATUS_PAGE_URL: &str = "https://status.notion.so";
const SESSION_COOKIE_NAME: &str = "token_v2";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";

/// Cookie domains: app host first, legacy `notion.so` kept for pre-move sessions.
const COOKIE_DOMAINS: &[&str] = &[
    "app.notion.com",
    "www.notion.com",
    "notion.com",
    "www.notion.so",
    "notion.so",
];

/// Shared monthly pace sentinel (`30d`) — calendar-month resolution replaces it
/// with the real cycle length ending at `resets_at` (upstream `ProviderPaceCapability`).
const MONTHLY_WINDOW_SENTINEL_MINUTES: u32 = 30 * 24 * 60;

pub struct NotionProvider {
    metadata: ProviderMetadata,
}

#[derive(Debug, Clone)]
struct NotionWorkspace {
    id: String,
    name: Option<String>,
    subscription_tier: Option<String>,
}

impl NotionWorkspace {
    fn may_have_allowance(&self) -> bool {
        matches!(
            self.subscription_tier
                .as_deref()
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("business") | Some("enterprise")
        )
    }

    fn display_tier(&self) -> Option<String> {
        let raw = self.subscription_tier.as_deref()?.trim();
        if raw.is_empty() {
            return None;
        }
        let mut chars = raw.chars();
        let first = chars.next()?.to_uppercase().collect::<String>();
        Some(format!("{first}{}", chars.as_str()))
    }
}

#[derive(Debug, Clone)]
struct NotionAccount {
    user_id: Option<String>,
    email: Option<String>,
    workspaces: Vec<NotionWorkspace>,
}

impl NotionAccount {
    fn resolve_workspace(&self, preferred_id: Option<&str>) -> Option<&NotionWorkspace> {
        if let Some(preferred) = normalize_space_id(preferred_id)
            && let Some(match_) = self
                .workspaces
                .iter()
                .find(|ws| normalize_space_id(Some(&ws.id)).as_deref() == Some(preferred.as_str()))
        {
            return Some(match_);
        }
        // Unknown preferred id is almost always a typo; fall back rather than 403.
        self.workspaces
            .iter()
            .find(|ws| ws.may_have_allowance())
            .or_else(|| self.workspaces.first())
    }
}

#[derive(Debug, Clone)]
struct RollingWindow {
    window: Option<String>,
    used: Option<f64>,
    limit: Option<f64>,
}

#[derive(Debug, Clone)]
struct BillingPeriodWindow {
    used: Option<f64>,
    limit: Option<f64>,
    period_end_ms: Option<f64>,
}

#[derive(Debug, Clone)]
struct CreditRateLimitStatus {
    status: Option<String>,
    window: Option<RollingWindow>,
    resets_in_seconds: Option<f64>,
    billing_period_window: Option<BillingPeriodWindow>,
}

impl CreditRateLimitStatus {
    fn is_not_applicable(&self) -> bool {
        self.status
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case("not_applicable"))
    }
}

impl NotionProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Notion,
                display_name: "Notion AI",
                session_label: "Rolling",
                weekly_label: "Monthly",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some(DASHBOARD_URL),
                status_page_url: Some(STATUS_PAGE_URL),
            },
        }
    }

    fn resolve_cookie_header(ctx: &FetchContext) -> Result<String, ProviderError> {
        if let Some(raw) = ctx.manual_cookie_header.as_deref()
            && let Some(header) = normalize_cookie_header(raw)
        {
            return Ok(header);
        }
        browser_cookie_header(COOKIE_DOMAINS)
            .and_then(|header| normalize_cookie_header(&header).ok_or(ProviderError::NoCookies))
    }

    async fn fetch_via_web(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProviderError> {
        let cookie_header = Self::resolve_cookie_header(ctx)?;
        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(ctx.web_timeout.max(1)))
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        let account = Self::fetch_account(&client, &cookie_header).await?;
        let workspace = account
            .resolve_workspace(ctx.workspace_id.as_deref())
            .cloned()
            .ok_or_else(|| {
                ProviderError::Other("No Notion workspace found for this account.".into())
            })?;

        let rate_limit = Self::fetch_rate_limit(&client, &cookie_header, &workspace.id).await?;

        if rate_limit.is_not_applicable() {
            let name = workspace.name.as_deref().unwrap_or("this workspace");
            return Err(ProviderError::Other(format!(
                "Notion AI usage allowance is not tracked for \"{name}\". \
                 Allowances apply to Business and Enterprise workspaces."
            )));
        }

        build_usage_snapshot(&rate_limit, Some(&workspace), Some(&account), Utc::now())
    }

    async fn fetch_account(
        client: &Client,
        cookie_header: &str,
    ) -> Result<NotionAccount, ProviderError> {
        let body = Self::post_json(
            client,
            GET_SPACES_URL,
            cookie_header,
            &Value::Object(Default::default()),
        )
        .await?;
        parse_spaces(&body)
    }

    async fn fetch_rate_limit(
        client: &Client,
        cookie_header: &str,
        space_id: &str,
    ) -> Result<CreditRateLimitStatus, ProviderError> {
        let mut map = serde_json::Map::new();
        map.insert("spaceId".into(), Value::String(space_id.to_string()));
        let body =
            Self::post_json(client, RATE_LIMIT_URL, cookie_header, &Value::Object(map)).await?;
        parse_rate_limit_status(&body)
    }

    async fn post_json(
        client: &Client,
        url: &str,
        cookie_header: &str,
        body: &Value,
    ) -> Result<Value, ProviderError> {
        let resp = client
            .post(url)
            .header("Cookie", cookie_header)
            .header("Content-Type", "application/json")
            .header("Accept", "*/*")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("User-Agent", USER_AGENT)
            .header("Referer", DASHBOARD_URL)
            .header("Origin", BASE_URL)
            .header("Sec-Fetch-Dest", "empty")
            .header("Sec-Fetch-Mode", "cors")
            .header("Sec-Fetch-Site", "same-origin")
            .json(body)
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status.as_u16() == 401 {
            return Err(ProviderError::AuthRequired);
        }
        if !status.is_success() {
            return Err(ProviderError::Other(format!(
                "Notion API error: HTTP {status}"
            )));
        }

        serde_json::from_str(&text)
            .map_err(|e| ProviderError::Parse(format!("Could not parse Notion usage: {e}")))
    }
}

impl Default for NotionProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for NotionProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Notion
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Fetching Notion AI usage");
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

    fn supports_cli(&self) -> bool {
        false
    }
}

fn build_usage_snapshot(
    rate_limit: &CreditRateLimitStatus,
    workspace: Option<&NotionWorkspace>,
    account: Option<&NotionAccount>,
    now: DateTime<Utc>,
) -> Result<UsageSnapshot, ProviderError> {
    let rolling = rate_limit.window.as_ref().and_then(|window| {
        percent(window.used, window.limit).map(|used_percent| {
            RateWindow::with_details(
                used_percent,
                rolling_minutes(window.window.as_deref()),
                rolling_reset(rate_limit.resets_in_seconds, now),
                None,
            )
        })
    });

    let billing = rate_limit
        .billing_period_window
        .as_ref()
        .and_then(|billing| {
            percent(billing.used, billing.limit).map(|used_percent| {
                RateWindow::with_details(
                    used_percent,
                    Some(MONTHLY_WINDOW_SENTINEL_MINUTES),
                    date_from_milliseconds(billing.period_end_ms),
                    None,
                )
            })
        });

    let (primary, secondary) = match (rolling, billing) {
        (Some(primary), secondary) => (primary, secondary),
        (None, Some(primary)) => (primary, None),
        (None, None) => {
            return Err(ProviderError::Parse(
                "getCreditRateLimitStatus returned no measurable usage windows.".into(),
            ));
        }
    };

    let mut snapshot = UsageSnapshot::new(primary);
    if let Some(secondary) = secondary {
        snapshot = snapshot.with_secondary(secondary);
    }
    snapshot.updated_at = now;

    if let Some(email) = account.and_then(|a| a.email.clone()) {
        snapshot = snapshot.with_email(email);
    }
    if let Some(name) = workspace.and_then(|w| w.name.clone()) {
        snapshot = snapshot.with_organization(name);
    }
    if let Some(tier) = workspace.and_then(|w| w.display_tier()) {
        snapshot = snapshot.with_login_method(tier);
    }

    Ok(snapshot)
}

fn parse_rate_limit_status(json: &Value) -> Result<CreditRateLimitStatus, ProviderError> {
    let status = json
        .get("status")
        .and_then(Value::as_str)
        .map(str::to_string);
    let window = json.get("window").and_then(|w| {
        if w.is_null() {
            return None;
        }
        Some(RollingWindow {
            window: w.get("window").and_then(Value::as_str).map(str::to_string),
            used: number_field(w, "used"),
            limit: number_field(w, "limit"),
        })
    });
    let billing_period_window = json.get("billingPeriodWindow").and_then(|w| {
        if w.is_null() {
            return None;
        }
        Some(BillingPeriodWindow {
            used: number_field(w, "used"),
            limit: number_field(w, "limit"),
            period_end_ms: number_field(w, "periodEndMs"),
        })
    });
    let resets_in_seconds = number_field(json, "resetsInSeconds");

    let parsed = CreditRateLimitStatus {
        status,
        window,
        resets_in_seconds,
        billing_period_window,
    };

    if !parsed.is_not_applicable()
        && parsed.window.is_none()
        && parsed.billing_period_window.is_none()
    {
        return Err(ProviderError::Parse(
            "getCreditRateLimitStatus returned no usage windows.".into(),
        ));
    }

    Ok(parsed)
}

fn parse_spaces(json: &Value) -> Result<NotionAccount, ProviderError> {
    let root = json
        .as_object()
        .ok_or_else(|| ProviderError::Parse("getSpaces response is not a JSON object.".into()))?;

    let user_id = resolve_user_id(root).ok_or_else(|| {
        ProviderError::Parse("getSpaces response did not identify a single user.".into())
    })?;

    let container = root
        .get(&user_id)
        .and_then(Value::as_object)
        .ok_or_else(|| {
            ProviderError::Parse("getSpaces response did not identify a single user.".into())
        })?;

    let mut email = None;
    let mut name = None;
    if let Some(users) = container.get("notion_user").and_then(Value::as_object) {
        let record = users
            .get(&user_id)
            .and_then(unwrap_record)
            .or_else(|| users.values().find_map(unwrap_record));
        if let Some(record) = record {
            email = record
                .get("email")
                .and_then(Value::as_str)
                .map(str::to_string);
            name = record
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
    }
    let _ = name; // identity carries email/workspace; display name unused

    let mut workspaces = Vec::new();
    if let Some(spaces) = container.get("space").and_then(Value::as_object) {
        let mut keys: Vec<&String> = spaces.keys().collect();
        keys.sort();
        for key in keys {
            let Some(record) = spaces.get(key).and_then(unwrap_record) else {
                continue;
            };
            let id = record
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or(key.as_str())
                .to_string();
            workspaces.push(NotionWorkspace {
                id,
                name: record
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                subscription_tier: record
                    .get("subscription_tier")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            });
        }
    }

    Ok(NotionAccount {
        user_id: Some(user_id),
        email,
        workspaces,
    })
}

fn resolve_user_id(root: &serde_json::Map<String, Value>) -> Option<String> {
    let identified: Vec<&String> = root
        .keys()
        .filter(|key| {
            let Some(container) = root.get(*key).and_then(Value::as_object) else {
                return false;
            };
            let Some(users) = container.get("notion_user").and_then(Value::as_object) else {
                return false;
            };
            let Some(record) = users.get(*key).and_then(unwrap_record) else {
                return false;
            };
            record.get("id").and_then(Value::as_str) == Some(key.as_str())
        })
        .collect();

    if identified.len() == 1 {
        return identified.first().map(|s| (*s).clone());
    }
    if identified.is_empty() && root.len() == 1 {
        return root.keys().next().cloned();
    }
    None
}

fn unwrap_record(raw: &Value) -> Option<&serde_json::Map<String, Value>> {
    let outer = raw.as_object()?;
    let Some(value) = outer.get("value").and_then(Value::as_object) else {
        return Some(outer);
    };
    if let Some(inner) = value.get("value").and_then(Value::as_object) {
        return Some(inner);
    }
    Some(value)
}

fn normalize_space_id(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        return None;
    }
    let compact: String = trimmed.chars().filter(|c| *c != '-').collect();
    let compact_lower = compact.to_ascii_lowercase();
    if compact_lower.len() == 32 && compact_lower.chars().all(|c| c.is_ascii_hexdigit()) {
        let chars: Vec<char> = compact_lower.chars().collect();
        let groups = [0..8, 8..12, 12..16, 16..20, 20..32];
        return Some(
            groups
                .into_iter()
                .map(|range| chars[range].iter().collect::<String>())
                .collect::<Vec<_>>()
                .join("-"),
        );
    }
    Some(trimmed.to_ascii_lowercase())
}

fn percent(used: Option<f64>, limit: Option<f64>) -> Option<f64> {
    let used = used?;
    let limit = limit?;
    if limit <= 0.0 {
        return None;
    }
    Some((used / limit * 100.0).max(0.0))
}

fn rolling_minutes(raw: Option<&str>) -> Option<u32> {
    let minutes = minutes_from_window_token(raw)?;
    if minutes == MONTHLY_WINDOW_SENTINEL_MINUTES {
        return None;
    }
    Some(minutes)
}

fn minutes_from_window_token(raw: Option<&str>) -> Option<u32> {
    let raw = raw?.trim().to_ascii_lowercase();
    if raw.is_empty() {
        return None;
    }
    let unit = raw.chars().last()?;
    let value: u32 = raw[..raw.len().saturating_sub(1)].parse().ok()?;
    if value == 0 {
        return None;
    }
    match unit {
        'm' => Some(value),
        'h' => Some(value.saturating_mul(60)),
        'd' => Some(value.saturating_mul(24 * 60)),
        'w' => Some(value.saturating_mul(7 * 24 * 60)),
        _ => None,
    }
}

fn rolling_reset(seconds: Option<f64>, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let seconds = seconds?;
    if seconds < 0.0 {
        return None;
    }
    // The window offset is whole seconds from the API, far below i64::MAX.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "API window offset is far below i64::MAX"
    )]
    let millis = (seconds * 1000.0) as i64;
    Some(now + chrono::Duration::milliseconds(millis))
}

fn date_from_milliseconds(raw: Option<f64>) -> Option<DateTime<Utc>> {
    let raw = raw?;
    if raw <= 0.0 {
        return None;
    }
    // Epoch milliseconds from the API; realistic dates are far below i64::MAX.
    #[expect(clippy::cast_possible_truncation, reason = "epoch ms dates fit i64")]
    let secs = (raw / 1000.0).floor() as i64;
    // The fractional second is in [0, 1), so nanoseconds stay below u32::MAX.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "fractional second keeps nanoseconds < u32::MAX"
    )]
    let nsecs = (((raw / 1000.0) - secs as f64) * 1_000_000_000.0) as u32;
    Utc.timestamp_opt(secs, nsecs).single()
}

fn number_field(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(|v| match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    })
}

/// Normalize a cookie header or bare `token_v2` value into a usable Cookie header.
fn normalize_cookie_header(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Strip optional `Cookie:` prefix from cURL captures.
    let without_prefix = trimmed
        .strip_prefix("Cookie:")
        .or_else(|| trimmed.strip_prefix("cookie:"))
        .map(str::trim)
        .unwrap_or(trimmed);

    if without_prefix.is_empty() {
        return None;
    }

    // Bare token_v2 value (no `=` pairs).
    if !without_prefix.contains('=') {
        return Some(format!("{SESSION_COOKIE_NAME}={without_prefix}"));
    }

    // Collapse whitespace around pairs.
    let pairs: Vec<&str> = without_prefix
        .split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty() && p.contains('='))
        .collect();
    if pairs.is_empty() {
        return None;
    }
    Some(pairs.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_spaces_json() -> Value {
        json!({
            "user-1111-2222-3333-444444444444": {
                "notion_user": {
                    "user-1111-2222-3333-444444444444": {
                        "value": {
                            "id": "user-1111-2222-3333-444444444444",
                            "email": "ada@example.com",
                            "name": "Ada"
                        }
                    }
                },
                "space": {
                    "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee": {
                        "value": {
                            "id": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
                            "name": "Personal",
                            "plan_type": "personal",
                            "subscription_tier": "personal"
                        }
                    },
                    "11111111-2222-3333-4444-555555555555": {
                        "value": {
                            "id": "11111111-2222-3333-4444-555555555555",
                            "name": "Acme Biz",
                            "plan_type": "team",
                            "subscription_tier": "business"
                        }
                    },
                    "99999999-8888-7777-6666-555555555555": {
                        "value": {
                            "value": {
                                "id": "99999999-8888-7777-6666-555555555555",
                                "name": "Nested Ent",
                                "plan_type": "team",
                                "subscription_tier": "enterprise"
                            }
                        }
                    }
                }
            }
        })
    }

    fn sample_rate_limit_json() -> Value {
        json!({
            "status": "ok",
            "window": {
                "creditType": "ai",
                "scope": "user",
                "window": "6h",
                "used": 30.0,
                "limit": 100.0
            },
            "resetsInSeconds": 3600.0,
            "billingPeriodWindow": {
                "creditType": "ai",
                "scope": "workspace",
                "cadence": "monthly",
                "used": 250.0,
                "limit": 1000.0,
                "periodEndMs": 1_735_689_600_000.0
            },
            "enforcement": "soft"
        })
    }

    #[test]
    fn parses_rate_limit_rolling_and_billing() {
        let status = parse_rate_limit_status(&sample_rate_limit_json()).unwrap();
        assert!(!status.is_not_applicable());
        let window = status.window.unwrap();
        assert_eq!(window.window.as_deref(), Some("6h"));
        assert_eq!(window.used, Some(30.0));
        assert_eq!(window.limit, Some(100.0));
        let billing = status.billing_period_window.unwrap();
        assert_eq!(billing.used, Some(250.0));
        assert_eq!(billing.limit, Some(1000.0));
        assert_eq!(billing.period_end_ms, Some(1_735_689_600_000.0));
        assert_eq!(status.resets_in_seconds, Some(3600.0));
    }

    #[test]
    fn builds_snapshot_with_rolling_primary_and_billing_secondary() {
        let status = parse_rate_limit_status(&sample_rate_limit_json()).unwrap();
        let account = parse_spaces(&sample_spaces_json()).unwrap();
        let workspace = account.resolve_workspace(None).unwrap();
        let now = Utc.with_ymd_and_hms(2025, 1, 1, 12, 0, 0).unwrap();
        let snap = build_usage_snapshot(&status, Some(workspace), Some(&account), now).unwrap();

        assert!((snap.primary.used_percent - 30.0).abs() < f64::EPSILON);
        assert_eq!(snap.primary.window_minutes, Some(6 * 60));
        assert_eq!(
            snap.primary.resets_at,
            Some(now + chrono::Duration::seconds(3600))
        );

        let secondary = snap.secondary.expect("billing secondary");
        assert!((secondary.used_percent - 25.0).abs() < f64::EPSILON);
        assert_eq!(
            secondary.window_minutes,
            Some(MONTHLY_WINDOW_SENTINEL_MINUTES)
        );
        assert_eq!(
            secondary.resets_at,
            date_from_milliseconds(Some(1_735_689_600_000.0))
        );

        assert_eq!(snap.account_email.as_deref(), Some("ada@example.com"));
        assert_eq!(snap.account_organization.as_deref(), Some("Acme Biz"));
        assert_eq!(snap.login_method.as_deref(), Some("Business"));
    }

    #[test]
    fn billing_only_window_becomes_primary() {
        let status = parse_rate_limit_status(&json!({
            "status": "ok",
            "billingPeriodWindow": {
                "used": 10.0,
                "limit": 40.0,
                "periodEndMs": 1_735_689_600_000.0
            }
        }))
        .unwrap();
        let now = Utc::now();
        let snap = build_usage_snapshot(&status, None, None, now).unwrap();
        assert!((snap.primary.used_percent - 25.0).abs() < f64::EPSILON);
        assert_eq!(
            snap.primary.window_minutes,
            Some(MONTHLY_WINDOW_SENTINEL_MINUTES)
        );
        assert!(snap.secondary.is_none());
    }

    #[test]
    fn preferred_workspace_id_is_honored_including_undashed() {
        let account = parse_spaces(&sample_spaces_json()).unwrap();
        let preferred = account
            .resolve_workspace(Some("99999999888877776666555555555555"))
            .unwrap();
        assert_eq!(preferred.name.as_deref(), Some("Nested Ent"));
        assert_eq!(preferred.subscription_tier.as_deref(), Some("enterprise"));

        let dashed = account
            .resolve_workspace(Some("99999999-8888-7777-6666-555555555555"))
            .unwrap();
        assert_eq!(dashed.id, preferred.id);
    }

    #[test]
    fn unknown_preferred_workspace_falls_back_to_business() {
        let account = parse_spaces(&sample_spaces_json()).unwrap();
        let ws = account
            .resolve_workspace(Some("00000000-0000-0000-0000-000000000000"))
            .unwrap();
        assert_eq!(ws.name.as_deref(), Some("Acme Biz"));
    }

    #[test]
    fn auto_selects_first_business_or_enterprise_workspace() {
        let account = parse_spaces(&sample_spaces_json()).unwrap();
        let ws = account.resolve_workspace(None).unwrap();
        // Sorted keys: 1111... (business) before 9999... (enterprise) before aaaa... (personal)
        assert_eq!(ws.subscription_tier.as_deref(), Some("business"));
    }

    #[test]
    fn not_applicable_status_parses_and_is_flagged() {
        let status = parse_rate_limit_status(&json!({
            "status": "not_applicable"
        }))
        .unwrap();
        assert!(status.is_not_applicable());
    }

    #[test]
    fn empty_rate_limit_body_is_rejected() {
        let err = parse_rate_limit_status(&json!({"status": "ok"})).unwrap_err();
        match err {
            ProviderError::Parse(msg) => assert!(msg.contains("no usage windows")),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn rolling_minutes_drop_monthly_sentinel_token() {
        assert_eq!(rolling_minutes(Some("6h")), Some(360));
        assert_eq!(rolling_minutes(Some("30d")), None);
        assert_eq!(rolling_minutes(Some("720h")), None);
        assert_eq!(minutes_from_window_token(Some("1w")), Some(7 * 24 * 60));
    }

    #[test]
    fn normalize_cookie_accepts_header_and_bare_token() {
        assert_eq!(
            normalize_cookie_header("token_v2=abc; other=1").as_deref(),
            Some("token_v2=abc; other=1")
        );
        assert_eq!(
            normalize_cookie_header("Cookie: token_v2=abc").as_deref(),
            Some("token_v2=abc")
        );
        assert_eq!(
            normalize_cookie_header("just-the-token-value").as_deref(),
            Some("token_v2=just-the-token-value")
        );
        assert!(normalize_cookie_header("   ").is_none());
    }

    #[test]
    fn normalize_space_id_dashes_32_hex() {
        assert_eq!(
            normalize_space_id(Some("11111111222233334444555555555555")).as_deref(),
            Some("11111111-2222-3333-4444-555555555555")
        );
        assert_eq!(
            normalize_space_id(Some("11111111-2222-3333-4444-555555555555")).as_deref(),
            Some("11111111-2222-3333-4444-555555555555")
        );
    }

    #[test]
    fn nested_value_records_parse() {
        let account = parse_spaces(&sample_spaces_json()).unwrap();
        assert!(
            account
                .workspaces
                .iter()
                .any(|w| w.name.as_deref() == Some("Nested Ent"))
        );
    }
}
