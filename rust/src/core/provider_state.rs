//! Canonical, backend-owned classification of provider refresh failures.
//!
//! Surfacing (FloatBar pill, Settings provider detail) must never classify
//! from the redacted error text: presentation copy drifts, and broad keyword
//! matches mislabel unrelated failures (e.g. Copilot's not-installed message
//! contains "sign in" and "legacy"). Instead, each provider error is mapped
//! to a small [`ProviderStateKind`] while the typed [`ProviderError`] still
//! exists, and the kind travels on `ProviderUsageSnapshot` / `ProviderDetail`.
//! The frontend only maps the kind to a locale key.
//!
//! Variant meanings are provider-relative: most providers raise
//! `NotInstalled` for a missing API key or auth file — a sign-in gate, not an
//! offline runtime — so the default maps it to
//! [`ProviderStateKind::NeedsAuthentication`]. Probes whose `NotInstalled`
//! genuinely means a local runtime or CLI binary is missing (language-server
//! probes, CLI/binary/plugin presence checks) override via
//! `Provider::error_state_kind` (e.g. `AntigravityProvider`, `ClaudeProvider`).

use serde::{Deserialize, Serialize};

use super::ProviderError;

/// User-facing provider availability state, derived from typed errors at the
/// backend boundary. Serialized (camelCase) to the frontend bridge.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderStateKind {
    /// Latest refresh succeeded; usage data is current.
    #[default]
    Ready,
    /// Credentials are missing or were rejected; sign-in is needed.
    NeedsAuthentication,
    /// Credentials authenticated but the session/token is expired.
    ExpiredSession,
    /// A local runtime backing this provider is not running.
    LocalRuntimeOffline,
    /// Anything else: parse failures, network/timeout, unknown responses.
    Unknown,
}

impl ProviderStateKind {
    /// Whether this state should mark the provider as having a problem in
    /// presentation surfaces (tone changes, hides usage details).
    pub const fn is_problem(self) -> bool {
        !matches!(self, ProviderStateKind::Ready)
    }
}

impl ProviderError {
    /// Classify this error into a presentation-safe state. Default mapping is
    /// variant-based; providers whose `ProviderError` variants carry
    /// provider-specific meaning override the pieces they must reinterpret
    /// (see `Provider::error_state_kind` implementations).
    pub fn state_kind(&self) -> ProviderStateKind {
        match self {
            // `NotInstalled` mostly reports missing credentials/keys (API
            // key or auth file absent) — a sign-in gate. Providers whose
            // probe genuinely means a local runtime or CLI binary is
            // missing override via `error_state_kind`.
            ProviderError::NotInstalled(_) => ProviderStateKind::NeedsAuthentication,
            // Expired tokens are "expired session"; other OAuth failures
            // (missing credentials, revoked, consent off) are sign-in gates.
            ProviderError::OAuthRevoked(_) => ProviderStateKind::NeedsAuthentication,
            ProviderError::OAuthExpired(_) => ProviderStateKind::ExpiredSession,
            ProviderError::OAuth(_) => ProviderStateKind::NeedsAuthentication,
            ProviderError::AuthRequired | ProviderError::NoCookies => {
                ProviderStateKind::NeedsAuthentication
            }
            ProviderError::Network(_)
            | ProviderError::Timeout
            | ProviderError::Parse(_)
            | ProviderError::UnsupportedSource(_)
            | ProviderError::Other(_) => ProviderStateKind::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_classification_matches_user_facing_meaning() {
        use ProviderError as E;
        use ProviderStateKind as K;

        assert_eq!(E::AuthRequired.state_kind(), K::NeedsAuthentication);
        assert_eq!(E::NoCookies.state_kind(), K::NeedsAuthentication);
        assert_eq!(
            E::OAuth("Claude OAuth credentials not found. Run `claude`.".into()).state_kind(),
            K::NeedsAuthentication
        );
        assert_eq!(
            E::OAuthExpired("Token expired. Run `claude login` to refresh.".into()).state_kind(),
            K::ExpiredSession
        );
        assert_eq!(
            E::OAuthExpired("Claude OAuth session expired and its stored refresh token was rejected by the server.".into()).state_kind(),
            K::ExpiredSession
        );
        assert_eq!(
            E::OAuthRevoked("revoked".into()).state_kind(),
            K::NeedsAuthentication
        );
        assert_eq!(
            E::NotInstalled("not found".into()).state_kind(),
            K::NeedsAuthentication
        );
        assert_eq!(E::Timeout.state_kind(), K::Unknown);
        // Real `reqwest::Error` built through the public API: a `file://`
        // request URL is rejected at build time, yielding a `BadScheme` error.
        assert_eq!(
            E::Network(
                reqwest::Client::new()
                    .get("file:///private.example.test/unreachable")
                    .build()
                    .unwrap_err()
            )
            .state_kind(),
            K::Unknown
        );
        assert_eq!(E::Parse("bad body".into()).state_kind(), K::Unknown);
        assert_eq!(E::Other("API error 500".into()).state_kind(), K::Unknown);
    }

    #[test]
    fn ready_is_the_only_non_problem_state() {
        assert!(!ProviderStateKind::Ready.is_problem());
        for kind in [
            ProviderStateKind::NeedsAuthentication,
            ProviderStateKind::ExpiredSession,
            ProviderStateKind::LocalRuntimeOffline,
            ProviderStateKind::Unknown,
        ] {
            assert!(kind.is_problem());
        }
    }

    #[test]
    fn serializes_as_camel_case_for_the_bridge() {
        assert_eq!(
            serde_json::to_value(ProviderStateKind::NeedsAuthentication).unwrap(),
            serde_json::json!("needsAuthentication")
        );
        assert_eq!(
            serde_json::to_value(ProviderStateKind::LocalRuntimeOffline).unwrap(),
            serde_json::json!("localRuntimeOffline")
        );
        assert_eq!(
            serde_json::from_value::<ProviderStateKind>(serde_json::json!("expiredSession"))
                .unwrap(),
            ProviderStateKind::ExpiredSession
        );
    }
}
