//! Wayfinder gateway provider.
//!
//! Wayfinder is a local-first gateway, so it has no account, quota, or
//! credential data. The provider exposes only gateway health, configured
//! models, and savings telemetry.

use async_trait::async_trait;
use reqwest::Url;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::net::IpAddr;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot, WayfinderRouteSummary, WayfinderUsageSnapshot,
    credentialed_http_client_builder,
};

pub const DEFAULT_GATEWAY_URL: &str = "http://127.0.0.1:8088";

const HEALTH_PATH: &str = "healthz";
const MODELS_PATH: &str = "router/models";
const SAVINGS_PATH: &str = "v1/savings?period=30d";
const METRICS_PATH: &str = "metrics";

#[derive(Debug, Clone, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub offline: bool,
    #[serde(default)]
    pub missing_keys: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelsResponse {
    #[serde(default)]
    pub models: Vec<ModelResponse>,
    #[serde(default)]
    pub dry_run: bool,
}

impl ModelsResponse {
    pub fn model_count(&self) -> usize {
        self.models.len()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelResponse {
    pub name: String,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub key_ok: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SavingsResponse {
    pub period_days: u32,
    pub unit: String,
    pub priced: bool,
    pub requests: u64,
    #[serde(default)]
    pub estimated_requests: u64,
    pub tokens: u64,
    pub realized: f64,
    pub baseline: f64,
    pub saved: f64,
    pub saved_pct: f64,
    #[serde(default)]
    pub by_route: BTreeMap<String, RouteResponse>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RouteResponse {
    pub requests: u64,
    pub realized: f64,
    pub baseline: f64,
    pub saved: f64,
    pub tokens: u64,
}

pub fn parse_gateway_url(raw: &str) -> Result<Url, ProviderError> {
    let trimmed = raw.trim();
    let url = Url::parse(trimmed)
        .map_err(|_| ProviderError::Other("Wayfinder gateway URL is invalid".to_string()))?;

    if url.scheme() != "http"
        || url.username() != ""
        || url.password().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
    {
        return Err(ProviderError::Other(
            "Wayfinder gateway URL must be HTTP without credentials or a fragment".to_string(),
        ));
    }

    let scheme_end = trimmed.find("://").map(|index| index + 3).unwrap_or(0);
    let authority_end = trimmed[scheme_end..]
        .find(['/', '?', '#'])
        .map(|index| scheme_end + index)
        .unwrap_or(trimmed.len());
    let authority = &trimmed[scheme_end..authority_end];
    if authority.contains('%') || authority.chars().any(char::is_whitespace) {
        return Err(ProviderError::Other(
            "Wayfinder gateway URL contains an encoded or invalid host".to_string(),
        ));
    }

    if !is_loopback_host(&url) {
        return Err(ProviderError::Other(
            "Wayfinder gateway must use localhost or a loopback address".to_string(),
        ));
    }

    Ok(url)
}

fn is_loopback_host(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_matches(['[', ']']);
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

pub fn endpoint_url(base: &Url, path: &str) -> Result<Url, ProviderError> {
    let mut base = base.clone();
    let base_path = base.path().trim_end_matches('/');
    base.set_path(&format!("{base_path}/"));
    base.join(path)
        .map_err(|_| ProviderError::Other("Wayfinder gateway path is invalid".to_string()))
}

pub fn parse_health(raw: &str) -> Result<HealthResponse, ProviderError> {
    serde_json::from_str(raw)
        .map_err(|_| ProviderError::Parse("Invalid Wayfinder health response".to_string()))
}

pub fn parse_models(raw: &str) -> Result<ModelsResponse, ProviderError> {
    serde_json::from_str(raw)
        .map_err(|_| ProviderError::Parse("Invalid Wayfinder models response".to_string()))
}

pub fn parse_savings(raw: &str) -> Result<SavingsResponse, ProviderError> {
    serde_json::from_str(raw)
        .map_err(|_| ProviderError::Parse("Invalid Wayfinder savings response".to_string()))
}

pub struct WayfinderProvider {
    metadata: ProviderMetadata,
}

impl WayfinderProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Wayfinder,
                display_name: "Wayfinder",
                session_label: "Gateway",
                weekly_label: "Savings",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: None,
                status_page_url: None,
            },
        }
    }

    async fn fetch_json<T: DeserializeOwned>(
        client: &reqwest::Client,
        url: &Url,
    ) -> Result<T, ProviderError> {
        let response = client.get(url.clone()).send().await?;
        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Wayfinder gateway returned HTTP {}",
                response.status()
            )));
        }
        response
            .json::<T>()
            .await
            .map_err(|_| ProviderError::Parse("Invalid Wayfinder gateway response".to_string()))
    }

    async fn fetch_metrics(client: &reqwest::Client, url: &Url) -> Result<(), ProviderError> {
        let response = client.get(url.clone()).send().await?;
        if response.status().is_success() {
            let _ = response.text().await?;
        }
        Ok(())
    }
}

impl Default for WayfinderProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for WayfinderProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Wayfinder
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        if ctx.source_mode != SourceMode::Auto {
            return Err(ProviderError::UnsupportedSource(ctx.source_mode));
        }

        let base = parse_gateway_url(ctx.gateway_url.as_deref().unwrap_or(DEFAULT_GATEWAY_URL))?;
        let client = credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(ctx.web_timeout.max(1)))
            .build()
            .map_err(ProviderError::Network)?;

        let health: HealthResponse =
            Self::fetch_json(&client, &endpoint_url(&base, HEALTH_PATH)?).await?;
        let models: ModelsResponse =
            Self::fetch_json(&client, &endpoint_url(&base, MODELS_PATH)?).await?;
        let savings: SavingsResponse =
            Self::fetch_json(&client, &endpoint_url(&base, SAVINGS_PATH)?).await?;
        // Metrics are best-effort enrichment; a failure must not fail the usage fetch.
        let _metrics = Self::fetch_metrics(&client, &endpoint_url(&base, METRICS_PATH)?).await;

        let wayfinder_usage = WayfinderUsageSnapshot {
            gateway_status: health.status,
            offline: health.offline,
            dry_run: models.dry_run,
            missing_keys: health.missing_keys,
            model_count: models.model_count(),
            models: models.models.into_iter().map(|model| model.name).collect(),
            requests: savings.requests,
            estimated_requests: savings.estimated_requests,
            tokens: savings.tokens,
            realized: savings.realized,
            baseline: savings.baseline,
            saved: savings.saved,
            saved_percent: savings.saved_pct,
            period_days: savings.period_days,
            unit: savings.unit,
            priced: savings.priced,
            routes: savings
                .by_route
                .into_iter()
                .map(|(name, route)| WayfinderRouteSummary {
                    name,
                    requests: route.requests,
                    tokens: route.tokens,
                    realized: route.realized,
                    baseline: route.baseline,
                    saved: route.saved,
                })
                .collect(),
        };

        Ok(
            ProviderFetchResult::new(UsageSnapshot::new(RateWindow::new(0.0)), "gateway")
                .with_wayfinder_usage(wayfinder_usage),
        )
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_loopback_http() {
        for raw in [
            "http://127.0.0.1:8088",
            "http://127.42.1.9:8088",
            "http://localhost:8088",
            "http://[::1]:8088",
        ] {
            assert!(parse_gateway_url(raw).is_ok(), "{raw}");
        }
    }

    #[test]
    fn rejects_unsafe_gateway_urls() {
        for raw in [
            "127.0.0.1:8088",
            "ftp://127.0.0.1:8088",
            "https://127.0.0.1:8088",
            "https://gateway.example.test/wayfinder",
            "http://192.168.1.20:8088",
            "http://user:password@127.0.0.1:8088",
            "http://127.0.0.1:8088/#fragment",
            "http://127.0.0.1%2f.attacker.test:8088",
            "https://proxy.test%2f.attacker.test/v1",
        ] {
            assert!(parse_gateway_url(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn endpoint_paths_preserve_gateway_prefix() {
        let base = parse_gateway_url("http://localhost:8088/wayfinder/").unwrap();
        assert_eq!(
            endpoint_url(&base, "healthz").unwrap().as_str(),
            "http://localhost:8088/wayfinder/healthz"
        );
    }

    #[test]
    fn parses_upstream_fixtures_without_identity_or_quota_fields() {
        let health = parse_health(include_str!("fixtures/wayfinder/health.json")).unwrap();
        let models = parse_models(include_str!("fixtures/wayfinder/models.json")).unwrap();
        let savings = parse_savings(include_str!("fixtures/wayfinder/savings.json")).unwrap();

        assert_eq!(health.status, "ok");
        assert!(!health.offline);
        assert!(health.missing_keys.is_empty());
        assert_eq!(models.model_count(), 2);
        assert!(!models.dry_run);
        assert_eq!(savings.requests, 14);
        assert_eq!(savings.tokens, 1028);
        assert_eq!(savings.by_route["local"].requests, 10);
        assert_eq!(savings.saved, 0.005694);
    }

    #[test]
    fn malformed_payloads_are_rejected() {
        assert!(parse_health("{\"status\":").is_err());
        assert!(parse_models("{\"models\": [").is_err());
        assert!(parse_savings("{\"requests\": \"many\"}").is_err());
    }
}
