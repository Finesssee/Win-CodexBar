//! MiniMax LocalStorage Importer
//!
//! Extracts session data from browser localStorage for MiniMax platform.
//! Supports Chrome, Edge, Firefox, and Brave browsers.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Session data extracted from MiniMax localStorage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MiniMaxSession {
    pub access_token: Option<String>,
    pub user_id: Option<String>,
    pub group_id: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub plan_type: Option<String>,
    pub source_label: String,
}

/// Error type for localStorage import
#[derive(Debug)]
pub enum ImportError {
    BrowserNotFound,
    StorageNotFound,
    ParseError(String),
    AccessDenied(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::BrowserNotFound => write!(f, "No supported browser found"),
            ImportError::StorageNotFound => write!(f, "localStorage data not found"),
            ImportError::ParseError(e) => write!(f, "Parse error: {}", e),
            ImportError::AccessDenied(e) => write!(f, "Access denied: {}", e),
        }
    }
}

impl std::error::Error for ImportError {}

/// MiniMax localStorage importer
pub struct MiniMaxLocalStorageImporter;

impl MiniMaxLocalStorageImporter {
    /// Import MiniMax session from browser localStorage
    pub fn import_session() -> Result<MiniMaxSession, ImportError> {
        // Try browsers in order of preference
        let browsers = Self::get_browser_paths();

        for (browser_name, ls_path) in browsers {
            if let Ok(session) = Self::extract_from_path(&ls_path, &browser_name) {
                return Ok(session);
            }
        }

        Err(ImportError::BrowserNotFound)
    }

    /// Get paths to browser localStorage databases
    fn get_browser_paths() -> Vec<(String, PathBuf)> {
        #[allow(
            unused_mut,
            reason = "mutability needed for conditional initialization that the compiler cannot prove"
        )]
        let mut paths = Vec::new();

        #[cfg(target_os = "windows")]
        {
            if let Some(local_data) = dirs::data_local_dir() {
                // Chrome
                let chrome_path = local_data
                    .join("Google")
                    .join("Chrome")
                    .join("User Data")
                    .join("Default")
                    .join("Local Storage")
                    .join("leveldb");
                if chrome_path.exists() {
                    paths.push(("Chrome".to_string(), chrome_path));
                }

                // Edge
                let edge_path = local_data
                    .join("Microsoft")
                    .join("Edge")
                    .join("User Data")
                    .join("Default")
                    .join("Local Storage")
                    .join("leveldb");
                if edge_path.exists() {
                    paths.push(("Edge".to_string(), edge_path));
                }

                // Brave
                let brave_path = local_data
                    .join("BraveSoftware")
                    .join("Brave-Browser")
                    .join("User Data")
                    .join("Default")
                    .join("Local Storage")
                    .join("leveldb");
                if brave_path.exists() {
                    paths.push(("Brave".to_string(), brave_path));
                }
            }
        }

        #[cfg(target_os = "macos")]
        {
            if let Some(home) = dirs::home_dir() {
                // Chrome
                let chrome_path = home
                    .join("Library")
                    .join("Application Support")
                    .join("Google")
                    .join("Chrome")
                    .join("Default")
                    .join("Local Storage")
                    .join("leveldb");
                if chrome_path.exists() {
                    paths.push(("Chrome".to_string(), chrome_path));
                }

                // Edge
                let edge_path = home
                    .join("Library")
                    .join("Application Support")
                    .join("Microsoft Edge")
                    .join("Default")
                    .join("Local Storage")
                    .join("leveldb");
                if edge_path.exists() {
                    paths.push(("Edge".to_string(), edge_path));
                }
            }
        }

        paths
    }

    /// Extract session from a localStorage path
    fn extract_from_path(
        path: &PathBuf,
        browser_name: &str,
    ) -> Result<MiniMaxSession, ImportError> {
        // Look for .ldb or .log files
        let entries =
            std::fs::read_dir(path).map_err(|e| ImportError::AccessDenied(e.to_string()))?;

        let mut minimax_data: Option<serde_json::Value> = None;

        for entry in entries.flatten() {
            let entry_path = entry.path();
            if let Some(ext) = entry_path.extension()
                && (ext == "ldb" || ext == "log")
            {
                // Read file and search for MiniMax data
                if let Ok(contents) = std::fs::read(&entry_path) {
                    // Search for MiniMax-related JSON in the binary content
                    if let Some(data) = Self::extract_minimax_json(&contents) {
                        minimax_data = Some(data);
                        break;
                    }
                }
            }
        }

        match minimax_data {
            Some(json) => Self::parse_session_from_json(&json, browser_name),
            None => Err(ImportError::StorageNotFound),
        }
    }

    /// Extract MiniMax JSON from binary localStorage data
    fn extract_minimax_json(data: &[u8]) -> Option<serde_json::Value> {
        // Convert to string, handling binary data
        let content = String::from_utf8_lossy(data);

        // Look for patterns that indicate MiniMax session data
        let patterns = [
            "minimax_user",
            "minimax_session",
            "platform.minimaxi.com",
            "mm_token",
            "mm_user_info",
        ];

        for pattern in patterns {
            if let Some(parsed) = Self::extract_json_after_pattern(&content, pattern) {
                return Some(parsed);
            }
        }

        None
    }

    fn extract_json_after_pattern(content: &str, pattern: &str) -> Option<serde_json::Value> {
        let json_start = Self::json_start_after_pattern(content, pattern)?;
        let remaining = &content[json_start..];
        let end_idx = Self::matching_json_object_end(remaining)?;
        serde_json::from_str(&remaining[..end_idx]).ok()
    }

    fn json_start_after_pattern(content: &str, pattern: &str) -> Option<usize> {
        let start_idx = content.find(pattern)?;
        content[start_idx..]
            .find('{')
            .map(|offset| start_idx + offset)
    }

    fn matching_json_object_end(content: &str) -> Option<usize> {
        let mut depth = 0;
        for (i, c) in content.char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Parse session from extracted JSON
    fn parse_session_from_json(
        json: &serde_json::Value,
        browser_name: &str,
    ) -> Result<MiniMaxSession, ImportError> {
        let access_token = json
            .get("access_token")
            .or_else(|| json.get("token"))
            .or_else(|| json.get("mm_token"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let user_id = json
            .get("user_id")
            .or_else(|| json.get("userId"))
            .or_else(|| json.get("id"))
            .and_then(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .or_else(|| v.as_i64().map(|n| n.to_string()))
            });

        let group_id = json
            .get("group_id")
            .or_else(|| json.get("groupId"))
            .and_then(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .or_else(|| v.as_i64().map(|n| n.to_string()))
            });

        let email = json
            .get("email")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let phone = json
            .get("phone")
            .or_else(|| json.get("mobile"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let plan_type = json
            .get("plan_type")
            .or_else(|| json.get("planType"))
            .or_else(|| json.get("plan"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Need at least access_token or user_id
        if access_token.is_none() && user_id.is_none() {
            return Err(ImportError::ParseError(
                "No valid session data found".to_string(),
            ));
        }

        Ok(MiniMaxSession {
            access_token,
            user_id,
            group_id,
            email,
            phone,
            plan_type,
            source_label: format!("{} localStorage", browser_name),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_session_json() {
        let json = serde_json::json!({
            "access_token": "test_token",
            "user_id": "12345",
            "group_id": "67890",
            "email": "test@example.com"
        });

        let result = MiniMaxLocalStorageImporter::parse_session_from_json(&json, "Chrome");
        assert!(result.is_ok());

        let session = result.unwrap();
        assert_eq!(session.access_token, Some("test_token".to_string()));
        assert_eq!(session.user_id, Some("12345".to_string()));
    }

    #[test]
    fn test_parse_session_empty() {
        let json = serde_json::json!({
            "foo": "bar"
        });

        let result = MiniMaxLocalStorageImporter::parse_session_from_json(&json, "Chrome");
        assert!(result.is_err());
    }
}
