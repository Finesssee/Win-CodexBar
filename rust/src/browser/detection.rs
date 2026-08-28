//! Browser detection for Windows and WSL
//!
//! On native Windows, uses standard AppData paths.
//! On WSL, resolves browser paths via /mnt/c/ to access Windows browser data.

#![allow(
    dead_code,
    reason = "browser detection types reserved for future cookie-based session management"
)]

use std::path::{Path, PathBuf};

use crate::wsl;

/// Supported browser types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrowserType {
    Chrome,
    Edge,
    Brave,
    Arc,
    Firefox,
    Chromium,
}

impl BrowserType {
    /// Get all browser types
    pub fn all() -> &'static [BrowserType] {
        &[
            BrowserType::Chrome,
            BrowserType::Edge,
            BrowserType::Brave,
            BrowserType::Arc,
            BrowserType::Firefox,
            BrowserType::Chromium,
        ]
    }

    /// Check if this is a Chromium-based browser
    pub fn is_chromium_based(&self) -> bool {
        !matches!(self, BrowserType::Firefox)
    }

    /// Get the display name
    pub fn display_name(&self) -> &'static str {
        match self {
            BrowserType::Chrome => "Google Chrome",
            BrowserType::Edge => "Microsoft Edge",
            BrowserType::Brave => "Brave",
            BrowserType::Arc => "Arc",
            BrowserType::Firefox => "Firefox",
            BrowserType::Chromium => "Chromium",
        }
    }
}

/// A detected browser installation
#[derive(Debug, Clone)]
pub struct DetectedBrowser {
    pub browser_type: BrowserType,
    pub user_data_dir: PathBuf,
    pub profiles: Vec<BrowserProfile>,
}

/// A browser profile
#[derive(Debug, Clone)]
pub struct BrowserProfile {
    pub name: String,
    pub path: PathBuf,
    pub is_default: bool,
}

impl BrowserProfile {
    /// Get the cookies database path for Chromium browsers
    pub fn cookies_db_path(&self) -> PathBuf {
        self.path.join("Network").join("Cookies")
    }

    /// Get the Local State file path (contains encryption key)
    pub fn local_state_path(&self, user_data_dir: &Path) -> PathBuf {
        user_data_dir.join("Local State")
    }
}

/// Browser detector for Windows and WSL
pub struct BrowserDetector;

impl BrowserDetector {
    /// Detect all installed browsers.
    ///
    /// On native Windows, scans standard AppData directories.
    /// On WSL, also scans Windows browser paths via /mnt/c/.
    pub fn detect_all() -> Vec<DetectedBrowser> {
        let mut browsers = Vec::new();

        for browser_type in BrowserType::all() {
            if let Some(browser) = Self::detect(*browser_type) {
                browsers.push(browser);
            }
        }

        if wsl::is_wsl() {
            let wsl_browsers = super::wsl_paths::WslBrowserDetector::detect_all();
            for wsl_browser in wsl_browsers {
                let already_found = browsers
                    .iter()
                    .any(|b| b.browser_type == wsl_browser.browser_type);
                if !already_found {
                    browsers.push(wsl_browser);
                }
            }
        }

        browsers
    }

    /// Detect a specific browser
    pub fn detect(browser_type: BrowserType) -> Option<DetectedBrowser> {
        let user_data_dir = Self::get_user_data_dir(browser_type)?;

        if !user_data_dir.exists() {
            return None;
        }

        let profiles = Self::detect_profiles(browser_type, &user_data_dir);

        if profiles.is_empty() {
            return None;
        }

        Some(DetectedBrowser {
            browser_type,
            user_data_dir,
            profiles,
        })
    }

    /// Get the user data directory for a browser
    fn get_user_data_dir(browser_type: BrowserType) -> Option<PathBuf> {
        // In WSL, prefer Windows AppData paths when available
        if wsl::is_wsl()
            && let Some(appdata_local) = wsl::windows_appdata_local()
        {
            let path = match browser_type {
                BrowserType::Chrome => Some(
                    appdata_local
                        .join("Google")
                        .join("Chrome")
                        .join("User Data"),
                ),
                BrowserType::Edge => Some(
                    appdata_local
                        .join("Microsoft")
                        .join("Edge")
                        .join("User Data"),
                ),
                BrowserType::Brave => Some(
                    appdata_local
                        .join("BraveSoftware")
                        .join("Brave-Browser")
                        .join("User Data"),
                ),
                BrowserType::Arc => Some(appdata_local.join("Arc").join("User Data")),
                BrowserType::Chromium => Some(appdata_local.join("Chromium").join("User Data")),
                BrowserType::Firefox => wsl::windows_appdata_roaming()
                    .map(|roaming| roaming.join("Mozilla").join("Firefox").join("Profiles")),
            };
            if let Some(ref p) = path
                && p.exists()
            {
                return path;
            }
        }

        let local_app_data = dirs::data_local_dir()?;
        let app_data = dirs::data_dir()?;

        let path = match browser_type {
            BrowserType::Chrome => local_app_data
                .join("Google")
                .join("Chrome")
                .join("User Data"),
            BrowserType::Edge => local_app_data
                .join("Microsoft")
                .join("Edge")
                .join("User Data"),
            BrowserType::Brave => local_app_data
                .join("BraveSoftware")
                .join("Brave-Browser")
                .join("User Data"),
            BrowserType::Arc => local_app_data.join("Arc").join("User Data"),
            BrowserType::Chromium => local_app_data.join("Chromium").join("User Data"),
            BrowserType::Firefox => app_data.join("Mozilla").join("Firefox").join("Profiles"),
        };

        Some(path)
    }

    /// Detect profiles within a browser's user data directory
    fn detect_profiles(browser_type: BrowserType, user_data_dir: &PathBuf) -> Vec<BrowserProfile> {
        if browser_type == BrowserType::Firefox {
            return Self::detect_firefox_profiles(user_data_dir);
        }

        Self::detect_chromium_profiles(user_data_dir)
    }

    /// Detect Chromium-based browser profiles
    fn detect_chromium_profiles(user_data_dir: &PathBuf) -> Vec<BrowserProfile> {
        let mut profiles = Vec::new();

        // Default profile
        let default_path = user_data_dir.join("Default");
        if default_path.exists() {
            profiles.push(BrowserProfile {
                name: "Default".to_string(),
                path: default_path,
                is_default: true,
            });
        }

        // Additional profiles (Profile 1, Profile 2, etc.)
        if let Ok(entries) = std::fs::read_dir(user_data_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("Profile ") {
                    let path = entry.path();
                    if path.is_dir() {
                        profiles.push(BrowserProfile {
                            name,
                            path,
                            is_default: false,
                        });
                    }
                }
            }
        }

        profiles
    }

    /// Detect Firefox profiles
    fn detect_firefox_profiles(profiles_dir: &PathBuf) -> Vec<BrowserProfile> {
        let mut profiles = Vec::new();

        if let Ok(entries) = std::fs::read_dir(profiles_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let path = entry.path();

                // Firefox profiles are named like "abcd1234.default" or "abcd1234.default-release"
                if path.is_dir() && name.contains('.') {
                    let is_default = name.contains("default");
                    profiles.push(BrowserProfile {
                        name,
                        path,
                        is_default,
                    });
                }
            }
        }

        profiles
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_browser_detection() {
        let browsers = BrowserDetector::detect_all();
        println!("Detected {} browsers", browsers.len());
        for browser in &browsers {
            println!(
                "  {} at {:?} ({} profiles)",
                browser.browser_type.display_name(),
                browser.user_data_dir,
                browser.profiles.len()
            );
        }
    }
}
