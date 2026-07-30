//! Amp provider implementation
//!
//! Amp is Sourcegraph's AI coding assistant
//! Fetches usage data from Amp's local config or API

use async_trait::async_trait;
use std::path::PathBuf;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};

/// Amp provider (Sourcegraph)
pub struct AmpProvider {
    metadata: ProviderMetadata,
}

impl AmpProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Amp,
                display_name: "Amp",
                session_label: "Usage",
                weekly_label: "Monthly",
                supports_opus: false,
                supports_credits: true,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://ampcode.com/settings/usage"),
                status_page_url: Some("https://sourcegraphstatus.com"),
            },
        }
    }

    /// Get Amp config directory
    fn get_amp_config_path() -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        {
            dirs::config_dir().map(|p| p.join("amp"))
        }
        #[cfg(not(target_os = "windows"))]
        {
            dirs::home_dir().map(|p| p.join(".amp"))
        }
    }

    /// Get Sourcegraph/Cody config directory (Amp might use this)
    fn get_cody_config_path() -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        {
            dirs::config_dir().map(|p| p.join("sourcegraph-cody"))
        }
        #[cfg(not(target_os = "windows"))]
        {
            dirs::home_dir().map(|p| p.join(".sourcegraph"))
        }
    }

    /// Read Amp/Sourcegraph access token
    async fn read_access_token(&self, ctx: &FetchContext) -> Result<String, ProviderError> {
        if let Some(token) = access_token_from_context(ctx) {
            return Ok(token);
        }

        if let Some(token) = access_token_from_environment() {
            return Ok(token);
        }

        if let Some(token) = Self::read_local_config_token().await {
            return Ok(token);
        }

        Err(ProviderError::AuthRequired)
    }

    async fn read_local_config_token() -> Option<String> {
        let amp_token = read_access_token_config(Self::get_amp_config_path()).await;
        if amp_token.is_some() {
            return amp_token;
        }

        read_access_token_config(Self::get_cody_config_path()).await
    }

    /// Fetch usage via Sourcegraph API
    async fn fetch_via_web(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProviderError> {
        let token = self.read_access_token(ctx).await?;

        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        // Sourcegraph Cody usage API
        let resp = client
            .get("https://sourcegraph.com/.api/cody/current-user/usage")
            .header("Authorization", format!("token {}", token))
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(ProviderError::AuthRequired);
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;

        self.parse_usage_response(&json)
    }

    fn parse_usage_response(
        &self,
        json: &serde_json::Value,
    ) -> Result<UsageSnapshot, ProviderError> {
        // Parse Sourcegraph/Amp usage response
        let used = json
            .get("completionsUsed")
            .or_else(|| json.get("used"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let limit = json
            .get("completionsLimit")
            .or_else(|| json.get("limit"))
            .and_then(|v| v.as_f64())
            .unwrap_or(500.0);

        let used_percent = if limit > 0.0 {
            (used / limit) * 100.0
        } else {
            0.0
        };

        let plan = json
            .get("plan")
            .or_else(|| json.get("tier"))
            .and_then(|v| v.as_str())
            .unwrap_or("Pro");

        let reset_time = json
            .get("resetAt")
            .or_else(|| json.get("periodEnd"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let primary_window = RateWindow::with_details(used_percent, None, None, reset_time);
        let usage = UsageSnapshot::new(primary_window).with_login_method(plan);

        Ok(usage)
    }

    /// Probe for Amp installation
    async fn probe_cli(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProviderError> {
        // Check ctx.api_key first
        let has_api_key = ctx.api_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false);

        let has_env =
            std::env::var("SRC_ACCESS_TOKEN").is_ok() || std::env::var("AMP_ACCESS_TOKEN").is_ok();

        let has_amp_config = Self::get_amp_config_path()
            .map(|p| p.join("config.json").exists())
            .unwrap_or(false);

        let has_cody_config = Self::get_cody_config_path()
            .map(|p| p.join("config.json").exists())
            .unwrap_or(false);

        if has_api_key || has_env || has_amp_config || has_cody_config {
            let usage =
                UsageSnapshot::new(RateWindow::new(0.0)).with_login_method("Amp (configured)");
            Ok(usage)
        } else {
            Err(ProviderError::NotInstalled(
                "Amp not configured. Set SRC_ACCESS_TOKEN environment variable or configure Amp."
                    .to_string(),
            ))
        }
    }
}

fn access_token_from_context(ctx: &FetchContext) -> Option<String> {
    ctx.api_key
        .as_deref()
        .filter(|api_key| !api_key.is_empty())
        .map(str::to_string)
}

fn access_token_from_environment() -> Option<String> {
    std::env::var("SRC_ACCESS_TOKEN")
        .ok()
        .or_else(|| std::env::var("AMP_ACCESS_TOKEN").ok())
}

async fn read_access_token_config(config_dir: Option<PathBuf>) -> Option<String> {
    let config_file = config_dir?.join("config.json");
    if !config_file.exists() {
        return None;
    }

    let content = tokio::fs::read_to_string(config_file).await.ok()?;
    let json = serde_json::from_str::<serde_json::Value>(&content).ok()?;
    json.get("accessToken")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

impl Default for AmpProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for AmpProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Amp
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Fetching Amp usage");

        match ctx.source_mode {
            SourceMode::Auto => {
                if let Ok(usage) = self.fetch_via_web(ctx).await {
                    return Ok(ProviderFetchResult::new(usage, "web"));
                }
                let usage = self.probe_cli(ctx).await?;
                Ok(ProviderFetchResult::new(usage, "cli"))
            }
            SourceMode::Web => {
                let usage = self.fetch_via_web(ctx).await?;
                Ok(ProviderFetchResult::new(usage, "web"))
            }
            SourceMode::Cli => {
                let usage = self.probe_cli(ctx).await?;
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

/// Monthly pace window sentinel used by Amp subscription/pace UI (30 days).
const AMP_MONTHLY_WINDOW_MINUTES: u32 = 30 * 24 * 60;

/// Parsed Amp subscription (Megawatt-style dual other/orb windows).
#[derive(Debug, Clone, PartialEq)]
pub struct AmpSubscriptionUsage {
    pub plan: String,
    pub other_used_percent: f64,
    pub orb_used_percent: f64,
    pub resets_at: chrono::DateTime<chrono::Utc>,
    pub reset_description: String,
}

/// Parse Amp Free percentage lines from CLI/display text (upstream 0.42.1+ shape).
///
/// Matches lines like:
/// - `Amp Free: 72% remaining today`
/// - `Amp Free: 72% remaining (resets daily)`
///
/// Returns **used** percent (100 - remaining). Not wired into the live fetch path:
/// Win Amp currently uses the Sourcegraph Cody API schema, which does not emit this text.
/// Kept as a pure helper for future CLI/text integration and parity tests.
pub fn parse_amp_free_percent_remaining(text: &str) -> Option<f64> {
    for line in text.lines() {
        let line = line.trim();
        let lower = line.to_ascii_lowercase();
        if !lower.starts_with("amp free:") {
            continue;
        }
        let rest = line["amp free:".len()..].trim();
        // Prefer percentage form over dollar `$used / $quota remaining`.
        let Some(percent_idx) = rest.find('%') else {
            continue;
        };
        let number_part = rest[..percent_idx].trim();
        // Reject dollar amounts mistaken for percentages (e.g. "$12 remaining").
        if number_part.contains('$') {
            continue;
        }
        let after = rest[percent_idx + 1..].trim().to_ascii_lowercase();
        if !after.starts_with("remaining") {
            continue;
        }
        let remaining: f64 = number_part.replace(',', "").parse().ok()?;
        if !remaining.is_finite() {
            continue;
        }
        let clamped = remaining.clamp(0.0, 100.0);
        return Some(100.0 - clamped);
    }
    None
}

/// Parse Amp subscription display text (Megawatt dual other/orb windows).
///
/// Matches:
/// `Subscription Megawatt: 42% other usage and 88% orb usage remaining - resets upon renewal in 12 days`
pub fn parse_amp_subscription_usage(
    text: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<AmpSubscriptionUsage> {
    let re = regex_lite::Regex::new(
        r"(?im)^\s*Subscription\s+(.+?):\s*([0-9][0-9,]*(?:\.[0-9]+)?)\s*%\s+other\s+usage\s+and\s+([0-9][0-9,]*(?:\.[0-9]+)?)\s*%\s+orb\s+usage\s+remaining\s*-\s*resets\s+upon\s+renewal\s+in\s+([0-9][0-9,]*)\s+days?(?:\s+-\s+https?://\S+)?\s*$",
    )
    .ok()?;

    for line in text.lines() {
        let Some(caps) = re.captures(line) else {
            continue;
        };
        let plan = caps.get(1)?.as_str().trim();
        if plan.is_empty() {
            continue;
        }
        let other_remaining = parse_amp_number(caps.get(2)?.as_str())?;
        let orb_remaining = parse_amp_number(caps.get(3)?.as_str())?;
        let renewal_days: i64 = caps.get(4)?.as_str().replace(',', "").parse().ok()?;
        if renewal_days < 0 {
            continue;
        }
        let reset_description = if renewal_days == 1 {
            "renews in 1 day".to_string()
        } else {
            format!("renews in {renewal_days} days")
        };
        return Some(AmpSubscriptionUsage {
            plan: plan.to_string(),
            other_used_percent: 100.0 - other_remaining.clamp(0.0, 100.0),
            orb_used_percent: 100.0 - orb_remaining.clamp(0.0, 100.0),
            resets_at: now + chrono::Duration::days(renewal_days),
            reset_description,
        });
    }
    None
}

/// Build a [`UsageSnapshot`] from Amp Free / subscription display text.
///
/// Subscription (Megawatt) wins for primary/secondary windows when present:
/// - primary = other usage
/// - secondary = orb usage
///
/// Free percent path fills primary when there is no subscription match.
pub fn usage_snapshot_from_amp_display_text(
    text: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<UsageSnapshot> {
    if let Some(sub) = parse_amp_subscription_usage(text, now) {
        // Labels: primary = "Other usage", secondary = "Orb usage"
        // (surfaced via provider session/weekly labels + plan login_method).
        let other = RateWindow::with_details(
            sub.other_used_percent,
            Some(AMP_MONTHLY_WINDOW_MINUTES),
            Some(sub.resets_at),
            Some(sub.reset_description.clone()),
        );
        let orb = RateWindow::with_details(
            sub.orb_used_percent,
            Some(AMP_MONTHLY_WINDOW_MINUTES),
            Some(sub.resets_at),
            Some(sub.reset_description),
        );
        return Some(
            UsageSnapshot::new(other)
                .with_secondary(orb)
                .with_login_method(sub.plan),
        );
    }

    let free_used = parse_amp_free_percent_remaining(text)?;
    let primary = RateWindow::with_details(
        free_used,
        Some(24 * 60),
        None,
        Some("resets daily".to_string()),
    );
    Some(UsageSnapshot::new(primary).with_login_method("Amp Free"))
}

fn parse_amp_number(raw: &str) -> Option<f64> {
    let value: f64 = raw.replace(',', "").parse().ok()?;
    value.is_finite().then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn dashboard_points_to_current_usage_page() {
        assert_eq!(
            AmpProvider::new().metadata().dashboard_url,
            Some("https://ampcode.com/settings/usage")
        );
    }

    #[test]
    fn parses_amp_free_percent_remaining_today() {
        let text = "Signed in as user@example.com\nAmp Free: 72% remaining today\n";
        assert_eq!(parse_amp_free_percent_remaining(text), Some(28.0));
    }

    #[test]
    fn parses_amp_free_percent_resets_daily() {
        let text = "Amp Free: 100% remaining (resets daily)";
        assert_eq!(parse_amp_free_percent_remaining(text), Some(0.0));
    }

    #[test]
    fn ignores_dollar_remaining_form() {
        let text = "Amp Free: $4.20 / $10 remaining (replenishes +$1 / hour)";
        assert_eq!(parse_amp_free_percent_remaining(text), None);
    }

    #[test]
    fn returns_none_when_amp_free_missing() {
        assert_eq!(
            parse_amp_free_percent_remaining("Individual credits: $3 remaining"),
            None
        );
    }

    #[test]
    fn parses_megawatt_subscription_dual_windows() {
        let now = Utc.with_ymd_and_hms(2026, 7, 1, 12, 0, 0).unwrap();
        let text = "Signed in as user@example.com (Acme)\n\
Subscription Megawatt: 42% other usage and 88% orb usage remaining - resets upon renewal in 12 days\n";
        let sub = parse_amp_subscription_usage(text, now).expect("subscription");
        assert_eq!(sub.plan, "Megawatt");
        assert!((sub.other_used_percent - 58.0).abs() < f64::EPSILON);
        assert!((sub.orb_used_percent - 12.0).abs() < f64::EPSILON);
        assert_eq!(sub.reset_description, "renews in 12 days");
        assert_eq!(sub.resets_at, now + chrono::Duration::days(12));

        let snapshot = usage_snapshot_from_amp_display_text(text, now).expect("snapshot");
        assert!((snapshot.primary.used_percent - 58.0).abs() < f64::EPSILON);
        assert_eq!(
            snapshot.primary.window_minutes,
            Some(AMP_MONTHLY_WINDOW_MINUTES)
        );
        assert_eq!(
            snapshot.primary.reset_description.as_deref(),
            Some("renews in 12 days")
        );
        let secondary = snapshot.secondary.expect("orb secondary");
        assert!((secondary.used_percent - 12.0).abs() < f64::EPSILON);
        assert_eq!(secondary.window_minutes, Some(AMP_MONTHLY_WINDOW_MINUTES));
        assert_eq!(snapshot.login_method.as_deref(), Some("Megawatt"));
    }

    #[test]
    fn megawatt_one_day_renewal_wording() {
        let now = Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap();
        let text = "Subscription Megawatt: 0% other usage and 100% orb usage remaining - resets upon renewal in 1 day";
        let sub = parse_amp_subscription_usage(text, now).unwrap();
        assert_eq!(sub.reset_description, "renews in 1 day");
        assert!((sub.other_used_percent - 100.0).abs() < f64::EPSILON);
        assert!((sub.orb_used_percent - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn free_path_still_builds_snapshot() {
        let now = Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap();
        let text = "Amp Free: 72% remaining today";
        let snapshot = usage_snapshot_from_amp_display_text(text, now).unwrap();
        assert!((snapshot.primary.used_percent - 28.0).abs() < f64::EPSILON);
        assert!(snapshot.secondary.is_none());
        assert_eq!(snapshot.login_method.as_deref(), Some("Amp Free"));
    }
}
