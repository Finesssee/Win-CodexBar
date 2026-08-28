//! Cursor API client for fetching usage information
//!
//! Uses browser cookies to authenticate with cursor.com API

use crate::core::{CostSnapshot, NamedRateWindow, ProviderError, RateWindow};
use crate::providers::browser_cookie_header;
use chrono::{DateTime, Utc};
use serde::Deserialize;

const BASE_URL: &str = "https://cursor.com";
const COOKIE_DOMAINS: [&str; 2] = ["cursor.com", "cursor.sh"];

#[derive(Debug)]
pub struct CursorUsageResult {
    pub(super) primary: RateWindow,
    pub(super) secondary: Option<RateWindow>,
    pub(super) model_specific: Option<RateWindow>,
    pub(super) cost: Option<CostSnapshot>,
    pub(super) email: Option<String>,
    pub(super) plan_type: Option<String>,
    pub(super) grok_bot: Option<NamedRateWindow>,
}

/// Cursor API client
pub struct CursorApi {
    client: reqwest::Client,
}

impl CursorApi {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    pub(super) fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Fetch usage information from Cursor API
    pub async fn fetch_usage(&self) -> Result<CursorUsageResult, ProviderError> {
        // Try to get cookies from browser
        let cookie_header = self.get_cookie_header()?;
        self.fetch_usage_with_cookie_header(&cookie_header).await
    }

    /// Fetch usage information with an already resolved Cookie header.
    pub async fn fetch_usage_with_cookie_header(
        &self,
        cookie_header: &str,
    ) -> Result<CursorUsageResult, ProviderError> {
        // Fetch usage summary and user info in parallel
        let (usage_result, user_result, sand_result) = tokio::join!(
            self.fetch_usage_summary(cookie_header),
            self.fetch_user_info(cookie_header),
            self.fetch_sand_usage(cookie_header)
        );

        let usage_summary = usage_result?;
        let user_info = user_result.ok();
        let mut result = self.build_result(usage_summary, user_info)?;
        result.grok_bot = sand_result.ok().flatten();
        Ok(result)
    }

    fn get_cookie_header(&self) -> Result<String, ProviderError> {
        browser_cookie_header(&COOKIE_DOMAINS)
    }

    async fn fetch_usage_summary(
        &self,
        cookie_header: &str,
    ) -> Result<UsageSummary, ProviderError> {
        let url = format!("{}/api/usage-summary", BASE_URL);

        let response = self
            .client
            .get(&url)
            .header("Cookie", cookie_header)
            .header("Accept", "application/json")
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await?;

        if response.status() == 401 || response.status() == 403 {
            return Err(ProviderError::AuthRequired);
        }

        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Cursor API returned {}",
                response.status()
            )));
        }

        // Try structured deserialization first, fall back to raw JSON on failure
        let text = response
            .text()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        serde_json::from_str::<UsageSummary>(&text).map_err(|e| {
            tracing::warn!(
                "Cursor usage-summary parse error: {e}; response length: {} bytes",
                text.len()
            );
            ProviderError::Parse(e.to_string())
        })
    }

    async fn fetch_sand_usage(
        &self,
        cookie_header: &str,
    ) -> Result<Option<NamedRateWindow>, ProviderError> {
        let url = format!("{}/api/dashboard/get-sand-usage-status", BASE_URL);
        let response = self
            .client
            .post(&url)
            .header("Cookie", cookie_header)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("Origin", BASE_URL)
            .body("{}")
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await?;
        if !response.status().is_success() {
            return Ok(None);
        }
        let status: SandUsageStatus = response
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        Ok(status.to_window())
    }

    async fn fetch_user_info(&self, cookie_header: &str) -> Result<UserInfo, ProviderError> {
        let url = format!("{}/api/auth/me", BASE_URL);

        let response = self
            .client
            .get(&url)
            .header("Cookie", cookie_header)
            .header("Accept", "application/json")
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(ProviderError::Other(
                "Failed to fetch user info".to_string(),
            ));
        }

        response
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))
    }

    fn build_result(
        &self,
        summary: UsageSummary,
        user_info: Option<UserInfo>,
    ) -> Result<CursorUsageResult, ProviderError> {
        let billing_end = summary
            .billing_cycle_end
            .as_ref()
            .and_then(|s| parse_iso_date(s));

        let (percent_used, secondary, model_specific, cost_snapshot) =
            if let Some(individual) = &summary.individual_usage {
                if let Some(plan) = &individual.plan {
                    let used_cents = plan.used.unwrap_or(0) as f64;
                    let limit_cents = plan
                        .limit
                        .or_else(|| plan.breakdown.as_ref().and_then(|b| b.total))
                        .unwrap_or(0) as f64;

                    // Upstream #2255: clamp plan usage at 100% when included usage
                    // exceeds the plan limit (overage must not paint >100% bars).
                    let percent = if let Some(percent) = plan.total_percent_used {
                        clamp_percent(percent)
                    } else if limit_cents > 0.0 {
                        clamp_percent((used_cents / limit_cents) * 100.0)
                    } else {
                        0.0
                    };

                    let secondary = plan.auto_percent_used.map(|v| {
                        RateWindow::with_details(clamp_percent(v), None, billing_end, None)
                    });

                    let model_specific = plan.api_percent_used.map(|v| {
                        RateWindow::with_details(clamp_percent(v), None, billing_end, None)
                    });

                    let cost = Self::on_demand_cost(individual.on_demand.as_ref(), billing_end)
                        .or_else(|| {
                            summary.team_usage.as_ref().and_then(|team| {
                                Self::on_demand_cost(team.on_demand.as_ref(), billing_end)
                            })
                        })
                        .unwrap_or_else(|| {
                            // Plan-included spend (cents → USD) when on-demand is off.
                            let mut cost = CostSnapshot::new(
                                used_cents / 100.0,
                                "USD",
                                plan_period_label(summary.billing_cycle_start.as_deref()),
                            );
                            if limit_cents > 0.0 {
                                cost = cost.with_limit(limit_cents / 100.0);
                            }
                            if let Some(reset) = billing_end {
                                cost = cost.with_resets_at(reset);
                            }
                            cost
                        });

                    (percent, secondary, model_specific, Some(cost))
                } else if let Some(overall) = &individual.overall {
                    let percent = Self::usage_percent(overall).unwrap_or(0.0);
                    let cost = Self::on_demand_cost(Some(overall), billing_end);
                    (percent, None, None, cost)
                } else {
                    (0.0, None, None, None)
                }
            } else if let Some(team) = &summary.team_usage {
                if let Some(pooled) = &team.pooled {
                    let percent = Self::usage_percent(pooled).unwrap_or(0.0);
                    let cost = Self::on_demand_cost(Some(pooled), billing_end);
                    (percent, None, None, cost)
                } else {
                    (0.0, None, None, None)
                }
            } else {
                (0.0, None, None, None)
            };

        let primary = RateWindow::with_details(percent_used, None, billing_end, None);

        let plan_type = summary
            .membership_type
            .as_ref()
            .map(|t| match t.to_lowercase().as_str() {
                "enterprise" => "Cursor Enterprise".to_string(),
                "pro" => "Cursor Pro".to_string(),
                "hobby" => "Cursor Hobby".to_string(),
                "team" => "Cursor Team".to_string(),
                other => format!("Cursor {}", capitalize(other)),
            });

        let email = user_info.as_ref().and_then(|u| u.email.clone());

        Ok(CursorUsageResult {
            primary,
            secondary,
            model_specific,
            cost: cost_snapshot,
            email,
            plan_type,
            grok_bot: None,
        })
    }

    fn on_demand_cost(
        on_demand: Option<&OnDemandUsage>,
        billing_end: Option<DateTime<Utc>>,
    ) -> Option<CostSnapshot> {
        let usage = on_demand?;
        if usage.enabled == Some(false) {
            return None;
        }

        let used_cents = usage.used.unwrap_or(0) as f64;
        let limit_cents = usage
            .limit
            .or_else(|| {
                usage
                    .remaining
                    .map(|remaining| remaining + usage.used.unwrap_or(0))
            })
            .unwrap_or(0) as f64;

        if used_cents <= 0.0 && limit_cents <= 0.0 {
            return None;
        }

        // usage-summary exposes on-demand spend in cents for the billing cycle.
        // Label it explicitly so the tray/detail cost line is not a vague "Monthly".
        let mut cost = CostSnapshot::new(used_cents / 100.0, "USD", "On-demand (billing cycle)");
        if limit_cents > 0.0 {
            cost = cost.with_limit(limit_cents / 100.0);
        }
        if let Some(reset) = billing_end {
            cost = cost.with_resets_at(reset);
        }
        Some(cost)
    }

    fn usage_percent(usage: &OnDemandUsage) -> Option<f64> {
        let used = usage.used.unwrap_or(0) as f64;
        let limit = usage
            .limit
            .or_else(|| {
                usage
                    .remaining
                    .map(|remaining| remaining + usage.used.unwrap_or(0))
            })
            .unwrap_or(0) as f64;
        (limit > 0.0).then_some(clamp_percent(used / limit * 100.0))
    }
}

fn clamp_percent(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    value.clamp(0.0, 100.0)
}

/// Period label for plan-included spend from usage-summary (no new network calls).
fn plan_period_label(billing_cycle_start: Option<&str>) -> String {
    // Upstream 0.50.1 #2951: match the Cursor dashboard's name for the
    // included-usage pool (Cursor + third-party models).
    match billing_cycle_start {
        Some(start) if !start.is_empty() => format!("Cursor and Third Party (since {start})"),
        _ => "Cursor and Third Party (billing cycle)".to_string(),
    }
}
impl Default for CursorApi {
    fn default() -> Self {
        Self::new()
    }
}

// --- API Response Types ---

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageSummary {
    billing_cycle_start: Option<String>,
    billing_cycle_end: Option<String>,
    membership_type: Option<String>,
    limit_type: Option<String>,
    is_unlimited: Option<bool>,
    individual_usage: Option<IndividualUsage>,
    team_usage: Option<TeamUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndividualUsage {
    plan: Option<PlanUsage>,
    on_demand: Option<OnDemandUsage>,
    overall: Option<OnDemandUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanUsage {
    enabled: Option<bool>,
    used: Option<i64>,
    limit: Option<i64>,
    remaining: Option<i64>,
    breakdown: Option<PlanBreakdown>,
    auto_percent_used: Option<f64>,
    api_percent_used: Option<f64>,
    total_percent_used: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanBreakdown {
    included: Option<i64>,
    bonus: Option<i64>,
    total: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OnDemandUsage {
    enabled: Option<bool>,
    used: Option<i64>,
    limit: Option<i64>,
    remaining: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TeamUsage {
    on_demand: Option<OnDemandUsage>,
    pooled: Option<OnDemandUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SandUsageStatus {
    current_period_start: Option<String>,
    next_reset_timestamp_utc: Option<String>,
    usage_percent: Option<f64>,
    has_non_zero_included_limit: Option<bool>,
}

impl SandUsageStatus {
    fn to_window(&self) -> Option<NamedRateWindow> {
        if self.has_non_zero_included_limit != Some(true) {
            return None;
        }
        let used = clamp_percent(self.usage_percent?);
        let start = self
            .current_period_start
            .as_deref()
            .and_then(parse_iso_date);
        let reset = self
            .next_reset_timestamp_utc
            .as_deref()
            .and_then(parse_iso_date);
        let minutes = start.zip(reset).and_then(|(start, reset)| {
            let minutes = (reset - start).num_minutes();
            // Guarded by the minutes > 0 check; rate windows are far
            // shorter than u32::MAX minutes.
            #[allow(
                clippy::cast_possible_truncation,
                reason = "guarded by the minutes > 0 check; rate windows are far shorter than u32::MAX minutes"
            )]
            let minutes_u32 = minutes as u32;
            (minutes > 0).then_some(minutes_u32)
        });
        Some(NamedRateWindow::new(
            "cursor-grok-bot",
            "Grok Bot",
            RateWindow::with_details(used, minutes, reset, None),
        ))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserInfo {
    email: Option<String>,
    email_verified: Option<bool>,
    name: Option<String>,
    sub: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    picture: Option<String>,
}

// --- Helper functions ---

fn parse_iso_date(s: &str) -> Option<DateTime<Utc>> {
    // Try with fractional seconds
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }

    // Try without fractional seconds
    if let Ok(dt) = chrono::DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%SZ") {
        return Some(dt.with_timezone(&Utc));
    }

    None
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api() -> CursorApi {
        CursorApi::new()
    }

    fn parse_summary(json: &str) -> UsageSummary {
        serde_json::from_str(json).expect("fixture should parse")
    }

    #[test]
    fn sand_usage_maps_to_weekly_extra_window() {
        let status = SandUsageStatus {
            current_period_start: Some("2026-08-18T00:00:00Z".into()),
            next_reset_timestamp_utc: Some("2026-08-25T00:00:00Z".into()),
            usage_percent: Some(37.5),
            has_non_zero_included_limit: Some(true),
        };
        let row = status.to_window().expect("grok bot window");
        assert_eq!(row.id, "cursor-grok-bot");
        assert_eq!(row.title, "Grok Bot");
        assert!((row.window.used_percent - 37.5).abs() < 0.001);
        assert_eq!(row.window.window_minutes, Some(10080));
    }

    #[test]
    fn sand_usage_hides_accounts_without_included_allowance() {
        let status = SandUsageStatus {
            current_period_start: None,
            next_reset_timestamp_utc: None,
            usage_percent: Some(0.0),
            has_non_zero_included_limit: Some(false),
        };
        assert!(status.to_window().is_none());
    }

    #[test]
    fn test_cursor_build_result_with_lanes() {
        let json = r#"{
            "billingCycleStart": "2026-03-01T00:00:00Z",
            "billingCycleEnd": "2026-04-01T00:00:00Z",
            "membershipType": "pro",
            "individualUsage": {
                "plan": {
                    "used": 1500,
                    "limit": 5000,
                    "totalPercentUsed": 30.0,
                    "autoPercentUsed": 20.0,
                    "apiPercentUsed": 10.0
                }
            }
        }"#;

        let summary = parse_summary(json);
        let result = api().build_result(summary, None).unwrap();

        assert!((result.primary.used_percent - 30.0).abs() < 0.01);

        let sec = result.secondary.expect("secondary should be present");
        assert!((sec.used_percent - 20.0).abs() < 0.01);
        assert!(sec.resets_at.is_some());

        let ms = result
            .model_specific
            .expect("model_specific should be present");
        assert!((ms.used_percent - 10.0).abs() < 0.01);
        assert!(ms.resets_at.is_some());

        assert!(result.cost.is_some());
        assert_eq!(result.plan_type.as_deref(), Some("Cursor Pro"));
    }

    #[test]
    fn clamps_plan_usage_percent_at_100_when_over_limit() {
        // Upstream #2255: included usage past limit must not paint >100%.
        let json = r#"{
            "membershipType": "pro",
            "individualUsage": {
                "plan": {
                    "used": 6000,
                    "limit": 5000,
                    "totalPercentUsed": 120.0,
                    "autoPercentUsed": 110.0,
                    "apiPercentUsed": 105.0
                }
            }
        }"#;
        let summary = parse_summary(json);
        let result = api().build_result(summary, None).unwrap();
        assert!((result.primary.used_percent - 100.0).abs() < 0.01);
        assert!((result.secondary.unwrap().used_percent - 100.0).abs() < 0.01);
        assert!((result.model_specific.unwrap().used_percent - 100.0).abs() < 0.01);
    }

    #[test]
    fn test_cursor_build_result_prefers_api_percent_fields() {
        let json = r#"{
            "membershipType": "pro",
            "autoModelSelectedDisplayMessage": "You've used 13% of your included total usage",
            "individualUsage": {
                "plan": {
                    "used": 2000,
                    "limit": 2000,
                    "breakdown": {
                        "included": 2000,
                        "bonus": 580,
                        "total": 2580
                    },
                    "autoPercentUsed": 17.2,
                    "apiPercentUsed": 0,
                    "totalPercentUsed": 13.230769230769232
                }
            }
        }"#;

        let summary = parse_summary(json);
        let result = api().build_result(summary, None).unwrap();

        assert!((result.primary.used_percent - 13.230769230769232).abs() < 0.01);
        assert!((result.secondary.unwrap().used_percent - 17.2).abs() < 0.01);
        assert!((result.model_specific.unwrap().used_percent - 0.0).abs() < 0.01);

        let cost = result
            .cost
            .expect("plan usage should still produce cost snapshot");
        assert!((cost.used - 20.0).abs() < 0.01);
        assert_eq!(cost.limit, Some(20.0));
        assert_eq!(result.plan_type.as_deref(), Some("Cursor Pro"));
    }

    #[test]
    fn test_cursor_build_result_cents_only() {
        let json = r#"{
            "billingCycleEnd": "2026-04-01T00:00:00Z",
            "membershipType": "pro",
            "individualUsage": {
                "plan": {
                    "used": 2500,
                    "limit": 5000
                }
            }
        }"#;

        let summary = parse_summary(json);
        let result = api().build_result(summary, None).unwrap();

        assert!((result.primary.used_percent - 50.0).abs() < 0.01);
        assert!(result.secondary.is_none(), "no autoPercentUsed in payload");
        assert!(
            result.model_specific.is_none(),
            "no apiPercentUsed in payload"
        );
        assert!(result.cost.is_some());
    }

    #[test]
    fn test_cursor_build_result_missing_plan() {
        let json = r#"{
            "membershipType": "hobby",
            "individualUsage": {}
        }"#;

        let summary = parse_summary(json);
        let result = api().build_result(summary, None).unwrap();

        assert!((result.primary.used_percent).abs() < 0.01);
        assert!(result.secondary.is_none());
        assert!(result.model_specific.is_none());
        assert!(result.cost.is_none());
    }

    #[test]
    fn test_cursor_on_demand_as_cost() {
        let json = r#"{
            "billingCycleEnd": "2026-04-01T00:00:00Z",
            "membershipType": "pro",
            "individualUsage": {
                "plan": {
                    "used": 800,
                    "limit": 5000,
                    "totalPercentUsed": 16.0
                },
                "onDemand": {
                    "enabled": true,
                    "used": 350,
                    "limit": 1000
                }
            }
        }"#;

        let summary = parse_summary(json);
        let result = api().build_result(summary, None).unwrap();

        assert!((result.primary.used_percent - 16.0).abs() < 0.01);
        let cost = result.cost.expect("cost should exist from on-demand usage");
        assert!((cost.used - 3.5).abs() < 0.01);
        assert_eq!(cost.limit, Some(10.0));
        assert_eq!(cost.period, "On-demand (billing cycle)");
    }

    #[test]
    fn plan_cost_period_uses_billing_cycle_start() {
        let json = r#"{
            "billingCycleStart": "2026-03-01T00:00:00Z",
            "billingCycleEnd": "2026-04-01T00:00:00Z",
            "membershipType": "pro",
            "individualUsage": {
                "plan": {
                    "used": 2500,
                    "limit": 5000
                }
            }
        }"#;
        let summary = parse_summary(json);
        let result = api().build_result(summary, None).unwrap();
        let cost = result.cost.expect("plan cost");
        assert!((cost.used - 25.0).abs() < 0.01);
        assert_eq!(cost.limit, Some(50.0));
        assert_eq!(
            cost.period,
            "Cursor and Third Party (since 2026-03-01T00:00:00Z)"
        );
    }

    #[test]
    fn test_cursor_individual_overall_fallback() {
        let summary =
            parse_summary(r#"{"individualUsage":{"overall":{"used":2500,"limit":10000}}}"#);
        let result = api().build_result(summary, None).unwrap();
        assert!((result.primary.used_percent - 25.0).abs() < 0.01);
        assert_eq!(result.cost.unwrap().limit, Some(100.0));
    }

    #[test]
    fn test_cursor_team_pooled_fallback() {
        let summary = parse_summary(r#"{"teamUsage":{"pooled":{"used":5000,"limit":10000}}}"#);
        let result = api().build_result(summary, None).unwrap();
        assert!((result.primary.used_percent - 50.0).abs() < 0.01);
        assert_eq!(result.cost.unwrap().used, 50.0);
    }
}
