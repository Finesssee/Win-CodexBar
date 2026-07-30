//! Config command implementation
//!
//! Utilities for validating and inspecting configuration.

use clap::{Parser, Subcommand};
use serde::de::DeserializeOwned;
use std::path::Path;

use crate::core::{ProviderId, TokenAccountStore, instantiate_provider};
use crate::settings::{ApiKeys, ManualCookies, Settings};

/// Arguments for the config command
#[derive(Parser, Debug)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// Validate configuration files
    Validate,
    /// Dump configuration to stdout
    Dump {
        /// Output format: json or toml
        #[arg(short, long, default_value = "json")]
        format: String,
        /// Include raw secrets (default: redact)
        #[arg(long = "show-secrets", default_value_t = false)]
        show_secrets: bool,
    },
    /// List providers and enabled state
    Providers,
    /// Enable a provider
    Enable {
        /// Provider CLI name or alias
        provider: String,
    },
    /// Disable a provider
    Disable {
        /// Provider CLI name or alias
        provider: String,
    },
    /// Store an API key for a provider
    SetApiKey {
        /// Provider CLI name or alias
        provider: String,
        /// API key to store
        #[arg(long = "api-key")]
        api_key: Option<String>,
        /// Read API key from stdin
        #[arg(long)]
        stdin: bool,
        /// Store the key without enabling the provider
        #[arg(long = "no-enable")]
        no_enable: bool,
    },
    /// Show configuration file paths
    Path,
}

/// Run the config command
pub async fn run(args: ConfigArgs) -> anyhow::Result<()> {
    match args.command {
        ConfigCommand::Validate => validate_config().await,
        ConfigCommand::Dump {
            format,
            show_secrets,
        } => dump_config(&format, show_secrets).await,
        ConfigCommand::Providers => list_providers().await,
        ConfigCommand::Enable { provider } => set_provider_enabled(&provider, true).await,
        ConfigCommand::Disable { provider } => set_provider_enabled(&provider, false).await,
        ConfigCommand::SetApiKey {
            provider,
            api_key,
            stdin,
            no_enable,
        } => set_api_key(&provider, api_key.as_deref(), stdin, !no_enable).await,
        ConfigCommand::Path => show_paths().await,
    }
}

/// Validate configuration files
async fn validate_config() -> anyhow::Result<()> {
    let mut report = ValidationReport::default();
    validate_settings_config(&mut report);
    validate_manual_cookies_config(&mut report);
    validate_token_accounts_config(&mut report);
    report.finish()
}

#[derive(Default)]
struct ValidationReport {
    errors: Vec<String>,
    warnings: Vec<String>,
}

impl ValidationReport {
    fn error(&mut self, message: impl Into<String>) {
        self.errors.push(message.into());
    }

    fn warning(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }

    fn finish(self) -> anyhow::Result<()> {
        print_validation_summary(&self.errors, &self.warnings)
    }
}

fn validate_settings_config(report: &mut ValidationReport) {
    print!("Checking settings.json... ");
    let Some(path) = Settings::settings_path() else {
        println!("ERROR");
        report.error("settings.json: Could not determine config path");
        return;
    };

    if validate_optional_json_file::<Settings>(&path, "settings.json", report) {
        return;
    }

    println!("NOT FOUND (using defaults)");
    report.warning("settings.json: File does not exist, using defaults");
}

fn validate_manual_cookies_config(report: &mut ValidationReport) {
    print!("Checking manual_cookies.json... ");
    let Some(path) = ManualCookies::cookies_path() else {
        println!("SKIP");
        return;
    };

    if !validate_optional_json_file::<ManualCookies>(&path, "manual_cookies.json", report) {
        println!("NOT FOUND (none configured)");
    }
}

fn validate_optional_json_file<T>(path: &Path, label: &str, report: &mut ValidationReport) -> bool
where
    T: DeserializeOwned,
{
    if !path.exists() {
        return false;
    }

    match read_json_config::<T>(path) {
        Ok(()) => println!("OK"),
        Err(ConfigFileError::Read(e)) => {
            println!("ERROR");
            report.error(format!("{label}: Could not read file: {e}"));
        }
        Err(ConfigFileError::Parse(e)) => {
            println!("INVALID");
            report.error(format!("{label}: {e}"));
        }
    }

    true
}

fn read_json_config<T>(path: &Path) -> Result<(), ConfigFileError>
where
    T: DeserializeOwned,
{
    let content = std::fs::read_to_string(path).map_err(ConfigFileError::Read)?;
    serde_json::from_str::<T>(&content)
        .map(|_| ())
        .map_err(ConfigFileError::Parse)
}

enum ConfigFileError {
    Read(std::io::Error),
    Parse(serde_json::Error),
}

fn validate_token_accounts_config(report: &mut ValidationReport) {
    print!("Checking token-accounts.json... ");
    let path = TokenAccountStore::default_path();
    if !path.exists() {
        println!("NOT FOUND (none configured)");
        return;
    }

    match TokenAccountStore::new().load() {
        Ok(_) => println!("OK"),
        Err(e) => {
            println!("INVALID");
            report.error(format!("token-accounts.json: {e}"));
        }
    }
}

fn print_validation_summary(errors: &[String], warnings: &[String]) -> anyhow::Result<()> {
    println!();
    if errors.is_empty() && warnings.is_empty() {
        println!("Configuration is valid.");
    } else {
        if !warnings.is_empty() {
            println!("Warnings:");
            for w in warnings {
                println!("  - {}", w);
            }
        }
        if !errors.is_empty() {
            println!("Errors:");
            for e in errors {
                println!("  - {}", e);
            }
            anyhow::bail!(
                "Configuration validation failed with {} error(s).",
                errors.len()
            );
        }
    }

    Ok(())
}

/// Dump configuration to stdout
async fn dump_config(format: &str, show_secrets: bool) -> anyhow::Result<()> {
    let value = build_dump_value()?;
    let value = sanitize_settings_for_dump(value, show_secrets);

    match format.to_lowercase().as_str() {
        "json" => {
            let json = serde_json::to_string_pretty(&value)?;
            println!("{}", json);
        }
        "toml" => {
            let toml = toml::to_string_pretty(&value)
                .map_err(|e| anyhow::anyhow!("Failed to convert dump to TOML: {e}"))?;
            println!("{}", toml);
        }
        _ => {
            anyhow::bail!("Unknown format '{}'. Supported formats: json, toml", format);
        }
    }

    Ok(())
}

fn build_dump_value() -> anyhow::Result<serde_json::Value> {
    let settings = Settings::load();
    let mut root = serde_json::to_value(&settings)?;

    if let Some(obj) = root.as_object_mut() {
        obj.insert(
            "api_keys".to_string(),
            serde_json::to_value(ApiKeys::load())?,
        );
        obj.insert(
            "manual_cookies".to_string(),
            serde_json::to_value(ManualCookies::load())?,
        );

        let token_accounts = TokenAccountStore::new()
            .load()
            .unwrap_or_default()
            .into_iter()
            .map(|(id, data)| (id.cli_name().to_string(), data))
            .collect::<std::collections::BTreeMap<_, _>>();
        obj.insert(
            "token_accounts".to_string(),
            serde_json::to_value(token_accounts)?,
        );
    }

    Ok(root)
}

/// Recursively redact secret-shaped fields for `config dump`.
///
/// When `show_secrets` is true the value is returned unchanged.
fn sanitize_settings_for_dump(value: serde_json::Value, show_secrets: bool) -> serde_json::Value {
    if show_secrets {
        return value;
    }
    redact_secrets_value(value)
}

fn is_secret_field_name(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(|c| *c != '_')
        .flat_map(|c| c.to_lowercase())
        .collect();
    matches!(
        normalized.as_str(),
        "apikey"
            | "secretkey"
            | "cookieheader"
            | "manualcookieheader"
            | "token"
            | "apitoken"
            | "httpproxypassword"
            | "password"
    )
}

fn redact_secrets_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, child) in map {
                if is_secret_field_name(&key) {
                    out.insert(key, serde_json::Value::String("[REDACTED]".to_string()));
                } else {
                    out.insert(key, redact_secrets_value(child));
                }
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(redact_secrets_value).collect())
        }
        other => other,
    }
}

/// List provider enabled state.
async fn list_providers() -> anyhow::Result<()> {
    let settings = Settings::load();
    for id in ProviderId::all() {
        let state = if settings.is_provider_enabled(*id) {
            "enabled"
        } else {
            "disabled"
        };
        let default_marker = if instantiate_provider(*id).metadata().default_enabled {
            " default"
        } else {
            ""
        };
        println!(
            "{}: {}{} ({})",
            id.cli_name(),
            state,
            default_marker,
            id.display_name()
        );
    }
    Ok(())
}

/// Enable or disable a provider by CLI name.
async fn set_provider_enabled(provider: &str, enabled: bool) -> anyhow::Result<()> {
    let id = parse_provider(provider)?;
    let mut settings = Settings::load();
    if enabled {
        settings.enable_provider(id);
    } else {
        settings.disable_provider(id);
    }
    settings.save()?;
    let state = if enabled { "enabled" } else { "disabled" };
    println!("Config: {state} {}", id.display_name());
    Ok(())
}

/// Store an API key and optionally enable the provider.
async fn set_api_key(
    provider: &str,
    api_key: Option<&str>,
    read_from_stdin: bool,
    enable_provider: bool,
) -> anyhow::Result<()> {
    let id = parse_provider(provider)?;
    ensure_provider_accepts_api_key(id)?;
    let api_key = resolve_api_key_input(api_key, read_from_stdin)?;

    let mut keys = ApiKeys::load();
    keys.set(id.cli_name(), &api_key, None);
    keys.save()?;

    if enable_provider {
        let mut settings = Settings::load();
        settings.enable_provider(id);
        settings.save()?;
    }

    let suffix = if enable_provider { " and enabled" } else { "" };
    println!("Config: stored API key for {}{suffix}", id.display_name());
    Ok(())
}

fn parse_provider(raw: &str) -> anyhow::Result<ProviderId> {
    ProviderId::from_cli_name(raw).ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown provider '{}'. Run `codexbar config providers` to list providers.",
            raw
        )
    })
}

fn ensure_provider_accepts_api_key(id: ProviderId) -> anyhow::Result<()> {
    if crate::settings::get_api_key_providers()
        .iter()
        .any(|provider| provider.id == id)
    {
        return Ok(());
    }
    anyhow::bail!("{} does not support stored API keys.", id.display_name())
}

fn resolve_api_key_input(api_key: Option<&str>, read_from_stdin: bool) -> anyhow::Result<String> {
    if api_key.is_some() && read_from_stdin {
        anyhow::bail!("Use either --api-key or --stdin, not both.");
    }

    let raw = if read_from_stdin {
        let mut buffer = String::new();
        use std::io::Read;
        std::io::stdin().read_to_string(&mut buffer)?;
        Some(buffer)
    } else {
        api_key.map(ToString::to_string)
    };

    let mut value = raw
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Missing API key. Pass --api-key <key> or use --stdin."))?;

    if (value.starts_with('"') && value.ends_with('"'))
        || (value.starts_with('\'') && value.ends_with('\''))
    {
        value.remove(0);
        value.pop();
    }

    let value = value.trim().to_string();
    if value.is_empty() {
        anyhow::bail!("Missing API key. Pass --api-key <key> or use --stdin.");
    }
    Ok(value)
}

/// Show configuration file paths
async fn show_paths() -> anyhow::Result<()> {
    println!("Configuration paths:");

    if let Some(path) = Settings::settings_path() {
        let exists = if path.exists() { "" } else { " (not found)" };
        println!("  Settings:       {}{}", path.display(), exists);
    } else {
        println!("  Settings:       (could not determine path)");
    }

    if let Some(path) = ManualCookies::cookies_path() {
        let exists = if path.exists() { "" } else { " (not found)" };
        println!("  Manual cookies: {}{}", path.display(), exists);
    } else {
        println!("  Manual cookies: (could not determine path)");
    }

    let token_path = TokenAccountStore::default_path();
    let exists = if token_path.exists() {
        ""
    } else {
        " (not found)"
    };
    println!("  Token accounts: {}{}", token_path.display(), exists);

    // Show config directory
    if let Some(config_dir) = dirs::config_dir() {
        let codexbar_dir = config_dir.join("CodexBar");
        println!();
        println!("Config directory: {}", codexbar_dir.display());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::sanitize_settings_for_dump;
    use serde_json::json;

    #[test]
    fn sanitize_settings_for_dump_redacts_secret_fields() {
        let raw = json!({
            "http_proxy_password": "proxy-secret",
            "provider_configs": {
                "claude": {
                    "manual_cookie_header": "sessionKey=abc",
                    "api_token": "tok-123"
                }
            },
            "api_keys": {
                "keys": {
                    "zai": {
                        "api_key": "cb_test_api_key_456",
                        "label": "Work",
                        "saved_at": "2026-01-01"
                    }
                }
            },
            "manual_cookies": {
                "cookies": {
                    "claude": {
                        "cookie_header": "sessionKey=secret-cookie",
                        "saved_at": "2026-01-01"
                    }
                }
            },
            "token_accounts": {
                "factory": {
                    "accounts": [{
                        "id": "11111111-1111-1111-1111-111111111111",
                        "label": "Team",
                        "token": "raw-token-value",
                        "added_at": 1
                    }]
                }
            },
            "secret_key": "top-secret"
        });

        let redacted = sanitize_settings_for_dump(raw, false);
        let text = serde_json::to_string(&redacted).expect("serialize");

        assert!(text.contains("[REDACTED]"));
        assert!(!text.contains("proxy-secret"));
        assert!(!text.contains("sessionKey=abc"));
        assert!(!text.contains("tok-123"));
        assert!(!text.contains("cb_test_api_key_456"));
        assert!(!text.contains("secret-cookie"));
        assert!(!text.contains("raw-token-value"));
        assert!(!text.contains("top-secret"));
        // Non-secret identity fields stay.
        assert!(text.contains("Work"));
        assert!(text.contains("Team"));
        assert!(text.contains("11111111-1111-1111-1111-111111111111"));
    }

    #[test]
    fn sanitize_settings_for_dump_show_secrets_keeps_raw() {
        let raw = json!({
            "api_key": "keep-me",
            "nested": { "token": "also-keep", "label": "Team" }
        });

        let out = sanitize_settings_for_dump(raw.clone(), true);
        assert_eq!(out, raw);
        assert_eq!(out["api_key"], "keep-me");
        assert_eq!(out["nested"]["token"], "also-keep");
    }
}
