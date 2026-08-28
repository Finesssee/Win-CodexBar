use serde::{Deserialize, Serialize};

use codexbar::core::ProviderId;

use crate::surface::SurfaceMode;

// Must mirror the frontend `SettingsTabId` union
// (apps/desktop-tauri/src/types/bridge.ts) and `TAB_META` in
// apps/desktop-tauri/src/surfaces/Settings.tsx.
const SETTINGS_TAB_IDS: &[&str] = &[
    "general",
    "providers",
    "notifications",
    "menuBar",
    "menu",
    "usageSpend",
    "advanced",
    "about",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SurfaceTarget {
    #[default]
    Summary,
    Dashboard,
    Provider {
        #[serde(rename = "providerId")]
        provider_id: String,
    },
    Settings {
        tab: String,
    },
}

impl SurfaceTarget {
    #[allow(
        dead_code,
        reason = "surface target type reserved for future window management integration"
    )]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "summary" => Some(Self::Summary),
            "dashboard" => Some(Self::Dashboard),
            _ => {
                if let Some(provider_id) = s.strip_prefix("provider:")
                    && !provider_id.is_empty()
                {
                    return Some(Self::Provider {
                        provider_id: provider_id.to_string(),
                    });
                }

                if let Some(tab) = s.strip_prefix("settings:")
                    && !tab.is_empty()
                {
                    return Some(Self::Settings {
                        tab: tab.to_string(),
                    });
                }

                None
            }
        }
    }

    pub fn default_for_mode(mode: SurfaceMode) -> Self {
        match mode {
            SurfaceMode::Hidden | SurfaceMode::TrayPanel => Self::Summary,
            SurfaceMode::PopOut => Self::Dashboard,
            SurfaceMode::Settings => Self::Settings {
                tab: "general".into(),
            },
        }
    }

    pub fn mode(&self) -> SurfaceMode {
        match self {
            Self::Summary => SurfaceMode::TrayPanel,
            Self::Dashboard | Self::Provider { .. } => SurfaceMode::PopOut,
            Self::Settings { .. } => SurfaceMode::Settings,
        }
    }
}

pub fn is_supported_provider_id(provider_id: &str) -> bool {
    ProviderId::all()
        .iter()
        .any(|provider| provider.cli_name() == provider_id)
}

pub fn is_supported_settings_tab(tab: &str) -> bool {
    SETTINGS_TAB_IDS.contains(&tab)
}

#[cfg(test)]
mod tests {
    use super::{SurfaceTarget, is_supported_provider_id, is_supported_settings_tab};
    use serde_json::json;

    #[test]
    fn parse_provider_target() {
        let target = SurfaceTarget::parse("provider:codex").unwrap();
        assert_eq!(
            target,
            SurfaceTarget::Provider {
                provider_id: "codex".into()
            }
        );
    }

    #[test]
    fn parse_settings_about_target() {
        let target = SurfaceTarget::parse("settings:about").unwrap();
        assert_eq!(
            target,
            SurfaceTarget::Settings {
                tab: "about".into()
            }
        );
    }

    #[test]
    fn serialize_provider_target_for_bridge() {
        let value = serde_json::to_value(SurfaceTarget::Provider {
            provider_id: "codex".into(),
        })
        .unwrap();

        assert_eq!(value, json!({ "kind": "provider", "providerId": "codex" }));
    }

    #[test]
    fn deserialize_settings_target_from_bridge() {
        let target: SurfaceTarget =
            serde_json::from_value(json!({ "kind": "settings", "tab": "general" })).unwrap();

        assert_eq!(
            target,
            SurfaceTarget::Settings {
                tab: "general".into()
            }
        );
    }

    #[test]
    fn target_mode_matches_surface_mode() {
        assert_eq!(
            SurfaceTarget::Summary.mode(),
            crate::surface::SurfaceMode::TrayPanel
        );
        assert_eq!(
            SurfaceTarget::Dashboard.mode(),
            crate::surface::SurfaceMode::PopOut
        );
        assert_eq!(
            SurfaceTarget::Provider {
                provider_id: "claude".into()
            }
            .mode(),
            crate::surface::SurfaceMode::PopOut
        );
        assert_eq!(
            SurfaceTarget::Settings {
                tab: "about".into()
            }
            .mode(),
            crate::surface::SurfaceMode::Settings
        );
    }

    #[test]
    fn supported_provider_ids_match_catalog() {
        assert!(is_supported_provider_id("codex"));
        assert!(is_supported_provider_id("bedrock"));
        assert!(!is_supported_provider_id("not-a-provider"));
    }

    #[test]
    fn supported_settings_tabs_match_shell_tabs() {
        assert!(is_supported_settings_tab("menuBar"));
        assert!(is_supported_settings_tab("about"));
        assert!(!is_supported_settings_tab("apiKeys"));
        assert!(!is_supported_settings_tab("security"));
    }
}
