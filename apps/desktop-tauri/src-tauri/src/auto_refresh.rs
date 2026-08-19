use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use codexbar::core::{
    AdaptiveRefreshInput, AdaptiveRefreshReason, ThermalPressure, next_delay as adaptive_next_delay,
};
use codexbar::settings::Settings;

const AUTO_REFRESH_POLL_INTERVAL: Duration = Duration::from_secs(15);

static LAST_MENU_OPEN: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
static LAST_CODING_ACTIVITY: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
static ADAPTIVE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Record that the tray/flyout panel just opened (recent-interaction signal).
pub fn note_menu_open() {
    *LAST_MENU_OPEN
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
}

/// Weak coding-activity signal (e.g. a refresh cycle that found live usage).
#[allow(dead_code)]
pub fn note_coding_activity() {
    *LAST_CODING_ACTIVITY
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
}

fn age_since(slot: &OnceLock<Mutex<Option<Instant>>>) -> Option<Duration> {
    let guard = slot
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    guard.map(|at| at.elapsed())
}

pub fn install(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut schedule: Option<(Duration, Instant, bool)> = None;
        loop {
            let settings = Settings::load();
            ADAPTIVE_ACTIVE.store(settings.adaptive_refresh, Ordering::Relaxed);
            let interval = resolve_refresh_interval(&settings);
            match interval {
                None => schedule = None,
                Some(interval) => {
                    let now = Instant::now();
                    let adaptive = settings.adaptive_refresh;
                    let scheduled_at = schedule
                        .filter(|(scheduled_interval, _, was_adaptive)| {
                            *scheduled_interval == interval && *was_adaptive == adaptive
                        })
                        .map(|(_, scheduled_at, _)| scheduled_at)
                        .unwrap_or(now);
                    if now >= scheduled_at {
                        let _ = crate::commands::do_refresh_providers_if_stale(&app).await;
                        let next_interval =
                            resolve_refresh_interval(&Settings::load()).unwrap_or(interval);
                        schedule = Some((
                            next_interval,
                            next_fixed_tick(scheduled_at, Instant::now(), next_interval),
                            adaptive,
                        ));
                    }
                }
            }
            // Sample coding-agent processes on each poll while Adaptive is on
            // so delays can drop to the 5m coding-activity cap without waiting
            // for a refresh tick.
            if ADAPTIVE_ACTIVE.load(Ordering::Relaxed)
                && crate::coding_activity::coding_agent_process_active()
            {
                note_coding_activity();
            }
            tokio::time::sleep(AUTO_REFRESH_POLL_INTERVAL).await;
        }
    });
}

const LOW_POWER_MIN_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// Pure upstream `BackgroundWorkPowerPolicy.automaticInterval` port:
/// floor automatic intervals to 30 minutes when low-power mode is on.
/// `None` (manual / no timer) stays `None`.
pub(crate) fn automatic_interval(
    requested: Option<Duration>,
    low_power_mode_enabled: bool,
) -> Option<Duration> {
    let requested = requested?;
    if !low_power_mode_enabled {
        return Some(requested);
    }
    Some(requested.max(LOW_POWER_MIN_INTERVAL))
}

fn resolve_refresh_interval(settings: &Settings) -> Option<Duration> {
    let effective_low_power = settings
        .low_power_mode_preference
        .resolve(system_battery_saver_enabled());
    let requested = if settings.adaptive_refresh {
        Some(adaptive_delay_now(effective_low_power))
    } else {
        refresh_interval(settings.refresh_interval_secs)
    };
    automatic_interval(requested, effective_low_power)
}

fn adaptive_delay_now(low_power_mode_enabled: bool) -> Duration {
    let decision = adaptive_next_delay(AdaptiveRefreshInput {
        last_menu_open_age: age_since(&LAST_MENU_OPEN),
        last_coding_activity_age: age_since(&LAST_CODING_ACTIVITY),
        low_power_mode_enabled,
        thermal_pressure: ThermalPressure::Nominal,
    });
    tracing::debug!(
        delay_secs = decision.delay.as_secs(),
        reason = ?decision.reason,
        "adaptive refresh delay"
    );
    let _ = AdaptiveRefreshReason::LongIdle;
    decision.delay
}

/// Best-effort Windows Battery Saver check. Returns false when unknown.
fn system_battery_saver_enabled() -> bool {
    #[cfg(windows)]
    {
        // SYSTEM_POWER_STATUS via kernel32. SystemStatusFlag bit 0x01 is
        // the Windows 10+ Battery Saver state. Automatic follows that explicit
        // system preference only; battery percentage alone is not equivalent.
        #[repr(C)]
        struct SystemPowerStatus {
            ac_line_status: u8,
            battery_flag: u8,
            battery_life_percent: u8,
            system_status_flag: u8,
            battery_life_time: u32,
            battery_full_life_time: u32,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> i32;
        }
        let mut status = SystemPowerStatus {
            ac_line_status: 255,
            battery_flag: 255,
            battery_life_percent: 255,
            system_status_flag: 0,
            battery_life_time: 0,
            battery_full_life_time: 0,
        };
        let ok = unsafe { GetSystemPowerStatus(&mut status) } != 0;
        if !ok {
            return false;
        }
        status.system_status_flag & 0x01 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn next_fixed_tick(
    previous_scheduled_at: Instant,
    completed_at: Instant,
    interval: Duration,
) -> Instant {
    let mut scheduled_at = previous_scheduled_at + interval;
    while scheduled_at <= completed_at {
        scheduled_at += interval;
    }
    scheduled_at
}

fn powertoys_local_usage_provider_ids(settings: &Settings) -> Vec<String> {
    if !settings.powertoys_status_pipe_enabled {
        return Vec::new();
    }

    settings
        .get_enabled_provider_ids()
        .into_iter()
        .map(|provider| provider.cli_name().to_string())
        .filter(|provider_id| matches!(provider_id.as_str(), "codex" | "claude"))
        .collect()
}

pub(crate) fn schedule_refresh_enrichment(settings: &Settings) {
    let provider_ids = powertoys_local_usage_provider_ids(settings);
    if provider_ids.is_empty() {
        return;
    }
    static ENRICHMENT: OnceLock<Arc<tokio::sync::Mutex<()>>> = OnceLock::new();
    let Ok(guard) = Arc::clone(ENRICHMENT.get_or_init(|| Arc::new(tokio::sync::Mutex::new(()))))
        .try_lock_owned()
    else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        let _guard = guard;
        crate::commands::refresh_provider_local_usage_cache(provider_ids).await;
    });
}

fn refresh_interval(seconds: u64) -> Option<Duration> {
    (seconds > 0).then(|| Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes tests that mutate the shared `LAST_MENU_OPEN` /
    /// `LAST_CODING_ACTIVITY` globals so parallel `#[test]` threads can't
    /// interpose a write between one test's clear and its read.
    static ADAPTIVE_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn adaptive_enabled_uses_policy_delay() {
        let _guard = ADAPTIVE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Clear shared activity slots so parallel/prior tests cannot shrink the delay.
        *LAST_MENU_OPEN
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        *LAST_CODING_ACTIVITY
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;

        let settings = Settings {
            adaptive_refresh: true,
            refresh_interval_secs: 0,
            ..Default::default()
        };
        let delay = resolve_refresh_interval(&settings).expect("adaptive always schedules");
        // No menu open → long idle 30m
        assert_eq!(delay, Duration::from_secs(30 * 60));
    }

    #[test]
    fn low_power_mode_floors_fixed_and_adaptive_intervals() {
        assert_eq!(
            automatic_interval(Some(Duration::from_secs(60)), true),
            Some(Duration::from_secs(30 * 60))
        );
        assert_eq!(
            automatic_interval(Some(Duration::from_secs(3600)), true),
            Some(Duration::from_secs(3600))
        );
        assert_eq!(
            automatic_interval(Some(Duration::from_secs(60)), false),
            Some(Duration::from_secs(60))
        );
        assert_eq!(automatic_interval(None, true), None);

        let settings = Settings {
            low_power_mode_preference: codexbar::settings::LowPowerModePreference::On,
            adaptive_refresh: false,
            refresh_interval_secs: 300,
            ..Default::default()
        };
        assert_eq!(
            resolve_refresh_interval(&settings),
            Some(Duration::from_secs(30 * 60))
        );
    }

    #[test]
    fn fixed_cadence_advances_from_the_scheduled_tick() {
        let start = Instant::now();
        let interval = Duration::from_secs(100);
        let first_tick = start + interval;

        assert_eq!(
            next_fixed_tick(first_tick, first_tick + Duration::from_secs(60), interval),
            start + Duration::from_secs(200)
        );
        assert_eq!(
            next_fixed_tick(first_tick, first_tick + Duration::from_secs(260), interval),
            start + Duration::from_secs(400)
        );
    }

    #[test]
    fn powertoys_local_usage_refresh_only_includes_supported_enabled_providers() {
        let mut settings = Settings::default();
        assert!(powertoys_local_usage_provider_ids(&settings).is_empty());

        settings.powertoys_status_pipe_enabled = true;
        settings.enabled_providers = ["codex".to_string(), "cursor".to_string()]
            .into_iter()
            .collect();

        assert_eq!(
            powertoys_local_usage_provider_ids(&settings),
            vec!["codex".to_string()]
        );
    }

    #[test]
    fn note_menu_open_sets_recent_age() {
        let _guard = ADAPTIVE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        note_menu_open();
        let age = age_since(&LAST_MENU_OPEN).expect("menu open recorded");
        assert!(age < Duration::from_secs(5));
    }
}
