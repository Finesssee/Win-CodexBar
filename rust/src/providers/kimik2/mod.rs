//! Kimi K2 provider implementation
//!
//! Fetches usage data from Kimi K2 API platform
//! Uses API key for credit-based usage totals

use async_trait::async_trait;
use std::collections::HashMap;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};

const KIMIK2_API_BASE_INTERNATIONAL: &str = "https://api.moonshot.ai";
const KIMIK2_API_BASE_CHINA: &str = "https://api.moonshot.cn";

/// Upstream 0.48.0 (`MoonshotSettingsReader`) environment keys.
const MOONSHOT_API_KEY_KEYS: [&str; 3] = ["MOONSHOT_API_KEY", "MOONSHOT_KEY", "KIMI_API_KEY"];
const MOONSHOT_REGION_ENV: &str = "MOONSHOT_REGION";
/// Legacy pre-0.48 region key kept as a fallback signal.
const MOONSHOT_LEGACY_REGION_ENV: &str = "MOONSHOT_API_REGION";
const MOONSHOT_CONFIG_API_KEY_ENV: &str = "CODEXBAR_MOONSHOT_API_KEY";
const MOONSHOT_CONFIG_API_KEY_REGION_ENV: &str = "CODEXBAR_MOONSHOT_API_KEY_REGION";

/// Upstream 0.48.0 `MoonshotRegion`: Kimi Code's regional Open Platform
/// planes. CN/intl keys are bound to their issuing host (#2621).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MoonshotRegion {
    International,
    China,
}

impl MoonshotRegion {
    fn base_url(self) -> &'static str {
        match self {
            MoonshotRegion::International => KIMIK2_API_BASE_INTERNATIONAL,
            MoonshotRegion::China => KIMIK2_API_BASE_CHINA,
        }
    }

    /// Upstream raw values are `international` / `china`; the local aliases
    /// (`cn`, `global`, `intl`) keep older settings working.
    fn parse(raw: &str) -> Option<Self> {
        let mut value = raw.trim();
        if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            value = value[1..value.len() - 1].trim();
        }
        match value.to_ascii_lowercase().as_str() {
            "china" | "cn" => Some(MoonshotRegion::China),
            "international" | "global" | "intl" | "us" => Some(MoonshotRegion::International),
            _ => None,
        }
    }
}

/// Upstream `MoonshotSettingsReader.region`: invalid/unset values default to
/// `international`.
fn region_from_env(env: &HashMap<String, String>) -> MoonshotRegion {
    [MOONSHOT_REGION_ENV, MOONSHOT_LEGACY_REGION_ENV]
        .iter()
        .find_map(|key| env.get(*key).and_then(|raw| MoonshotRegion::parse(raw)))
        .unwrap_or(MoonshotRegion::International)
}

/// Upstream fetch-region: persisted settings win, then environment.
fn effective_region(ctx: &FetchContext, env: &HashMap<String, String>) -> MoonshotRegion {
    ctx.api_region
        .as_deref()
        .and_then(MoonshotRegion::parse)
        .unwrap_or_else(|| region_from_env(env))
}

fn cleaned_value(raw: &str) -> Option<String> {
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

/// Upstream `MoonshotSettingsReader.apiKey(for:)`:
/// 1. `CODEXBAR_MOONSHOT_API_KEY` only when
///    `CODEXBAR_MOONSHOT_API_KEY_REGION` names this region.
/// 2. Ambient keys (`MOONSHOT_API_KEY`, `MOONSHOT_KEY`, local `KIMI_API_KEY`)
///    only when the environment-selected region is this one.
fn api_key_for_region(env: &HashMap<String, String>, region: MoonshotRegion) -> Option<String> {
    let config_key = env
        .get(MOONSHOT_CONFIG_API_KEY_ENV)
        .and_then(|raw| cleaned_value(raw));
    if let Some(config_key) = config_key {
        let bound = env
            .get(MOONSHOT_CONFIG_API_KEY_REGION_ENV)
            .and_then(|raw| MoonshotRegion::parse(raw));
        if bound == Some(region) {
            return Some(config_key);
        }
    }

    if region_from_env(env) != region {
        return None;
    }
    MOONSHOT_API_KEY_KEYS
        .iter()
        .find_map(|key| env.get(*key).and_then(|raw| cleaned_value(raw)))
}

/// Kimi K2 provider (API-based credits)
pub struct KimiK2Provider {
    metadata: ProviderMetadata,
}

impl KimiK2Provider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::KimiK2,
                display_name: "Moonshot / Kimi Open Platform",
                session_label: "Balance",
                weekly_label: "Cash",
                supports_opus: false,
                supports_credits: true,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://platform.moonshot.ai/console/account"),
                status_page_url: None,
            },
        }
    }

    /// Region-bound API key resolution (upstream 0.48.0 #2621):
    /// explicit settings key → region-bound `CODEXBAR_MOONSHOT_API_KEY` →
    /// ambient keys when they belong to the fetched region → legacy local
    /// `config/moonshot/config.json` (ambient class).
    fn get_api_key(
        api_key: Option<&str>,
        region: MoonshotRegion,
        env: &HashMap<String, String>,
    ) -> Option<String> {
        if let Some(key) = api_key.and_then(cleaned_value) {
            return Some(key);
        }

        if let Some(key) = api_key_for_region(env, region) {
            return Some(key);
        }

        // Legacy local config file (ambient class: only for the environment's
        // own region, to keep the host binding invariant).
        if region_from_env(env) == region
            && let Some(config_dir) = dirs::config_dir()
        {
            let config_file = config_dir.join("moonshot").join("config.json");
            if config_file.exists()
                && let Ok(content) = std::fs::read_to_string(&config_file)
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(&content)
                && let Some(key) = json
                    .get("api_key")
                    .and_then(|v| v.as_str())
                    .and_then(cleaned_value)
            {
                return Some(key);
            }
        }

        None
    }

    /// Base URLs to try, in order. `None` (no region signal anywhere) keeps
    /// the legacy dual-fallback for accounts created before region binding;
    /// any explicit region signal pins the request to its issuing plane.
    fn api_bases(region: Option<MoonshotRegion>) -> &'static [&'static str] {
        match region {
            Some(MoonshotRegion::China) => &[KIMIK2_API_BASE_CHINA],
            Some(MoonshotRegion::International) => &[KIMIK2_API_BASE_INTERNATIONAL],
            None => &[KIMIK2_API_BASE_INTERNATIONAL, KIMIK2_API_BASE_CHINA],
        }
    }

    /// Fetch usage via Moonshot API
    async fn fetch_via_api(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProviderError> {
        let env: HashMap<String, String> = std::env::vars().collect();
        let region_signal = ctx
            .api_region
            .as_deref()
            .and_then(MoonshotRegion::parse)
            .or_else(|| {
                [MOONSHOT_REGION_ENV, MOONSHOT_LEGACY_REGION_ENV]
                    .iter()
                    .find_map(|key| env.get(*key).and_then(|raw| MoonshotRegion::parse(raw)))
            });
        let region = region_signal.unwrap_or_else(|| effective_region(ctx, &env));
        let api_key = Self::get_api_key(ctx.api_key.as_deref(), region, &env).ok_or_else(|| {
            ProviderError::NotInstalled(
                "Moonshot API key not found. Set it in Preferences → Providers, MOONSHOT_API_KEY, or MOONSHOT_KEY."
                    .to_string(),
            )
        })?;

        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        let api_bases = Self::api_bases(region_signal);
        let mut auth_error = false;

        for api_base in api_bases {
            let resp = client
                .get(format!("{}/v1/users/me/balance", api_base))
                .header("Authorization", format!("Bearer {}", api_key))
                .header("Accept", "application/json")
                .send()
                .await?;

            if !resp.status().is_success() {
                let status = resp.status();
                if status.as_u16() == 401 || status.as_u16() == 403 || status.as_u16() == 404 {
                    auth_error = true;
                    continue;
                }
                return Err(ProviderError::Other(format!("API error: {}", status)));
            }

            let json: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| ProviderError::Parse(e.to_string()))?;

            return self.parse_usage_response(&json);
        }

        if auth_error {
            Err(ProviderError::AuthRequired)
        } else {
            Err(ProviderError::Other(
                "Moonshot API endpoint not configured".to_string(),
            ))
        }
    }

    /// Parse Kimi K2 usage response
    fn parse_usage_response(
        &self,
        json: &serde_json::Value,
    ) -> Result<UsageSnapshot, ProviderError> {
        let code_ok = json
            .get("code")
            .and_then(|v| v.as_i64())
            .is_none_or(|code| code == 0);
        let status_ok = json.get("status").and_then(|v| v.as_bool()).unwrap_or(true);
        if !code_ok || !status_ok {
            let code = json
                .get("code")
                .map(|v| v.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let scode = json
                .get("scode")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            return Err(ProviderError::Other(format!(
                "Moonshot API error: code {code}, scode {scode}"
            )));
        }

        // Extract balance/credit information
        let data = json.get("data").unwrap_or(json);

        // Available balance (credits remaining)
        let available_balance = data
            .get("available_balance")
            .or_else(|| data.get("balance"))
            .and_then(finite_json_f64)
            .unwrap_or(0.0);

        // Total credits (used + available)
        let total_credits = data
            .get("total_balance")
            .or_else(|| data.get("total"))
            .and_then(finite_json_f64)
            .unwrap_or(available_balance.max(0.0));

        // Used credits
        let used_credits = data
            .get("used_balance")
            .or_else(|| data.get("used"))
            .and_then(finite_json_f64)
            .unwrap_or(total_credits - available_balance);

        // Calculate percentage used
        let used_percent = if total_credits > 0.0 {
            (used_credits / total_credits) * 100.0
        } else {
            0.0
        };

        // Cash balance (if any)
        let voucher_balance = data.get("voucher_balance").and_then(finite_json_f64);
        let cash_balance = data.get("cash_balance").and_then(finite_json_f64);

        // Create primary rate window (credits used)
        let mut primary = RateWindow::new(used_percent);
        primary.reset_description = Some(format!("Balance ${available_balance:.2}"));

        let mut login_method = format!("Balance: ${available_balance:.2}");
        if let Some(cash) = cash_balance
            && cash < 0.0
        {
            login_method.push_str(&format!(" · ${:.2} in deficit", cash.abs()));
        }

        fn finite_json_f64(value: &serde_json::Value) -> Option<f64> {
            match value {
                serde_json::Value::Number(number) => number.as_f64(),
                serde_json::Value::String(text) => text.trim().replace(',', "").parse().ok(),
                _ => None,
            }
            .filter(|value: &f64| value.is_finite())
        }

        let mut usage = UsageSnapshot::new(primary).with_login_method(login_method);

        // Add secondary window for cash balance if available
        if let Some(voucher) = voucher_balance {
            let mut voucher_window = RateWindow::new(0.0);
            voucher_window.reset_description = Some(format!("Voucher ${voucher:.2}"));
            usage = usage.with_extra_rate_window("voucher", "Voucher balance", voucher_window);
        }

        if let Some(cash) = cash_balance {
            let mut cash_window = RateWindow::new(if cash < 0.0 { 100.0 } else { 0.0 });
            cash_window.reset_description = Some(format!("Cash ${cash:.2}"));
            usage = usage.with_extra_rate_window("cash", "Cash balance", cash_window);
        }

        Ok(usage)
    }
}

impl Default for KimiK2Provider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for KimiK2Provider {
    fn id(&self) -> ProviderId {
        ProviderId::KimiK2
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Fetching Kimi K2 usage");

        match ctx.source_mode {
            SourceMode::Auto | SourceMode::Web | SourceMode::OAuth => {
                let usage = self.fetch_via_api(ctx).await?;
                Ok(ProviderFetchResult::new(usage, "api"))
            }
            SourceMode::Cli => Err(ProviderError::UnsupportedSource(SourceMode::Cli)),
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::Web]
    }

    fn supports_web(&self) -> bool {
        true
    }

    fn supports_cli(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn explicit_api_key_overrides_environment_lookup() {
        assert_eq!(
            KimiK2Provider::get_api_key(
                Some("kimi-direct-key"),
                MoonshotRegion::International,
                &env(&[])
            ),
            Some("kimi-direct-key".to_string())
        );
    }

    #[test]
    fn api_key_prefers_moonshot_api_key() {
        let env = env(&[
            ("MOONSHOT_API_KEY", "primary-token"),
            ("MOONSHOT_KEY", "fallback-token"),
        ]);
        assert_eq!(
            api_key_for_region(&env, MoonshotRegion::International).as_deref(),
            Some("primary-token")
        );
    }

    #[test]
    fn api_key_strips_quotes() {
        let env = env(&[("MOONSHOT_KEY", "\"quoted-token\"")]);
        assert_eq!(
            api_key_for_region(&env, MoonshotRegion::International).as_deref(),
            Some("quoted-token")
        );
    }

    #[test]
    fn region_defaults_to_international_for_unknown_values() {
        assert_eq!(
            region_from_env(&env(&[("MOONSHOT_REGION", "moon")])),
            MoonshotRegion::International
        );
        assert_eq!(region_from_env(&env(&[])), MoonshotRegion::International);
    }

    #[test]
    fn region_parses_china() {
        assert_eq!(
            region_from_env(&env(&[("MOONSHOT_REGION", "china")])),
            MoonshotRegion::China
        );
        assert_eq!(
            region_from_env(&env(&[("MOONSHOT_REGION", "MoonshotRegion.china")])),
            MoonshotRegion::International
        );
        // Legacy local key remains a signal.
        assert_eq!(
            region_from_env(&env(&[("MOONSHOT_API_REGION", "cn")])),
            MoonshotRegion::China
        );
        // Upstream key wins over the legacy key.
        assert_eq!(
            region_from_env(&env(&[
                ("MOONSHOT_REGION", "international"),
                ("MOONSHOT_API_REGION", "china")
            ])),
            MoonshotRegion::International
        );
    }

    #[test]
    fn region_bound_config_key_is_unavailable_to_the_other_host() {
        let env = env(&[
            ("CODEXBAR_MOONSHOT_API_KEY", "china-token"),
            ("CODEXBAR_MOONSHOT_API_KEY_REGION", "china"),
        ]);

        assert_eq!(
            api_key_for_region(&env, MoonshotRegion::China).as_deref(),
            Some("china-token")
        );
        assert_eq!(
            api_key_for_region(&env, MoonshotRegion::International),
            None
        );
    }

    #[test]
    fn environment_key_requires_matching_explicit_china_region() {
        let unscoped = env(&[("MOONSHOT_API_KEY", "china-token")]);
        let china = env(&[
            ("MOONSHOT_API_KEY", "china-token"),
            ("MOONSHOT_REGION", "china"),
        ]);

        // Upstream: ambient keys bind to the environment's default region
        // (international) unless MOONSHOT_REGION says otherwise.
        assert_eq!(api_key_for_region(&unscoped, MoonshotRegion::China), None);
        assert_eq!(
            api_key_for_region(&unscoped, MoonshotRegion::International).as_deref(),
            Some("china-token")
        );
        assert_eq!(
            api_key_for_region(&china, MoonshotRegion::China).as_deref(),
            Some("china-token")
        );
        assert_eq!(
            api_key_for_region(&china, MoonshotRegion::International),
            None
        );
    }

    #[test]
    fn api_bases_pin_to_explicit_region_or_fallback_to_both() {
        assert_eq!(
            KimiK2Provider::api_bases(None),
            &[KIMIK2_API_BASE_INTERNATIONAL, KIMIK2_API_BASE_CHINA]
        );
        assert_eq!(
            KimiK2Provider::api_bases(Some(MoonshotRegion::International)),
            &[KIMIK2_API_BASE_INTERNATIONAL]
        );
        assert_eq!(
            KimiK2Provider::api_bases(Some(MoonshotRegion::China)),
            &[KIMIK2_API_BASE_CHINA]
        );
    }

    #[test]
    fn effective_region_prefers_settings_then_env() {
        let env = env(&[("MOONSHOT_REGION", "china")]);
        let ctx = FetchContext {
            api_region: Some("international".to_string()),
            ..FetchContext::default()
        };
        assert_eq!(
            super::effective_region(&ctx, &env),
            MoonshotRegion::International
        );

        let ctx = FetchContext::default();
        assert_eq!(super::effective_region(&ctx, &env), MoonshotRegion::China);
    }

    #[test]
    fn parses_string_balances_and_ignores_non_finite_values() {
        let usage = KimiK2Provider::new()
            .parse_usage_response(&serde_json::json!({
                "data": {
                    "available_balance": "12.50",
                    "total_balance": "50",
                    "used_balance": "37.50",
                    "voucher_balance": "Infinity",
                    "cash_balance": "-5"
                }
            }))
            .unwrap();

        assert_eq!(
            usage.primary.reset_description.as_deref(),
            Some("Balance $12.50")
        );
        assert!((usage.primary.used_percent - 75.0).abs() < f64::EPSILON);
        assert_eq!(usage.extra_rate_windows.len(), 1);
        assert_eq!(usage.extra_rate_windows[0].id, "cash");
    }
}
