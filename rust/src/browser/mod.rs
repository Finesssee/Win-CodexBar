//! Browser detection and cookie extraction for Windows and WSL

pub mod cookie_cache;
pub mod cookies;
pub mod detection;
pub mod watchdog;
pub mod wsl_paths;

// Re-exports for future UI integration
#[allow(
    unused_imports,
    reason = "watchdog API re-exported for future UI integration; no caller outside this module exists yet"
)]
pub use watchdog::{WatchdogConfig, WatchdogError, WebProbeWatchdog, global_watchdog};
