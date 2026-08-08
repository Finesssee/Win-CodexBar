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
}
