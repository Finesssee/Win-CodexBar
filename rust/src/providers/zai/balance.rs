use serde_json::Value;

const BALANCE_URL: &str = "https://www.bigmodel.cn/api/biz/account/query-customer-account-report";

pub(super) async fn fetch_cn_balance(client: &reqwest::Client, authorization: &str) -> Option<f64> {
    let response = client
        .get(BALANCE_URL)
        .header("Authorization", authorization)
        .header("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body: Value = response.json().await.ok()?;
    if body.get("success").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let data = body.get("data")?;
    ["availableBalance", "balance"]
        .into_iter()
        .find_map(|key| finite_nonnegative_number(data.get(key)))
}

fn finite_nonnegative_number(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        })
        .filter(|value| value.is_finite() && *value >= 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balance_parser_prefers_available_balance_and_rejects_null() {
        let payload = serde_json::json!({
            "availableBalance": 12.5,
            "balance": 99.0
        });
        assert_eq!(
            finite_nonnegative_number(payload.get("availableBalance")),
            Some(12.5)
        );
        let null_payload = serde_json::json!({"availableBalance": null});
        assert_eq!(
            finite_nonnegative_number(null_payload.get("availableBalance")),
            None
        );
    }

    #[test]
    fn balance_parser_accepts_numeric_strings_but_rejects_negative() {
        let numeric = serde_json::json!("42.25");
        let negative = serde_json::json!(-1.0);
        assert_eq!(finite_nonnegative_number(Some(&numeric)), Some(42.25));
        assert_eq!(finite_nonnegative_number(Some(&negative)), None);
    }
}
