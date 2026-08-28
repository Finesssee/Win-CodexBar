//! Host module for process management and command execution

pub mod command_runner;
pub mod session;

// Re-exports for future CLI integration
#[allow(
    unused_imports,
    reason = "command runner API re-exported for future CLI integration"
)]
pub use command_runner::{
    CommandError, CommandOptions, CommandResult, CommandRunner, RollingBuffer,
};
