//! Upstream 0.53 Usage & Spend accounting bridge.

use codexbar::spend_contract::{SpendContract, build_local_spend_contract};

#[tauri::command]
pub async fn get_spend_contract(
    provider_id: String,
    history_days: Option<u32>,
    include_open_codex: Option<bool>,
) -> Result<SpendContract, String> {
    let provider = provider_id.trim().to_ascii_lowercase();
    if !matches!(provider.as_str(), "codex" | "claude" | "opencodego") {
        return Err(format!("Spend contract is unavailable for provider: {provider}"));
    }
    let days = history_days.unwrap_or(30);
    let include_import = include_open_codex.unwrap_or(false) && provider == "codex";
    tauri::async_runtime::spawn_blocking(move || {
        build_local_spend_contract(&provider, days, include_import)
    })
    .await
    .map_err(|error| format!("spend contract worker failed: {error}"))
}
