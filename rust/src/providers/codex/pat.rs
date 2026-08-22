//! Codex personal-access-token usage source (upstream 0.54.0 #3060).

use std::path::Path;

use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;

use crate::core::ProviderError;

const WHOAMI_URL: &str = "https://auth.openai.com/api/accounts/v1/user-auth-credential/whoami";
const ORIGINATOR: &str = "codex_cli_rs";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct PatWhoami {
    pub account_id: Option<String>,
    pub email: Option<String>,
    pub plan_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WhoamiResponse {
    chatgpt_account_id: Option<String>,
    chatgpt_plan_type: Option<String>,
    email: Option<String>,
}

pub(super) fn load_token(auth_path: &Path) -> Result<String, ProviderError> {
    let content = std::fs::read_to_string(auth_path).map_err(|error| {
        ProviderError::NotInstalled(format!(
            "Codex auth.json does not contain a usable personal access token: {error}"
        ))
    })?;
    parse_token_json(&content)
}

pub(super) fn parse_token_json(content: &str) -> Result<String, ProviderError> {
    let json: Value = serde_json::from_str(content).map_err(|error| {
        ProviderError::Parse(format!("Invalid Codex credentials JSON: {error}"))
    })?;
    ["personal_access_token", "personalAccessToken"]
        .into_iter()
        .find_map(|key| json.get(key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            ProviderError::NotInstalled(
                "Codex auth.json contains no personal access token.".to_string(),
            )
        })
}

pub(super) async fn fetch_usage(
    client: &Client,
    base_url: &str,
    token: &str,
    cli_version: Option<&str>,
) -> Result<(Value, PatWhoami), ProviderError> {
    let user_agent = user_agent(cli_version);
    let whoami_response = client
        .get(WHOAMI_URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("User-Agent", &user_agent)
        .header("Accept", "application/json")
        .header("originator", ORIGINATOR)
        .send()
        .await?;
    let whoami = decode_whoami(whoami_response).await?;

    let usage_url = format!("{}/wham/usage", base_url.trim_end_matches('/'));
    let mut request = client
        .get(usage_url)
        .header("Authorization", format!("Bearer {token}"))
        .header("User-Agent", user_agent)
        .header("Accept", "application/json")
        .header("originator", ORIGINATOR);
    if let Some(account_id) = whoami.account_id.as_deref() {
        request = request.header("ChatGPT-Account-Id", account_id);
    }
    let response = request.send().await?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(ProviderError::AuthRequired);
    }
    if !status.is_success() {
        return Err(ProviderError::Other(format!(
            "Codex PAT usage API returned {status}"
        )));
    }
    let usage = response.json::<Value>().await.map_err(|error| {
        ProviderError::Parse(format!("Invalid Codex PAT usage response: {error}"))
    })?;
    Ok((usage, whoami))
}

async fn decode_whoami(response: reqwest::Response) -> Result<PatWhoami, ProviderError> {
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(ProviderError::AuthRequired);
    }
    if !status.is_success() {
        return Err(ProviderError::Other(format!(
            "Codex PAT whoami returned {status}"
        )));
    }
    let response = response.json::<WhoamiResponse>().await.map_err(|error| {
        ProviderError::Parse(format!("Invalid Codex PAT whoami response: {error}"))
    })?;
    Ok(PatWhoami {
        account_id: nonempty(response.chatgpt_account_id),
        email: nonempty(response.email),
        plan_type: nonempty(response.chatgpt_plan_type),
    })
}

fn nonempty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(super) fn user_agent(cli_version: Option<&str>) -> String {
    let version = cli_version.and_then(normalize_cli_version);
    let platform = if cfg!(windows) {
        "Windows"
    } else {
        std::env::consts::OS
    };
    match version {
        Some(version) => format!(
            "codex_cli_rs/{version} ({platform}; {})",
            std::env::consts::ARCH
        ),
        None => format!("codex_cli_rs ({platform}; {})", std::env::consts::ARCH),
    }
}

fn normalize_cli_version(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut parts = trimmed.split_whitespace();
    let first = parts.next()?;
    if first.eq_ignore_ascii_case("codex-cli") {
        return parts.next().filter(|value| !value.is_empty());
    }
    Some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pat_separately_from_other_auth_fields() {
        let json = r#"{
          "personal_access_token": " pat_value ",
          "OPENAI_API_KEY": "api_value",
          "tokens": {"access_token":"oauth_value", "refresh_token":"refresh_value"}
        }"#;
        assert_eq!(parse_token_json(json).unwrap(), "pat_value");
        assert!(parse_token_json(r#"{"personal_access_token":"   "}"#).is_err());
    }

    #[test]
    fn accepts_camel_case_pat_alias() {
        assert_eq!(
            parse_token_json(r#"{"personalAccessToken":"pat_camel"}"#).unwrap(),
            "pat_camel"
        );
    }

    #[test]
    fn codex_cli_user_agent_uses_detected_version() {
        assert!(
            user_agent(Some("codex-cli 0.148.0-alpha.9"))
                .starts_with("codex_cli_rs/0.148.0-alpha.9 (")
        );
        assert!(user_agent(None).starts_with("codex_cli_rs ("));
    }
}
