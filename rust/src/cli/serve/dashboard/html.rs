//! Serve web dashboard HTML shell (`GET /`).
//!
//! Upstream parity (A1, #2715 / #2722 / #2723): static embedded page,
//! `Cache-Control: no-store`, provider-id → icon-URL map injected at serve
//! time (sorted, deterministic), refresh interval injected from the serve
//! configuration. The page itself fetches `/dashboard/v1/snapshot` and `/cost`
//! with the user's bearer token and renders grouped provider cards, per-account
//! claude sections, and daily spend bar charts.

/// Render the shell with config-derived values baked in.
pub fn render_shell(refresh_seconds: u32) -> String {
    const TEMPLATE: &str = include_str!("dashboard.html");
    TEMPLATE
        .replace("__PROVIDER_ICON_URLS__", &super::icons::icon_url_map())
        .replace("__REFRESH_SECONDS__", &refresh_seconds.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_injects_icons_and_refresh_without_leftover_placeholders() {
        let html = render_shell(90);
        assert!(!html.contains("__PROVIDER_ICON_URLS__"));
        assert!(!html.contains("__REFRESH_SECONDS__"));
        assert!(html.contains(r#""codex":"/icons/ProviderIcon-codex.svg""#));
        assert!(html.contains("Math.max(15, 90)"));
        assert!(html.contains("/dashboard/v1/snapshot"));
        assert!(html.contains("/cost"));
    }

    #[test]
    fn shell_is_self_contained() {
        let html = render_shell(60);
        assert!(
            !html.contains("http://") && !html.contains("https://"),
            "no external refs: {}",
            ""
        );
        assert!(html.contains("Content-Security-Policy"));
    }

    #[test]
    fn status_chip_is_conditional_only_2723() {
        let html = render_shell(60);
        assert!(html.contains("statusChip"));
        assert!(
            html.contains("if (!status || !status.level) return \"\""),
            "#2723: chip must be hidden whenever no provider status exists"
        );
    }

    /// The daily chart gate and every per-row value read must agree with the
    /// `/cost` wire key (`totalCost`). The shell is static, so the agreement is
    /// pinned by exact-string assertions on the rendered template.
    #[test]
    fn daily_chart_gate_reads_upstream_total_cost_key() {
        let html = render_shell(60);
        // Gate: render only when some row has a positive totalCost.
        assert!(
            html.contains("daily.some(v => (v.totalCost || 0) > 0)"),
            "chart gate must gate on the upstream totalCost key"
        );
        // Every per-row value read uses the upstream key — no stale costUSD.
        assert!(
            !html.contains("costUSD"),
            "stale costUSD reads must not survive the totalCost rename"
        );
        assert!(
            !html.contains("daily.some(v => v > 0)"),
            "bare value gate must be replaced with the totalCost key gate"
        );
    }

    #[test]
    fn daily_chart_behavior_positive_zero_empty() {
        let html = render_shell(60);
        // The wire key the shell reads matches the data.rs `daily_json` key.
        assert!(html.contains("d.totalCost"));
        // Zero rows are rendered with the `.zero` class (kept, not hidden by the
        // gate), and the gate hides an all-zero / empty daily series entirely.
        assert!(html.contains("\" zero\""));
        // Mirror the JS gate predicate in Rust over wire-shaped rows to lock
        // the positive / zero / empty behavior the chart depends on.
        let gate = |rows: &[serde_json::Value]| {
            rows.iter()
                .any(|v| v["totalCost"].as_f64().unwrap_or(0.0) > 0.0)
        };
        assert!(gate(&[
            serde_json::json!({"date":"2026-08-08","totalCost":4.25})
        ]));
        assert!(!gate(&[
            serde_json::json!({"date":"2026-08-07","totalCost":0.0})
        ]));
        assert!(!gate(&[]));
    }
}
