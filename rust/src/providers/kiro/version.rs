//! Kiro CLI Version Detection
//!
//! Detect and parse Kiro CLI version for compatibility checks.

#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Cached CLI path
static CLI_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Cached CLI version
static CLI_VERSION: OnceLock<Option<String>> = OnceLock::new();
const KIRO_CLI_PATH_ENV: &str = "CODEXBAR_KIRO_CLI_PATH";

fn is_allowed_kiro_binary(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }

    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    #[cfg(target_os = "windows")]
    {
        file_name.eq_ignore_ascii_case("kiro-cli.exe")
    }

    #[cfg(not(target_os = "windows"))]
    {
        file_name == "kiro-cli" || file_name == "kiro"
    }
}

fn env_override_cli_path() -> Option<PathBuf> {
    let raw = std::env::var(KIRO_CLI_PATH_ENV).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let path = PathBuf::from(trimmed);
    if is_allowed_kiro_binary(&path) {
        return Some(path);
    }

    None
}

/// Kiro CLI version info
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KiroVersion {
    /// Major version number
    pub major: u32,
    /// Minor version number
    pub minor: u32,
    /// Patch version number
    pub patch: u32,
    /// Pre-release suffix (e.g., "beta.1")
    pub prerelease: Option<String>,
    /// Build metadata
    pub build: Option<String>,
    /// Raw version string
    pub raw: String,
}

impl KiroVersion {
    /// Parse a version string
    pub fn parse(version: &str) -> Option<Self> {
        let trimmed = version.trim();
        if trimmed.is_empty() {
            return None;
        }

        // Handle "kiro-cli X.Y.Z" prefix
        let version_part = if trimmed.to_lowercase().starts_with("kiro-cli ") {
            &trimmed[9..]
        } else if trimmed.to_lowercase().starts_with("kiro ") {
            &trimmed[5..]
        } else {
            trimmed
        }
        .trim();

        // Split off pre-release and build metadata
        let (version_core, prerelease, build) = Self::split_version_parts(version_part);

        // Parse X.Y.Z
        let parts: Vec<&str> = version_core.split('.').collect();
        if parts.is_empty() {
            return None;
        }

        let major = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
        let minor = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        let patch = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);

        Some(KiroVersion {
            major,
            minor,
            patch,
            prerelease,
            build,
            raw: trimmed.to_string(),
        })
    }

    /// Split version into core, prerelease, and build parts
    fn split_version_parts(version: &str) -> (String, Option<String>, Option<String>) {
        let mut core = version.to_string();
        let mut prerelease = None;
        let mut build = None;

        // Extract build metadata first (after +)
        if let Some(plus_idx) = core.find('+') {
            build = Some(core[plus_idx + 1..].to_string());
            core = core[..plus_idx].to_string();
        }

        // Extract prerelease (after -)
        if let Some(dash_idx) = core.find('-') {
            prerelease = Some(core[dash_idx + 1..].to_string());
            core = core[..dash_idx].to_string();
        }

        (core, prerelease, build)
    }

    /// Check if this version is at least the specified version
    pub fn at_least(&self, major: u32, minor: u32, patch: u32) -> bool {
        if self.major > major {
            return true;
        }
        if self.major < major {
            return false;
        }
        if self.minor > minor {
            return true;
        }
        if self.minor < minor {
            return false;
        }
        self.patch >= patch
    }

    /// Check if this is a prerelease version
    pub fn is_prerelease(&self) -> bool {
        self.prerelease.is_some()
    }

    /// Get display string
    pub fn display(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl std::fmt::Display for KiroVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.raw)
    }
}

impl PartialOrd for KiroVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for KiroVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match self.major.cmp(&other.major) {
            std::cmp::Ordering::Equal => {}
            ord => return ord,
        }
        match self.minor.cmp(&other.minor) {
            std::cmp::Ordering::Equal => {}
            ord => return ord,
        }
        self.patch.cmp(&other.patch)
    }
}

/// Find Kiro CLI binary path
pub fn find_kiro_cli() -> Option<PathBuf> {
    CLI_PATH
        .get_or_init(|| {
            // 1. Check explicit environment override first
            if let Some(path) = env_override_cli_path() {
                return Some(path);
            }

            // 2. Hardened PATH lookup - use which but validate the result
            //    (avoids CWD hijacking by not executing bare command names)
            if let Ok(path) = which::which("kiro-cli")
                && is_allowed_kiro_binary(&path)
            {
                return Some(path);
            }
            if let Ok(path) = which::which("kiro")
                && is_allowed_kiro_binary(&path)
            {
                return Some(path);
            }

            // 3. Fall back to known install locations
            #[cfg(target_os = "windows")]
            {
                let possible_paths = [
                    dirs::data_local_dir()
                        .map(|p| p.join("Programs").join("Kiro").join("kiro-cli.exe")),
                    Some(PathBuf::from("C:\\Program Files\\Kiro\\kiro-cli.exe")),
                ];
                for path in possible_paths.into_iter().flatten() {
                    if is_allowed_kiro_binary(&path) {
                        return Some(path);
                    }
                }
            }

            None
        })
        .clone()
}

/// Detect Kiro CLI version
pub fn detect_version() -> Option<String> {
    CLI_VERSION
        .get_or_init(|| {
            let cli_path = find_kiro_cli()?;

            #[cfg(windows)]
            const CREATE_NO_WINDOW: u32 = 0x08000000;

            let mut cmd = Command::new(&cli_path);
            cmd.arg("--version");
            #[cfg(windows)]
            cmd.creation_flags(CREATE_NO_WINDOW);

            let output = cmd.output().ok()?;

            if !output.status.success() {
                return None;
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = if stdout.trim().is_empty() {
                stderr.to_string()
            } else {
                stdout.to_string()
            };

            let trimmed = combined.trim();
            if trimmed.is_empty() {
                return None;
            }

            // Output is like "kiro-cli 1.23.1" or just "1.23.1"
            let version = if trimmed.to_lowercase().starts_with("kiro-cli ") {
                trimmed[9..].trim().to_string()
            } else if trimmed.to_lowercase().starts_with("kiro ") {
                trimmed[5..].trim().to_string()
            } else {
                trimmed.to_string()
            };

            Some(version)
        })
        .clone()
}

/// Get parsed Kiro version
pub fn get_version() -> Option<KiroVersion> {
    detect_version().and_then(|v| KiroVersion::parse(&v))
}

/// Check if Kiro CLI is installed
pub fn is_installed() -> bool {
    find_kiro_cli().is_some()
}

/// Check if the installed Kiro CLI version is compatible
pub fn is_compatible(min_major: u32, min_minor: u32, min_patch: u32) -> bool {
    match get_version() {
        Some(v) => v.at_least(min_major, min_minor, min_patch),
        None => false,
    }
}

/// Reset cached values (for testing)
#[cfg(test)]
pub fn reset_cache() {
    // Can't reset OnceLock in stable Rust, this is just for documentation
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn temp_binary_path(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("codexbar-kiro-version-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp binary directory");
        let path = dir.join(name);
        File::create(&path).expect("create temp binary placeholder");
        path
    }

    #[test]
    fn test_parse_version_simple() {
        let v = KiroVersion::parse("1.23.4").unwrap();
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 23);
        assert_eq!(v.patch, 4);
        assert!(v.prerelease.is_none());
    }

    #[test]
    fn test_parse_version_with_prefix() {
        let v = KiroVersion::parse("kiro-cli 1.2.3").unwrap();
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);

        let v = KiroVersion::parse("Kiro 2.0.0").unwrap();
        assert_eq!(v.major, 2);
    }

    #[test]
    fn test_parse_version_with_prerelease() {
        let v = KiroVersion::parse("1.0.0-beta.1").unwrap();
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 0);
        assert_eq!(v.patch, 0);
        assert_eq!(v.prerelease, Some("beta.1".to_string()));
        assert!(v.is_prerelease());
    }

    #[test]
    fn test_parse_version_with_build() {
        let v = KiroVersion::parse("1.0.0+build123").unwrap();
        assert_eq!(v.build, Some("build123".to_string()));
    }

    #[test]
    fn test_version_comparison() {
        let v1 = KiroVersion::parse("1.2.3").unwrap();
        let v2 = KiroVersion::parse("1.2.4").unwrap();
        let v3 = KiroVersion::parse("1.3.0").unwrap();
        let v4 = KiroVersion::parse("2.0.0").unwrap();

        assert!(v1 < v2);
        assert!(v2 < v3);
        assert!(v3 < v4);
        assert!(v1 < v4);
    }

    #[test]
    fn test_at_least() {
        let v = KiroVersion::parse("1.5.2").unwrap();

        assert!(v.at_least(1, 5, 2));
        assert!(v.at_least(1, 5, 0));
        assert!(v.at_least(1, 4, 0));
        assert!(v.at_least(0, 9, 0));

        assert!(!v.at_least(1, 5, 3));
        assert!(!v.at_least(1, 6, 0));
        assert!(!v.at_least(2, 0, 0));
    }

    #[test]
    fn test_display() {
        let v = KiroVersion::parse("1.2.3-beta+build").unwrap();
        assert_eq!(v.display(), "1.2.3");
        assert_eq!(v.to_string(), "1.2.3-beta+build");
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_rejects_gui_kiro_binary_as_cli() {
        let cli_path = temp_binary_path("kiro-cli.exe");
        let gui_path = temp_binary_path("kiro.exe");

        assert!(is_allowed_kiro_binary(&cli_path));
        assert!(
            !is_allowed_kiro_binary(&gui_path),
            "kiro.exe is the Electron GUI app on Windows; running it as a CLI spawns the IDE"
        );

        // Best-effort cleanup of temp binaries; leftover files are harmless.
        let _removed_cli = std::fs::remove_file(cli_path);
        let _removed_gui = std::fs::remove_file(gui_path);
    }
}
