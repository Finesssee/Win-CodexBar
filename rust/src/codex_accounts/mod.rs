//! Multi-account Codex support for CodexBar.
//!
//! This module is a Rust port of the Windows core of
//! [`ademisler/codexcontrol`](https://github.com/ademisler/codexcontrol) (MIT),
//! which manages multiple Codex accounts through isolated `CODEX_HOME`
//! directories and switches the ambient Codex identity. See `NOTICE`/LICENSE
//! for the upstream MIT attribution.
//!
//! The model is deliberately mirror-shaped: a `CodexAccount` lives in either the
//! ambient home (`~/.codex`) or an app-managed home (`managed-homes/<uuid>`),
//! quota snapshots are fetched per account, and switching swaps the ambient
//! `auth.json` plus the Codex Desktop MSIX session state.

pub mod account_manager;
pub mod api;
pub mod codex_desktop;
pub mod file_locations;
pub mod models;
pub mod stores;

pub use account_manager::{
    CodexAccountManager, CodexAccountManagerError, CodexLoginOutcome, CodexLoginResult,
    CodexSwitchResult, ManagedLoginProcess,
};
pub use api::{AuthBackedIdentity, AuthCredentials, CodexAccountApi, CodexApiError, load_identity};
pub use codex_desktop::{
    CodexDesktopControlError, build_restart_command, build_restart_script,
    encode_powershell_script, restart_codex_desktop,
};
pub use models::{
    AccountUsageSnapshot, CodexAccount, CodexAccountSource, CreditsBalanceSnapshot,
    RemovedAccountIdentity, UsageWindowSnapshot, utc_now,
};
pub use stores::{AccountStore, SnapshotStore};
