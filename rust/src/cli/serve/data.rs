//! `/usage` and `/cost` data route handlers.
//!
//! Moved verbatim from the pre-0.48.0 serve module; the only 0.48.0 change is
//! the additive `daily` field on `/cost` — the web dashboard's daily spend bar
//! charts ride this array (upstream #2722 fetches `/cost` for the same data).

use serde_json::json;

use crate::cli::usage::ProviderSelection;
use crate::core::{CostScanOptions, FetchContext, ProviderId, SourceMode, instantiate_provider};
use crate::cost_scanner::{self, CostScanner};

use super::json_response;

pub async fn usage_response(provider: Option<&str>) -> String {
    let selection = match ProviderSelection::from_arg(provider) {
        Ok(selection) => selection,
        Err(error) => {
            return json_response(400, json!({ "error": error.to_string() }));
        }
    };
    let ctx = FetchContext {
        source_mode: SourceMode::Auto,
        include_credits: true,
        web_timeout: 60,
        verbose: false,
        manual_cookie_header: None,
        api_key: None,
        workspace_id: None,
        api_region: None,
        gateway_url: None,
        auto_prefer_web: false,
        // Serve `/usage` is a background poll read: keep the short optional-
        // join grace (upstream #2583), unlike `codexbar usage` which blocks
        // for the full completeness window.
        requires_optional_usage_completeness: false,
    };

    let mut results = Vec::new();
    for provider_id in selection.as_list() {
        let provider = instantiate_provider(provider_id);
        match provider.fetch_usage(&ctx).await {
            Ok(result) => results.push(json!({
                "provider": provider_id.cli_name(),
                "source": result.source_label,
                "usage": result.usage,
                "cost": result.cost,
            })),
            Err(error) => results.push(json!({
                "provider": provider_id.cli_name(),
                "error": error.to_string(),
            })),
        }
    }
    json_response(200, serde_json::Value::Array(results))
}

pub async fn cost_response(provider: Option<&str>) -> String {
    let selection = match ProviderSelection::from_arg(provider) {
        Ok(selection) => selection,
        Err(error) => {
            return json_response(400, json!({ "error": error.to_string() }));
        }
    };
    let scanner = CostScanner::new(30).with_options(CostScanOptions::app_driven());
    let mut results = Vec::new();
    for provider_id in selection.as_list() {
        if provider_id == ProviderId::Antigravity {
            let history = crate::providers::antigravity::local_sessions::summarize(30);
            results.push(antigravity_cost_payload(history, 30));
            continue;
        }
        let (supported, summary) = match provider_id {
            ProviderId::Codex => (true, scanner.scan_codex()),
            ProviderId::Claude => (true, scanner.scan_claude()),
            _ => (false, Default::default()),
        };
        if supported {
            // Daily spend history for the dashboard bar charts. The debounced
            // helper reuses the cache the summary scan just warmed, so no
            // second disk walk happens per request.
            let daily = daily_json(cost_scanner::get_daily_cost_history(
                provider_id.cli_name(),
                30,
            ));
            results.push(json!({
                "provider": provider_id.cli_name(),
                "supported": true,
                "days_scanned": 30,
                "cost": {
                    "total_usd": summary.total_cost_usd,
                    "currency": "USD"
                },
                "daily": daily,
                "tokens": {
                    "input": summary.input_tokens,
                    "output": summary.output_tokens,
                    "cached": summary.cached_tokens
                },
                "sessions_count": summary.sessions_count,
                "by_model": summary.by_model,
            }));
        } else {
            results.push(json!({
                "provider": provider_id.cli_name(),
                "supported": false,
                "error": "Local cost scanning not available for this provider"
            }));
        }
    }
    json_response(200, serde_json::Value::Array(results))
}

fn antigravity_cost_payload(
    history: crate::providers::antigravity::local_sessions::LocalSessionSummary,
    days: u32,
) -> serde_json::Value {
    use crate::providers::antigravity::local_sessions::LocalHistoryCoverage;
    let coverage = match history.coverage {
        LocalHistoryCoverage::Complete => "complete",
        LocalHistoryCoverage::Partial => "partial",
        LocalHistoryCoverage::Unavailable => "unavailable",
    };
    let complete = matches!(history.coverage, LocalHistoryCoverage::Complete);
    json!({
        "provider": "antigravity",
        "supported": true,
        "days_scanned": days,
        "cost": {"total_usd": serde_json::Value::Null, "currency": serde_json::Value::Null},
        "daily": [],
        "tokens": {"total": complete.then_some(history.total_tokens)},
        "sessions_count": complete.then_some(history.session_count),
        "historyCoverage": coverage,
        "knownZero": complete && history.total_tokens == 0,
        "note": "Local token history; dollar costs unavailable"
    })
}

/// Dashboard-charts shape for one provider's daily spend: [{date, totalCost}].
fn daily_json(daily: Vec<(String, Option<f64>)>) -> serde_json::Value {
    serde_json::Value::Array(
        daily
            .into_iter()
            .map(|(date, cost_usd)| json!({ "date": date, "totalCost": cost_usd }))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_array_shape_matches_dashboard_charts_contract() {
        let daily = daily_json(vec![
            ("2026-08-07".to_string(), Some(0.0)),
            ("2026-08-08".to_string(), Some(4.25)),
            ("2026-08-09".to_string(), None),
        ]);
        let rows = daily.as_array().unwrap();
        assert_eq!(rows[0]["date"], "2026-08-07");
        assert_eq!(rows[1]["totalCost"], 4.25);
        assert_eq!(rows[0]["totalCost"], 0.0);
        assert!(rows[2]["totalCost"].is_null());
    }

    #[test]
    fn antigravity_cost_payload_is_token_only_and_preserves_partial_unknown() {
        use crate::providers::antigravity::local_sessions::{
            LocalHistoryCoverage, LocalSessionSummary,
        };
        let complete = antigravity_cost_payload(
            LocalSessionSummary {
                total_tokens: 42,
                session_count: 1,
                coverage: LocalHistoryCoverage::Complete,
            },
            30,
        );
        assert!(complete["cost"]["total_usd"].is_null());
        assert_eq!(complete["tokens"]["total"], 42);
        assert_eq!(complete["historyCoverage"], "complete");

        let partial = antigravity_cost_payload(
            LocalSessionSummary {
                total_tokens: 42,
                session_count: 1,
                coverage: LocalHistoryCoverage::Partial,
            },
            30,
        );
        assert!(partial["tokens"]["total"].is_null());
        assert_eq!(partial["historyCoverage"], "partial");
    }
    #[test]
    fn daily_rows_use_upstream_total_cost_key_only() {
        let daily = daily_json(vec![
            ("2026-08-07".to_string(), Some(0.0)),
            ("2026-08-08".to_string(), Some(4.25)),
            ("2026-08-09".to_string(), None),
        ]);
        let serialized = daily.to_string();
        assert!(
            serialized.contains("\"totalCost\""),
            "wire key is totalCost"
        );
        assert!(
            !serialized.contains("cost_usd") && !serialized.contains("costUSD"),
            "no stale daily cost keys may leak to the wire"
        );
    }

    #[test]
    fn daily_empty_array_has_no_rows() {
        let daily = daily_json(vec![]);
        assert_eq!(daily.as_array().unwrap().len(), 0);
    }

    #[test]
    fn daily_zero_values_are_preserved_not_filtered() {
        let daily = daily_json(vec![("2026-08-07".to_string(), Some(0.0))]);
        let rows = daily.as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["totalCost"], 0.0);
    }
}
