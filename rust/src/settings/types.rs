use super::*;

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageThresholdOverride {
    pub high: Option<f64>,
    pub critical: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsageThresholds {
    pub high: f64,
    pub critical: f64,
}

/// WAV files assigned to individual notification events.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NotificationSoundPaths {
    pub predictive_warning: Option<String>,
    pub high_usage: Option<String>,
    pub critical_usage: Option<String>,
    pub exhausted: Option<String>,
    pub status_issue: Option<String>,
    pub session_depleted: Option<String>,
    pub session_restored: Option<String>,
}

/// Sound theme used when an event has no custom WAV file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NotificationSoundTheme {
    /// Use the existing Windows system-sound mapping.
    #[default]
    Windows,
    /// Use a distinct built-in CodexBar sound for each event.
    CodexBar,
}

pub fn normalize_usage_threshold_overrides(
    values: HashMap<String, UsageThresholdOverride>,
) -> HashMap<String, UsageThresholdOverride> {
    let known = crate::core::cli_name_map();
    values
        .into_iter()
        .filter_map(|(key, mut value)| {
            let (provider, window) = key
                .split_once(':')
                .map_or((key.as_str(), None), |(provider, window)| {
                    (provider, Some(window))
                });
            if !known.contains_key(provider)
                || window.is_some_and(|window| !matches!(window, "session" | "weekly"))
            {
                return None;
            }
            value.high = value.high.map(|number| number.clamp(0.0, 100.0));
            value.critical = value.critical.map(|number| number.clamp(0.0, 100.0));
            (value.high.is_some() || value.critical.is_some()).then_some((key, value))
        })
        .collect()
}

impl Settings {
    pub fn usage_thresholds(&self, provider: ProviderId, window: &str) -> UsageThresholds {
        let provider_key = provider.cli_name();
        let window_key = format!("{provider_key}:{window}");
        let provider_override = self.provider_usage_thresholds.get(provider_key);
        let window_override = self.provider_usage_thresholds.get(&window_key);
        UsageThresholds {
            high: window_override
                .and_then(|value| value.high)
                .or_else(|| provider_override.and_then(|value| value.high))
                .unwrap_or(self.high_usage_threshold),
            critical: window_override
                .and_then(|value| value.critical)
                .or_else(|| provider_override.and_then(|value| value.critical))
                .unwrap_or(self.critical_usage_threshold),
        }
    }
}

/// UI language for the application
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// English (default)
    #[default]
    English,
    /// Chinese (Simplified)
    Chinese,
    /// Chinese (Traditional, Taiwan)
    ChineseTraditional,
    /// Japanese
    Japanese,
    /// Korean
    Korean,
    /// Spanish (Mexican)
    Spanish,
    /// Russian
    Russian,
    /// Turkish
    Turkish,
}

impl Language {
    /// Get the display name for this language
    pub fn display_name(&self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Chinese => "中文",
            Language::ChineseTraditional => "繁體中文",
            Language::Japanese => "日本語",
            Language::Korean => "한국어",
            Language::Spanish => "Español",
            Language::Russian => "Русский",
            Language::Turkish => "Türkçe",
        }
    }

    /// Get all available languages
    pub fn all() -> &'static [Language] {
        &[
            Language::English,
            Language::Chinese,
            Language::ChineseTraditional,
            Language::Japanese,
            Language::Korean,
            Language::Spanish,
            Language::Russian,
            Language::Turkish,
        ]
    }

    /// Stable label used in bridge JSON and persisted settings
    /// (e.g. "english", "spanish").
    pub fn label(&self) -> &'static str {
        match self {
            Language::English => "english",
            Language::Chinese => "chinese",
            Language::ChineseTraditional => "chinesetraditional",
            Language::Japanese => "japanese",
            Language::Korean => "korean",
            Language::Spanish => "spanish",
            Language::Russian => "russian",
            Language::Turkish => "turkish",
        }
    }

    /// Accepted input aliases — short codes and native names (all lowercase).
    /// Used by resolve() for flexible language parsing.
    pub fn accepted_aliases(&self) -> &'static [&'static str] {
        match self {
            Language::English => &["en", "en-us"],
            Language::Chinese => &["zh", "zh-cn", "zh-hans", "中文"],
            Language::ChineseTraditional => &["zh-tw", "zh-hant", "zh-hant-tw", "繁體中文"],
            Language::Japanese => &["ja", "ja-jp", "日本語"],
            Language::Korean => &["ko", "ko-kr", "한국어"],
            Language::Spanish => &["es", "es-mx", "español"],
            Language::Russian => &["ru", "ru-ru", "русский"],
            Language::Turkish => &["tr", "tr-tr", "türkçe", "turkce"],
        }
    }

    /// Resolve a language from any recognized input string.
    /// Matches against label() and all accepted_aliases().
    /// Case-insensitive via Unicode-aware to_lowercase().
    pub fn resolve(raw: &str) -> Option<Language> {
        let normalized = raw.trim().to_lowercase();
        for lang in Self::all() {
            if normalized == lang.label() {
                return Some(*lang);
            }
            for alias in lang.accepted_aliases() {
                if normalized == *alias {
                    return Some(*lang);
                }
            }
        }
        None
    }
}

/// UI theme preference (Phase 12).
///
/// `Auto` resolves at runtime via `prefers-color-scheme` in the frontend;
/// `Light` and `Dark` are explicit overrides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemePreference {
    #[default]
    Auto,
    Light,
    Dark,
}

impl ThemePreference {
    pub fn all() -> &'static [ThemePreference] {
        &[
            ThemePreference::Auto,
            ThemePreference::Light,
            ThemePreference::Dark,
        ]
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            ThemePreference::Auto => "Auto",
            ThemePreference::Light => "Light",
            ThemePreference::Dark => "Dark",
        }
    }
}

/// Update channel for receiving updates
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    #[default]
    Stable,
    Beta,
}

impl UpdateChannel {
    /// Get the display name for this channel
    pub fn display_name(&self) -> &'static str {
        match self {
            UpdateChannel::Stable => "Stable",
            UpdateChannel::Beta => "Beta",
        }
    }

    /// Get a description for this channel
    pub fn description(&self) -> &'static str {
        match self {
            UpdateChannel::Stable => "Receive stable, tested releases",
            UpdateChannel::Beta => "Get early access to new features",
        }
    }
}

/// Tray icon display mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TrayIconMode {
    /// Single tray icon showing the primary provider or merged view
    #[default]
    Single,
    /// One tray icon per enabled provider
    PerProvider,
}

impl TrayIconMode {
    /// Get the display name for this mode
    pub fn display_name(&self) -> &'static str {
        match self {
            TrayIconMode::Single => "Single Icon",
            TrayIconMode::PerProvider => "Per Provider",
        }
    }

    /// Get a description for this mode
    pub fn description(&self) -> &'static str {
        match self {
            TrayIconMode::Single => "Show one tray icon for all providers",
            TrayIconMode::PerProvider => "Show a separate tray icon for each enabled provider",
        }
    }
}

/// Metric preference for display in tray and UI
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MetricPreference {
    #[default]
    Automatic,
    Session,
    Weekly,
    Model,
    Tertiary,
    Credits,
    #[serde(rename = "extraUsage", alias = "extrausage")]
    ExtraUsage,
    /// Current-month plan spend for PAYG providers (e.g. Mistral) that have
    /// cost data but no rate-limit window (#2821, #2947).
    MonthlyPlan,
    Average,
}

impl MetricPreference {
    /// Get all available metric preferences
    pub fn all() -> &'static [MetricPreference] {
        &[
            MetricPreference::Automatic,
            MetricPreference::Session,
            MetricPreference::Weekly,
            MetricPreference::Model,
            MetricPreference::Tertiary,
            MetricPreference::Credits,
            MetricPreference::ExtraUsage,
            MetricPreference::MonthlyPlan,
            MetricPreference::Average,
        ]
    }

    /// Get the display name for this metric
    pub fn display_name(&self) -> &'static str {
        match self {
            MetricPreference::Automatic => "Automatic",
            MetricPreference::Session => "Session",
            MetricPreference::Weekly => "Weekly",
            MetricPreference::Model => "Model",
            MetricPreference::Tertiary => "Tertiary",
            MetricPreference::Credits => "Credits",
            MetricPreference::ExtraUsage => "Extra usage",
            MetricPreference::MonthlyPlan => "Monthly plan spend",
            MetricPreference::Average => "Average",
        }
    }

    /// Get a description for this metric
    pub fn description(&self) -> &'static str {
        match self {
            MetricPreference::Automatic => "Automatically select the best metric",
            MetricPreference::Session => "Current session usage",
            MetricPreference::Weekly => "Weekly usage limit",
            MetricPreference::Model => "Model-specific limit",
            MetricPreference::Tertiary => "Tertiary usage limit",
            MetricPreference::Credits => "Credit balance",
            MetricPreference::ExtraUsage => "On-demand or extra usage budget",
            MetricPreference::MonthlyPlan => "Current-month plan spend (PAYG)",
            MetricPreference::Average => "Average across metrics",
        }
    }
}

/// How cost is rendered on provider MenuCards (#2976).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CostSummaryDisplayStyle {
    #[default]
    Compact,
    Detailed,
    Hidden,
}

impl CostSummaryDisplayStyle {
    pub fn all() -> &'static [CostSummaryDisplayStyle] {
        &[
            CostSummaryDisplayStyle::Compact,
            CostSummaryDisplayStyle::Detailed,
            CostSummaryDisplayStyle::Hidden,
        ]
    }
}

/// Per-provider configuration values.
///
/// All fields are optional / falsy-default so unused providers serialize as
/// empty objects (or skip serialization entirely). Defaults are applied via
/// the accessor methods on [`Settings`] (e.g. cookie source defaults to
/// `"auto"`, region defaults are provider-specific).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cookie_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manual_cookie_header: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_token: Option<String>,
    /// Optional secondary management credential for providers that expose one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub management_api_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// Wayfinder gateway URL override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ide_base_path: Option<String>,
    /// Codex-only: opt out of OpenAI web "extras" surfaces.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openai_web_extras: Option<bool>,
    /// Codex-only: show or hide Codex Spark quota rows in presentation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spark_usage_visible: Option<bool>,
    /// Codex-only: enable historical usage tracking in UI.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub historical_tracking: bool,
    /// Claude-only: avoid keychain prompts when reading credentials.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub avoid_keychain_prompts: bool,
    /// Per-provider accent color override (hex, e.g. "#FF5733"). `None`
    /// means the shipped brand color is used (#2972).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent_color: Option<String>,
}
