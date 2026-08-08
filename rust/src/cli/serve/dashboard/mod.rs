//! Serve web dashboard module: snapshot schema + builder, producer, TTL
//! coordinator, HTML shell, and embedded brand icons.
//!
//! Route handlers here are pure functions of an injected [`DashboardState`] so
//! every route stays socket-testable end to end.

pub mod coordinator;
pub mod html;
pub mod icons;
pub mod snapshot;
pub mod source;

use coordinator::SnapshotCoordinator;
use snapshot::DashboardIdentity;

/// Everything the dashboard routes need, assembled once at serve startup.
#[derive(Clone)]
pub struct DashboardState {
    pub coordinator: SnapshotCoordinator,
    pub identity: DashboardIdentity,
    pub refresh_seconds: u32,
}

impl std::fmt::Debug for DashboardState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DashboardState")
            .field("coordinator", &self.coordinator)
            .field("identity", &self.identity)
            .field("refresh_seconds", &self.refresh_seconds)
            .finish()
    }
}

impl DashboardState {
    /// Production wiring: live producer behind the TTL coordinator.
    pub fn live(refresh_seconds: u32, identity: DashboardIdentity) -> Self {
        let producer = source::SnapshotProducer::new(refresh_seconds, identity);
        let coordinator = SnapshotCoordinator::new(
            std::time::Duration::from_secs(refresh_seconds.max(1) as u64),
            std::sync::Arc::new(move || producer.collect()),
        );
        Self {
            coordinator,
            identity,
            refresh_seconds,
        }
    }

    /// Test wiring: any build closure (stubbed counters, delays, failures).
    #[cfg(test)]
    pub fn stub(
        build: coordinator::SnapshotBuildFn,
        ttl_seconds: u32,
        identity: DashboardIdentity,
    ) -> Self {
        Self {
            coordinator: SnapshotCoordinator::new(
                std::time::Duration::from_secs(ttl_seconds as u64),
                build,
            ),
            identity,
            refresh_seconds: 60,
        }
    }
}

/// `GET /` — the embedded web dashboard shell. Static per config; no account
/// data, so no auth (data endpoints stay token-gated). `Cache-Control:
/// no-store` per upstream so a stale binary never pins an old shell.
pub fn home_response(state: &DashboardState) -> String {
    super::http_response(
        200,
        "text/html; charset=utf-8",
        html::render_shell(state.refresh_seconds),
        &[("Cache-Control", "no-store")],
    )
}

/// `GET /icons/<name>.svg` — embedded brand art. Public (no account data) and
/// immutable per binary (upstream's exact cache policy). SVG assets are verified
/// valid UTF-8 in `icons` tests.
pub fn icon_response(name: &str) -> String {
    match icons::icon_bytes(name).map(std::str::from_utf8) {
        Some(Ok(svg)) => super::http_response(
            200,
            "image/svg+xml",
            svg.to_string(),
            &[("Cache-Control", "public, max-age=86400, immutable")],
        ),
        _ => super::json_response(404, serde_json::json!({ "error": "not found" })),
    }
}

/// `GET /dashboard/v1/snapshot` — the stable v1 JSON contract. Bearer-gated by
/// the caller; `Cache-Control: no-store` on every `/dashboard/v1/*` response
/// per upstream. 500 only if the build genuinely errored (never cached).
pub async fn snapshot_response(state: &DashboardState) -> String {
    match state.coordinator.get().await {
        Ok(payload) => super::http_response(
            200,
            "application/json; charset=utf-8",
            serde_json::to_string_pretty(payload.as_ref()).unwrap_or_else(|_| "{}".to_string()),
            &[("Cache-Control", "no-store")],
        ),
        Err(message) => super::http_response(
            500,
            "application/json; charset=utf-8",
            serde_json::to_string(&serde_json::json!({ "error": message }))
                .unwrap_or_else(|_| "{\"error\":\"unknown\"}".to_string()),
            &[("Cache-Control", "no-store")],
        ),
    }
}
