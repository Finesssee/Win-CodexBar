//! MiniMax Token Plan console fetch — fallback for Token Plan accounts whose
//! legacy coding-plan endpoint answers base_resp 2062 ("no active token plan
//! subscription"): they have no coding-plan subscription, but the console
//! token-plan endpoints still carry quota + subscription data (issue #254).
//! Upstream (steipete/CodexBar) has no support for these endpoints; the quota
//! payload reuses the shared coding-plan schema (see `coding_plan`).

use chrono::{DateTime, Utc};

use crate::core::{ProviderError, ProviderFetchResult, RateWindow, UsageSnapshot};

use super::{
    MiniMaxProvider, MiniMaxRegion, attach_billing_summary, coding_plan, coding_plan_html,
    format_count, scalar_string, value_i64,
};

/// Token Plan console endpoints (www host; issue #254): the charge-API
/// usage view plus the console backend summary/credit views.
fn token_plan_usage_url(region: MiniMaxRegion) -> String {
    format!(
        "{}/v1/api/openplatform/charge/token_plan/usage",
        region.www_base_url()
    )
}

fn token_plan_usage_summary_url(region: MiniMaxRegion) -> String {
    format!(
        "{}/backend/account/token_plan/usage_summary",
        region.www_base_url()
    )
}

fn token_plan_credit_url(region: MiniMaxRegion) -> String {
    format!(
        "{}/backend/account/token_plan_credit",
        region.www_base_url()
    )
}

/// Token Plan web fetch (issue #254): Token Plan accounts have no
/// coding-plan subscription, so the coding-plan endpoints answer base_resp
/// 2062 ("no active token plan subscription"). The console token-plan
/// endpoints still carry quota + subscription data. Each endpoint is
/// best-effort; when nothing yields data the caller surfaces the original
/// coding-plan error.
pub(crate) async fn fetch_token_plan_with_cookie(
    provider: &MiniMaxProvider,
    cookie_header: &str,
    region: MiniMaxRegion,
) -> Result<ProviderFetchResult, ProviderError> {
    tracing::debug!("MiniMax: fetching token-plan console endpoints");
    let now = Utc::now();

    // (a) charge-API token-plan usage — same model_remains/services schema
    // family as the coding-plan remains endpoint.
    let usage_snapshot = fetch_token_plan_usage_once(cookie_header, region, now).await?;

    // (b)/(c) console backend views — best-effort, every failure → absent.
    let summary =
        fetch_token_plan_backend_json(cookie_header, &token_plan_usage_summary_url(region), region)
            .await
            .and_then(|json| parse_token_plan_summary(&json));
    let title =
        fetch_token_plan_backend_json(cookie_header, &token_plan_credit_url(region), region)
            .await
            .and_then(|json| parse_token_plan_credit_title(&json));

    let mut result = assemble_token_plan_result(
        usage_snapshot,
        summary,
        title,
        ProviderError::Parse("MiniMax token plan endpoints returned no data".into()),
        now,
    )?;

    // Best-effort billing enrichment — same as the coding-plan path.
    if let Ok(summary) = provider.fetch_billing_summary(cookie_header, region).await {
        result = attach_billing_summary(result, summary);
    } else {
        tracing::warn!("MiniMax billing history unavailable; quota from token plan only");
    }
    Ok(result)
}

/// Shared GET envelope for the token-plan console endpoints — copied from
/// fetch_remains_once. Network errors propagate; callers map statuses.
async fn token_plan_get(
    cookie_header: &str,
    url: &str,
    region: MiniMaxRegion,
) -> Result<reqwest::Response, ProviderError> {
    let client = crate::core::credentialed_http_client_builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| ProviderError::Other(e.to_string()))?;

    let base = region.base_url();
    Ok(client
        .get(url)
        .header("Cookie", cookie_header)
        .header("Accept", "application/json, text/plain, */*")
        .header("X-Requested-With", "XMLHttpRequest")
        .header("User-Agent", MiniMaxProvider::WEB_USER_AGENT)
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Origin", base)
        .header(
            "Referer",
            &format!("{base}/user-center/payment/coding-plan"),
        )
        .send()
        .await?)
}

/// (a) `charge/token_plan/usage`. 401/403 → AuthRequired; any other
/// non-success or unparseable body is swallowed as absent (the backend
/// views may still carry data).
async fn fetch_token_plan_usage_once(
    cookie_header: &str,
    region: MiniMaxRegion,
    now: DateTime<Utc>,
) -> Result<Option<coding_plan::MiniMaxCodingPlanSnapshot>, ProviderError> {
    let url = token_plan_usage_url(region);
    let response = token_plan_get(cookie_header, &url, region).await?;

    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(ProviderError::AuthRequired);
    }
    if !status.is_success() {
        tracing::debug!("MiniMax token plan usage returned status {status}");
        return Ok(None);
    }

    let json: serde_json::Value = match response.json().await {
        Ok(json) => json,
        Err(e) => {
            tracing::debug!("MiniMax token plan usage returned non-JSON body: {e}");
            return Ok(None);
        }
    };
    match coding_plan::parse_coding_plan_value(&json, now) {
        Ok(snapshot) => Ok(Some(snapshot)),
        Err(e) => {
            tracing::debug!("MiniMax token plan usage parse failed: {e}");
            Ok(None)
        }
    }
}

/// (b)/(c) console backend JSON — every failure is swallowed as absent.
async fn fetch_token_plan_backend_json(
    cookie_header: &str,
    url: &str,
    region: MiniMaxRegion,
) -> Option<serde_json::Value> {
    let response = match token_plan_get(cookie_header, url, region).await {
        Ok(response) => response,
        Err(e) => {
            tracing::debug!("MiniMax token plan backend GET {url} failed: {e}");
            return None;
        }
    };
    let status = response.status();
    if !status.is_success() {
        tracing::debug!("MiniMax token plan backend GET {url} returned status {status}");
        return None;
    }
    match response.json().await {
        Ok(json) => Some(json),
        Err(e) => {
            tracing::debug!("MiniMax token plan backend GET {url} returned non-JSON: {e}");
            None
        }
    }
}

/// Token Plan subscription summary parsed from the console
/// `token_plan/usage_summary` backend view (issue #254).
#[derive(Debug, Clone, PartialEq, Eq)]
struct TokenPlanSummary {
    total_days: Option<i64>,
    active_days: Option<i64>,
    total_token_consumed: Option<i64>,
}

impl TokenPlanSummary {
    /// "123,456 tokens · 5 / 30 days active" — whichever halves are present.
    fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(consumed) = self.total_token_consumed {
            parts.push(format!("{} tokens", format_count(consumed)));
        }
        match (self.active_days, self.total_days) {
            (Some(active), Some(total)) => parts.push(format!("{active} / {total} days active")),
            (Some(active), None) => parts.push(format!("{active} days active")),
            (None, Some(total)) => parts.push(format!("{total} days total")),
            (None, None) => {}
        }
        parts.join(" · ")
    }

    fn window(&self) -> RateWindow {
        RateWindow::informational(self.describe())
    }
}

/// Look up a token-plan field on `data` first, then the root object (the
/// console backend views differ in nesting, same convention as parse_remains).
fn token_plan_field<'a>(json: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    json.get("data")
        .and_then(|d| d.get(key))
        .or_else(|| json.get(key))
}

/// (b) Lenient `token_plan/usage_summary` parse — numeric strings allowed; any
/// one of the three fields suffices.
fn parse_token_plan_summary(json: &serde_json::Value) -> Option<TokenPlanSummary> {
    let summary = TokenPlanSummary {
        total_days: value_i64(token_plan_field(json, "total_days")),
        active_days: value_i64(token_plan_field(json, "active_days")),
        total_token_consumed: value_i64(token_plan_field(json, "total_token_consumed")),
    };
    (summary.total_days.is_some()
        || summary.active_days.is_some()
        || summary.total_token_consumed.is_some())
    .then_some(summary)
}

/// (c) Plan title from the `token_plan_credit` backend view.
fn parse_token_plan_credit_title(json: &serde_json::Value) -> Option<String> {
    for key in [
        "current_subscribe_title",
        "plan_name",
        "combo_title",
        "current_plan_title",
    ] {
        if let Some(title) =
            scalar_string(token_plan_field(json, key)).filter(|t| !t.trim().is_empty())
        {
            return Some(title.trim().to_string());
        }
    }
    if let Some(title) = token_plan_field(json, "current_combo_card")
        .and_then(|card| card.get("title"))
        .and_then(|t| scalar_string(Some(t)))
        .filter(|t| !t.trim().is_empty())
    {
        return Some(title.trim().to_string());
    }
    scalar_string(token_plan_field(json, "title"))
        .filter(|t| !t.trim().is_empty())
        .map(|t| t.trim().to_string())
}

/// (d) Assemble the token-plan result: a quota snapshot with usable rows wins,
/// then the summary-only informational snapshot, else the caller's original
/// coding-plan error unchanged.
fn assemble_token_plan_result(
    usage_snapshot: Option<coding_plan::MiniMaxCodingPlanSnapshot>,
    summary: Option<TokenPlanSummary>,
    title: Option<String>,
    fallback_err: ProviderError,
    now: DateTime<Utc>,
) -> Result<ProviderFetchResult, ProviderError> {
    // (a) charge-API snapshot. to_usage_snapshot errors when the snapshot
    // carries no rows, which is exactly the "parsed but empty" case.
    if let Some(snapshot) = usage_snapshot
        && let Ok(mut usage) = coding_plan_html::to_usage_snapshot(&snapshot, now)
    {
        if let Some(summary) = &summary {
            usage =
                usage.with_extra_rate_window("token-plan-summary", "Token Plan", summary.window());
        }
        if let Some(title) = title {
            usage = usage.with_login_method(title);
        }
        return Ok(ProviderFetchResult::new(usage, "web"));
    }
    // (b) summary-only informational snapshot.
    if let Some(summary) = summary {
        let mut usage = UsageSnapshot::new(summary.window());
        if let Some(title) = title {
            usage = usage.with_login_method(title);
        }
        return Ok(ProviderFetchResult::new(usage, "web"));
    }
    Err(fallback_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn fixed_now() -> DateTime<Utc> {
        // 2026-08-03T12:00:00Z = 1,785,758,400 unix seconds.
        Utc.timestamp_opt(1_785_758_400, 0).unwrap()
    }

    /// Reporter-quoted `/backend/account/token_plan/usage_summary` shape:
    /// mixed numeric string + numbers, plus a daily breakdown we ignore.
    fn reporter_usage_summary_json() -> serde_json::Value {
        serde_json::json!({
            "data": {
                "total_days": "30",
                "active_days": 5,
                "total_token_consumed": 123456,
                "daily": [
                    { "date": "2026-08-01", "token_consumed": 40000 },
                    { "date": "2026-08-02", "token_consumed": 83456 }
                ]
            }
        })
    }

    /// Reporter-quoted `/v1/api/openplatform/charge/token_plan/usage`
    /// shape: rolling 5-hour window at 2% used + a status-3 weekly lane
    /// that the shared parser renders as unlimited.
    fn reporter_token_plan_usage_json() -> serde_json::Value {
        serde_json::json!({
            "data": {
                "model_remains": [{
                    "model_name": "MiniMax-M2.5",
                    "current_interval_total_count": 2000,
                    "current_interval_usage_count": 40,
                    "current_interval_remaining_percent": 98,
                    "current_interval_status": 0,
                    "start_time": 1785754800,
                    "end_time": 1785772800,
                    "current_weekly_total_count": 0,
                    "current_weekly_usage_count": 0,
                    "current_weekly_remaining_percent": 100,
                    "current_weekly_status": 3,
                    "weekly_start_time": 1785715200,
                    "weekly_end_time": 1786320000
                }]
            }
        })
    }

    #[test]
    fn parses_token_plan_summary_leniently() {
        let summary = parse_token_plan_summary(&reporter_usage_summary_json()).unwrap();
        assert_eq!(summary.total_days, Some(30));
        assert_eq!(summary.active_days, Some(5));
        assert_eq!(summary.total_token_consumed, Some(123456));
        assert_eq!(summary.describe(), "123,456 tokens · 5 / 30 days active");

        // Root-level fields work too.
        let summary = parse_token_plan_summary(&serde_json::json!({
            "total_days": 365,
            "active_days": 12
        }))
        .unwrap();
        assert_eq!(summary.describe(), "12 / 365 days active");
    }

    #[test]
    fn token_plan_summary_absent_when_no_known_fields() {
        assert!(parse_token_plan_summary(&serde_json::json!({})).is_none());
        assert!(
            parse_token_plan_summary(&serde_json::json!({
                "base_resp": { "status_code": 2046, "status_msg": "no summary" }
            }))
            .is_none()
        );
    }

    #[test]
    fn parses_token_plan_credit_title() {
        // Reporter-quoted `/backend/account/token_plan_credit` shape.
        let json = serde_json::json!({
            "data": { "current_subscribe_title": "TokenPlanPlus annual membership" }
        });
        assert_eq!(
            parse_token_plan_credit_title(&json).as_deref(),
            Some("TokenPlanPlus annual membership")
        );

        let combo = serde_json::json!({ "current_combo_card": { "title": "Combo Star" } });
        assert_eq!(
            parse_token_plan_credit_title(&combo).as_deref(),
            Some("Combo Star")
        );

        let generic = serde_json::json!({ "title": " TokenPlan " });
        assert_eq!(
            parse_token_plan_credit_title(&generic).as_deref(),
            Some("TokenPlan")
        );

        assert!(parse_token_plan_credit_title(&serde_json::json!({})).is_none());
    }

    #[test]
    fn charge_token_plan_usage_flows_through_shared_parser() {
        let json = reporter_token_plan_usage_json();
        let snapshot = coding_plan::parse_coding_plan_value(&json, fixed_now()).unwrap();
        let coding_plan::MiniMaxCodingPlanSnapshot::Remains { rows, .. } = &snapshot else {
            panic!("expected Remains");
        };
        assert_eq!(rows.len(), 2);

        let usage = coding_plan_html::to_usage_snapshot(&snapshot, fixed_now()).unwrap();
        // 5-hour rolling window: 98 remaining → 2% used, 300 minutes.
        assert!((usage.primary.used_percent - 2.0).abs() < 0.01);
        assert_eq!(usage.primary.window_minutes, Some(300));
        // Status-3 weekly lane → unlimited secondary.
        let secondary = usage.secondary.expect("weekly lane");
        assert_eq!(secondary.used_percent, 0.0);
        assert_eq!(secondary.reset_description.as_deref(), Some("Unlimited"));
    }

    #[test]
    fn assembles_full_token_plan_result() {
        let snapshot =
            coding_plan::parse_coding_plan_value(&reporter_token_plan_usage_json(), fixed_now())
                .unwrap();
        let summary = parse_token_plan_summary(&reporter_usage_summary_json()).unwrap();

        let result = assemble_token_plan_result(
            Some(snapshot),
            Some(summary),
            Some("TokenPlanPlus annual membership".to_string()),
            ProviderError::Other("original".into()),
            fixed_now(),
        )
        .unwrap();

        assert_eq!(result.source_label, "web");
        assert!((result.usage.primary.used_percent - 2.0).abs() < 0.01);
        assert_eq!(
            result.usage.login_method.as_deref(),
            Some("TokenPlanPlus annual membership")
        );
        let row = result
            .usage
            .extra_rate_windows
            .iter()
            .find(|w| w.id == "token-plan-summary")
            .expect("token-plan-summary row attached");
        assert_eq!(row.title, "Token Plan");
        assert!(row.window.is_informational);
        assert_eq!(
            row.window.reset_description.as_deref(),
            Some("123,456 tokens · 5 / 30 days active")
        );
    }

    #[test]
    fn assembles_summary_only_token_plan_result() {
        let summary = parse_token_plan_summary(&reporter_usage_summary_json()).unwrap();
        let result = assemble_token_plan_result(
            None,
            Some(summary),
            Some("TokenPlanPlus annual membership".to_string()),
            ProviderError::Other("original".into()),
            fixed_now(),
        )
        .unwrap();

        assert!(result.usage.primary.is_informational);
        assert_eq!(
            result.usage.primary.reset_description.as_deref(),
            Some("123,456 tokens · 5 / 30 days active")
        );
        assert_eq!(
            result.usage.login_method.as_deref(),
            Some("TokenPlanPlus annual membership")
        );
    }

    #[test]
    fn parsed_but_rowless_usage_snapshot_still_uses_summary() {
        // (a) parsed fine but carries no quota rows → (b) summary wins.
        let snapshot = coding_plan::MiniMaxCodingPlanSnapshot::Services(vec![]);
        let summary = parse_token_plan_summary(&reporter_usage_summary_json()).unwrap();
        let result = assemble_token_plan_result(
            Some(snapshot),
            Some(summary),
            None,
            ProviderError::Other("original".into()),
            fixed_now(),
        )
        .unwrap();
        assert!(result.usage.primary.is_informational);
    }

    #[test]
    fn all_absent_falls_back_to_original_coding_plan_error() {
        let original = ProviderError::Other("no active token plan subscription".into());
        let err = assemble_token_plan_result(None, None, None, original, fixed_now()).unwrap_err();
        match err {
            ProviderError::Other(msg) => {
                assert_eq!(msg, "no active token plan subscription")
            }
            other => panic!("expected original error unchanged, got {other:?}"),
        }
    }
}
