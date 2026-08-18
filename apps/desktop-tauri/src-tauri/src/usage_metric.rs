//! Canonical single-metric selection shared by native and webview surfaces.

use std::cmp::Ordering;

use codexbar::core::ProviderId;
use codexbar::settings::{MetricPreference, Settings};

use crate::commands::{ProviderUsageSnapshot, RateWindowSnapshot};

pub(crate) fn selected_usage_window(
    snapshot: &ProviderUsageSnapshot,
    settings: &Settings,
) -> RateWindowSnapshot {
    let provider = ProviderId::from_cli_name(&snapshot.provider_id);
    let preference = provider
        .map(|id| settings.get_provider_metric(id))
        .unwrap_or_default();

    preferred_window(snapshot, provider, preference)
        .or_else(|| automatic_window(snapshot, provider))
        .unwrap_or_else(|| snapshot.primary.clone())
}

fn preferred_window(
    snapshot: &ProviderUsageSnapshot,
    provider: Option<ProviderId>,
    preference: MetricPreference,
) -> Option<RateWindowSnapshot> {
    match preference {
        MetricPreference::Automatic => automatic_window(snapshot, provider),
        // A missing session is represented by an informational zero-percent
        // placeholder. Fall through to Automatic instead of displaying it.
        MetricPreference::Session if snapshot.primary.is_informational => None,
        MetricPreference::Session => Some(snapshot.primary.clone()),
        MetricPreference::Weekly => non_informational(snapshot.secondary.as_ref())
            .or_else(|| non_informational(Some(&snapshot.primary)))
            .cloned(),
        MetricPreference::Model => snapshot
            .model_specific
            .clone()
            .or_else(|| non_informational(Some(&snapshot.primary)).cloned()),
        MetricPreference::Tertiary => snapshot
            .tertiary
            .clone()
            .or_else(|| snapshot.secondary.clone())
            .or_else(|| non_informational(Some(&snapshot.primary)).cloned()),
        MetricPreference::Credits => cost_window(snapshot),
        MetricPreference::ExtraUsage => {
            extra_usage_window(snapshot).or_else(|| cost_window(snapshot))
        }
        MetricPreference::Average => average_window(snapshot),
        MetricPreference::MonthlyPlan => cost_window(snapshot),
    }
}

fn automatic_window(
    snapshot: &ProviderUsageSnapshot,
    provider: Option<ProviderId>,
) -> Option<RateWindowSnapshot> {
    if provider == Some(ProviderId::Claude) {
        let weekly = non_informational(snapshot.secondary.as_ref());
        if let (Some(model), Some(weekly)) = (snapshot.model_specific.as_ref(), weekly) {
            let model_exhausted = model.is_exhausted || model.used_percent >= 100.0;
            let weekly_has_remaining = !weekly.is_exhausted && weekly.used_percent < 100.0;
            if model_exhausted && weekly_has_remaining {
                return Some(weekly.clone());
            }
        }
        if snapshot.primary.is_informational {
            return weekly.cloned();
        }
    }

    highest_window(
        std::iter::once(&snapshot.primary)
            .chain(snapshot.secondary.iter())
            .chain(snapshot.model_specific.iter())
            .chain(snapshot.tertiary.iter())
            .chain(
                snapshot
                    .extra_rate_windows
                    .iter()
                    .map(|extra| &extra.window),
            )
            .filter(|window| !window.is_informational),
    )
    .cloned()
}

fn average_window(snapshot: &ProviderUsageSnapshot) -> Option<RateWindowSnapshot> {
    if snapshot.primary.is_informational {
        return snapshot.secondary.clone();
    }
    let secondary = snapshot.secondary.as_ref()?;
    Some(derived_window(
        (snapshot.primary.used_percent + secondary.used_percent) / 2.0,
        None,
    ))
}

fn cost_window(snapshot: &ProviderUsageSnapshot) -> Option<RateWindowSnapshot> {
    let cost = snapshot.cost.as_ref()?;
    let limit = cost.limit?;
    if limit <= 0.0 {
        return None;
    }
    Some(derived_window(
        (cost.used / limit) * 100.0,
        cost.resets_at.clone(),
    ))
}

fn extra_usage_window(snapshot: &ProviderUsageSnapshot) -> Option<RateWindowSnapshot> {
    highest_window(
        snapshot
            .extra_rate_windows
            .iter()
            .map(|extra| &extra.window),
    )
    .cloned()
}

fn derived_window(used_percent: f64, resets_at: Option<String>) -> RateWindowSnapshot {
    let used_percent = used_percent.clamp(0.0, 100.0);
    RateWindowSnapshot {
        used_percent,
        remaining_percent: 100.0 - used_percent,
        window_minutes: None,
        resets_at,
        reset_description: None,
        is_exhausted: used_percent >= 100.0,
        is_informational: false,
        reserve_percent: None,
        reserve_description: None,
        reserve_will_last_to_reset: false,
        reserve_eta_seconds: None,
    }
}

fn non_informational(window: Option<&RateWindowSnapshot>) -> Option<&RateWindowSnapshot> {
    window.filter(|window| !window.is_informational)
}

fn highest_window<'a>(
    windows: impl Iterator<Item = &'a RateWindowSnapshot>,
) -> Option<&'a RateWindowSnapshot> {
    windows.max_by(|a, b| {
        a.used_percent
            .partial_cmp(&b.used_percent)
            .unwrap_or(Ordering::Equal)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(used_percent: f64) -> RateWindowSnapshot {
        derived_window(used_percent, None)
    }

    fn snapshot() -> ProviderUsageSnapshot {
        ProviderUsageSnapshot {
            provider_id: "codex".to_string(),
            display_name: "Codex".to_string(),
            primary: window(20.0),
            primary_label: None,
            secondary: Some(window(60.0)),
            secondary_label: None,
            model_specific: None,
            tertiary: None,
            tertiary_label: None,
            extra_rate_windows: Vec::new(),
            cost: None,
            plan_name: None,
            account_email: None,
            source_label: "test".to_string(),
            updated_at: "2026-08-16T00:00:00Z".to_string(),
            error: None,
            pace: None,
            account_organization: None,
            tray_status_label: None,
            fetch_duration_ms: None,
            wayfinder_usage: None,
            session_equivalent_forecast: None,
        }
    }

    #[test]
    fn weekly_preference_selects_the_weekly_window() {
        let snapshot = snapshot();
        let mut settings = Settings::default();
        settings.set_provider_metric(ProviderId::Codex, MetricPreference::Weekly);

        assert_eq!(
            selected_usage_window(&snapshot, &settings).used_percent,
            60.0
        );
    }

    #[test]
    fn missing_selected_session_falls_back_to_a_real_window() {
        let mut snapshot = snapshot();
        snapshot.primary.is_informational = true;
        snapshot.primary.used_percent = 0.0;
        let mut settings = Settings::default();
        settings.set_provider_metric(ProviderId::Codex, MetricPreference::Session);

        assert_eq!(
            selected_usage_window(&snapshot, &settings).used_percent,
            60.0
        );
    }

    #[test]
    fn automatic_selects_the_highest_real_window() {
        let snapshot = snapshot();

        assert_eq!(
            selected_usage_window(&snapshot, &Settings::default()).used_percent,
            60.0
        );
    }

    #[test]
    fn average_preference_derives_the_combined_percentage() {
        let mut snapshot = snapshot();
        snapshot.provider_id = "gemini".to_string();
        let mut settings = Settings::default();
        settings.set_provider_metric(ProviderId::Gemini, MetricPreference::Average);

        let selected = selected_usage_window(&snapshot, &settings);
        assert_eq!(selected.used_percent, 40.0);
        assert_eq!(selected.remaining_percent, 60.0);
    }

    #[test]
    fn presentation_payload_flattens_the_snapshot_and_selected_metric() {
        let presentation = crate::commands::ProviderUsagePresentationSnapshot::new(
            snapshot(),
            &Settings::default(),
        );
        let value = serde_json::to_value(presentation).expect("serialize presentation");

        assert_eq!(value["providerId"], "codex");
        assert_eq!(value["selectedMetric"]["usedPercent"], 60.0);
        assert!(value.get("snapshot").is_none());
    }
}
