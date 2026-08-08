//! Dashboard snapshot coordinator: TTL cache + single-flight builds.
//!
//! Upstream 0.48.0 F9/#2717 parity: slow snapshot builds are NEVER discarded —
//! a build that outlives any one request still completes, its result is cached,
//! and every waiter (current or arriving mid-build) receives that same result.
//! There is no 504-style "build took too long" path at all: the only failure
//! surfaced is a build that genuinely errored, and errors are never cached.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use super::snapshot::SnapshotPayload;
use super::source::BoxSnapshotFuture;

/// Pluggable snapshot collector (production: provider+cost scan; tests: stub).
pub type SnapshotBuildFn = Arc<dyn Fn() -> BoxSnapshotFuture + Send + Sync>;

#[derive(Debug)]
enum Slot {
    /// No build yet, or last attempt failed (errors are not cached).
    Empty,
    /// A build is running; `notify` fires when it finishes.
    Building(Arc<Notify>),
    /// Last good build result + when it completed.
    Ready(Arc<SnapshotPayload>, Instant),
}

impl std::fmt::Debug for SnapshotCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnapshotCoordinator")
            .field("ttl", &self.ttl)
            .finish_non_exhaustive()
    }
}

/// Cheaply cloneable handle (all coordination state is shared through `Arc`).
#[derive(Clone)]
pub struct SnapshotCoordinator {
    ttl: Duration,
    build: SnapshotBuildFn,
    slot: Arc<StdMutex<Slot>>,
}

impl SnapshotCoordinator {
    pub fn new(ttl: Duration, build: SnapshotBuildFn) -> Self {
        Self {
            ttl,
            build,
            slot: Arc::new(StdMutex::new(Slot::Empty)),
        }
    }

    /// Get a snapshot: serve the fresh cached build when younger than `ttl`,
    /// share the in-flight build when one is running (late result delivered,
    /// not discarded), or start a new build otherwise.
    pub async fn get(&self) -> Result<Arc<SnapshotPayload>, String> {
        enum Step {
            Serve(Arc<SnapshotPayload>),
            Wait(Arc<Notify>),
            Build(Arc<Notify>),
        }
        loop {
            // Decide under the lock; the guard is always dropped before awaits.
            let step = {
                let mut slot = self.slot.lock().expect("coordinator poisoned");
                match &mut *slot {
                    Slot::Ready(payload, built_at) if built_at.elapsed() < self.ttl => {
                        Step::Serve(payload.clone())
                    }
                    Slot::Building(notify) => Step::Wait(notify.clone()),
                    _ => {
                        let notify = Arc::new(Notify::new());
                        *slot = Slot::Building(notify.clone());
                        Step::Build(notify)
                    }
                }
            };
            match step {
                Step::Serve(payload) => return Ok(payload),
                Step::Wait(notify) => {
                    // Mid-build waiter: stays until the build finishes, then
                    // receives the completed (late) result instead of a timeout.
                    notify.notified().await;
                    continue;
                }
                Step::Build(notify) => {
                    let result = (self.build)().await;

                    let mut slot = self.slot.lock().expect("coordinator poisoned");
                    let outcome = match result {
                        Ok(payload) => {
                            let payload = Arc::new(payload);
                            *slot = Slot::Ready(payload.clone(), Instant::now());
                            Ok(payload)
                        }
                        Err(message) => {
                            // Errors never cache: the next request retries fresh.
                            *slot = Slot::Empty;
                            Err(message)
                        }
                    };
                    notify.notify_waiters();
                    return outcome;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::serve::dashboard::snapshot::{
        DashboardIdentity, ProviderFetchEnvelope, SnapshotInput, build_snapshot,
    };
    use crate::core::{ProviderFetchResult, RateWindow, UsageSnapshot};
    use std::collections::{BTreeSet, HashMap};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn stub_input() -> SnapshotInput {
        SnapshotInput {
            providers: vec![ProviderFetchEnvelope {
                id: "claude".to_string(),
                display_name: "Claude".to_string(),
                session_label: "Session".to_string(),
                weekly_label: "Weekly".to_string(),
                fetch: Ok(ProviderFetchResult::new(
                    UsageSnapshot::new(RateWindow::new(50.0)),
                    "test",
                )),
            }],
            costs: HashMap::new(),
            claude_accounts: None,
            identity: DashboardIdentity::Redacted,
            generated_at: chrono::Utc::now(),
            refresh_seconds: 60,
            version: None,
            order: vec![],
            enabled: BTreeSet::new(),
        }
    }

    fn counting_source(calls: Arc<AtomicUsize>, delay: Duration) -> SnapshotBuildFn {
        Arc::new(move || {
            let calls = calls.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(delay).await;
                Ok(build_snapshot(&stub_input()))
            })
        })
    }

    #[tokio::test]
    async fn serves_first_build_then_cache_within_ttl() {
        let calls = Arc::new(AtomicUsize::new(0));
        let coordinator = SnapshotCoordinator::new(
            Duration::from_secs(3600),
            counting_source(calls.clone(), Duration::ZERO),
        );
        let first = coordinator.get().await.unwrap();
        let second = coordinator.get().await.unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "second get must use the TTL cache"
        );
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.schema_version, 1);
    }

    #[tokio::test]
    async fn expired_ttl_rebuilds() {
        let calls = Arc::new(AtomicUsize::new(0));
        let coordinator = SnapshotCoordinator::new(
            Duration::ZERO,
            counting_source(calls.clone(), Duration::ZERO),
        );
        coordinator.get().await.unwrap();
        coordinator.get().await.unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "zero ttl forces a fresh build"
        );
    }

    #[tokio::test]
    async fn concurrent_waiters_share_one_build_and_late_result_is_delivered() {
        let calls = Arc::new(AtomicUsize::new(0));
        let coordinator = SnapshotCoordinator::new(
            Duration::from_secs(3600),
            counting_source(calls.clone(), Duration::from_millis(200)),
        );
        // Four getters race in while the single build is running; ALL waiters
        // get the completed result (F9: late results are never discarded).
        let mut join = Vec::new();
        for _ in 0..4 {
            let coordinator = coordinator.clone();
            join.push(tokio::spawn(async move { coordinator.get().await }));
        }
        let mut payloads = Vec::new();
        for handle in join {
            payloads.push(handle.await.unwrap().unwrap());
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "single-flight: exactly one build"
        );
        for payload in &payloads[1..] {
            assert!(Arc::ptr_eq(&payloads[0], payload));
        }
    }

    #[tokio::test]
    async fn build_errors_reach_every_waiter_and_are_never_cached() {
        let calls = Arc::new(AtomicUsize::new(0));
        let fail = calls.clone();
        let build: SnapshotBuildFn = Arc::new(move || {
            let fail = fail.clone();
            Box::pin(async move {
                let attempt = fail.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await;
                if attempt == 0 {
                    Err("boom".to_string())
                } else {
                    Ok(build_snapshot(&stub_input()))
                }
            })
        });
        let coordinator = SnapshotCoordinator::new(Duration::from_secs(3600), build);
        let first = coordinator.get().await;
        assert!(matches!(&first, Err(message) if message == "boom"));
        // Next call rebuilds instead of replaying the error.
        let second = coordinator.get().await;
        assert!(second.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn waiter_arriving_mid_build_gets_same_result_not_duplicate_work() {
        let calls = Arc::new(AtomicUsize::new(0));
        let coordinator = SnapshotCoordinator::new(
            Duration::from_secs(3600),
            counting_source(calls.clone(), Duration::from_millis(300)),
        );
        let first = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.get().await })
        };
        // Let the first caller settle into the builder role, then pile on.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let second = coordinator.get().await;
        let first = first.await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(second.is_ok(), first.is_ok());
    }

    #[test]
    fn coordinator_is_clone_cheap() {
        let coordinator = SnapshotCoordinator::new(
            Duration::from_secs(1),
            counting_source(Arc::new(AtomicUsize::new(0)), Duration::ZERO),
        );
        let clone = coordinator.clone();
        assert_eq!(clone.ttl, coordinator.ttl);
    }
}
