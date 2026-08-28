//! WSL (Windows Subsystem for Linux) detection and environment helpers
//!
//! Provides utilities to detect if CodexBar is running inside WSL,
//! and to resolve Windows filesystem paths from within the Linux environment.

use std::path::PathBuf;

/// WSL distribution information
#[derive(Debug, Clone)]
pub struct WslInfo {
    pub distro_name: String,
    pub windows_username: Option<String>,
    pub drive_mount: PathBuf,
}

/// Detect if we are running inside WSL
pub fn is_wsl() -> bool {
    if let Ok(version) = std::fs::read_to_string("/proc/version") {
        let v = version.to_lowercase();
        if v.contains("microsoft") || v.contains("wsl") {
            return true;
        }
    }

    if std::env::var("WSL_DISTRO_NAME").is_ok() {
        return true;
    }

    if std::path::Path::new("/run/WSL").exists() {
        return true;
    }

    false
}

/// Get WSL environment information.
/// Returns None if not running inside WSL.
pub fn get_wsl_info() -> Option<WslInfo> {
    if !is_wsl() {
        return None;
    }

    let distro_name = std::env::var("WSL_DISTRO_NAME")
        .or_else(|_| {
            std::fs::read_to_string("/etc/os-release").map(|content| {
                content
                    .lines()
                    .find(|l| l.starts_with("NAME="))
                    .map(|l| l.trim_start_matches("NAME=").trim_matches('"').to_string())
                    .unwrap_or_else(|| "Unknown".to_string())
            })
        })
        .unwrap_or_else(|_| "Unknown".to_string());

    let drive_mount = PathBuf::from("/mnt/c");
    let windows_username = resolve_windows_username(&drive_mount);

    Some(WslInfo {
        distro_name,
        windows_username,
        drive_mount,
    })
}

/// Resolve the Windows username by looking at /mnt/c/Users/
fn resolve_windows_username(drive_mount: &std::path::Path) -> Option<String> {
    let users_dir = drive_mount.join("Users");
    if !users_dir.exists() {
        return None;
    }

    windows_user_from_env(&users_dir).or_else(|| first_windows_profile_dir(&users_dir))
}

fn windows_user_from_env(users_dir: &std::path::Path) -> Option<String> {
    let wsl_user = std::env::var("USER").ok()?;
    let candidate = users_dir.join(&wsl_user);
    (candidate.exists() && candidate.is_dir() && !is_system_user_dir(&wsl_user)).then_some(wsl_user)
}

fn first_windows_profile_dir(users_dir: &std::path::Path) -> Option<String> {
    let entries = std::fs::read_dir(users_dir).ok()?;
    entries
        .flatten()
        .find_map(|entry| windows_profile_name(&entry))
}

fn windows_profile_name(entry: &std::fs::DirEntry) -> Option<String> {
    let name = entry.file_name().to_string_lossy().to_string();
    (!is_system_user_dir(&name) && entry.path().is_dir() && entry.path().join("AppData").exists())
        .then_some(name)
}

fn is_system_user_dir(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "public"
            | "default"
            | "default user"
            | "all users"
            | "desktop"
            | "administrator"
            | "$recycle.bin"
            | "system volume information"
    )
}

/// Convert a Windows path to its WSL equivalent.
///
/// `C:\Users\John\AppData\Local` becomes `/mnt/c/Users/John/AppData/Local`.
#[allow(
    dead_code,
    reason = "WSL detection helper reserved for future cross-platform integration"
)]
pub fn windows_path_to_wsl(windows_path: &str) -> Option<PathBuf> {
    let path = windows_path.replace('\\', "/");

    if path.len() >= 2 && path.as_bytes()[1] == b':' {
        let drive_letter = (path.as_bytes()[0] as char).to_lowercase().next()?;
        let rest = path[2..].trim_start_matches('/');
        return Some(PathBuf::from(format!("/mnt/{}/{}", drive_letter, rest)));
    }

    None
}

/// Get the Windows AppData/Local path from within WSL
pub fn windows_appdata_local() -> Option<PathBuf> {
    let info = get_wsl_info()?;
    let user = info.windows_username?;
    Some(
        info.drive_mount
            .join("Users")
            .join(user)
            .join("AppData")
            .join("Local"),
    )
}

/// Get the Windows AppData/Roaming path from within WSL
pub fn windows_appdata_roaming() -> Option<PathBuf> {
    let info = get_wsl_info()?;
    let user = info.windows_username?;
    Some(
        info.drive_mount
            .join("Users")
            .join(user)
            .join("AppData")
            .join("Roaming"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_system_user_dir() {
        assert!(is_system_user_dir("Public"));
        assert!(is_system_user_dir("Default"));
        assert!(is_system_user_dir("Default User"));
        assert!(!is_system_user_dir("John"));
        assert!(!is_system_user_dir("alice"));
    }

    #[test]
    fn test_windows_path_to_wsl() {
        assert_eq!(
            windows_path_to_wsl(r"C:\Users\John\AppData\Local"),
            Some(PathBuf::from("/mnt/c/Users/John/AppData/Local"))
        );
        assert_eq!(
            windows_path_to_wsl("D:\\Games"),
            Some(PathBuf::from("/mnt/d/Games"))
        );
        assert_eq!(windows_path_to_wsl("/home/user"), None);
    }
}
