use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::core::ProviderId;

const ZED_DEFAULT_URL: &str = "https://cloud.zed.dev/client/users/me";

pub fn validate_provider_workspace_value(
    provider_id: ProviderId,
    raw: &str,
) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }

    match provider_id {
        ProviderId::OpenAIApi => validate_id(trimmed, "OpenAI project ID", |value| {
            value.starts_with("proj_") && has_safe_id_chars(value)
        }),
        ProviderId::OpenCodeGo => validate_id(trimmed, "OpenCode Go workspace ID", |value| {
            value.starts_with("wrk_") && has_safe_id_chars(value)
        }),
        ProviderId::Devin => validate_id(trimmed, "Devin organization", |value| {
            let Some((prefix, org)) = value.split_once('/') else {
                return false;
            };
            prefix == "org"
                && (2..=80).contains(&org.len())
                && org
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        }),
        ProviderId::Zed => validate_zed_url(trimmed),
        ProviderId::Xai => {
            if trimmed.contains('/') || trimmed == "." || trimmed == ".." {
                return Err(
                    "The xAI team ID must be a single identifier without path separators"
                        .to_string(),
                );
            }
            Ok(trimmed.to_string())
        }
        ProviderId::LiteLLM => validate_token_endpoint(trimmed, "LiteLLM base URL", |_| true),
        ProviderId::Sub2Api => validate_sub2api_base_url(trimmed),
        _ => Ok(trimmed.to_string()),
    }
}

fn validate_sub2api_base_url(raw: &str) -> Result<String, String> {
    match crate::providers::sub2api::validated_sub2api_base_url(raw) {
        Ok(url) => Ok(url.to_string().trim_end_matches('/').to_string()),
        Err(err) => Err(err.to_string()),
    }
}

fn validate_id(
    value: &str,
    label: &str,
    is_valid: impl FnOnce(&str) -> bool,
) -> Result<String, String> {
    if value.chars().any(|c| c.is_control() || c.is_whitespace()) || !is_valid(value) {
        return Err(format!("{label} is invalid"));
    }
    Ok(value.to_string())
}

fn has_safe_id_chars(value: &str) -> bool {
    (6..=128).contains(&value.len())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn validate_zed_url(raw: &str) -> Result<String, String> {
    let url = validate_token_endpoint(raw, "Zed API URL", |host| host == "cloud.zed.dev")?;
    if url != ZED_DEFAULT_URL {
        return Err(
            "Zed API URL must be the default https://cloud.zed.dev/client/users/me endpoint"
                .to_string(),
        );
    }
    Ok(url)
}

fn validate_token_endpoint(
    raw: &str,
    label: &str,
    host_allowed: impl FnOnce(&str) -> bool,
) -> Result<String, String> {
    let lower = raw.to_ascii_lowercase();
    if raw.chars().any(|c| c.is_control())
        || ["%2f", "%5c", "%3f", "%23", "%40", "%3a"]
            .iter()
            .any(|encoded| lower.contains(encoded))
    {
        return Err(format!(
            "{label} must not contain control characters or encoded host delimiters"
        ));
    }

    let url = reqwest::Url::parse(raw).map_err(|e| format!("Invalid {label}: {e}"))?;
    let host = url
        .host_str()
        .ok_or_else(|| format!("{label} must include a host"))?
        .to_ascii_lowercase();

    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || host.contains('%')
        || host.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(format!(
            "{label} must use HTTPS without user info or encoded host tricks"
        ));
    }
    if is_blocked_host(&host) || !host_allowed(&host) {
        return Err(format!("{label} host is not allowed"));
    }

    Ok(url.to_string().trim_end_matches('/').to_string())
}

fn is_blocked_host(host: &str) -> bool {
    let normalized = host.trim_end_matches('.').to_ascii_lowercase();
    if normalized == "localhost" || normalized.ends_with(".localhost") {
        return true;
    }
    let ip_candidate = normalized
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(&normalized);
    ip_candidate.parse::<IpAddr>().is_ok_and(|ip| match ip {
        IpAddr::V4(ip) => is_blocked_ipv4(ip),
        IpAddr::V6(ip) => is_blocked_ipv6(ip),
    })
}

fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.octets()[0] == 0
        || ip.octets()[0] >= 224
}

fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    ip.is_loopback()
        || ip.is_unspecified()
        || ((ip.segments()[0] & 0xfe00) == 0xfc00)
        || ((ip.segments()[0] & 0xffc0) == 0xfe80)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_workspace_ids_by_provider() {
        assert_eq!(
            validate_provider_workspace_value(ProviderId::OpenAIApi, " proj_abc-123 ").unwrap(),
            "proj_abc-123"
        );
        assert_eq!(
            validate_provider_workspace_value(ProviderId::Devin, "org/acme_123").unwrap(),
            "org/acme_123"
        );
        assert_eq!(
            validate_provider_workspace_value(ProviderId::OpenCodeGo, " wrk_abc-123 ").unwrap(),
            "wrk_abc-123"
        );
        assert!(
            validate_provider_workspace_value(ProviderId::OpenAIApi, "https://evil.test").is_err()
        );
        assert!(
            validate_provider_workspace_value(ProviderId::Devin, "https://api.devin.ai").is_err()
        );
        assert!(validate_provider_workspace_value(ProviderId::OpenCodeGo, "wrk_abc/123").is_err());
    }

    #[test]
    fn validates_token_endpoint_hosts() {
        assert_eq!(
            validate_provider_workspace_value(
                ProviderId::LiteLLM,
                "https://litellm.example.com/v1"
            )
            .unwrap(),
            "https://litellm.example.com/v1"
        );
        for value in [
            "http://litellm.example.com",
            "https://user@litellm.example.com",
            "https://127.0.0.1",
            "https://10.0.0.5",
            "https://[::1]",
            "https://example.com%2f.evil.test",
        ] {
            assert!(
                validate_provider_workspace_value(ProviderId::LiteLLM, value).is_err(),
                "accepted {value}"
            );
        }
    }

    #[test]
    fn validates_sub2api_base_url() {
        assert_eq!(
            validate_provider_workspace_value(ProviderId::Sub2Api, "https://api.example.com/")
                .unwrap(),
            "https://api.example.com"
        );
        assert!(
            validate_provider_workspace_value(ProviderId::Sub2Api, "http://127.0.0.1:8080").is_ok()
        );
        assert!(
            validate_provider_workspace_value(ProviderId::Sub2Api, "http://api.example.com")
                .is_err()
        );
        assert!(
            validate_provider_workspace_value(
                ProviderId::Sub2Api,
                "https://user:pass@api.example.com"
            )
            .is_err()
        );
    }

    #[test]
    fn zed_allows_only_default_cloud_endpoint() {
        assert_eq!(
            validate_provider_workspace_value(
                ProviderId::Zed,
                "https://cloud.zed.dev/client/users/me"
            )
            .unwrap(),
            "https://cloud.zed.dev/client/users/me"
        );
        assert!(
            validate_provider_workspace_value(ProviderId::Zed, "https://evil.example/users/me")
                .is_err()
        );
        assert!(
            validate_provider_workspace_value(
                ProviderId::Zed,
                "https://cloud.zed.dev.evil/users/me"
            )
            .is_err()
        );
    }
}
