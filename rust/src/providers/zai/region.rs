//! z.ai API region policy (upstream 0.48.0 `ZaiAPIRegion`).
//!
//! Centralizes the Global (`api.z.ai`) vs BigModel CN (`open.bigmodel.cn`)
//! endpoint/auth routing for GLM Coding Plan accounts. Region-specific
//! behavior lives here — not in the provider fetch path.

use reqwest::Url;

/// Canonical quota API path shared by both regions.
const QUOTA_PATH: &str = "/api/monitor/usage/quota/limit";
/// Per-model usage API path shared by both regions.
const MODEL_USAGE_PATH: &str = "/api/monitor/usage/model-usage";

/// Which z.ai API plane a credential belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ZaiRegion {
    /// International plane, `https://api.z.ai`.
    Global,
    /// China plane, `https://open.bigmodel.cn` (Zhipu/GLM).
    BigModelCn,
}

impl ZaiRegion {
    /// Human-readable label (matches upstream `displayName`).
    pub fn display_name(self) -> &'static str {
        match self {
            ZaiRegion::Global => "Global (api.z.ai)",
            ZaiRegion::BigModelCn => "BigModel CN (open.bigmodel.cn)",
        }
    }
    /// API base URL for this region.
    pub fn base_url(self) -> Url {
        Url::parse(match self {
            ZaiRegion::Global => "https://api.z.ai",
            ZaiRegion::BigModelCn => "https://open.bigmodel.cn",
        })
        .expect("region base URL is a valid constant")
    }

    /// Quota-limit endpoint for this region.
    pub fn quota_limit_url(self) -> Url {
        self.base_url()
            .join(QUOTA_PATH)
            .expect("region quota URL is a valid constant")
    }

    /// Model-usage endpoint for this region.
    pub fn model_usage_url(self) -> Url {
        self.base_url()
            .join(MODEL_USAGE_PATH)
            .expect("region model-usage URL is a valid constant")
    }

    /// Canonical host for this region's quota endpoint.
    pub fn canonical_host(self) -> &'static str {
        match self {
            ZaiRegion::Global => "api.z.ai",
            ZaiRegion::BigModelCn => "open.bigmodel.cn",
        }
    }

    /// Personal-plan dashboard for this region.
    pub fn dashboard_url(self) -> Url {
        Url::parse(match self {
            ZaiRegion::Global => "https://z.ai/manage-apikey/coding-plan/personal/my-plan",
            ZaiRegion::BigModelCn => "https://bigmodel.cn/coding-plan/personal/usage",
        })
        .expect("region dashboard URL is a valid constant")
    }

    /// Team dashboard for this region (global reuses the personal dashboard).
    pub fn team_dashboard_url(self) -> Url {
        match self {
            ZaiRegion::Global => self.dashboard_url(),
            ZaiRegion::BigModelCn => Url::parse("https://bigmodel.cn/coding-plan/team/usage-stats")
                .expect("region team dashboard URL is a valid constant"),
        }
    }

    /// Parse the persisted settings `api_region` value into a region.
    ///
    /// Accepted values cover the upstream region IDs (`global`,
    /// `bigmodel-cn`) plus the legacy settings aliases used since the BigModel
    /// CN endpoints landed (`cn`, `bigmodel`, `bigmodel_cn`, `international`,
    /// `intl`). Unknown/empty values return `Global` (upstream default).
    pub fn from_settings_value(raw: Option<&str>) -> Self {
        match raw
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("bigmodel-cn") | Some("bigmodel_cn") | Some("bigmodel") | Some("cn")
            | Some("china") => ZaiRegion::BigModelCn,
            _ => ZaiRegion::Global,
        }
    }
}

impl std::fmt::Display for ZaiRegion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}

/// Hosts that belong to one of the two canonical z.ai planes.
///
/// Endpoint overrides pointing at these hosts are pinned to the matching
/// region; any other host (relays, local proxies) is region-neutral.
pub fn canonical_region_for_host(host: &str) -> Option<ZaiRegion> {
    let host = host.trim().to_ascii_lowercase();
    if host == ZaiRegion::Global.canonical_host() {
        Some(ZaiRegion::Global)
    } else if host == ZaiRegion::BigModelCn.canonical_host() {
        Some(ZaiRegion::BigModelCn)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_urls_match_upstream() {
        assert_eq!(
            ZaiRegion::Global.quota_limit_url().as_str(),
            "https://api.z.ai/api/monitor/usage/quota/limit"
        );
        assert_eq!(
            ZaiRegion::BigModelCn.quota_limit_url().as_str(),
            "https://open.bigmodel.cn/api/monitor/usage/quota/limit"
        );
        assert_eq!(
            ZaiRegion::Global.model_usage_url().as_str(),
            "https://api.z.ai/api/monitor/usage/model-usage"
        );
        assert_eq!(
            ZaiRegion::BigModelCn.model_usage_url().as_str(),
            "https://open.bigmodel.cn/api/monitor/usage/model-usage"
        );
        assert_eq!(
            ZaiRegion::BigModelCn.team_dashboard_url().as_str(),
            "https://bigmodel.cn/coding-plan/team/usage-stats"
        );
    }

    #[test]
    fn settings_aliases_map_to_regions() {
        assert_eq!(
            ZaiRegion::from_settings_value(Some("cn")),
            ZaiRegion::BigModelCn
        );
        assert_eq!(
            ZaiRegion::from_settings_value(Some(" bigmodel ")),
            ZaiRegion::BigModelCn
        );
        assert_eq!(
            ZaiRegion::from_settings_value(Some("bigmodel-cn")),
            ZaiRegion::BigModelCn
        );
        assert_eq!(
            ZaiRegion::from_settings_value(Some("bigmodel_cn")),
            ZaiRegion::BigModelCn
        );
        assert_eq!(
            ZaiRegion::from_settings_value(Some("global")),
            ZaiRegion::Global
        );
        assert_eq!(
            ZaiRegion::from_settings_value(Some("intl")),
            ZaiRegion::Global
        );
        assert_eq!(ZaiRegion::from_settings_value(Some("")), ZaiRegion::Global);
        assert_eq!(ZaiRegion::from_settings_value(None), ZaiRegion::Global);
    }

    #[test]
    fn canonical_hosts_bind_to_regions() {
        assert_eq!(
            canonical_region_for_host("api.z.ai"),
            Some(ZaiRegion::Global)
        );
        assert_eq!(
            canonical_region_for_host("OPEN.BIGMODEL.CN"),
            Some(ZaiRegion::BigModelCn)
        );
        assert_eq!(canonical_region_for_host("relay.example.com"), None);
    }
}
