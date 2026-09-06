//! Codex (OpenAI/ChatGPT) provider implementation
//!
//! Fetches usage data from ChatGPT's backend API using OAuth credentials
//! stored by the Codex CLI in ~/.codex/auth.json

mod api;
mod pat;
mod weekly_reset;

use async_trait::async_trait;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    SourceMode,
};

pub use api::CodexApi;

/// Codex provider for fetching AI usage limits
pub struct CodexProvider {
    metadata: ProviderMetadata,
    api: CodexApi,
}

impl CodexProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Codex,
                display_name: "Codex",
                session_label: "Session",
                weekly_label: "Weekly",
                supports_opus: false,
                supports_credits: true,
                default_enabled: true,
                is_primary: true,
                dashboard_url: Some("https://chatgpt.com/codex/settings/usage"),
                status_page_url: Some("https://status.openai.com"),
            },
            api: CodexApi::new(),
        }
    }
}

fn fetch_result(
    usage: crate::core::UsageSnapshot,
    cost: Option<crate::core::CostSnapshot>,
    source: &str,
) -> ProviderFetchResult {
    let mut result = ProviderFetchResult::new(usage, source);
    if let Some(cost) = cost {
        result = result.with_cost(cost);
    }
    result
}

fn pat_allows_auto_fallback(error: &ProviderError) -> bool {
    matches!(
        error,
        ProviderError::AuthRequired | ProviderError::NotInstalled(_)
    )
}

impl Default for CodexProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for CodexProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Codex
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Fetching Codex usage");

        if ctx.source_mode == SourceMode::Web {
            return Err(ProviderError::UnsupportedSource(SourceMode::Web));
        }

        if ctx.source_mode == SourceMode::Auto && self.api.has_pat_credentials() {
            let version = detect_codex_version();
            match self.api.fetch_usage_pat(version.as_deref()).await {
                Ok((usage, cost)) => return Ok(fetch_result(usage, cost, "pat")),
                Err(error) if pat_allows_auto_fallback(&error) => {
                    tracing::debug!("Codex PAT unavailable in Auto; trying OAuth: {error}");
                }
                Err(error) => return Err(error),
            }
        }

        match self.api.fetch_usage().await {
            Ok((usage, cost)) => Ok(fetch_result(usage, cost, "oauth")),
            Err(error) => {
                tracing::warn!("Codex API fetch failed: {error}");
                Err(error)
            }
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::OAuth, SourceMode::Cli]
    }

    fn supports_oauth(&self) -> bool {
        true
    }

    fn supports_cli(&self) -> bool {
        true
    }

    fn detect_version(&self) -> Option<String> {
        detect_codex_version()
    }
}

/// Try to find the codex CLI binary
fn which_codex() -> Option<std::path::PathBuf> {
    // Check common locations on Windows
    let possible_paths = [
        // In PATH
        which::which("codex").ok(),
        // npm global install
        dirs::data_dir().map(|p| p.join("npm").join("codex.cmd")),
        // AppData locations
        dirs::data_local_dir().map(|p| p.join("Programs").join("codex").join("codex.exe")),
    ];

    possible_paths.into_iter().flatten().find(|p| p.exists())
}

/// Detect the version of the codex CLI
fn detect_codex_version() -> Option<String> {
    let codex_path = which_codex()?;

    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut cmd = std::process::Command::new(codex_path);
    cmd.args(["--version"]);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let output = cmd.output().ok()?;

    if output.status.success() {
        let version_str = String::from_utf8_lossy(&output.stdout);
        super::extract_semver(&version_str)
    } else {
        None
    }
}

#[cfg(test)]
mod pat_strategy_tests {
    use super::*;

    #[test]
    fn pat_auto_fallback_is_narrow() {
        assert!(pat_allows_auto_fallback(&ProviderError::AuthRequired));
        assert!(pat_allows_auto_fallback(&ProviderError::NotInstalled(
            "missing".into()
        )));
        assert!(!pat_allows_auto_fallback(&ProviderError::Parse(
            "bad".into()
        )));
        assert!(!pat_allows_auto_fallback(&ProviderError::Other(
            "server".into()
        )));
    }
}
