//! Codex Desktop restart control (MSIX).
//!
//! Port of `windows/.../codex_desktop.py` (MIT): builds a hidden PowerShell
//! script that stops the Codex Desktop processes, syncs the MSIX session state
//! (backup/restore of the session entries) and relaunches the app.

use std::io;
use std::path::{Path, PathBuf};

use base64::Engine;

use super::file_locations::{
    DESKTOP_SESSION_STATE_ENTRIES, app_support_directory, codex_desktop_session_root,
    ensure_directories,
};

pub const DEFAULT_RESTART_DELAY_SECONDS: f64 = 0.8;

/// Friendly Codex Desktop control error.
#[derive(Debug, thiserror::Error)]
pub enum CodexDesktopControlError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

pub fn restart_log_path() -> PathBuf {
    app_support_directory().join("codex-desktop-restart.log")
}

pub fn restart_script_path() -> PathBuf {
    app_support_directory().join("codex-desktop-restart.ps1")
}

/// Render the PowerShell restart script.
pub fn build_restart_script(
    delay_seconds: f64,
    session_root: Option<&Path>,
    backup_destination: Option<&Path>,
    restore_source: Option<&Path>,
) -> String {
    // Delay is bounded, non-negative seconds rendered to whole milliseconds;
    // rounding already makes the value whole, so f64->u64 cannot overflow.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "delay is non-negative seconds; whole milliseconds by design"
    )]
    let delay_ms = (delay_seconds.max(0.0) * 1000.0).round() as u64;
    let log_path = powershell_literal_path(&restart_log_path());
    let effective_session_root = session_root
        .map(Path::to_path_buf)
        .or_else(codex_desktop_session_root);
    let session_root_literal = powershell_path_or_null(effective_session_root.as_deref());
    let backup_destination_literal = powershell_path_or_null(backup_destination);
    let restore_source_literal = powershell_path_or_null(restore_source);
    let session_entries_literal = powershell_string_array(DESKTOP_SESSION_STATE_ENTRIES);

    format!(
        r#"$ErrorActionPreference = 'Stop'
$logPath = {log_path}
$sessionRoot = {session_root_literal}
$backupDestination = {backup_destination_literal}
$restoreSource = {restore_source_literal}
$sessionEntries = {session_entries_literal}
New-Item -ItemType Directory -Path ([System.IO.Path]::GetDirectoryName($logPath)) -Force | Out-Null
function Write-Log([string]$message) {{
    Add-Content -LiteralPath $logPath -Value ("[{{0}}] {{1}}" -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss.fff'), $message)
}}
function Clear-SessionEntry([string]$root, [string]$relativePath) {{
    if (-not $root) {{
        return
    }}
    $targetPath = Join-Path $root $relativePath
    if (Test-Path -LiteralPath $targetPath) {{
        Remove-Item -LiteralPath $targetPath -Recurse -Force -ErrorAction Stop
    }}
}}
function Copy-SessionEntry([string]$sourceRoot, [string]$destinationRoot, [string]$relativePath) {{
    if (-not $sourceRoot -or -not $destinationRoot) {{
        return
    }}
    $sourcePath = Join-Path $sourceRoot $relativePath
    if (-not (Test-Path -LiteralPath $sourcePath)) {{
        return
    }}
    $destinationPath = Join-Path $destinationRoot $relativePath
    $parentPath = [System.IO.Path]::GetDirectoryName($destinationPath)
    if ($parentPath) {{
        New-Item -ItemType Directory -Path $parentPath -Force | Out-Null
    }}
    $item = Get-Item -LiteralPath $sourcePath -Force
    if ($item.PSIsContainer) {{
        Copy-Item -LiteralPath $sourcePath -Destination $destinationPath -Recurse -Force
        return
    }}
    Copy-Item -LiteralPath $sourcePath -Destination $destinationPath -Force
}}
function Sync-DesktopSessionState() {{
    if (-not $sessionRoot) {{
        Write-Log 'Desktop session root was not detected.'
        return
    }}
    Write-Log ("Desktop session root: " + $sessionRoot)
    Write-Log ("Backup destination: " + $(if ($backupDestination) {{ $backupDestination }} else {{ '<none>' }}))
    Write-Log ("Restore source: " + $(if ($restoreSource) {{ $restoreSource }} else {{ '<none>' }}))
    if (-not (Test-Path -LiteralPath $sessionRoot)) {{
        Write-Log ("Desktop session root is missing: " + $sessionRoot)
        return
    }}
    if ($backupDestination) {{
        New-Item -ItemType Directory -Path $backupDestination -Force | Out-Null
        foreach ($relativePath in $sessionEntries) {{
            try {{
                Clear-SessionEntry $backupDestination $relativePath
                Copy-SessionEntry $sessionRoot $backupDestination $relativePath
                Write-Log ("Backed up session entry: " + $relativePath)
            }} catch {{
                Write-Log ("Failed to back up session entry " + $relativePath + ": " + $_.Exception.Message)
            }}
        }}
        Write-Log ("Backed up desktop session state to " + $backupDestination)
    }}
    if ($restoreSource) {{
        if (-not (Test-Path -LiteralPath $restoreSource)) {{
            Write-Log ("Restore source is missing; leaving the current desktop session in place: " + $restoreSource)
            return
        }}
        foreach ($relativePath in $sessionEntries) {{
            try {{
                Clear-SessionEntry $sessionRoot $relativePath
                Copy-SessionEntry $restoreSource $sessionRoot $relativePath
                Write-Log ("Restored session entry: " + $relativePath)
            }} catch {{
                Write-Log ("Failed to restore session entry " + $relativePath + ": " + $_.Exception.Message)
            }}
        }}
        Write-Log ("Restored desktop session state from " + $restoreSource)
    }}
}}
Write-Log 'Restart requested.'
$mainProcess = Get-CimInstance Win32_Process | Where-Object {{
    $_.Name -eq 'Codex.exe' -and
    $_.ExecutablePath -and
    $_.ExecutablePath -notlike '*\resources\codex.exe' -and
    $_.CommandLine -notmatch '--type='
}} | Select-Object -First 1
$launcherPath = $mainProcess.ExecutablePath
if ($launcherPath) {{
    Write-Log ("Using running launcher path: " + $launcherPath)
}}
if (-not $launcherPath) {{
    $package = Get-AppxPackage | Where-Object {{
        $_.Name -eq 'OpenAI.Codex' -or $_.PackageFamilyName -like 'OpenAI.Codex*'
    }} | Sort-Object Version -Descending | Select-Object -First 1
    if ($package -and $package.InstallLocation) {{
        $launcherPath = Join-Path $package.InstallLocation 'app\Codex.exe'
        Write-Log ("Using package launcher path: " + $launcherPath)
    }}
}}
if (-not $launcherPath) {{
    Write-Log 'Unable to locate the Codex Desktop executable.'
    throw 'Unable to locate the Codex Desktop executable.'
}}
Start-Sleep -Milliseconds {delay_ms}
$codexProcesses = Get-CimInstance Win32_Process | Where-Object {{
    $_.Name -ieq 'Codex.exe' -or
    $_.ExecutablePath -like '*\OpenAI.Codex_*\app\Codex.exe' -or
    $_.ExecutablePath -like '*\OpenAI.Codex_*\app\resources\codex.exe'
}}
Write-Log ("Found " + $codexProcesses.Count + " Codex processes to stop.")
$codexProcesses | ForEach-Object {{
    try {{
        & taskkill.exe /PID $_.ProcessId /F /T | Out-Null
        Write-Log ("taskkill succeeded for PID " + $_.ProcessId)
    }} catch {{
        Write-Log ("taskkill failed for PID " + $_.ProcessId + ": " + $_.Exception.Message)
    }}
    try {{
        Stop-Process -Id $_.ProcessId -Force -ErrorAction Stop
        Write-Log ("Stop-Process succeeded for PID " + $_.ProcessId)
    }} catch {{
        Write-Log ("Stop-Process failed for PID " + $_.ProcessId + ": " + $_.Exception.Message)
    }}
}}
$deadline = (Get-Date).AddSeconds(8)
while ((Get-Date) -lt $deadline) {{
    $remaining = Get-CimInstance Win32_Process | Where-Object {{
        $_.Name -ieq 'Codex.exe' -or
        $_.ExecutablePath -like '*\OpenAI.Codex_*\app\Codex.exe' -or
        $_.ExecutablePath -like '*\OpenAI.Codex_*\app\resources\codex.exe'
    }}
    if (-not $remaining) {{
        Write-Log 'All Codex processes exited.'
        break
    }}
    Write-Log ("Still waiting for " + $remaining.Count + " Codex processes to exit.")
    $remaining | ForEach-Object {{
        try {{
            & taskkill.exe /PID $_.ProcessId /F /T | Out-Null
        }} catch {{}}
    }}
    Start-Sleep -Milliseconds 250
}}
if (Get-CimInstance Win32_Process | Where-Object {{
    $_.Name -ieq 'Codex.exe' -or
    $_.ExecutablePath -like '*\OpenAI.Codex_*\app\Codex.exe' -or
    $_.ExecutablePath -like '*\OpenAI.Codex_*\app\resources\codex.exe'
}}) {{
    Write-Log 'Continuing with relaunch after timeout while some Codex processes still appear alive.'
}}
Sync-DesktopSessionState
Start-Sleep -Milliseconds 700
Start-Process -FilePath $launcherPath
Write-Log 'Codex Desktop relaunched.'
"#
    )
    .trim_end()
    .to_string()
}

/// Encode a script as base64 UTF-16LE (PowerShell `-EncodedCommand`).
pub fn encode_powershell_script(script: &str) -> String {
    let utf16: Vec<u16> = script.encode_utf16().collect();
    let bytes: Vec<u8> = utf16
        .into_iter()
        .flat_map(|unit| unit.to_le_bytes())
        .collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Build the hidden PowerShell command line for a script file.
pub fn build_restart_command(script_path: &Path) -> Vec<String> {
    let powershell_exe = std::env::var("WINDIR")
        .map(|windir| {
            PathBuf::from(windir).join("System32\\WindowsPowerShell\\v1.0\\powershell.exe")
        })
        .unwrap_or_else(|_| {
            PathBuf::from(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe")
        });
    vec![
        powershell_exe.to_string_lossy().to_string(),
        "-NoProfile".to_string(),
        "-NonInteractive".to_string(),
        "-WindowStyle".to_string(),
        "Hidden".to_string(),
        "-ExecutionPolicy".to_string(),
        "Bypass".to_string(),
        "-File".to_string(),
        script_path.to_string_lossy().to_string(),
    ]
}

/// Write the restart script and launch a hidden PowerShell that runs it.
pub fn restart_codex_desktop(
    delay_seconds: f64,
    session_root: Option<&Path>,
    backup_destination: Option<&Path>,
    restore_source: Option<&Path>,
) -> Result<(), CodexDesktopControlError> {
    ensure_directories()?;
    let script = build_restart_script(
        delay_seconds,
        session_root,
        backup_destination,
        restore_source,
    );
    fs_write(restart_script_path(), script)?;
    launch_hidden_powershell(&restart_script_path())
}

#[cfg(windows)]
fn fs_write(path: PathBuf, content: String) -> io::Result<()> {
    std::fs::write(path, content)
}

#[cfg(windows)]
fn launch_hidden_powershell(script_path: &Path) -> Result<(), CodexDesktopControlError> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    let mut command = Command::new(&build_restart_command(script_path)[0]);
    command.args(&build_restart_command(script_path)[1..]);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP (never DETACHED_PROCESS).
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    command.spawn().map(|_| ()).map_err(|error| {
        CodexDesktopControlError::Message(format!("Failed to restart Codex Desktop: {error}"))
    })
}

#[cfg(not(windows))]
fn fs_write(path: PathBuf, content: String) -> io::Result<()> {
    std::fs::write(path, content)
}

#[cfg(not(windows))]
fn launch_hidden_powershell(_script_path: &Path) -> Result<(), CodexDesktopControlError> {
    Err(CodexDesktopControlError::Message(
        "Codex Desktop restart is only available on Windows.".to_string(),
    ))
}

fn powershell_literal_path(path: &Path) -> String {
    let normalized = path.to_string_lossy().replace('/', "\\");
    format!("'{}'", normalized.replace('\'', "''"))
}

fn powershell_path_or_null(path: Option<&Path>) -> String {
    match path {
        Some(path) => powershell_literal_path(path),
        None => "$null".to_string(),
    }
}

fn powershell_string_array(values: &[&str]) -> String {
    let quoted: Vec<String> = values
        .iter()
        .map(|value| format!("'{}'", value.replace('\'', "''")))
        .collect();
    format!("@({})", quoted.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_restart_script_includes_restart_flow() {
        let script = build_restart_script(1.25, None, None, None);
        assert!(script.contains("Write-Log"));
        assert!(script.contains("Get-CimInstance Win32_Process"));
        assert!(script.contains("Get-AppxPackage"));
        assert!(script.contains("taskkill.exe /PID"));
        assert!(script.contains("Stop-Process -Id $_.ProcessId -Force"));
        assert!(script.contains("Start-Process -FilePath $launcherPath"));
        assert!(script.contains("Start-Sleep -Milliseconds 1250"));
    }

    #[test]
    fn build_restart_script_includes_desktop_session_sync() {
        let script = build_restart_script(
            0.5,
            Some(Path::new(
                r"C:\Users\test\AppData\Local\Packages\OpenAI.Codex_test\LocalCache\Roaming\Codex",
            )),
            Some(Path::new(
                r"C:\Users\test\AppData\Roaming\CodexControl\managed-homes\current\desktop-session",
            )),
            Some(Path::new(
                r"C:\Users\test\AppData\Roaming\CodexControl\managed-homes\target\desktop-session",
            )),
        );

        assert!(script.contains(
            "$sessionRoot = 'C:\\Users\\test\\AppData\\Local\\Packages\\OpenAI.Codex_test\\LocalCache\\Roaming\\Codex'"
        ));
        assert!(script.contains(
            "$backupDestination = 'C:\\Users\\test\\AppData\\Roaming\\CodexControl\\managed-homes\\current\\desktop-session'"
        ));
        assert!(script.contains(
            "$restoreSource = 'C:\\Users\\test\\AppData\\Roaming\\CodexControl\\managed-homes\\target\\desktop-session'"
        ));
        assert!(script.contains("function Sync-DesktopSessionState()"));
        assert!(script.contains("Copy-SessionEntry $sessionRoot $backupDestination $relativePath"));
        assert!(script.contains("Copy-SessionEntry $restoreSource $sessionRoot $relativePath"));
        assert!(script.contains("Clear-SessionEntry $sessionRoot $relativePath"));
        assert!(
            script.contains(
                "Restore source is missing; leaving the current desktop session in place"
            )
        );
        assert!(script.contains("Failed to back up session entry"));
        assert!(script.contains("Failed to restore session entry"));
    }

    #[test]
    fn encode_powershell_script_round_trips_utf16le() {
        use base64::Engine;
        let script = "Start-Process -FilePath 'Codex.exe'";
        let encoded = encode_powershell_script(script);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        let decoded = String::from_utf16(&units).unwrap();
        assert_eq!(decoded, script);
    }

    #[test]
    fn build_restart_command_invokes_hidden_powershell_file() {
        let command = build_restart_command(Path::new(r"C:\temp\restart.ps1"));
        assert!(command[0].to_lowercase().ends_with("powershell.exe"));
        assert!(command.contains(&"-WindowStyle".to_string()));
        assert!(command.contains(&"Hidden".to_string()));
        assert_eq!(command[command.len() - 2], "-File");
        assert!(
            command
                .last()
                .unwrap()
                .to_lowercase()
                .ends_with("temp\\restart.ps1")
        );
    }
}
