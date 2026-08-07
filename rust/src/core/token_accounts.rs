//! Token Account Multi-Support
//!
//! Store and manage multiple accounts/tokens per provider.
//! Supports parallel fetching and account switching.

use crate::core::ProviderId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use uuid::Uuid;

/// How to inject a token into a fetch request
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TokenInjection {
    /// Inject as Cookie header value
    CookieHeader,
    /// Inject as environment variable
    Environment { key: String },
}

/// Support definition for a provider's token accounts
#[derive(Debug, Clone)]
pub struct TokenAccountSupport {
    /// Display title for the UI
    pub title: &'static str,
    /// Subtitle/description for the UI
    pub subtitle: &'static str,
    /// Placeholder text for input field
    pub placeholder: &'static str,
    /// How tokens are injected
    pub injection: TokenInjection,
    /// Whether manual cookie source is required
    pub requires_manual_cookie_source: bool,
    /// Cookie name to use when normalizing (e.g., "sessionKey")
    pub cookie_name: Option<&'static str>,
}

impl TokenAccountSupport {
    /// Get token account support for a provider
    pub fn for_provider(provider: ProviderId) -> Option<Self> {
        match provider {
            ProviderId::Claude => Some(TokenAccountSupport {
                title: "Session tokens",
                subtitle: "Store Claude sessionKey cookies for settings-page usage. OAuth tokens are kept as a legacy fallback.",
                placeholder: "Paste sessionKey value or Cookie: sessionKey=...",
                injection: TokenInjection::CookieHeader,
                requires_manual_cookie_source: true,
                cookie_name: Some("sessionKey"),
            }),
            ProviderId::Zai => Some(TokenAccountSupport {
                title: "API tokens",
                subtitle: "Stored locally in token-accounts.json. Team usage can use workspace_id as organization|project.",
                placeholder: "Paste token...",
                injection: TokenInjection::Environment {
                    key: "Z_AI_API_KEY".to_string(),
                },
                requires_manual_cookie_source: false,
                cookie_name: None,
            }),
            ProviderId::Cursor => Some(TokenAccountSupport {
                title: "Session tokens",
                subtitle: "Store multiple Cursor Cookie headers.",
                placeholder: "Cookie: ...",
                injection: TokenInjection::CookieHeader,
                requires_manual_cookie_source: true,
                cookie_name: None,
            }),
            ProviderId::OpenCode => Some(TokenAccountSupport {
                title: "Session tokens",
                subtitle: "Store multiple OpenCode Cookie headers.",
                placeholder: "Cookie: ...",
                injection: TokenInjection::CookieHeader,
                requires_manual_cookie_source: true,
                cookie_name: None,
            }),
            ProviderId::Factory => Some(TokenAccountSupport {
                title: "Session tokens",
                subtitle: "Store multiple Factory Cookie headers.",
                placeholder: "Cookie: ...",
                injection: TokenInjection::CookieHeader,
                requires_manual_cookie_source: true,
                cookie_name: None,
            }),
            ProviderId::Alibaba => Some(TokenAccountSupport {
                title: "Session tokens",
                subtitle: "Store multiple Alibaba Cookie headers.",
                placeholder: "Cookie: ...",
                injection: TokenInjection::CookieHeader,
                requires_manual_cookie_source: true,
                cookie_name: None,
            }),
            ProviderId::AlibabaTokenPlan => Some(TokenAccountSupport {
                title: "Session tokens",
                subtitle: "Store multiple Alibaba Token Plan Cookie headers.",
                placeholder: "Cookie: cna=...; login_aliyunid_csrf=...",
                injection: TokenInjection::CookieHeader,
                requires_manual_cookie_source: true,
                cookie_name: None,
            }),
            ProviderId::MiniMax => Some(TokenAccountSupport {
                title: "Session tokens",
                subtitle: "Store multiple MiniMax Cookie headers.",
                placeholder: "Cookie: ...",
                injection: TokenInjection::CookieHeader,
                requires_manual_cookie_source: true,
                cookie_name: None,
            }),
            ProviderId::Augment => Some(TokenAccountSupport {
                title: "Session tokens",
                subtitle: "Store multiple Augment Cookie headers.",
                placeholder: "Cookie: ...",
                injection: TokenInjection::CookieHeader,
                requires_manual_cookie_source: true,
                cookie_name: None,
            }),
            ProviderId::Amp => Some(TokenAccountSupport {
                title: "Session tokens",
                subtitle: "Store multiple Amp Cookie headers.",
                placeholder: "Cookie: ...",
                injection: TokenInjection::CookieHeader,
                requires_manual_cookie_source: true,
                cookie_name: None,
            }),
            ProviderId::Ollama => Some(TokenAccountSupport {
                title: "Session tokens",
                subtitle: "Store multiple Ollama Cookie headers or __Secure-session values.",
                placeholder: "__Secure-session value or Cookie: ...",
                injection: TokenInjection::CookieHeader,
                requires_manual_cookie_source: true,
                cookie_name: Some("__Secure-session"),
            }),
            ProviderId::T3Chat => Some(TokenAccountSupport {
                title: "Session tokens",
                subtitle: "Store multiple T3 Chat Cookie headers or full browser cURL captures.",
                placeholder: "Cookie: ... or curl ... -H 'Cookie: ...'",
                injection: TokenInjection::CookieHeader,
                requires_manual_cookie_source: true,
                cookie_name: None,
            }),
            ProviderId::ZoomMate => Some(TokenAccountSupport {
                title: "Session tokens",
                subtitle: "Store multiple ZoomMate Cookie headers or credits/status cURL captures.",
                placeholder: "Cookie: ... or curl 'https://ai.zoom.us/.../credits/status' -H 'Authorization: Bearer ...'",
                injection: TokenInjection::CookieHeader,
                requires_manual_cookie_source: true,
                cookie_name: None,
            }),
            ProviderId::Mistral => Some(TokenAccountSupport {
                title: "Session tokens",
                subtitle: "Store multiple Mistral Cookie headers.",
                placeholder: "Cookie: ...",
                injection: TokenInjection::CookieHeader,
                requires_manual_cookie_source: true,
                cookie_name: None,
            }),
            ProviderId::Manus => Some(TokenAccountSupport {
                title: "Session tokens",
                subtitle: "Store multiple Manus session_id values.",
                placeholder: "session_id value or Cookie: ...",
                injection: TokenInjection::CookieHeader,
                requires_manual_cookie_source: true,
                cookie_name: Some("session_id"),
            }),
            ProviderId::MiMo => Some(TokenAccountSupport {
                title: "Session tokens",
                subtitle: "Store multiple Xiaomi MiMo Cookie headers.",
                placeholder: "Cookie: api-platform_serviceToken=...; userId=...",
                injection: TokenInjection::CookieHeader,
                requires_manual_cookie_source: true,
                cookie_name: None,
            }),
            ProviderId::CommandCode => Some(TokenAccountSupport {
                title: "Session tokens",
                subtitle: "Store multiple Command Code Cookie headers or Better Auth values.",
                placeholder: "Cookie: __Secure-commandcode_prod_.session_token=... or better-auth value",
                injection: TokenInjection::CookieHeader,
                requires_manual_cookie_source: true,
                cookie_name: Some("__Secure-better-auth.session_token"),
            }),
            ProviderId::Qoder => Some(TokenAccountSupport {
                title: "Session tokens",
                subtitle: "Store multiple Qoder Cookie headers.",
                placeholder: "Cookie: ...",
                injection: TokenInjection::CookieHeader,
                requires_manual_cookie_source: true,
                cookie_name: None,
            }),
            ProviderId::CodeBuddy => Some(TokenAccountSupport {
                title: "Session tokens",
                subtitle: "Store CodeBuddy CN Cookie headers (from plans-usage DevTools cURL).",
                placeholder: "Cookie: session=...; ... (or paste full Cookie header)",
                injection: TokenInjection::CookieHeader,
                requires_manual_cookie_source: true,
                cookie_name: None,
            }),
            ProviderId::Sakana => Some(TokenAccountSupport {
                title: "Session tokens",
                subtitle: "Store multiple Sakana Console Cookie headers.",
                placeholder: "Cookie: ...",
                injection: TokenInjection::CookieHeader,
                requires_manual_cookie_source: true,
                cookie_name: None,
            }),
            ProviderId::Notion => Some(TokenAccountSupport {
                title: "Session tokens",
                subtitle: "Store multiple Notion Cookie headers or token_v2 values.",
                placeholder: "Cookie: token_v2=... or paste the token_v2 value",
                injection: TokenInjection::CookieHeader,
                requires_manual_cookie_source: true,
                cookie_name: Some("token_v2"),
            }),
            ProviderId::Sub2Api => Some(TokenAccountSupport {
                title: "Group API keys",
                subtitle: "Store multiple sub2api group API keys with labels such as Claude, Codex, or Gemini.",
                placeholder: "sk-...",
                injection: TokenInjection::Environment {
                    key: "SUB2API_API_KEY".to_string(),
                },
                requires_manual_cookie_source: false,
                cookie_name: None,
            }),
            ProviderId::DeepInfra => Some(TokenAccountSupport {
                title: "API keys",
                subtitle: "Store multiple DeepInfra API keys.",
                placeholder: "API key from deepinfra.com/dash",
                injection: TokenInjection::Environment {
                    key: "DEEPINFRA_API_KEY".to_string(),
                },
                requires_manual_cookie_source: false,
                cookie_name: None,
            }),
            ProviderId::AiAnd => Some(TokenAccountSupport {
                title: "API keys",
                subtitle: "Store multiple ai& API keys.",
                placeholder: "API key from console.aiand.com",
                injection: TokenInjection::Environment {
                    key: "AIAND_API_KEY".to_string(),
                },
                requires_manual_cookie_source: false,
                cookie_name: None,
            }),
            ProviderId::ZenMux => Some(TokenAccountSupport {
                title: "API keys",
                subtitle: "Store multiple ZenMux Management API keys.",
                placeholder: "Management API key",
                injection: TokenInjection::Environment {
                    key: "ZENMUX_MANAGEMENT_API_KEY".to_string(),
                },
                requires_manual_cookie_source: false,
                cookie_name: None,
            }),
            ProviderId::ClinePass => Some(TokenAccountSupport {
                title: "API keys",
                subtitle: "Store multiple ClinePass API keys.",
                placeholder: "API key",
                injection: TokenInjection::Environment {
                    key: "CLINEPASS_API_KEY".to_string(),
                },
                requires_manual_cookie_source: false,
                cookie_name: None,
            }),
            ProviderId::Neuralwatt => Some(TokenAccountSupport {
                title: "API keys",
                subtitle: "Store multiple Neuralwatt API keys.",
                placeholder: "API key",
                injection: TokenInjection::Environment {
                    key: "NEURALWATT_API_KEY".to_string(),
                },
                requires_manual_cookie_source: false,
                cookie_name: None,
            }),
            ProviderId::Xai => Some(TokenAccountSupport {
                title: "Management API keys",
                subtitle: "Store multiple xAI Management API keys. Team ID is set separately under provider settings.",
                placeholder: "xai-... Management API key from console.x.ai",
                injection: TokenInjection::Environment {
                    key: "XAI_MANAGEMENT_API_KEY".to_string(),
                },
                requires_manual_cookie_source: false,
                cookie_name: None,
            }),
            // Upstream 0.45 #2271: labeled OpenRouter API keys via token accounts.
            ProviderId::OpenRouter => Some(TokenAccountSupport {
                title: "API keys",
                subtitle: "Store multiple OpenRouter API keys.",
                placeholder: "sk-or-v1-...",
                injection: TokenInjection::Environment {
                    key: "OPENROUTER_API_KEY".to_string(),
                },
                requires_manual_cookie_source: false,
                cookie_name: None,
            }),
            ProviderId::Copilot => Some(TokenAccountSupport {
                title: "GitHub accounts",
                subtitle: "Store GitHub OAuth tokens for Copilot plan usage.",
                placeholder: "Sign in with GitHub or paste a GitHub OAuth token...",
                injection: TokenInjection::Environment {
                    key: "GITHUB_TOKEN".to_string(),
                },
                requires_manual_cookie_source: false,
                cookie_name: None,
            }),
            // These providers don't support token accounts
            ProviderId::Codex
            | ProviderId::Gemini
            | ProviderId::Antigravity
            | ProviderId::Kiro
            | ProviderId::VertexAI
            | ProviderId::Kimi
            | ProviderId::KimiK2
            | ProviderId::JetBrains
            | ProviderId::Warp
            | ProviderId::AzureOpenAI
            | ProviderId::NanoGPT
            | ProviderId::Infini
            | ProviderId::Perplexity
            | ProviderId::Abacus
            | ProviderId::OpenCodeGo
            | ProviderId::Kilo
            | ProviderId::Bedrock
            | ProviderId::Codebuff
            | ProviderId::DeepSeek
            | ProviderId::Windsurf
            | ProviderId::Doubao
            | ProviderId::Crof
            | ProviderId::StepFun
            | ProviderId::Venice
            | ProviderId::OpenAIApi
            | ProviderId::Grok
            | ProviderId::ElevenLabs
            | ProviderId::Deepgram
            | ProviderId::Groq
            | ProviderId::LLMProxy
            | ProviderId::Chutes
            | ProviderId::LiteLLM
            | ProviderId::Poe
            | ProviderId::Devin
            | ProviderId::Zed
            | ProviderId::CrossModel
            | ProviderId::LongCat
            | ProviderId::Wayfinder
            | ProviderId::QwenCloud => None,
        }
    }

    /// Check if a provider supports token accounts
    pub fn is_supported(provider: ProviderId) -> bool {
        Self::for_provider(provider).is_some()
    }

    /// Get environment override for a token
    pub fn env_override(provider: ProviderId, token: &str) -> Option<HashMap<String, String>> {
        let support = Self::for_provider(provider)?;
        match &support.injection {
            TokenInjection::Environment { key } => {
                let mut map = HashMap::new();
                map.insert(key.clone(), token.to_string());
                Some(map)
            }
            TokenInjection::CookieHeader => {
                // Check for Claude OAuth token
                if provider == ProviderId::Claude
                    && let Some(normalized) = Self::normalized_claude_oauth_token(token)
                    && Self::is_claude_oauth_token(&normalized)
                {
                    let mut map = HashMap::new();
                    map.insert("CODEXBAR_CLAUDE_OAUTH_TOKEN".to_string(), normalized);
                    return Some(map);
                }
                None
            }
        }
    }

    /// Normalize a cookie header for a provider
    pub fn normalized_cookie_header(provider: ProviderId, token: &str) -> String {
        let trimmed = token.trim();
        let Some(support) = Self::for_provider(provider) else {
            return trimmed.to_string();
        };

        let Some(cookie_name) = support.cookie_name else {
            return trimmed.to_string();
        };

        let lower = trimmed.to_lowercase();
        if lower.contains("cookie:") || trimmed.contains('=') {
            return trimmed.to_string();
        }

        format!("{}={}", cookie_name, trimmed)
    }

    /// Check if a token is a Claude OAuth token
    pub fn is_claude_oauth_token(token: &str) -> bool {
        let Some(trimmed) = Self::normalized_claude_oauth_token(token) else {
            return false;
        };
        let lower = trimmed.to_lowercase();
        if lower.contains("cookie:") || trimmed.contains('=') {
            return false;
        }
        lower.starts_with("sk-ant-oat")
    }

    /// Normalize a Claude OAuth token
    fn normalized_claude_oauth_token(token: &str) -> Option<String> {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            return None;
        }
        let lower = trimmed.to_lowercase();
        if lower.starts_with("bearer ") {
            Some(trimmed[7..].trim().to_string())
        } else {
            Some(trimmed.to_string())
        }
    }
}

/// A single token account for a provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenAccount {
    /// Unique identifier
    pub id: Uuid,
    /// User-provided label
    pub label: String,
    /// The token/cookie value
    pub token: String,
    /// When this account was added (Unix timestamp in seconds)
    pub added_at: i64,
    /// When this account was last used (Unix timestamp in seconds)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<i64>,
}

impl TokenAccount {
    /// Create a new token account
    pub fn new(label: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            label: label.into(),
            token: token.into(),
            added_at: Utc::now().timestamp(),
            last_used: None,
        }
    }

    /// Mark this account as used
    pub fn mark_used(&mut self) {
        self.last_used = Some(Utc::now().timestamp());
    }

    /// Get display name
    pub fn display_name(&self) -> &str {
        &self.label
    }

    /// Get added_at as DateTime
    pub fn added_at_datetime(&self) -> DateTime<Utc> {
        DateTime::from_timestamp(self.added_at, 0).unwrap_or_else(Utc::now)
    }

    /// Get last_used as DateTime
    pub fn last_used_datetime(&self) -> Option<DateTime<Utc>> {
        self.last_used
            .and_then(|ts| DateTime::from_timestamp(ts, 0))
    }
}

/// Account data for a provider
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderAccountData {
    /// File format version
    #[serde(default = "default_version")]
    pub version: u32,
    /// List of accounts
    pub accounts: Vec<TokenAccount>,
    /// Index of the active account
    #[serde(default)]
    pub active_index: usize,
}

fn default_version() -> u32 {
    1
}

impl ProviderAccountData {
    /// Create new empty account data
    pub fn new() -> Self {
        Self {
            version: 1,
            accounts: Vec::new(),
            active_index: 0,
        }
    }

    /// Get the clamped active index
    pub fn clamped_active_index(&self) -> usize {
        if self.accounts.is_empty() {
            return 0;
        }
        self.active_index.min(self.accounts.len() - 1)
    }

    /// Get the active account
    pub fn active_account(&self) -> Option<&TokenAccount> {
        self.accounts.get(self.clamped_active_index())
    }

    /// Get the active account mutably
    pub fn active_account_mut(&mut self) -> Option<&mut TokenAccount> {
        let idx = self.clamped_active_index();
        self.accounts.get_mut(idx)
    }

    /// Add a new account
    pub fn add_account(&mut self, account: TokenAccount) {
        self.accounts.push(account);
    }

    /// Remove an account by ID
    pub fn remove_account(&mut self, id: Uuid) -> Option<TokenAccount> {
        let pos = self.accounts.iter().position(|a| a.id == id)?;
        let removed = self.accounts.remove(pos);
        // Adjust active index if needed
        if self.active_index >= self.accounts.len() && !self.accounts.is_empty() {
            self.active_index = self.accounts.len() - 1;
        }
        Some(removed)
    }

    /// Set the active account by index
    pub fn set_active(&mut self, index: usize) {
        self.active_index = index.min(self.accounts.len().saturating_sub(1));
    }

    /// Set the active account by ID
    pub fn set_active_by_id(&mut self, id: Uuid) -> bool {
        if let Some(pos) = self.accounts.iter().position(|a| a.id == id) {
            self.active_index = pos;
            true
        } else {
            false
        }
    }

    /// Check if this provider has multiple accounts
    pub fn has_multiple(&self) -> bool {
        self.accounts.len() > 1
    }

    /// Get account count
    pub fn count(&self) -> usize {
        self.accounts.len()
    }
}

/// File format for storing all provider accounts
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenAccountsFile {
    version: u32,
    providers: HashMap<String, ProviderAccountData>,
}

/// Token account store for persisting accounts to disk
pub struct TokenAccountStore {
    file_path: PathBuf,
}

/// Errors that can occur with token account storage
#[derive(Debug, thiserror::Error)]
pub enum TokenAccountError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

impl TokenAccountStore {
    /// Create a new store with the default path
    pub fn new() -> Self {
        Self {
            file_path: Self::default_path(),
        }
    }

    /// Create a store with a custom path
    pub fn with_path(path: PathBuf) -> Self {
        Self { file_path: path }
    }

    /// Get the default storage path
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .map(|dir| dir.join("CodexBar"))
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".codexbar")
            })
            .join("token-accounts.json")
    }

    /// Load all accounts from disk
    pub fn load(&self) -> Result<HashMap<ProviderId, ProviderAccountData>, TokenAccountError> {
        if !self.file_path.exists() {
            return Ok(HashMap::new());
        }

        let data = crate::secure_file::read_string(&self.file_path)?;
        let file: TokenAccountsFile = serde_json::from_str(&data)?;

        let mut result = HashMap::new();
        for (key, value) in file.providers {
            if let Some(provider) = ProviderId::from_cli_name(&key) {
                result.insert(provider, value);
            }
        }
        Ok(result)
    }

    /// Save all accounts to disk
    pub fn save(
        &self,
        accounts: &HashMap<ProviderId, ProviderAccountData>,
    ) -> Result<(), TokenAccountError> {
        // Ensure directory exists
        if let Some(parent) = self.file_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let providers: HashMap<String, ProviderAccountData> = accounts
            .iter()
            .map(|(k, v)| (k.cli_name().to_string(), v.clone()))
            .collect();

        let file = TokenAccountsFile {
            version: 1,
            providers,
        };

        let json = serde_json::to_string_pretty(&file)?;
        crate::secure_file::write_string(&self.file_path, &json)?;
        Ok(())
    }

    /// Ensure the accounts file exists
    pub fn ensure_exists(&self) -> Result<PathBuf, TokenAccountError> {
        if self.file_path.exists() {
            return Ok(self.file_path.clone());
        }
        self.save(&HashMap::new())?;
        Ok(self.file_path.clone())
    }

    /// Load accounts for a specific provider
    pub fn load_provider(
        &self,
        provider: ProviderId,
    ) -> Result<ProviderAccountData, TokenAccountError> {
        let all = self.load()?;
        Ok(all.get(&provider).cloned().unwrap_or_default())
    }

    /// Save accounts for a specific provider
    pub fn save_provider(
        &self,
        provider: ProviderId,
        data: &ProviderAccountData,
    ) -> Result<(), TokenAccountError> {
        let mut all = self.load()?;
        all.insert(provider, data.clone());
        self.save(&all)
    }
}

impl Default for TokenAccountStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Override for temporarily using a different token during fetch
#[derive(Debug, Clone)]
pub struct TokenAccountOverride {
    /// The provider being overridden
    pub provider: ProviderId,
    /// The account being used
    pub account: TokenAccount,
    /// Environment variables to set
    pub env_override: Option<HashMap<String, String>>,
    /// Cookie header to use
    pub cookie_header: Option<String>,
}

impl TokenAccountOverride {
    /// Create an override from an account
    pub fn from_account(provider: ProviderId, account: TokenAccount) -> Self {
        let env_override = TokenAccountSupport::env_override(provider, &account.token);
        let cookie_header = if env_override.is_none() {
            Some(TokenAccountSupport::normalized_cookie_header(
                provider,
                &account.token,
            ))
        } else {
            None
        };

        Self {
            provider,
            account,
            env_override,
            cookie_header,
        }
    }
}

/// Maximum number of accounts to fetch per provider
pub const MAX_ACCOUNTS_PER_FETCH: usize = 6;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_account_support() {
        assert!(TokenAccountSupport::is_supported(ProviderId::Claude));
        assert!(TokenAccountSupport::is_supported(ProviderId::Cursor));
        assert!(TokenAccountSupport::is_supported(ProviderId::Copilot));
        assert!(TokenAccountSupport::is_supported(ProviderId::OpenRouter));
        assert!(!TokenAccountSupport::is_supported(ProviderId::Codex));
        assert!(!TokenAccountSupport::is_supported(ProviderId::Gemini));
    }

    #[test]
    fn openrouter_token_accounts_inject_api_key_env() {
        let support = TokenAccountSupport::for_provider(ProviderId::OpenRouter).unwrap();
        assert_eq!(support.title, "API keys");
        assert_eq!(support.placeholder, "sk-or-v1-...");
        assert!(!support.requires_manual_cookie_source);
        match &support.injection {
            TokenInjection::Environment { key } => assert_eq!(key, "OPENROUTER_API_KEY"),
            other => panic!("expected environment injection, got {other:?}"),
        }

        let mut data = ProviderAccountData::new();
        data.add_account(TokenAccount::new("Personal", "sk-or-v1-personal"));
        data.add_account(TokenAccount::new("Work", "sk-or-v1-work"));
        data.set_active(1);

        let active = data.active_account().unwrap();
        assert_eq!(active.label, "Work");
        let env = TokenAccountSupport::env_override(ProviderId::OpenRouter, &active.token).unwrap();
        assert_eq!(
            env.get("OPENROUTER_API_KEY").map(String::as_str),
            Some("sk-or-v1-work")
        );

        let override_data =
            TokenAccountOverride::from_account(ProviderId::OpenRouter, active.clone());
        assert_eq!(
            override_data
                .env_override
                .as_ref()
                .and_then(|m| m.get("OPENROUTER_API_KEY"))
                .map(String::as_str),
            Some("sk-or-v1-work")
        );
        assert!(override_data.cookie_header.is_none());
    }

    #[test]
    fn test_claude_oauth_detection() {
        assert!(TokenAccountSupport::is_claude_oauth_token(
            "sk-ant-oat01-abc123"
        ));
        assert!(TokenAccountSupport::is_claude_oauth_token(
            "Bearer sk-ant-oat01-abc123"
        ));
        assert!(!TokenAccountSupport::is_claude_oauth_token(
            "sessionKey=abc123"
        ));
        assert!(!TokenAccountSupport::is_claude_oauth_token(
            "Cookie: foo=bar"
        ));
    }

    #[test]
    fn test_normalize_cookie_header() {
        let header =
            TokenAccountSupport::normalized_cookie_header(ProviderId::Claude, "abc123token");
        assert_eq!(header, "sessionKey=abc123token");

        let header = TokenAccountSupport::normalized_cookie_header(
            ProviderId::Claude,
            "sessionKey=already_formatted",
        );
        assert_eq!(header, "sessionKey=already_formatted");

        let header = TokenAccountSupport::normalized_cookie_header(ProviderId::Ollama, "abc123");
        assert_eq!(header, "__Secure-session=abc123");
    }

    #[test]
    fn test_provider_account_data() {
        let mut data = ProviderAccountData::new();
        assert_eq!(data.clamped_active_index(), 0);
        assert!(data.active_account().is_none());

        let account = TokenAccount::new("Test", "token123");
        let id = account.id;
        data.add_account(account);

        assert_eq!(data.count(), 1);
        assert!(data.active_account().is_some());
        assert_eq!(data.active_account().unwrap().label, "Test");

        data.remove_account(id);
        assert_eq!(data.count(), 0);
    }

    #[test]
    fn test_multiple_accounts() {
        let mut data = ProviderAccountData::new();
        data.add_account(TokenAccount::new("Account 1", "token1"));
        data.add_account(TokenAccount::new("Account 2", "token2"));

        assert!(data.has_multiple());
        assert_eq!(data.active_account().unwrap().label, "Account 1");

        data.set_active(1);
        assert_eq!(data.active_account().unwrap().label, "Account 2");
    }
}
