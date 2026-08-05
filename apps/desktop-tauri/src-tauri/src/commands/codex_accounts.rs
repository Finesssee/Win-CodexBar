use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use uuid::Uuid;

use codexbar::codex_accounts::{
    AccountStore, CodexAccount, CodexAccountApi, CodexAccountManager, CodexAccountManagerError,
    CodexApiError, CodexSwitchResult, SnapshotStore, restart_codex_desktop,
};

use crate::state::AppState;

use super::*;

// ── Codex multi-account (ADR 0003, milestone 2) ──────────────────────

const DEFAULT_FETCH_TIMEOUT_SECONDS: u64 = 60;

/// All stored + discovered Codex accounts, with the stored list preferred.
pub(crate) fn load_codex_accounts() -> Result<Vec<CodexAccount>, String> {
    let store = AccountStore::new();
    let existing = store.load_accounts().map_err(|e| e.to_string())?;

    let manager = CodexAccountManager::new();
    let managed = manager
        .discover_managed_accounts(&existing)
        .map_err(|e| e.to_string())?;
    let ambient = manager.discover_ambient_account(&existing);

    let mut merged: Vec<CodexAccount> = managed.clone();
    if let Some(ambient) = ambient {
        if let Some(entry) = merged.iter_mut().find(|account| account.matches(&ambient)) {
            entry.merge_from(&ambient);
        } else {
            merged.push(ambient);
        }
    }

    // Reconcile persisted metadata (nickname, stored timestamps) for managed homes.
    let mut reconciled: Vec<CodexAccount> = existing
        .iter()
        .map(|account| {
            let mut account = account.clone();
            if let Some(fresh) = managed.iter().find(|fresh| fresh.matches(&account)) {
                account.merge_from(fresh);
            }
            account
        })
        .collect();

    // Add any newly discovered accounts that are not yet persisted.
    for candidate in &merged {
        if !reconciled.iter().any(|account| account.matches(candidate)) {
            reconciled.push(candidate.clone());
        }
    }

    Ok(reconciled)
}

/// Persist the given accounts to the account store.
pub(crate) fn persist_codex_accounts(accounts: &[CodexAccount]) -> Result<(), String> {
    let store = AccountStore::new();
    let (_existing, removed) = store.load().map_err(|e| e.to_string())?;
    store
        .save(accounts, Some(&removed))
        .map_err(|e| e.to_string())
}

/// Refresh quota snapshots for every Codex account (ADR 0003 multi-account
/// lanes).
///
/// Runs on the same refresh cycle as the ambient Codex provider lane: each
/// account (ambient + managed) is fetched concurrently, bounded by the shared
/// provider fetch semaphore, and persisted to the account snapshot store. A
/// `codex-accounts-updated` event lets surfaces (Settings accounts panel)
/// re-read the store without manual fetch.
///
/// Failures are per-account and non-fatal: the ambient provider snapshot and
/// the on-demand `codex_account_fetch` command remain authoritative, and the
/// store keeps the last good snapshot per account.
pub(crate) async fn refresh_codex_account_lanes(
    app: tauri::AppHandle,
    fetch_permits: Arc<tokio::sync::Semaphore>,
) {
    let accounts = match load_codex_accounts() {
        Ok(accounts) => accounts,
        Err(e) => {
            tracing::warn!("codex account lanes: failed to load accounts: {e}");
            return;
        }
    };
    if accounts.is_empty() {
        return;
    }

    let mut handles = Vec::with_capacity(accounts.len());
    for account in accounts {
        let permits = Arc::clone(&fetch_permits);
        handles.push(tokio::spawn(async move {
            let Ok(_permit) = permits.acquire_owned().await else {
                return None;
            };
            let api = CodexAccountApi::new();
            let home_path = account.codex_home_path.clone();
            let email_hint = account.email_hint.clone();
            match tokio::time::timeout(
                std::time::Duration::from_secs(DEFAULT_FETCH_TIMEOUT_SECONDS),
                api.fetch_snapshot(&home_path, email_hint.as_deref(), true),
            )
            .await
            {
                Ok(Ok(snapshot)) => Some((account.id, snapshot)),
                Ok(Err(e)) => {
                    tracing::debug!(
                        "codex account lane {} failed: {}",
                        account.id,
                        into_api_message(e)
                    );
                    None
                }
                Err(_) => {
                    tracing::debug!("codex account lane {} timed out", account.id);
                    None
                }
            }
        }));
    }

    let mut snapshots = SnapshotStore::new().load().unwrap_or_default();
    for handle in handles {
        if let Ok(Some((id, snapshot))) = handle.await {
            snapshots.insert(id, snapshot);
        }
    }
    if let Err(e) = SnapshotStore::new().save(&snapshots) {
        tracing::warn!("codex account lanes: failed to persist snapshots: {e}");
    }
    events::emit_codex_accounts_updated(&app);
}

#[tauri::command]
pub fn codex_accounts_list() -> Result<Vec<CodexAccount>, String> {
    load_codex_accounts()
}

#[tauri::command]
pub async fn codex_account_add(app: tauri::AppHandle) -> Result<CodexAccount, String> {
    let manager = CodexAccountManager::new();
    let account = tauri::async_runtime::spawn_blocking(move || manager.add_managed_account(None))
        .await
        .map_err(|e| e.to_string())?
        .map_err(into_user_message)?;

    if let Err(e) = refresh_persisted_accounts(app) {
        tracing::error!("failed to persist accounts after add: {e}");
    }
    Ok(account)
}

#[tauri::command]
pub fn codex_account_remove(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let manager = CodexAccountManager::new();
    let accounts = load_codex_accounts()?;
    let target = accounts
        .iter()
        .find(|account| account.id.to_string() == id)
        .ok_or_else(|| "Codex account not found.".to_string())?;

    manager
        .remove_managed_files_if_owned(target)
        .map_err(into_user_message)?;

    let remaining: Vec<CodexAccount> = accounts
        .into_iter()
        .filter(|account| account.id.to_string() != id)
        .collect();
    persist_codex_accounts(&remaining)?;
    events::emit_settings_changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn codex_account_switch(
    app: tauri::AppHandle,
    id: String,
) -> Result<CodexSwitchResult, String> {
    let manager = CodexAccountManager::new();
    let accounts = load_codex_accounts()?;
    let target = accounts
        .iter()
        .find(|account| account.id.to_string() == id)
        .ok_or_else(|| "Codex account not found.".to_string())?
        .clone();

    let persisted = accounts.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        manager.switch_active_account(&target, &persisted)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(into_user_message)?;

    // Materialized ambient account may need persisting.
    if let Some(materialized) = &result.materialized_account {
        let mut accounts = load_codex_accounts()?;
        if let Some(entry) = accounts.iter_mut().find(|a| a.matches(materialized)) {
            entry.merge_from(materialized);
        } else {
            accounts.push(materialized.clone());
        }
        persist_codex_accounts(&accounts)?;
    }

    events::emit_settings_changed(&app);
    Ok(result)
}

#[tauri::command]
pub async fn codex_account_fetch(
    app: tauri::AppHandle,
    id: String,
) -> Result<codexbar::codex_accounts::AccountUsageSnapshot, String> {
    let accounts = load_codex_accounts()?;
    let target = accounts
        .iter()
        .find(|account| account.id.to_string() == id)
        .ok_or_else(|| "Codex account not found.".to_string())?
        .clone();

    let api = CodexAccountApi::new();
    let home_path = target.codex_home_path.clone();
    let email_hint = target.email_hint.clone();
    let snapshot = tokio::time::timeout(
        std::time::Duration::from_secs(DEFAULT_FETCH_TIMEOUT_SECONDS),
        api.fetch_snapshot(&home_path, email_hint.as_deref(), true),
    )
    .await
    .map_err(|_| "Timed out waiting for the Codex usage API.".to_string())?
    .map_err(into_api_message)?;

    // Persist snapshot to the snapshot store, keyed by account id.
    if let Ok(mut snapshots) = SnapshotStore::new().load() {
        snapshots.insert(target.id, snapshot.clone());
        let _ = SnapshotStore::new().save(&snapshots);
    }

    if refresh_persisted_accounts(app).is_err() {
        // Non-fatal: the snapshot was still fetched.
    }
    Ok(snapshot)
}

#[tauri::command]
pub fn codex_account_snapshots()
-> Result<HashMap<Uuid, codexbar::codex_accounts::AccountUsageSnapshot>, String> {
    SnapshotStore::new().load().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn codex_account_restart_desktop(
    _app: tauri::AppHandle,
    session_root: Option<String>,
    backup_destination: Option<String>,
    restore_source: Option<String>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let session_root = session_root.map(PathBuf::from);
        let backup_destination = backup_destination.map(PathBuf::from);
        let restore_source = restore_source.map(PathBuf::from);
        restart_codex_desktop(
            0.8,
            session_root.as_deref(),
            backup_destination.as_deref(),
            restore_source.as_deref(),
        )
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Merge discovered accounts back into the persisted list after identity
/// changes (login/switch) so the store reflects reality.
fn refresh_persisted_accounts(app: tauri::AppHandle) -> Result<(), String> {
    let accounts = load_codex_accounts()?;
    persist_codex_accounts(&accounts)?;
    events::emit_settings_changed(&app);
    Ok(())
}

fn into_user_message(error: CodexAccountManagerError) -> String {
    match error {
        CodexAccountManagerError::Message(msg) => msg,
        CodexAccountManagerError::Io(e) => e.to_string(),
    }
}

fn into_api_message(error: CodexApiError) -> String {
    match error {
        CodexApiError::Message(msg) => msg,
        CodexApiError::Network(e) => format!("network error: {e}"),
        CodexApiError::Parse(e) => format!("failed to parse Codex payload: {e}"),
    }
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexAccountsStateBridge {
    pub accounts: Vec<CodexAccount>,
    pub snapshots: HashMap<Uuid, codexbar::codex_accounts::AccountUsageSnapshot>,
}

#[tauri::command]
pub fn get_codex_accounts_state(
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<CodexAccountsStateBridge, String> {
    let _guard = state.lock().map_err(|e| e.to_string())?;
    Ok(CodexAccountsStateBridge {
        accounts: load_codex_accounts()?,
        snapshots: codex_account_snapshots()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_account() -> CodexAccount {
        CodexAccount::new(
            Uuid::new_v4(),
            None,
            Some("user@example.com".to_string()),
            Some("auth0|acct".to_string()),
            Some("acct".to_string()),
            std::path::PathBuf::from("/tmp/fake-home"),
            codexbar::codex_accounts::CodexAccountSource::ManagedByApp,
            codexbar::codex_accounts::utc_now(),
            codexbar::codex_accounts::utc_now(),
            Some(codexbar::codex_accounts::utc_now()),
        )
    }

    #[test]
    fn into_user_message_preserves_friendly_text() {
        assert_eq!(
            into_user_message(CodexAccountManagerError::Message(
                "The `codex` command could not be found.".to_string()
            )),
            "The `codex` command could not be found."
        );
    }

    #[test]
    fn sample_account_serializes_camel_case() {
        let json = serde_json::to_value(sample_account()).unwrap();
        assert!(json.get("codexHomePath").is_some());
        assert!(json.get("providerAccountId").is_some());
    }
}
