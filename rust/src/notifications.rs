//! System notifications for CodexBar
//!
//! Provides Windows toast notifications for usage alerts

#![allow(dead_code)]

use crate::core::ProviderId;
use crate::core::{RateWindow, UsagePace};
use crate::locale::{self, LocaleKey};
use crate::settings::Settings;
use crate::sound::{NotificationSoundEvent, play_alert};
use chrono::{DateTime, Utc};

/// Notification types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationType {
    /// Usage is approaching limit (high threshold)
    HighUsage,
    /// Usage is critical (critical threshold)
    CriticalUsage,
    /// Usage limit exhausted
    Exhausted,
    /// Provider status issue
    StatusIssue,
    /// Session quota depleted (at 100% usage)
    SessionDepleted,
    /// Session quota restored (back from 100%)
    SessionRestored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PredictiveWarningWindow {
    Session,
    Weekly,
}

impl PredictiveWarningWindow {
    fn localized_label(self, language: crate::settings::Language) -> String {
        locale::get_text(
            language,
            match self {
                Self::Session => LocaleKey::ProviderSession,
                Self::Weekly => LocaleKey::ProviderWeekly,
            },
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PredictiveResetWindow {
    window_minutes: Option<u32>,
    resets_at: DateTime<Utc>,
}

impl PredictiveResetWindow {
    fn belongs_to_same_cycle(&self, other: &Self) -> bool {
        if self.window_minutes != other.window_minutes {
            return false;
        }
        let tolerance_secs = self
            .window_minutes
            .map(|minutes| i64::from(minutes) * 30)
            .unwrap_or(300)
            .max(300);
        (self.resets_at - other.resets_at).num_seconds().abs() < tolerance_secs
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PredictiveWarningKey {
    provider: ProviderId,
    identity: String,
    window: PredictiveWarningWindow,
    reset: PredictiveResetWindow,
}

impl NotificationType {
    pub fn title(&self) -> &'static str {
        match self {
            NotificationType::HighUsage => "High Usage Warning",
            NotificationType::CriticalUsage => "Critical Usage Alert",
            NotificationType::Exhausted => "Usage Limit Reached",
            NotificationType::StatusIssue => "Provider Status Issue",
            NotificationType::SessionDepleted => "Session Depleted",
            NotificationType::SessionRestored => "Session Restored",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            NotificationType::HighUsage => "⚠️",
            NotificationType::CriticalUsage => "🔴",
            NotificationType::Exhausted => "🚫",
            NotificationType::StatusIssue => "⚡",
            NotificationType::SessionDepleted => "🔴",
            NotificationType::SessionRestored => "✅",
        }
    }

    fn is_threshold_toast(self) -> bool {
        matches!(
            self,
            NotificationType::HighUsage
                | NotificationType::CriticalUsage
                | NotificationType::Exhausted
        )
    }
}

/// Dedupe identity for threshold toasts.
/// - `account`: stable per-account discriminator (email, token-account id, …).
///   Empty string is the legacy single-account lane.
/// - `window`: rate window id (`"session"`, `"weekly"`, …) so budgets arm independently.
type ThresholdKey = (
    ProviderId,
    String, /* account */
    String, /* window */
    NotificationType,
);

/// Session-transition tracking key: provider + account identity.
type SessionTransitionKey = (ProviderId, String /* account */);

/// Notification manager
pub struct NotificationManager {
    /// Track which notifications have been sent to avoid spam
    sent_notifications: std::collections::HashSet<ThresholdKey>,
    /// Track previous session percent for depleted/restored transitions (per account)
    previous_session_percent: std::collections::HashMap<SessionTransitionKey, f64>,
    predictive_warning_keys: std::collections::HashSet<PredictiveWarningKey>,
    deepseek_pricing_period: Option<String>,
}

impl NotificationManager {
    pub fn new() -> Self {
        Self {
            sent_notifications: std::collections::HashSet::new(),
            previous_session_percent: std::collections::HashMap::new(),
            predictive_warning_keys: std::collections::HashSet::new(),
            deepseek_pricing_period: None,
        }
    }

    /// Observe a DeepSeek pricing period and notify once per observed transition.
    /// Intentionally silent; this advisory must not play a notification sound.
    pub fn notify_pricing_transition(&mut self, period: &str, settings: &Settings) {
        let changed = self
            .deepseek_pricing_period
            .as_deref()
            .is_some_and(|previous| previous != period);
        self.deepseek_pricing_period = Some(period.to_string());
        if !settings.show_notifications || !changed {
            return;
        }
        let label = match period {
            "peak" => "peak",
            "offPeak" => "off-peak",
            _ => "standard/pre-schedule",
        };
        self.show_toast(
            "DeepSeek pricing schedule",
            &format!("DeepSeek is currently in {label} hours."),
        );
    }

    pub fn record_predictive_observation(
        &mut self,
        enabled: bool,
        provider: ProviderId,
        identity: &str,
        window: PredictiveWarningWindow,
        rate_window: &RateWindow,
        pace: &UsagePace,
    ) -> bool {
        if !enabled {
            self.predictive_warning_keys
                .retain(|key| key.provider != provider);
            return false;
        }
        if !matches!(provider, ProviderId::Claude | ProviderId::Codex) || identity.is_empty() {
            return false;
        }
        let Some(resets_at) = rate_window.resets_at else {
            return false;
        };
        let key = PredictiveWarningKey {
            provider,
            identity: identity.to_string(),
            window,
            reset: PredictiveResetWindow {
                window_minutes: rate_window.window_minutes,
                resets_at,
            },
        };

        let warned_this_cycle = self.predictive_warning_keys.iter().any(|existing| {
            existing.provider == key.provider
                && existing.identity == key.identity
                && existing.window == key.window
                && existing.reset.belongs_to_same_cycle(&key.reset)
        });
        self.predictive_warning_keys.retain(|existing| {
            existing.provider != key.provider
                || existing.identity != key.identity
                || existing.window != key.window
        });

        if pace.will_last_to_reset {
            return false;
        }
        if !pace
            .eta_seconds
            .is_some_and(|eta| eta.is_finite() && eta > 0.0)
        {
            return false;
        }

        self.predictive_warning_keys.insert(key);
        !warned_this_cycle
    }

    pub fn set_predictive_warnings_enabled(&mut self, provider: ProviderId, enabled: bool) {
        if !enabled {
            self.predictive_warning_keys
                .retain(|key| key.provider != provider);
        }
    }

    pub fn check_predictive_pace(
        &mut self,
        provider: ProviderId,
        identity: &str,
        window: PredictiveWarningWindow,
        rate_window: &RateWindow,
        pace: &UsagePace,
        settings: &Settings,
    ) {
        if !self.record_predictive_observation(
            settings.show_notifications && settings.predictive_pace_warning_enabled,
            provider,
            identity,
            window,
            rate_window,
            pace,
        ) {
            return;
        }

        let eta = format_duration(pace.eta_seconds.unwrap_or_default());
        let provider_name = provider.display_name();
        let window_label = window.localized_label(settings.ui_language);
        let title = locale::format_locale(
            settings.ui_language,
            LocaleKey::PredictivePaceWarningTitle,
            &[provider_name, &window_label],
        );
        let body = locale::format_locale(
            settings.ui_language,
            LocaleKey::PredictivePaceWarningBody,
            &[&eta],
        );
        self.show_toast(&title, &body);
        Self::play_notification_sound(NotificationSoundEvent::PredictiveWarning, settings);
    }

    /// Check usage and send notifications if thresholds are crossed.
    ///
    /// `account` is a stable account discriminator (email, token-account id, …).
    /// Pass `""` for single-account providers when no identity is available.
    pub fn check_and_notify(
        &mut self,
        provider: ProviderId,
        account: &str,
        window: &str,
        used_percent: f64,
        settings: &Settings,
    ) {
        if !settings.show_notifications {
            return;
        }

        let thresholds = settings.usage_thresholds(provider, window);
        let notification_type = if used_percent >= 100.0 {
            Some(NotificationType::Exhausted)
        } else if used_percent >= thresholds.critical {
            Some(NotificationType::CriticalUsage)
        } else if used_percent >= thresholds.high {
            Some(NotificationType::HighUsage)
        } else {
            // Clear only this provider+account+window's threshold toasts so a cool
            // session on one account cannot re-arm another account's weekly (or
            // another window on the same account) on the next poll.
            self.sent_notifications.retain(|(p, a, w, t)| {
                *p != provider || a != account || w != window || !t.is_threshold_toast()
            });
            None
        };

        if let Some(notif_type) = notification_type {
            let key = (
                provider,
                account.to_string(),
                window.to_string(),
                notif_type,
            );
            if !self.sent_notifications.contains(&key) {
                self.send_notification(provider, window, used_percent, notif_type, settings);
                self.sent_notifications.insert(key);
            }
        }
    }

    /// Send a notification for a status issue
    pub fn notify_status_issue(
        &mut self,
        provider: ProviderId,
        description: &str,
        settings: &Settings,
    ) {
        let key = (
            provider,
            String::new(),
            String::new(),
            NotificationType::StatusIssue,
        );
        if !self.sent_notifications.contains(&key) {
            self.send_status_notification(provider, description, settings);
            self.sent_notifications.insert(key);
        }
    }

    /// Clear status issue notification (when resolved)
    pub fn clear_status_issue(&mut self, provider: ProviderId) {
        self.sent_notifications.remove(&(
            provider,
            String::new(),
            String::new(),
            NotificationType::StatusIssue,
        ));
    }

    /// Check session quota transitions (depleted/restored)
    /// Call this with each usage update to detect transitions.
    ///
    /// `account` scopes depleted/restored state so multi-account providers do not
    /// cross-arm session transitions.
    pub fn check_session_transition(
        &mut self,
        provider: ProviderId,
        account: &str,
        current_percent: f64,
        settings: &Settings,
    ) {
        if !settings.show_notifications {
            return;
        }

        const DEPLETED_THRESHOLD: f64 = 99.99; // Consider depleted at 99.99%+

        let transition_key: SessionTransitionKey = (provider, account.to_string());
        let previous_percent = self
            .previous_session_percent
            .get(&transition_key)
            .copied()
            .unwrap_or(0.0);

        // Check for depleted transition: was not depleted, now is
        if previous_percent < DEPLETED_THRESHOLD && current_percent >= DEPLETED_THRESHOLD {
            let title = NotificationType::SessionDepleted.title();
            let body = format!(
                "{} session depleted. 0% left. Will notify when available again.",
                provider.display_name()
            );
            self.show_toast(title, &body);
            Self::play_notification_sound(NotificationSoundEvent::SessionDepleted, settings);
            self.sent_notifications.insert((
                provider,
                account.to_string(),
                "session".to_string(),
                NotificationType::SessionDepleted,
            ));
        }
        // Check for restored transition: was depleted, now is not
        else if previous_percent >= DEPLETED_THRESHOLD && current_percent < DEPLETED_THRESHOLD {
            // Only notify restored if we previously sent a depleted notification
            let depleted_key = (
                provider,
                account.to_string(),
                "session".to_string(),
                NotificationType::SessionDepleted,
            );
            if self.sent_notifications.contains(&depleted_key) {
                let title = NotificationType::SessionRestored.title();
                let body = format!(
                    "{} session restored. Session quota is available again.",
                    provider.display_name()
                );
                self.show_toast(title, &body);
                Self::play_notification_sound(NotificationSoundEvent::SessionRestored, settings);
                self.sent_notifications.remove(&depleted_key);
            }
        }

        // Update the tracked previous percent for this account
        self.previous_session_percent
            .insert(transition_key, current_percent);
    }

    /// Send a Windows toast notification with sound
    fn send_notification(
        &self,
        provider: ProviderId,
        window: &str,
        used_percent: f64,
        notif_type: NotificationType,
        settings: &Settings,
    ) {
        let title = notif_type.title();
        let body = Self::notification_body(provider, window, used_percent, notif_type);
        self.show_toast(title, &body);
        Self::play_notification_sound(Self::sound_event_for(notif_type), settings);
    }

    fn window_label(window: &str) -> &str {
        match window {
            "session" => "session",
            "weekly" => "weekly",
            other if !other.is_empty() => other,
            _ => "usage",
        }
    }

    fn notification_body(
        provider: ProviderId,
        window: &str,
        used_percent: f64,
        notif_type: NotificationType,
    ) -> String {
        let provider_name = provider.display_name();
        let window_label = Self::window_label(window);
        match notif_type {
            NotificationType::HighUsage => {
                format!(
                    "{provider_name} {window_label} usage at {used_percent:.0}% - approaching limit"
                )
            }
            NotificationType::CriticalUsage => {
                format!(
                    "{provider_name} {window_label} usage at {used_percent:.0}% - critically high!"
                )
            }
            NotificationType::Exhausted => {
                format!("{provider_name} {window_label} usage limit exhausted ({used_percent:.0}%)")
            }
            NotificationType::StatusIssue => format!("{provider_name} is experiencing issues"),
            NotificationType::SessionDepleted => {
                format!("{provider_name} session depleted. 0% left.")
            }
            NotificationType::SessionRestored => {
                format!("{provider_name} session restored. Quota available again.")
            }
        }
    }

    fn sound_event_for(notif_type: NotificationType) -> NotificationSoundEvent {
        match notif_type {
            NotificationType::HighUsage => NotificationSoundEvent::HighUsage,
            NotificationType::CriticalUsage => NotificationSoundEvent::CriticalUsage,
            NotificationType::Exhausted => NotificationSoundEvent::Exhausted,
            NotificationType::StatusIssue => NotificationSoundEvent::StatusIssue,
            NotificationType::SessionDepleted => NotificationSoundEvent::SessionDepleted,
            NotificationType::SessionRestored => NotificationSoundEvent::SessionRestored,
        }
    }

    fn play_notification_sound(event: NotificationSoundEvent, settings: &Settings) {
        if let Err(error) = play_alert(event, settings) {
            tracing::warn!(?event, %error, "notification sound failed to play");
        }
    }

    fn send_status_notification(
        &self,
        provider: ProviderId,
        description: &str,
        settings: &Settings,
    ) {
        let title = NotificationType::StatusIssue.title();
        let body = format!("{}: {}", provider.display_name(), description);
        self.show_toast(title, &body);
        Self::play_notification_sound(NotificationSoundEvent::StatusIssue, settings);
    }

    #[cfg(target_os = "windows")]
    fn show_toast(&self, title: &str, body: &str) {
        use std::os::windows::process::CommandExt;
        use std::process::Command;
        use std::sync::Once;

        // Register our AUMID (App User Model ID) exactly once per process so that
        // CreateToastNotifier("CodexBar") finds a valid registration rather than
        // silently returning a null notifier.
        static AUMID_INIT: Once = Once::new();
        AUMID_INIT.call_once(ensure_aumid_registered);

        // Escape for XML content to prevent injection
        fn xml_escape(s: &str) -> String {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
                .replace('\'', "&apos;")
        }

        let safe_title = xml_escape(title);
        let safe_body = xml_escape(body);

        // Uses ToastGeneric (Win 10+) and wraps in try/catch so PowerShell exits
        // with code 1 on failure rather than swallowing the error silently.
        // Single-quoted here-string (@'...'@) prevents variable expansion of the
        // XML content by PowerShell.
        let script = format!(
            r#"try {{
    [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
    [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null
    $template = @'
<toast><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual><audio silent="true"/></toast>
'@
    $xml = New-Object Windows.Data.Xml.Dom.XmlDocument
    $xml.LoadXml($template)
    $toast = [Windows.UI.Notifications.ToastNotification]::new($xml)
    $notifier = [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier("CodexBar")
    if ($null -eq $notifier) {{ throw "CreateToastNotifier returned null" }}
    $notifier.Show($toast)
}} catch {{
    [System.Console]::Error.WriteLine("CodexBar toast failed: $_")
    exit 1
}}"#,
            safe_title, safe_body
        );

        match Command::new("powershell")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &script,
            ])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .spawn()
        {
            Ok(_) => tracing::debug!("Toast notification dispatched: {}", title),
            Err(e) => tracing::warn!("Failed to dispatch toast notification '{}': {}", title, e),
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn show_toast(&self, title: &str, body: &str) {
        use std::process::Command;

        // Try notify-send first (works on most Linux distros including WSL with WSLg)
        if let Ok(output) = Command::new("notify-send")
            .args([
                "--app-name=CodexBar",
                "--icon=dialog-information",
                title,
                body,
            ])
            .output()
            && output.status.success()
        {
            tracing::debug!("Sent notification via notify-send: {}", title);
            return;
        }

        tracing::info!("Notification: {} - {}", title, body);
    }
}

fn format_duration(seconds: f64) -> String {
    let total_minutes = (seconds / 60.0).ceil().max(1.0) as i64;
    let days = total_minutes / 1440;
    let hours = (total_minutes % 1440) / 60;
    let minutes = total_minutes % 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

impl Default for NotificationManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Register the CodexBar App User Model ID (AUMID) in the Windows registry so that
/// `CreateToastNotifier("CodexBar")` resolves to a valid notifier instead of returning
/// null.  Must be called at least once before the first toast.  Safe to call multiple
/// times (idempotent registry write).
#[cfg(target_os = "windows")]
fn ensure_aumid_registered() {
    use winreg::RegKey;
    use winreg::enums::*;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    // HKCU\SOFTWARE\Classes\AppUserModelId\<AUMID> is the documented path for
    // registering Win32 desktop app AUMIDs without a COM server or Start Menu shortcut.
    let result = hkcu
        .create_subkey(r"SOFTWARE\Classes\AppUserModelId\CodexBar")
        .and_then(|(key, _)| key.set_value("DisplayName", &"CodexBar"));

    match result {
        Ok(()) => tracing::debug!("CodexBar AUMID registered for Windows toast notifications"),
        Err(e) => tracing::warn!("Failed to register CodexBar AUMID: {}", e),
    }
}

/// Simple notification function for one-off notifications
pub fn show_notification(title: &str, body: &str) {
    let manager = NotificationManager::new();
    manager.show_toast(title, body);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{PaceStage, RateWindow, UsagePace};
    use chrono::{DateTime, Duration, Utc};

    #[test]
    fn notification_types_map_to_their_sound_events() {
        let mappings = [
            (
                NotificationType::HighUsage,
                NotificationSoundEvent::HighUsage,
            ),
            (
                NotificationType::CriticalUsage,
                NotificationSoundEvent::CriticalUsage,
            ),
            (
                NotificationType::Exhausted,
                NotificationSoundEvent::Exhausted,
            ),
            (
                NotificationType::StatusIssue,
                NotificationSoundEvent::StatusIssue,
            ),
            (
                NotificationType::SessionDepleted,
                NotificationSoundEvent::SessionDepleted,
            ),
            (
                NotificationType::SessionRestored,
                NotificationSoundEvent::SessionRestored,
            ),
        ];

        for (notification_type, sound_event) in mappings {
            assert_eq!(
                NotificationManager::sound_event_for(notification_type),
                sound_event
            );
        }
    }

    fn pace(will_last_to_reset: bool, eta_seconds: Option<f64>) -> UsagePace {
        UsagePace {
            stage: PaceStage::Ahead,
            delta_percent: 20.0,
            expected_used_percent: 40.0,
            actual_used_percent: 60.0,
            eta_seconds,
            will_last_to_reset,
        }
    }

    fn window(now: DateTime<Utc>, offset: Duration, minutes: u32) -> RateWindow {
        RateWindow::with_details(60.0, Some(minutes), Some(now + offset), None)
    }

    #[test]
    fn predictive_warning_notifies_once_until_recovery_then_rearms() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let window = window(now, Duration::hours(3), 300);
        let risk = pace(false, Some(3600.0));
        let recovery = pace(true, None);
        let mut manager = NotificationManager::new();

        assert!(manager.record_predictive_observation(
            true,
            ProviderId::Claude,
            "oauth:person@example.com",
            PredictiveWarningWindow::Session,
            &window,
            &risk,
        ));
        assert!(!manager.record_predictive_observation(
            true,
            ProviderId::Claude,
            "oauth:person@example.com",
            PredictiveWarningWindow::Session,
            &window,
            &risk,
        ));
        assert!(!manager.record_predictive_observation(
            true,
            ProviderId::Claude,
            "oauth:person@example.com",
            PredictiveWarningWindow::Session,
            &window,
            &recovery,
        ));
        assert!(manager.record_predictive_observation(
            true,
            ProviderId::Claude,
            "oauth:person@example.com",
            PredictiveWarningWindow::Session,
            &window,
            &risk,
        ));
    }

    #[test]
    fn predictive_warning_reset_jitter_does_not_retrigger() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let mut manager = NotificationManager::new();
        let risk = pace(false, Some(3600.0));

        assert!(manager.record_predictive_observation(
            true,
            ProviderId::Codex,
            "oauth:account-a",
            PredictiveWarningWindow::Weekly,
            &window(now, Duration::days(3), 10080),
            &risk,
        ));
        assert!(!manager.record_predictive_observation(
            true,
            ProviderId::Codex,
            "oauth:account-a",
            PredictiveWarningWindow::Weekly,
            &window(now, Duration::days(3) + Duration::minutes(5), 10080),
            &risk,
        ));
    }

    #[test]
    fn predictive_warning_isolates_provider_identity_source_and_window() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let reset = window(now, Duration::hours(3), 300);
        let risk = pace(false, Some(3600.0));
        let mut manager = NotificationManager::new();

        for (provider, identity, warning_window) in [
            (
                ProviderId::Claude,
                "cli:person@example.com",
                PredictiveWarningWindow::Session,
            ),
            (
                ProviderId::Claude,
                "oauth:person@example.com",
                PredictiveWarningWindow::Session,
            ),
            (
                ProviderId::Claude,
                "token-account:1",
                PredictiveWarningWindow::Session,
            ),
            (
                ProviderId::Claude,
                "oauth:person@example.com",
                PredictiveWarningWindow::Weekly,
            ),
            (
                ProviderId::Codex,
                "oauth:person@example.com",
                PredictiveWarningWindow::Session,
            ),
        ] {
            assert!(manager.record_predictive_observation(
                true,
                provider,
                identity,
                warning_window,
                &reset,
                &risk,
            ));
        }
    }

    #[test]
    fn predictive_warning_requires_enabled_confident_positive_risk() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let reset = window(now, Duration::hours(3), 300);
        let mut manager = NotificationManager::new();

        for (enabled, observation) in [
            (false, pace(false, Some(3600.0))),
            (true, pace(true, None)),
            (true, pace(false, Some(0.0))),
        ] {
            assert!(!manager.record_predictive_observation(
                enabled,
                ProviderId::Claude,
                "oauth:person@example.com",
                PredictiveWarningWindow::Session,
                &reset,
                &observation,
            ));
        }

        assert!(manager.record_predictive_observation(
            true,
            ProviderId::Claude,
            "oauth:person@example.com",
            PredictiveWarningWindow::Session,
            &reset,
            &pace(false, Some(3600.0)),
        ));
    }

    #[test]
    fn session_below_high_does_not_rearm_weekly_high_toast() {
        // Repro for #198: session cool + weekly hot on every refresh used to
        // clear all provider keys on the session call, then re-fire weekly.
        let mut manager = NotificationManager::new();
        let settings = Settings::default();
        assert!(settings.show_notifications);
        assert!((settings.high_usage_threshold - 70.0).abs() < f64::EPSILON);

        let account = "";
        let weekly_key = (
            ProviderId::Claude,
            account.to_string(),
            "weekly".to_string(),
            NotificationType::HighUsage,
        );

        manager.check_and_notify(ProviderId::Claude, account, "session", 20.0, &settings);
        manager.check_and_notify(ProviderId::Claude, account, "weekly", 76.0, &settings);
        assert!(manager.sent_notifications.contains(&weekly_key));

        // Simulate several refresh cycles: session still cool, weekly still hot.
        for _ in 0..5 {
            manager.check_and_notify(ProviderId::Claude, account, "session", 20.0, &settings);
            manager.check_and_notify(ProviderId::Claude, account, "weekly", 76.0, &settings);
        }
        assert_eq!(
            manager
                .sent_notifications
                .iter()
                .filter(|key| key == &&weekly_key)
                .count(),
            1,
            "weekly high toast must arm only once while still above threshold"
        );

        // Drop weekly below high → re-arm allowed on next climb.
        manager.check_and_notify(ProviderId::Claude, account, "weekly", 50.0, &settings);
        assert!(!manager.sent_notifications.contains(&weekly_key));
        manager.check_and_notify(ProviderId::Claude, account, "weekly", 76.0, &settings);
        assert!(manager.sent_notifications.contains(&weekly_key));
    }

    #[test]
    fn threshold_keys_isolate_session_and_weekly() {
        let mut manager = NotificationManager::new();
        let settings = Settings::default();
        let account = "";

        manager.check_and_notify(ProviderId::Claude, account, "session", 75.0, &settings);
        manager.check_and_notify(ProviderId::Claude, account, "weekly", 75.0, &settings);

        assert!(manager.sent_notifications.contains(&(
            ProviderId::Claude,
            account.to_string(),
            "session".to_string(),
            NotificationType::HighUsage,
        )));
        assert!(manager.sent_notifications.contains(&(
            ProviderId::Claude,
            account.to_string(),
            "weekly".to_string(),
            NotificationType::HighUsage,
        )));

        // Cool only session; weekly stays armed.
        manager.check_and_notify(ProviderId::Claude, account, "session", 10.0, &settings);
        assert!(!manager.sent_notifications.contains(&(
            ProviderId::Claude,
            account.to_string(),
            "session".to_string(),
            NotificationType::HighUsage,
        )));
        assert!(manager.sent_notifications.contains(&(
            ProviderId::Claude,
            account.to_string(),
            "weekly".to_string(),
            NotificationType::HighUsage,
        )));
    }

    /// Mirrors `notify_usage_thresholds` in the Tauri shell: each refresh
    /// calls session then weekly. Confidence pass for #198 over many cycles.
    #[test]
    fn refresh_loop_session_cool_weekly_hot_toasts_once() {
        let mut manager = NotificationManager::new();
        let settings = Settings::default();
        let account = "";
        let weekly_high = (
            ProviderId::Claude,
            account.to_string(),
            "weekly".to_string(),
            NotificationType::HighUsage,
        );

        let mut weekly_fires = 0usize;
        for _ in 0..30 {
            let before = manager.sent_notifications.contains(&weekly_high);
            // Same order as apps/desktop-tauri/.../providers.rs
            manager.check_and_notify(ProviderId::Claude, account, "session", 20.0, &settings);
            manager.check_and_notify(ProviderId::Claude, account, "weekly", 76.0, &settings);
            let after = manager.sent_notifications.contains(&weekly_high);
            if after && !before {
                weekly_fires += 1;
            }
        }

        assert_eq!(
            weekly_fires, 1,
            "weekly high must fire exactly once across 30 refresh cycles"
        );
        assert!(manager.sent_notifications.contains(&weekly_high));
        // Session never armed high while cool.
        assert!(!manager.sent_notifications.contains(&(
            ProviderId::Claude,
            account.to_string(),
            "session".to_string(),
            NotificationType::HighUsage,
        )));
    }

    #[test]
    fn threshold_keys_isolate_accounts_on_same_provider() {
        // Two accounts on the same provider can each fire High once for weekly.
        let mut manager = NotificationManager::new();
        let settings = Settings::default();

        let key_a = (
            ProviderId::Claude,
            "account-a".to_string(),
            "weekly".to_string(),
            NotificationType::HighUsage,
        );
        let key_b = (
            ProviderId::Claude,
            "account-b".to_string(),
            "weekly".to_string(),
            NotificationType::HighUsage,
        );

        manager.check_and_notify(ProviderId::Claude, "account-a", "weekly", 80.0, &settings);
        manager.check_and_notify(ProviderId::Claude, "account-b", "weekly", 80.0, &settings);

        assert!(manager.sent_notifications.contains(&key_a));
        assert!(manager.sent_notifications.contains(&key_b));

        // Re-poll both still hot: neither re-fires (still armed, no second insert).
        manager.check_and_notify(ProviderId::Claude, "account-a", "weekly", 80.0, &settings);
        manager.check_and_notify(ProviderId::Claude, "account-b", "weekly", 80.0, &settings);
        assert_eq!(
            manager
                .sent_notifications
                .iter()
                .filter(|k| k.0 == ProviderId::Claude
                    && k.2 == "weekly"
                    && k.3 == NotificationType::HighUsage)
                .count(),
            2
        );
    }

    #[test]
    fn account_a_session_cool_does_not_clear_account_b_weekly() {
        let mut manager = NotificationManager::new();
        let settings = Settings::default();

        let key_b_weekly = (
            ProviderId::Claude,
            "account-b".to_string(),
            "weekly".to_string(),
            NotificationType::HighUsage,
        );

        manager.check_and_notify(ProviderId::Claude, "account-b", "weekly", 80.0, &settings);
        assert!(manager.sent_notifications.contains(&key_b_weekly));

        // Account A session cool must not clear account B weekly armed state.
        manager.check_and_notify(ProviderId::Claude, "account-a", "session", 10.0, &settings);
        assert!(
            manager.sent_notifications.contains(&key_b_weekly),
            "account A session cool must not clear account B weekly threshold key"
        );

        // Account A weekly cool must also not clear account B.
        manager.check_and_notify(ProviderId::Claude, "account-a", "weekly", 10.0, &settings);
        assert!(manager.sent_notifications.contains(&key_b_weekly));
    }

    #[test]
    fn session_transition_isolates_accounts() {
        let mut manager = NotificationManager::new();
        let settings = Settings::default();

        let depleted_a = (
            ProviderId::Claude,
            "account-a".to_string(),
            "session".to_string(),
            NotificationType::SessionDepleted,
        );
        let depleted_b = (
            ProviderId::Claude,
            "account-b".to_string(),
            "session".to_string(),
            NotificationType::SessionDepleted,
        );

        manager.check_session_transition(ProviderId::Claude, "account-a", 50.0, &settings);
        manager.check_session_transition(ProviderId::Claude, "account-a", 100.0, &settings);
        assert!(manager.sent_notifications.contains(&depleted_a));
        assert!(!manager.sent_notifications.contains(&depleted_b));

        // Account B still has quota; restoring A must not affect B's lane.
        manager.check_session_transition(ProviderId::Claude, "account-b", 40.0, &settings);
        manager.check_session_transition(ProviderId::Claude, "account-a", 20.0, &settings);
        assert!(!manager.sent_notifications.contains(&depleted_a));
        assert!(!manager.sent_notifications.contains(&depleted_b));

        // Account B can still fire depleted independently.
        manager.check_session_transition(ProviderId::Claude, "account-b", 100.0, &settings);
        assert!(manager.sent_notifications.contains(&depleted_b));
    }
}
