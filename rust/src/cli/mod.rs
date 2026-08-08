//! CLI module - command-line interface
//!
//! Matches the original CodexBar CLI structure:
//! - `codexbar` - launches the menu bar GUI app (default)
//! - `codexbar usage` - print usage from providers
//! - `codexbar cost` - print local token cost usage
//! - `codexbar autostart` - manage Windows auto-start

#![allow(dead_code)]

pub mod account;
pub mod autostart;
pub mod config;
pub mod cost;
pub mod dashboard;
pub mod diagnose;
pub mod guard;
pub mod hooks;
pub mod serve;
pub mod sessions;
pub mod tty_runner;
pub mod usage;
pub mod workspaces;

use clap::{Parser, Subcommand};

/// Exit codes matching original CodexBar
pub mod exit_codes {
    pub const SUCCESS: i32 = 0;
    pub const UNEXPECTED_FAILURE: i32 = 1;
    /// Guard: remaining quota below `--min-remaining` threshold.
    pub const GUARD_BLOCKED: i32 = 1;
    pub const PROVIDER_MISSING: i32 = 2;
    pub const PARSE_ERROR: i32 = 3;
    pub const CLI_TIMEOUT: i32 = 4;
    /// Invalid CLI arguments (`EX_USAGE`).
    pub const USAGE_ERROR: i32 = 64;
    /// Quota could not be checked (`EX_UNAVAILABLE`); used by `codexbar guard`.
    pub const UNAVAILABLE: i32 = 69;
}

/// CodexBar - Monitor AI provider usage limits
///
/// CLI for inspecting provider usage and managing local config. The desktop
/// menubar shell now lives in `apps/desktop-tauri/`; this binary is CLI-only.
#[derive(Parser, Debug)]
#[command(name = "codexbar")]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    // === Global flags ===
    /// Enable verbose logging
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Emit machine-readable logs (JSON) to stderr
    #[arg(long = "json-output", global = true)]
    pub json_output: bool,

    /// Disable ANSI colors in output
    #[arg(long = "no-color", global = true)]
    pub no_color: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Print usage from enabled providers as text or JSON (default command)
    Usage(usage::UsageArgs),

    /// Print local token cost usage (Claude + Codex) without web/CLI access
    Cost(cost::CostArgs),

    /// Gate automation on one provider's remaining quota
    Guard(guard::GuardArgs),

    /// Export safe provider diagnostics as JSON
    Diagnose(diagnose::DiagnoseArgs),

    /// List or focus local and configured remote agent sessions
    Sessions(sessions::SessionsArgs),

    /// Serve usage and cost JSON on 127.0.0.1
    Serve(serve::ServeArgs),

    /// Emit a one-shot dashboard snapshot (JSON to stdout or --output file)
    Dashboard(dashboard::DashboardArgs),

    /// Manage auto-start on Windows boot
    Autostart(autostart::AutostartArgs),

    /// Manage token accounts for providers
    Account(account::AccountArgs),

    /// Configuration utilities
    Config(config::ConfigArgs),

    /// List, enable, disable, or test external hooks
    Hooks(hooks::HooksArgs),

    /// List local Codex project/workspace usage
    Workspaces(workspaces::WorkspacesArgs),
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn usage_subcommand_help_mentions_nanogpt_provider() {
        let mut command = Cli::command();
        let usage = command
            .find_subcommand_mut("usage")
            .expect("usage subcommand should exist");
        let mut output = Vec::new();
        usage
            .write_long_help(&mut output)
            .expect("usage help should render");

        let help = String::from_utf8(output).expect("help should be valid utf-8");
        assert!(help.contains("nanogpt"));
    }
}
