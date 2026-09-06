use super::*;

// ── Provider summaries + ordering ─────────────────────────────────────

/// Lightweight provider entry returned to the UI after a reorder.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSummary {
    pub id: String,
    pub display_name: String,
    pub enabled: bool,
    pub order: u32,
}

/// Build `ProviderSummary` list honouring the persisted `provider_order`.
pub(crate) fn build_provider_summaries(settings: &Settings) -> Vec<ProviderSummary> {
    let order = settings.provider_display_order_names();

    let by_id: std::collections::HashMap<String, &ProviderId> = ProviderId::all()
        .iter()
        .map(|p| (p.cli_name().to_string(), p))
        .collect();

    order
        .iter()
        .filter_map(|id| {
            by_id.get(id).and_then(|p| {
                let enabled = settings.enabled_providers.contains(id);
                // Soft-removed providers (upstream #2254) stay hidden unless already enabled.
                if p.is_deprecated() && !enabled {
                    return None;
                }
                Some(ProviderSummary {
                    id: id.clone(),
                    display_name: p.display_name().to_string(),
                    enabled,
                    // `order` is assigned below, over the emitted (post-filter) list,
                    // so deprecated gaps never leave holes in the display indices.
                    order: 0,
                })
            })
        })
        .enumerate()
        .map(|(idx, mut s)| {
            s.order = idx as u32;
            s
        })
        .collect()
}

#[tauri::command]
pub fn reorder_providers(
    app: tauri::AppHandle,
    ids: Vec<String>,
) -> Result<Vec<ProviderSummary>, String> {
    let mut settings = Settings::load();
    settings.provider_order = codexbar::settings::normalize_provider_order(&ids);
    settings.save().map_err(|e| e.to_string())?;
    crate::tray_bridge::refresh_tray_presentation(&app);
    // Notify open surfaces (tray flyout, pop-out window) so their provider grid
    // and cards re-render in the new order immediately after a drag-reorder.
    crate::events::emit_settings_changed(&app);
    Ok(build_provider_summaries(&settings))
}

// ── Per-provider usage source ─────────────────────────────────────────

pub(crate) fn provider_usage_source_lookup(
    settings: &Settings,
    provider_id: &str,
) -> Option<String> {
    parse_provider_arg(provider_id)
        .ok()
        .map(|id| settings.usage_source(id).to_string())
}

#[tauri::command]
pub fn set_provider_usage_source(provider_id: String, source: String) -> Result<(), String> {
    let id = parse_provider_arg(&provider_id)?;
    let mode = SourceMode::parse(source.trim())
        .ok_or_else(|| format!("Invalid usage source '{source}' for provider '{provider_id}'"))?;
    let provider = instantiate_provider(id);
    if !provider.available_sources().contains(&mode) {
        return Err(format!(
            "Usage source '{source}' is unavailable for provider '{provider_id}'"
        ));
    }
    let value = match mode {
        SourceMode::Auto => "auto",
        SourceMode::Cli => "cli",
        SourceMode::OAuth => "oauth",
        SourceMode::Web => "web",
    };
    let mut settings = Settings::load();
    settings.set_usage_source(id, value);
    settings.save().map_err(|e| e.to_string())
}

// ── OpenRouter Management API key ────────────────────────────────────

#[tauri::command]
pub fn has_openrouter_management_api_key() -> bool {
    Settings::load()
        .management_api_token(ProviderId::OpenRouter)
        .is_some()
}

#[tauri::command]
pub fn set_openrouter_management_api_key(api_key: String) -> Result<(), String> {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return Err("Management API key must not be empty".to_string());
    }
    validate_single_line_secret(trimmed, "Management API key", MAX_API_KEY_LEN)?;
    let mut settings = Settings::load();
    settings.set_management_api_token(ProviderId::OpenRouter, Some(trimmed.to_string()));
    settings.save().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn remove_openrouter_management_api_key() -> Result<(), String> {
    let mut settings = Settings::load();
    settings.set_management_api_token(ProviderId::OpenRouter, None);
    settings.save().map_err(|error| error.to_string())
}

// ── Per-provider cookie source + region ───────────────────────────────

/// Map a CLI-name string to a `ProviderId` whose cookie source is exposed in
/// the UI. Returns `None` for providers without a user-facing cookie source.
fn cookie_source_provider(provider_id: &str) -> Option<codexbar::core::ProviderId> {
    use codexbar::core::ProviderId;
    Some(match provider_id {
        "codex" => ProviderId::Codex,
        "claude" => ProviderId::Claude,
        "cursor" => ProviderId::Cursor,
        "opencode" => ProviderId::OpenCode,
        "factory" => ProviderId::Factory,
        "alibaba" => ProviderId::Alibaba,
        "alibabatokenplan" => ProviderId::AlibabaTokenPlan,
        "kimi" | "kimik2" => ProviderId::Kimi,
        "minimax" => ProviderId::MiniMax,
        "augment" => ProviderId::Augment,
        "amp" => ProviderId::Amp,
        "ollama" => ProviderId::Ollama,
        "mistral" => ProviderId::Mistral,
        "qoder" => ProviderId::Qoder,
        "codebuddy" => ProviderId::CodeBuddy,
        "sakana" => ProviderId::Sakana,
        "notion" => ProviderId::Notion,
        "grok" => ProviderId::Grok,
        _ => return None,
    })
}

pub(crate) fn provider_cookie_source_lookup(
    settings: &Settings,
    provider_id: &str,
) -> Option<String> {
    cookie_source_provider(provider_id).map(|id| settings.cookie_source(id).to_string())
}

pub(crate) fn provider_cookie_source_set(
    settings: &mut Settings,
    provider_id: &str,
    source: String,
) -> Result<(), String> {
    let id = cookie_source_provider(provider_id)
        .ok_or_else(|| format!("Provider '{provider_id}' does not expose a cookie source"))?;
    settings.set_cookie_source(id, source);
    Ok(())
}

#[tauri::command]
pub fn set_provider_cookie_source(provider_id: String, source: String) -> Result<(), String> {
    let source = source.trim();
    if source.is_empty()
        || !cookie_source_options_for(&provider_id, Language::English)
            .iter()
            .any(|option| option.value == source)
    {
        return Err(format!(
            "Invalid cookie source '{source}' for provider '{provider_id}'"
        ));
    }
    let mut settings = Settings::load();
    provider_cookie_source_set(&mut settings, &provider_id, source.to_string())?;
    settings.save().map_err(|e| e.to_string())
}

fn region_provider(provider_id: &str) -> Option<codexbar::core::ProviderId> {
    use codexbar::core::ProviderId;
    Some(match provider_id {
        "alibaba" => ProviderId::Alibaba,
        "alibabatokenplan" => ProviderId::AlibabaTokenPlan,
        "zai" => ProviderId::Zai,
        "minimax" => ProviderId::MiniMax,
        _ => return None,
    })
}

pub(crate) fn provider_region_lookup(settings: &Settings, provider_id: &str) -> Option<String> {
    region_provider(provider_id).map(|id| {
        if id == codexbar::core::ProviderId::MiniMax {
            codexbar::providers::MiniMaxProvider::region_from_settings(Some(
                settings.api_region(id),
            ))
            .settings_value()
            .to_string()
        } else {
            settings.api_region(id).to_string()
        }
    })
}

pub(crate) fn provider_region_set(
    settings: &mut Settings,
    provider_id: &str,
    region: String,
) -> Result<(), String> {
    let id = region_provider(provider_id)
        .ok_or_else(|| format!("Provider '{provider_id}' does not have a region picker"))?;
    settings.set_api_region(id, region);
    Ok(())
}

#[tauri::command]
pub fn set_provider_region(provider_id: String, region: String) -> Result<(), String> {
    let region = region.trim();
    if region.is_empty()
        || !region_options_for(&provider_id)
            .iter()
            .any(|option| option.value == region)
    {
        return Err(format!(
            "Invalid region '{region}' for provider '{provider_id}'"
        ));
    }
    let mut settings = Settings::load();
    provider_region_set(&mut settings, &provider_id, region.to_string())?;
    settings.save().map_err(|e| e.to_string())
}

fn workspace_provider(provider_id: &str) -> Option<codexbar::core::ProviderId> {
    use codexbar::core::ProviderId;
    Some(match provider_id {
        "openaiapi" => ProviderId::OpenAIApi,
        "litellm" => ProviderId::LiteLLM,
        "devin" => ProviderId::Devin,
        "opencodego" => ProviderId::OpenCodeGo,
        "zed" => ProviderId::Zed,
        "xai" => ProviderId::Xai,
        _ => return None,
    })
}

#[tauri::command]
pub fn set_provider_workspace_id(provider_id: String, workspace_id: String) -> Result<(), String> {
    let id = workspace_provider(&provider_id).ok_or_else(|| {
        format!("Provider '{provider_id}' does not expose a workspace/project id")
    })?;
    let workspace_id = codexbar::settings::validate_provider_workspace_value(id, &workspace_id)?;
    let mut settings = Settings::load();
    prevent_litellm_key_retargeting(id, settings.workspace_id(id), &workspace_id)?;
    settings.set_workspace_id(id, workspace_id);
    settings.save().map_err(|e| e.to_string())
}

fn prevent_litellm_key_retargeting(
    id: codexbar::core::ProviderId,
    current_workspace_id: &str,
    next_workspace_id: &str,
) -> Result<(), String> {
    litellm_workspace_change_allowed(
        id,
        current_workspace_id,
        next_workspace_id,
        ApiKeys::load().has_key(id.cli_name()),
    )
}

fn litellm_workspace_change_allowed(
    id: codexbar::core::ProviderId,
    current_workspace_id: &str,
    next_workspace_id: &str,
    has_saved_api_key: bool,
) -> Result<(), String> {
    if id != codexbar::core::ProviderId::LiteLLM || next_workspace_id.trim().is_empty() {
        return Ok(());
    }

    let current = current_workspace_id.trim().trim_end_matches('/');
    let next = next_workspace_id.trim().trim_end_matches('/');
    if current.eq_ignore_ascii_case(next) || !has_saved_api_key {
        return Ok(());
    }

    Err(
        "Remove the saved LiteLLM API key before changing the LiteLLM base URL, then save the key again for the new endpoint."
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use codexbar::core::ProviderId;

    use super::{litellm_workspace_change_allowed, workspace_provider};

    #[test]
    fn maps_opencode_go_workspace_provider() {
        assert_eq!(
            workspace_provider("opencodego"),
            Some(ProviderId::OpenCodeGo)
        );
    }

    #[test]
    fn litellm_endpoint_change_requires_reentering_saved_key() {
        assert!(
            litellm_workspace_change_allowed(
                ProviderId::LiteLLM,
                "https://old.example.com",
                "https://new.example.com",
                true,
            )
            .is_err()
        );
        assert!(
            litellm_workspace_change_allowed(
                ProviderId::LiteLLM,
                "https://old.example.com",
                "https://old.example.com/",
                true,
            )
            .is_ok()
        );
        assert!(
            litellm_workspace_change_allowed(
                ProviderId::LiteLLM,
                "https://old.example.com",
                "",
                true,
            )
            .is_ok()
        );
        assert!(
            litellm_workspace_change_allowed(
                ProviderId::LiteLLM,
                "https://old.example.com",
                "https://new.example.com",
                false,
            )
            .is_ok()
        );
    }
}

#[tauri::command]
pub fn get_provider_workspace_id(provider_id: String) -> Result<Option<String>, String> {
    let Some(id) = workspace_provider(&provider_id) else {
        return Ok(None);
    };
    let value = Settings::load().workspace_id(id).trim().to_string();
    Ok((!value.is_empty()).then_some(value))
}

fn gateway_provider(provider_id: &str) -> Option<codexbar::core::ProviderId> {
    (provider_id == "wayfinder").then_some(codexbar::core::ProviderId::Wayfinder)
}

#[tauri::command]
pub fn set_provider_gateway_url(provider_id: String, gateway_url: String) -> Result<(), String> {
    let id = gateway_provider(&provider_id)
        .ok_or_else(|| format!("Provider '{provider_id}' does not expose a gateway URL"))?;
    let gateway_url = gateway_url.trim();
    codexbar::providers::wayfinder::parse_gateway_url(gateway_url)
        .map_err(|error| error.to_string())?;

    let mut settings = Settings::load();
    settings.set_gateway_url(id, gateway_url.to_string());
    settings.save().map_err(|error| error.to_string())
}

// ── Phase 6c — cookie source & region option catalogs ────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CookieSourceOption {
    pub value: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RegionOption {
    pub value: String,
    pub label: String,
}

fn cookie_option(
    lang: Language,
    value: &str,
    auto_desc: impl Into<String>,
    manual_desc: impl Into<String>,
    off_desc: Option<&str>,
) -> CookieSourceOption {
    let (label, description) = match value {
        "auto" => (
            locale::get_text(lang, locale::LocaleKey::Automatic),
            auto_desc.into(),
        ),
        "manual" => (
            locale::get_text(lang, locale::LocaleKey::CookieSourceManual),
            manual_desc.into(),
        ),
        "off" => (
            locale::get_text(lang, locale::LocaleKey::ProviderDisabled),
            off_desc.unwrap_or("").to_string(),
        ),
        other => (other.to_string(), String::new()),
    };
    CookieSourceOption {
        value: value.to_string(),
        label,
        description: if description.is_empty() {
            None
        } else {
            Some(description)
        },
    }
}

/// Returns the catalog of cookie source options for a given provider,
/// mirroring the `egui` ComboBox choices in `preferences.rs`.
/// Empty vec means the provider does not expose a cookie-source picker.
pub fn cookie_source_options_for(provider_id: &str, lang: Language) -> Vec<CookieSourceOption> {
    match provider_id {
        "codex" => vec![
            cookie_option(
                lang,
                "auto",
                locale::get_text(lang, locale::LocaleKey::ProviderCodexAutoImportHelp),
                "Paste a Cookie header from a chatgpt.com request.",
                Some("Disable OpenAI dashboard cookie usage."),
            ),
            cookie_option(
                lang,
                "manual",
                "",
                "Paste a Cookie header from a chatgpt.com request.",
                None,
            ),
            cookie_option(
                lang,
                "off",
                "",
                "",
                Some("Disable OpenAI dashboard cookie usage."),
            ),
        ],
        "claude" => vec![
            cookie_option(
                lang,
                "auto",
                locale::get_text(lang, locale::LocaleKey::ProviderClaudeCookiesHelp),
                "",
                None,
            ),
            cookie_option(
                lang,
                "manual",
                "",
                locale::get_text(lang, locale::LocaleKey::ProviderClaudeCookiesHelp),
                None,
            ),
        ],
        "cursor" => vec![
            cookie_option(
                lang,
                "auto",
                locale::get_text(lang, locale::LocaleKey::ProviderCursorCookieSourceHelp),
                "",
                None,
            ),
            cookie_option(
                lang,
                "manual",
                "",
                "Paste a Cookie header from a cursor.com request.",
                None,
            ),
        ],
        "grok" => vec![
            cookie_option(
                lang,
                "auto",
                "Automatic imports grok.com browser cookies.",
                "Paste a Cookie header from a grok.com request.",
                Some("Grok browser cookies are disabled."),
            ),
            cookie_option(
                lang,
                "manual",
                "",
                "Paste a Cookie header from a grok.com request.",
                None,
            ),
            cookie_option(
                lang,
                "off",
                "",
                "",
                Some("Grok browser cookies are disabled."),
            ),
        ],
        "opencode" => vec![
            cookie_option(
                lang,
                "auto",
                "Automatic imports browser cookies from opencode.ai.",
                "",
                None,
            ),
            cookie_option(
                lang,
                "manual",
                "",
                "Paste a Cookie header from the billing page.",
                None,
            ),
        ],
        "factory" => vec![
            cookie_option(
                lang,
                "auto",
                "Automatic imports browser cookies and WorkOS sessions.",
                "",
                None,
            ),
            cookie_option(
                lang,
                "manual",
                "",
                "Paste a Cookie header from Factory.",
                None,
            ),
        ],
        "alibaba" | "alibabatokenplan" => vec![
            cookie_option(
                lang,
                "auto",
                "Automatic imports browser cookies from Model Studio / Bailian.",
                "",
                None,
            ),
            cookie_option(
                lang,
                "manual",
                "",
                "Paste a Cookie header from Model Studio or Bailian.",
                None,
            ),
        ],
        "kimi" | "kimik2" => vec![
            cookie_option(lang, "auto", "Automatic imports browser cookies.", "", None),
            cookie_option(
                lang,
                "manual",
                "",
                "Paste a cookie header or the kimi-auth token value.",
                None,
            ),
            cookie_option(lang, "off", "", "", Some("Kimi cookies are disabled.")),
        ],
        "minimax" => vec![
            cookie_option(
                lang,
                "auto",
                "Automatic imports browser cookies and Coding Plan tokens.",
                "",
                None,
            ),
            cookie_option(
                lang,
                "manual",
                "",
                "Paste a Cookie header from the Coding Plan page.",
                None,
            ),
        ],
        "augment" => vec![
            cookie_option(lang, "auto", "Automatic imports browser cookies.", "", None),
            cookie_option(
                lang,
                "manual",
                "",
                "Paste a Cookie header from the Augment dashboard.",
                None,
            ),
        ],
        "amp" => vec![
            cookie_option(lang, "auto", "Automatic imports browser cookies.", "", None),
            cookie_option(
                lang,
                "manual",
                "",
                "Paste a Cookie header from Amp settings.",
                None,
            ),
        ],
        "ollama" => vec![
            cookie_option(lang, "auto", "Automatic imports browser cookies.", "", None),
            cookie_option(
                lang,
                "manual",
                "",
                "Paste a Cookie header from Ollama settings.",
                None,
            ),
        ],
        "mistral" => vec![
            cookie_option(
                lang,
                "auto",
                "Automatic imports browser cookies from Mistral Admin.",
                "",
                None,
            ),
            cookie_option(
                lang,
                "manual",
                "",
                "Paste a Cookie header from admin.mistral.ai.",
                None,
            ),
        ],
        "notion" => vec![
            cookie_option(
                lang,
                "auto",
                "Automatically imports the browser session cookie.",
                "",
                None,
            ),
            cookie_option(
                lang,
                "manual",
                "",
                "Paste a full cookie header or the token_v2 value.",
                None,
            ),
            cookie_option(lang, "off", "", "", Some("Notion cookies are disabled.")),
        ],
        _ => Vec::new(),
    }
}

/// Returns the API region options for a given provider.
/// Empty vec means the provider has no region picker.
pub fn region_options_for(provider_id: &str) -> Vec<RegionOption> {
    match provider_id {
        "alibaba" => codexbar::providers::AlibabaRegion::ALL
            .iter()
            .map(|region| RegionOption {
                value: region.settings_value().to_string(),
                label: region.display_name().to_string(),
            })
            .collect(),
        "zai" => vec![
            RegionOption {
                value: "global".to_string(),
                label: "Global".to_string(),
            },
            RegionOption {
                value: "china".to_string(),
                label: "China Mainland (BigModel)".to_string(),
            },
        ],
        "minimax" => vec![
            RegionOption {
                value: "global".to_string(),
                label: codexbar::providers::MiniMaxRegion::Global
                    .display_name()
                    .to_string(),
            },
            RegionOption {
                value: "cn".to_string(),
                label: codexbar::providers::MiniMaxRegion::ChinaMainland
                    .display_name()
                    .to_string(),
            },
        ],
        "alibabatokenplan" => codexbar::providers::AlibabaTokenPlanRegion::ALL
            .iter()
            .copied()
            .map(|region| RegionOption {
                value: region.as_str().to_string(),
                label: region.display_name().to_string(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[tauri::command]
pub fn get_provider_cookie_source_options(
    provider_id: String,
) -> Result<Vec<CookieSourceOption>, String> {
    let lang = Settings::load().ui_language;
    Ok(cookie_source_options_for(&provider_id, lang))
}

#[tauri::command]
pub fn get_provider_region_options(provider_id: String) -> Result<Vec<RegionOption>, String> {
    Ok(region_options_for(&provider_id))
}
