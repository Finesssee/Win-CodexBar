//! Web Probe Watchdog
//!
//! Monitors and manages child web probe processes to prevent orphaned processes.
//! Ensures that browser automation or scraping processes are properly cleaned up.

#![allow(
    dead_code,
    reason = "watchdog types reserved for future browser-session monitoring integration"
)]

use std::collections::HashMap;
use std::process::Child;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// A tracked child process
pub struct TrackedProcess {
    pub child: Child,
    pub name: String,
    pub started_at: Instant,
    pub timeout: Duration,
}

/// Watchdog configuration
pub struct WatchdogConfig {
    /// How often to check for stale processes (default: 5 seconds)
    pub check_interval: Duration,
    /// Default timeout for processes (default: 60 seconds)
    pub default_timeout: Duration,
    /// Maximum number of concurrent processes (default: 10)
    pub max_processes: usize,
}

impl Default for WatchdogConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_secs(5),
            default_timeout: Duration::from_secs(60),
            max_processes: 10,
        }
    }
}

/// Watchdog for managing web probe processes
pub struct WebProbeWatchdog {
    config: WatchdogConfig,
    processes: Arc<RwLock<HashMap<u32, TrackedProcess>>>,
    is_running: Arc<AtomicBool>,
    next_id: Arc<RwLock<u32>>,
}

impl WebProbeWatchdog {
    /// Create a new watchdog with default config
    pub fn new() -> Self {
        Self::with_config(WatchdogConfig::default())
    }

    /// Create a new watchdog with custom config
    pub fn with_config(config: WatchdogConfig) -> Self {
        Self {
            config,
            processes: Arc::new(RwLock::new(HashMap::new())),
            is_running: Arc::new(AtomicBool::new(false)),
            next_id: Arc::new(RwLock::new(1)),
        }
    }

    /// Start the watchdog monitoring loop
    pub fn start(&self) -> tokio::task::JoinHandle<()> {
        if self.is_running.load(Ordering::SeqCst) {
            tracing::warn!("[Watchdog] Already running");
            return tokio::spawn(async {});
        }

        self.is_running.store(true, Ordering::SeqCst);

        tracing::info!("[Watchdog] Starting process watchdog");

        let is_running = self.is_running.clone();
        let processes = self.processes.clone();
        let check_interval = self.config.check_interval;

        tokio::spawn(async move {
            while is_running.load(Ordering::SeqCst) {
                tokio::time::sleep(check_interval).await;

                if !is_running.load(Ordering::SeqCst) {
                    break;
                }

                // Check for stale processes
                let mut procs = processes.write().await;
                let mut to_remove = Vec::new();

                for (&id, tracked) in procs.iter_mut() {
                    let elapsed = tracked.started_at.elapsed();

                    // Check if process has exceeded timeout
                    if elapsed > tracked.timeout {
                        tracing::warn!(
                            "[Watchdog] Process {} ({}) exceeded timeout ({:?}), killing",
                            tracked.name,
                            id,
                            tracked.timeout
                        );

                        // Try to kill the process
                        if let Err(e) = tracked.child.kill() {
                            tracing::error!("[Watchdog] Failed to kill process {}: {}", id, e);
                        }

                        to_remove.push(id);
                    }

                    // Check if process has already exited
                    match tracked.child.try_wait() {
                        Ok(Some(status)) => {
                            tracing::debug!(
                                "[Watchdog] Process {} ({}) exited with status {:?}",
                                tracked.name,
                                id,
                                status
                            );
                            to_remove.push(id);
                        }
                        Ok(None) => {
                            // Still running
                        }
                        Err(e) => {
                            tracing::error!("[Watchdog] Error checking process {}: {}", id, e);
                            to_remove.push(id);
                        }
                    }
                }

                // Remove finished processes
                for id in to_remove {
                    procs.remove(&id);
                }
            }

            tracing::info!("[Watchdog] Process watchdog stopped");
        })
    }

    /// Stop the watchdog and kill all tracked processes
    pub async fn stop(&self) {
        tracing::info!("[Watchdog] Stopping watchdog and killing all tracked processes");
        self.is_running.store(false, Ordering::SeqCst);

        // Kill all remaining processes
        let mut procs = self.processes.write().await;
        for (id, mut tracked) in procs.drain() {
            tracing::debug!("[Watchdog] Killing process {} ({})", tracked.name, id);
            if let Err(e) = tracked.child.kill() {
                tracing::error!("[Watchdog] Failed to kill process {}: {}", id, e);
            }
        }
    }

    /// Check if the watchdog is running
    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    /// Register a process to be watched
    pub async fn register(
        &self,
        name: impl Into<String>,
        child: Child,
    ) -> Result<u32, WatchdogError> {
        self.register_with_timeout(name, child, self.config.default_timeout)
            .await
    }

    /// Register a process with a custom timeout
    pub async fn register_with_timeout(
        &self,
        name: impl Into<String>,
        child: Child,
        timeout: Duration,
    ) -> Result<u32, WatchdogError> {
        let mut procs = self.processes.write().await;

        // Check if we're at capacity
        if procs.len() >= self.config.max_processes {
            return Err(WatchdogError::TooManyProcesses);
        }

        // Get next ID
        let mut id_guard = self.next_id.write().await;
        let id = *id_guard;
        *id_guard += 1;

        let tracked = TrackedProcess {
            child,
            name: name.into(),
            started_at: Instant::now(),
            timeout,
        };

        procs.insert(id, tracked);

        tracing::debug!(
            "[Watchdog] Registered process {} (id={}, timeout={:?})",
            procs.get(&id).map(|p| p.name.as_str()).unwrap_or("unknown"),
            id,
            timeout
        );

        Ok(id)
    }

    /// Unregister a process (it completed normally)
    pub async fn unregister(&self, id: u32) -> Option<TrackedProcess> {
        let mut procs = self.processes.write().await;
        procs.remove(&id)
    }

    /// Kill a specific process
    pub async fn kill(&self, id: u32) -> Result<(), WatchdogError> {
        let mut procs = self.processes.write().await;

        if let Some(mut tracked) = procs.remove(&id) {
            tracked
                .child
                .kill()
                .map_err(|e| WatchdogError::KillFailed(e.to_string()))?;
            Ok(())
        } else {
            Err(WatchdogError::NotFound)
        }
    }

    /// Get the number of tracked processes
    pub async fn count(&self) -> usize {
        self.processes.read().await.len()
    }
}

impl Default for WebProbeWatchdog {
    fn default() -> Self {
        Self::new()
    }
}

/// Watchdog errors
#[derive(Debug, Clone)]
pub enum WatchdogError {
    /// Too many processes are already being tracked
    TooManyProcesses,
    /// Process not found
    NotFound,
    /// Failed to kill process
    KillFailed(String),
}

impl std::fmt::Display for WatchdogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WatchdogError::TooManyProcesses => write!(f, "Too many processes being tracked"),
            WatchdogError::NotFound => write!(f, "Process not found"),
            WatchdogError::KillFailed(e) => write!(f, "Failed to kill process: {}", e),
        }
    }
}

impl std::error::Error for WatchdogError {}

/// Global watchdog instance
static GLOBAL_WATCHDOG: std::sync::OnceLock<WebProbeWatchdog> = std::sync::OnceLock::new();

/// Get the global watchdog instance
pub fn global_watchdog() -> &'static WebProbeWatchdog {
    GLOBAL_WATCHDOG.get_or_init(WebProbeWatchdog::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_watchdog_config_default() {
        let config = WatchdogConfig::default();
        assert_eq!(config.check_interval, Duration::from_secs(5));
        assert_eq!(config.default_timeout, Duration::from_secs(60));
        assert_eq!(config.max_processes, 10);
    }

    #[test]
    fn test_watchdog_new() {
        let watchdog = WebProbeWatchdog::new();
        assert!(!watchdog.is_running());
    }

    #[tokio::test]
    async fn test_watchdog_count_empty() {
        let watchdog = WebProbeWatchdog::new();
        assert_eq!(watchdog.count().await, 0);
    }
}
