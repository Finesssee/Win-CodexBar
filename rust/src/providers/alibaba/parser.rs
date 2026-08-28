use chrono::{DateTime, TimeZone, Utc};

use crate::core::{ProviderError, RateWindow, UsageSnapshot};

pub(crate) fn parse_response(json: &serde_json::Value) -> Result<UsageSnapshot, ProviderError> {
    let top_code = json.get("code").and_then(|v| v.as_str()).unwrap_or("");
    if !top_code.is_empty() && top_code != "200" {
        if top_code == "401" || top_code == "403" {
            return Err(ProviderError::AuthRequired);
        }
        let msg = json
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or(top_code);
        return Err(ProviderError::Other(format!("API error: {msg}")));
    }

    if let Some(ret) = json.pointer("/data/DataV2/ret").and_then(|v| v.as_array()) {
        let joined: String = ret
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(";");
        if joined.contains("No Authority")
            || joined.contains("10032390")
            || joined.contains("NeedLogin")
        {
            return Err(ProviderError::AuthRequired);
        }
    }

    let instances = json
        .pointer("/data/DataV2/data/data/codingPlanInstanceInfos")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            ProviderError::Parse("codingPlanInstanceInfos not found in response".into())
        })?;

    let instance = instances
        .iter()
        .find(|i| i.get("status").and_then(|s| s.as_str()) == Some("VALID"))
        .or_else(|| instances.first())
        .ok_or_else(|| ProviderError::Parse("no Coding Plan instance in response".into()))?;

    let quota = instance
        .get("codingPlanQuotaInfo")
        .ok_or_else(|| ProviderError::Parse("codingPlanQuotaInfo missing".into()))?;

    let plan_name = instance
        .get("instanceName")
        .and_then(|v| v.as_str())
        .unwrap_or("Coding Plan");

    let ms_to_dt = |key: &str| -> Option<DateTime<Utc>> {
        quota
            .get(key)
            .and_then(|v| v.as_i64())
            .and_then(|ms| Utc.timestamp_opt(ms / 1000, 0).single())
    };

    let pct = |used_key: &str, total_key: &str| -> f64 {
        let used = quota.get(used_key).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let total = quota.get(total_key).and_then(|v| v.as_f64()).unwrap_or(1.0);
        if total > 0.0 {
            (used / total * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        }
    };

    let detail = |used_key: &str, total_key: &str| -> Option<String> {
        let used = quota.get(used_key).and_then(|v| v.as_f64())?;
        let total = quota.get(total_key).and_then(|v| v.as_f64())?;
        // Token counts are whole numbers; the fractional part is rounding
        // noise from JSON float parsing.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "token counts are whole numbers; fractional part is rounding noise"
        )]
        let used_tokens = used as i64;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "token counts are whole numbers; fractional part is rounding noise"
        )]
        let total_tokens = total as i64;
        Some(format!(
            "{} / {} tokens",
            fmt_tokens(used_tokens),
            fmt_tokens(total_tokens)
        ))
    };

    let five_hour = RateWindow::with_details(
        pct("per5HourUsedQuota", "per5HourTotalQuota"),
        Some(300),
        ms_to_dt("per5HourQuotaNextRefreshTime"),
        detail("per5HourUsedQuota", "per5HourTotalQuota"),
    );
    let weekly = RateWindow::with_details(
        pct("perWeekUsedQuota", "perWeekTotalQuota"),
        Some(7 * 24 * 60),
        ms_to_dt("perWeekQuotaNextRefreshTime"),
        detail("perWeekUsedQuota", "perWeekTotalQuota"),
    );
    let monthly_reset = ms_to_dt("perBillMonthQuotaNextRefreshTime");
    let monthly = RateWindow::with_details(
        pct("perBillMonthUsedQuota", "perBillMonthTotalQuota"),
        RateWindow::monthly_window_minutes(monthly_reset).or(Some(30 * 24 * 60)),
        monthly_reset,
        detail("perBillMonthUsedQuota", "perBillMonthTotalQuota"),
    );

    Ok(UsageSnapshot::new(five_hour)
        .with_secondary(weekly)
        .with_tertiary(monthly)
        .with_login_method(plan_name))
}

fn fmt_tokens(n: i64) -> String {
    let negative = n < 0;
    let magnitude = (n as i128).unsigned_abs();
    let digits = magnitude.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    for (i, ch) in digits.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    let mut result: String = out.chars().rev().collect();
    if negative {
        result.insert(0, '-');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_response() -> serde_json::Value {
        serde_json::json!({
            "code": "200",
            "data": {
                "DataV2": {
                    "data": {
                        "data": {
                            "codingPlanInstanceInfos": [{
                                "instanceName": "Coding Plan Pro",
                                "status": "VALID",
                                "codingPlanQuotaInfo": {
                                    "per5HourUsedQuota": 0,
                                    "per5HourTotalQuota": 6000,
                                    "per5HourQuotaNextRefreshTime": 1780731422000_i64,
                                    "perWeekUsedQuota": 2019,
                                    "perWeekTotalQuota": 45000,
                                    "perWeekQuotaNextRefreshTime": 1780848000000_i64,
                                    "perBillMonthUsedQuota": 25,
                                    "perBillMonthTotalQuota": 90000,
                                    "perBillMonthQuotaNextRefreshTime": 1783267200000_i64
                                }
                            }]
                        }
                    }
                }
            }
        })
    }

    #[test]
    fn parses_real_response_shape() {
        let usage = parse_response(&sample_response()).unwrap();

        assert!((usage.primary.used_percent - 0.0).abs() < 0.01);
        assert_eq!(usage.primary.window_minutes, Some(300));
        assert!(usage.primary.resets_at.is_some());

        let weekly = usage.secondary.unwrap();
        assert!((weekly.used_percent - 4.487).abs() < 0.01);

        let monthly = usage.tertiary.unwrap();
        assert!((monthly.used_percent - 0.028).abs() < 0.01);
        // perBillMonthQuotaNextRefreshTime 1783267200000 = 2026-07-05 → 30-day cycle.
        assert_eq!(monthly.window_minutes, Some(30 * 24 * 60));

        assert_eq!(usage.login_method.as_deref(), Some("Coding Plan Pro"));
    }

    #[test]
    fn picks_valid_instance_over_first() {
        let json = serde_json::json!({
            "code": "200",
            "data": { "DataV2": { "data": { "data": {
                "codingPlanInstanceInfos": [
                    {
                        "instanceName": "Expired",
                        "status": "EXPIRED",
                        "codingPlanQuotaInfo": {
                            "per5HourUsedQuota": 100, "per5HourTotalQuota": 100,
                            "perWeekUsedQuota": 100, "perWeekTotalQuota": 100,
                            "perBillMonthUsedQuota": 100, "perBillMonthTotalQuota": 100
                        }
                    },
                    {
                        "instanceName": "Coding Plan Pro",
                        "status": "VALID",
                        "codingPlanQuotaInfo": {
                            "per5HourUsedQuota": 0, "per5HourTotalQuota": 6000,
                            "perWeekUsedQuota": 10, "perWeekTotalQuota": 45000,
                            "perBillMonthUsedQuota": 25, "perBillMonthTotalQuota": 90000
                        }
                    }
                ]
            }}}}
        });
        let usage = parse_response(&json).unwrap();
        assert_eq!(usage.login_method.as_deref(), Some("Coding Plan Pro"));
        assert!(usage.primary.used_percent < 1.0);
    }

    #[test]
    fn no_authority_response_maps_to_auth_required() {
        let json = serde_json::json!({
            "code": "200",
            "data": { "DataV2": { "ret": ["10032390::No Authority"], "data": {} } }
        });
        assert!(matches!(
            parse_response(&json),
            Err(ProviderError::AuthRequired)
        ));
    }

    #[test]
    fn fmt_tokens_formats_correctly() {
        assert_eq!(fmt_tokens(6000), "6,000");
        assert_eq!(fmt_tokens(90000), "90,000");
        assert_eq!(fmt_tokens(25), "25");
        assert_eq!(fmt_tokens(1000000), "1,000,000");
    }

    #[test]
    fn fmt_tokens_handles_negative_values() {
        assert_eq!(fmt_tokens(-100), "-100");
        assert_eq!(fmt_tokens(-1000), "-1,000");
        assert_eq!(fmt_tokens(-1000000), "-1,000,000");
        assert_eq!(fmt_tokens(-1), "-1");
        assert_eq!(fmt_tokens(0), "0");
        assert_eq!(fmt_tokens(i64::MIN), "-9,223,372,036,854,775,808");
    }
}
