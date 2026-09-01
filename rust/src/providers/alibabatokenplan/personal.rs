//! Alibaba Token Plan Personal/Solo OneConsole path (upstream 0.46.0).

use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::region::AlibabaTokenPlanRegion;
use super::{
    LANGUAGE, PERSONAL_CONSOLE_PRODUCT, PERSONAL_QUOTA_CONFIG_API, PERSONAL_SUBSCRIPTION_API,
    PERSONAL_USAGE_API, TokenPlanSnapshot, USER_AGENT, cookie_value, date_field,
    expand_json_strings, find_object_containing_any_of, is_likely_login_html, number_field,
    percentage_points, throw_if_error_payload,
};
use crate::core::{FetchContext, ProviderError};

struct PersonalApiContext<'a> {
    client: &'a reqwest::Client,
    cookie_header: &'a str,
    region: AlibabaTokenPlanRegion,
    sec_token: Option<&'a str>,
    fetch_context: &'a FetchContext,
}

pub(super) async fn fetch_personal_usage(
    client: &reqwest::Client,
    cookie_header: &str,
    region: AlibabaTokenPlanRegion,
    sec_token: Option<&str>,
    fetch_context: &FetchContext,
) -> Result<TokenPlanSnapshot, ProviderError> {
    let context = PersonalApiContext {
        client,
        cookie_header,
        region,
        sec_token,
        fetch_context,
    };
    let mut subscription_params = Map::new();
    subscription_params.insert(
        "commodityCode".into(),
        Value::String(region.product_code().to_string()),
    );
    let subscription_body =
        post_personal_api_optional(&context, PERSONAL_SUBSCRIPTION_API, subscription_params).await;

    let quota_config_body =
        post_personal_api_optional(&context, PERSONAL_QUOTA_CONFIG_API, Map::new()).await;

    const MAX_USAGE_ATTEMPTS: usize = 3;
    const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(400);
    for attempt in 0..MAX_USAGE_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(RETRY_DELAY).await;
        }
        let usage_body = post_personal_api(&context, PERSONAL_USAGE_API, Map::new()).await?;
        match parse_personal_usage(
            &usage_body,
            subscription_body.as_deref(),
            quota_config_body.as_deref(),
        ) {
            Ok(snapshot) => return Ok(snapshot),
            Err(error) if personal_usage_success_without_windows(&usage_body) => {
                tracing::info!(
                    attempt = attempt + 1,
                    max_attempts = MAX_USAGE_ATTEMPTS,
                    "Alibaba Token Plan Personal usage returned no windows; retrying"
                );
                if attempt + 1 == MAX_USAGE_ATTEMPTS {
                    return Err(ProviderError::Other(
                        "Alibaba Token Plan usage is temporarily unavailable; it will refresh automatically."
                            .into(),
                    ));
                }
                let _ = error;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("bounded usage retry loop always returns")
}

fn personal_usage_success_without_windows(data: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(data) else {
        return false;
    };
    let expanded = expand_json_strings(value);
    let has_windows =
        find_object_containing_any_of(&expanded, &["per5HourPercentage", "per1WeekPercentage"])
            .is_some();
    if has_windows {
        return false;
    }
    let code_success = expanded
        .get("code")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|code| code.eq_ignore_ascii_case("SUCCESS") || code == "200");
    let response_success = expanded
        .get("successResponse")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let error_empty = expanded
        .get("errorCode")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_none_or(str::is_empty);
    code_success && response_success && error_empty
}

async fn post_personal_api(
    context: &PersonalApiContext<'_>,
    api: &str,
    data_parameters: Map<String, Value>,
) -> Result<Vec<u8>, ProviderError> {
    let url = personal_api_url(api, context.region);
    let form = build_personal_form(
        api,
        data_parameters,
        context.cookie_header,
        context.region,
        context.sec_token,
    );

    let mut request = context
        .client
        .post(&url)
        .timeout(std::time::Duration::from_secs(
            context.fetch_context.web_timeout.max(1),
        ))
        .header("Cookie", context.cookie_header)
        .header("Accept", "application/json, text/plain, */*")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Origin", context.region.gateway_base_url())
        .header("Referer", context.region.dashboard_url())
        .header("User-Agent", USER_AGENT)
        .header("X-Requested-With", "XMLHttpRequest")
        .form(&form);

    if let Some(csrf) = cookie_value("login_aliyunid_csrf", context.cookie_header)
        .or_else(|| cookie_value("csrf", context.cookie_header))
    {
        request = request
            .header("x-xsrf-token", csrf.clone())
            .header("x-csrf-token", csrf);
    }

    let response = request.send().await?;
    let status = response.status();
    let body = response.bytes().await?;
    if !status.is_success() {
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ProviderError::AuthRequired);
        }
        return Err(ProviderError::Other(format!(
            "Alibaba Token Plan Personal API error: HTTP {status}"
        )));
    }
    Ok(body.to_vec())
}

async fn post_personal_api_optional(
    context: &PersonalApiContext<'_>,
    api: &str,
    data_parameters: Map<String, Value>,
) -> Option<Vec<u8>> {
    post_personal_api(context, api, data_parameters).await.ok()
}

fn build_personal_form(
    api: &str,
    data_parameters: Map<String, Value>,
    cookie_header: &str,
    region: AlibabaTokenPlanRegion,
    sec_token: Option<&str>,
) -> Vec<(&'static str, String)> {
    let params_json = build_personal_params_json(api, data_parameters, cookie_header, region);
    let mut form = vec![
        ("product", PERSONAL_CONSOLE_PRODUCT.to_string()),
        ("action", region.personal_api_action().to_string()),
        ("region", region.current_region_id().to_string()),
        ("language", LANGUAGE.to_string()),
        ("params", params_json),
    ];
    if let Some(token) = sec_token.filter(|token| !token.trim().is_empty()) {
        form.push(("sec_token", token.to_string()));
    }
    form
}

fn personal_api_url(api: &str, region: AlibabaTokenPlanRegion) -> String {
    format!(
        "{}/data/api.json?action={}&product={}&api={api}&_v=undefined",
        region.quota_base_url(),
        region.personal_api_action(),
        PERSONAL_CONSOLE_PRODUCT,
    )
}

fn build_personal_params_json(
    api: &str,
    mut data_parameters: Map<String, Value>,
    cookie_header: &str,
    region: AlibabaTokenPlanRegion,
) -> String {
    let dashboard = region.dashboard_url();
    let domain = dashboard
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or_default();

    let mut cornerstone = Map::new();
    cornerstone.insert(
        "feTraceId".into(),
        Value::String(Uuid::new_v4().to_string().to_lowercase()),
    );
    cornerstone.insert("feURL".into(), Value::String(dashboard.to_string()));
    cornerstone.insert("protocol".into(), Value::String("V2".into()));
    cornerstone.insert("console".into(), Value::String("ONE_CONSOLE".into()));
    cornerstone.insert("productCode".into(), Value::String("p_efm".into()));
    // Let the gateway resolve the Personal/Solo session workspace. A captured
    // Teams switchAgent is workspace-bound and rejects other accounts.
    cornerstone.insert("switchUserType".into(), json!(3));
    cornerstone.insert("domain".into(), Value::String(domain.to_string()));
    cornerstone.insert(
        "consoleSite".into(),
        Value::String(region.personal_console_site().to_string()),
    );
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

pub(super) fn parse_personal_usage(
    usage_data: &[u8],
    subscription_data: Option<&[u8]>,
    quota_config_data: Option<&[u8]>,
) -> Result<TokenPlanSnapshot, ProviderError> {
    if usage_data.is_empty() {
        return Err(ProviderError::Parse(
            "Empty Alibaba Token Plan Personal response".into(),
        ));
    }

    let value: Value = serde_json::from_slice(usage_data).map_err(|_| {
        if is_likely_login_html(usage_data) {
            ProviderError::AuthRequired
        } else {
            ProviderError::Parse("Invalid Alibaba Token Plan Personal JSON response".into())
        }
    })?;
    let expanded = expand_json_strings(value);
    throw_if_error_payload(&expanded)?;

    let usage =
        find_object_containing_any_of(&expanded, &["per5HourPercentage", "per1WeekPercentage"])
            .ok_or_else(|| {
                ProviderError::Parse("Missing Alibaba Token Plan Personal usage windows".into())
            })?;

    let five_hour = percentage_points(number_field(&usage, "per5HourPercentage"));
    let weekly = percentage_points(number_field(&usage, "per1WeekPercentage"));
    if five_hour.is_none() && weekly.is_none() {
        return Err(ProviderError::Parse(
            "Missing Alibaba Token Plan Personal usage windows".into(),
        ));
    }

    let plan_code = subscription_data.and_then(plan_code_from_bytes);
    let plan_name = plan_code
        .as_deref()
        .map(display_plan_name)
        .or_else(|| Some("Personal".to_string()));
    let quota = quota_config_data
        .zip(plan_code.as_ref())
        .and_then(|(data, code)| quota_totals_from_bytes(data, code));

    Ok(TokenPlanSnapshot {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::UsageSnapshot;
    use crate::providers::alibabatokenplan::AlibabaTokenPlanProvider;

    #[test]
    fn personal_form_forwards_optional_sec_token() {
        let with_token = build_personal_form(
            PERSONAL_USAGE_API,
            Map::new(),
            "cna=test-anon",
            AlibabaTokenPlanRegion::IntlPersonal,
            Some("personal-sec-token"),
        );
        assert!(
            with_token
                .iter()
                .any(|(key, value)| { *key == "sec_token" && value == "personal-sec-token" })
        );

        let without_token = build_personal_form(
            PERSONAL_USAGE_API,
            Map::new(),
            "cna=test-anon",
            AlibabaTokenPlanRegion::IntlPersonal,
            None,
        );
        assert!(!without_token.iter().any(|(key, _)| *key == "sec_token"));
    }

    #[test]
    fn personal_request_omits_captured_workspace_agent() {
        let params = build_personal_params_json(
            PERSONAL_USAGE_API,
            Map::new(),
            "cna=test-anon",
            AlibabaTokenPlanRegion::IntlPersonal,
        );
        let value: Value = serde_json::from_str(&params).unwrap();
        let cornerstone = value
            .get("Data")
            .and_then(|data| data.get("cornerstoneParam"))
            .and_then(Value::as_object)
            .unwrap();

        assert!(!cornerstone.contains_key("switchAgent"));
        assert_eq!(cornerstone.get("switchUserType"), Some(&json!(3)));
    }

    #[test]
    fn nested_workspace_error_surfaces_real_code_without_auth_eviction() {
        let payload = json!({
            "code": "200",
            "successResponse": true,
            "data": {
                "success": false,
                "httpStatus": 200,
                "errorCode": "BailianGateway.Workspace.NotAuthorised"
            }
        });

        let error =
            crate::providers::alibabatokenplan::throw_if_error_payload(&payload).unwrap_err();
        assert!(matches!(
            error,
            ProviderError::Other(message)
                if message.contains("BailianGateway.Workspace.NotAuthorised")
        ));
    }

    #[test]
    fn nested_gateway_error_prefers_error_message() {
        let payload = json!({
            "code": "200",
            "successResponse": true,
            "data": {
                "success": false,
                "httpStatus": 200,
                "errorCode": "BailianGateway.Quota.ServiceUnavailable",
                "errorMsg": "quota service unavailable"
            }
        });

        let error =
            crate::providers::alibabatokenplan::throw_if_error_payload(&payload).unwrap_err();
        assert!(matches!(
            error,
            ProviderError::Other(message) if message.contains("quota service unavailable")
        ));
    }

    #[test]
    fn success_envelope_without_windows_is_transient() {
        let payload = json!({
            "code": "SUCCESS",
            "successResponse": true,
            "errorCode": "",
            "data": {"success": true, "httpStatus": 200}
        });
        assert!(personal_usage_success_without_windows(
            payload.to_string().as_bytes()
        ));
    }

    #[test]
    fn success_envelope_with_windows_is_not_transient() {
        let payload = json!({
            "code": "SUCCESS",
            "successResponse": true,
            "errorCode": "",
            "data": {"per5HourPercentage": 0.5}
        });
        assert!(!personal_usage_success_without_windows(
            payload.to_string().as_bytes()
        ));
    }

    #[test]
    fn parses_personal_usage_fixture_with_plan_name() {
        let usage = serde_json::json!({
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
        let subscription = serde_json::json!({
            "code": "200",
            "data": {
                "specCode": "pro",
                "planName": "pro"
            }
        });
        let quota_config = serde_json::json!({
            "code": "200",
            "data": {
                "pro": {
                    "five_hour": 1000000,
                    "weekly": 5000000
                }
            }
        });

        let snapshot = parse_personal_usage(
            usage.to_string().as_bytes(),
            Some(subscription.to_string().as_bytes()),
            Some(quota_config.to_string().as_bytes()),
        )
        .unwrap();

        assert_eq!(snapshot.plan_name.as_deref(), Some("Pro"));
        assert!((snapshot.five_hour_used_percent.unwrap() - 0.09973083333333333).abs() < 1e-9);
        assert!((snapshot.weekly_used_percent.unwrap() - 0.03014725).abs() < 1e-9);
        assert_eq!(snapshot.five_hour_total_quota, Some(1_000_000.0));
        assert_eq!(snapshot.weekly_total_quota, Some(5_000_000.0));
        assert!(snapshot.five_hour_resets_at.is_some());
        assert!(snapshot.weekly_resets_at.is_some());

        let usage_snap: UsageSnapshot =
            AlibabaTokenPlanProvider::snapshot_to_usage(snapshot).unwrap();
        assert!((usage_snap.primary.used_percent - 0.09973083333333333).abs() < 1e-9);
        assert_eq!(usage_snap.primary.window_minutes, Some(300));
        assert_eq!(
            usage_snap.secondary.as_ref().and_then(|w| w.window_minutes),
            Some(10080)
        );
        assert_eq!(usage_snap.login_method.as_deref(), Some("Pro"));
    }

    #[test]
    fn weekly_only_personal_payload_promotes_weekly_window() {
        let usage = serde_json::json!({
            "code": "200",
            "data": {
                "DataV2": {
                    "ret": [{}],
                    "data": {
                        "msg": "",
                        "code": "SUCCESS",
                        "data": {
                            "per1WeekResetTime": 1788640320000_i64,
                            "per1WeekPercentage": 0.4145182484706
                        },
                        "success": true
                    }
                },
                "success": true,
                "httpStatus": 200,
                "errorCode": "",
                "api": "zeldaHttp.apikeyMgr./tokenplan/personal/api/v2/usage",
                "errorMsg": ""
            },
            "successResponse": true
        });

        let snapshot = parse_personal_usage(usage.to_string().as_bytes(), None, None).unwrap();
        assert!(snapshot.five_hour_used_percent.is_none());
        assert!((snapshot.weekly_used_percent.unwrap() - 41.45182484706).abs() < 1e-9);
        assert!(snapshot.weekly_resets_at.is_some());

        let usage_snap: UsageSnapshot =
            AlibabaTokenPlanProvider::snapshot_to_usage(snapshot).unwrap();
        assert!((usage_snap.primary.used_percent - 41.45182484706).abs() < 1e-9);
        assert_eq!(usage_snap.primary.window_minutes, Some(10080));
        assert!(usage_snap.secondary.is_none());
        assert_eq!(usage_snap.login_method.as_deref(), Some("Personal"));
    }
}
