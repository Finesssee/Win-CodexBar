//! Credential loading (environment / file / OS keyring), persistence of
//! refreshed tokens back to disk, and the in-memory refreshed-credentials
//! cache.
//!
//! The cache is keyed by [`CredentialSource`] so a refreshed token read from
//! one source (e.g. the credentials file) can never shadow credentials read
//! from a different source (e.g. an environment-provided token) that happens
//! to compare as "fresher" under the naive `expires_at` ordering.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use super::ClaudeOAuthCredentials;
use crate::core::ProviderError;

const CREDENTIALS_PATH: &str = ".claude/.credentials.json";
const KEYRING_SERVICE: &str = "Claude Code-credentials";
const ENV_TOKEN_KEY: &str = "CODEXBAR_CLAUDE_OAUTH_TOKEN";
const ENV_SCOPES_KEY: &str = "CODEXBAR_CLAUDE_OAUTH_SCOPES";

/// Monotonic counter to make the persist temp-file name unique per write, so
/// concurrent refreshes (multiple instances / overlapping polls) never share a
/// temp path.
static PERSIST_TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Identifies where a set of OAuth credentials was loaded from, so the
/// refreshed-credentials cache never mixes tokens across sources.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum CredentialSource {
    Environment,
    File(PathBuf),
    Keyring(String), // the account string that matched
}

/// In-memory cache of the most recently refreshed credentials, keyed by
/// [`CredentialSource`]. Consulted when a disk persist fails, so we don't hit
/// the refresh endpoint (and rotate the refresh token) on every poll.
static REFRESHED_CREDENTIALS: OnceLock<Mutex<HashMap<CredentialSource, ClaudeOAuthCredentials>>> =
    OnceLock::new();

/// Raw JSON structure from Claude CLI credentials file
#[derive(Debug, Deserialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<OAuthData>,
}

#[derive(Debug, Deserialize)]
struct OAuthData {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    #[serde(rename = "refreshToken")]
    refresh_token: Option<String>,
    #[serde(rename = "expiresAt")]
    expires_at: Option<f64>, // milliseconds since epoch
    scopes: Option<Vec<String>>,
    #[serde(rename = "rateLimitTier")]
    rate_limit_tier: Option<String>,
}

fn refreshed_cache() -> &'static Mutex<HashMap<CredentialSource, ClaudeOAuthCredentials>> {
    REFRESHED_CREDENTIALS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Look up a cached refreshed credential for `source`, returning it only if
/// it is fresher than `file_creds` (i.e. what was just re-read from disk).
pub(super) fn cached_refreshed_if_fresher(
    source: &CredentialSource,
    file_creds: &ClaudeOAuthCredentials,
) -> Option<ClaudeOAuthCredentials> {
    let guard = refreshed_cache().lock().ok()?;
    let cached = guard.get(source)?;
    let fresher = match (cached.expires_at, file_creds.expires_at) {
        (Some(cached_at), Some(file_at)) => cached_at > file_at,
        (Some(_), None) => true,
        _ => false,
    };
    fresher.then(|| cached.clone())
}

/// Store `credentials` in the refreshed-credentials cache under `source`.
pub(super) fn store_refreshed(source: &CredentialSource, credentials: &ClaudeOAuthCredentials) {
    if let Ok(mut guard) = refreshed_cache().lock() {
        guard.insert(source.clone(), credentials.clone());
    }
}

/// Load OAuth credentials from environment, file, or Claude Code's OS credential store.
pub(super) fn load_credentials() -> Result<(ClaudeOAuthCredentials, CredentialSource), ProviderError>
{
    // Try environment variables first
    if let Some(creds) = load_from_environment() {
        return Ok((creds, CredentialSource::Environment));
    }

    // Claude Code's own credential stores require explicit consent.
    if !crate::providers::claude::claude_code_consent() {
        return Err(ProviderError::OAuth(
            "Reading Claude Code's credentials is off. Enable \"Allow reading Claude Code's credentials\" in Settings → Providers → Claude to use OAuth, or rely on Auto/CLI usage.".to_string(),
        ));
    }

    // Try credentials file
    let file_error = match load_from_file() {
        Ok(creds) => return Ok((creds, CredentialSource::File(credentials_path()?))),
        Err(err) => err,
    };

    // Current Claude Code builds store the same JSON payload in the OS credential store.
    if let Some((creds, source)) = load_from_keyring()? {
        return Ok((creds, source));
    }

    Err(file_error)
}

/// Load credentials from environment variables
fn load_from_environment() -> Option<ClaudeOAuthCredentials> {
    let token = std::env::var(ENV_TOKEN_KEY).ok()?;
    let token = token.trim();
    if token.is_empty() {
        return None;
    }

    let scopes: Vec<String> = std::env::var(ENV_SCOPES_KEY)
        .ok()
        .map(|s| {
            s.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_else(|| vec!["user:profile".to_string()]);

    Some(ClaudeOAuthCredentials {
        access_token: token.to_string(),
        refresh_token: None,
        expires_at: None, // Environment tokens don't expire
        scopes,
        rate_limit_tier: None,
    })
}

/// Load credentials from ~/.claude/.credentials.json
fn load_from_file() -> Result<ClaudeOAuthCredentials, ProviderError> {
    let path = credentials_path()?;

    if !path.exists() {
        return Err(ProviderError::OAuth(
            "Claude OAuth credentials not found. Run `claude` to authenticate.".to_string(),
        ));
    }

    let content = std::fs::read_to_string(&path)
        .map_err(|e| ProviderError::OAuth(format!("Failed to read credentials file: {}", e)))?;

    parse_credentials_json(&content)
}

/// Load credentials from Claude Code's OS keychain / credential manager entry.
fn load_from_keyring() -> Result<Option<(ClaudeOAuthCredentials, CredentialSource)>, ProviderError>
{
    for account in keyring_account_candidates() {
        let entry = match keyring::Entry::new(KEYRING_SERVICE, &account) {
            Ok(entry) => entry,
            Err(err) => {
                tracing::debug!(
                    "Failed to open Claude Code credential entry for account {}: {}",
                    account,
                    err
                );
                continue;
            }
        };

        let content = match entry.get_password() {
            Ok(content) => content,
            Err(keyring::Error::NoEntry) => continue,
            Err(keyring::Error::Ambiguous(_)) => continue,
            Err(err) => {
                tracing::debug!(
                    "Failed to read Claude Code credential entry for account {}: {}",
                    account,
                    err
                );
                continue;
            }
        };

        if content.trim().is_empty() {
            continue;
        }

        return parse_credentials_json(&content)
            .map(|creds| Some((creds, CredentialSource::Keyring(account))));
    }

    #[cfg(target_os = "macos")]
    if let Some(result) = load_from_macos_security_cli()? {
        return Ok(Some(result));
    }

    Ok(None)
}

#[cfg(target_os = "macos")]
fn load_from_macos_security_cli()
-> Result<Option<(ClaudeOAuthCredentials, CredentialSource)>, ProviderError> {
    for account in keyring_account_candidates() {
        let output = match std::process::Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-s",
                KEYRING_SERVICE,
                "-a",
                &account,
                "-w",
            ])
            .output()
        {
            Ok(output) => output,
            Err(err) => {
                tracing::debug!(
                    "Failed to run macOS security CLI for Claude credentials: {}",
                    err
                );
                continue;
            }
        };

        if !output.status.success() {
            continue;
        }

        let content = String::from_utf8_lossy(&output.stdout);
        if content.trim().is_empty() {
            continue;
        }

        return parse_credentials_json(content.trim())
            .map(|creds| Some((creds, CredentialSource::Keyring(account))));
    }

    Ok(None)
}

fn parse_credentials_json(content: &str) -> Result<ClaudeOAuthCredentials, ProviderError> {
    if let Ok(file) = serde_json::from_str::<CredentialsFile>(content)
        && let Some(oauth) = file.claude_ai_oauth
    {
        return credentials_from_oauth_data(oauth);
    }

    let oauth: OAuthData = serde_json::from_str(content)
        .map_err(|e| ProviderError::OAuth(format!("Invalid credentials format: {}", e)))?;
    credentials_from_oauth_data(oauth)
}

fn credentials_from_oauth_data(oauth: OAuthData) -> Result<ClaudeOAuthCredentials, ProviderError> {
    let access_token = oauth.access_token.ok_or_else(|| {
        ProviderError::OAuth(
            "Claude OAuth access token missing. Run `claude` to authenticate.".to_string(),
        )
    })?;

    let access_token = access_token.trim().to_string();
    if access_token.is_empty() {
        return Err(ProviderError::OAuth(
            "Claude OAuth access token is empty. Run `claude` to authenticate.".to_string(),
        ));
    }

    // Convert milliseconds to DateTime
    let expires_at = oauth.expires_at.map(|millis| {
        // OAuth expiry is stored in whole seconds; sub-second precision is dropped.
        #[allow(
            clippy::cast_possible_truncation,
            reason = "expiry is tracked at whole-second granularity"
        )]
        let secs = (millis / 1000.0) as i64;
        DateTime::from_timestamp(secs, 0).unwrap_or_else(Utc::now)
    });

    Ok(ClaudeOAuthCredentials {
        access_token,
        refresh_token: oauth.refresh_token,
        expires_at,
        scopes: oauth.scopes.unwrap_or_default(),
        rate_limit_tier: oauth.rate_limit_tier,
    })
}

fn keyring_account_candidates() -> Vec<String> {
    let mut candidates = Vec::new();
    for key in ["USER", "USERNAME"] {
        if let Ok(value) = std::env::var(key) {
            push_keyring_candidate(&mut candidates, value);
        }
    }

    #[cfg(not(windows))]
    {
        if let Ok(output) = std::process::Command::new("whoami").output()
            && output.status.success()
        {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            push_keyring_candidate(&mut candidates, value.clone());
            if let Some((_, username)) = value.rsplit_once('\\') {
                push_keyring_candidate(&mut candidates, username.to_string());
            }
            if let Some((_, username)) = value.rsplit_once('/') {
                push_keyring_candidate(&mut candidates, username.to_string());
            }
        }
    }

    candidates
}

fn push_keyring_candidate(candidates: &mut Vec<String>, value: String) {
    let value = value.trim();
    if value.is_empty() || candidates.iter().any(|candidate| candidate == value) {
        return;
    }
    candidates.push(value.to_string());
}

/// Get the credentials file path
fn credentials_path() -> Result<PathBuf, ProviderError> {
    dirs::home_dir()
        .map(|home| home.join(CREDENTIALS_PATH))
        .ok_or_else(|| ProviderError::OAuth("Could not find home directory".to_string()))
}

/// Persist refreshed tokens back to `~/.claude/.credentials.json`, updating
/// only the `claudeAiOauth` token fields and leaving everything else (e.g.
/// `mcpOAuth`) untouched. Written atomically via a temp file + rename.
pub(super) fn persist_refreshed_credentials(
    credentials: &ClaudeOAuthCredentials,
) -> Result<(), ProviderError> {
    // Never rotate Claude Code's refresh-token chain without explicit
    // consent (#2745): skipping the persist keeps CodexBar read-only.
    if !crate::providers::claude::claude_code_consent() {
        return Ok(());
    }

    let path = credentials_path()?;
    if !path.exists() {
        // Loaded from keyring/env; there is no file to update.
        return Ok(());
    }

    let content = std::fs::read_to_string(&path)
        .map_err(|e| ProviderError::OAuth(format!("Failed to read credentials file: {e}")))?;
    let mut root: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| ProviderError::OAuth(format!("Failed to parse credentials file: {e}")))?;

    apply_refresh_to_credentials_json(&mut root, credentials)?;

    let serialized = serde_json::to_string_pretty(&root)
        .map_err(|e| ProviderError::OAuth(format!("Failed to serialize credentials: {e}")))?;

    let parent = path
        .parent()
        .ok_or_else(|| ProviderError::OAuth("Credentials path has no parent".to_string()))?;
    let tmp = parent.join(format!(
        ".credentials.json.codexbar-tmp.{}.{}",
        std::process::id(),
        PERSIST_TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(&tmp, serialized.as_bytes())
        .map_err(|e| ProviderError::OAuth(format!("Failed to write credentials temp file: {e}")))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| ProviderError::OAuth(format!("Failed to replace credentials file: {e}")))?;
    Ok(())
}

/// Pure JSON merge used by [`persist_refreshed_credentials`]. Updates only
/// the token fields inside `claudeAiOauth`.
fn apply_refresh_to_credentials_json(
    root: &mut serde_json::Value,
    credentials: &ClaudeOAuthCredentials,
) -> Result<(), ProviderError> {
    let oauth = root
        .get_mut("claudeAiOauth")
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| {
            ProviderError::OAuth("credentials file missing claudeAiOauth object".to_string())
        })?;

    oauth.insert(
        "accessToken".to_string(),
        serde_json::Value::String(credentials.access_token.clone()),
    );
    if let Some(refresh_token) = &credentials.refresh_token {
        oauth.insert(
            "refreshToken".to_string(),
            serde_json::Value::String(refresh_token.clone()),
        );
    }
    if let Some(expires_at) = credentials.expires_at {
        oauth.insert(
            "expiresAt".to_string(),
            serde_json::Value::Number(expires_at.timestamp_millis().into()),
        );
    }
    if !credentials.scopes.is_empty() {
        oauth.insert(
            "scopes".to_string(),
            serde_json::Value::Array(
                credentials
                    .scopes
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        CredentialSource, apply_refresh_to_credentials_json, cached_refreshed_if_fresher,
        parse_credentials_json, store_refreshed,
    };
    use crate::providers::claude::oauth::ClaudeOAuthCredentials;

    #[test]
    fn parses_claude_code_credentials_payload() {
        let credentials = parse_credentials_json(
            r#"{
                "claudeAiOauth": {
                    "accessToken": "token",
                    "refreshToken": "refresh",
                    "expiresAt": 1770000000000,
                    "scopes": ["user:profile"],
                    "rateLimitTier": "default_claude_ai"
                }
            }"#,
        )
        .expect("Claude Code credential payload should parse");

        assert_eq!(credentials.access_token, "token");
        assert_eq!(credentials.refresh_token.as_deref(), Some("refresh"));
        assert_eq!(credentials.scopes, vec!["user:profile"]);
        assert_eq!(
            credentials.rate_limit_tier.as_deref(),
            Some("default_claude_ai")
        );
        assert!(credentials.expires_at.is_some());
    }

    #[test]
    fn parses_direct_oauth_credentials_payload() {
        let credentials = parse_credentials_json(
            r#"{
                "accessToken": "token",
                "scopes": ["user:profile"]
            }"#,
        )
        .expect("direct OAuth payload should parse");

        assert_eq!(credentials.access_token, "token");
        assert_eq!(credentials.scopes, vec!["user:profile"]);
    }

    #[test]
    fn rejects_credentials_payload_without_access_token() {
        let error = parse_credentials_json(
            r#"{
                "claudeAiOauth": {
                    "refreshToken": "refresh",
                    "scopes": ["user:profile"]
                }
            }"#,
        )
        .expect_err("access token is required");

        assert!(
            error
                .to_string()
                .contains("Claude OAuth access token missing")
        );
    }

    #[test]
    fn apply_refresh_updates_only_oauth_block_and_preserves_others() {
        let mut root: serde_json::Value = serde_json::from_str(
            r#"{
                "mcpOAuth": {"some-server": {"accessToken": "keepme"}},
                "claudeAiOauth": {
                    "accessToken": "old",
                    "refreshToken": "old-refresh",
                    "expiresAt": 1000,
                    "scopes": ["user:profile"],
                    "subscriptionType": "max"
                }
            }"#,
        )
        .unwrap();

        let creds = ClaudeOAuthCredentials {
            access_token: "fresh-access".to_string(),
            refresh_token: Some("fresh-refresh".to_string()),
            expires_at: chrono::DateTime::from_timestamp(2_000, 0),
            scopes: vec!["user:profile".to_string(), "user:inference".to_string()],
            rate_limit_tier: None,
        };

        apply_refresh_to_credentials_json(&mut root, &creds).unwrap();

        // Unrelated top-level blocks are preserved untouched.
        assert_eq!(root["mcpOAuth"]["some-server"]["accessToken"], "keepme");
        // Non-token fields inside claudeAiOauth are preserved.
        assert_eq!(root["claudeAiOauth"]["subscriptionType"], "max");
        // Token fields are updated.
        assert_eq!(root["claudeAiOauth"]["accessToken"], "fresh-access");
        assert_eq!(root["claudeAiOauth"]["refreshToken"], "fresh-refresh");
        assert_eq!(root["claudeAiOauth"]["expiresAt"], 2_000_000i64);
        assert_eq!(
            root["claudeAiOauth"]["scopes"],
            serde_json::json!(["user:profile", "user:inference"])
        );
    }

    /// Regression test for the cross-source cache contamination bug: an
    /// environment-provided token (which has no `expires_at`) must never be
    /// shadowed by a cached refreshed credential that came from the
    /// credentials *file* source, even though the naive `(Some(_), None) =>
    /// true` freshness rule would treat any file-cached value as "fresher"
    /// than an env token with no expiry.
    #[test]
    fn env_source_not_shadowed_by_file_cache() {
        // Distinct, unique source key so this test can't collide with other
        // tests touching the same process-global cache when run in parallel.
        let file_source = CredentialSource::File(std::path::PathBuf::from(
            "env_source_not_shadowed_by_file_cache-unique-marker.json",
        ));

        let file_cached_creds = ClaudeOAuthCredentials {
            access_token: "file-refreshed-token".to_string(),
            refresh_token: Some("file-refresh".to_string()),
            expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
            scopes: vec!["user:profile".to_string()],
            rate_limit_tier: None,
        };
        store_refreshed(&file_source, &file_cached_creds);

        let env_creds = ClaudeOAuthCredentials {
            access_token: "env-token".to_string(),
            refresh_token: None,
            expires_at: None,
            scopes: vec!["user:profile".to_string()],
            rate_limit_tier: None,
        };

        // Looking up under the Environment source must not see the File
        // source's cached (and "fresher"-by-the-naive-rule) entry.
        let result = cached_refreshed_if_fresher(&CredentialSource::Environment, &env_creds);
        assert!(
            result.is_none(),
            "environment credentials must not be shadowed by a file-sourced cache entry"
        );

        // Sanity check: the file source's own cache entry is still there and
        // still considered fresher than a file-read with no expiry.
        let file_disk_creds = ClaudeOAuthCredentials {
            access_token: "file-disk-token".to_string(),
            refresh_token: Some("file-disk-refresh".to_string()),
            expires_at: None,
            scopes: vec!["user:profile".to_string()],
            rate_limit_tier: None,
        };
        let same_source_result = cached_refreshed_if_fresher(&file_source, &file_disk_creds);
        assert_eq!(
            same_source_result.map(|c| c.access_token),
            Some("file-refreshed-token".to_string())
        );
    }
}
