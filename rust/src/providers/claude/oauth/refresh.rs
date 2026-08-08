//! OAuth token refresh HTTP call.
//!
//! POSTs `grant_type=refresh_token` to the OAuth token endpoint, mirroring the
//! Claude CLI's own refresh call, and builds the new credentials.

use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

use super::ClaudeOAuthCredentials;

/// OAuth token endpoint + client id used to refresh an expired access token.
/// Mirrors the Claude CLI's own prod `TOKEN_URL` / `CLIENT_ID`.
const TOKEN_REFRESH_URL: &str = "https://platform.claude.com/v1/oauth/token";
const OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const OAUTH_BETA_HEADER: &str = "oauth-2025-04-20";
/// Fallback access-token lifetime if a refresh response omits `expires_in`.
const DEFAULT_ACCESS_TTL_SECS: i64 = 3600;

/// Response from the OAuth token refresh endpoint (`grant_type=refresh_token`).
#[derive(Debug, Deserialize)]
struct RefreshTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    scope: Option<String>,
}

/// How a failed refresh attempt is classified for backoff (upstream 0.48.0
/// #2650): a 4xx rejection of the stored refresh token is *terminal* — the
/// same grant can never succeed, no matter how often we retry — while
/// transport errors, timeouts, and 5xx responses may be transient. Routing the
/// two classes onto different cooldowns keeps a dead token from hammering the
/// refresh endpoint on every usage poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RefreshFailureKind {
    Terminal,
    Transient,
}

/// A failed refresh attempt with its retry classification.
#[derive(Debug)]
pub(super) struct RefreshFailure {
    pub(super) kind: RefreshFailureKind,
    pub(super) message: String,
}

impl RefreshFailure {
    fn transient(message: impl Into<String>) -> Self {
        Self {
            kind: RefreshFailureKind::Transient,
            message: message.into(),
        }
    }

    fn from_http_status(status: reqwest::StatusCode, body: &str) -> Self {
        let message = format!(
            "Token refresh failed ({status}): {}",
            body.chars().take(200).collect::<String>()
        );
        // Upstream `refreshFailureDisposition` (ClaudeOAuthCredentials.swift):
        // only HTTP 400/401 with an OAuth `error` of `invalid_grant` (case-
        // insensitive) is terminal -- the stored refresh token is dead for
        // good. 403, other 4xx, and 5xx are transient (a retry can still heal
        // them); 400/401 *without* invalid_grant is likewise transient.
        let kind = match status.as_u16() {
            400 | 401
                if extract_oauth_error(body)
                    .is_some_and(|err| err.eq_ignore_ascii_case("invalid_grant")) =>
            {
                RefreshFailureKind::Terminal
            }
            _ => RefreshFailureKind::Transient,
        };
        Self { kind, message }
    }
}

/// Parse the OAuth `error` field from a refresh-failure response body.
fn extract_oauth_error(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value.get("error")?.as_str().map(str::to_string)
}

/// POST `grant_type=refresh_token` to the OAuth token endpoint, mirroring the
/// Claude CLI's own refresh call, and build the new credentials.
pub(super) async fn refresh_access_token(
    client: &Client,
    refresh_token: &str,
    current: &ClaudeOAuthCredentials,
) -> Result<ClaudeOAuthCredentials, RefreshFailure> {
    let mut body = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": OAUTH_CLIENT_ID,
    });
    if !current.scopes.is_empty() {
        body["scope"] = serde_json::Value::String(current.scopes.join(" "));
    }

    let response = client
        .post(TOKEN_REFRESH_URL)
        .header("anthropic-beta", OAUTH_BETA_HEADER)
        .header("Accept", "application/json")
        .json(&body)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|err| RefreshFailure::transient(err.to_string()))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(RefreshFailure::from_http_status(status, &text));
    }

    let refreshed: RefreshTokenResponse = response
        .json()
        .await
        .map_err(|e| RefreshFailure::transient(format!("Failed to parse refresh response: {e}")))?;

    let access_token = refreshed.access_token.trim().to_string();
    if access_token.is_empty() {
        return Err(RefreshFailure::transient(
            "Token refresh returned an empty access token",
        ));
    }

    // The endpoint returns `expires_in`; if it is ever omitted, fall back to
    // a conservative TTL so the token is still treated as fresh for a bounded
    // window (and cached) instead of triggering a per-poll refresh storm.
    let ttl_secs = refreshed.expires_in.unwrap_or(DEFAULT_ACCESS_TTL_SECS);
    let expires_at = Some(Utc::now() + chrono::Duration::seconds(ttl_secs));

    let refresh_token = refreshed
        .refresh_token
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .or_else(|| current.refresh_token.clone());

    let scopes = refreshed
        .scope
        .map(|s| s.split_whitespace().map(str::to_string).collect::<Vec<_>>())
        .filter(|scopes| !scopes.is_empty())
        .unwrap_or_else(|| current.scopes.clone());

    Ok(ClaudeOAuthCredentials {
        access_token,
        refresh_token,
        expires_at,
        scopes,
        rate_limit_tier: current.rate_limit_tier.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::{RefreshFailure, RefreshFailureKind, RefreshTokenResponse};

    #[test]
    fn parses_refresh_token_response() {
        let resp: RefreshTokenResponse = serde_json::from_str(
            r#"{
                "access_token": "new-access",
                "refresh_token": "new-refresh",
                "expires_in": 28800,
                "scope": "user:inference user:profile",
                "token_type": "Bearer"
            }"#,
        )
        .expect("refresh response should parse");

        assert_eq!(resp.access_token, "new-access");
        assert_eq!(resp.refresh_token.as_deref(), Some("new-refresh"));
        assert_eq!(resp.expires_in, Some(28800));
        assert_eq!(resp.scope.as_deref(), Some("user:inference user:profile"));
    }

    // Upstream 0.48.0 #2650: only 400/401 with `error: invalid_grant` is
    // terminal -- the stored refresh token is dead for good. 403, other 4xx,
    // and 5xx stay transient (a retry can still heal them); 400/401 *without*
    // invalid_grant is likewise transient.
    #[test]
    fn invalid_grant_on_400_or_401_is_terminal() {
        for status in [400, 401] {
            let failure = RefreshFailure::from_http_status(
                reqwest::StatusCode::from_u16(status).unwrap(),
                r#"{"error":"invalid_grant"}"#,
            );
            assert_eq!(failure.kind, RefreshFailureKind::Terminal, "HTTP {status}");
        }
        // Case-insensitive match.
        let failure = RefreshFailure::from_http_status(
            reqwest::StatusCode::from_u16(400).unwrap(),
            r#"{"error":"INVALID_GRANT"}"#,
        );
        assert_eq!(failure.kind, RefreshFailureKind::Terminal);
    }

    #[test]
    fn forbidden_and_non_grant_4xx_stay_transient() {
        // 403 is never terminal, even with invalid_grant in the body.
        let failure = RefreshFailure::from_http_status(
            reqwest::StatusCode::from_u16(403).unwrap(),
            r#"{"error":"invalid_grant"}"#,
        );
        assert_eq!(failure.kind, RefreshFailureKind::Transient);
        // 400/401 with a different OAuth error are transient.
        for status in [400, 401] {
            let failure = RefreshFailure::from_http_status(
                reqwest::StatusCode::from_u16(status).unwrap(),
                r#"{"error":"invalid_client"}"#,
            );
            assert_eq!(failure.kind, RefreshFailureKind::Transient, "HTTP {status}");
        }
        // 400/401 with no parseable error field are transient.
        for status in [400, 401] {
            let failure = RefreshFailure::from_http_status(
                reqwest::StatusCode::from_u16(status).unwrap(),
                "busy",
            );
            assert_eq!(failure.kind, RefreshFailureKind::Transient, "HTTP {status}");
        }
    }

    #[test]
    fn refresh_server_and_rate_limit_errors_stay_transient() {
        for status in [408, 429, 500, 502, 503] {
            let failure = RefreshFailure::from_http_status(
                reqwest::StatusCode::from_u16(status).unwrap(),
                "busy",
            );
            assert_eq!(failure.kind, RefreshFailureKind::Transient, "HTTP {status}");
        }
    }
}
