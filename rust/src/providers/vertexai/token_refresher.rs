//! VertexAI OAuth Token Refresher
//!
//! Handles OAuth token refresh for Google Cloud Vertex AI credentials.
//! Provides automatic token refresh before expiry and caches tokens.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

/// OAuth credentials for Vertex AI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VertexAIOAuthCredentials {
    pub access_token: String,
    pub refresh_token: String,
    pub client_id: String,
    pub client_secret: String,
    pub project_id: Option<String>,
    pub email: Option<String>,
    pub expiry_date: Option<DateTime<Utc>>,
}

impl VertexAIOAuthCredentials {
    /// Check if the access token is expired or about to expire
    pub fn is_expired(&self) -> bool {
        if let Some(expiry) = self.expiry_date {
            // Consider expired if less than 5 minutes remaining
            let buffer = chrono::Duration::seconds(300);
            Utc::now() + buffer >= expiry
        } else {
            // No expiry info, assume expired to be safe
            true
        }
    }

    /// Check if token is valid and not expired
    pub fn is_valid(&self) -> bool {
        !self.access_token.is_empty() && !self.is_expired()
    }
}

/// Refresh error types
#[derive(Debug, Clone)]
pub enum RefreshError {
    /// Refresh token expired - need to re-authenticate
    Expired,
    /// Token was revoked
    Revoked,
    /// Network error during refresh
    NetworkError(String),
    /// Invalid response from server
    InvalidResponse(String),
}

impl std::fmt::Display for RefreshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RefreshError::Expired => write!(
                f,
                "Refresh token expired. Run `gcloud auth application-default login` again."
            ),
            RefreshError::Revoked => write!(
                f,
                "Refresh token was revoked. Run `gcloud auth application-default login` again."
            ),
            RefreshError::NetworkError(e) => write!(f, "Network error during token refresh: {}", e),
            RefreshError::InvalidResponse(e) => write!(f, "Invalid refresh response: {}", e),
        }
    }
}

impl std::error::Error for RefreshError {}

/// Token refresher for Vertex AI OAuth
pub struct VertexAITokenRefresher {
    cached_credentials: Arc<RwLock<Option<VertexAIOAuthCredentials>>>,
}

impl VertexAITokenRefresher {
    /// Create a new token refresher
    pub fn new() -> Self {
        Self {
            cached_credentials: Arc::new(RwLock::new(None)),
        }
    }

    /// Refresh the access token using the refresh token
    pub async fn refresh(
        &self,
        credentials: VertexAIOAuthCredentials,
    ) -> Result<VertexAIOAuthCredentials, RefreshError> {
        if credentials.refresh_token.is_empty() {
            return Err(RefreshError::InvalidResponse(
                "No refresh token available".to_string(),
            ));
        }

        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| RefreshError::NetworkError(e.to_string()))?;

        let body = [
            ("client_id", credentials.client_id.as_str()),
            ("client_secret", credentials.client_secret.as_str()),
            ("refresh_token", credentials.refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ];

        let resp = client
            .post("https://oauth2.googleapis.com/token")
            .form(&body)
            .send()
            .await
            .map_err(|e| RefreshError::NetworkError(e.to_string()))?;

        let status = resp.status();

        if status.as_u16() == 400 || status.as_u16() == 401 {
            let body = resp.text().await.unwrap_or_default();
            if body.contains("invalid_grant") {
                return Err(RefreshError::Expired);
            }
            if body.contains("unauthorized_client") {
                return Err(RefreshError::Revoked);
            }
            return Err(RefreshError::InvalidResponse(format!("Error: {}", body)));
        }

        if !status.is_success() {
            return Err(RefreshError::InvalidResponse(format!("Status {}", status)));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| RefreshError::InvalidResponse(e.to_string()))?;

        let new_access_token = json
            .get("access_token")
            .and_then(|v| v.as_str())
            .unwrap_or(&credentials.access_token)
            .to_string();

        let expires_in = json
            .get("expires_in")
            .and_then(|v| v.as_f64())
            .unwrap_or(3600.0);

        // OAuth expires_in is a whole-second lifetime (default 3600), far below i64::MAX.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "token lifetime seconds fit i64"
        )]
        let expiry_seconds = expires_in as i64;
        let new_expiry_date = Utc::now() + chrono::Duration::seconds(expiry_seconds);

        // Extract email from new ID token if present
        let email = json
            .get("id_token")
            .and_then(|v| v.as_str())
            .and_then(Self::extract_email_from_id_token)
            .or(credentials.email.clone());

        let new_credentials = VertexAIOAuthCredentials {
            access_token: new_access_token,
            refresh_token: credentials.refresh_token,
            client_id: credentials.client_id,
            client_secret: credentials.client_secret,
            project_id: credentials.project_id,
            email,
            expiry_date: Some(new_expiry_date),
        };

        // Cache the new credentials
        *self.cached_credentials.write().await = Some(new_credentials.clone());

        Ok(new_credentials)
    }

    /// Get a valid access token, refreshing if necessary
    pub async fn get_valid_token(
        &self,
        credentials: VertexAIOAuthCredentials,
    ) -> Result<String, RefreshError> {
        // Check cache first
        if let Some(cached) = self.cached_credentials.read().await.clone()
            && cached.is_valid()
        {
            return Ok(cached.access_token);
        }

        // Check if provided credentials are still valid
        if credentials.is_valid() {
            *self.cached_credentials.write().await = Some(credentials.clone());
            return Ok(credentials.access_token);
        }

        // Need to refresh
        let refreshed = self.refresh(credentials).await?;
        Ok(refreshed.access_token)
    }

    /// Extract email from JWT ID token
    fn extract_email_from_id_token(token: &str) -> Option<String> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() < 2 {
            return None;
        }

        // Decode the payload (second part)
        let payload = parts[1];
        let mut padded = payload.replace('-', "+").replace('_', "/");

        // Add padding if needed
        let remainder = padded.len() % 4;
        if remainder > 0 {
            padded.push_str(&"=".repeat(4 - remainder));
        }

        let decoded =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &padded).ok()?;

        let json: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
        json.get("email")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    /// Clear the cached credentials
    pub async fn clear_cache(&self) {
        *self.cached_credentials.write().await = None;
    }
}

impl Default for VertexAITokenRefresher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credentials_expired_no_expiry() {
        let creds = VertexAIOAuthCredentials {
            access_token: "test".to_string(),
            refresh_token: "refresh".to_string(),
            client_id: "id".to_string(),
            client_secret: "secret".to_string(),
            project_id: None,
            email: None,
            expiry_date: None,
        };
        assert!(creds.is_expired());
    }

    #[test]
    fn test_credentials_not_expired() {
        let creds = VertexAIOAuthCredentials {
            access_token: "test".to_string(),
            refresh_token: "refresh".to_string(),
            client_id: "id".to_string(),
            client_secret: "secret".to_string(),
            project_id: None,
            email: None,
            expiry_date: Some(Utc::now() + chrono::Duration::hours(1)),
        };
        assert!(!creds.is_expired());
        assert!(creds.is_valid());
    }
}
