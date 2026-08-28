//! Codex account management: discovery, authentication, switching, removal.
//!
//! Port of `windows/.../account_manager.py` (MIT). Manages isolated managed
//! homes under `managed-homes/`, discovers the ambient `~/.codex` identity, and
//! switches the active identity by swapping `auth.json` into the ambient home,
//! rewriting the Codex Desktop `creator_id` global state and backing up/restoring
//! the desktop session.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use thiserror::Error;
use uuid::Uuid;

use super::api::{AuthBackedIdentity, CodexApiError, load_identity};
use super::file_locations::{
    ambient_codex_home, auth_backups_directory, codex_desktop_session_root,
    desktop_session_snapshot_path, ensure_directories, managed_homes_directory,
};
use super::login_runner::{CodexLoginOutcome, CodexLoginRunner, ManagedLoginProcess};
use super::models::{CodexAccount, CodexAccountSource, utc_now};

/// Friendly account manager error.
#[derive(Debug, Error)]
pub enum CodexAccountManagerError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<CodexApiError> for CodexAccountManagerError {
    fn from(value: CodexApiError) -> Self {
        CodexAccountManagerError::Message(value.to_string())
    }
}

/// Result of switching the active account.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSwitchResult {
    pub materialized_account: Option<CodexAccount>,
    pub backup_path: Option<PathBuf>,
    pub ambient_account: Option<CodexAccount>,
    pub desktop_session_backup_path: Option<PathBuf>,
    pub desktop_session_restore_path: Option<PathBuf>,
    pub desktop_session_restore_exists: bool,
}

/// Discovers, authenticates and switches Codex accounts.
#[derive(Debug, Default)]
pub struct CodexAccountManager;

impl CodexAccountManager {
    pub fn new() -> Self {
        Self
    }

    /// Start a `codex login` into a fresh managed home.
    pub fn add_managed_account(
        &self,
        handle: Option<&ManagedLoginProcess>,
    ) -> Result<CodexAccount, CodexAccountManagerError> {
        ensure_directories()?;
        let home_path = managed_homes_directory().join(Uuid::new_v4().to_string());
        fs::create_dir_all(&home_path)?;

        match self.authenticate_account(&home_path, CodexAccountSource::ManagedByApp, None, handle)
        {
            Ok(account) => Ok(account),
            Err(error) => {
                // Best-effort teardown of the fresh managed home on failure;
                // a removal error cannot change the authentication outcome.
                let _removed_home = fs::remove_dir_all(&home_path);
                Err(error)
            }
        }
    }

    /// Re-run `codex login` for an existing account.
    pub fn reauthenticate(
        &self,
        account: &CodexAccount,
        handle: Option<&ManagedLoginProcess>,
    ) -> Result<CodexAccount, CodexAccountManagerError> {
        self.authenticate_account(
            &account.codex_home_path,
            account.source,
            Some(account),
            handle,
        )
    }

    /// Remove app-owned managed homes matching this account.
    pub fn remove_managed_files_if_owned(
        &self,
        account: &CodexAccount,
    ) -> Result<(), CodexAccountManagerError> {
        if !account.source.owns_files() {
            return Ok(());
        }

        let root = fs::canonicalize(managed_homes_directory())
            .unwrap_or_else(|_| managed_homes_directory());
        let targets = self.managed_home_paths_matching(account)?;

        for target in targets {
            let resolved = fs::canonicalize(&target).unwrap_or_else(|_| target.clone());
            let relative = resolved.strip_prefix(&root).map_err(|_| {
                CodexAccountManagerError::Message(
                    "This path is not an app-managed home directory.".to_string(),
                )
            })?;
            if relative.as_os_str().is_empty() {
                return Err(CodexAccountManagerError::Message(
                    "Refusing to remove the managed-homes root.".to_string(),
                ));
            }
            if target.exists() {
                fs::remove_dir_all(&target)?;
            }
        }
        Ok(())
    }

    /// Discover managed homes and merge them against the stored accounts.
    pub fn discover_managed_accounts(
        &self,
        existing: &[CodexAccount],
    ) -> Result<Vec<CodexAccount>, CodexAccountManagerError> {
        ensure_directories()?;
        let mut discovered = Vec::new();
        let mut entries: Vec<PathBuf> = fs::read_dir(managed_homes_directory())?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_dir())
            .collect();
        entries.sort_by_key(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_lowercase())
                .unwrap_or_default()
        });
        for home_path in entries {
            if let Some(account) = self.discovered_managed_account(&home_path, existing) {
                discovered.push(account);
            }
        }
        Ok(discovered)
    }

    /// Discover the ambient `~/.codex` account.
    pub fn discover_ambient_account(&self, existing: &[CodexAccount]) -> Option<CodexAccount> {
        let home_path = ambient_codex_home();
        let auth_path = home_path.join("auth.json");
        if !home_path.is_dir() || !auth_path.exists() {
            return None;
        }
        let identity = load_identity(&home_path).ok()?;
        if identity.email.is_none() && identity.provider_account_id.is_none() {
            return None;
        }

        let candidate =
            candidate_account(identity.clone(), &home_path, CodexAccountSource::Ambient);
        let matched = existing.iter().find(|account| candidate.matches(account));
        let discovered_at = directory_timestamp(&home_path);
        Some(build_discovered_account(
            matched,
            identity,
            home_path,
            CodexAccountSource::Ambient,
            discovered_at,
        ))
    }

    /// Identity of the currently active (ambient) account, if any.
    pub fn load_active_identity(&self) -> Option<AuthBackedIdentity> {
        let auth_path = ambient_codex_home().join("auth.json");
        if !auth_path.exists() {
            return None;
        }
        load_identity(&ambient_codex_home()).ok()
    }

    /// Switch the ambient identity to `target`, materializing the previous
    /// ambient account as managed and preserving the desktop session.
    pub fn switch_active_account(
        &self,
        target: &CodexAccount,
        existing: &[CodexAccount],
    ) -> Result<CodexSwitchResult, CodexAccountManagerError> {
        ensure_directories()?;

        let target_auth_path = target.codex_home_path.join("auth.json");
        if !target_auth_path.exists() {
            return Err(CodexAccountManagerError::Message(
                "The selected account does not contain `auth.json`.".to_string(),
            ));
        }

        let ambient_account = self.discover_ambient_account(existing);
        let session_root = codex_desktop_session_root();
        let mut materialized_account: Option<CodexAccount> = None;
        if let Some(ambient) = &ambient_account {
            let is_ambient = ambient.source == CodexAccountSource::Ambient;
            if is_ambient && !ambient.matches(target) {
                materialized_account = Some(self.materialize_as_managed(ambient)?);
            }
        }

        let mut desktop_session_backup_path: Option<PathBuf> = None;
        let mut desktop_session_restore_path: Option<PathBuf> = None;
        let mut desktop_session_restore_exists = false;
        if session_root.is_some() {
            if let Some(materialized) = &materialized_account {
                desktop_session_backup_path =
                    Some(desktop_session_snapshot_path(&materialized.codex_home_path));
            }
            let snapshot_path = desktop_session_snapshot_path(&target.codex_home_path);
            desktop_session_restore_path = Some(snapshot_path.clone());
            desktop_session_restore_exists = path_has_children(&snapshot_path);
        }

        fs::create_dir_all(ambient_codex_home())?;
        let backup_path = self.backup_ambient_auth()?;
        fs::copy(&target_auth_path, ambient_codex_home().join("auth.json"))?;
        self.sync_ambient_global_state(
            ambient_account
                .as_ref()
                .and_then(|account| account.provider_account_id.clone()),
            self.target_account_id(target)?,
        );

        Ok(CodexSwitchResult {
            materialized_account,
            backup_path,
            ambient_account: self.discover_ambient_account(existing),
            desktop_session_backup_path,
            desktop_session_restore_path,
            desktop_session_restore_exists,
        })
    }

    /// Copy the ambient account into an app-managed home.
    pub fn materialize_as_managed(
        &self,
        account: &CodexAccount,
    ) -> Result<CodexAccount, CodexAccountManagerError> {
        ensure_directories()?;

        let source_auth_path = account.codex_home_path.join("auth.json");
        if !source_auth_path.exists() {
            return Err(CodexAccountManagerError::Message(
                "The current active account does not contain `auth.json`.".to_string(),
            ));
        }

        let destination_home = managed_homes_directory().join(Uuid::new_v4().to_string());
        fs::create_dir_all(&destination_home)?;
        fs::copy(&source_auth_path, destination_home.join("auth.json"))?;

        let now = utc_now();
        Ok(CodexAccount::new(
            account.id,
            account.nickname.clone(),
            account.email_hint.clone(),
            account.auth_subject.clone(),
            account.provider_account_id.clone(),
            destination_home,
            CodexAccountSource::ManagedByApp,
            account.created_at,
            now,
            Some(account.last_authenticated_at.unwrap_or(now)),
        ))
    }

    fn backup_ambient_auth(&self) -> Result<Option<PathBuf>, CodexAccountManagerError> {
        ensure_directories()?;
        let auth_path = ambient_codex_home().join("auth.json");
        if !auth_path.exists() {
            return Ok(None);
        }
        let backup_path =
            auth_backups_directory().join(format!("ambient-auth-{}.json", timestamp_slug()));
        fs::copy(&auth_path, &backup_path)?;
        Ok(Some(backup_path))
    }

    fn target_account_id(&self, target: &CodexAccount) -> Result<Option<String>, CodexApiError> {
        if let Some(account_id) = &target.provider_account_id {
            return Ok(Some(account_id.clone()));
        }
        let identity = load_identity(&target.codex_home_path)?;
        Ok(identity.provider_account_id)
    }

    fn sync_ambient_global_state(
        &self,
        previous_account_id: Option<String>,
        target_account_id: Option<String>,
    ) {
        let Some(target_account_id) = target_account_id else {
            return;
        };
        for file_name in [".codex-global-state.json", ".codex-global-state.json.bak"] {
            self.rewrite_creator_id(
                &ambient_codex_home().join(file_name),
                previous_account_id.as_deref(),
                &target_account_id,
            );
        }
    }

    fn rewrite_creator_id(
        &self,
        path: &Path,
        previous_account_id: Option<&str>,
        target_account_id: &str,
    ) {
        if !path.exists() {
            return;
        }
        let Ok(content) = fs::read_to_string(path) else {
            return;
        };
        let Ok(mut payload) = serde_json::from_str::<serde_json::Value>(&content) else {
            return;
        };
        if !payload.is_object() {
            return;
        }
        let Some(atom_state) = payload
            .get_mut("electron-persisted-atom-state")
            .and_then(|value| value.as_object_mut())
        else {
            return;
        };
        let Some(environment) = atom_state
            .get_mut("environment")
            .and_then(|value| value.as_object_mut())
        else {
            return;
        };
        let Some(creator_id) = environment.get("creator_id") else {
            return;
        };
        let Some(updated) =
            updated_creator_id(creator_id.as_str(), previous_account_id, target_account_id)
        else {
            return;
        };
        if updated == creator_id.as_str().unwrap_or_default() {
            return;
        }
        environment.insert("creator_id".to_string(), serde_json::Value::String(updated));
        let Ok(encoded) = serde_json::to_string_pretty(&payload) else {
            return;
        };
        // Best-effort teardown: persisting the rewritten payload is advisory
        // and a write error cannot change the in-memory rewrite result.
        let _written_payload = fs::write(path, format!("{encoded}\n"));
    }

    fn managed_home_paths_matching(
        &self,
        account: &CodexAccount,
    ) -> Result<Vec<PathBuf>, CodexAccountManagerError> {
        ensure_directories()?;
        let mut targets: Vec<PathBuf> = vec![
            std::path::absolute(&account.codex_home_path)
                .unwrap_or_else(|_| account.codex_home_path.clone()),
        ];
        let mut seen_keys: std::collections::HashSet<String> =
            [managed_home_key(targets[0].as_path())]
                .into_iter()
                .collect();

        for entry in fs::read_dir(managed_homes_directory())? {
            let Ok(entry) = entry else {
                continue;
            };
            let home_path = entry.path();
            if !home_path.is_dir() {
                continue;
            }
            let Some(candidate) =
                self.discovered_managed_account(&home_path, std::slice::from_ref(account))
            else {
                continue;
            };
            if !candidate.matches(account) {
                continue;
            }
            let resolved = std::path::absolute(&home_path).unwrap_or_else(|_| home_path.clone());
            let key = managed_home_key(resolved.as_path());
            if seen_keys.contains(&key) {
                continue;
            }
            targets.push(resolved);
            seen_keys.insert(key);
        }
        Ok(targets)
    }

    fn authenticate_account(
        &self,
        home_path: &Path,
        source: CodexAccountSource,
        existing: Option<&CodexAccount>,
        handle: Option<&ManagedLoginProcess>,
    ) -> Result<CodexAccount, CodexAccountManagerError> {
        let result = CodexLoginRunner::run(home_path, Duration::from_secs(180), handle);

        match &result.outcome {
            CodexLoginOutcome::Cancelled => {
                return Err(CodexAccountManagerError::Message(
                    "Account setup cancelled.".to_string(),
                ));
            }
            CodexLoginOutcome::MissingBinary => {
                return Err(CodexAccountManagerError::Message(
                    "The `codex` command could not be found.".to_string(),
                ));
            }
            CodexLoginOutcome::TimedOut(_) => {
                return Err(CodexAccountManagerError::Message(
                    "The Codex sign-in flow timed out.".to_string(),
                ));
            }
            CodexLoginOutcome::LaunchFailed(output) => {
                return Err(CodexAccountManagerError::Message(format!(
                    "Failed to start the Codex sign-in flow: {output}"
                )));
            }
            CodexLoginOutcome::Failed(output) => {
                return Err(CodexAccountManagerError::Message(format!(
                    "The Codex sign-in flow did not complete.\n{output}"
                )));
            }
            CodexLoginOutcome::Success(_) => {}
        }

        let identity = load_identity(home_path)?;
        if identity.email.is_none() && identity.provider_account_id.is_none() {
            return Err(CodexAccountManagerError::Message(
                "Sign-in completed, but the account identity could not be read.".to_string(),
            ));
        }

        let now = utc_now();
        Ok(CodexAccount::new(
            existing
                .map(|account| account.id)
                .unwrap_or_else(Uuid::new_v4),
            existing.and_then(|account| account.nickname.clone()),
            identity
                .email
                .or_else(|| existing.and_then(|account| account.email_hint.clone())),
            identity
                .auth_subject
                .or_else(|| existing.and_then(|account| account.auth_subject.clone())),
            identity
                .provider_account_id
                .or_else(|| existing.and_then(|account| account.provider_account_id.clone())),
            home_path.to_path_buf(),
            source,
            existing.map(|account| account.created_at).unwrap_or(now),
            now,
            Some(now),
        ))
    }

    fn discovered_managed_account(
        &self,
        home_path: &Path,
        existing: &[CodexAccount],
    ) -> Option<CodexAccount> {
        if !home_path.is_dir() {
            return None;
        }
        let auth_path = home_path.join("auth.json");
        if !auth_path.exists() {
            return None;
        }
        let identity = load_identity(home_path).ok()?;
        if identity.email.is_none() && identity.provider_account_id.is_none() {
            return None;
        }

        let discovered_at = directory_timestamp(home_path);
        let candidate = candidate_account(
            identity.clone(),
            home_path,
            CodexAccountSource::ManagedByApp,
        );
        let matched = existing.iter().find(|account| candidate.matches(account));
        Some(build_discovered_account(
            matched,
            identity,
            home_path.to_path_buf(),
            CodexAccountSource::ManagedByApp,
            discovered_at,
        ))
    }
}

fn candidate_account(
    identity: AuthBackedIdentity,
    home_path: &Path,
    source: CodexAccountSource,
) -> CodexAccount {
    CodexAccount::new(
        Uuid::new_v4(),
        None,
        identity.email.clone(),
        identity.auth_subject.clone(),
        identity.provider_account_id.clone(),
        home_path.to_path_buf(),
        source,
        utc_now(),
        utc_now(),
        None,
    )
}

fn build_discovered_account(
    matched: Option<&CodexAccount>,
    identity: AuthBackedIdentity,
    home_path: PathBuf,
    source: CodexAccountSource,
    discovered_at: DateTime<Utc>,
) -> CodexAccount {
    CodexAccount::new(
        matched
            .map(|account| account.id)
            .unwrap_or_else(Uuid::new_v4),
        matched.and_then(|account| account.nickname.clone()),
        identity
            .email
            .or_else(|| matched.and_then(|account| account.email_hint.clone())),
        identity
            .auth_subject
            .or_else(|| matched.and_then(|account| account.auth_subject.clone())),
        identity
            .provider_account_id
            .or_else(|| matched.and_then(|account| account.provider_account_id.clone())),
        home_path,
        source,
        matched
            .map(|account| account.created_at)
            .unwrap_or(discovered_at),
        matched
            .map(|account| account.updated_at.max(discovered_at))
            .unwrap_or(discovered_at),
        matched
            .and_then(|account| account.last_authenticated_at)
            .or(Some(discovered_at)),
    )
}

fn directory_timestamp(path: &Path) -> DateTime<Utc> {
    let auth_path = path.join("auth.json");
    if auth_path.exists()
        && let Ok(metadata) = fs::metadata(&auth_path)
        && let Ok(modified) = metadata.modified()
    {
        return modified.into();
    }
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map(Into::into)
        .unwrap_or_else(|_| utc_now())
}

fn managed_home_key(path: &Path) -> String {
    std::path::absolute(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_lowercase()
}

fn path_has_children(path: &Path) -> bool {
    if !path.exists() || !path.is_dir() {
        return false;
    }
    fs::read_dir(path)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}

fn timestamp_slug() -> String {
    utc_now().format("%Y%m%d-%H%M%S").to_string()
}

/// Compute the replacement `creator_id` for the target account.
fn updated_creator_id(
    creator_id: Option<&str>,
    previous_account_id: Option<&str>,
    target_account_id: &str,
) -> Option<String> {
    let creator_id = creator_id?.trim();
    if creator_id.is_empty() {
        return None;
    }
    if creator_id == target_account_id || creator_id.ends_with(&format!("__{target_account_id}")) {
        return Some(creator_id.to_string());
    }
    if let Some(previous) = previous_account_id
        && creator_id.contains(previous)
    {
        return Some(creator_id.replace(previous, target_account_id));
    }
    if looks_like_uuid(creator_id) {
        return Some(target_account_id.to_string());
    }
    if let Some((prefix, suffix)) = creator_id.rsplit_once("__")
        && looks_like_uuid(suffix)
    {
        return Some(format!("{prefix}__{target_account_id}"));
    }
    None
}

fn looks_like_uuid(value: &str) -> bool {
    Uuid::parse_str(value.trim()).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    /// Write an auth.json carrying a JWT identity for the given account id.
    fn write_auth(home_path: &Path, email: &str, account_id: &str) {
        let payload = serde_json::json!({
            "email": email,
            "sub": format!("auth0|{account_id}"),
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": "team",
                "chatgpt_account_id": account_id,
            },
        });
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap());
        let auth_payload = serde_json::json!({
            "tokens": {
                "access_token": format!("access-{account_id}"),
                "refresh_token": format!("refresh-{account_id}"),
                "id_token": format!("header.{encoded}.signature"),
                "account_id": account_id,
            },
            "last_refresh": "2026-04-23T00:00:00Z",
        });
        std::fs::write(
            home_path.join("auth.json"),
            serde_json::to_vec_pretty(&auth_payload).unwrap(),
        )
        .unwrap();
    }

    fn make_account(home_path: PathBuf, email: &str, account_id: &str) -> CodexAccount {
        CodexAccount::new(
            Uuid::new_v4(),
            None,
            Some(email.to_string()),
            Some(format!("auth0|{account_id}")),
            Some(account_id.to_string()),
            home_path,
            CodexAccountSource::ManagedByApp,
            utc_now(),
            utc_now(),
            Some(utc_now()),
        )
    }

    #[test]
    fn remove_managed_account_removes_duplicate_homes_for_same_provider() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        super::super::file_locations::with_app_support_directory(root.to_path_buf());

        let account_id = "83c5ae92-f5ee-41f8-9528-199110d1d0f9";
        let first_home = root.join("managed-homes").join("first");
        let duplicate_home = root.join("managed-homes").join("duplicate");
        let other_home = root.join("managed-homes").join("other");
        for home in [&first_home, &duplicate_home, &other_home] {
            std::fs::create_dir_all(home).unwrap();
        }
        write_auth(&first_home, "user@example.com", account_id);
        write_auth(&duplicate_home, "user@example.com", account_id);
        write_auth(&other_home, "user@example.com", "different-provider");

        let account = make_account(first_home.clone(), "user@example.com", account_id);
        let manager = CodexAccountManager::new();
        manager.remove_managed_files_if_owned(&account).unwrap();

        assert!(!first_home.exists());
        assert!(!duplicate_home.exists());
        assert!(other_home.exists());

        super::super::file_locations::clear_app_support_directory_override();
    }

    #[test]
    fn switch_active_account_updates_global_state_creator_id() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        super::super::file_locations::with_app_support_directory(root.to_path_buf());

        let old_account_id = "1ea93d04-5c50-42e3-857b-3db850785967";
        let new_account_id = "83c5ae92-f5ee-41f8-9528-199110d1d0f9";

        let ambient_home = root.join(".codex");
        let target_home = root.join("managed-homes").join("target");
        let desktop_session_root = root.join("package-session");

        std::fs::create_dir_all(&ambient_home).unwrap();
        std::fs::create_dir_all(&target_home).unwrap();
        std::fs::create_dir_all(&desktop_session_root).unwrap();

        write_auth(&ambient_home, "old@example.com", old_account_id);
        write_auth(&target_home, "new@example.com", new_account_id);
        let target_session_dir = target_home.join("desktop-session").join("Network");
        std::fs::create_dir_all(&target_session_dir).unwrap();
        std::fs::write(target_session_dir.join("Cookies"), "cookie-data").unwrap();

        let global_state = serde_json::json!({
            "electron-persisted-atom-state": {
                "environment": {
                    "creator_id": format!("user-e9H3MsspGTF7UZJ8uaXuML55__{old_account_id}"),
                }
            }
        });
        for file_name in [".codex-global-state.json", ".codex-global-state.json.bak"] {
            std::fs::write(
                ambient_home.join(file_name),
                serde_json::to_vec_pretty(&global_state).unwrap(),
            )
            .unwrap();
        }

        super::super::file_locations::with_ambient_codex_home(ambient_home.clone());
        super::super::file_locations::with_codex_desktop_session_root(desktop_session_root.clone());

        let manager = CodexAccountManager::new();
        let target_account = make_account(target_home.clone(), "new@example.com", new_account_id);
        let result = manager
            .switch_active_account(&target_account, std::slice::from_ref(&target_account))
            .unwrap();

        let ambient_auth: serde_json::Value =
            serde_json::from_slice(&std::fs::read(ambient_home.join("auth.json")).unwrap())
                .unwrap();
        assert_eq!(ambient_auth["tokens"]["account_id"], new_account_id);
        assert_eq!(
            result
                .ambient_account
                .unwrap()
                .provider_account_id
                .as_deref(),
            Some(new_account_id)
        );
        let materialized = result.materialized_account.unwrap();
        assert_eq!(
            materialized.provider_account_id.as_deref(),
            Some(old_account_id)
        );
        assert_eq!(
            result.desktop_session_backup_path.unwrap(),
            materialized.codex_home_path.join("desktop-session")
        );
        assert_eq!(
            result.desktop_session_restore_path.unwrap(),
            target_home.join("desktop-session")
        );
        assert!(result.desktop_session_restore_exists);

        let backup_files: Vec<PathBuf> = std::fs::read_dir(root.join("auth-backups"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("ambient-auth-")
            })
            .collect();
        assert_eq!(backup_files.len(), 1);
        let backup: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&backup_files[0]).unwrap()).unwrap();
        assert_eq!(backup["tokens"]["account_id"], old_account_id);

        for file_name in [".codex-global-state.json", ".codex-global-state.json.bak"] {
            let payload: serde_json::Value =
                serde_json::from_slice(&std::fs::read(ambient_home.join(file_name)).unwrap())
                    .unwrap();
            let creator_id = payload["electron-persisted-atom-state"]["environment"]["creator_id"]
                .as_str()
                .unwrap();
            assert_eq!(
                creator_id,
                format!("user-e9H3MsspGTF7UZJ8uaXuML55__{new_account_id}")
            );
        }

        super::super::file_locations::clear_app_support_directory_override();
        super::super::file_locations::clear_ambient_codex_home_override();
        super::super::file_locations::clear_codex_desktop_session_root_override();
    }
}
