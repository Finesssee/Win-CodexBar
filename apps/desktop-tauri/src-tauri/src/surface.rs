use serde::{Deserialize, Serialize};

/// The four surfaces the desktop shell can present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SurfaceMode {
    #[default]
    Hidden,
    TrayPanel,
    PopOut,
    Settings,
}

impl SurfaceMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hidden => "hidden",
            Self::TrayPanel => "trayPanel",
            Self::PopOut => "popOut",
            Self::Settings => "settings",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "hidden" => Some(Self::Hidden),
            "trayPanel" => Some(Self::TrayPanel),
            "popOut" => Some(Self::PopOut),
            "settings" => Some(Self::Settings),
            _ => None,
        }
    }

    /// Window properties that the shell must apply when entering this mode.
    pub fn window_properties(self) -> WindowProperties {
        match self {
            Self::Hidden => WindowProperties {
                visible: false,
                decorations: false,
                resizable: false,
                width: 0.0,
                height: 0.0,
                min_width: None,
                min_height: None,
                always_on_top: false,
                blur_dismiss: false,
                skip_taskbar: true,
            },
            // TrayPanel is the "Pop Out Dashboard" flyout: anchored above the
            // tray icon, always-on-top, auto-hides on click-outside (blur), and
            // never shows in the taskbar. It is resizable so the user can widen
            // or heighten it; the chosen size persists (position stays anchored).
            Self::TrayPanel => WindowProperties {
                visible: true,
                decorations: false,
                resizable: true,
                width: 328.0,
                height: 776.0,
                min_width: Some(300.0),
                min_height: Some(360.0),
                always_on_top: true,
                blur_dismiss: true,
                skip_taskbar: true,
            },
            // PopOut is the default "window mode": a normal, draggable,
            // resizable window that shows in the taskbar. This app draws its
            // own chrome (borderless + DWM dark caption + frontend drag
            // region), so PopOut must use `decorations: false` like the
            // working Settings window — native decorations are cancelled by
            // the WM_NCCALCSIZE subclass and produce no usable title bar.
            Self::PopOut => WindowProperties {
                visible: true,
                decorations: false,
                resizable: true,
                width: 420.0,
                height: 680.0,
                min_width: Some(320.0),
                min_height: Some(240.0),
                always_on_top: false,
                blur_dismiss: false,
                skip_taskbar: false,
            },
            Self::Settings => WindowProperties {
                visible: true,
                decorations: true,
                resizable: true,
                width: 496.0,
                height: 580.0,
                min_width: None,
                min_height: None,
                always_on_top: false,
                blur_dismiss: false,
                skip_taskbar: false,
            },
        }
    }
}

/// Describes what the window should look like in a given surface mode.
#[derive(Debug, Clone)]
pub struct WindowProperties {
    pub visible: bool,
    pub decorations: bool,
    pub resizable: bool,
    pub width: f64,
    pub height: f64,
    pub min_width: Option<f64>,
    pub min_height: Option<f64>,
    pub always_on_top: bool,
    /// Whether the window should auto-hide when it loses focus.
    #[allow(
        dead_code,
        reason = "surface helper reserved for future window management integration"
    )]
    pub blur_dismiss: bool,
    /// Whether the window should be hidden from the Windows taskbar. Widget
    /// surfaces (TrayPanel) stay hidden; the PopOut window mode shows there.
    pub skip_taskbar: bool,
}

/// Returned by the state machine when a transition succeeds.
#[derive(Debug)]
pub struct SurfaceTransition {
    pub from: SurfaceMode,
    pub to: SurfaceMode,
    pub properties: WindowProperties,
}

/// Tracks the current surface mode and validates transitions.
pub struct SurfaceStateMachine {
    current: SurfaceMode,
}

impl Default for SurfaceStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl SurfaceStateMachine {
    pub fn new() -> Self {
        Self {
            current: SurfaceMode::Hidden,
        }
    }

    pub fn current(&self) -> SurfaceMode {
        self.current
    }

    /// Attempt to transition to `target`. Returns `None` if already in that mode.
    pub fn transition(&mut self, target: SurfaceMode) -> Option<SurfaceTransition> {
        if self.current == target {
            return None;
        }
        let from = self.current;
        self.current = target;
        Some(SurfaceTransition {
            from,
            to: target,
            properties: target.window_properties(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use crate::surface_target::SurfaceTarget;

    #[test]
    fn starts_hidden() {
        let sm = SurfaceStateMachine::new();
        assert_eq!(sm.current(), SurfaceMode::Hidden);
    }

    #[test]
    fn app_state_starts_hidden_with_summary_target() {
        let state = AppState::new();
        assert_eq!(state.surface_machine.current(), SurfaceMode::Hidden);
        assert_eq!(state.current_target, SurfaceTarget::Summary);
    }

    #[test]
    fn noop_for_same_mode() {
        let mut sm = SurfaceStateMachine::new();
        assert!(sm.transition(SurfaceMode::Hidden).is_none());
    }

    #[test]
    fn hidden_to_tray_panel() {
        let mut sm = SurfaceStateMachine::new();
        let t = sm.transition(SurfaceMode::TrayPanel).unwrap();
        assert_eq!(t.from, SurfaceMode::Hidden);
        assert_eq!(t.to, SurfaceMode::TrayPanel);
        assert!(t.properties.visible);
        assert!(!t.properties.decorations);
        assert!(t.properties.always_on_top);
        assert!(t.properties.blur_dismiss);
        assert_eq!(sm.current(), SurfaceMode::TrayPanel);
    }

    #[test]
    fn tray_panel_to_pop_out() {
        let mut sm = SurfaceStateMachine::new();
        sm.transition(SurfaceMode::TrayPanel);
        let t = sm.transition(SurfaceMode::PopOut).unwrap();
        assert_eq!(t.from, SurfaceMode::TrayPanel);
        // PopOut is borderless (custom DWM chrome), resizable, shows in the
        // taskbar, and never blur-dismisses.
        assert!(!t.properties.decorations);
        assert!(t.properties.resizable);
        assert!(!t.properties.blur_dismiss);
        assert!(!t.properties.skip_taskbar);
    }

    #[test]
    fn pop_out_to_settings() {
        let mut sm = SurfaceStateMachine::new();
        sm.transition(SurfaceMode::PopOut);
        let t = sm.transition(SurfaceMode::Settings).unwrap();
        assert_eq!(t.from, SurfaceMode::PopOut);
        assert_eq!(t.to, SurfaceMode::Settings);
        assert!(t.properties.decorations);
        assert!(t.properties.resizable);
    }

    #[test]
    fn settings_to_hidden() {
        let mut sm = SurfaceStateMachine::new();
        sm.transition(SurfaceMode::Settings);
        let t = sm.transition(SurfaceMode::Hidden).unwrap();
        assert!(!t.properties.visible);
    }

    #[test]
    fn round_trip_all_modes() {
        let mut sm = SurfaceStateMachine::new();
        for mode in [
            SurfaceMode::TrayPanel,
            SurfaceMode::PopOut,
            SurfaceMode::Settings,
            SurfaceMode::Hidden,
        ] {
            let t = sm.transition(mode).unwrap();
            assert_eq!(t.to, mode);
        }
    }

    #[test]
    fn parse_round_trip() {
        for mode in [
            SurfaceMode::Hidden,
            SurfaceMode::TrayPanel,
            SurfaceMode::PopOut,
            SurfaceMode::Settings,
        ] {
            assert_eq!(SurfaceMode::parse(mode.as_str()), Some(mode));
        }
    }

    #[test]
    fn parse_unknown_returns_none() {
        assert_eq!(SurfaceMode::parse("bogus"), None);
    }

    #[test]
    fn tray_panel_properties() {
        let props = SurfaceMode::TrayPanel.window_properties();
        assert_eq!(props.width, 328.0);
        assert_eq!(props.height, 776.0);
    }

    #[test]
    fn tray_panel_is_resizable_blur_dismiss_flyout() {
        let props = SurfaceMode::TrayPanel.window_properties();
        // "Pop Out Dashboard" flyout: resizable, anchored, auto-hide, no taskbar.
        assert!(props.resizable);
        assert!(props.blur_dismiss);
        assert!(props.always_on_top);
        assert!(props.skip_taskbar);
        assert!(!props.decorations);
        assert_eq!(props.min_width, Some(300.0));
        assert_eq!(props.min_height, Some(360.0));
    }

    #[test]
    fn settings_properties() {
        let props = SurfaceMode::Settings.window_properties();
        assert_eq!(props.width, 496.0);
        assert_eq!(props.height, 580.0);
    }

    #[test]
    fn pop_out_min_size() {
        let props = SurfaceMode::PopOut.window_properties();
        assert_eq!(props.min_width, Some(320.0));
        assert_eq!(props.min_height, Some(240.0));
    }
}
