use std::collections::HashMap;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use serde_json::Value;

use super::{AlibabaTokenPlanRegion, TokenPlanSnapshot};
use crate::core::ProviderError;
use crate::host::{CommandError, CommandOptions, CommandRunner};

const CLI_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
const CHILD_ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "PATHEXT",
    "HOME",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "TEMP",
    "TMP",
    "SystemRoot",
    "SYSTEMROOT",
    "ComSpec",
    "COMSPEC",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
];

pub(super) async fn fetch_cli_usage(
    region: AlibabaTokenPlanRegion,
) -> Result<TokenPlanSnapshot, ProviderError> {
    let environment = sanitized_child_environment(std::env::vars());
    let runner = environment.into_iter().fold(
        CommandRunner::new().without_inherited_env(),
        |runner, (key, value)| runner.with_env(key, value),
    );
    let options = CommandOptions {
        timeout: CLI_TIMEOUT,
        initial_delay: Duration::ZERO,
        extra_args: cli_arguments(region),
        ..CommandOptions::default()
    };

    let result = runner
        .run_async("bl", None, &options)
        .await
        .map_err(map_command_error)?;
    if result.timed_out {
        return Err(ProviderError::Timeout);
    }
    if result.exit_code != Some(0) {
        return Err(ProviderError::Other(
            "Bailian CLI could not load Token Plan usage. Sign in with 'bl' and try again."
                .to_string(),
        ));
    }
    if result.text.len() > MAX_OUTPUT_BYTES {
        return Err(ProviderError::Parse(
            "Bailian CLI Token Plan usage response was too large".to_string(),
        ));
    }
    parse_cli_usage(&result.text)
}

fn map_command_error(error: CommandError) -> ProviderError {
    match error {
        CommandError::BinaryNotFound(_) => ProviderError::NotInstalled(
            "Bailian CLI 'bl' is not installed or not on PATH.".to_string(),
        ),
        CommandError::TimedOut => ProviderError::Timeout,
        CommandError::LaunchFailed(_) | CommandError::IoError(_) => ProviderError::Other(
            "Bailian CLI could not load Token Plan usage. Sign in with 'bl' and try again."
                .to_string(),
        ),
    }
}

pub(super) fn cli_arguments(region: AlibabaTokenPlanRegion) -> Vec<String> {
    [
        "usage",
        "token-plan",
        "--console-region",
        region.current_region_id(),
        "--console-site",
        region.cli_console_site(),
        "--output",
        "json",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

pub(super) fn sanitized_child_environment(
    environment: impl IntoIterator<Item = (String, String)>,
) -> HashMap<String, String> {
    environment
        .into_iter()
        .filter(|(key, _)| {
            CHILD_ENV_ALLOWLIST
                .iter()
                .any(|allowed| key.eq_ignore_ascii_case(allowed))
        })
        .collect()
}

pub(super) fn parse_cli_usage(text: &str) -> Result<TokenPlanSnapshot, ProviderError> {
    let value: Value = serde_json::from_str(text).map_err(|_| {
        ProviderError::Parse("Bailian CLI returned an unsupported Token Plan usage response".into())
    })?;
    let object = value.as_object().ok_or_else(|| {
        ProviderError::Parse("Bailian CLI returned an unsupported Token Plan usage response".into())
    })?;

    let five_hour_ratio = ratio(object.get("per5HourPercentage"));
    let weekly_ratio = ratio(object.get("per1WeekPercentage"));
    if five_hour_ratio.is_none() && weekly_ratio.is_none() {
        return Err(ProviderError::Parse(
            "Bailian CLI returned an unsupported Token Plan usage response".into(),
        ));
    }

    Ok(TokenPlanSnapshot {
        plan_name: Some("Token Plan".to_string()),
        used_quota: None,
        total_quota: None,
        remaining_quota: None,
        resets_at: None,
        five_hour_used_percent: five_hour_ratio.map(|ratio| ratio * 100.0),
        five_hour_total_quota: None,
        five_hour_resets_at: five_hour_ratio
            .and_then(|_| reset_date(object.get("per5HourResetTime"))),
        weekly_used_percent: weekly_ratio.map(|ratio| ratio * 100.0),
        weekly_total_quota: None,
        weekly_resets_at: weekly_ratio.and_then(|_| reset_date(object.get("per1WeekResetTime"))),
    })
}

fn ratio(value: Option<&Value>) -> Option<f64> {
    let ratio = value?.as_f64()?;
    (ratio.is_finite() && (0.0..=1.0).contains(&ratio)).then_some(ratio)
}

fn reset_date(value: Option<&Value>) -> Option<chrono::DateTime<Utc>> {
    let milliseconds = value?.as_f64()?;
    if !milliseconds.is_finite() || milliseconds <= 0.0 {
        return None;
    }
    let rounded = milliseconds.round();
    if rounded < 1.0 || rounded >= 9_223_372_036_854_775_808.0 {
        return None;
    }
    let millis = format!("{rounded:.0}").parse::<i64>().ok()?;
    Utc.timestamp_millis_opt(millis).single()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_cli_windows_and_millisecond_resets() {
        let parsed = parse_cli_usage(
            r#"{"per5HourPercentage":0.25,"per5HourResetTime":1787000400000,"per1WeekPercentage":0.7,"per1WeekResetTime":1787001180000}"#,
        )
        .unwrap();
        assert_eq!(parsed.five_hour_used_percent, Some(25.0));
        assert_eq!(
            parsed.five_hour_resets_at,
            Utc.timestamp_millis_opt(1_787_000_400_000).single()
        );
        assert_eq!(parsed.weekly_used_percent, Some(70.0));
        assert_eq!(
            parsed.weekly_resets_at,
            Utc.timestamp_millis_opt(1_787_001_180_000).single()
        );
    }

    #[test]
    fn reset_date_rejects_non_finite_and_out_of_range_values() {
        assert!(reset_date(Some(&Value::from(-1.0))).is_none());
        assert!(reset_date(Some(&Value::from(0.0))).is_none());
        assert!(reset_date(Some(&Value::from(f64::NAN))).is_none());
        assert!(reset_date(Some(&Value::from(f64::INFINITY))).is_none());
        assert!(reset_date(Some(&Value::from(f64::MAX))).is_none());
        assert!(reset_date(Some(&Value::from(i64::MAX as f64 + 4096.0))).is_none());
        let fractional = reset_date(Some(&Value::from(1_787_000_400_250.5)));
        assert_eq!(
            fractional,
            Utc.timestamp_millis_opt(1_787_000_400_251).single()
        );
        let integer_valued_float = reset_date(Some(&Value::from(1_787_000_400_000.0)));
        assert_eq!(
            integer_valued_float,
            Utc.timestamp_millis_opt(1_787_000_400_000).single()
        );
    }

    #[test]
    fn accepts_either_valid_window_and_rejects_no_valid_window() {
        let weekly =
            parse_cli_usage(r#"{"per5HourPercentage":"bad","per1WeekPercentage":0.7}"#).unwrap();
        assert_eq!(weekly.five_hour_used_percent, None);
        assert_eq!(weekly.weekly_used_percent, Some(70.0));

        assert!(
            parse_cli_usage(
                r#"{"per5HourPercentage":true,"per1WeekPercentage":-0.1,"percentage":0.5}"#
            )
            .is_err()
        );
    }

    #[test]
    fn regional_cli_arguments_match_bailian_contract() {
        assert_eq!(
            cli_arguments(AlibabaTokenPlanRegion::CnPersonal),
            vec![
                "usage",
                "token-plan",
                "--console-region",
                "cn-beijing",
                "--console-site",
                "domestic",
                "--output",
                "json"
            ]
        );
        assert_eq!(
            cli_arguments(AlibabaTokenPlanRegion::Intl),
            vec![
                "usage",
                "token-plan",
                "--console-region",
                "ap-southeast-1",
                "--console-site",
                "international",
                "--output",
                "json"
            ]
        );
    }

    #[test]
    fn child_environment_drops_unrelated_secrets() {
        let sanitized = sanitized_child_environment([
            ("PATH".to_string(), "fixture".to_string()),
            ("USERPROFILE".to_string(), "C:\\Users\\fixture".to_string()),
            ("HTTPS_PROXY".to_string(), "http://proxy".to_string()),
            ("AWS_SECRET_ACCESS_KEY".to_string(), "secret".to_string()),
            (
                "ALIBABA_TOKEN_PLAN_COOKIE".to_string(),
                "cookie".to_string(),
            ),
            ("SSH_AUTH_SOCK".to_string(), "socket".to_string()),
        ]);
        assert_eq!(sanitized.get("PATH").map(String::as_str), Some("fixture"));
        assert!(sanitized.contains_key("USERPROFILE"));
        assert!(sanitized.contains_key("HTTPS_PROXY"));
        assert!(!sanitized.contains_key("AWS_SECRET_ACCESS_KEY"));
        assert!(!sanitized.contains_key("ALIBABA_TOKEN_PLAN_COOKIE"));
        assert!(!sanitized.contains_key("SSH_AUTH_SOCK"));
    }
}
