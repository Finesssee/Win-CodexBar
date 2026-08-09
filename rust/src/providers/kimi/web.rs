//! Kimi web (`kimi.com`) cookie auth and the web-token resolution chain.
//!
//! Upstream 0.48.0 policies ported here (#2623 / `KimiBrowserImportPolicy`):
//! browser cookie import — and reading the Kimi Desktop session store — are
//! disabled when the Kimi cookie source is `off`. The shared token chain is
//! manual cookie header → Kimi Desktop session → browser import (upstream
//! `KimiWebEnrichmentTokenResolver`), used both by the web fetch itself and
//! by the Code-API/CLI monthly enrichment.

use reqwest::Client;

use super::desktop_token::KimiDesktopAuthToken;
use super::{
    KIMI_COOKIE_DOMAINS, KIMI_SUBSCRIPTION_STATS_URL, KIMI_WEB_USAGE_URL, KimiProvider,
    KimiSubscriptionStatsResponse, KimiWebUsageResponse, apply_subscription_windows, kimi_web_post,
};
use crate::browser::cookies::get_cookie_header;
use crate::core::{ProviderError, ProviderId, UsageSnapshot};

/// Persisted Kimi cookie-source value ("manual" default; matches the
/// `claude`/settings convention of a fresh read at fetch time).
pub(crate) fn cookie_source() -> String {
    crate::settings::Settings::load()
        .cookie_source(ProviderId::Kimi)
        .to_string()
}

/// Upstream `KimiBrowserImportPolicy.allowsImport`: everything but `off`.
fn browser_import_allowed(cookie_source: &str) -> bool {
    !cookie_source.eq_ignore_ascii_case("off")
}

/// Web auth token chain for both the web fetch and the Code-API enrichment
/// (upstream `KimiWebEnrichmentTokenResolver.resolve`):
/// 1. Manual cookie header (its `kimi-auth`/auth cookie), source-independent.
/// 2. Kimi Desktop session token (skipped when cookie source is `off`).
/// 3. Browser cookie import (skipped when cookie source is `off`).
pub(crate) fn web_auth_token(manual_header: Option<&str>) -> Option<String> {
    resolve_web_token(WebTokenInput {
        manual_header,
        cookie_source: &cookie_source(),
        desktop_token: KimiDesktopAuthToken::load,
        browser_token: browser_auth_token,
    })
}

struct WebTokenInput<'a> {
    manual_header: Option<&'a str>,
    cookie_source: &'a str,
    desktop_token: fn() -> Option<String>,
    browser_token: fn() -> Option<String>,
}

fn resolve_web_token(input: WebTokenInput) -> Option<String> {
    if let Some(header) = input.manual_header
        && let Ok(token) = KimiProvider::auth_token_from_cookie_header(header)
    {
        return Some(token);
    }
    if !browser_import_allowed(input.cookie_source) {
        return None;
    }
    if let Some(token) = (input.desktop_token)() {
        return Some(token);
    }
    (input.browser_token)()
}

/// Browser import only: the first usable `kimi-auth`-class token from any of
/// the registered Kimi cookie domains.
fn browser_auth_token() -> Option<String> {
    KIMI_COOKIE_DOMAINS
        .iter()
        .find_map(|domain| {
            get_cookie_header(domain)
                .ok()
                .filter(|header| !header.is_empty())
        })
        .and_then(|header| KimiProvider::auth_token_from_cookie_header(&header).ok())
}

/// Fetch usage via Kimi web API (weekly quota + rate limit + subscription).
pub(crate) async fn fetch_via_web(
    cookie_header: Option<&str>,
) -> Result<UsageSnapshot, ProviderError> {
    let token = web_auth_token(cookie_header).ok_or_else(|| {
        if browser_import_allowed(&cookie_source()) {
            ProviderError::AuthRequired
        } else {
            ProviderError::Other(
                "Kimi cookie source is Off; provide a manual cookie header or enable browser import."
                    .into(),
            )
        }
    })?;

    let client = crate::core::credentialed_http_client_builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| ProviderError::Other(e.to_string()))?;

    let resp = kimi_web_post(
        &client,
        KIMI_WEB_USAGE_URL,
        &token,
        serde_json::json!({ "scope": ["FEATURE_CODING"] }),
    )
    .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(ProviderError::AuthRequired);
        }
        return Err(ProviderError::Other(format!("API error: {}", status)));
    }

    let usage: KimiWebUsageResponse = resp
        .json()
        .await
        .map_err(|e| ProviderError::Parse(e.to_string()))?;

    let subscription = match kimi_web_post(
        &client,
        KIMI_SUBSCRIPTION_STATS_URL,
        &token,
        serde_json::json!({}),
    )
    .await
    {
        Ok(response) if response.status().is_success() => response.json().await.ok(),
        _ => None,
    };

    snapshot_from_web_usage_response(usage, subscription)
}

pub(super) fn snapshot_from_web_usage_response(
    response: KimiWebUsageResponse,
    subscription: Option<KimiSubscriptionStatsResponse>,
) -> Result<UsageSnapshot, ProviderError> {
    let coding = response
        .usages
        .into_iter()
        .find(|usage| usage.scope == "FEATURE_CODING")
        .ok_or_else(|| ProviderError::Parse("Kimi FEATURE_CODING usage missing".into()))?;
    let primary = KimiProvider::rate_window_from_usage_detail(&coding.detail, Some(10080))?;
    let mut usage = UsageSnapshot::new(primary).with_login_method("Kimi");

    if let Some(limit) = coding.limits.unwrap_or_default().into_iter().next() {
        let window_minutes = limit.window.as_ref().and_then(super::kimi_window_minutes);
        let rate_limit =
            KimiProvider::rate_window_from_usage_detail(&limit.detail, window_minutes)?;
        usage = usage.with_secondary(rate_limit);
    }

    if let Some(subscription) = subscription.as_ref() {
        usage = apply_subscription_windows(usage, subscription);
    }

    Ok(usage)
}

// Kept for `code_api`: resolve the subscription stats snapshot with a web
// token; any failure means "no enrichment", never an error.
pub(super) async fn fetch_subscription_for_enrichment(
    client: &Client,
    token: &str,
) -> Option<KimiSubscriptionStatsResponse> {
    match kimi_web_post(
        client,
        KIMI_SUBSCRIPTION_STATS_URL,
        token,
        serde_json::json!({}),
    )
    .await
    {
        Ok(response) if response.status().is_success() => response.json().await.ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn static_desktop() -> Option<String> {
        Some("desktop-token".to_string())
    }

    fn static_browser() -> Option<String> {
        Some("browser-token".to_string())
    }

    fn no_token() -> Option<String> {
        None
    }

    fn input<'a>(
        manual_header: Option<&'a str>,
        cookie_source: &'a str,
        desktop_token: Option<&'static str>,
        browser_token: Option<&'static str>,
    ) -> WebTokenInput<'a> {
        WebTokenInput {
            manual_header,
            cookie_source,
            desktop_token: if desktop_token.is_some() {
                static_desktop
            } else {
                no_token
            },
            browser_token: if browser_token.is_some() {
                static_browser
            } else {
                no_token
            },
        }
    }

    #[test]
    fn manual_cookie_header_wins_regardless_of_source() {
        let token = resolve_web_token(input(
            Some("kimi-auth=manual-token"),
            "off",
            Some("desktop-token"),
            Some("browser-token"),
        ));
        assert_eq!(token.as_deref(), Some("manual-token"));
    }

    #[test]
    fn desktop_token_precedes_browser_import() {
        let token = resolve_web_token(input(
            None,
            "browser",
            Some("desktop-token"),
            Some("browser-token"),
        ));
        assert_eq!(token.as_deref(), Some("desktop-token"));
    }

    #[test]
    fn cookie_source_off_blocks_desktop_and_browser_but_not_manual() {
        assert_eq!(
            resolve_web_token(input(
                None,
                "off",
                Some("desktop-token"),
                Some("browser-token")
            )),
            None
        );
        assert_eq!(
            resolve_web_token(input(None, "off", None, Some("browser-token"))),
            None
        );
        assert_eq!(
            resolve_web_token(input(Some("kimi-auth=manual"), "off", None, None)).as_deref(),
            Some("manual")
        );
    }

    #[test]
    fn browser_token_used_when_desktop_absent() {
        let token = resolve_web_token(input(None, "auto", None, Some("browser-token")));
        assert_eq!(token.as_deref(), Some("browser-token"));
    }

    #[test]
    fn manual_default_source_still_allows_desktop_token() {
        // Upstream: desktop-session token applies for any non-off source;
        // the local default ("manual") must keep desktop sessions working.
        let token = resolve_web_token(input(None, "manual", Some("desktop-token"), None));
        assert_eq!(token.as_deref(), Some("desktop-token"));
    }

    #[test]
    fn browser_import_gate_is_case_insensitive() {
        assert!(!browser_import_allowed("OFF"));
        assert!(browser_import_allowed("browser"));
        assert!(browser_import_allowed("manual"));
    }
}
