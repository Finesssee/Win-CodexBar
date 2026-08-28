//! Dashboard snapshot coordinator: TTL cache + single-flight builds.
//!
//! Upstream 0.48.0 F9/#2717 parity: slow snapshot builds are NEVER discarded —
//! a build that outlives any one request still completes, its result is cached,
//! and every waiter (current or arriving mid-build) receives that same result.
//! There is no 504-style "build took too long" path at all: the only failure
//! surfaced is a build that genuinely errored, and errors are never cached —
//! but every waiter that joined the failing generation receives that same
//! stored error (the builder still reports it once), while only the NEXT
//! caller retries with a fresh build.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use tokio::sync::Notify;
use tokio::sync::futures::OwnedNotified;

use super::snapshot::SnapshotPayload;
use super::source::BoxSnapshotFuture;

/// Pluggable snapshot collector (production: provider+cost scan; tests: stub).
pub type SnapshotBuildFn = Arc<dyn Fn() -> BoxSnapshotFuture + Send + Sync>;

/// A single build generation: everything a waiter needs to receive THAT
/// generation's exact outcome. Owning the notification channel and the stored
/// outcome together (instead of re-deriving the result from the global slot
/// after a wake) is what lets all of one generation's waiters fan out to the
/// same error, while a cancelled/dropped build (outcome `None` forever)
/// cleanly routes waiters into a retry loop.
#[derive(Debug)]
struct Flight {
    /// Fires once when the builder resolves (success, error, or drop).
    notify: Arc<Notify>,
    /// The completed outcome, stored by the builder BEFORE `notify_waiters`:
    /// a woken waiter is therefore guaranteed to either read `Some` or observe
    /// `None` only when the builder was cancelled/dropped mid-flight.
    outcome: StdMutex<Option<Result<Arc<SnapshotPayload>, String>>>,
}

#[derive(Debug)]
enum Slot {
    /// No build yet, or last attempt failed (errors are not cached).
    Empty,
    /// A build generation is running; joining callers wait on its `Flight`.
    Building(Arc<Flight>),
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
    /// join the in-flight build generation when one is running (receiving THAT
    /// generation's outcome — shared success or the same error), or start a
    /// new build generation otherwise.
    pub async fn get(&self) -> Result<Arc<SnapshotPayload>, String> {
        loop {
            // Decide under the lock; the guard is always dropped before awaits.
            //
            // Waiter lost-wakeup contract: the waiter creates AND enables its
            // `OwnedNotified` while holding this same decision guard — the
            // guard that observes `Slot::Building`. For the slot itself (the
            // global cache/generation pointer) the builder holds this same
            // mutex for every update (success, error, or guard-driven reset);
            // the per-flight outcome is stored under the flight's own mutex
            // strictly BEFORE `notify_waiters`, which is what any woken waiter
            // synchronizes with. In both cases `notify_waiters` comes last, so by the time the decision
            // guard drops the waiter's future is already on the notify wait
            // list and no `notify_waiters` for this build can have fired in
            // between. The registered future is then carried out past the
            // guard drop and awaited unlocked (`OwnedNotified` owns the
            // `Arc<Notify>`, so no borrow of the guard or slot contents
            // escapes the critical section).
            enum Decision {
                Serve(Arc<SnapshotPayload>),
                Wait(Arc<Flight>, Pin<Box<OwnedNotified>>),
                Build(Arc<Flight>),
            }
            let decision = {
                let mut slot = self.slot.lock().expect("coordinator poisoned");
                match &mut *slot {
                    Slot::Ready(payload, built_at) if built_at.elapsed() < self.ttl => {
                        Decision::Serve(payload.clone())
                    }
                    Slot::Building(flight) => {
                        // Register AND enable the waiter on THIS flight's Notify
                        // before releasing the guard that observed Building —
                        // closes the `notify_waiters` lost-wakeup window: a build
                        // completing in the instant after our decision cannot
                        // fire before this future is on the wait list.
                        let mut notified = Box::pin(flight.notify.clone().notified_owned());
                        notified.as_mut().enable();
                        Decision::Wait(flight.clone(), notified)
                    }
                    Slot::Empty | Slot::Ready(_, _) => {
                        let flight = Arc::new(Flight {
                            notify: Arc::new(Notify::new()),
                            outcome: StdMutex::new(None),
                        });
                        *slot = Slot::Building(flight.clone());
                        Decision::Build(flight)
                    }
                }
            };
            match decision {
                Decision::Serve(payload) => return Ok(payload),
                Decision::Wait(flight, notified) => {
                    // Already registered+enabled under the guard that observed
                    // `Slot::Building`; await after unlock — no lock is held
                    // across this await.
                    notified.await;
                    // The builder stores the flight outcome BEFORE notifying,
                    // so `Some` is the resolution of the exact generation we
                    // joined: every waiter of it fans out to the same success
                    // (cheaply cloned `Arc`) or the same error. `None` means
                    // the builder was cancelled/panicked mid-flight and its
                    // guard already reset the global slot to `Empty` — re-scan
                    // and retry; no result is ever fabricated here.
                    let outcome = flight.outcome.lock().expect("coordinator poisoned").clone();
                    match outcome {
                        Some(outcome) => return outcome,
                        None => continue,
                    }
                }
                Decision::Build(flight) => {
                    // A build that exits, is cancelled, or panics resets the
                    // stranded `Slot::Building` to `Empty` and wakes waiters so
                    // they start a fresh build instead of hanging on a `Notify`
                    // that can no longer fire.
                    let mut guard = BuildGuard::new(self.slot.clone(), flight.clone());
                    let outcome = (self.build)().await.map(Arc::new);

                    // Store THIS generation's outcome first: enabling
                    // notifications were all registered under decision guards,
                    // and waiters only read the outcome after being notified —
                    // storing before `notify_waiters` guarantees every woken
                    // waiter observes it.
                    *flight.outcome.lock().expect("coordinator poisoned") = Some(outcome.clone());

                    let mut slot = self.slot.lock().expect("coordinator poisoned");
                    match &outcome {
                        Ok(payload) => {
                            // Success caches globally for TTL reuse.
                            *slot = Slot::Ready(payload.clone(), Instant::now());
                        }
                        Err(_) => {
                            // Errors never cache: the NEXT request retries
                            // fresh, while THIS generation's joined waiters
                            // still fan out to the stored error above.
                            *slot = Slot::Empty;
                        }
                    }
                    drop(slot);
                    // The slot now reflects completion; defuse the guard so its
                    // drop does not reset a build we already finished.
                    guard.disarm();
                    flight.notify.notify_waiters();
                    return outcome;
                }
            }
        }
    }
}

/// Completion guard for an in-flight build. A `get()` call that is cancelled
/// or whose build panics drops this mid-build; the guard then resets the
/// stranded `Slot::Building` back to `Empty` (only if the slot still holds the
/// SAME flight) and wakes that flight's waiters — with the outcome left
/// `None`, so waiters loop and start a fresh build rather than hanging on a
/// dead `Notify` or receiving a fabricated result. On a normal completion
/// path the builder calls `disarm()` first so the drop is a no-op.
struct BuildGuard {
    slot: Arc<StdMutex<Slot>>,
    flight: Arc<Flight>,
    armed: bool,
}

impl BuildGuard {
    fn new(slot: Arc<StdMutex<Slot>>, flight: Arc<Flight>) -> Self {
        Self {
            slot,
            flight,
            armed: true,
        }
    }

    /// The builder has already updated the slot itself; suppress the reset.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for BuildGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // A poisoned lock means another thread panicked while holding it; do
        // not double-panic during unwinding — leave the slot as it is.
        if let Ok(mut slot) = self.slot.lock()
            && matches!(&*slot, Slot::Building(current) if Arc::ptr_eq(current, &self.flight))
        {
            *slot = Slot::Empty;
            drop(slot);
            // Flight outcome stays `None`: woken waiters observe the drop and
            // loop to retry (a fresh flight) instead of receiving anything
            // fabricate from a build that never completed.
            self.flight.notify.notify_waiters();
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

    /// Deterministic error fan-out: ALL waiters that joined one build
    /// generation must receive THAT generation's error — identical to the
    /// builder's — with NO duplicate rebuild. Driven by manual polls (no
    /// sleeps, no scheduler dependence): the builder future is polled once to
    /// park inside the blocking attempt-1 build, then each waiter is polled
    /// once to join the same flight (a parked manual poll proves it joined —
    /// the enable completes synchronously inside that poll), attempt 1 is
    /// released to `Err("boom")` through a oneshot gate, and every parked
    /// waiter must resolve to the same "boom" on its very next poll.
    #[tokio::test]
    async fn build_error_fans_out_to_all_waiters_of_the_same_generation() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let build: SnapshotBuildFn = {
            let calls = calls.clone();
            let release_rx = Arc::new(StdMutex::new(Some(release_rx)));
            Arc::new(move || {
                let calls = calls.clone();
                let release_rx = release_rx.clone();
                Box::pin(async move {
                    let attempt = calls.fetch_add(1, Ordering::SeqCst);
                    if attempt == 0 {
                        // Park until the test releases this generation to its error.
                        let gate = release_rx.lock().expect("poisoned").take();
                        let gate = gate.expect("attempt-1 gate released only once");
                        // Best-effort gate await; either outcome releases the parked build.
                        let _released = gate.await;
                        Err("boom".to_string())
                    } else {
                        Ok(build_snapshot(&stub_input()))
                    }
                })
            })
        };
        let coordinator = SnapshotCoordinator::new(Duration::from_secs(3600), build);

        // Attempt 1: one manual poll claims the builder role synchronously and
        // parks inside the blocking build.
        let mut builder = Box::pin(coordinator.get());
        let waker = futures::task::noop_waker();
        let mut cx = std::task::Context::from_waker(&waker);
        assert!(
            builder.as_mut().poll(&mut cx).is_pending(),
            "attempt 1 must be parked inside the blocking build"
        );

        // N waiters join the SAME generation: each first manual poll observes
        // Building, registers AND enables on the flight under the decision
        // guard, then parks — all in that single poll (deterministic seam,
        // no sleeps/yields).
        const N: usize = 4;
        let mut waiters: Vec<_> = (0..N)
            .map(|_| {
                let mut waiter = Box::pin(coordinator.get());
                assert!(
                    waiter.as_mut().poll(&mut cx).is_pending(),
                    "waiter must join and park on the in-flight generation"
                );
                waiter
            })
            .collect();

        // Release attempt 1 to its error and drive the builder to completion.
        // Best-effort release; the receiver is dropped if the build already errored.
        let _released = release_tx.send(());
        let std::task::Poll::Ready(Err(message)) = builder.as_mut().poll(&mut cx) else {
            panic!("builder must resolve with the attempt-1 error");
        };
        assert_eq!(message, "boom");

        // Every joined waiter fans out to the same stored error — the enabled
        // notification was already fired, so each waiter resolves immediately
        // on its next poll, and crucially builds NOTHING new.
        for waiter in waiters.iter_mut() {
            match waiter.as_mut().poll(&mut cx) {
                std::task::Poll::Ready(Err(message)) => assert_eq!(message, "boom"),
                _ => panic!("waiter must receive the same generation error"),
            }
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "error fan-out must not trigger a duplicate rebuild"
        );

        // Only a LATER get() retries: attempt 2 builds fresh and succeeds.
        let retry = tokio::time::timeout(Duration::from_secs(5), coordinator.get())
            .await
            .expect("retry build never resolved within 5s")
            .unwrap();
        assert_eq!(retry.schema_version, 1);
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

    // ── F1 lost-wakeup / cancellation / panic regressions ───────────────────

    /// Deterministic lost-wakeup regression: completion is forced into the
    /// exact window between the waiter's decision poll and its await poll,
    /// with zero scheduler races — the test holds the slot lock and the
    /// wakers directly. The waiter's registration MUST be bound to the same
    /// decision guard that observed `Slot::Building` (not deferred past an
    /// unlock): `notify_waiters` only reaches already-registered waiters, so
    /// any registration that happens after the decision guard dropped would
    /// miss this completion and hang forever (the timeout catches it).
    #[tokio::test]
    async fn completion_in_decision_window_sets_waiter_notified() {
        let coordinator = SnapshotCoordinator::new(
            Duration::from_secs(3600),
            counting_source(Arc::new(AtomicUsize::new(0)), Duration::ZERO),
        );
        // Place the slot in Building exactly as a real in-flight build would.
        let flight = Arc::new(Flight {
            notify: Arc::new(Notify::new()),
            outcome: StdMutex::new(None),
        });
        *coordinator.slot.lock().expect("coordinator poisoned") = Slot::Building(flight.clone());

        // First poll: decision observes Building and must register+enable the
        // waiter UNDER the decision guard, before the guard is released.
        let mut waiter = Box::pin(coordinator.get());
        let waker = futures::task::noop_waker();
        let mut cx = std::task::Context::from_waker(&waker);
        assert!(
            waiter.as_mut().poll(&mut cx).is_pending(),
            "waiter must park on the in-flight build"
        );

        // The build completes in the window after the waiter's decision,
        // mirroring the real builder: store the flight outcome, swap the slot
        // to Ready, then fire notify_waiters while the waiter is NOT being
        // polled. A waiter whose registration depends on a later lock
        // acquisition would sleep through this wakeup forever.
        let payload = Arc::new(build_snapshot(&stub_input()));
        *flight.outcome.lock().expect("coordinator poisoned") = Some(Ok(payload.clone()));
        *coordinator.slot.lock().expect("coordinator poisoned") =
            Slot::Ready(payload.clone(), Instant::now());
        flight.notify.notify_waiters();

        // The waiter wakes from the enabled registration and returns the
        // flight's stored outcome; the window-resident completion is not lost.
        let served = tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("lost wakeup: waiter hung on a completed build")
            .unwrap();
        assert!(Arc::ptr_eq(&payload, &served));
    }

    /// A waiter that observes `Slot::Building` must register its `Notified`
    /// before the lock drops, so a builder completing the instant the waiter
    /// unlocks cannot lose the wakeup. Bounding the whole join by a timeout
    /// turns a regression (a waiter hanging on a `Notify` that already fired)
    /// into a fast test failure instead of an infinite hang.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn waiters_racing_completion_never_lose_the_wakeup() {
        for round in 0..25 {
            let calls = Arc::new(AtomicUsize::new(0));
            let coordinator = SnapshotCoordinator::new(
                Duration::from_secs(3600),
                counting_source(calls.clone(), Duration::from_millis(2)),
            );
            // Builder starts first, then waiters pile on around completion.
            let mut join = Vec::new();
            join.push({
                let coordinator = coordinator.clone();
                tokio::spawn(async move { coordinator.get().await })
            });
            tokio::time::sleep(Duration::from_millis(1)).await;
            for _ in 0..8 {
                let coordinator = coordinator.clone();
                join.push(tokio::spawn(async move { coordinator.get().await }));
            }
            let payloads = tokio::time::timeout(Duration::from_secs(5), async {
                let mut out = Vec::new();
                for handle in join {
                    out.push(handle.await.unwrap().unwrap());
                }
                out
            })
            .await
            .expect("lost wakeup: a waiter never resolved within 5s");
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "single-flight preserved across round {round}"
            );
            for payload in &payloads[1..] {
                assert!(Arc::ptr_eq(&payloads[0], payload));
            }
        }
    }

    /// When the builder task is cancelled mid-build, the stranded
    /// `Slot::Building` must reset to `Empty` and wake any waiters, so a later
    /// `get()` starts a fresh build instead of hanging on a dead `Notify`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_builder_resets_slot_and_wakes_waiters() {
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(tokio::sync::Notify::new());
        let build: SnapshotBuildFn = {
            let calls = calls.clone();
            let started = started.clone();
            Arc::new(move || {
                let calls = calls.clone();
                let started = started.clone();
                Box::pin(async move {
                    let attempt = calls.fetch_add(1, Ordering::SeqCst);
                    started.notify_waiters();
                    if attempt == 0 {
                        // Never resolves — only cancellation ends this build.
                        std::future::pending::<Result<_, String>>().await
                    } else {
                        Ok(build_snapshot(&stub_input()))
                    }
                })
            })
        };
        let coordinator = SnapshotCoordinator::new(Duration::from_secs(3600), build);

        let builder = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.get().await })
        };
        tokio::time::timeout(Duration::from_secs(5), started.notified())
            .await
            .expect("builder never started within 5s");

        // A waiter parked on the in-flight build.
        let waiter = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.get().await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;

        builder.abort(); // cancel the builder mid-build
        // Reap the aborted builder task; its result is intentionally discarded.
        let _aborted = builder.await;

        // The waiter and a fresh caller both complete via a rebuilt (attempt 2).
        let fresh = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.get().await })
        };
        let outcomes = tokio::time::timeout(Duration::from_secs(5), async {
            [waiter.await.unwrap(), fresh.await.unwrap()]
        })
        .await
        .expect("stranded Building: a get() never resolved within 5s");
        assert!(outcomes[0].is_ok());
        assert!(outcomes[1].is_ok());
        assert!(
            calls.load(Ordering::SeqCst) >= 2,
            "cancelled build did not deliver; a fresh build ran"
        );
    }

    /// A build that panics must reset `Slot::Building` to `Empty` and wake
    /// waiters, so the next `get()` rebuilds instead of hanging on the panicked
    /// build's `Notify`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn panicked_builder_resets_slot_and_wakes_waiters() {
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(tokio::sync::Notify::new());
        let build: SnapshotBuildFn = {
            let calls = calls.clone();
            let started = started.clone();
            Arc::new(move || {
                let calls = calls.clone();
                let started = started.clone();
                Box::pin(async move {
                    let attempt = calls.fetch_add(1, Ordering::SeqCst);
                    started.notify_waiters();
                    if attempt == 0 {
                        panic!("simulated build failure");
                    }
                    Ok(build_snapshot(&stub_input()))
                })
            })
        };
        let coordinator = SnapshotCoordinator::new(Duration::from_secs(3600), build);

        // The panicking build runs in a spawned task so the test body survives.
        let builder = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.get().await })
        };
        tokio::time::timeout(Duration::from_secs(5), started.notified())
            .await
            .expect("builder never started within 5s");

        // The panic (caught by the spawned task) resets the slot.
        let join_err = builder.await.unwrap_err();
        assert!(join_err.is_panic(), "expected the build to panic");

        // A later get() rebuilds fresh and succeeds within a timeout.
        let retry = tokio::time::timeout(Duration::from_secs(5), coordinator.get())
            .await
            .expect("stranded Building: retry never resolved within 5s");
        assert!(retry.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
