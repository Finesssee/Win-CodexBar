//! Vertex AI provider implementation
//!
//! Fetches usage data from Google Cloud Vertex AI
//! Uses Google Cloud credentials for authentication

mod token_refresher;

// Re-exports for OAuth token refresh
#[allow(
    unused_imports,
    reason = "imports needed for future VertexAI provider wiring"
)]
pub use token_refresher::{RefreshError, VertexAIOAuthCredentials, VertexAITokenRefresher};

use async_trait::async_trait;
use std::path::PathBuf;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};

/// Vertex AI provider
pub struct VertexAIProvider {
    metadata: ProviderMetadata,
}

impl VertexAIProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::VertexAI,
                display_name: "Vertex AI",
                session_label: "Usage",
                weekly_label: "Monthly",
                supports_opus: false,
                supports_credits: true,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://console.cloud.google.com/vertex-ai"),
                status_page_url: Some("https://status.cloud.google.com"),
            },
        }
    }

    /// Get Google Cloud credentials path
    fn get_gcloud_config_path() -> Option<PathBuf> {
        // Check GOOGLE_APPLICATION_CREDENTIALS env var first
        if let Ok(path) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS") {
            return Some(PathBuf::from(path));
        }

        // Default gcloud config location
        #[cfg(target_os = "windows")]
        {
            dirs::config_dir().map(|p| {
                p.join("gcloud")
                    .join("application_default_credentials.json")
            })
        }
        #[cfg(not(target_os = "windows"))]
        {
            dirs::home_dir().map(|p| {
                p.join(".config")
                    .join("gcloud")
                    .join("application_default_credentials.json")
            })
        }
    }

    /// Find gcloud CLI
    fn which_gcloud() -> Option<PathBuf> {
        let possible_paths = [
            which::which("gcloud").ok(),
            #[cfg(target_os = "windows")]
            Some(PathBuf::from(
                "C:\\Program Files (x86)\\Google\\Cloud SDK\\google-cloud-sdk\\bin\\gcloud.cmd",
            )),
            #[cfg(target_os = "windows")]
            Some(PathBuf::from(
                "C:\\Users\\Public\\google-cloud-sdk\\bin\\gcloud.cmd",
            )),
            #[cfg(not(target_os = "windows"))]
            None,
        ];

        possible_paths.into_iter().flatten().find(|p| p.exists())
    }

    /// Read access token from gcloud config
    async fn get_access_token(&self) -> Result<String, ProviderError> {
        let creds_path = Self::get_gcloud_config_path().ok_or_else(|| {
            ProviderError::NotInstalled("Google Cloud credentials not found".to_string())
        })?;

        if creds_path.exists() {
            let content = tokio::fs::read_to_string(&creds_path)
                .await
                .map_err(|e| ProviderError::Other(e.to_string()))?;

            let json: serde_json::Value =
                serde_json::from_str(&content).map_err(|e| ProviderError::Parse(e.to_string()))?;

            // Check for refresh token flow
            if let Some(refresh_token) = json.get("refresh_token").and_then(|v| v.as_str()) {
                let client_id = json
                    .get("client_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let client_secret = json
                    .get("client_secret")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();

                return self
                    .refresh_access_token(refresh_token, client_id, client_secret)
                    .await;
            }
        }

        // Try running gcloud auth print-access-token
        if let Some(gcloud) = Self::which_gcloud() {
            #[cfg(windows)]
            const CREATE_NO_WINDOW: u32 = 0x08000000;

            let mut cmd = tokio::process::Command::new(gcloud);
            cmd.args(["auth", "print-access-token"]);
            #[cfg(windows)]
            cmd.creation_flags(CREATE_NO_WINDOW);

            let output = cmd
                .output()
                .await
                .map_err(|e| ProviderError::Other(e.to_string()))?;

            if output.status.success() {
                let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !token.is_empty() {
                    return Ok(token);
                }
            }
        }

        Err(ProviderError::AuthRequired)
    }

    async fn refresh_access_token(
        &self,
        refresh_token: &str,
        client_id: &str,
        client_secret: &str,
    ) -> Result<String, ProviderError> {
        let client = reqwest::Client::new();

        let resp = client
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("refresh_token", refresh_token),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(ProviderError::AuthRequired);
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;

        json.get("access_token")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| ProviderError::Parse("No access_token in response".to_string()))
    }

    /// Fetch usage via Vertex AI API
    async fn fetch_via_web(&self) -> Result<UsageSnapshot, ProviderError> {
        let token = self.get_access_token().await?;

        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        // Get project ID from config
        let project_id = self
            .get_project_id()
            .await
            .unwrap_or_else(|_| "unknown".to_string());

        // Vertex AI billing/quota API
        let resp = client
            .get(format!(
                "https://cloudresourcemanager.googleapis.com/v1/projects/{}",
                project_id
            ))
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await;

        match resp {
            Ok(r) if r.status().is_success() => {
                let json: serde_json::Value = r
                    .json()
                    .await
                    .map_err(|e| ProviderError::Parse(e.to_string()))?;
                self.parse_usage_response(&json, &project_id)
            }
            _ => {
                // Return placeholder with project info
                let usage = UsageSnapshot::new(RateWindow::new(0.0))
                    .with_login_method(format!("Vertex AI ({})", project_id));
                Ok(usage)
            }
        }
    }

    async fn get_project_id(&self) -> Result<String, ProviderError> {
        // Check GOOGLE_CLOUD_PROJECT env var
        if let Ok(project) = std::env::var("GOOGLE_CLOUD_PROJECT") {
            return Ok(project);
        }

        // Try to read from gcloud config
        #[cfg(target_os = "windows")]
        let config_path = dirs::config_dir().map(|p| p.join("gcloud").join("properties"));
        #[cfg(not(target_os = "windows"))]
        let config_path =
            dirs::home_dir().map(|p| p.join(".config").join("gcloud").join("properties"));

        if let Some(path) = config_path
            && path.exists()
        {
            let content = tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| ProviderError::Other(e.to_string()))?;

            for line in content.lines() {
                if line.starts_with("project")
                    && let Some(proj) = line.split('=').nth(1)
                {
                    return Ok(proj.trim().to_string());
                }
            }
        }

        Err(ProviderError::Other("Project ID not found".to_string()))
    }

    fn parse_usage_response(
        &self,
        json: &serde_json::Value,
        project_id: &str,
    ) -> Result<UsageSnapshot, ProviderError> {
        // Parse project info - actual usage would require Cloud Billing API
        let project_name = json
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(project_id);

        let usage = UsageSnapshot::new(RateWindow::new(0.0))
            .with_login_method(format!("Vertex AI ({})", project_name));

        Ok(usage)
    }

    /// Probe CLI for detection
    async fn probe_cli(&self) -> Result<UsageSnapshot, ProviderError> {
        let gcloud = Self::which_gcloud().ok_or_else(|| {
            ProviderError::NotInstalled(
                "gcloud CLI not found. Install from https://cloud.google.com/sdk".to_string(),
            )
        })?;

        if gcloud.exists() {
            let project = self.get_project_id().await.ok();
            let label = if let Some(p) = project {
                format!("Vertex AI ({})", p)
            } else {
                "Vertex AI (installed)".to_string()
            };

            let usage = UsageSnapshot::new(RateWindow::new(0.0)).with_login_method(&label);
            Ok(usage)
        } else {
            Err(ProviderError::NotInstalled("gcloud not found".to_string()))
        }
    }
}

impl Default for VertexAIProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for VertexAIProvider {
    fn id(&self) -> ProviderId {
        ProviderId::VertexAI
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Fetching Vertex AI usage");

        match ctx.source_mode {
            SourceMode::Auto => {
                if let Ok(usage) = self.fetch_via_web().await {
                    return Ok(ProviderFetchResult::new(usage, "web"));
                }
                let usage = self.probe_cli().await?;
                Ok(ProviderFetchResult::new(usage, "cli"))
            }
            SourceMode::Web => {
                let usage = self.fetch_via_web().await?;
                Ok(ProviderFetchResult::new(usage, "web"))
            }
            SourceMode::Cli => {
                let usage = self.probe_cli().await?;
                Ok(ProviderFetchResult::new(usage, "cli"))
            }
            SourceMode::OAuth => Err(ProviderError::UnsupportedSource(SourceMode::OAuth)),
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::Web, SourceMode::Cli]
    }

    fn supports_web(&self) -> bool {
        true
    }

    fn supports_cli(&self) -> bool {
        true
    }
}
