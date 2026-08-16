//! Status page polling for AI providers
//!
//! Fetches operational status from provider status pages

#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Status level for a provider
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum StatusLevel {
    /// All systems operational
    Operational,
    /// Degraded performance
    Degraded,
    /// Partial outage
    Partial,
    /// Major outage
    Major,
    /// Unknown status
    #[default]
    Unknown,
}

impl StatusLevel {
    /// Create from a string indicator
    pub fn from_indicator(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "operational" | "none" | "green" | "ok" => StatusLevel::Operational,
            "degraded" | "degraded_performance" | "yellow" => StatusLevel::Degraded,
            "partial" | "partial_outage" | "orange" => StatusLevel::Partial,
            "major" | "major_outage" | "critical" | "red" => StatusLevel::Major,
            _ => StatusLevel::Unknown,
        }
    }

    /// Get a human-readable description
    pub fn description(&self) -> &'static str {
        match self {
            StatusLevel::Operational => "All Systems Operational",
            StatusLevel::Degraded => "Degraded Performance",
            StatusLevel::Partial => "Partial Outage",
            StatusLevel::Major => "Major Outage",
            StatusLevel::Unknown => "Status Unknown",
        }
    }
}

/// Provider status information
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderStatus {
    pub level: StatusLevel,
    pub description: String,
    pub last_updated: Option<String>,
    pub components: Vec<ComponentStatus>,
}

/// Individual component status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentStatus {
    pub name: String,
    pub status: StatusLevel,
}

/// Status page URLs for known providers
pub fn get_status_page_url(provider: &str) -> Option<&'static str> {
    match provider.to_lowercase().as_str() {
        "claude" | "anthropic" => Some("https://status.anthropic.com"),
        "codex" | "openai" => Some("https://status.openai.com"),
        "gemini" | "google" => Some("https://status.cloud.google.com"),
        "copilot" | "github" => Some("https://www.githubstatus.com"),
        "cursor" => Some("https://status.cursor.com"),
        "factory" | "droid" => None, // Factory.ai doesn't have a public status page
        "zai" | "z.ai" => None,      // z.ai doesn't have a public status page
        _ => None,
    }
}

/// Fetch status from a Statuspage.io-based status page
pub async fn fetch_statuspage_io(url: &str) -> Result<ProviderStatus, String> {
    let client = crate::core::apply_app_proxy(reqwest::Client::builder())
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    // Statuspage.io API endpoint
    let api_url = format!("{}/api/v2/status.json", url.trim_end_matches('/'));

    let resp = client
        .get(&api_url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    // Parse Statuspage.io format
    let status = json
        .get("status")
        .and_then(|s| s.get("indicator"))
        .and_then(|i| i.as_str())
        .map(StatusLevel::from_indicator)
        .unwrap_or(StatusLevel::Unknown);

    let description = json
        .get("status")
        .and_then(|s| s.get("description"))
        .and_then(|d| d.as_str())
        .unwrap_or("Unknown")
        .to_string();

    let last_updated = json
        .get("page")
        .and_then(|p| p.get("updated_at"))
        .and_then(|u| u.as_str())
        .map(|s| s.to_string());

    Ok(ProviderStatus {
        level: status,
        description,
        last_updated,
        components: Vec::new(),
    })
}

/// Fetch status with components from a Statuspage.io-based status page
pub async fn fetch_statuspage_io_components(url: &str) -> Result<ProviderStatus, String> {
    let client = crate::core::apply_app_proxy(reqwest::Client::builder())
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    // Statuspage.io components endpoint
    let api_url = format!("{}/api/v2/components.json", url.trim_end_matches('/'));

    let resp = client
        .get(&api_url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    let mut components = Vec::new();
    let mut overall_status = StatusLevel::Operational;

    if let Some(comps) = json.get("components").and_then(|c| c.as_array()) {
        for comp in comps {
            let name = comp
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("Unknown");
            let status_str = comp
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("unknown");
            let status = StatusLevel::from_indicator(status_str);

            // Update overall status to worst component
            if (status as u8) > (overall_status as u8) {
                overall_status = status;
            }

            components.push(ComponentStatus {
                name: name.to_string(),
                status,
            });
        }
    }

    Ok(ProviderStatus {
        level: overall_status,
        description: overall_status.description().to_string(),
        last_updated: None,
        components,
    })
}

/// Fetch status for a specific provider.
///
/// Upstream 0.50.0 #2925: transport errors are omitted until a real status
/// fetch succeeds — a failed refresh never replaces the last successful
/// result, so long-running consumers (hooks watch, serve) keep showing the
/// last known status instead of dropping to "unknown".
pub async fn fetch_provider_status(provider: &str) -> Option<ProviderStatus> {
    let url = get_status_page_url(provider)?;

    // Try the simple status endpoint first
    let fresh = match fetch_statuspage_io(url).await {
        Ok(status) => Some(status),
        Err(_) => {
            // Fall back to components endpoint
            fetch_statuspage_io_components(url).await.ok()
        }
    };
    match fresh {
        Some(status) => {
            cache_last_success(provider, &status);
            Some(status)
        }
        None => last_cached_success(provider),
    }
}

/// Process-wide cache of the last successful status per provider.
fn status_cache() -> &'static std::sync::Mutex<HashMap<String, ProviderStatus>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, ProviderStatus>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn cache_last_success(provider: &str, status: &ProviderStatus) {
    if let Ok(mut cache) = status_cache().lock() {
        cache.insert(provider.to_string(), status.clone());
    }
}

fn last_cached_success(provider: &str) -> Option<ProviderStatus> {
    status_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(provider).cloned())
}

/// Fetch status for all providers in parallel
pub async fn fetch_all_statuses(providers: &[&str]) -> HashMap<String, ProviderStatus> {
    let futures: Vec<_> = providers
        .iter()
        .map(|&p| async move {
            let status = fetch_provider_status(p).await;
            (p.to_string(), status)
        })
        .collect();

    let results = futures::future::join_all(futures).await;

    results
        .into_iter()
        .filter_map(|(provider, status)| status.map(|s| (provider, s)))
        .collect()
}
