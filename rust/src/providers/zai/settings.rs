//! z.ai credential + endpoint resolution (upstream 0.48.0 `ZaiSettingsReader`).
//!
//! Port of upstream "route GLM credentials by region" (#2623) and
//! "China Kimi and GLM quota routing" (#2621):
//!
//! - `Z_AI_API_KEY` (and the legacy `ZAI_API_TOKEN`) feed either region.
//! - BigModel aliases (`BIGMODEL_API_KEY`, `ZHIPU_API_KEY`, `ZHIPUAI_API_KEY`,
//!   `GLM_API_KEY`) and coding-relay files are read **only** for BigModel CN.
//! - Endpoint overrides (`Z_AI_QUOTA_URL`, `Z_AI_API_HOST`) pointing at a
//!   canonical host of the *other* region are rejected before any bearer
//!   token is sent (`EndpointRegionMismatch`); custom relay hosts pass.

use std::collections::HashMap;
use std::path::Path;

use reqwest::Url;

use super::region::{ZaiRegion, canonical_region_for_host};

/// Environment variable map for testable resolution (production passes
/// `std::env::vars().collect()`).
pub type EnvMap = HashMap<String, String>;

pub const ZAI_API_KEY_ENV: &str = "Z_AI_API_KEY";
/// Legacy pre-region alias accepted for either region (local, pre-0.48).
pub const ZAI_LEGACY_API_KEY_ENV: &str = "ZAI_API_TOKEN";
pub const ZAI_API_HOST_ENV: &str = "Z_AI_API_HOST";
pub const ZAI_QUOTA_URL_ENV: &str = "Z_AI_QUOTA_URL";

/// BigModel CN environment aliases (upstream `bigModelAPITokenKeys`).
pub const BIGMODEL_API_TOKEN_KEYS: [&str; 4] = [
    "BIGMODEL_API_KEY",
    "ZHIPU_API_KEY",
    "ZHIPUAI_API_KEY",
    "GLM_API_KEY",
];

/// BigModel CN relay-file paths relative to the home directory (upstream
/// `bigModelAPIKeyRelativePaths`).
pub const BIGMODEL_API_KEY_RELATIVE_PATHS: [&str; 3] = [
    ".coding-relay/glm-api-key",
    ".config/bigmodel/api_key",
    ".config/zhipu/api_key",
];

/// Errors mirroring upstream `ZaiSettingsError`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ZaiSettingsError {
    #[error(
        "z.ai API token not found. Set apiKey in CodexBar settings, Z_AI_API_KEY, or a BigModel CN credential."
    )]
    MissingToken,
    #[error("z.ai endpoint override {0} must use HTTPS or a bare host.")]
    InvalidEndpointOverride(&'static str),
    #[error("z.ai endpoint override {0} does not match the selected {1} region.")]
    EndpointRegionMismatch(&'static str, ZaiRegion),
}

pub struct ZaiSettingsReader;

impl ZaiSettingsReader {
    /// Resolve the API token for a region (upstream `apiToken(for:)`).
    ///
    /// `Z_AI_API_KEY`/legacy alias apply to both regions; BigModel env keys
    /// and relay-file keys are China-only. Relay files contribute their first
    /// non-empty line, whitespace/quote-trimmed.
    pub fn api_token(env: &EnvMap, home: &Path, region: ZaiRegion) -> Option<String> {
        if let Some(token) = env_get(env, ZAI_API_KEY_ENV)
            .and_then(cleaned)
            .or_else(|| env_get(env, ZAI_LEGACY_API_KEY_ENV).and_then(cleaned))
        {
            return Some(token);
        }
        if region != ZaiRegion::BigModelCn {
            return None;
        }
        for key in BIGMODEL_API_TOKEN_KEYS {
            if let Some(token) = env_get(env, key).and_then(cleaned) {
                return Some(token);
            }
        }
        for relative in BIGMODEL_API_KEY_RELATIVE_PATHS {
            let path = home.join(relative);
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(token) = raw.lines().next().and_then(cleaned) {
                return Some(token);
            }
        }
        None
    }

    /// Region inferred from endpoint overrides only (upstream
    /// `inferredRegion`): a canonical BigModel CN override host selects CN;
    /// anything else is Global. Persisted settings take precedence over this
    /// in the provider (`fetch` applies the settings region first).
    pub fn inferred_region(env: &EnvMap) -> ZaiRegion {
        let host = Self::quota_url_override(env)
            .ok()
            .flatten()
            .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
            .or_else(|| {
                env_get(env, ZAI_API_HOST_ENV).and_then(|raw| {
                    normalized_https_url(raw)
                        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
                })
            });
        match host.as_deref().and_then(canonical_region_for_host) {
            Some(ZaiRegion::BigModelCn) => ZaiRegion::BigModelCn,
            _ => ZaiRegion::Global,
        }
    }

    /// Validated `Z_AI_QUOTA_URL` override, if any (upstream `quotaURL`).
    ///
    /// Returns `Ok(None)` when the override is unset, and
    /// `InvalidEndpointOverride` for non-HTTPS values so a broken override
    /// never silently downgrades the transfer.
    pub fn quota_url_override(env: &EnvMap) -> Result<Option<Url>, ZaiSettingsError> {
        env_get(env, ZAI_QUOTA_URL_ENV)
            .and_then(cleaned)
            .map(|raw| {
                normalized_https_url(&raw)
                    .ok_or(ZaiSettingsError::InvalidEndpointOverride(ZAI_QUOTA_URL_ENV))
            })
            .transpose()
    }

    /// `Z_AI_API_HOST` override expanded to the quota endpoint.
    pub fn quota_url_from_api_host(env: &EnvMap) -> Result<Option<Url>, ZaiSettingsError> {
        let Some(raw) = env_get(env, ZAI_API_HOST_ENV).and_then(cleaned) else {
            return Ok(None);
        };
        let mut url = normalized_https_url(&raw)
            .ok_or(ZaiSettingsError::InvalidEndpointOverride(ZAI_API_HOST_ENV))?;
        url.set_path("api/monitor/usage/quota/limit");
        url.set_query(None);
        Ok(Some(url))
    }

    /// Validate all endpoint overrides against the selected region *before*
    /// any authenticated request (upstream `validateEndpointOverrides(region:)`).
    pub fn validate_endpoint_overrides(
        env: &EnvMap,
        region: ZaiRegion,
    ) -> Result<(), ZaiSettingsError> {
        Self::validate_quota_endpoint_override(env, region)?;
        Self::validate_api_host_endpoint_override(env, region)
    }

    pub fn validate_quota_endpoint_override(
        env: &EnvMap,
        region: ZaiRegion,
    ) -> Result<(), ZaiSettingsError> {
        if env_get(env, ZAI_QUOTA_URL_ENV).and_then(cleaned).is_some() {
            let url = Self::quota_url_override(env)?.expect("override present");
            return validate_known_host(&url, region, ZAI_QUOTA_URL_ENV);
        }
        Self::validate_api_host_endpoint_override(env, region)
    }

    pub fn validate_api_host_endpoint_override(
        env: &EnvMap,
        region: ZaiRegion,
    ) -> Result<(), ZaiSettingsError> {
        let Some(raw) = env_get(env, ZAI_API_HOST_ENV).and_then(cleaned) else {
            return Ok(());
        };
        let url = normalized_https_url(&raw)
            .ok_or(ZaiSettingsError::InvalidEndpointOverride(ZAI_API_HOST_ENV))?;
        validate_known_host(&url, region, ZAI_API_HOST_ENV)
    }
}

/// Canonical cross-region override rejection (upstream `validateKnownHost`).
///
/// Only canonical plane hosts are pinned: `api.z.ai` overrides under BigModel
/// CN selection (and vice versa) fail, while custom relay/proxy hosts remain
/// legal in either region.
fn validate_known_host(
    url: &Url,
    region: ZaiRegion,
    key: &'static str,
) -> Result<(), ZaiSettingsError> {
    let Some(host) = url.host_str() else {
        return Ok(());
    };
    match canonical_region_for_host(host) {
        Some(host_region) if host_region != region => {
            Err(ZaiSettingsError::EndpointRegionMismatch(key, region))
        }
        _ => Ok(()),
    }
}

/// Upstream `ProviderEndpointOverrideValidator.normalizedHTTPSURL`:
/// bare hosts are promoted to HTTPS; explicit schemes must be HTTPS; no
/// user info or query allowed.
fn normalized_https_url(raw: &str) -> Option<Url> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let candidate = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    let url = Url::parse(&candidate).ok()?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    Some(url)
}

/// Upstream `cleaned`: trim, strip one matched quote pair, trim again.
pub fn cleaned(raw: &str) -> Option<String> {
    let mut value = raw.trim();
    if value.is_empty() {
        return None;
    }
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        value = value[1..value.len() - 1].trim();
    }
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn env_get<'a>(env: &'a EnvMap, key: &str) -> Option<&'a str> {
    env.get(key).map(String::as_str)
}

/// Collect the process environment into an [`EnvMap`] for production use.
pub fn process_env() -> EnvMap {
    std::env::vars().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> EnvMap {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn api_token_reads_from_environment() {
        let map = env(&[("Z_AI_API_KEY", "abc123")]);
        assert_eq!(
            ZaiSettingsReader::api_token(&map, Path::new("/nonexistent"), ZaiRegion::Global)
                .as_deref(),
            Some("abc123")
        );
        assert_eq!(
            ZaiSettingsReader::api_token(&map, Path::new("/nonexistent"), ZaiRegion::BigModelCn)
                .as_deref(),
            Some("abc123")
        );
    }

    #[test]
    fn legacy_alias_feeds_both_regions() {
        let map = env(&[("ZAI_API_TOKEN", "legacy-token")]);
        for region in [ZaiRegion::Global, ZaiRegion::BigModelCn] {
            assert_eq!(
                ZaiSettingsReader::api_token(&map, Path::new("/nonexistent"), region).as_deref(),
                Some("legacy-token")
            );
        }
    }

    #[test]
    fn bigmodel_aliases_are_available_only_to_china_region() {
        let map = env(&[("BIGMODEL_API_KEY", "china-token")]);
        assert_eq!(
            ZaiSettingsReader::api_token(&map, Path::new("/nonexistent"), ZaiRegion::BigModelCn)
                .as_deref(),
            Some("china-token")
        );
        assert_eq!(
            ZaiSettingsReader::api_token(&map, Path::new("/nonexistent"), ZaiRegion::Global),
            None
        );
    }

    #[test]
    fn bigmodel_alias_precedence_follows_upstream_key_order() {
        let map = env(&[
            ("GLM_API_KEY", "glm"),
            ("ZHIPU_API_KEY", "zhipu"),
            ("ZHIPUAI_API_KEY", "zhipuai"),
            ("BIGMODEL_API_KEY", "bigmodel"),
        ]);
        assert_eq!(
            ZaiSettingsReader::api_token(&map, Path::new("/nonexistent"), ZaiRegion::BigModelCn)
                .as_deref(),
            Some("bigmodel")
        );
        let map = env(&[("GLM_API_KEY", "glm"), ("ZHIPUAI_API_KEY", "zhipuai")]);
        assert_eq!(
            ZaiSettingsReader::api_token(&map, Path::new("/nonexistent"), ZaiRegion::BigModelCn)
                .as_deref(),
            Some("zhipuai")
        );
    }

    #[test]
    fn glm_relay_file_is_available_only_to_china_region() {
        let home = tempfile::tempdir().expect("tempdir");
        let relay_dir = home.path().join(".coding-relay");
        std::fs::create_dir_all(&relay_dir).expect("mkdir relay");
        std::fs::write(
            relay_dir.join("glm-api-key"),
            " relay-china-token\nignored-second-line",
        )
        .expect("write relay key");

        let map = env(&[]);
        assert_eq!(
            ZaiSettingsReader::api_token(&map, home.path(), ZaiRegion::BigModelCn).as_deref(),
            Some("relay-china-token")
        );
        assert_eq!(
            ZaiSettingsReader::api_token(&map, home.path(), ZaiRegion::Global),
            None
        );
    }

    #[test]
    fn relay_file_paths_follow_upstream_order() {
        // Each path in upstream's `bigModelAPIKeyRelativePaths` is honored in
        // isolation.
        for (dir, name, token) in [
            (".coding-relay", "glm-api-key", "relay-glm"),
            (".config/bigmodel", "api_key", "relay-bigmodel"),
            (".config/zhipu", "api_key", "relay-zhipu"),
        ] {
            let home = tempfile::tempdir().expect("tempdir");
            let dir = home.path().join(dir);
            std::fs::create_dir_all(&dir).expect("mkdir");
            std::fs::write(dir.join(name), format!("{token}\n")).expect("write");
            let map = env(&[]);
            assert_eq!(
                ZaiSettingsReader::api_token(&map, home.path(), ZaiRegion::BigModelCn).as_deref(),
                Some(token),
                "expected {token} from {name}"
            );
        }

        // Earlier path wins when several relay files exist.
        let home = tempfile::tempdir().expect("tempdir");
        for (dir, name, token) in [
            (".coding-relay", "glm-api-key", "relay-glm"),
            (".config/zhipu", "api_key", "relay-zhipu"),
        ] {
            let dir = home.path().join(dir);
            std::fs::create_dir_all(&dir).expect("mkdir");
            std::fs::write(dir.join(name), format!("{token}\n")).expect("write");
        }
        let map = env(&[]);
        assert_eq!(
            ZaiSettingsReader::api_token(&map, home.path(), ZaiRegion::BigModelCn).as_deref(),
            Some("relay-glm")
        );
    }

    #[test]
    fn unreadable_or_empty_relay_files_are_skipped() {
        let home = tempfile::tempdir().expect("tempdir");
        let relay_dir = home.path().join(".coding-relay");
        std::fs::create_dir_all(&relay_dir).expect("mkdir relay");
        std::fs::write(relay_dir.join("glm-api-key"), "\n  \n").expect("write empty");
        let map = env(&[]);
        assert_eq!(
            ZaiSettingsReader::api_token(&map, home.path(), ZaiRegion::BigModelCn),
            None
        );
    }

    #[test]
    fn canonical_endpoint_override_must_match_selected_region() {
        let err = ZaiSettingsReader::validate_endpoint_overrides(
            &env(&[("Z_AI_API_HOST", "open.bigmodel.cn")]),
            ZaiRegion::Global,
        )
        .unwrap_err();
        assert_eq!(
            err,
            ZaiSettingsError::EndpointRegionMismatch(ZAI_API_HOST_ENV, ZaiRegion::Global)
        );

        let err = ZaiSettingsReader::validate_endpoint_overrides(
            &env(&[("Z_AI_API_HOST", "api.z.ai")]),
            ZaiRegion::BigModelCn,
        )
        .unwrap_err();
        assert_eq!(
            err,
            ZaiSettingsError::EndpointRegionMismatch(ZAI_API_HOST_ENV, ZaiRegion::BigModelCn)
        );

        let err = ZaiSettingsReader::validate_quota_endpoint_override(
            &env(&[(
                "Z_AI_QUOTA_URL",
                "https://api.z.ai/api/monitor/usage/quota/limit",
            )]),
            ZaiRegion::BigModelCn,
        )
        .unwrap_err();
        assert_eq!(
            err,
            ZaiSettingsError::EndpointRegionMismatch(ZAI_QUOTA_URL_ENV, ZaiRegion::BigModelCn)
        );
    }

    #[test]
    fn custom_relay_hosts_pass_region_validation() {
        ZaiSettingsReader::validate_endpoint_overrides(
            &env(&[("Z_AI_API_HOST", "relay.example.com")]),
            ZaiRegion::Global,
        )
        .expect("custom relay allowed in global");
        ZaiSettingsReader::validate_endpoint_overrides(
            &env(&[("Z_AI_QUOTA_URL", "https://relay.example.com/quota")]),
            ZaiRegion::BigModelCn,
        )
        .expect("custom relay allowed in cn");
    }

    #[test]
    fn non_https_or_userinfo_overrides_are_invalid() {
        assert_eq!(
            ZaiSettingsReader::validate_quota_endpoint_override(
                &env(&[("Z_AI_QUOTA_URL", "http://open.bigmodel.cn")]),
                ZaiRegion::BigModelCn,
            )
            .unwrap_err(),
            ZaiSettingsError::InvalidEndpointOverride(ZAI_QUOTA_URL_ENV)
        );
        assert_eq!(
            ZaiSettingsReader::validate_api_host_endpoint_override(
                &env(&[("Z_AI_API_HOST", "user@api.z.ai")]),
                ZaiRegion::Global,
            )
            .unwrap_err(),
            ZaiSettingsError::InvalidEndpointOverride(ZAI_API_HOST_ENV)
        );
    }

    #[test]
    fn inferred_region_follows_override_host() {
        assert_eq!(
            ZaiSettingsReader::inferred_region(&env(&[("Z_AI_API_HOST", "open.bigmodel.cn")])),
            ZaiRegion::BigModelCn
        );
        assert_eq!(
            ZaiSettingsReader::inferred_region(&env(&[(
                "Z_AI_QUOTA_URL",
                "https://open.bigmodel.cn/api/monitor/usage/quota/limit"
            )])),
            ZaiRegion::BigModelCn
        );
        assert_eq!(
            ZaiSettingsReader::inferred_region(&env(&[("Z_AI_API_HOST", "relay.example.com")])),
            ZaiRegion::Global
        );
        assert_eq!(
            ZaiSettingsReader::inferred_region(&env(&[("Z_AI_API_HOST", "api.z.ai")])),
            ZaiRegion::Global
        );
    }

    #[test]
    fn bare_host_override_expands_to_quota_path() {
        let url = ZaiSettingsReader::quota_url_from_api_host(&env(&[(
            "Z_AI_API_HOST",
            "open.bigmodel.cn",
        )]))
        .expect("valid host")
        .expect("some");
        assert_eq!(
            url.as_str(),
            "https://open.bigmodel.cn/api/monitor/usage/quota/limit"
        );
    }

    #[test]
    fn cleaned_strips_quotes_and_whitespace() {
        assert_eq!(cleaned("  \"token\"  ").as_deref(), Some("token"));
        assert_eq!(cleaned("'token'").as_deref(), Some("token"));
        assert_eq!(cleaned("token").as_deref(), Some("token"));
        assert_eq!(cleaned("   "), None);
        assert_eq!(cleaned("\"\""), None);
    }
}
