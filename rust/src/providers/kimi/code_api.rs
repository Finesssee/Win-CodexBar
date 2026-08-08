//! Kimi Code API path (api-key + Kimi Code CLI credential), including the
//! upstream 0.48.0 enrichment (#2622): Code API / CLI usage snapshots are
//! merged with the monthly membership pool + Code 7-day limit when a
//! `kimi.com` web token is available (manual cookie → Kimi Desktop session →
//! browser import; gated by Cookie Source Off).

use reqwest::Url;
use std::path::{Path, PathBuf};

use super::web;
use super::{
    FetchContext, KimiCodeApiUsageResponse, KimiProvider, ProviderError, UsageSnapshot,
    ascii_header_value, cleaned_env, cleaned_owned, kimi_window_minutes,
};

const KIMI_CODE_API_BASE: &str = "https://api.kimi.com";
const KIMI_CODE_API_KEY_ENV: &str = "KIMI_CODE_API_KEY";
const KIMI_CODE_BASE_URL_ENV: &str = "KIMI_CODE_BASE_URL";
const KIMI_CODE_HOME_ENV: &str = "KIMI_CODE_HOME";
const KIMI_CODE_OAUTH_HOST_ENV: &str = "KIMI_CODE_OAUTH_HOST";
const KIMI_OAUTH_HOST_ENV: &str = "KIMI_OAUTH_HOST";
const KIMI_CODE_CLI_PLATFORM: &str = "kimi_code_cli";
/// CLI access tokens must remain valid for at least this long to be reused.
const KIMI_CODE_CREDENTIAL_MIN_TTL_SECS: f64 = 60.0;

#[derive(Debug, serde::Deserialize)]
struct KimiCodeCredentialFile {
    #[serde(default, alias = "accessToken")]
    access_token: String,
    #[serde(default)]
    #[allow(dead_code)]
    refresh_token: Option<String>,
    #[serde(default, alias = "expiresAt")]
    expires_at: Option<serde_json::Value>,
}

/// Fetch usage via the Kimi Code API; optionally enrich the snapshot with the
/// web membership pool (upstream #2622). Enrichment failures degrade silently
/// to the un-enriched snapshot.
pub(crate) async fn fetch_via_code_api(
    ctx: &FetchContext,
    api_key_override: Option<&str>,
    identity_headers_override: Option<&[(&str, String)]>,
    login_method: &str,
) -> Result<UsageSnapshot, ProviderError> {
    let api_key = code_api_key(api_key_override.or(ctx.api_key.as_deref()))?;
    let base_url = code_api_base_url()?;
    let endpoint = code_api_usage_endpoint(&base_url)?;
    let client = crate::core::credentialed_http_client_builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| ProviderError::Other(e.to_string()))?;

    let mut request = client
        .get(endpoint)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json");
    if let Some(headers) = identity_headers_override {
        for (name, value) in headers {
            request = request.header(*name, value);
        }
    }

    let resp = request.send().await?;

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED
        || resp.status() == reqwest::StatusCode::FORBIDDEN
    {
        return Err(ProviderError::AuthRequired);
    }
    if !resp.status().is_success() {
        return Err(ProviderError::Other(format!(
            "Kimi Code API returned status {}",
            resp.status()
        )));
    }

    let json: KimiCodeApiUsageResponse = resp.json().await.map_err(|e| {
        ProviderError::Parse(format!("Failed to parse Kimi Code API response: {e}"))
    })?;
    let mut snapshot = snapshot_from_code_api_response(json)?;
    snapshot.login_method = Some(login_method.to_string());

    // Upstream #2622: enrich Code API + CLI usage with the monthly membership
    // pool from a signed-in Kimi Desktop (or browser/manual) session.
    if let Some(web_token) = web::web_auth_token(ctx.manual_cookie_header.as_deref()) {
        match web::fetch_subscription_for_enrichment(&client, &web_token).await {
            Some(subscription) => {
                snapshot = super::apply_subscription_windows(snapshot, &subscription);
            }
            None => {
                tracing::debug!("Kimi Code monthly enrichment unavailable");
            }
        }
    }

    Ok(snapshot)
}

pub(super) fn snapshot_from_code_api_response(
    response: KimiCodeApiUsageResponse,
) -> Result<UsageSnapshot, ProviderError> {
    let primary = KimiProvider::rate_window_from_usage_detail(&response.usage, None)?;
    let mut usage = UsageSnapshot::new(primary).with_login_method("Code API");

    if let Some(limit) = response.limits.unwrap_or_default().into_iter().next() {
        let window_minutes = limit.window.as_ref().and_then(kimi_window_minutes);
        let rate_limit =
            KimiProvider::rate_window_from_usage_detail(&limit.detail, window_minutes)?;
        usage = usage.with_secondary(rate_limit);
    }

    Ok(usage)
}

pub(crate) fn code_api_key(explicit: Option<&str>) -> Result<String, ProviderError> {
    if let Some(key) = explicit.map(str::trim).filter(|key| !key.is_empty()) {
        return Ok(key.to_string());
    }
    cleaned_env(KIMI_CODE_API_KEY_ENV).ok_or(ProviderError::AuthRequired)
}

fn code_api_base_url() -> Result<Url, ProviderError> {
    let raw = cleaned_env(KIMI_CODE_BASE_URL_ENV).unwrap_or_else(|| KIMI_CODE_API_BASE.to_string());
    crate::providers::validated_https_url(&raw, "Kimi Code API base")
}

pub(super) fn code_api_usage_endpoint(base_url: &Url) -> Result<Url, ProviderError> {
    let base = base_url.as_str().trim_end_matches('/');
    let path = base_url.path().trim_matches('/');
    let endpoint = if path == "coding/v1" || path.ends_with("/coding/v1") {
        format!("{base}/usages")
    } else if path == "coding" || path.ends_with("/coding") {
        format!("{base}/v1/usages")
    } else {
        format!("{base}/coding/v1/usages")
    };
    Url::parse(&endpoint)
        .map_err(|_| ProviderError::Other("Kimi Code API usage endpoint is invalid".into()))
}

/// Whether env base/OAuth overrides mean we must not reuse CLI-owned credentials.
fn has_code_endpoint_override() -> bool {
    cleaned_env(KIMI_CODE_BASE_URL_ENV).is_some()
        || cleaned_env(KIMI_CODE_OAUTH_HOST_ENV).is_some()
        || cleaned_env(KIMI_OAUTH_HOST_ENV).is_some()
}

/// Home for Kimi Code CLI state (`%USERPROFILE%\.kimi-code` or `KIMI_CODE_HOME`).
pub(crate) fn kimi_code_home() -> Option<PathBuf> {
    if let Some(override_home) = cleaned_env(KIMI_CODE_HOME_ENV) {
        return Some(PathBuf::from(override_home));
    }
    dirs::home_dir().map(|home| home.join(".kimi-code"))
}

/// Read-only access to a still-fresh Kimi Code CLI access token.
///
/// Never refreshes or rewrites CLI-owned `credentials/kimi-code.json`.
/// Skips when `KIMI_CODE_BASE_URL` / OAuth host overrides are set.
pub(crate) fn kimi_code_cli_access_token(now_unix: f64) -> Option<String> {
    if has_code_endpoint_override() {
        return None;
    }
    let home = kimi_code_home()?;
    let credential = read_kimi_code_credential(&home)?;
    let token = cleaned_owned(credential.access_token)?;
    if !is_kimi_code_credential_fresh(credential.expires_at, now_unix) {
        return None;
    }
    Some(token)
}

pub(crate) fn kimi_code_cli_identity_headers(home: &Path) -> Vec<(&'static str, String)> {
    // Only send device id when the CLI file exists — never mint a fresh UUID
    // per fetch (unstable fingerprinting toward Moonshot).
    let device_id = read_kimi_code_device_id(home);
    let version = env!("CARGO_PKG_VERSION").to_string();
    let os_name = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let model = format!("{os_name} {arch}");
    let mut headers = vec![
        ("User-Agent", format!("CodexBar/{version}")),
        ("X-Msh-Platform", KIMI_CODE_CLI_PLATFORM.to_string()),
        ("X-Msh-Version", version),
        ("X-Msh-Device-Name", "codexbar".to_string()),
        ("X-Msh-Device-Model", ascii_header_value(&model)),
        ("X-Msh-Os-Version", ascii_header_value(os_name)),
    ];
    if let Some(device_id) = device_id {
        headers.push(("X-Msh-Device-Id", device_id));
    }
    headers
}

fn read_kimi_code_credential(home: &Path) -> Option<KimiCodeCredentialFile> {
    let path = home.join("credentials").join("kimi-code.json");
    let data = std::fs::read(path).ok()?;
    serde_json::from_slice(&data).ok()
}

fn read_kimi_code_device_id(home: &Path) -> Option<String> {
    let path = home.join("device_id");
    let raw = std::fs::read_to_string(path).ok()?;
    cleaned_owned(raw)
}

fn is_kimi_code_credential_fresh(expires_at: Option<serde_json::Value>, now_unix: f64) -> bool {
    let Some(expires) = super::value_as_f64(expires_at.as_ref()) else {
        return false;
    };
    if !expires.is_finite() {
        return false;
    }
    // Support both seconds and millisecond epoch values.
    let expires_secs = if expires > 10_000_000_000.0 {
        expires / 1000.0
    } else {
        expires
    };
    expires_secs > now_unix + KIMI_CODE_CREDENTIAL_MIN_TTL_SECS
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::LazyLock;

    static ENV_LOCK: LazyLock<std::sync::Mutex<()>> = LazyLock::new(|| std::sync::Mutex::new(()));

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn write_temp_kimi_code_home(
        access_token: &str,
        expires_at: Option<serde_json::Value>,
    ) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let credentials = dir.path().join("credentials");
        std::fs::create_dir_all(&credentials).expect("mkdir credentials");
        let mut payload = serde_json::Map::new();
        payload.insert("access_token".into(), json!(access_token));
        payload.insert("refresh_token".into(), json!("refresh"));
        if let Some(expires) = expires_at {
            payload.insert("expires_at".into(), expires);
        }
        std::fs::write(
            credentials.join("kimi-code.json"),
            serde_json::to_vec_pretty(&serde_json::Value::Object(payload)).unwrap(),
        )
        .expect("write credentials");
        dir
    }

    #[test]
    fn code_api_usage_endpoint_normalizes_base_paths() {
        let root = Url::parse("https://api.kimi.com").unwrap();
        assert_eq!(
            code_api_usage_endpoint(&root).unwrap().as_str(),
            "https://api.kimi.com/coding/v1/usages"
        );
        let coding = Url::parse("https://proxy.example/kimi/coding").unwrap();
        assert_eq!(
            code_api_usage_endpoint(&coding).unwrap().as_str(),
            "https://proxy.example/kimi/coding/v1/usages"
        );
        let versioned = Url::parse("https://proxy.example/kimi/coding/v1").unwrap();
        assert_eq!(
            code_api_usage_endpoint(&versioned).unwrap().as_str(),
            "https://proxy.example/kimi/coding/v1/usages"
        );
    }

    #[test]
    fn reuses_fresh_cli_credential_without_rewriting_file() {
        let _guard = env_lock();
        let now = 1_800_000_000.0_f64;
        let home = write_temp_kimi_code_home("oauth-token", Some(json!(now + 3600.0)));
        let cred_path = home.path().join("credentials").join("kimi-code.json");
        let original = std::fs::read(&cred_path).unwrap();
        let original_modified = std::fs::metadata(&cred_path).unwrap().modified().unwrap();

        // SAFETY: guarded by env_lock for process-wide env mutation in tests.
        unsafe {
            std::env::remove_var(KIMI_CODE_BASE_URL_ENV);
            std::env::remove_var(KIMI_CODE_OAUTH_HOST_ENV);
            std::env::remove_var(KIMI_OAUTH_HOST_ENV);
            std::env::set_var(KIMI_CODE_HOME_ENV, home.path());
        }

        let token = kimi_code_cli_access_token(now);
        assert_eq!(token.as_deref(), Some("oauth-token"));

        let after = std::fs::read(&cred_path).unwrap();
        let after_modified = std::fs::metadata(&cred_path).unwrap().modified().unwrap();
        assert_eq!(after, original);
        assert_eq!(after_modified, original_modified);

        let headers = kimi_code_cli_identity_headers(home.path());
        assert!(
            headers
                .iter()
                .any(|(k, v)| *k == "X-Msh-Platform" && v == KIMI_CODE_CLI_PLATFORM)
        );

        unsafe {
            std::env::remove_var(KIMI_CODE_HOME_ENV);
        }
    }

    #[test]
    fn rejects_expired_or_missing_expiry_cli_credentials() {
        let now = 1_800_000_000.0_f64;
        for expires in [Some(json!(now + 30.0)), None, Some(json!("not-a-time"))] {
            let home = write_temp_kimi_code_home("oauth", expires);
            let cred = read_kimi_code_credential(home.path()).expect("credential present");
            assert!(!is_kimi_code_credential_fresh(cred.expires_at, now));
        }

        let home = write_temp_kimi_code_home("oauth", Some(json!(now + 120.0)));
        let cred = read_kimi_code_credential(home.path()).unwrap();
        assert!(is_kimi_code_credential_fresh(cred.expires_at, now));
    }

    #[test]
    fn skips_cli_credential_when_endpoint_overrides_present() {
        let _guard = env_lock();
        let now = 1_800_000_000.0_f64;
        let home = write_temp_kimi_code_home("oauth-token", Some(json!(now + 3600.0)));

        unsafe {
            std::env::set_var(KIMI_CODE_HOME_ENV, home.path());
            std::env::set_var(KIMI_CODE_BASE_URL_ENV, "https://proxy.example.com/kimi");
        }
        assert!(has_code_endpoint_override());
        assert!(kimi_code_cli_access_token(now).is_none());

        unsafe {
            std::env::remove_var(KIMI_CODE_BASE_URL_ENV);
            std::env::set_var(KIMI_CODE_OAUTH_HOST_ENV, "https://oauth.example.com");
        }
        assert!(kimi_code_cli_access_token(now).is_none());

        unsafe {
            std::env::remove_var(KIMI_CODE_OAUTH_HOST_ENV);
            std::env::remove_var(KIMI_CODE_HOME_ENV);
        }
    }

    #[test]
    fn credential_freshness_accepts_millisecond_expiry() {
        let now = 1_800_000_000.0_f64;
        assert!(is_kimi_code_credential_fresh(
            Some(json!((now + 3600.0) * 1000.0)),
            now
        ));
    }

    #[test]
    fn credential_freshness_requires_sixty_second_margin() {
        assert!((KIMI_CODE_CREDENTIAL_MIN_TTL_SECS - 60.0).abs() < f64::EPSILON);
    }
}
