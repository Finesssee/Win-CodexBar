//! External hook runner for quota / provider events (upstream #2001).
//!
//! Loads rules from `hooks.json` next to `settings.json` and runs matching
//! binaries with a narrow env whitelist + JSON stdin payload.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Stable event names used in config, env vars, and the JSON payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEventType {
    QuotaLow,
    QuotaReached,
    QuotaReset,
    ProviderUnavailable,
    ProviderRecovered,
    RefreshFailed,
}

impl HookEventType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::QuotaLow => "quota_low",
            Self::QuotaReached => "quota_reached",
            Self::QuotaReset => "quota_reset",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::ProviderRecovered => "provider_recovered",
            Self::RefreshFailed => "refresh_failed",
        }
    }

    /// Events that can repeat every refresh while a condition persists.
    pub fn is_rate_limited(self) -> bool {
        matches!(self, Self::ProviderUnavailable | Self::RefreshFailed)
    }
}

/// Single quota/provider event handed to external hooks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HookEvent {
    pub event: HookEventType,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    /// Remaining capacity 0..=100 when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_percent: Option<f64>,
    /// Used fraction 0..=1 (upstream-compatible env/payload).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub timestamp: String,
}

impl HookEvent {
    pub fn new(event: HookEventType, provider: impl Into<String>) -> Self {
        Self {
            event,
            provider: provider.into(),
            account: None,
            window: None,
            remaining_percent: None,
            usage_percent: None,
            status: None,
            timestamp: utc_now_iso(),
        }
    }

    pub fn with_remaining_percent(mut self, remaining: f64) -> Self {
        let remaining = remaining.clamp(0.0, 100.0);
        self.remaining_percent = Some(remaining);
        self.usage_percent = Some(((100.0 - remaining) / 100.0).clamp(0.0, 1.0));
        self
    }

    pub fn with_used_percent(mut self, used: f64) -> Self {
        let used = used.clamp(0.0, 100.0);
        self.usage_percent = Some(used / 100.0);
        self.remaining_percent = Some(100.0 - used);
        self
    }

    pub fn with_window(mut self, window: impl Into<String>) -> Self {
        self.window = Some(window.into());
        self
    }

    pub fn with_account(mut self, account: impl Into<String>) -> Self {
        self.account = Some(account.into());
        self
    }

    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }
    pub fn with_usage_fraction(mut self, usage: f64) -> Self {
        let usage = usage.clamp(0.0, 1.0);
        self.usage_percent = Some(usage);
        self.remaining_percent = Some((1.0 - usage) * 100.0);
        self
    }

    pub fn with_timestamp(mut self, ts: chrono::DateTime<chrono::Utc>) -> Self {
        self.timestamp = format_unix_utc(ts.timestamp().max(0) as u64);
        self
    }

    /// `CODEXBAR_*` environment variables for the hook process.
    pub fn environment_variables(&self) -> HashMap<String, String> {
        let mut env = HashMap::new();
        env.insert("CODEXBAR_EVENT".into(), self.event.as_str().into());
        env.insert("CODEXBAR_PROVIDER".into(), self.provider.clone());
        env.insert("CODEXBAR_TIMESTAMP".into(), self.timestamp.clone());
        if let Some(account) = &self.account {
            env.insert("CODEXBAR_ACCOUNT".into(), account.clone());
        }
        if let Some(window) = &self.window {
            env.insert("CODEXBAR_WINDOW".into(), window.clone());
        }
        if let Some(usage) = self.usage_percent {
            env.insert("CODEXBAR_USAGE_PERCENT".into(), format_number(usage));
        }
        if let Some(remaining) = self.remaining_percent {
            env.insert(
                "CODEXBAR_REMAINING_PERCENT".into(),
                format_number(remaining),
            );
        }
        if let Some(status) = &self.status {
            env.insert("CODEXBAR_STATUS".into(), status.clone());
        }
        env
    }

    pub fn json_payload(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(self).map_err(|e| e.to_string())
    }
}

/// One configured hook rule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HookRule {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Single event (upstream shape). Prefer this for new configs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<HookEventType>,
    /// Optional multi-event list (task shape). Either `event` or `events` must match.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<HookEventType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// For quota_low: fire when usage fraction ≥ threshold (0..=1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    pub executable: PathBuf,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_true() -> bool {
    true
}

fn default_timeout_secs() -> u64 {
    10
}

impl HookRule {
    fn matched_events(&self) -> Vec<HookEventType> {
        let mut out = self.events.clone();
        if let Some(event) = self.event
            && !out.contains(&event)
        {
            out.push(event);
        }
        out
    }

    pub fn matches(&self, event: &HookEvent) -> bool {
        if !self.enabled {
            return false;
        }
        if !self.matched_events().contains(&event.event) {
            return false;
        }
        if !self.executable.is_absolute() || self.executable.as_os_str().is_empty() {
            return false;
        }
        if self.timeout_secs == 0 || self.timeout_secs > 300 {
            return false;
        }
        if let Some(provider) = &self.provider
            && provider != &event.provider
        {
            return false;
        }
        if event.event == HookEventType::QuotaLow
            && let Some(threshold) = self.threshold
        {
            let usage = event.usage_percent.unwrap_or(0.0);
            if !(threshold.is_finite() && threshold > 0.0 && threshold <= 1.0) || usage < threshold
            {
                return false;
            }
        }
        true
    }
}

/// Top-level hooks config file contents.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct HooksConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub events: Vec<HookRule>,
}

impl HooksConfig {
    pub const MAX_RULES: usize = 32;
    pub const MAX_PAYLOAD_BYTES: usize = 4096;

    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("CodexBar").join("hooks.json"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        Self::load_from(&path)
    }

    pub fn load_from(path: &Path) -> Self {
        let Ok(content) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        serde_json::from_str(content.trim_start_matches('\u{feff}')).unwrap_or_default()
    }

    /// Persist config to the standard hooks.json path (creates parent dirs).
    pub fn save(&self) -> Result<PathBuf, String> {
        let path = Self::path().ok_or_else(|| "config directory unavailable".to_string())?;
        self.save_to(&path)?;
        Ok(path)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create hooks dir: {e}"))?;
        }
        let body = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, body).map_err(|e| format!("write hooks.json: {e}"))
    }

    pub fn matching_rules(&self, event: &HookEvent) -> Vec<&HookRule> {
        if !self.enabled || self.events.len() > Self::MAX_RULES {
            return Vec::new();
        }
        self.events
            .iter()
            .filter(|rule| rule.matches(event))
            .collect()
    }

    /// Rules that match the event ignoring the top-level `enabled` flag (for `hooks test`).
    pub fn matching_rules_ignoring_master_switch(&self, event: &HookEvent) -> Vec<&HookRule> {
        if self.events.len() > Self::MAX_RULES {
            return Vec::new();
        }
        self.events
            .iter()
            .filter(|rule| rule.matches(event))
            .collect()
    }
}

/// Env keys forwarded from the parent process (never secrets).
const FORWARDED_ENV_KEYS: &[&str] = &["PATH", "HOME", "USER", "TEMP", "TMP", "USERPROFILE"];

/// Builds the process environment for a hook.
pub fn build_hook_environment(
    base: &HashMap<String, String>,
    event: &HookEvent,
) -> HashMap<String, String> {
    let mut env = HashMap::new();
    for key in FORWARDED_ENV_KEYS {
        if let Some(value) = base.get(*key) {
            env.insert((*key).to_string(), value.clone());
        }
    }
    for (key, value) in event.environment_variables() {
        env.insert(key, value);
    }
    env
}

/// Fire-and-forget hook runner.
pub struct HookRunner;

impl HookRunner {
    /// Dispatch matching rules for `event`. Failures are logged, never returned.
    pub fn dispatch(event: &HookEvent, config: &HooksConfig, rate_limiter: &HookRateLimiter) {
        let rules = config.matching_rules(event);
        if rules.is_empty() {
            return;
        }
        if event.event.is_rate_limited() && !rate_limiter.allow(event) {
            tracing::debug!(
                event = event.event.as_str(),
                provider = %event.provider,
                "hook suppressed by rate limiter"
            );
            return;
        }
        let base_env: HashMap<String, String> = std::env::vars().collect();
        for rule in rules {
            if let Err(err) = Self::run(rule, event, &base_env) {
                tracing::warn!(
                    event = event.event.as_str(),
                    provider = %event.provider,
                    reason = %err,
                    "hook failed"
                );
            } else {
                tracing::info!(
                    event = event.event.as_str(),
                    provider = %event.provider,
                    "ran hook"
                );
            }
        }
    }

    /// Best-effort load + dispatch when settings allow hooks.
    pub fn dispatch_if_enabled(event: HookEvent, hooks_enabled: bool) {
        if !hooks_enabled {
            return;
        }
        let config = HooksConfig::load();
        if !config.enabled {
            return;
        }
        HOOK_RATE_LIMITER.with(|limiter| {
            Self::dispatch(&event, &config, limiter);
        });
    }

    pub fn run(
        rule: &HookRule,
        event: &HookEvent,
        base_env: &HashMap<String, String>,
    ) -> Result<(), String> {
        let payload = event.json_payload()?;
        if payload.len() > HooksConfig::MAX_PAYLOAD_BYTES {
            return Err("payload too large".into());
        }
        if !rule.executable.is_absolute() {
            return Err("executable must be absolute".into());
        }
        if !rule.executable.exists() {
            return Err("executable not found".into());
        }

        let env = build_hook_environment(base_env, event);
        let mut child = Command::new(&rule.executable)
            .args(&rule.arguments)
            .env_clear()
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("launch failed: {e}"))?;

        if let Some(mut stdin) = child.stdin.take() {
            // Fire-and-forget payload delivery: a hook that closed stdin early
            // yields a write error, which must not abort hook execution.
            let _delivered = stdin.write_all(&payload);
        }

        let timeout = Duration::from_secs(rule.timeout_secs.max(1));
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if status.success() {
                        return Ok(());
                    }
                    return Err(format!("exit {}", status.code().unwrap_or(-1)));
                }
                Ok(None) => {
                    if started.elapsed() >= timeout {
                        // Teardown after timeout: kill/wait results cannot change
                        // the outcome, which is already reported as timed out.
                        let _killed = child.kill();
                        let _reaped = child.wait();
                        return Err("timed out".into());
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(format!("wait failed: {e}")),
            }
        }
    }
}

/// In-process storm suppression for spammy events.
#[derive(Debug)]
pub struct HookRateLimiter {
    last_fired: Mutex<HashMap<String, Instant>>,
    window: Duration,
}

impl HookRateLimiter {
    pub const DEFAULT_WINDOW_SECS: u64 = 600;

    pub fn new(window: Duration) -> Self {
        Self {
            last_fired: Mutex::new(HashMap::new()),
            window,
        }
    }

    pub fn allow(&self, event: &HookEvent) -> bool {
        let key = rate_limit_key(event);
        let Ok(mut map) = self.last_fired.lock() else {
            return true;
        };
        let now = Instant::now();
        if let Some(previous) = map.get(&key)
            && now.duration_since(*previous) < self.window
        {
            return false;
        }
        map.insert(key, now);
        true
    }
}

impl Default for HookRateLimiter {
    fn default() -> Self {
        Self::new(Duration::from_secs(Self::DEFAULT_WINDOW_SECS))
    }
}

fn rate_limit_key(event: &HookEvent) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        event.event.as_str(),
        event.provider,
        event.account.as_deref().unwrap_or(""),
        event.window.as_deref().unwrap_or("")
    )
}

thread_local! {
    static HOOK_RATE_LIMITER: HookRateLimiter = HookRateLimiter::default();
}

fn utc_now_iso() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // RFC3339-ish UTC without chrono dependency in this module path.
    // Good enough for hooks; refresh path also has chrono elsewhere.
    format_unix_utc(secs)
}

fn format_unix_utc(secs: u64) -> String {
    // Manual UTC formatting: YYYY-MM-DDTHH:MM:SSZ
    const SECS_PER_DAY: u64 = 86_400;
    let days = secs / SECS_PER_DAY;
    let tod = secs % SECS_PER_DAY;
    let hour = tod / 3600;
    let min = (tod % 3600) / 60;
    let sec = tod % 60;
    #[allow(
        clippy::cast_possible_wrap,
        reason = "secs comes from duration since the UNIX epoch, so days fits i64 by a wide margin"
    )]
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Howard Hinnant civil_from_days (proleptic Gregorian).
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    #[allow(
        clippy::cast_possible_wrap,
        reason = "yoe < 400 by the era math and era * 400 stays far inside i64 for any representable day count"
    )]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    #[allow(
        clippy::cast_possible_truncation,
        reason = "Hinnant's algorithm bounds m to 1..=12 and d to 1..=31; y only exceeds i32 for day offsets billions of years from real clock time"
    )]
    let date = (y as i32, m as u32, d as u32);
    date
}

fn format_number(value: f64) -> String {
    if value.is_finite() && (value - value.round()).abs() < f64::EPSILON && value.abs() < 1e15 {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "the guard above ensures value is finite, integral and |value| < 1e15, so the cast is exact"
        )]
        let integral = value as i64;
        format!("{integral}")
    } else {
        format!("{value}")
    }
}

/// Tracks last used% per provider/account/window so hooks fire on crossings only.
static QUOTA_HOOK_BASELINES: Mutex<Option<HashMap<String, f64>>> = Mutex::new(None);

fn baseline_key(provider: &str, account: &str, window: &str) -> String {
    format!("{provider}\u{1f}{account}\u{1f}{window}")
}

/// Emit quota hooks when used% crosses high/critical/exhausted (best-effort).
///
/// First sample only establishes a baseline — no fire on launch while already high.
pub fn emit_quota_threshold_hooks(
    hooks_enabled: bool,
    provider_cli: &str,
    window: &str,
    used_percent: f64,
    high_threshold: f64,
    critical_threshold: f64,
    account: Option<&str>,
) {
    if !hooks_enabled {
        return;
    }
    let account = account.unwrap_or("").to_string();
    let key = baseline_key(provider_cli, &account, window);
    let used = used_percent.clamp(0.0, 100.0);

    let previous = {
        let Ok(mut guard) = QUOTA_HOOK_BASELINES.lock() else {
            return;
        };
        let map = guard.get_or_insert_with(HashMap::new);
        let prev = map.get(&key).copied();
        map.insert(key, used);
        prev
    };
    let Some(previous) = previous else {
        return;
    };

    let mut events: Vec<HookEventType> = Vec::new();
    // Exhausted first so a jump 60→100 fires reached (and low is still useful).
    if previous < 100.0 && used >= 100.0 {
        events.push(HookEventType::QuotaReached);
    }
    if used < 100.0
        && (previous < critical_threshold && used >= critical_threshold
            || previous < high_threshold && used >= high_threshold)
    {
        events.push(HookEventType::QuotaLow);
    }
    // Reset edge: was exhausted, now recovered.
    if previous >= 99.99 && used < 99.99 {
        events.push(HookEventType::QuotaReset);
    }
    if events.is_empty() {
        return;
    }

    let provider = provider_cli.to_string();
    let window = window.to_string();
    std::thread::Builder::new()
        .name("codexbar-hook".into())
        .spawn(move || {
            for event_type in events {
                let mut event =
                    HookEvent::new(event_type, provider.clone()).with_used_percent(used);
                if !window.is_empty() {
                    event = event.with_window(window.clone());
                }
                if !account.is_empty() {
                    event = event.with_account(account.clone());
                }
                HookRunner::dispatch_if_enabled(event, true);
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn payload_env_includes_event_and_remaining() {
        let event = HookEvent::new(HookEventType::QuotaLow, "claude")
            .with_used_percent(80.0)
            .with_window("session")
            .with_account("user@example.com");
        let env = event.environment_variables();
        assert_eq!(
            env.get("CODEXBAR_EVENT").map(String::as_str),
            Some("quota_low")
        );
        assert_eq!(
            env.get("CODEXBAR_PROVIDER").map(String::as_str),
            Some("claude")
        );
        assert_eq!(
            env.get("CODEXBAR_WINDOW").map(String::as_str),
            Some("session")
        );
        assert_eq!(
            env.get("CODEXBAR_ACCOUNT").map(String::as_str),
            Some("user@example.com")
        );
        assert_eq!(
            env.get("CODEXBAR_USAGE_PERCENT").map(String::as_str),
            Some("0.8")
        );
        assert_eq!(
            env.get("CODEXBAR_REMAINING_PERCENT").map(String::as_str),
            Some("20")
        );
        let payload = event.json_payload().unwrap();
        assert!(payload.len() < HooksConfig::MAX_PAYLOAD_BYTES);
        let parsed: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(parsed["event"], "quota_low");
        assert_eq!(parsed["provider"], "claude");
        assert!((parsed["remaining_percent"].as_f64().unwrap() - 20.0).abs() < 0.01);
    }

    #[test]
    fn build_hook_environment_whitelists_only_safe_keys() {
        let mut base = HashMap::new();
        base.insert("PATH".into(), "/usr/bin".into());
        base.insert("HOME".into(), "/home/u".into());
        base.insert("USER".into(), "u".into());
        base.insert("TEMP".into(), "C:\\Temp".into());
        base.insert("SECRET_API_KEY".into(), "should-not-pass".into());
        base.insert("OPENAI_API_KEY".into(), "nope".into());
        let event = HookEvent::new(HookEventType::QuotaReached, "codex").with_used_percent(100.0);
        let env = build_hook_environment(&base, &event);
        assert_eq!(env.get("PATH").map(String::as_str), Some("/usr/bin"));
        assert_eq!(env.get("HOME").map(String::as_str), Some("/home/u"));
        assert_eq!(env.get("USER").map(String::as_str), Some("u"));
        assert_eq!(env.get("TEMP").map(String::as_str), Some("C:\\Temp"));
        assert!(!env.contains_key("SECRET_API_KEY"));
        assert!(!env.contains_key("OPENAI_API_KEY"));
        assert_eq!(
            env.get("CODEXBAR_EVENT").map(String::as_str),
            Some("quota_reached")
        );
    }

    #[test]
    fn rate_limiter_suppresses_repeat_within_window() {
        let limiter = HookRateLimiter::new(Duration::from_secs(600));
        let event = HookEvent::new(HookEventType::RefreshFailed, "cursor");
        assert!(limiter.allow(&event));
        assert!(!limiter.allow(&event));
        let other = HookEvent::new(HookEventType::RefreshFailed, "claude");
        assert!(limiter.allow(&other));
    }

    #[test]
    fn rule_matches_event_list_and_provider() {
        #[cfg(windows)]
        let absolute = PathBuf::from(r"C:\Windows\System32\cmd.exe");
        #[cfg(not(windows))]
        let absolute = PathBuf::from("/usr/bin/true");
        let rule = HookRule {
            enabled: true,
            event: None,
            events: vec![HookEventType::QuotaLow, HookEventType::QuotaReached],
            provider: Some("codex".into()),
            threshold: Some(0.7),
            executable: absolute,
            arguments: vec![],
            timeout_secs: 10,
        };
        let match_event = HookEvent::new(HookEventType::QuotaLow, "codex").with_used_percent(80.0);
        assert!(rule.matches(&match_event));
        let low = HookEvent::new(HookEventType::QuotaLow, "codex").with_used_percent(50.0);
        assert!(!rule.matches(&low));
        let other = HookEvent::new(HookEventType::QuotaLow, "claude").with_used_percent(80.0);
        assert!(!rule.matches(&other));
    }

    #[test]
    fn config_matching_respects_enabled_flag() {
        let rule = HookRule {
            enabled: true,
            event: Some(HookEventType::QuotaReset),
            events: vec![],
            provider: None,
            threshold: None,
            executable: PathBuf::from("C:\\Windows\\System32\\cmd.exe"),
            arguments: vec![],
            timeout_secs: 5,
        };
        let event = HookEvent::new(HookEventType::QuotaReset, "claude");
        let disabled = HooksConfig {
            enabled: false,
            events: vec![rule.clone()],
        };
        assert!(disabled.matching_rules(&event).is_empty());
        let enabled = HooksConfig {
            enabled: true,
            events: vec![rule],
        };
        assert_eq!(enabled.matching_rules(&event).len(), 1);
    }

    #[test]
    fn relative_executable_never_matches() {
        let rule = HookRule {
            enabled: true,
            event: Some(HookEventType::QuotaLow),
            events: vec![],
            provider: None,
            threshold: None,
            executable: PathBuf::from("notify.sh"),
            arguments: vec![],
            timeout_secs: 10,
        };
        let event = HookEvent::new(HookEventType::QuotaLow, "codex").with_used_percent(90.0);
        assert!(!rule.matches(&event));
    }

    #[test]
    fn rate_limiter_key_is_stable() {
        let a = HookEvent::new(HookEventType::ProviderUnavailable, "warp")
            .with_window("session")
            .with_account("a");
        let b = HookEvent::new(HookEventType::ProviderUnavailable, "warp")
            .with_window("session")
            .with_account("a");
        assert_eq!(rate_limit_key(&a), rate_limit_key(&b));
        let _ = Arc::new(a);
    }
}
