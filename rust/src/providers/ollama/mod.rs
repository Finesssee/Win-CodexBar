//! Ollama provider implementation
//!
//! Fetches usage data by scraping the Ollama settings page
//! Uses session cookies from browser or manual input

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use regex_lite::Regex;
use reqwest::Url;
use serde::Deserialize;

use crate::browser::cookies::{Cookie, CookieExtractor};
use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};
use crate::settings::ApiKeys;

/// Ollama settings page URL
const OLLAMA_SETTINGS_URL: &str = "https://ollama.com/settings";
const OLLAMA_TAGS_URL: &str = "https://ollama.com/api/tags";
const OLLAMA_VALIDATION_URL: &str = "https://ollama.com/api/web_search";
const OLLAMA_COOKIE_DOMAIN: &str = "ollama.com";
const OLLAMA_SESSION_COOKIE_NAME: &str = "__Secure-session";
const OLLAMA_SESSION_COOKIE_NAMES: &[&str] = &[
    "session",
    OLLAMA_SESSION_COOKIE_NAME,
    "ollama_session",
    "__Host-ollama_session",
    "wos-session",
    "__Secure-next-auth.session-token",
    "next-auth.session-token",
];

/// Ollama provider
pub struct OllamaProvider {
    metadata: ProviderMetadata,
}

#[derive(Debug, Clone, PartialEq)]
struct UsageBlock {
    used_percent: f64,
    window_minutes: Option<u32>,
    resets_at: Option<DateTime<Utc>>,
    reset_description: Option<String>,
}

enum OllamaCookieSource {
    Manual(String),
    Browser(Vec<Cookie>),
}

impl OllamaCookieSource {
    fn header_for_url(&self, url: &Url) -> Option<String> {
        match self {
            Self::Manual(header) => should_attach_ollama_cookie(url).then(|| header.clone()),
            Self::Browser(cookies) => ollama_cookie_header_for_url(cookies, url),
        }
    }
}

impl OllamaProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Ollama,
                display_name: "Ollama",
                session_label: "Session",
                weekly_label: "Weekly",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://ollama.com/settings"),
                status_page_url: None,
            },
        }
    }

    /// Fetch usage by scraping ollama.com/settings
    async fn fetch_usage_web(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProviderError> {
        let cookies = self.resolve_cookie_source(ctx)?;

        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(ctx.web_timeout))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;
        let start_url =
            Url::parse(OLLAMA_SETTINGS_URL).map_err(|e| ProviderError::Other(e.to_string()))?;

        match fetch_settings_html_at(&client, &cookies, start_url.clone()).await {
            Ok(html) => {
                // Only cache non-manual browser/validated sessions for reuse.
                if ctx.manual_cookie_header.is_none() {
                    Self::cache_validated_session_cookie(&cookies);
                }
                self.parse_usage_html(&html)
            }
            Err(ProviderError::AuthRequired) if ctx.manual_cookie_header.is_none() => {
                // Cached/imported session expired — clear and re-import once.
                Self::invalidate_cached_session_cookie();
                let fresh = resolve_browser_cookie_header(true)?
                    .map(OllamaCookieSource::Manual)
                    .ok_or(ProviderError::AuthRequired)?;
                let html = fetch_settings_html_at(&client, &fresh, start_url).await?;
                Self::cache_validated_session_cookie(&fresh);
                self.parse_usage_html(&html)
            }
            Err(err) => Err(err),
        }
    }

    async fn fetch_usage_api(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProviderError> {
        let api_key = Self::resolve_api_key(ctx).ok_or(ProviderError::AuthRequired)?;
        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(ctx.web_timeout.max(1)))
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        let validation_url =
            Url::parse(OLLAMA_VALIDATION_URL).map_err(|e| ProviderError::Other(e.to_string()))?;
        let tags_url =
            Url::parse(OLLAMA_TAGS_URL).map_err(|e| ProviderError::Other(e.to_string()))?;
        Self::fetch_usage_api_at(&client, &api_key, validation_url, tags_url).await
    }

    async fn fetch_usage_api_at(
        client: &reqwest::Client,
        api_key: &str,
        validation_url: Url,
        tags_url: Url,
    ) -> Result<UsageSnapshot, ProviderError> {
        let api_key = clean_secret(Some(api_key)).ok_or(ProviderError::AuthRequired)?;
        if !crate::core::is_same_origin(&validation_url, &tags_url) {
            return Err(ProviderError::Other(
                "Ollama API endpoints must share an origin.".to_string(),
            ));
        }

        let validation = client
            .post(validation_url)
            .bearer_auth(&api_key)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("User-Agent", "CodexBar/1.0")
            .body(r#"{"query":""}"#)
            .send()
            .await?;
        match validation.status() {
            reqwest::StatusCode::OK | reqwest::StatusCode::BAD_REQUEST => {}
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
                return Err(ollama_api_key_error());
            }
            status => {
                return Err(ProviderError::Other(format!(
                    "Ollama API validation returned status {}",
                    status.as_u16()
                )));
            }
        }

        let response = client
            .get(tags_url)
            .bearer_auth(&api_key)
            .header("Accept", "application/json")
            .header("User-Agent", "CodexBar/1.0")
            .send()
            .await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        match status {
            reqwest::StatusCode::OK => Self::parse_api_tags(&bytes),
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
                Err(ollama_api_key_error())
            }
            _ => Err(ProviderError::Other(format!(
                "Ollama API returned status {status}"
            ))),
        }
    }

    fn resolve_api_key(ctx: &FetchContext) -> Option<String> {
        ctx.api_key
            .as_deref()
            .and_then(|key| clean_secret(Some(key)))
            .or_else(|| {
                ["OLLAMA_API_KEY", "OLLAMA_KEY"].iter().find_map(|name| {
                    std::env::var(name)
                        .ok()
                        .and_then(|value| clean_secret(Some(&value)))
                })
            })
            .or_else(|| {
                ApiKeys::load()
                    .get("ollama")
                    .and_then(|key| clean_secret(Some(key)))
            })
    }

    fn has_api_key(ctx: &FetchContext) -> bool {
        ctx.api_key
            .as_deref()
            .and_then(|key| clean_secret(Some(key)))
            .is_some()
            || ["OLLAMA_API_KEY", "OLLAMA_KEY"].iter().any(|name| {
                std::env::var(name)
                    .ok()
                    .and_then(|value| clean_secret(Some(&value)))
                    .is_some()
            })
            || ApiKeys::load()
                .get("ollama")
                .and_then(|key| clean_secret(Some(key)))
                .is_some()
    }

    fn parse_api_tags(bytes: &[u8]) -> Result<UsageSnapshot, ProviderError> {
        #[derive(Deserialize)]
        struct TagsResponse {
            models: Vec<serde_json::Value>,
        }

        let response: TagsResponse = serde_json::from_slice(bytes)
            .map_err(|e| ProviderError::Parse(format!("Could not parse Ollama API tags: {e}")))?;
        let mut primary = RateWindow::new(0.0);
        primary.reset_description =
            Some(format!("{} cloud models available", response.models.len()));
        Ok(UsageSnapshot::new(primary).with_login_method("API key"))
    }

    fn normalize_cookie_header(input: &str) -> Option<String> {
        let mut header = input.trim();
        if header.is_empty() {
            return None;
        }

        if header
            .get(.."cookie:".len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("cookie:"))
        {
            header = header["cookie:".len()..].trim();
        }

        if header.is_empty() {
            return None;
        }

        if header.contains('=') {
            Some(header.to_string())
        } else {
            Some(format!("{OLLAMA_SESSION_COOKIE_NAME}={header}"))
        }
    }

    /// Resolve cookies from manual cookies, validated cache, or browser import.
    ///
    /// Upstream #2404: reuse the last validated browser session cookie header
    /// across refreshes until auth fails, then re-import.
    fn resolve_cookie_source(
        &self,
        ctx: &FetchContext,
    ) -> Result<OllamaCookieSource, ProviderError> {
        // Check manual cookie header first
        if let Some(cookie) = &ctx.manual_cookie_header
            && let Some(header) = Self::normalize_cookie_header(cookie)
        {
            return has_recognized_ollama_session_cookie(&header)
                .then_some(OllamaCookieSource::Manual(header))
                .ok_or(ProviderError::NoCookies);
        }

        match resolve_browser_cookie_header(false)? {
            Some(header) => Ok(OllamaCookieSource::Manual(header)),
            None => Err(ProviderError::NoCookies),
        }
    }

    /// After a successful web fetch, cache the validated browser/manual session header.
    fn cache_validated_session_cookie(source: &OllamaCookieSource) {
        use crate::browser::cookie_cache::CookieHeaderCache;
        if let Some(header) = source.header_for_url(
            &Url::parse(OLLAMA_SETTINGS_URL)
                .unwrap_or_else(|_| Url::parse("https://ollama.com/settings").expect("static url")),
        ) {
            let label = match source {
                OllamaCookieSource::Manual(_) => "validated",
                OllamaCookieSource::Browser(_) => "browser",
            };
            let _ = CookieHeaderCache::store(ProviderId::Ollama, &header, label);
        }
    }

    /// Clear cached session after auth failure so the next refresh re-imports.
    fn invalidate_cached_session_cookie() {
        use crate::browser::cookie_cache::CookieHeaderCache;
        CookieHeaderCache::clear(ProviderId::Ollama);
    }
}

/// Resolve a browser/session cookie header for Ollama.
///
/// When `force_reimport` is false, prefers the last validated cached header
/// (upstream #2404). On force or cache miss, imports from the browser.
fn resolve_browser_cookie_header(force_reimport: bool) -> Result<Option<String>, ProviderError> {
    use crate::browser::cookie_cache::CookieHeaderCache;

    if !force_reimport
        && let Some(cached) = CookieHeaderCache::load(ProviderId::Ollama)
        && has_recognized_ollama_session_cookie(&cached.cookie_header)
    {
        return Ok(Some(cached.cookie_header));
    }

    match crate::providers::browser_cookies_for_domain(OLLAMA_COOKIE_DOMAIN) {
        Ok(cookies) => {
            let url =
                Url::parse(OLLAMA_SETTINGS_URL).map_err(|e| ProviderError::Other(e.to_string()))?;
            Ok(ollama_cookie_header_for_url(&cookies, &url)
                .filter(|h| has_recognized_ollama_session_cookie(h)))
        }
        Err(ProviderError::NoCookies) => Ok(None),
        Err(err) => Err(err),
    }
}

/// Pure decision helper for the Ollama session reuse path (unit-tested).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OllamaSessionAction {
    UseCached,
    ReimportBrowser,
}

fn ollama_session_action(
    has_cached_validated: bool,
    auth_failed: bool,
    force_reimport: bool,
) -> OllamaSessionAction {
    if force_reimport || auth_failed || !has_cached_validated {
        OllamaSessionAction::ReimportBrowser
    } else {
        OllamaSessionAction::UseCached
    }
}

impl OllamaProvider {
    /// Parse usage data from the Ollama settings HTML page
    fn parse_usage_html(&self, html: &str) -> Result<UsageSnapshot, ProviderError> {
        // Check if we're signed out
        if html.contains("Sign in")
            && !html.contains("Cloud Usage")
            && !html.contains("Session usage")
        {
            return Err(ProviderError::AuthRequired);
        }

        let session_block =
            self.parse_usage_block(&["Session usage", "Hourly usage"], html, Some(5 * 60));
        let weekly_block = self.parse_usage_block(&["Weekly usage"], html, Some(7 * 24 * 60));

        if session_block.is_none() && weekly_block.is_none() {
            return Err(ProviderError::Parse(
                "Could not find usage data on Ollama settings page".to_string(),
            ));
        }

        let primary = rate_window_from_usage_block(session_block.as_ref());
        let mut usage = UsageSnapshot::new(primary);

        // Parse plan name
        if let Some(plan) = self.parse_plan_name(html) {
            usage = usage.with_login_method(&plan);
        }

        // Parse account email
        if let Some(email) = self.parse_account_email(html) {
            usage = usage.with_login_method(&email);
        }

        if let Some(weekly) = weekly_block.as_ref() {
            usage = usage.with_secondary(rate_window_from_usage_block(Some(weekly)));
        }

        Ok(usage)
    }

    /// Parse a usage block by looking for a label then extracting the percentage
    fn parse_usage_block(
        &self,
        labels: &[&str],
        html: &str,
        window_minutes: Option<u32>,
    ) -> Option<UsageBlock> {
        for label in labels {
            if let Some(pos) = html.find(label) {
                let tail = &html[pos..];
                let end = usage_block_end(tail, label).unwrap_or_else(|| tail.len().min(4000));
                let window = &tail[..end.min(tail.len())];

                // Try "XX% used" pattern
                let used_re = Regex::new(r"(\d+(?:\.\d+)?)\s*%\s*used").ok()?;
                if let Some(caps) = used_re.captures(window)
                    && let Ok(val) = caps[1].parse::<f64>()
                {
                    return Some(UsageBlock {
                        used_percent: val,
                        window_minutes,
                        resets_at: parse_first_datetime(window),
                        reset_description: parse_reset_description(window),
                    });
                }

                // Try "width: XX%" pattern (progress bar CSS)
                let width_re = Regex::new(r"width:\s*(\d+(?:\.\d+)?)%").ok()?;
                if let Some(caps) = width_re.captures(window)
                    && let Ok(val) = caps[1].parse::<f64>()
                {
                    return Some(UsageBlock {
                        used_percent: val,
                        window_minutes,
                        resets_at: parse_first_datetime(window),
                        reset_description: parse_reset_description(window),
                    });
                }
            }
        }
        None
    }

    /// Parse plan name from "Cloud Usage" section
    fn parse_plan_name(&self, html: &str) -> Option<String> {
        let re = Regex::new(r#"Cloud Usage\s*</span>\s*<span[^>]*>([^<]+)</span>"#).ok()?;
        re.captures(html)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().trim().to_string())
    }

    /// Parse account email from the page
    fn parse_account_email(&self, html: &str) -> Option<String> {
        let re = Regex::new(r#"[\w.+-]+@[\w-]+\.[\w.-]+"#).ok()?;
        re.find(html).map(|m| m.as_str().to_string())
    }
}

impl Default for OllamaProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for OllamaProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Ollama
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Fetching Ollama usage");

        match ctx.source_mode {
            SourceMode::Auto => {
                if Self::has_api_key(ctx)
                    && let Ok(usage) = self.fetch_usage_api(ctx).await
                {
                    return Ok(ProviderFetchResult::new(usage, "api"));
                }
                let usage = self.fetch_usage_web(ctx).await?;
                Ok(ProviderFetchResult::new(usage, "web"))
            }
            SourceMode::Web => {
                let usage = self.fetch_usage_web(ctx).await?;
                Ok(ProviderFetchResult::new(usage, "web"))
            }
            SourceMode::OAuth | SourceMode::Cli => {
                Err(ProviderError::UnsupportedSource(ctx.source_mode))
            }
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::Web]
    }

    fn supports_web(&self) -> bool {
        true
    }

    fn supports_cli(&self) -> bool {
        false
    }
}

fn clean_secret(raw: Option<&str>) -> Option<String> {
    let mut value = raw?.trim().to_string();
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        value.remove(0);
        value.pop();
    }
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn usage_block_end(tail: &str, current_label: &str) -> Option<usize> {
    ["Session usage", "Hourly usage", "Weekly usage"]
        .iter()
        .filter(|label| **label != current_label)
        .filter_map(|label| tail.get(current_label.len()..)?.find(label))
        .map(|idx| idx + current_label.len())
        .min()
        .map(|idx| idx.min(4000))
}

fn rate_window_from_usage_block(block: Option<&UsageBlock>) -> RateWindow {
    block
        .map(|block| {
            RateWindow::with_details(
                block.used_percent,
                block.window_minutes,
                block.resets_at,
                block.reset_description.clone(),
            )
        })
        .unwrap_or_else(|| RateWindow::new(0.0))
}

fn parse_first_datetime(html: &str) -> Option<DateTime<Utc>> {
    let re =
        Regex::new(r#"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})"#).ok()?;
    let raw = re.find(html)?.as_str();
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|date| date.with_timezone(&Utc))
}

fn parse_reset_description(html: &str) -> Option<String> {
    let re = Regex::new(r"(?i)(resets?\s+in\s+[^<\n\r]+|reset\s+[^<\n\r]+)").ok()?;
    re.find(html)
        .map(|m| strip_html_entities(m.as_str()).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn strip_html_entities(value: &str) -> String {
    value
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&#x2F;", "/")
}

fn should_attach_ollama_cookie(url: &Url) -> bool {
    url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case(OLLAMA_COOKIE_DOMAIN))
}

fn has_recognized_ollama_session_cookie(header: &str) -> bool {
    header.split(';').any(|pair| {
        let name = pair.trim().split_once('=').map(|(name, _)| name.trim());
        name.is_some_and(is_recognized_ollama_session_cookie_name)
    })
}

fn ollama_cookie_header_for_url(cookies: &[Cookie], url: &Url) -> Option<String> {
    let cookies: Vec<_> = cookies
        .iter()
        .filter(|cookie| cookie_applies_to_ollama_url(cookie, url))
        .cloned()
        .collect();
    let header = CookieExtractor::build_cookie_header(&cookies);
    has_recognized_ollama_session_cookie(&header).then_some(header)
}

fn cookie_applies_to_ollama_url(cookie: &Cookie, url: &Url) -> bool {
    let domain = cookie
        .domain
        .trim()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let path = if cookie.path.is_empty() {
        "/"
    } else {
        cookie.path.as_str()
    };
    let request_path = url.path();
    should_attach_ollama_cookie(url)
        && (domain == OLLAMA_COOKIE_DOMAIN
            || domain.strip_prefix('.') == Some(OLLAMA_COOKIE_DOMAIN))
        && (path == "/"
            || request_path == path
            || (request_path.starts_with(path)
                && (path.ends_with('/') || request_path.as_bytes().get(path.len()) == Some(&b'/'))))
}

fn is_recognized_ollama_session_cookie_name(name: &str) -> bool {
    OLLAMA_SESSION_COOKIE_NAMES.contains(&name)
        || is_chunked_nextauth_cookie_name(name, "__Secure-next-auth.session-token")
        || is_chunked_nextauth_cookie_name(name, "next-auth.session-token")
}

fn is_chunked_nextauth_cookie_name(name: &str, base_name: &str) -> bool {
    name.strip_prefix(base_name)
        .and_then(|suffix| suffix.strip_prefix('.'))
        .is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn is_ollama_sign_in_redirect(url: &Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
        return false;
    };
    let path = url.path().to_ascii_lowercase();
    if host == OLLAMA_COOKIE_DOMAIN || host == "www.ollama.com" {
        return path == "/signin" || path.starts_with("/signin/") || path.contains("/login");
    }
    host == "signin.ollama.com"
        || (host.ends_with(".workos.com") && path.starts_with("/user_management/authorize"))
}

async fn fetch_settings_html_at(
    client: &reqwest::Client,
    source: &OllamaCookieSource,
    start_url: Url,
) -> Result<String, ProviderError> {
    let mut current_url = start_url;

    for _ in 0..5 {
        let mut request = client
            .get(current_url.clone())
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36",
            );
        if let Some(cookie_header) = source.header_for_url(&current_url) {
            request = request.header("Cookie", cookie_header);
        }

        let response = request.send().await?;
        if response.status().is_redirection() {
            let Some(location) = response.headers().get(reqwest::header::LOCATION) else {
                return Err(ProviderError::Other(
                    "Ollama redirect missing Location header".to_string(),
                ));
            };
            let location = location
                .to_str()
                .map_err(|e| ProviderError::Other(e.to_string()))?;
            let next_url = current_url
                .join(location)
                .map_err(|e| ProviderError::Other(e.to_string()))?;
            if is_ollama_sign_in_redirect(&next_url)
                || !crate::core::is_same_origin(&current_url, &next_url)
            {
                return Err(ProviderError::AuthRequired);
            }
            current_url = next_url;
            continue;
        }

        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            || response.status() == reqwest::StatusCode::FORBIDDEN
            || is_ollama_sign_in_redirect(response.url())
        {
            return Err(ProviderError::AuthRequired);
        }
        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Ollama returned status {}",
                response.status()
            )));
        }
        return response
            .text()
            .await
            .map_err(|e| ProviderError::Other(e.to_string()));
    }

    Err(ProviderError::Other(
        "Ollama returned too many redirects".to_string(),
    ))
}

fn ollama_api_key_error() -> ProviderError {
    ProviderError::Other("Ollama API key is invalid or revoked.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_raw_ollama_session_cookie_value() {
        assert_eq!(
            OllamaProvider::normalize_cookie_header("abc123"),
            Some("__Secure-session=abc123".to_string())
        );
    }

    #[test]
    fn preserves_full_cookie_header() {
        assert_eq!(
            OllamaProvider::normalize_cookie_header("__Secure-session=abc123; aid=device"),
            Some("__Secure-session=abc123; aid=device".to_string())
        );
    }

    #[test]
    fn strips_cookie_header_prefix() {
        assert_eq!(
            OllamaProvider::normalize_cookie_header("Cookie: __Secure-session=abc123"),
            Some("__Secure-session=abc123".to_string())
        );
    }

    #[test]
    fn ignores_empty_cookie_input() {
        assert_eq!(OllamaProvider::normalize_cookie_header("   "), None);
        assert_eq!(OllamaProvider::normalize_cookie_header("Cookie:   "), None);
    }

    #[test]
    fn recognizes_exact_authkit_and_nextauth_session_cookie_names() {
        assert!(has_recognized_ollama_session_cookie(
            "wos-session=auth; theme=dark"
        ));
        assert!(has_recognized_ollama_session_cookie(
            "__Secure-next-auth.session-token.0=auth"
        ));
        assert!(!has_recognized_ollama_session_cookie(
            "notwos-session=auth; theme=dark"
        ));
        assert!(!has_recognized_ollama_session_cookie(
            "next-auth.session-token.evil=auth"
        ));
        assert!(!has_recognized_ollama_session_cookie("theme=dark"));
    }

    #[test]
    fn limits_browser_cookie_headers_to_ollama_settings_scope() {
        use crate::browser::cookies::Cookie;

        let cookie = |name: &str, domain: &str, path: &str| Cookie {
            name: name.to_string(),
            value: "test".to_string(),
            domain: domain.to_string(),
            path: path.to_string(),
            expires: None,
            is_secure: true,
            is_http_only: true,
        };
        let cookies = [
            cookie("wos-session", ".ollama.com", "/"),
            cookie("wos-session", "signin.ollama.com", "/"),
            cookie("__Secure-session", "ollama.com", "/signin"),
        ];

        assert_eq!(
            ollama_cookie_header_for_url(
                &cookies,
                &Url::parse("https://ollama.com/settings").unwrap()
            )
            .as_deref(),
            Some("wos-session=test")
        );
        assert_eq!(
            ollama_cookie_header_for_url(
                &[cookie("__Secure-session", "ollama.com", "/settings")],
                &Url::parse("https://ollama.com/api/tags").unwrap()
            ),
            None
        );
        assert_eq!(
            ollama_cookie_header_for_url(
                &[cookie("__Secure-session", "ollama.com", "/settings")],
                &Url::parse("https://ollama.com/settings/account").unwrap()
            )
            .as_deref(),
            Some("__Secure-session=test")
        );
        let source = OllamaCookieSource::Browser(vec![
            cookie("__Secure-session", "ollama.com", "/settings"),
            cookie("wos-session", "ollama.com", "/api"),
        ]);
        assert_eq!(
            source
                .header_for_url(&Url::parse("https://ollama.com/settings").unwrap())
                .as_deref(),
            Some("__Secure-session=test")
        );
        assert_eq!(
            source
                .header_for_url(&Url::parse("https://ollama.com/api/models").unwrap())
                .as_deref(),
            Some("wos-session=test")
        );
    }

    #[test]
    fn only_attaches_web_cookie_to_https_ollama_urls() {
        assert!(should_attach_ollama_cookie(
            &Url::parse("https://ollama.com/settings").unwrap()
        ));
        assert!(!should_attach_ollama_cookie(
            &Url::parse("http://ollama.com/settings").unwrap()
        ));
        assert!(!should_attach_ollama_cookie(
            &Url::parse("https://example.com/settings").unwrap()
        ));
    }

    #[test]
    fn recognizes_workos_signin_redirects_as_expired_sessions() {
        assert!(is_ollama_sign_in_redirect(
            &Url::parse("https://signin.ollama.com/?client_id=test").unwrap()
        ));
        assert!(is_ollama_sign_in_redirect(
            &Url::parse("https://auth.workos.com/user_management/authorize?client_id=test")
                .unwrap()
        ));
        assert!(!is_ollama_sign_in_redirect(
            &Url::parse("https://auth.workos.com/other").unwrap()
        ));
        assert!(!is_ollama_sign_in_redirect(
            &Url::parse("http://signin.ollama.com/").unwrap()
        ));
    }

    #[tokio::test]
    async fn settings_fetch_follows_same_origin_redirects() {
        let mut server = mockito::Server::new_async().await;
        let first = server
            .mock("GET", "/settings")
            .with_status(302)
            .with_header("location", "/settings/account")
            .create_async()
            .await;
        let second = server
            .mock("GET", "/settings/account")
            .with_status(200)
            .with_body("<html>usage</html>")
            .create_async()
            .await;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();

        let html = fetch_settings_html_at(
            &client,
            &OllamaCookieSource::Manual("__Secure-session=test".to_string()),
            Url::parse(&format!("{}/settings", server.url())).unwrap(),
        )
        .await
        .unwrap();

        first.assert_async().await;
        second.assert_async().await;
        assert_eq!(html, "<html>usage</html>");
    }

    #[tokio::test]
    async fn settings_fetch_stops_before_following_signin_or_workos_redirects() {
        for location in [
            "https://signin.ollama.com/?client_id=test",
            "https://auth.workos.com/user_management/authorize?client_id=test",
        ] {
            let mut server = mockito::Server::new_async().await;
            let first = server
                .mock("GET", "/settings")
                .with_status(302)
                .with_header("location", location)
                .create_async()
                .await;
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap();

            let error = fetch_settings_html_at(
                &client,
                &OllamaCookieSource::Manual("__Secure-session=test".to_string()),
                Url::parse(&format!("{}/settings", server.url())).unwrap(),
            )
            .await
            .unwrap_err();

            first.assert_async().await;
            assert!(matches!(error, ProviderError::AuthRequired));
        }
    }

    #[tokio::test]
    async fn settings_fetch_reports_redirect_exhaustion() {
        let mut server = mockito::Server::new_async().await;
        let redirect = server
            .mock("GET", "/settings")
            .expect(5)
            .with_status(302)
            .with_header("location", "/settings")
            .create_async()
            .await;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();

        let error = fetch_settings_html_at(
            &client,
            &OllamaCookieSource::Manual("__Secure-session=test".to_string()),
            Url::parse(&format!("{}/settings", server.url())).unwrap(),
        )
        .await
        .unwrap_err();

        redirect.assert_async().await;
        assert_eq!(error.to_string(), "Ollama returned too many redirects");
    }

    #[tokio::test]
    async fn validates_trimmed_key_before_fetching_public_model_catalog() {
        let mut server = mockito::Server::new_async().await;
        let validation = server
            .mock("POST", "/api/web_search")
            .match_header("authorization", "Bearer ollama-key")
            .match_header("content-type", "application/json")
            .match_body(r#"{"query":""}"#)
            .with_status(400)
            .create_async()
            .await;
        let catalog = server
            .mock("GET", "/api/tags")
            .match_header("authorization", "Bearer ollama-key")
            .with_status(200)
            .with_body(r#"{"models":[{"name":"gpt-oss"}]}"#)
            .create_async()
            .await;
        let client = reqwest::Client::new();

        let snapshot = OllamaProvider::fetch_usage_api_at(
            &client,
            "  ollama-key  ",
            Url::parse(&format!("{}/api/web_search", server.url())).unwrap(),
            Url::parse(&format!("{}/api/tags", server.url())).unwrap(),
        )
        .await
        .unwrap();

        validation.assert_async().await;
        catalog.assert_async().await;
        assert_eq!(snapshot.login_method.as_deref(), Some("API key"));
    }

    #[tokio::test]
    async fn rejects_unproven_validation_responses_before_catalog_fetch() {
        let mut server = mockito::Server::new_async().await;
        let validation = server
            .mock("POST", "/api/web_search")
            .with_status(422)
            .create_async()
            .await;
        let catalog = server
            .mock("GET", "/api/tags")
            .expect(0)
            .with_status(200)
            .create_async()
            .await;
        let client = reqwest::Client::new();

        let error = OllamaProvider::fetch_usage_api_at(
            &client,
            "ollama-key",
            Url::parse(&format!("{}/api/web_search", server.url())).unwrap(),
            Url::parse(&format!("{}/api/tags", server.url())).unwrap(),
        )
        .await
        .unwrap_err();

        validation.assert_async().await;
        catalog.assert_async().await;
        assert_eq!(
            error.to_string(),
            "Ollama API validation returned status 422"
        );
    }

    #[test]
    fn strips_wrapping_quotes_from_api_key() {
        assert_eq!(
            clean_secret(Some("  'ollama-key'  ")),
            Some("ollama-key".to_string())
        );
        assert_eq!(
            clean_secret(Some("  \"ollama-key\"  ")),
            Some("ollama-key".to_string())
        );
    }

    #[test]
    fn parses_api_tags_model_count() {
        let snapshot =
            OllamaProvider::parse_api_tags(br#"{"models":[{"name":"gpt-oss"},{"name":"qwen3"}]}"#)
                .unwrap();
        assert_eq!(snapshot.primary.used_percent, 0.0);
        assert_eq!(
            snapshot.primary.reset_description.as_deref(),
            Some("2 cloud models available")
        );
        assert_eq!(snapshot.login_method.as_deref(), Some("API key"));
    }

    #[test]
    fn api_auth_error_names_invalid_or_revoked_key() {
        assert_eq!(
            ollama_api_key_error().to_string(),
            "Ollama API key is invalid or revoked."
        );
    }

    #[test]
    fn parses_ollama_usage_blocks_with_window_bounds() {
        let provider = OllamaProvider::new();
        let html = r#"
            <section>Session usage <div style="width: 42%"></div><span>resets in 2h</span></section>
            <section>Weekly usage <span>84% used</span><time>2026-06-01T00:00:00Z</time></section>
        "#;
        let session = provider
            .parse_usage_block(&["Session usage", "Hourly usage"], html, Some(300))
            .unwrap();
        let weekly = provider
            .parse_usage_block(&["Weekly usage"], html, Some(10080))
            .unwrap();
        assert_eq!(session.used_percent, 42.0);
        assert_eq!(session.window_minutes, Some(300));
        assert_eq!(session.reset_description.as_deref(), Some("resets in 2h"));
        assert_eq!(weekly.used_percent, 84.0);
        assert!(weekly.resets_at.is_some());
    }

    #[test]
    fn session_action_reuses_cached_until_auth_fails() {
        assert_eq!(
            ollama_session_action(true, false, false),
            OllamaSessionAction::UseCached
        );
        assert_eq!(
            ollama_session_action(true, true, false),
            OllamaSessionAction::ReimportBrowser
        );
        assert_eq!(
            ollama_session_action(false, false, false),
            OllamaSessionAction::ReimportBrowser
        );
        assert_eq!(
            ollama_session_action(true, false, true),
            OllamaSessionAction::ReimportBrowser
        );
    }

    #[test]
    fn recognized_session_cookie_required_for_cache_reuse() {
        assert!(has_recognized_ollama_session_cookie(
            "__Secure-session=abc123; path=/"
        ));
        assert!(!has_recognized_ollama_session_cookie("foo=bar; baz=qux"));
    }
}
