//! Path resolution for Codex account storage and Codex Desktop session state.
//!
//! Mirrors `windows/.../file_locations.py` (MIT), adapted to CodexBar's
//! `%config%/CodexBar` convention.

use std::path::{Path, PathBuf};

/// Entries of the Codex Desktop MSIX session that must be preserved/restored
/// when switching accounts. Mirrors `DESKTOP_SESSION_STATE_ENTRIES`.
pub const DESKTOP_SESSION_STATE_ENTRIES: &[&str] = &[
    "blob_storage",
    "DIPS",
    "DIPS-wal",
    "Local State",
    "Local Storage",
    "Network",
    "Partitions",
    "Preferences",
    "Session Storage",
    "SharedStorage",
    "SharedStorage-wal",
    "shared_proto_db",
];

fn localappdata_directory() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("LOCALAPPDATA") {
        let path = PathBuf::from(path.trim());
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    None
}

thread_local! {
    static APP_SUPPORT_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
    static AMBIENT_HOME_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
    static DESKTOP_SESSION_ROOT_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Base directory holding the Codex account store (accounts.json, managed-homes).
pub fn app_support_directory() -> PathBuf {
    APP_SUPPORT_OVERRIDE
        .with(|cell| cell.borrow().clone())
        .unwrap_or_else(|| {
            dirs::config_dir()
                .map(|dir| dir.join("CodexBar").join("codex-accounts"))
                .unwrap_or_else(|| PathBuf::from(".").join("codex-accounts"))
        })
}

/// Override the app support root (tests / shell). Returns the previous value.
pub fn with_app_support_directory(path: PathBuf) -> Option<PathBuf> {
    APP_SUPPORT_OVERRIDE.with(|cell| {
        let previous = cell.borrow().clone();
        *cell.borrow_mut() = Some(path);
        previous
    })
}

pub fn clear_app_support_directory_override() {
    APP_SUPPORT_OVERRIDE.with(|cell| *cell.borrow_mut() = None);
}

pub fn accounts_file() -> PathBuf {
    app_support_directory().join("accounts.json")
}

pub fn snapshots_file() -> PathBuf {
    app_support_directory().join("snapshots.json")
}

pub fn managed_homes_directory() -> PathBuf {
    app_support_directory().join("managed-homes")
}

pub fn auth_backups_directory() -> PathBuf {
    app_support_directory().join("auth-backups")
}

/// The environment (ambient) Codex home.
pub fn ambient_codex_home() -> PathBuf {
    AMBIENT_HOME_OVERRIDE
        .with(|cell| cell.borrow().clone())
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".codex")
        })
}

/// Override the ambient home root (tests / shell).
pub fn with_ambient_codex_home(path: PathBuf) {
    AMBIENT_HOME_OVERRIDE.with(|cell| *cell.borrow_mut() = Some(path));
}

pub fn clear_ambient_codex_home_override() {
    AMBIENT_HOME_OVERRIDE.with(|cell| *cell.borrow_mut() = None);
}

pub const DESKTOP_SESSION_SNAPSHOT_DIRECTORY_NAME: &str = "desktop-session";

/// Ensure required directories exist.
pub fn ensure_directories() -> std::io::Result<()> {
    std::fs::create_dir_all(app_support_directory())?;
    std::fs::create_dir_all(managed_homes_directory())?;
    std::fs::create_dir_all(auth_backups_directory())
}

/// Discover the active Codex Desktop MSIX package session root
/// (`%LOCALAPPDATA%\Packages\OpenAI.Codex*\LocalCache\Roaming\Codex`).
pub fn codex_desktop_session_root() -> Option<PathBuf> {
    if let Some(override_path) = DESKTOP_SESSION_ROOT_OVERRIDE.with(|cell| cell.borrow().clone()) {
        return Some(override_path);
    }
    let packages_root = localappdata_directory()?.join("Packages");
    if !packages_root.exists() {
        return None;
    }
    let entries = std::fs::read_dir(&packages_root).ok()?;
    let mut packages: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("OpenAI.Codex"))
        })
        .collect();
    packages.sort();
    for package in packages {
        let session_root = package.join("LocalCache").join("Roaming").join("Codex");
        if session_root.exists() {
            return Some(session_root);
        }
    }
    None
}

/// Override the desktop session root (tests / shell).
pub fn with_codex_desktop_session_root(path: PathBuf) {
    DESKTOP_SESSION_ROOT_OVERRIDE.with(|cell| *cell.borrow_mut() = Some(path));
}

pub fn clear_codex_desktop_session_root_override() {
    DESKTOP_SESSION_ROOT_OVERRIDE.with(|cell| *cell.borrow_mut() = None);
}

/// Per-account desktop-session snapshot directory.
pub fn desktop_session_snapshot_path(home_path: &Path) -> PathBuf {
    home_path.join(DESKTOP_SESSION_SNAPSHOT_DIRECTORY_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_state_entries_are_embedded() {
        assert!(DESKTOP_SESSION_STATE_ENTRIES.contains(&"Local Storage"));
    }

    #[test]
    fn desktop_session_snapshot_path_nests_under_home() {
        let p = desktop_session_snapshot_path(Path::new("/tmp/acct"));
        assert_eq!(p, Path::new("/tmp/acct/desktop-session"));
    }

    #[test]
    fn app_support_default_resolves_to_config_dir() {
        clear_app_support_directory_override();
        let dir = app_support_directory();
        assert!(dir.to_string_lossy().contains("codex-accounts"));
    }
}
