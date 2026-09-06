use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

use crate::core::{CostDailyPoint, CostSnapshot, ProviderError};

const MAX_ACTIVITY_ROWS: usize = 20_000;

pub(super) fn parse_activity_cost(
    payloads: &[Value],
    now: DateTime<Utc>,
) -> Result<CostSnapshot, ProviderError> {
    let latest_completed = now.date_naive() - Duration::days(1);
    let cutoff = latest_completed - Duration::days(29);
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut daily: BTreeMap<String, f64> = BTreeMap::new();
    let mut total = 0.0;
    let mut rows_seen = 0usize;

    for payload in payloads {
        let rows = payload
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ProviderError::Parse("OpenRouter activity.data must be an array".into())
            })?;
        rows_seen = rows_seen.saturating_add(rows.len());
        if rows_seen > MAX_ACTIVITY_ROWS {
            return Err(ProviderError::Parse(
                "OpenRouter activity.data exceeds 20000 rows".into(),
            ));
        }
        for (index, row) in rows.iter().enumerate() {
            let object = row.as_object().ok_or_else(|| {
                ProviderError::Parse(format!(
                    "OpenRouter activity.data[{index}] must be an object"
                ))
            })?;
            let raw_day = object
                .get("date")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ProviderError::Parse(format!(
                        "OpenRouter activity.data[{index}].date is missing"
                    ))
                })?;
            let day = normalize_activity_day(raw_day).ok_or_else(|| {
                ProviderError::Parse(format!(
                    "OpenRouter activity.data[{index}].date must be YYYY-MM-DD or YYYY-MM-DD HH:MM:SS"
                ))
            })?;
            let parsed_day = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").map_err(|_| {
                ProviderError::Parse(format!(
                    "OpenRouter activity.data[{index}].date must be a real calendar date"
                ))
            })?;
            if parsed_day > latest_completed || parsed_day < cutoff {
                continue;
            }
            let model = object
                .get("model_permaslug")
                .or_else(|| object.get("model"))
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or("");
            if model.len() > 64 {
                return Err(ProviderError::Parse(format!(
                    "OpenRouter activity.data[{index}].model exceeds 64 characters"
                )));
            }
            let prompt = nonnegative_integer(object.get("prompt_tokens"), index, "prompt_tokens")?;
            let completion =
                nonnegative_integer(object.get("completion_tokens"), index, "completion_tokens")?;
            let reasoning = match object.get("reasoning_tokens") {
                Some(Value::Null) | None => 0,
                value => nonnegative_integer(value, index, "reasoning_tokens")?,
            };
            if reasoning > completion {
                return Err(ProviderError::Parse(format!(
                    "OpenRouter activity.data[{index}].reasoning_tokens exceeds completion_tokens"
                )));
            }
            let requests = nonnegative_integer(object.get("requests"), index, "requests")?;
            let metered = nonnegative_number(object.get("usage"), index, "usage")?;
            let estimated = match object.get("byok_usage_inference") {
                Some(Value::Null) | None => 0.0,
                value => nonnegative_number(value, index, "byok_usage_inference")?,
            };
            let cost = metered + estimated;
            if !cost.is_finite() {
                return Err(ProviderError::Parse(
                    "OpenRouter Activity spend overflowed".into(),
                ));
            }
            let identity = format!(
                "{day}|{model}|{}|{}|{}",
                object
                    .get("endpoint_id")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
                object
                    .get("provider_name")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
                object
                    .get("workspace_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
            );
            let signature = format!(
                "{prompt}|{completion}|{reasoning}|{requests}|{metered:.12}|{estimated:.12}"
            );
            if let Some(existing) = seen.get(&identity) {
                if existing != &signature {
                    return Err(ProviderError::Parse(
                        "OpenRouter Activity contains conflicting duplicate rows".into(),
                    ));
                }
                continue;
            }
            seen.insert(identity, signature);
            total += cost;
            *daily.entry(day.to_string()).or_default() += cost;
        }
    }

    if !total.is_finite() {
        return Err(ProviderError::Parse(
            "OpenRouter Activity spend overflowed".into(),
        ));
    }
    Ok(
        CostSnapshot::new(total, "USD", "Last 30 days (UTC)").with_daily(
            daily
                .into_iter()
                .map(|(day, amount)| CostDailyPoint { day, amount })
                .collect(),
        ),
    )
}

fn normalize_activity_day(raw: &str) -> Option<&str> {
    let bytes = raw.as_bytes();
    let shape_ok = match bytes.len() {
        10 => true,
        19 => {
            bytes[10] == b' '
                && bytes[13] == b':'
                && bytes[16] == b':'
                && bytes[11..13].iter().all(u8::is_ascii_digit)
                && bytes[14..16].iter().all(u8::is_ascii_digit)
                && bytes[17..19].iter().all(u8::is_ascii_digit)
        }
        _ => false,
    };
    if !shape_ok
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes[0..4].iter().all(u8::is_ascii_digit)
        || !bytes[5..7].iter().all(u8::is_ascii_digit)
        || !bytes[8..10].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    Some(&raw[..10])
}

fn nonnegative_integer(
    value: Option<&Value>,
    index: usize,
    field: &str,
) -> Result<u64, ProviderError> {
    let value = value.ok_or_else(|| {
        ProviderError::Parse(format!(
            "OpenRouter activity.data[{index}].{field} is missing"
        ))
    })?;
    value.as_u64().ok_or_else(|| {
        ProviderError::Parse(format!(
            "OpenRouter activity.data[{index}].{field} must be a nonnegative integer"
        ))
    })
}

fn nonnegative_number(
    value: Option<&Value>,
    index: usize,
    field: &str,
) -> Result<f64, ProviderError> {
    value
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or_else(|| {
            ProviderError::Parse(format!(
                "OpenRouter activity.data[{index}].{field} must be finite and nonnegative"
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-22T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn aggregates_metered_and_byok_spend_without_double_counting_latest_completed_day() {
        let history = serde_json::json!({"data":[
            {"date":"2026-08-20","model":"m0","prompt_tokens":5,"completion_tokens":2,"reasoning_tokens":0,"requests":1,"usage":0.5,"byok_usage_inference":0.0},
            {"date":"2026-08-21","model":"m1","prompt_tokens":10,"completion_tokens":5,"reasoning_tokens":2,"requests":1,"usage":1.25,"byok_usage_inference":0.25}
        ]});
        let latest_completed = serde_json::json!({"data":[
            {"date":"2026-08-21","model":"m1","prompt_tokens":10,"completion_tokens":5,"reasoning_tokens":2,"requests":1,"usage":1.25,"byok_usage_inference":0.25}
        ]});
        let cost = parse_activity_cost(&[history, latest_completed], now()).unwrap();
        assert!((cost.used - 2.0).abs() < 1e-12);
        assert_eq!(cost.daily.len(), 2);
        assert_eq!(cost.period, "Last 30 days (UTC)");
    }

    #[test]
    fn rejects_conflicting_duplicate_activity_rows() {
        let a = serde_json::json!({"data":[
            {"date":"2026-08-21","model":"m","prompt_tokens":10,"completion_tokens":5,"requests":1,"usage":1.0}
        ]});
        let b = serde_json::json!({"data":[
            {"date":"2026-08-21","model":"m","prompt_tokens":11,"completion_tokens":5,"requests":1,"usage":1.0}
        ]});
        assert!(parse_activity_cost(&[a, b], now()).is_err());
    }

    #[test]
    fn filters_rows_outside_exact_30_day_window() {
        let payload = serde_json::json!({"data":[
            {"date":"2026-07-22","model":"old","prompt_tokens":10,"completion_tokens":5,"requests":1,"usage":99.0},
            {"date":"2026-07-23","model":"in","prompt_tokens":10,"completion_tokens":5,"requests":1,"usage":1.0},
            {"date":"2026-08-22","model":"today","prompt_tokens":10,"completion_tokens":5,"requests":1,"usage":99.0}
        ]});
        let cost = parse_activity_cost(&[payload], now()).unwrap();
        assert_eq!(cost.used, 1.0);
    }

    #[test]
    fn accepts_timestamp_shaped_activity_dates_and_normalizes_to_the_utc_day() {
        for date in ["2026-08-21", "2026-08-21 00:00:00"] {
            let payload = serde_json::json!({"data":[
                {"date":date,"model":"m","prompt_tokens":10,"completion_tokens":5,"requests":1,"usage":1.0}
            ]});
            let cost = parse_activity_cost(&[payload], now()).unwrap();
            assert_eq!(cost.daily.len(), 1);
            assert_eq!(cost.daily[0].day, "2026-08-21");
        }
    }

    #[test]
    fn rejects_unsupported_or_impossible_activity_timestamp_dates() {
        for date in ["2026-08-21T00:00:00", "2026-02-31 00:00:00"] {
            let payload = serde_json::json!({"data":[
                {"date":date,"model":"m","prompt_tokens":10,"completion_tokens":5,"requests":1,"usage":1.0}
            ]});
            assert!(parse_activity_cost(&[payload], now()).is_err());
        }
    }
}
