//! Shared library surface for CodexBar.
//!
//! This keeps the current Rust implementation usable from the existing CLI/bin
//! while giving the rewrite a stable crate dependency for future shells.

pub mod agent_sessions;
pub mod browser;
pub mod cli;
pub mod codex_accounts;
pub mod codex_workspaces;
pub mod core;
pub mod cost_scanner;
pub mod host;
pub mod locale;
pub mod logging;
pub mod login;
pub mod notifications;
pub mod providers;
pub mod secure_file;
pub mod settings;
pub mod sound;
pub mod spend_contract;

pub mod status;
pub mod tray;
pub mod updater;
pub mod wsl;

mod codex_costs;
mod codex_sessions;
mod pi_session_cost;
