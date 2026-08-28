//! GitHub Copilot provider implementation
//!
//! Fetches usage data from GitHub's Copilot API using stored OAuth token

mod api;
pub mod device_flow;

use async_trait::async_trait;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    ProviderStateKind, SourceMode,
};

pub use api::CopilotApi;

/// GitHub Copilot provider for fetching AI usage limits
pub struct CopilotProvider {
    metadata: ProviderMetadata,
    api: CopilotApi,
}

impl CopilotProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Copilot,
                display_name: "GitHub Copilot",
                session_label: "Premium",
                weekly_label: "Chat",
                supports_opus: false,
                supports_credits: false,
                default_enabled: true,
                is_primary: false,
                dashboard_url: Some("https://github.com/settings/copilot"),
                status_page_url: Some("https://www.githubstatus.com/"),
            },
            api: CopilotApi::new(),
        }
    }
}

impl Default for CopilotProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for CopilotProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Copilot
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Fetching GitHub Copilot usage via GitHub OAuth");

        match self.api.fetch_usage(ctx.api_key.as_deref()).await {
            Ok(usage) => Ok(ProviderFetchResult::new(usage, "oauth")),
            Err(e) => {
                tracing::warn!("Copilot API fetch failed: {}", e);
                Err(e)
            }
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::OAuth]
    }

    fn supports_oauth(&self) -> bool {
        true
    }

    /// Copilot's `NotInstalled` means no usable GitHub token was found
    /// (missing credential, not a missing local runtime), so it surfaces as a
    /// sign-in gate rather than an offline runtime.
    fn error_state_kind(&self, error: &ProviderError) -> ProviderStateKind {
        match error {
            ProviderError::NotInstalled(_) => ProviderStateKind::NeedsAuthentication,
            _ => error.state_kind(),
        }
    }
}

#[cfg(test)]
mod state_kind_tests {
    use super::*;

    #[test]
    fn not_installed_maps_to_needs_authentication() {
        let provider = CopilotProvider::new();
        assert_eq!(
            provider.error_state_kind(&ProviderError::NotInstalled(
                "GitHub Copilot token not found. Sign in with GitHub.".into(),
            )),
            ProviderStateKind::NeedsAuthentication
        );
    }

    #[test]
    fn other_variants_use_the_default_mapping() {
        let provider = CopilotProvider::new();
        assert_eq!(
            provider.error_state_kind(&ProviderError::Timeout),
            ProviderStateKind::Unknown
        );
        assert_eq!(
            provider.error_state_kind(&ProviderError::OAuth("Token expired.".into())),
            ProviderStateKind::ExpiredSession
        );
    }
}
