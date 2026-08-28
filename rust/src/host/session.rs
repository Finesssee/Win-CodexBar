//! Shared host-session helpers reusable across the egui shell and Tauri shell.
//!
//! Keeps detection logic for SSH / RDP / remote sessions and primary-monitor
//! work-area queries in the shared crate so shells don't duplicate the logic.

/// Whether the current process is running inside an SSH session.
pub fn is_ssh_session() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_CLIENT").is_some()
}

/// Whether the current process is running inside a Windows Remote Desktop session.
#[cfg(windows)]
pub fn is_remote_session() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_REMOTESESSION};
    // SAFETY: GetSystemMetrics is a query-only Win32 call taking a constant
    // metric index; it reads no caller memory and returns a small int.
    unsafe { GetSystemMetrics(SM_REMOTESESSION) != 0 }
}

/// Non-Windows platforms cannot be in a Windows remote-desktop session.
#[cfg(not(windows))]
pub fn is_remote_session() -> bool {
    false
}

/// User-facing message explaining why launch is blocked under SSH.
pub fn ssh_session_error_message() -> &'static str {
    "CodexBar can't render its native window from an SSH session on this machine.\n\nOpen it from the logged-in Windows desktop session instead, or use the installed CLI over SSH:\n\n  codexbar-cli usage -p claude\n\nThe startup log is written to %TEMP%\\codexbar_launch_<pid>.log."
}

/// User-facing message explaining why launch is blocked under RDP.
pub fn remote_session_error_message() -> &'static str {
    "CodexBar can't render its native window inside a Windows Remote Desktop session on this machine.\n\nRun it from the local desktop session instead, or use the installed CLI while connected over RDP:\n\n  codexbar-cli usage -p claude\n\nThe startup log is written to %TEMP%\\codexbar_launch_<pid>.log."
}

/// Return a user-facing reason for blocking launch, or `None` when launch is fine.
pub fn launch_block_reason(is_ssh: bool, is_remote: bool) -> Option<&'static str> {
    if is_ssh {
        Some(ssh_session_error_message())
    } else if is_remote {
        Some(remote_session_error_message())
    } else {
        None
    }
}

/// Detect current launch-block reason by probing environment + OS APIs directly.
pub fn current_launch_block_reason() -> Option<&'static str> {
    launch_block_reason(is_ssh_session(), is_remote_session())
}

/// Primary-monitor work area in physical pixels (excludes the taskbar on Windows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkAreaPixels {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Return the primary monitor's work area in physical pixels.
///
/// Returns `None` when the platform cannot report a work area (non-Windows
/// desktops without X/Wayland helpers, headless CI, etc.).
#[cfg(windows)]
pub fn primary_work_area_pixels() -> Option<WorkAreaPixels> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };

    let mut rect = RECT::default();
    // SAFETY: SystemParametersInfoW with SPI_GETWORKAREA reads the desktop work
    // area into the single RECT we pass by mutable pointer; the flags field is
    // zero and the function performs no other memory access.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some((&mut rect as *mut RECT).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .is_ok()
    };

    if !ok {
        return None;
    }

    Some(WorkAreaPixels {
        x: rect.left,
        y: rect.top,
        width: (rect.right - rect.left).max(0),
        height: (rect.bottom - rect.top).max(0),
    })
}

#[cfg(not(windows))]
pub fn primary_work_area_pixels() -> Option<WorkAreaPixels> {
    None
}

// ── Credential detection helpers ─────────────────────────────────────

/// Filesystem path to the Gemini CLI's stored OAuth credentials, if a home
/// directory is available on this machine.
pub fn gemini_cli_credentials_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|home| home.join(".gemini").join("oauth_creds.json"))
}

/// `true` when the Gemini CLI's credentials file exists (i.e. the user has
/// signed in via `gemini auth login` locally).
pub fn gemini_cli_signed_in() -> bool {
    gemini_cli_credentials_path()
        .map(|p| p.exists())
        .unwrap_or(false)
}

/// Filesystem path to VertexAI application-default credentials. Respects the
/// `GOOGLE_APPLICATION_CREDENTIALS` env var when set, otherwise falls back to
/// the gcloud well-known location under the OS config dir.
pub fn vertexai_credentials_path() -> Option<std::path::PathBuf> {
    if let Ok(path) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS")
        && !path.trim().is_empty()
    {
        return Some(std::path::PathBuf::from(path));
    }
    dirs::config_dir().map(|config| {
        config
            .join("gcloud")
            .join("application_default_credentials.json")
    })
}

/// `true` when VertexAI application-default credentials exist on disk.
pub fn vertexai_signed_in() -> bool {
    vertexai_credentials_path()
        .map(|p| p.exists())
        .unwrap_or(false)
}

/// Detect JetBrains / Google-IDE configuration directories under the user's
/// config home. Returns an empty list if none are present.
pub fn jetbrains_detected_ide_paths() -> Vec<std::path::PathBuf> {
    let Some(config_dir) = dirs::config_dir() else {
        return Vec::new();
    };
    let roots = [config_dir.join("JetBrains"), config_dir.join("Google")];
    let mut out = Vec::new();
    for root in roots {
        if !root.exists() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_block_reason_prioritises_ssh() {
        let reason = launch_block_reason(true, true).expect("blocked");
        assert!(reason.contains("SSH session"));
    }

    #[test]
    fn launch_block_reason_reports_remote_when_only_remote() {
        let reason = launch_block_reason(false, true).expect("blocked");
        assert!(reason.contains("Remote Desktop"));
    }

    #[test]
    fn launch_block_reason_none_when_neither() {
        assert!(launch_block_reason(false, false).is_none());
    }
}
