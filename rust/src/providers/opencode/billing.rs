//! OpenCode billing subsystem (pay-as-you-go workspaces).
//!
//! Extracted from `mod.rs` (upstream 0.49.5 #2504/#2697). The billing
//! server function carries the monthly spend fields pay-as-you-go
//! workspaces bill against; this module owns the DTO, JSON/SolidStart
//! parsing, the HTTP fallback, and the presentation mapping.

use serde_json::Value;
use uuid::Uuid;

use crate::core::{ProviderError, RateWindow, UsageSnapshot};

use super::{BASE_URL, OpenCodeProvider, SERVER_URL};

/// Customer/billing server function carrying the monthly spend fields
/// pay-as-you-go workspaces bill against (upstream 0.49.5 #2504/#2697).
pub(super) const BILLING_SERVER_ID: &str =
    "c83b78a614689c38ebee981f9b39a8b377716db85c1fd7dbab604adc02d3313d";

/// Billing/customer payload for an OpenCode workspace (upstream
/// `OpenCodeZenBillingInfo`). `monthlyUsage` and `balance` arrive as
/// fixed-point integers scaled by 1e8; `monthlyLimit` is whole USD.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenCodeZenBilling {
    /// Spend in the current monthly cycle, in USD.
    pub monthly_usage_usd: f64,
    /// Configured monthly spend limit, in USD (`None` when unset).
    pub monthly_limit_usd: Option<f64>,
    /// Remaining prepaid balance, in USD.
    pub balance_usd: Option<f64>,
    /// Whether the workspace still carries a subscription object (legacy
    /// quota accounts).
    pub has_subscription: bool,
}

const ZEN_USD_SCALE: f64 = 100_000_000.0;

/// Parse the customer/billing payload. The response may arrive as
/// SolidStart's `$R[...]` JavaScript payload rather than JSON, so the JSON
/// path is tried first and a tolerant field scan is the fallback. A
/// `customerID` must be present before any number is trusted (upstream
/// `OpenCodeZenBillingParser`).
pub(super) fn parse_zen_billing(text: &str) -> Option<OpenCodeZenBilling> {
    if let Ok(value) = serde_json::from_str::<Value>(text)
        && let Some(customer) = find_customer_object(&value)
    {
        let raw_usage = json_number(customer.get("monthlyUsage")?)?;
        return Some(OpenCodeZenBilling {
            monthly_usage_usd: raw_usage / ZEN_USD_SCALE,
            monthly_limit_usd: customer.get("monthlyLimit").and_then(json_number),
            balance_usd: customer
                .get("balance")
                .and_then(json_number)
                .map(|v| v / ZEN_USD_SCALE),
            has_subscription: customer.get("subscription").is_some_and(|s| !s.is_null()),
        });
    }
    parse_zen_billing_payload(text)
}

/// Find the object carrying a non-empty `customerID`, at the root or nested
/// one level under any key.
fn find_customer_object(value: &Value) -> Option<&serde_json::Map<String, Value>> {
    let mut candidates: Vec<&serde_json::Map<String, Value>> = Vec::new();
    if let Some(object) = value.as_object() {
        candidates.push(object);
        for child in object.values() {
            if let Some(child_object) = child.as_object() {
                candidates.push(child_object);
            }
        }
    }
    candidates.into_iter().find(|object| {
        object
            .get("customerID")
            .and_then(|id| id.as_str())
            .is_some_and(|id| !id.is_empty())
    })
}

fn json_number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|i| i as f64))
        .or_else(|| value.as_str()?.parse().ok())
}

/// Tolerant `$R[...]` payload scan with the same field semantics.
fn parse_zen_billing_payload(text: &str) -> Option<OpenCodeZenBilling> {
    let customer_id = regex_lite::Regex::new(r#""?customerID"?\s*:\s*"[^"]+""#).ok()?;
    if !customer_id.is_match(text) {
        return None;
    }
    let raw_usage = payload_number("monthlyUsage", text)?;
    Some(OpenCodeZenBilling {
        monthly_usage_usd: raw_usage / ZEN_USD_SCALE,
        monthly_limit_usd: payload_number("monthlyLimit", text),
        balance_usd: payload_number("balance", text).map(|v| v / ZEN_USD_SCALE),
        has_subscription: payload_has_subscription(text),
    })
}

fn payload_number(field: &str, text: &str) -> Option<f64> {
    let re =
        regex_lite::Regex::new(&format!(r#""?{field}"?\s*:\s*(-?[0-9]+(?:\.[0-9]+)?)"#)).ok()?;
    re.captures(text)?.get(1)?.as_str().parse().ok()
}

/// A subscription is only considered present when the field exists and is
/// not `null`, so a pay-as-you-go payload never routes back into the retired
/// subscription path.
fn payload_has_subscription(text: &str) -> bool {
    let Ok(present) = regex_lite::Regex::new(r#""?subscription"?\s*:\s*[^,}]+"#) else {
        return false;
    };
    let Ok(is_null) = regex_lite::Regex::new(r#""?subscription"?\s*:\s*null"#) else {
        return false;
    };
    present.is_match(text) && !is_null.is_match(text)
}

impl OpenCodeProvider {
    /// Only subscription-shaped failures are worth retrying against billing
    /// (upstream `canFallBackToBilling`): credential and transport failures
    /// would fail the same way on the billing call.
    pub(super) fn can_fall_back_to_billing(err: &ProviderError) -> bool {
        matches!(err, ProviderError::Parse(_) | ProviderError::Other(_))
    }

    /// Billing/customer payload fallback for pay-as-you-go workspaces.
    ///
    /// Returns `Err` only for an actionable signed-out diagnosis (which wins
    /// over the original error, matching upstream), `Ok(None)` when the
    /// billing payload does not carry monthly usage fields or still reports
    /// a subscription.
    pub(super) async fn fetch_pay_as_you_go_usage(
        &self,
        workspace_id: &str,
        cookie_header: &str,
    ) -> Result<Option<UsageSnapshot>, ProviderError> {
        let referer = format!("https://opencode.ai/workspace/{workspace_id}");
        let args = serde_json::json!([workspace_id]);
        let encoded_args = Self::url_encode(&args.to_string());
        let url = format!(
            "{}?id={}&args={}",
            SERVER_URL, BILLING_SERVER_ID, encoded_args
        );

        let response = self
            .client
            .get(&url)
            .header("Cookie", cookie_header)
            .header("X-Server-Id", BILLING_SERVER_ID)
            .header("X-Server-Instance", format!("server-fn:{}", Uuid::new_v4()))
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            )
            .header("Origin", BASE_URL)
            .header("Referer", referer)
            .header(
                "Accept",
                "text/javascript, application/json;q=0.9, */*;q=0.8",
            )
            .send()
            .await;

        let response = match response {
            Ok(response) => response,
            Err(err) => {
                tracing::debug!("OpenCode billing fallback transport failed: {err}");
                return Ok(None);
            }
        };
        if response.status().as_u16() == 401 || response.status().as_u16() == 403 {
            return Err(ProviderError::AuthRequired);
        }
        if !response.status().is_success() {
            tracing::debug!("OpenCode billing fallback returned {}", response.status());
            return Ok(None);
        }
        let text = match response.text().await {
            Ok(text) => text,
            Err(err) => {
                tracing::debug!("OpenCode billing fallback body read failed: {err}");
                return Ok(None);
            }
        };
        if self.looks_signed_out(&text) {
            return Err(ProviderError::AuthRequired);
        }

        let Some(billing) = parse_zen_billing(&text) else {
            tracing::debug!("OpenCode billing payload missing monthly usage fields");
            return Ok(None);
        };
        if billing.has_subscription {
            tracing::debug!(
                "OpenCode billing fallback still reports a subscription; preserving error"
            );
            return Ok(None);
        }
        tracing::debug!(
            "OpenCode billing usage resolved (limit {})",
            if billing.monthly_limit_usd.is_some() {
                "set"
            } else {
                "unset"
            }
        );
        Ok(Some(self.snapshot_from_pay_as_you_go(&billing)))
    }

    /// Pay-as-you-go presentation: monthly spend against the configured
    /// monthly limit (when set) with the prepaid balance in the login label.
    pub(super) fn snapshot_from_pay_as_you_go(
        &self,
        billing: &OpenCodeZenBilling,
    ) -> UsageSnapshot {
        let used_percent = billing
            .monthly_limit_usd
            .filter(|limit| *limit > 0.0 && limit.is_finite())
            .map(|limit| ((billing.monthly_usage_usd / limit) * 100.0).clamp(0.0, 100.0))
            .unwrap_or(0.0);
        let mut primary = RateWindow::with_details(
            used_percent,
            Some(30 * 24 * 60),
            None,
            Some(format!(
                "${:.2} spent this month",
                billing.monthly_usage_usd
            )),
        );
        primary.is_informational = billing.monthly_limit_usd.is_none();
        let mut usage = UsageSnapshot::new(primary);
        let label = match billing.balance_usd {
            Some(balance) => format!("Pay-as-you-go · ${balance:.2} prepaid"),
            None => "Pay-as-you-go".to_string(),
        };
        usage = usage.with_login_method(label);
        usage
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ProviderError;
    use crate::providers::opencode::OpenCodeProvider;

    #[test]
    fn zen_billing_json_path_scales_usage_and_balance() {
        // Upstream 0.49.5 #2504/#2697: monthlyUsage/balance are fixed-point
        // 1e8 integers; monthlyLimit is whole USD; a null subscription marks
        // a pay-as-you-go workspace.
        let billing = parse_zen_billing(
            r#"{
              "customerID": "cus_123",
              "monthlyUsage": 1250000000,
              "monthlyLimit": 50,
              "balance": 200000000,
              "subscription": null
            }"#,
        )
        .expect("zen billing");

        assert!((billing.monthly_usage_usd - 12.5).abs() < 1e-9);
        assert_eq!(billing.monthly_limit_usd, Some(50.0));
        assert!((billing.balance_usd.unwrap() - 2.0).abs() < 1e-9);
        assert!(!billing.has_subscription);

        let snapshot = OpenCodeProvider::new().snapshot_from_pay_as_you_go(&billing);
        assert!((snapshot.primary.used_percent - 25.0).abs() < 0.01);
        assert_eq!(
            snapshot.primary.reset_description.as_deref(),
            Some("$12.50 spent this month")
        );
        assert_eq!(
            snapshot.login_method.as_deref(),
            Some("Pay-as-you-go · $2.00 prepaid")
        );
    }

    #[test]
    fn zen_billing_payload_scan_and_guards() {
        // $R[...] script payload fallback.
        let billing = parse_zen_billing(
            "$R[0]={\"customerID\":\"cus_9\",\"monthlyUsage\":500000000,\"balance\":null,\"subscription\":null}",
        )
        .expect("payload scan");
        assert!((billing.monthly_usage_usd - 5.0).abs() < 1e-9);
        assert!(billing.balance_usd.is_none());
        assert!(!billing.has_subscription);

        // No customerID → nothing is trusted.
        assert!(parse_zen_billing("{\"monthlyUsage\": 500000000}").is_none());
        // No monthlyUsage → no snapshot.
        assert!(parse_zen_billing("$R[0]={\"customerID\":\"cus_9\",\"balance\":1}").is_none());

        // A present, non-null subscription object keeps the legacy path.
        let subscribed = parse_zen_billing(
            r#"{"customerID":"cus_9","monthlyUsage":1,"subscription":{"plan":"pro"}}"#,
        )
        .expect("subscribed billing");
        assert!(subscribed.has_subscription);
        assert!(payload_has_subscription(
            "\"subscription\": {\"plan\": \"pro\"}"
        ));
        assert!(!payload_has_subscription("\"subscription\": null"));
        assert!(!payload_has_subscription("no subscription field"));
    }

    #[test]
    fn only_subscription_shaped_errors_fall_back_to_billing() {
        assert!(OpenCodeProvider::can_fall_back_to_billing(
            &ProviderError::Parse("missing usage percent".into())
        ));
        assert!(OpenCodeProvider::can_fall_back_to_billing(
            &ProviderError::Other("OpenCode subscription API returned 500".into())
        ));
        assert!(!OpenCodeProvider::can_fall_back_to_billing(
            &ProviderError::AuthRequired
        ));
    }
}
