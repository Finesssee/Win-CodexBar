//! Cursor provider implementation
//!
//! Fetches usage data from Cursor's API using browser cookies

mod api;
mod app_auth;
mod token_cost;

use async_trait::async_trait;

use crate::core::{
    CostSnapshot, FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId,
    ProviderMetadata, RateWindow, SourceMode, UsageSnapshot,
};

pub use api::CursorApi;

/// Cursor provider for fetching AI usage limits
pub struct CursorProvider {
    metadata: ProviderMetadata,
    api: CursorApi,
}

impl CursorProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Cursor,
                display_name: "Cursor",
                session_label: "Plan",
                weekly_label: "Cursor",
                supports_opus: false,
                // Upstream #2338: Cursor has no account credit balance to advertise.
                supports_credits: false,
                default_enabled: true,
                is_primary: false,
                dashboard_url: Some("https://cursor.com/dashboard/usage"),
                status_page_url: None,
            },
            api: CursorApi::new(),
        }
    }

    async fn fetch_web_usage(
        &self,
        ctx: &FetchContext,
    ) -> Result<
        (
            api::CursorUsageResult,
            Option<token_cost::CursorTokenCostReport>,
        ),
        ProviderError,
    > {
        let cookie_header = if let Some(cookie_header) = ctx.manual_cookie_header.as_deref() {
            cookie_header.to_string()
        } else {
            // Upstream 0.50.0 #2398: Automatic mode prefers the signed-in
            // Cursor app's read-only local session over browser cookies.
            // A rejected app session (stale token, account mismatch)
            // surfaces in the log and falls back to the browser import.
            if ctx.source_mode == SourceMode::Auto
                && let Some(app_result) = self.fetch_via_app_session().await
            {
                return Ok(app_result);
            }
            crate::providers::browser_cookie_header(&["cursor.com", "cursor.sh"])?
        };

        self.fetch_usage_and_token_report(&cookie_header).await
    }

    /// One usage pass with the app's local session; `None` means the app
    /// session was unavailable or rejected (caller falls back to cookies).
    async fn fetch_via_app_session(
        &self,
    ) -> Option<(
        api::CursorUsageResult,
        Option<token_cost::CursorTokenCostReport>,
    )> {
        let app_cookie = app_auth::preferred_auto_cookie_header()?;
        let usage = match self.api.fetch_usage_with_cookie_header(&app_cookie).await {
            Ok(usage) => usage,
            Err(err) => {
                tracing::debug!(
                    "Cursor app session rejected ({err}); falling back to browser cookies"
                );
                return None;
            }
        };
        app_auth::store_validated_app_session(&app_cookie);
        let token_report = self.fetch_token_report_best_effort(&app_cookie).await;
        Some((usage, token_report))
    }

    async fn fetch_usage_and_token_report(
        &self,
        cookie_header: &str,
    ) -> Result<
        (
            api::CursorUsageResult,
            Option<token_cost::CursorTokenCostReport>,
        ),
        ProviderError,
    > {
        let usage = self
            .api
            .fetch_usage_with_cookie_header(cookie_header)
            .await?;
        let token_report = self.fetch_token_report_best_effort(cookie_header).await;
        Ok((usage, token_report))
    }

    /// Best-effort token-cost page; never fail the main usage fetch.
    async fn fetch_token_report_best_effort(
        &self,
        cookie_header: &str,
    ) -> Option<token_cost::CursorTokenCostReport> {
        match token_cost::fetch_token_cost_report(
            self.api.client(),
            cookie_header,
            Some(token_cost::default_since()),
            Some(chrono::Utc::now()),
        )
        .await
        {
            Ok(report) => Some(report),
            Err(err) => {
                tracing::debug!("Cursor token-cost events unavailable: {err}");
                None
            }
        }
    }

    fn build_usage_snapshot(
        primary: RateWindow,
        secondary: Option<RateWindow>,
        model_specific: Option<RateWindow>,
        email: Option<String>,
        plan_type: Option<String>,
        token_report: Option<&token_cost::CursorTokenCostReport>,
    ) -> UsageSnapshot {
        let mut usage = UsageSnapshot::new(primary);
        if let Some(sec) = secondary {
            usage = usage.with_secondary(sec);
        }
        if let Some(ms) = model_specific {
            usage = usage.with_model_specific(ms);
        }
        if let Some(e) = email {
            usage = usage.with_email(e);
        }
        if let Some(plan) = plan_type {
            usage = usage.with_login_method(plan);
        }
        if let Some(report) = token_report {
            for window in report.to_extra_windows() {
                usage.extra_rate_windows.push(window);
            }
        }
        usage
    }

    fn build_fetch_result(
        usage: UsageSnapshot,
        cost: Option<CostSnapshot>,
        token_report: Option<&token_cost::CursorTokenCostReport>,
        include_credits: bool,
    ) -> ProviderFetchResult {
        // On-demand / plan cost follows the shared optional-usage setting
        // (`FetchContext.include_credits` ↔ upstream showOptionalCreditsAndExtraUsage).
        let cost = if include_credits {
            token_report
                .and_then(|r| r.merge_into_cost(cost.clone()))
                .or(cost)
        } else {
            None
        };
        let mut result = ProviderFetchResult::new(usage, "web");
        if let Some(c) = cost {
            result = result.with_cost(c);
        }
        result
    }
}

impl Default for CursorProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for CursorProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Cursor
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Fetching Cursor usage via web API");

        match ctx.source_mode {
            // Cli is only ever set by the shell for "no cookie yet"; treat it as
            // web so empty-manual users get browser cookie attempt (or AuthRequired)
            // instead of "Source mode 'Cli' not supported" (#212).
            SourceMode::Auto | SourceMode::Web | SourceMode::Cli => {
                match self.fetch_web_usage(ctx).await {
                    Ok((
                        (primary, secondary, model_specific, cost, email, plan_type),
                        token_report,
                    )) => {
                        let usage = Self::build_usage_snapshot(
                            primary,
                            secondary,
                            model_specific,
                            email,
                            plan_type,
                            token_report.as_ref(),
                        );
                        Ok(Self::build_fetch_result(
                            usage,
                            cost,
                            token_report.as_ref(),
                            ctx.include_credits,
                        ))
                    }
                    Err(e) => {
                        tracing::warn!("Cursor API fetch failed: {}", e);
                        Err(e)
                    }
                }
            }
            SourceMode::OAuth => Err(ProviderError::UnsupportedSource(ctx.source_mode)),
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::Web]
    }

    fn supports_web(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::FetchContext;

    #[tokio::test]
    async fn cli_mode_does_not_return_unsupported_source() {
        let provider = CursorProvider::new();
        let ctx = FetchContext {
            source_mode: SourceMode::Cli,
            manual_cookie_header: None,
            ..FetchContext::default()
        };
        let err = provider
            .fetch_usage(&ctx)
            .await
            .expect_err("no cookies on this machine");
        // Must not be UnsupportedSource — that was the user-visible #212 bug.
        assert!(
            !matches!(err, ProviderError::UnsupportedSource(_)),
            "unexpected UnsupportedSource: {err}"
        );
        assert!(
            matches!(
                err,
                ProviderError::NoCookies | ProviderError::AuthRequired | ProviderError::Other(_)
            ),
            "expected cookie/auth style error, got: {err}"
        );
    }

    #[tokio::test]
    async fn oauth_mode_still_unsupported() {
        let provider = CursorProvider::new();
        let ctx = FetchContext {
            source_mode: SourceMode::OAuth,
            ..FetchContext::default()
        };
        let err = provider
            .fetch_usage(&ctx)
            .await
            .expect_err("oauth unsupported");
        assert!(matches!(
            err,
            ProviderError::UnsupportedSource(SourceMode::OAuth)
        ));
    }

    #[test]
    fn does_not_advertise_unsupported_credits() {
        let provider = CursorProvider::new();
        assert!(!provider.metadata().supports_credits);
    }

    #[test]
    fn on_demand_cost_follows_include_credits_setting() {
        let usage = UsageSnapshot::new(RateWindow::new(16.0));
        let cost = CostSnapshot::new(3.5, "USD", "On-demand (billing cycle)").with_limit(10.0);

        let shown =
            CursorProvider::build_fetch_result(usage.clone(), Some(cost.clone()), None, true);
        assert!(
            shown.cost.is_some(),
            "include_credits=true keeps on-demand cost"
        );

        let hidden = CursorProvider::build_fetch_result(usage, Some(cost), None, false);
        assert!(
            hidden.cost.is_none(),
            "include_credits=false hides on-demand extra usage"
        );
    }
}
