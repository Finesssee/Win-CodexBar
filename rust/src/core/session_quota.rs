//! Session Quota Notifications
//!
//! Monitors session quota changes and sends notifications when:
//! - Quota becomes depleted (0% remaining)
//! - Quota is restored (becomes available again)

#![allow(
    dead_code,
    reason = "quota types reserved for future session-limit enforcement"
)]
#![allow(
    unused_imports,
    reason = "imports needed for future session-limit enforcement wiring"
)]

use crate::core::ProviderId;

/// Threshold for considering quota as depleted (0.01%)
const DEPLETED_THRESHOLD: f64 = 0.0001;

/// Session quota transition states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionQuotaTransition {
    /// No change in quota state
    None,
    /// Quota just became depleted
    Depleted,
    /// Quota was restored after being depleted
    Restored,
}

/// Logic for detecting session quota transitions
pub struct SessionQuotaLogic;

impl SessionQuotaLogic {
    /// Check if remaining percentage is considered depleted
    pub fn is_depleted(remaining: Option<f64>) -> bool {
        match remaining {
            Some(r) => r <= DEPLETED_THRESHOLD,
            None => false,
        }
    }

    /// Detect transition between previous and current remaining percentages
    pub fn transition(
        previous_remaining: Option<f64>,
        current_remaining: Option<f64>,
    ) -> SessionQuotaTransition {
        let Some(current) = current_remaining else {
            return SessionQuotaTransition::None;
        };
        let Some(previous) = previous_remaining else {
            return SessionQuotaTransition::None;
        };

        let was_depleted = previous <= DEPLETED_THRESHOLD;
        let is_depleted = current <= DEPLETED_THRESHOLD;

        if !was_depleted && is_depleted {
            SessionQuotaTransition::Depleted
        } else if was_depleted && !is_depleted {
            SessionQuotaTransition::Restored
        } else {
            SessionQuotaTransition::None
        }
    }
}

/// Session quota notifier that tracks state and sends notifications
pub struct SessionQuotaNotifier {
    /// Previous remaining percentages by provider
    previous_remaining: std::collections::HashMap<ProviderId, f64>,
}

impl SessionQuotaNotifier {
    /// Create a new notifier
    pub fn new() -> Self {
        Self {
            previous_remaining: std::collections::HashMap::new(),
        }
    }

    /// Update quota for a provider and send notification if state changed
    pub fn update(&mut self, provider: ProviderId, current_remaining: Option<f64>) {
        let previous = self.previous_remaining.get(&provider).copied();
        let transition = SessionQuotaLogic::transition(previous, current_remaining);

        if transition != SessionQuotaTransition::None {
            self.post_notification(transition, provider);
        }

        // Store current as previous for next comparison
        if let Some(remaining) = current_remaining {
            self.previous_remaining.insert(provider, remaining);
        }
    }

    /// Post a notification for the given transition
    fn post_notification(&self, transition: SessionQuotaTransition, provider: ProviderId) {
        let provider_name = provider.display_name();

        let (title, body) = match transition {
            SessionQuotaTransition::None => return,
            SessionQuotaTransition::Depleted => (
                format!("{} session depleted", provider_name),
                "0% left. Will notify when it's available again.".to_string(),
            ),
            SessionQuotaTransition::Restored => (
                format!("{} session restored", provider_name),
                "Session quota is available again.".to_string(),
            ),
        };

        tracing::info!(
            provider = %provider.cli_name(),
            transition = ?transition,
            "Session quota notification: {} - {}",
            title,
            body
        );

        // Note: Actual toast notification is handled by NotificationManager.check_session_transition()
        // This function is for logging and integration with the notification system
    }

    /// Reset tracking for a provider
    pub fn reset(&mut self, provider: ProviderId) {
        self.previous_remaining.remove(&provider);
    }

    /// Reset all tracking
    pub fn reset_all(&mut self) {
        self.previous_remaining.clear();
    }
}

impl Default for SessionQuotaNotifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_depleted() {
        assert!(SessionQuotaLogic::is_depleted(Some(0.0)));
        assert!(SessionQuotaLogic::is_depleted(Some(0.00001)));
        assert!(!SessionQuotaLogic::is_depleted(Some(0.01)));
        assert!(!SessionQuotaLogic::is_depleted(Some(50.0)));
        assert!(!SessionQuotaLogic::is_depleted(None));
    }

    #[test]
    fn test_transition_none() {
        // No change when both are above threshold
        assert_eq!(
            SessionQuotaLogic::transition(Some(50.0), Some(40.0)),
            SessionQuotaTransition::None
        );
        // No change when both are below threshold
        assert_eq!(
            SessionQuotaLogic::transition(Some(0.0), Some(0.0)),
            SessionQuotaTransition::None
        );
        // No change with None values
        assert_eq!(
            SessionQuotaLogic::transition(None, Some(50.0)),
            SessionQuotaTransition::None
        );
        assert_eq!(
            SessionQuotaLogic::transition(Some(50.0), None),
            SessionQuotaTransition::None
        );
    }

    #[test]
    fn test_transition_depleted() {
        assert_eq!(
            SessionQuotaLogic::transition(Some(10.0), Some(0.0)),
            SessionQuotaTransition::Depleted
        );
        assert_eq!(
            SessionQuotaLogic::transition(Some(0.001), Some(0.00001)),
            SessionQuotaTransition::Depleted
        );
    }

    #[test]
    fn test_transition_restored() {
        assert_eq!(
            SessionQuotaLogic::transition(Some(0.0), Some(10.0)),
            SessionQuotaTransition::Restored
        );
        assert_eq!(
            SessionQuotaLogic::transition(Some(0.00001), Some(1.0)),
            SessionQuotaTransition::Restored
        );
    }
}
