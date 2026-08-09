//! Domain model for Codex accounts and their usage snapshots.
//!
//! Field names intentionally mirror CodexControl's `windows/.../models.py` (MIT)
//! so stored data interops with that project.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// `parse_from_rfc3339` requires an offset; append `Z` only when none is present.
pub fn parse_datetime(value: &str) -> Option<DateTime<Utc>> {
    let text = value.trim();
    if text.is_empty() {
        return None;
    }
    // `parse_from_rfc3339` requires an offset; append `Z` only when none is present.
    let text_dt = text.trim();
    let has_offset = text_dt.ends_with(['Z', 'z']) || contains_offset(text_dt);
    let normalized = if has_offset {
        String::from(text_dt)
    } else {
        format!("{text_dt}Z")
    };
    DateTime::parse_from_rfc3339(&normalized)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Does the string carry an explicit `+HH:MM` / `-HH:MM` UTC offset (not `Z`)?
fn contains_offset(text: &str) -> bool {
    let Some(time_start) = text.find('T') else {
        return false;
    };
    let rest = &text[time_start + 1..];
    let Some(sign) = rest.rfind(['+', '-']) else {
        return false;
    };
    let tail = &rest[sign + 1..];
    let mut chars = tail.chars();
    let digits = chars.next().is_some_and(|c| c.is_ascii_digit())
        && chars.next().is_some_and(|c| c.is_ascii_digit());
    digits && tail.contains(':')
}

/// Format a UTC instant the same way CodexControl does (`...Z`).
pub fn format_datetime(value: Option<DateTime<Utc>>) -> Option<String> {
    value.map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
}

pub fn utc_now() -> DateTime<Utc> {
    Utc::now()
}

fn normalize_identifier(value: Option<&str>) -> Option<String> {
    value
        .map(|v| v.trim().to_lowercase())
        .filter(|v| !v.is_empty())
}

/// Where an account's `CODEX_HOME` lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CodexAccountSource {
    /// The environment's `~/.codex` (the identity the Codex CLI/Desktop uses).
    Ambient,
    /// An app-owned home directory under `managed-homes/`.
    ManagedByApp,
}

impl CodexAccountSource {
    pub fn display_name(self) -> &'static str {
        match self {
            CodexAccountSource::Ambient => "System",
            CodexAccountSource::ManagedByApp => "Managed",
        }
    }

    /// Whether the app owns (and may delete) this account's files.
    pub fn owns_files(self) -> bool {
        matches!(self, CodexAccountSource::ManagedByApp)
    }

    pub fn from_raw(value: &str) -> Option<Self> {
        match value {
            "ambient" => Some(CodexAccountSource::Ambient),
            "managedByApp" | "importedCodexBar" => Some(CodexAccountSource::ManagedByApp),
            _ => None,
        }
    }
}

/// A stored Codex account.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexAccount {
    pub id: Uuid,
    pub nickname: Option<String>,
    pub email_hint: Option<String>,
    pub auth_subject: Option<String>,
    pub provider_account_id: Option<String>,
    pub codex_home_path: PathBuf,
    pub source: CodexAccountSource,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_authenticated_at: Option<DateTime<Utc>>,
}

impl CodexAccount {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: Uuid,
        nickname: Option<String>,
        email_hint: Option<String>,
        auth_subject: Option<String>,
        provider_account_id: Option<String>,
        codex_home_path: PathBuf,
        source: CodexAccountSource,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
        last_authenticated_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            id,
            nickname,
            email_hint,
            auth_subject,
            provider_account_id,
            codex_home_path,
            source,
            created_at,
            updated_at,
            last_authenticated_at,
        }
    }

    pub fn display_name(&self) -> String {
        if let Some(nickname) = self
            .nickname
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return nickname.to_string();
        }
        if let Some(email) = self.email_hint.as_deref().filter(|s| !s.is_empty()) {
            return email.to_string();
        }
        self.codex_home_path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| self.codex_home_path.display().to_string())
    }

    pub fn normalized_email_hint(&self) -> Option<String> {
        normalize_identifier(self.email_hint.as_deref())
    }

    pub fn normalized_auth_subject(&self) -> Option<String> {
        normalize_identifier(self.auth_subject.as_deref())
    }

    pub fn normalized_provider_account_id(&self) -> Option<String> {
        normalize_identifier(self.provider_account_id.as_deref())
    }

    pub fn standardized_home_path(&self) -> String {
        std::path::absolute(&self.codex_home_path)
            .unwrap_or_else(|_| self.codex_home_path.clone())
            .to_string_lossy()
            .to_lowercase()
    }

    fn source_priority(&self) -> u8 {
        if self.source.owns_files() { 2 } else { 1 }
    }

    fn recency_date(&self) -> DateTime<Utc> {
        self.last_authenticated_at.unwrap_or(self.updated_at)
    }

    /// Whether two accounts refer to the same identity.
    pub fn matches(&self, other: &CodexAccount) -> bool {
        if self.standardized_home_path() == other.standardized_home_path() {
            return true;
        }
        if let (Some(a), Some(b)) = (
            self.normalized_provider_account_id(),
            other.normalized_provider_account_id(),
        ) && a == b
        {
            return true;
        }
        if self.normalized_provider_account_id().is_some()
            || other.normalized_provider_account_id().is_some()
        {
            return false;
        }
        if let (Some(a), Some(b)) = (
            self.normalized_auth_subject(),
            other.normalized_auth_subject(),
        ) && a == b
        {
            return true;
        }
        if let (Some(a), Some(b)) = (self.normalized_email_hint(), other.normalized_email_hint())
            && a == b
        {
            return true;
        }
        false
    }

    /// Merge a fresher discovery into this account, preferring managed/recency.
    pub fn merge_from(&mut self, other: &CodexAccount) {
        if self
            .nickname
            .as_deref()
            .map(str::trim)
            .is_none_or(|s| s.is_empty())
        {
            self.nickname = other.nickname.clone();
        }

        let prefer_other = other.source_priority() > self.source_priority()
            || (other.source_priority() == self.source_priority()
                && other.recency_date() >= self.recency_date());

        let pick = |mine: &mut Option<String>, value: Option<&String>| {
            let newer = prefer_other && value.is_some_and(|v| !v.trim().is_empty());
            if newer || mine.is_none() {
                *mine = value.cloned();
            }
        };
        pick(&mut self.email_hint, other.email_hint.as_ref());
        pick(&mut self.auth_subject, other.auth_subject.as_ref());
        pick(
            &mut self.provider_account_id,
            other.provider_account_id.as_ref(),
        );

        if prefer_other {
            self.source = other.source;
            self.codex_home_path = other.codex_home_path.clone();
        }

        self.updated_at = self.updated_at.max(other.updated_at);
        self.last_authenticated_at = match (self.last_authenticated_at, other.last_authenticated_at)
        {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
    }
}

/// Identity of a previously-removed account, kept to avoid re-adding it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovedAccountIdentity {
    pub id: Uuid,
    pub email_hint: Option<String>,
    pub auth_subject: Option<String>,
    pub provider_account_id: Option<String>,
    pub codex_home_path: PathBuf,
    pub source: CodexAccountSource,
    pub removed_at: DateTime<Utc>,
}

impl RemovedAccountIdentity {
    pub fn from_account(account: &CodexAccount) -> Self {
        Self {
            id: Uuid::new_v4(),
            email_hint: account.email_hint.clone(),
            auth_subject: account.auth_subject.clone(),
            provider_account_id: account.provider_account_id.clone(),
            codex_home_path: account.codex_home_path.clone(),
            source: account.source,
            removed_at: utc_now(),
        }
    }

    pub fn matches(&self, account: &CodexAccount) -> bool {
        if self.standardized_home_path() == account.standardized_home_path() {
            return true;
        }
        if let (Some(a), Some(b)) = (
            normalize_identifier(self.provider_account_id.as_deref()),
            account.normalized_provider_account_id(),
        ) && a == b
        {
            return true;
        }
        if self
            .provider_account_id
            .as_ref()
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
            || account.provider_account_id.as_ref().is_some()
        {
            return false;
        }
        if let (Some(a), Some(b)) = (
            normalize_identifier(self.auth_subject.as_deref()),
            account.normalized_auth_subject(),
        ) && a == b
        {
            return true;
        }
        if let (Some(a), Some(b)) = (
            normalize_identifier(self.email_hint.as_deref()),
            account.normalized_email_hint(),
        ) && a == b
        {
            return true;
        }
        false
    }

    fn standardized_home_path(&self) -> String {
        std::path::absolute(&self.codex_home_path)
            .unwrap_or_else(|_| self.codex_home_path.clone())
            .to_string_lossy()
            .to_lowercase()
    }
}

/// A single quota window (session or weekly).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindowSnapshot {
    pub used_percent: f64,
    pub reset_at: Option<DateTime<Utc>>,
    pub limit_window_seconds: i64,
}

impl UsageWindowSnapshot {
    pub fn new(
        used_percent: f64,
        reset_at: Option<DateTime<Utc>>,
        limit_window_seconds: i64,
    ) -> Self {
        Self {
            used_percent,
            reset_at,
            limit_window_seconds,
        }
    }

    pub fn remaining_percent(&self) -> f64 {
        100.0_f64.max(self.used_percent) - self.used_percent
    }

    pub fn role(&self) -> WindowRole {
        use crate::core::RateWindowCadence;
        match RateWindowCadence::from_seconds(self.limit_window_seconds) {
            RateWindowCadence::Session => WindowRole::Session,
            RateWindowCadence::Weekly => WindowRole::Weekly,
            RateWindowCadence::Monthly => WindowRole::Monthly,
            RateWindowCadence::Unknown => WindowRole::Unknown,
        }
    }
}

/// Normalized role of a window based on its duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowRole {
    Session,
    Weekly,
    Monthly,
    Unknown,
}

/// Codex credits balance.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditsBalanceSnapshot {
    pub has_credits: bool,
    pub unlimited: bool,
    pub balance: Option<f64>,
}

impl CreditsBalanceSnapshot {
    pub fn display_value(&self) -> String {
        if self.unlimited {
            return "Unlimited".to_string();
        }
        if let Some(balance) = self.balance {
            return format!("{balance:.2}")
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string();
        }
        if self.has_credits {
            return "Available".to_string();
        }
        "None".to_string()
    }
}

/// A fetched snapshot for one Codex account.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsageSnapshot {
    pub email: Option<String>,
    pub provider_account_id: Option<String>,
    pub plan: Option<String>,
    pub allowed: Option<bool>,
    pub limit_reached: Option<bool>,
    pub primary_window: Option<UsageWindowSnapshot>,
    pub secondary_window: Option<UsageWindowSnapshot>,
    pub credits: Option<CreditsBalanceSnapshot>,
    pub updated_at: DateTime<Utc>,
}

impl AccountUsageSnapshot {
    pub fn is_quota_blocked(&self) -> bool {
        self.limit_reached == Some(true) || self.allowed == Some(false)
    }

    pub fn has_quota_windows(&self) -> bool {
        self.primary_window.is_some() || self.secondary_window.is_some()
    }

    pub fn has_usable_quota_now(&self) -> bool {
        if self.is_quota_blocked() {
            return false;
        }
        let values = [self.primary_window.as_ref(), self.secondary_window.as_ref()]
            .into_iter()
            .flatten()
            .map(|w| w.remaining_percent());
        let mut values = values.peekable();
        values.peek().is_some() && values.any(|v| v > 0.001)
    }

    pub fn lowest_remaining_percent(&self) -> f64 {
        if self.is_quota_blocked() {
            return 0.0;
        }
        [self.secondary_window.as_ref(), self.primary_window.as_ref()]
            .into_iter()
            .flatten()
            .map(|w| w.remaining_percent())
            .fold(f64::MAX, f64::min)
    }

    pub fn next_reset_at(&self) -> Option<DateTime<Utc>> {
        [self.primary_window.as_ref(), self.secondary_window.as_ref()]
            .into_iter()
            .flatten()
            .filter_map(|w| w.reset_at)
            .min()
    }
}

/// Sort weight used to order accounts by practical usefulness.
pub fn account_sort_priority(snapshot: &AccountUsageSnapshot) -> u8 {
    if snapshot.has_usable_quota_now() {
        0
    } else if snapshot.next_reset_at().is_some() {
        1
    } else {
        2
    }
}

fn _path_is_trailing(path: &Path) -> bool {
    path.as_os_str()
        .to_string_lossy()
        .ends_with(std::path::MAIN_SEPARATOR)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(
        id: &str,
        home: &str,
        source: CodexAccountSource,
        provider_id: Option<&str>,
    ) -> CodexAccount {
        CodexAccount::new(
            Uuid::parse_str(id).unwrap(),
            None,
            None,
            None,
            provider_id.map(str::to_string),
            PathBuf::from(home),
            source,
            utc_now(),
            utc_now(),
            None,
        )
    }

    #[test]
    fn matches_by_home_path() {
        let a = account(
            "11111111-1111-1111-1111-111111111111",
            "/x/a",
            CodexAccountSource::ManagedByApp,
            None,
        );
        let b = account(
            "22222222-2222-2222-2222-222222222222",
            "/x/a",
            CodexAccountSource::ManagedByApp,
            None,
        );
        assert!(a.matches(&b));
    }

    #[test]
    fn matches_by_provider_account_id() {
        let a = account(
            "11111111-1111-1111-1111-111111111111",
            "/x/a",
            CodexAccountSource::ManagedByApp,
            Some("acct-1"),
        );
        let b = account(
            "22222222-2222-2222-2222-222222222222",
            "/y/b",
            CodexAccountSource::ManagedByApp,
            Some("ACCT-1"),
        );
        assert!(a.matches(&b));
    }

    #[test]
    fn disambiguates_different_provider_ids() {
        let a = account(
            "11111111-1111-1111-1111-111111111111",
            "/x/a",
            CodexAccountSource::ManagedByApp,
            Some("acct-1"),
        );
        let b = account(
            "22222222-2222-2222-2222-222222222222",
            "/y/b",
            CodexAccountSource::ManagedByApp,
            Some("acct-2"),
        );
        assert!(!a.matches(&b));
    }

    #[test]
    fn source_displays_and_ownership() {
        assert_eq!(CodexAccountSource::Ambient.display_name(), "System");
        assert_eq!(CodexAccountSource::ManagedByApp.display_name(), "Managed");
        assert!(CodexAccountSource::ManagedByApp.owns_files());
        assert!(!CodexAccountSource::Ambient.owns_files());
    }

    #[test]
    fn window_role_classification() {
        assert_eq!(
            UsageWindowSnapshot::new(0.0, None, 18_000).role(),
            WindowRole::Session
        );
        assert_eq!(
            UsageWindowSnapshot::new(0.0, None, 604_800).role(),
            WindowRole::Weekly
        );
        assert_eq!(
            UsageWindowSnapshot::new(0.0, None, 1234).role(),
            WindowRole::Unknown
        );
        assert_eq!(
            UsageWindowSnapshot::new(0.0, None, 2_592_000).role(),
            WindowRole::Monthly
        );
    }

    #[test]
    fn blocked_account_has_no_usable_quota() {
        let snapshot = AccountUsageSnapshot {
            email: None,
            provider_account_id: None,
            plan: None,
            allowed: Some(false),
            limit_reached: None,
            primary_window: Some(UsageWindowSnapshot::new(10.0, None, 18_000)),
            secondary_window: None,
            credits: None,
            updated_at: utc_now(),
        };
        assert!(snapshot.is_quota_blocked());
        assert!(!snapshot.has_usable_quota_now());
        assert_eq!(snapshot.lowest_remaining_percent(), 0.0);
    }

    #[test]
    fn parse_datetime_accepts_z_and_offset() {
        assert!(parse_datetime("2026-01-01T00:00:00Z").is_some());
        assert!(parse_datetime("2026-01-01T00:00:00+00:00").is_some());
        assert!(parse_datetime("").is_none());
    }

    #[test]
    fn credits_display_value() {
        assert_eq!(
            CreditsBalanceSnapshot {
                has_credits: true,
                unlimited: true,
                balance: None
            }
            .display_value(),
            "Unlimited"
        );
        assert_eq!(
            CreditsBalanceSnapshot {
                has_credits: true,
                unlimited: false,
                balance: Some(12.50)
            }
            .display_value(),
            "12.5"
        );
        assert_eq!(
            CreditsBalanceSnapshot {
                has_credits: true,
                unlimited: false,
                balance: None
            }
            .display_value(),
            "Available"
        );
        assert_eq!(
            CreditsBalanceSnapshot {
                has_credits: false,
                unlimited: false,
                balance: None
            }
            .display_value(),
            "None"
        );
    }

    #[test]
    fn merge_prefers_managed_and_recency() {
        let mut managed = account(
            "11111111-1111-1111-1111-111111111111",
            "/x/managed",
            CodexAccountSource::ManagedByApp,
            None,
        );
        managed.nickname = Some("My acct".to_string());
        let ambient = account(
            "22222222-2222-2222-2222-222222222222",
            "~/.codex-like/ambient",
            CodexAccountSource::Ambient,
            None,
        );
        managed.merge_from(&ambient);
        assert_eq!(managed.source, CodexAccountSource::ManagedByApp);
        assert_eq!(managed.display_name(), "My acct");
    }

    #[test]
    fn display_name_falls_back_to_home() {
        let acct = account(
            "11111111-1111-1111-1111-111111111111",
            "/x/my-home-dir",
            CodexAccountSource::ManagedByApp,
            None,
        );
        assert!(
            acct.display_name().ends_with("my-home-dir")
                || acct.display_name().contains("my-home-dir")
        );
        let _ = _path_is_trailing(std::path::Path::new("/x/"));
    }
}
