//! Auto-update checker for CodexBar
//! Checks GitHub releases for new versions and handles background downloads

use crate::settings::UpdateChannel;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::watch;

const GITHUB_REPO: &str = "Finesssee/Win-CodexBar";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// State of the update download process
#[derive(Debug, Clone, PartialEq, Default)]
pub enum UpdateState {
    /// No update available or not checked
    #[default]
    Idle,
    /// Update available but not downloaded
    Available,
    /// Currently downloading with progress (0.0 to 1.0)
    Downloading(f32),
    /// Download complete, ready to install
    Ready(PathBuf),
    /// Download or install failed
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateDelivery {
    Installer,
    Manual,
}

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub version: String,
    pub download_url: String,
    pub expected_sha256: Option<String>,
    #[allow(
        dead_code,
        reason = "update metadata fields are deserialized for version comparison but not all are read"
    )]
    pub release_url: String,
    #[allow(
        dead_code,
        reason = "update metadata fields are deserialized for version comparison but not all are read"
    )]
    pub release_notes: String,
    pub delivery: UpdateDelivery,
}

impl UpdateInfo {
    pub fn supports_auto_apply(&self) -> bool {
        self.delivery == UpdateDelivery::Installer
    }

    pub fn supports_auto_download(&self) -> bool {
        self.delivery == UpdateDelivery::Installer && self.expected_sha256.is_some()
    }
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    body: Option<String>,
    assets: Vec<GitHubAsset>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    #[allow(
        dead_code,
        reason = "update metadata fields are deserialized for version comparison but not all are read"
    )]
    prerelease: bool,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    digest: Option<String>,
}

/// Check for updates from GitHub releases
///
/// When `channel` is `UpdateChannel::Beta`, includes pre-release versions.
/// When `channel` is `UpdateChannel::Stable`, only considers stable releases.
#[allow(
    dead_code,
    reason = "update check response fields are deserialized for parsing but not all are read"
)]
pub async fn check_for_updates() -> Option<UpdateInfo> {
    check_for_updates_with_channel(UpdateChannel::Stable).await
}

/// Check for updates from GitHub releases with a specific channel
///
/// When `channel` is `UpdateChannel::Beta`, includes pre-release versions.
/// When `channel` is `UpdateChannel::Stable`, only considers stable releases.
pub async fn check_for_updates_with_channel(channel: UpdateChannel) -> Option<UpdateInfo> {
    let client = update_client()?;
    let response = client.get(release_url(channel)).send().await.ok()?;
    let release = parse_release_response(response, channel).await?;
    let remote_version = remote_version_from_tag(&release.tag_name);

    if is_newer_version(remote_version, CURRENT_VERSION) {
        select_release_target(&release)
    } else {
        None
    }
}

fn release_url(channel: UpdateChannel) -> String {
    match channel {
        UpdateChannel::Beta => format!("https://api.github.com/repos/{}/releases", GITHUB_REPO),
        UpdateChannel::Stable => {
            format!(
                "https://api.github.com/repos/{}/releases/latest",
                GITHUB_REPO
            )
        }
    }
}

fn update_client() -> Option<reqwest::Client> {
    crate::core::apply_app_proxy(reqwest::Client::builder())
        .user_agent("CodexBar")
        .build()
        .ok()
}

async fn parse_release_response(
    response: reqwest::Response,
    channel: UpdateChannel,
) -> Option<GitHubRelease> {
    if !response.status().is_success() {
        tracing::debug!("GitHub API returned status: {}", response.status());
        return None;
    }

    match channel {
        UpdateChannel::Beta => {
            let releases: Vec<GitHubRelease> = response.json().await.ok()?;
            releases.into_iter().find(|r| !r.draft)
        }
        UpdateChannel::Stable => response.json().await.ok(),
    }
}

fn remote_version_from_tag(tag_name: &str) -> &str {
    tag_name
        .trim_start_matches('v')
        .split('-')
        .next()
        .unwrap_or(tag_name)
}

fn select_release_target(release: &GitHubRelease) -> Option<UpdateInfo> {
    let installer = release
        .assets
        .iter()
        .find(|a| is_installer_asset_name(&a.name));

    let (download_url, delivery, expected_sha256) = if let Some(asset) = installer {
        (
            asset.browser_download_url.clone(),
            UpdateDelivery::Installer,
            asset
                .digest
                .as_deref()
                .and_then(parse_sha256_digest)
                .map(str::to_string),
        )
    } else {
        (release.html_url.clone(), UpdateDelivery::Manual, None)
    };

    Some(UpdateInfo {
        version: release.tag_name.clone(),
        download_url,
        expected_sha256,
        release_url: release.html_url.clone(),
        release_notes: release.body.clone().unwrap_or_default(),
        delivery,
    })
}

fn is_installer_asset_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with("-setup.exe") || lower.ends_with(".msi")
}

fn parse_version_triplet(v: &str) -> (u32, u32, u32) {
    let parts: Vec<u32> = v.split('.').filter_map(|p| p.parse().ok()).collect();
    (
        parts.first().copied().unwrap_or(0),
        parts.get(1).copied().unwrap_or(0),
        parts.get(2).copied().unwrap_or(0),
    )
}

fn installer_version_from_name(name: &str) -> Option<(u32, u32, u32)> {
    let lower = name.to_ascii_lowercase();
    let stem = lower
        .strip_suffix("-setup.exe")
        .or_else(|| lower.strip_suffix(".msi"))?;

    let version_candidate = stem.split_once('-').map(|(_, rest)| rest).unwrap_or(stem);
    let version_text: String = version_candidate
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect();

    if version_text.is_empty() {
        return None;
    }

    let version = parse_version_triplet(&version_text);
    if version == (0, 0, 0) {
        return None;
    }

    Some(version)
}

fn parse_sha256_digest(digest: &str) -> Option<&str> {
    let (algo, hex) = digest.split_once(':')?;
    if !algo.eq_ignore_ascii_case("sha256") {
        return None;
    }

    let hex = hex.trim();
    if hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(hex)
    } else {
        None
    }
}

/// Compare semantic versions, returns true if remote is newer
fn is_newer_version(remote: &str, current: &str) -> bool {
    let remote_v = parse_version_triplet(remote);
    let current_v = parse_version_triplet(current);

    remote_v > current_v
}

/// Get the current version
#[allow(
    dead_code,
    reason = "update info struct reserved for future UI integration"
)]
pub fn current_version() -> &'static str {
    CURRENT_VERSION
}

/// Get the download directory for updates
fn get_download_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|p| p.join("CodexBar").join("updates"))
}

/// Download an update with progress reporting
///
/// Returns a receiver that will receive progress updates (0.0 to 1.0)
/// and the final downloaded file path on completion.
pub async fn download_update(
    update_info: &UpdateInfo,
    progress_tx: watch::Sender<UpdateState>,
) -> Result<PathBuf, String> {
    validate_auto_download(update_info)?;

    let file_path = prepare_download_path(update_info)?;
    let response = start_download(&update_info.download_url).await?;
    write_download_response(response, &file_path, &progress_tx).await?;
    verify_download_hash(&file_path, expected_update_sha256(update_info)?).await?;

    // Signal download complete; the receiver may be gone, which is fine.
    let _ready_signal = progress_tx.send(UpdateState::Ready(file_path.clone()));

    Ok(file_path)
}

fn validate_auto_download(update_info: &UpdateInfo) -> Result<(), String> {
    if update_info.supports_auto_download() {
        Ok(())
    } else {
        Err("This update must be downloaded manually from the release page.".to_string())
    }
}

fn prepare_download_path(update_info: &UpdateInfo) -> Result<PathBuf, String> {
    let download_dir =
        get_download_dir().ok_or_else(|| "Could not determine download directory".to_string())?;

    std::fs::create_dir_all(&download_dir)
        .map_err(|e| format!("Failed to create download directory: {}", e))?;

    Ok(download_dir.join(download_filename(&update_info.download_url)))
}

fn download_filename(download_url: &str) -> String {
    download_url
        .split('/')
        .next_back()
        .unwrap_or("CodexBar-Setup.exe")
        .to_string()
}

fn expected_update_sha256(update_info: &UpdateInfo) -> Result<&str, String> {
    update_info
        .expected_sha256
        .as_deref()
        .ok_or_else(|| "Missing SHA256 digest for update asset".to_string())
}

fn update_http_client() -> Result<reqwest::Client, String> {
    crate::core::apply_app_proxy(reqwest::Client::builder())
        .user_agent("CodexBar")
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))
}

async fn start_download(download_url: &str) -> Result<reqwest::Response, String> {
    let response = update_http_client()?
        .get(download_url)
        .send()
        .await
        .map_err(|e| format!("Failed to start download: {}", e))?;

    if response.status().is_success() {
        Ok(response)
    } else {
        Err(format!(
            "Download failed with status: {}",
            response.status()
        ))
    }
}

async fn write_download_response(
    response: reqwest::Response,
    file_path: &Path,
    progress_tx: &watch::Sender<UpdateState>,
) -> Result<(), String> {
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;

    let total_size = response.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(file_path)
        .await
        .map_err(|e| format!("Failed to create file: {}", e))?;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Error downloading chunk: {}", e))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("Failed to write chunk: {}", e))?;

        downloaded += chunk.len() as u64;
        send_download_progress(progress_tx, downloaded, total_size);
    }

    file.flush()
        .await
        .map_err(|e| format!("Failed to flush file: {}", e))
}

fn send_download_progress(
    progress_tx: &watch::Sender<UpdateState>,
    downloaded: u64,
    total_size: u64,
) {
    let progress = if total_size > 0 {
        (downloaded as f32 / total_size as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // Best-effort progress update; a dropped receiver is fine.
    let _progress_update = progress_tx.send(UpdateState::Downloading(progress));
}

/// Verify the SHA256 hash of a downloaded file against release metadata.
async fn verify_download_hash(file_path: &PathBuf, expected_hash: &str) -> Result<(), String> {
    let actual = sha256_file_async(file_path).await?;
    if let Err(e) = verify_sha256_hex(&actual, expected_hash) {
        // Best-effort cleanup of the corrupt download; the hash error is returned regardless.
        let _removed = std::fs::remove_file(file_path);
        return Err(e);
    }

    tracing::info!("SHA256 verification passed for {:?}", file_path);
    Ok(())
}

/// Re-verify an installer immediately before launching it.
pub fn verify_installer_hash(file_path: &Path, expected_hash: &str) -> Result<(), String> {
    let actual = sha256_file(file_path)?;
    verify_sha256_hex(&actual, expected_hash)
}

fn verify_sha256_hex(actual_hash: &str, expected_hash: &str) -> Result<(), String> {
    let expected = expected_hash.trim().to_ascii_lowercase();
    if expected.len() != 64 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("Invalid SHA256 digest provided for update asset".to_string());
    }

    if actual_hash != expected {
        return Err("SHA256 mismatch. Download may be corrupted or tampered.".to_string());
    }

    Ok(())
}

async fn sha256_file_async(file_path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};

    let file_bytes = tokio::fs::read(file_path)
        .await
        .map_err(|e| format!("Failed to read downloaded file for hashing: {}", e))?;

    let mut hasher = Sha256::new();
    hasher.update(&file_bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

fn sha256_file(file_path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};

    let file_bytes = std::fs::read(file_path)
        .map_err(|e| format!("Failed to read downloaded file for hashing: {}", e))?;

    let mut hasher = Sha256::new();
    hasher.update(&file_bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

/// Start background download of an update
///
/// Returns a receiver that can be polled for progress updates.
#[allow(
    dead_code,
    reason = "update info struct reserved for future UI integration"
)]
pub fn start_background_download(
    update_info: UpdateInfo,
) -> (
    Arc<watch::Receiver<UpdateState>>,
    std::thread::JoinHandle<()>,
) {
    let (tx, rx) = watch::channel(UpdateState::Available);
    let rx = Arc::new(rx);

    let handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            match download_update(&update_info, tx.clone()).await {
                Ok(_path) => {
                    // UpdateState::Ready is already sent by download_update
                }
                Err(e) => {
                    // Best-effort failure signal; the receiver may already be gone.
                    let _failure_signal = tx.send(UpdateState::Failed(e));
                }
            }
        });
    });

    (rx, handle)
}

/// Apply a downloaded update by spawning the installer and exiting
///
/// This function will:
/// 1. Spawn the installer executable
/// 2. Exit the current application
///
/// The installer should handle upgrading the application while it's closed.
pub fn apply_update(installer_path: &PathBuf) -> Result<(), String> {
    // Verify the file exists
    if !installer_path.exists() {
        return Err(format!("Installer not found: {:?}", installer_path));
    }

    let file_name = installer_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if !is_installer_asset_name(file_name) {
        return Err(
            "Downloaded update is not an installer. Open the release page to update manually."
                .to_string(),
        );
    }

    #[cfg(target_os = "windows")]
    spawn_windows_installer(
        installer_path,
        &windows_update_relaunch_path(
            &std::env::current_exe()
                .map_err(|e| format!("Failed to determine current executable for restart: {e}"))?,
        ),
    )?;

    #[cfg(not(target_os = "windows"))]
    {
        use std::process::Command;

        Command::new(installer_path)
            .spawn()
            .map_err(|e| format!("Failed to launch installer: {}", e))?;
    }

    // Exit the application to allow the installer to proceed
    std::process::exit(0);
}

#[cfg(target_os = "windows")]
fn windows_update_relaunch_path(current_exe: &Path) -> PathBuf {
    let file_name = current_exe.file_name().and_then(|name| name.to_str());
    if file_name.is_some_and(|name| name.eq_ignore_ascii_case("codexbar-desktop.exe"))
        && let Some(primary_desktop_exe) = current_exe
            .parent()
            .map(|dir| dir.join("codexbar.exe"))
            .filter(|path| path.exists())
    {
        return primary_desktop_exe;
    }

    current_exe.to_path_buf()
}

#[cfg(target_os = "windows")]
fn spawn_windows_installer(installer_path: &Path, relaunch_path: &Path) -> Result<(), String> {
    use std::process::Command;

    let plan = windows_installer_launch_plan(installer_path)?;
    Command::new(windows_powershell_path())
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &windows_installer_apply_script(&plan, std::process::id(), relaunch_path),
        ])
        .spawn()
        .map_err(|e| format!("Failed to launch installer: {}", e))?;
    Ok(())
}

#[cfg(target_os = "windows")]
struct WindowsInstallerLaunchPlan {
    program: PathBuf,
    args: Vec<std::ffi::OsString>,
}

#[cfg(target_os = "windows")]
fn windows_installer_launch_plan(
    installer_path: &Path,
) -> Result<WindowsInstallerLaunchPlan, String> {
    let extension = installer_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if extension == "msi" {
        return Ok(WindowsInstallerLaunchPlan {
            program: PathBuf::from("msiexec.exe"),
            args: vec![
                std::ffi::OsString::from("/i"),
                installer_path.as_os_str().to_os_string(),
                std::ffi::OsString::from("/quiet"),
                std::ffi::OsString::from("/norestart"),
            ],
        });
    }

    // CodexBar release setup executables are built by rust/installer/codexbar.iss
    // (Inno Setup). Silent installs skip the installer's postinstall [Run]
    // entry, so the update helper relaunches CodexBar after setup exits.
    Ok(WindowsInstallerLaunchPlan {
        program: installer_path.to_path_buf(),
        args: vec![
            std::ffi::OsString::from("/SILENT"),
            std::ffi::OsString::from("/SUPPRESSMSGBOXES"),
            std::ffi::OsString::from("/CLOSEAPPLICATIONS"),
            std::ffi::OsString::from("/NORESTART"),
        ],
    })
}

#[cfg(target_os = "windows")]
fn windows_installer_apply_script(
    plan: &WindowsInstallerLaunchPlan,
    current_pid: u32,
    relaunch_path: &Path,
) -> String {
    format!(
        "Wait-Process -Id {current_pid} -ErrorAction SilentlyContinue; \
         $p = Start-Process -FilePath {} -ArgumentList {} -PassThru -Wait; \
         if ($p.ExitCode -eq 0 -and (Test-Path {})) {{ \
           Start-Process -FilePath {} -ArgumentList @('menubar') \
         }}",
        powershell_single_quoted(&plan.program.to_string_lossy()),
        powershell_argument_list(&plan.args),
        powershell_single_quoted(&relaunch_path.to_string_lossy()),
        powershell_single_quoted(&relaunch_path.to_string_lossy()),
    )
}

#[cfg(target_os = "windows")]
fn powershell_argument_list(args: &[std::ffi::OsString]) -> String {
    let args = args
        .iter()
        .map(|arg| powershell_single_quoted(&arg.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(",");
    format!("@({args})")
}

#[cfg(target_os = "windows")]
fn powershell_single_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(target_os = "windows")]
fn windows_powershell_path() -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .map(|root| {
            root.join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe")
        })
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from("powershell.exe"))
}

/// Check if there's a pending update ready to install
#[allow(
    dead_code,
    reason = "update info struct reserved for future UI integration"
)]
pub fn get_pending_update() -> Option<PathBuf> {
    let download_dir = get_download_dir()?;

    if !download_dir.exists() {
        return None;
    }

    find_pending_installer_in_dir(&download_dir)
}

fn find_pending_installer_in_dir(download_dir: &Path) -> Option<PathBuf> {
    let current_version = parse_version_triplet(CURRENT_VERSION);

    // Only treat newer installer assets as pending updates, and prefer the highest
    // installer version when multiple cached installers are present.
    std::fs::read_dir(download_dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let file_name = path.file_name()?.to_str()?;
            let installer_version = installer_version_from_name(file_name)?;
            if installer_version <= current_version {
                return None;
            }

            let modified = entry
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs())
                .unwrap_or(0);

            Some(((installer_version, modified), path))
        })
        .max_by_key(|(sort_key, _)| *sort_key)
        .map(|(_, path)| path)
}

/// Clean up downloaded updates
#[allow(
    dead_code,
    reason = "update info struct reserved for future UI integration"
)]
pub fn cleanup_downloads() {
    if let Some(download_dir) = get_download_dir() {
        let _cleaned = std::fs::remove_dir_all(&download_dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_comparison() {
        assert!(is_newer_version("1.0.1", "1.0.0"));
        assert!(is_newer_version("1.1.0", "1.0.0"));
        assert!(is_newer_version("2.0.0", "1.0.0"));
        assert!(!is_newer_version("1.0.0", "1.0.0"));
        assert!(!is_newer_version("0.9.0", "1.0.0"));
        assert!(is_newer_version("1.0.0", "0.1.0"));
    }

    #[test]
    fn prefers_installer_asset_for_auto_update() {
        let release = GitHubRelease {
            tag_name: "v1.2.6".to_string(),
            html_url: "https://github.com/Finesssee/Win-CodexBar/releases/tag/v1.2.6".to_string(),
            body: None,
            assets: vec![
                GitHubAsset {
                    name: "codexbar.exe".to_string(),
                    browser_download_url: "https://example.com/codexbar.exe".to_string(),
                    digest: None,
                },
                GitHubAsset {
                    name: "CodexBar-1.2.6-Setup.exe".to_string(),
                    browser_download_url: "https://example.com/CodexBar-1.2.6-Setup.exe"
                        .to_string(),
                    digest: Some(
                        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                            .to_string(),
                    ),
                },
            ],
            draft: false,
            prerelease: false,
        };

        let update = select_release_target(&release).expect("update target");

        assert_eq!(
            update.download_url,
            "https://example.com/CodexBar-1.2.6-Setup.exe"
        );
        assert!(update.supports_auto_apply());
        assert!(update.supports_auto_download());
    }

    #[test]
    fn falls_back_to_manual_release_when_only_portable_exe_exists() {
        let release = GitHubRelease {
            tag_name: "v1.2.6".to_string(),
            html_url: "https://github.com/Finesssee/Win-CodexBar/releases/tag/v1.2.6".to_string(),
            body: None,
            assets: vec![GitHubAsset {
                name: "codexbar.exe".to_string(),
                browser_download_url: "https://example.com/codexbar.exe".to_string(),
                digest: None,
            }],
            draft: false,
            prerelease: false,
        };

        let update = select_release_target(&release).expect("update target");

        assert_eq!(
            update.download_url,
            "https://github.com/Finesssee/Win-CodexBar/releases/tag/v1.2.6"
        );
        assert!(!update.supports_auto_apply());
    }

    #[test]
    fn finds_newest_pending_installer_and_ignores_portable_exe() {
        let temp = tempfile::tempdir().expect("temp dir");
        let (major, minor, patch) = parse_version_triplet(CURRENT_VERSION);
        let portable = temp.path().join("codexbar.exe");
        let older = temp
            .path()
            .join(format!("CodexBar-{}.{}.{}-Setup.exe", major, minor, patch));
        let newer = temp.path().join(format!(
            "CodexBar-{}.{}.{}-Setup.exe",
            major,
            minor,
            patch + 1
        ));

        std::fs::write(&portable, b"portable").expect("write portable");
        std::fs::write(&older, b"older installer").expect("write older installer");
        std::fs::write(&newer, b"newer installer").expect("write newer installer");

        let pending = find_pending_installer_in_dir(temp.path()).expect("pending installer");

        assert_eq!(pending, newer);
    }

    #[test]
    fn ignores_cached_installers_for_current_or_older_versions() {
        let temp = tempfile::tempdir().expect("temp dir");
        let (major, minor, patch) = parse_version_triplet(CURRENT_VERSION);
        let current = temp
            .path()
            .join(format!("CodexBar-{}.{}.{}-Setup.exe", major, minor, patch));
        let older = temp.path().join(format!(
            "CodexBar-{}.{}.{}-Setup.exe",
            major,
            minor,
            patch.saturating_sub(1)
        ));

        std::fs::write(&current, b"current installer").expect("write current installer");
        std::fs::write(&older, b"older installer").expect("write older installer");

        assert!(find_pending_installer_in_dir(temp.path()).is_none());
    }

    #[test]
    fn parses_prerelease_installer_names_for_beta_updates() {
        let (major, minor, patch) = parse_version_triplet(CURRENT_VERSION);
        assert_eq!(
            installer_version_from_name(&format!(
                "CodexBar-{}.{}.{}-beta.1-Setup.exe",
                major,
                minor,
                patch + 1
            )),
            Some((major, minor, patch + 1))
        );
    }

    #[test]
    fn verify_installer_hash_accepts_matching_sha256() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("CodexBar-1.2.3-Setup.exe");
        std::fs::write(&path, b"installer bytes").expect("write installer");

        let expected = sha256_file(&path).expect("hash");
        assert!(verify_installer_hash(&path, &expected).is_ok());
    }

    #[test]
    fn verify_installer_hash_rejects_mismatched_sha256() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("CodexBar-1.2.3-Setup.exe");
        std::fs::write(&path, b"installer bytes").expect("write installer");

        let wrong = "0".repeat(64);
        let err = verify_installer_hash(&path, &wrong).unwrap_err();
        assert!(err.contains("SHA256 mismatch"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_setup_exe_uses_inno_silent_flags() {
        let path = PathBuf::from(r"C:\Temp\CodexBar-1.2.3-Setup.exe");

        let plan = windows_installer_launch_plan(&path).expect("launch plan");

        assert_eq!(plan.program, path);
        assert_eq!(
            plan.args,
            vec![
                std::ffi::OsString::from("/SILENT"),
                std::ffi::OsString::from("/SUPPRESSMSGBOXES"),
                std::ffi::OsString::from("/CLOSEAPPLICATIONS"),
                std::ffi::OsString::from("/NORESTART"),
            ]
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_apply_script_waits_for_current_process_before_installing() {
        let path = PathBuf::from(r"C:\Temp\CodexBar-1.2.3-Setup.exe");
        let relaunch_path = PathBuf::from(r"C:\Program Files\CodexBar\codexbar.exe");
        let plan = windows_installer_launch_plan(&path).expect("launch plan");

        let script = windows_installer_apply_script(&plan, 12345, &relaunch_path);

        assert!(script.contains("Wait-Process -Id 12345"));
        assert!(script.contains(r"Start-Process -FilePath 'C:\Temp\CodexBar-1.2.3-Setup.exe'"));
        assert!(script.contains(
            "-ArgumentList @('/SILENT','/SUPPRESSMSGBOXES','/CLOSEAPPLICATIONS','/NORESTART')"
        ));
        assert!(script.contains("-PassThru -Wait"));
        assert!(script.contains(r"Start-Process -FilePath 'C:\Program Files\CodexBar\codexbar.exe' -ArgumentList @('menubar')"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_update_relaunch_path_prefers_primary_desktop_exe_from_legacy_alias() {
        let temp = tempfile::tempdir().expect("temp dir");
        let desktop_path = temp.path().join("codexbar.exe");
        let legacy_desktop_path = temp.path().join("codexbar-desktop.exe");
        std::fs::write(&desktop_path, b"desktop").expect("write desktop");
        std::fs::write(&legacy_desktop_path, b"legacy desktop").expect("write legacy desktop");

        assert_eq!(
            windows_update_relaunch_path(&legacy_desktop_path),
            desktop_path
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_msi_uses_msiexec_quiet_install() {
        let path = PathBuf::from(r"C:\Temp\CodexBar-1.2.3.msi");

        let plan = windows_installer_launch_plan(&path).expect("launch plan");

        assert_eq!(plan.program, PathBuf::from("msiexec.exe"));
        assert_eq!(
            plan.args,
            vec![
                std::ffi::OsString::from("/i"),
                path.as_os_str().to_os_string(),
                std::ffi::OsString::from("/quiet"),
                std::ffi::OsString::from("/norestart"),
            ]
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn powershell_quoting_escapes_single_quotes() {
        assert_eq!(
            powershell_single_quoted(r"C:\Temp\CodexBar's Setup.exe"),
            r"'C:\Temp\CodexBar''s Setup.exe'"
        );
    }
}
