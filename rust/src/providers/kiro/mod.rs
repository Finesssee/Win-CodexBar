//! Kiro provider implementation
//!
//! Fetches usage data from Kiro (Amazon's AI coding assistant)
//! Uses kiro-cli for authentication and usage fetching

mod usage_limits;
pub mod version;

// Re-exports for version compatibility checking
#[allow(unused_imports)]
pub use version::{
    KiroVersion, detect_version, find_kiro_cli, get_version, is_compatible, is_installed,
};

use async_trait::async_trait;
use chrono::Datelike;
use regex_lite::Regex;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};

/// Kiro provider (AWS AI assistant)
pub struct KiroProvider {
    metadata: ProviderMetadata,
}

struct KiroCliUsage {
    plan_name: String,
    matched_new_format: bool,
    is_managed_plan: bool,
    reset_date: Option<chrono::DateTime<chrono::Utc>>,
    credits_percent: f64,
    matched_percent: bool,
    matched_credits: bool,
    bonus_window: Option<RateWindow>,
    overages_enabled: bool,
    overage_credits_used: Option<f64>,
    estimated_overage_cost: Option<f64>,
}

impl KiroProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Kiro,
                display_name: "Kiro",
                session_label: "Session",
                weekly_label: "Monthly",
                supports_opus: false,
                supports_credits: true,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://kiro.dev/account"),
                status_page_url: Some("https://health.aws.amazon.com"),
            },
        }
    }

    /// Get Kiro config directory
    fn get_kiro_config_path() -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        {
            dirs::config_dir().map(|p| p.join("Kiro"))
        }
        #[cfg(not(target_os = "windows"))]
        {
            dirs::home_dir().map(|p| p.join(".kiro"))
        }
    }

    /// Find Kiro CLI binary
    fn which_kiro() -> Option<PathBuf> {
        version::find_kiro_cli()
    }

    /// Check if user is logged in by running `kiro-cli whoami`
    async fn ensure_logged_in(&self) -> Result<(), ProviderError> {
        let cli_path = Self::which_kiro().ok_or_else(|| {
            ProviderError::NotInstalled(
                "kiro-cli not found. Install from https://kiro.dev".to_string(),
            )
        })?;

        #[cfg(windows)]
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        let mut cmd = Command::new(&cli_path);
        cmd.arg("whoami")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        let output = cmd
            .output()
            .await
            .map_err(|e| ProviderError::Other(format!("Failed to run kiro-cli: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_lowercase();
        let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
        let combined = format!("{} {}", stdout, stderr);

        if combined.contains("not logged in") || combined.contains("login required") {
            return Err(ProviderError::AuthRequired);
        }

        if !output.status.success() {
            return Err(ProviderError::Other(format!(
                "kiro-cli whoami failed with status {}",
                output.status.code().unwrap_or(-1)
            )));
        }

        Ok(())
    }

    /// Fetch usage via kiro-cli
    async fn fetch_via_cli(&self) -> Result<UsageSnapshot, ProviderError> {
        // First ensure we're logged in
        self.ensure_logged_in().await?;

        let cli_path = Self::which_kiro()
            .ok_or_else(|| ProviderError::NotInstalled("kiro-cli not found".to_string()))?;

        // Run the usage command.
        // Windows intentionally uses pipe-first (stdout/stderr Stdio::piped) rather than a dual
        // ConPTY path: kiro-cli `/usage` under --no-interactive emits parseable text on pipes, and
        // a second ConPTY probe would add flaky process-lifetime cost without better quota data.
        #[cfg(windows)]
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        let mut cmd = Command::new(&cli_path);
        cmd.args(["chat", "--no-interactive", "/usage"])
            .env("TERM", "xterm-256color")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        let output = cmd
            .output()
            .await
            .map_err(|e| ProviderError::Other(format!("Failed to run kiro-cli: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = if stdout.trim().is_empty() {
            stderr.as_ref()
        } else {
            stdout.as_ref()
        };

        // Check for login errors
        let lowered = combined.to_lowercase();
        if lowered.contains("not logged in")
            || lowered.contains("login required")
            || lowered.contains("failed to initialize auth portal")
            || lowered.contains("kiro-cli login")
            || lowered.contains("oauth error")
        {
            return Err(ProviderError::AuthRequired);
        }

        self.parse_cli_output(combined)
    }

    /// Parse CLI output to extract usage information
    fn parse_cli_output(&self, output: &str) -> Result<UsageSnapshot, ProviderError> {
        let stripped = Self::strip_ansi(output);
        let trimmed = stripped.trim();
        Self::validate_cli_output(trimmed, &stripped)?;
        let lowered = stripped.to_lowercase();
        let parsed = Self::parse_usage_fields(&stripped, &lowered);

        if let Some(usage) = Self::usage_without_metrics(&parsed) {
            return Ok(usage);
        }

        let mut usage = Self::usage_with_metrics(&parsed);
        usage = Self::apply_overage_windows(usage, &parsed);

        if let Some(bonus) = parsed.bonus_window {
            usage = usage.with_secondary(bonus);
        }

        Ok(usage)
    }

    fn validate_cli_output(trimmed: &str, stripped: &str) -> Result<(), ProviderError> {
        if trimmed.is_empty() {
            return Err(ProviderError::Parse(
                "Empty output from kiro-cli".to_string(),
            ));
        }

        if stripped
            .to_lowercase()
            .contains("could not retrieve usage information")
        {
            return Err(ProviderError::Parse(
                "Kiro CLI could not retrieve usage information".to_string(),
            ));
        }

        Ok(())
    }

    fn usage_without_metrics(parsed: &KiroCliUsage) -> Option<UsageSnapshot> {
        if parsed.matched_percent || parsed.matched_credits {
            return None;
        }

        let method = if parsed.matched_new_format || parsed.plan_name != "Kiro" {
            parsed.plan_name.as_str()
        } else {
            "Kiro (installed)"
        };

        Some(UsageSnapshot::new(RateWindow::new(0.0)).with_login_method(method))
    }

    fn usage_with_metrics(parsed: &KiroCliUsage) -> UsageSnapshot {
        let primary =
            RateWindow::with_details(parsed.credits_percent, None, parsed.reset_date, None);
        UsageSnapshot::new(primary).with_login_method(&parsed.plan_name)
    }

    fn parse_usage_fields(stripped: &str, lowered: &str) -> KiroCliUsage {
        let (plan_name, matched_new_format) = Self::parse_plan_name(stripped);
        let (credits_percent, matched_percent, matched_credits) =
            Self::parse_credit_usage(stripped);
        let (overages_enabled, overage_credits_used, estimated_overage_cost) =
            Self::parse_overages(stripped);

        KiroCliUsage {
            plan_name,
            matched_new_format,
            is_managed_plan: lowered.contains("managed by admin")
                || lowered.contains("managed by organization"),
            reset_date: Self::capture_text(stripped, r"resets on (\d{2}/\d{2})")
                .as_deref()
                .and_then(Self::parse_reset_date),
            credits_percent,
            matched_percent,
            matched_credits,
            bonus_window: Self::parse_bonus_window(stripped),
            overages_enabled,
            overage_credits_used,
            estimated_overage_cost,
        }
    }

    fn parse_plan_name(stripped: &str) -> (String, bool) {
        if let Some(plan_line) = Self::capture_text(stripped, r"Plan:\s*(.+)")
            && let Some(first_line) = plan_line.lines().next()
        {
            return (first_line.trim().to_string(), true);
        }

        let legacy = Self::capture_text(stripped, r"\|\s*(KIRO\s+\w+)")
            .unwrap_or_else(|| "Kiro".to_string());
        (legacy, false)
    }

    fn parse_credit_usage(stripped: &str) -> (f64, bool, bool) {
        if let Some(percent) = Self::capture_number(stripped, r"█+\s*(\d+)%") {
            return (percent, true, false);
        }

        let Some((used, total)) =
            Self::capture_number_pair(stripped, r"\((\d+\.?\d*)\s+of\s+(\d+)\s+covered")
        else {
            return (0.0, false, false);
        };

        let percent = if total > 0.0 {
            (used / total) * 100.0
        } else {
            0.0
        };
        (percent, false, true)
    }

    fn parse_bonus_window(stripped: &str) -> Option<RateWindow> {
        let (used, total) =
            Self::capture_number_pair(stripped, r"Bonus credits:\s*(\d+\.?\d*)/(\d+)")?;
        if total <= 0.0 {
            return None;
        }

        let expiry_desc = Self::capture_text(stripped, r"expires in (\d+) days?")
            .map(|days| format!("expires in {days}d"));
        Some(RateWindow::with_details(
            (used / total) * 100.0,
            None,
            None,
            expiry_desc,
        ))
    }

    fn parse_overages(stripped: &str) -> (bool, Option<f64>, Option<f64>) {
        let enabled = Self::capture_text(stripped, r"(?i)Overages:\s*([^\n]+)")
            .is_some_and(|value| value.to_lowercase().starts_with("enabled"));
        let credits_used = Self::capture_number(stripped, r"(?i)Credits used:\s*(\d+\.?\d*)");
        let estimated_cost =
            Self::capture_number(stripped, r"(?i)Est\.\s*cost:\s*\$?(\d+\.?\d*)\s*USD");
        (enabled, credits_used, estimated_cost)
    }

    fn apply_overage_windows(mut usage: UsageSnapshot, parsed: &KiroCliUsage) -> UsageSnapshot {
        if !parsed.overages_enabled {
            return usage;
        }

        if let Some(credits) = parsed.overage_credits_used {
            usage = usage.with_extra_rate_window(
                "kiro-overage-credits",
                "Overage usage",
                RateWindow::with_details(0.0, None, None, Some(format!("{credits:.2} credits"))),
            );
        }
        if let Some(cost) = parsed.estimated_overage_cost {
            usage = usage.with_extra_rate_window(
                "kiro-overage-cost",
                "Overage cost",
                RateWindow::with_details(0.0, None, None, Some(format!("${cost:.2} USD"))),
            );
        }

        usage
    }

    fn capture_text(text: &str, pattern: &str) -> Option<String> {
        Regex::new(pattern)
            .ok()
            .and_then(|re| re.captures(text))
            .and_then(|caps| caps.get(1).map(|m| m.as_str().trim().to_string()))
    }

    fn capture_number(text: &str, pattern: &str) -> Option<f64> {
        Self::capture_text(text, pattern)?.parse().ok()
    }

    fn capture_number_pair(text: &str, pattern: &str) -> Option<(f64, f64)> {
        Regex::new(pattern)
            .ok()
            .and_then(|re| re.captures(text))
            .and_then(|caps| {
                Some((
                    caps.get(1)?.as_str().parse().ok()?,
                    caps.get(2)?.as_str().parse().ok()?,
                ))
            })
    }

    /// Strip ANSI escape sequences from text
    fn strip_ansi(text: &str) -> String {
        // Simple ANSI stripping - remove escape sequences
        let mut result = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '\x1B' {
                // Skip escape sequence
                if chars.peek() == Some(&'[') {
                    chars.next(); // consume '['
                    // Skip until we hit a letter
                    while let Some(&next) = chars.peek() {
                        chars.next();
                        if next.is_ascii_alphabetic() {
                            break;
                        }
                    }
                } else if chars.peek() == Some(&']') {
                    // OSC sequence - skip until BEL or ST
                    for next in chars.by_ref() {
                        if next == '\x07' || next == '\\' {
                            break;
                        }
                    }
                }
            } else {
                result.push(c);
            }
        }

        result
    }

    /// Parse reset date from MM/DD format
    fn parse_reset_date(date_str: &str) -> Option<chrono::DateTime<chrono::Utc>> {
        let parts: Vec<&str> = date_str.split('/').collect();
        if parts.len() != 2 {
            return None;
        }

        let month: u32 = parts[0].parse().ok()?;
        let day: u32 = parts[1].parse().ok()?;

        let now = chrono::Utc::now();
        let current_year = now.year();

        // Try current year first
        if let Some(date) = chrono::NaiveDate::from_ymd_opt(current_year, month, day) {
            let datetime = date.and_hms_opt(0, 0, 0)?;
            let utc =
                chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(datetime, chrono::Utc);
            if utc > now {
                return Some(utc);
            }
        }

        // If in the past, use next year
        if let Some(date) = chrono::NaiveDate::from_ymd_opt(current_year + 1, month, day) {
            let datetime = date.and_hms_opt(0, 0, 0)?;
            return Some(chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
                datetime,
                chrono::Utc,
            ));
        }

        None
    }
}

impl Default for KiroProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for KiroProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Kiro
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Fetching Kiro usage");

        match ctx.source_mode {
            SourceMode::Auto | SourceMode::Cli | SourceMode::Web => {
                // Web has no independent Kiro transport; it intentionally shares
                // the same CLI baseline and optional GetUsageLimits enrichment.
                let usage = self.fetch_via_cli().await?;
                let usage = match usage_limits::fetch_usage_limits().await {
                    Ok(limits) => usage_limits::apply_usage_limits(usage, &limits),
                    Err(error) => {
                        tracing::debug!("Kiro GetUsageLimits enrichment unavailable: {error}");
                        usage
                    }
                };
                Ok(ProviderFetchResult::new(usage, "cli"))
            }
            SourceMode::OAuth => Err(ProviderError::UnsupportedSource(SourceMode::OAuth)),
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::Cli]
    }

    fn supports_web(&self) -> bool {
        false
    }

    fn supports_cli(&self) -> bool {
        true
    }
    /// Kiro's CLI probes raise `NotInstalled` when the `kiro-cli` binary is
    /// missing ("kiro-cli not found. Install from https://kiro.dev") — an
    /// installation gap, not a credential problem — so it surfaces as an
    /// offline local runtime (matching the pre-backend classifier's
    /// treatment of CLI-presence failures). The guard is message-scoped:
    /// the state-database token lookup ("Kiro CLI state database not found
    /// at ...") is auth-flavored and keeps the default mapping.
    fn error_state_kind(&self, error: &ProviderError) -> crate::core::ProviderStateKind {
        match error {
            ProviderError::NotInstalled(msg) if msg.contains("kiro-cli not found") => {
                crate::core::ProviderStateKind::LocalRuntimeOffline
            }
            _ => error.state_kind(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_presence_maps_to_local_runtime_offline_but_state_db_stays_default() {
        assert_eq!(
            KiroProvider::new().error_state_kind(&ProviderError::NotInstalled(
                "kiro-cli not found. Install from https://kiro.dev".to_string(),
            )),
            crate::core::ProviderStateKind::LocalRuntimeOffline
        );
        // The state-database token lookup is auth-flavored and keeps the
        // default mapping.
        assert_eq!(
            KiroProvider::new().error_state_kind(&ProviderError::NotInstalled(
                "Kiro CLI state database not found at C:\\Users\\x\\Kiro-Cli\\data.sqlite3"
                    .to_string(),
            )),
            crate::core::ProviderStateKind::NeedsAuthentication
        );
    }
}
