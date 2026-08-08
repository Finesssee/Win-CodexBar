use std::sync::Mutex;
use std::time::Duration;

use chrono::Utc;
use serde::Serialize;
use tauri::Manager;

use crate::commands::{
    ProviderLocalUsageSummary, ProviderUsageSnapshot, RateWindowSnapshot,
    cached_provider_local_usage_summary,
};
use crate::state::AppState;

pub const STATUS_PIPE_NAME: &str = r"\\.\pipe\WinCodexBar.Status";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerToysSnapshot {
    version: u32,
    updated_at: String,
    providers: Vec<PowerToysProviderSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerToysProviderSnapshot {
    id: String,
    name: String,
    status_text: String,
    subtitle: Option<String>,
    primary_label: Option<String>,
    primary: RateWindowSnapshot,
    secondary_label: Option<String>,
    secondary: Option<RateWindowSnapshot>,
    today_cost: Option<f64>,
    thirty_day_cost: Option<f64>,
    latest_tokens: Option<u64>,
    thirty_day_tokens: Option<u64>,
    top_model: Option<String>,
    updated_at: String,
    error: Option<String>,
}

#[cfg(not(windows))]
pub fn install(_app: tauri::AppHandle) {}

#[cfg(windows)]
pub fn install(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        run_status_pipe(app).await;
    });
}

pub fn snapshot(app: &tauri::AppHandle) -> PowerToysSnapshot {
    let providers = app
        .state::<Mutex<AppState>>()
        .lock()
        .map(|guard| guard.provider_cache.clone())
        .unwrap_or_default()
        .into_iter()
        .map(provider_snapshot)
        .collect();

    PowerToysSnapshot {
        version: 1,
        updated_at: Utc::now().to_rfc3339(),
        providers,
    }
}

fn provider_snapshot(provider: ProviderUsageSnapshot) -> PowerToysProviderSnapshot {
    let local_usage = cached_local_usage(&provider.provider_id);
    let status_text = if provider.error.is_some() {
        "error".to_string()
    } else {
        format!("{}%", provider.primary.used_percent.round())
    };
    let subtitle = provider_subtitle(&provider, local_usage.as_ref());

    PowerToysProviderSnapshot {
        id: provider.provider_id,
        name: provider.display_name,
        status_text,
        subtitle,
        primary_label: provider.primary_label,
        primary: provider.primary,
        secondary_label: provider.secondary_label,
        secondary: provider.secondary,
        today_cost: local_usage.as_ref().and_then(|summary| summary.today_cost),
        thirty_day_cost: local_usage
            .as_ref()
            .and_then(|summary| summary.thirty_day_cost),
        latest_tokens: local_usage
            .as_ref()
            .and_then(|summary| summary.latest_tokens),
        thirty_day_tokens: local_usage
            .as_ref()
            .and_then(|summary| summary.thirty_day_tokens),
        top_model: local_usage.and_then(|summary| summary.top_model),
        updated_at: provider.updated_at,
        error: provider.error,
    }
}

fn provider_subtitle(
    provider: &ProviderUsageSnapshot,
    local_usage: Option<&ProviderLocalUsageSummary>,
) -> Option<String> {
    let mut parts = Vec::new();
    if let (Some(label), Some(secondary)) = (&provider.secondary_label, &provider.secondary) {
        parts.push(format!("{} {}%", label, secondary.used_percent.round()));
    }
    if let Some(cost) = local_usage.and_then(|summary| summary.today_cost) {
        parts.push(format!("Today ${cost:.2}"));
    }
    if parts.is_empty() {
        provider
            .primary
            .reset_description
            .as_ref()
            .map(|reset| reset.to_string())
    } else {
        Some(parts.join(" · "))
    }
}

fn cached_local_usage(provider_id: &str) -> Option<ProviderLocalUsageSummary> {
    cached_provider_local_usage_summary(provider_id)
}

#[cfg(windows)]
async fn run_status_pipe(app: tauri::AppHandle) {
    use tokio::io::AsyncWriteExt;
    use tokio::net::windows::named_pipe::ServerOptions;

    loop {
        let server = match ServerOptions::new().create(STATUS_PIPE_NAME) {
            Ok(server) => server,
            Err(err) => {
                tracing::warn!("failed to create PowerToys status pipe: {err}");
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };

        if let Err(err) = server.connect().await {
            tracing::warn!("PowerToys status pipe connection failed: {err}");
            continue;
        }

        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let mut server = server;
            let payload = serde_json::to_vec(&snapshot(&app)).unwrap_or_else(|err| {
                tracing::warn!("failed to serialize PowerToys snapshot: {err}");
                b"{\"version\":1,\"providers\":[]}".to_vec()
            });
            let _ = server.write_all(&payload).await;
            let _ = server.write_all(b"\n").await;
            let _ = server.flush().await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate_window(used_percent: f64) -> RateWindowSnapshot {
        RateWindowSnapshot {
            used_percent,
            remaining_percent: 100.0 - used_percent,
            window_minutes: None,
            resets_at: None,
            reset_description: None,
            is_exhausted: false,
            is_informational: false,
            reserve_percent: None,
            reserve_description: None,
            reserve_eta_seconds: None,
            reserve_will_last_to_reset: false,
        }
    }

    #[test]
    fn provider_snapshot_omits_account_identity_fields() {
        let snapshot = provider_snapshot(ProviderUsageSnapshot {
            provider_id: "test-provider".to_string(),
            display_name: "Test Provider".to_string(),
            primary: rate_window(42.0),
            primary_label: Some("Session".to_string()),
            secondary: None,
            secondary_label: None,
            model_specific: None,
            tertiary: None,
            tertiary_label: None,
            extra_rate_windows: Vec::new(),
            cost: None,
            plan_name: Some("Team".to_string()),
            account_email: Some("dev@example.com".to_string()),
            source_label: "web".to_string(),
            updated_at: "2026-07-09T00:00:00Z".to_string(),
            error: None,
            pace: None,
            account_organization: Some("Example Org".to_string()),
            tray_status_label: None,
            fetch_duration_ms: None,
            wayfinder_usage: None,
            session_equivalent_forecast: None,
        });
        let value = serde_json::to_value(snapshot).unwrap();

        assert!(value.get("planName").is_none());
        assert!(value.get("accountEmail").is_none());
    }

    #[test]
    fn provider_snapshot_includes_cached_local_usage_fields() {
        crate::commands::cache_provider_local_usage_summary_for_test(
            "test-provider",
            Some(ProviderLocalUsageSummary {
                today_cost: Some(1.25),
                thirty_day_cost: Some(12.5),
                thirty_day_tokens: Some(42_000),
                latest_tokens: Some(1_200),
                top_model: Some("gpt-5".to_string()),
                estimate_note: "cached".to_string(),
                token_cost_updated_at_ms: 1234,
            }),
        );

        let snapshot = provider_snapshot(ProviderUsageSnapshot {
            provider_id: "test-provider".to_string(),
            display_name: "Test Provider".to_string(),
            primary: rate_window(42.0),
            primary_label: Some("Session".to_string()),
            secondary: None,
            secondary_label: None,
            model_specific: None,
            tertiary: None,
            tertiary_label: None,
            extra_rate_windows: Vec::new(),
            cost: None,
            plan_name: None,
            account_email: None,
            source_label: "web".to_string(),
            updated_at: "2026-07-09T00:00:00Z".to_string(),
            error: None,
            pace: None,
            account_organization: None,
            tray_status_label: None,
            fetch_duration_ms: None,
            wayfinder_usage: None,
            session_equivalent_forecast: None,
        });
        let value = serde_json::to_value(snapshot).unwrap();

        assert_eq!(value.get("todayCost").and_then(|v| v.as_f64()), Some(1.25));
        assert_eq!(
            value.get("thirtyDayCost").and_then(|v| v.as_f64()),
            Some(12.5)
        );
        assert_eq!(
            value.get("latestTokens").and_then(|v| v.as_u64()),
            Some(1_200)
        );
        assert_eq!(
            value.get("thirtyDayTokens").and_then(|v| v.as_u64()),
            Some(42_000)
        );
        assert_eq!(
            value.get("topModel").and_then(|v| v.as_str()),
            Some("gpt-5")
        );
    }
}
