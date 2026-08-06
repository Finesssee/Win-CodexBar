//! Proof/debug harness for the Tauri desktop shell.
//!
//! Activated by the `CODEXBAR_PROOF_MODE` environment variable.  The value
//! specifies a target surface and optional settings tab to display on
//! startup, e.g.:
//!
//!   - `trayPanel`          — show the tray panel
//!   - `popOut`             — show the pop-out dashboard
//!   - `popOut:provider:codex` — show a provider pop-out
//!   - `settings`           — show settings (General tab)
//!   - `settings:menuBar`   — show settings on the Menu Bar tab
//!   - `settings:usageSpend` — show settings on the Usage & Spend tab
//!   - `settings:about`     — show settings on the About tab
//!
//! In proof mode the shell immediately transitions to the requested surface
//! and suppresses blur-dismiss so the window stays visible for automated
//! screenshot capture.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::shell;
use crate::state::AppState;
use crate::surface::SurfaceMode;
use crate::surface_target::{SurfaceTarget, is_supported_provider_id, is_supported_settings_tab};

/// Proof configuration parsed from `CODEXBAR_PROOF_MODE`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofConfig {
    /// The surface to show on startup (serialized as the camelCase id).
    pub target_surface: String,
    /// Optional settings tab id (e.g. `"menuBar"`, `"usageSpend"`).
    pub settings_tab: Option<String>,
    /// Optional target payload for richer proof routing, such as
    /// `"provider:codex"` for pop-out provider views.
    pub target_payload: Option<String>,
}

impl ProofConfig {
    /// Read proof configuration from the environment.
    ///
    /// Returns `None` when `CODEXBAR_PROOF_MODE` is unset or empty.
    pub fn from_env() -> Option<Self> {
        let raw = std::env::var("CODEXBAR_PROOF_MODE").ok()?;
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }

        let (surface_str, payload) = if let Some((s, t)) = raw.split_once(':') {
            (s, Some(t.to_string()))
        } else {
            (raw, None)
        };

        let Some(surface_mode) = SurfaceMode::parse(surface_str) else {
            tracing::warn!("CODEXBAR_PROOF_MODE: unknown surface '{surface_str}', ignoring");
            return None;
        };

        if !proof_payload_is_supported(surface_mode, payload.as_deref()) {
            tracing::warn!("CODEXBAR_PROOF_MODE: unsupported target '{raw}', ignoring");
            return None;
        }

        Some(ProofConfig {
            target_surface: surface_str.to_string(),
            settings_tab: (surface_str == SurfaceMode::Settings.as_str())
                .then_some(payload.clone())
                .flatten(),
            target_payload: payload,
        })
    }

    /// Resolve the target `SurfaceMode` enum value.
    pub fn surface_mode(&self) -> SurfaceMode {
        SurfaceMode::parse(&self.target_surface).unwrap_or(SurfaceMode::TrayPanel)
    }

    pub fn surface_target(&self) -> SurfaceTarget {
        match self.surface_mode() {
            SurfaceMode::Hidden | SurfaceMode::TrayPanel => SurfaceTarget::Summary,
            SurfaceMode::PopOut => self
                .target_payload
                .as_deref()
                .and_then(SurfaceTarget::parse)
                .filter(|target| target.mode() == SurfaceMode::PopOut)
                .unwrap_or(SurfaceTarget::Dashboard),
            SurfaceMode::Settings => SurfaceTarget::Settings {
                tab: self
                    .settings_tab
                    .clone()
                    .unwrap_or_else(|| "general".into()),
            },
        }
    }
}

/// Immediately transition to the proof-mode target surface.
///
/// Called from the Tauri `setup` closure when proof mode is active.
pub fn activate(app: &AppHandle) {
    let config = {
        let st = app.state::<Mutex<AppState>>();
        st.lock().unwrap().proof_config.clone()
    };

    let Some(config) = config else { return };
    let target = config.surface_mode();
    let position = match target {
        // Detached surfaces are larger than tray panels. Let their normal
        // positioning paths center/clamp them instead of reusing tray coords.
        SurfaceMode::Settings | SurfaceMode::PopOut => None,
        _ => proof_window_position(app),
    };
    tracing::info!(
        "proof-harness: activating surface={} tab={:?} position={:?}",
        config.target_surface,
        config.settings_tab,
        position,
    );

    match shell::transition_to_target(app, target, config.surface_target(), position) {
        Ok(mode) => tracing::info!("proof-harness: transition succeeded → {mode:?}"),
        Err(err) => tracing::error!("proof-harness: transition FAILED: {err}"),
    }
}

/// Bottom inset (physical px) kept between the proof panel's bottom edge and
/// the monitor work-area bottom (#265).
const PROOF_BOTTOM_INSET_PX: i32 = 8;

/// Mirror of the frontend auto-fit ceiling (`TRAY_MAX_MEASURE_HEIGHT`,
/// logical px in `useTrayPanelLayout.ts`): the proof panel can settle
/// anywhere up to this height.
const PROOF_MAX_SETTLE_HEIGHT_LOGICAL: f64 = 920.0;

/// Justified proof-panel anchor (#265). The harness anchors `main` once and
/// never re-anchors (the flyout-only `reanchor_tray_panel` path is a no-op
/// here, and Win32 ignores `set_max_size` for programmatic resizes), so an
/// anchor computed from the DEFAULT initial size lets a tall auto-fit settle
/// (up to the frontend's 920px cap) push the bottom edge under the taskbar.
/// When `anchor_y + max settle` would exceed `work_bottom - inset`, move the
/// anchor up just enough that even the tallest settle stays fully on screen;
/// otherwise keep the historical anchor unchanged.
fn proof_anchor_y(anchor_y: i32, work_bottom: i32, max_settle_height_px: i32) -> i32 {
    let justified = work_bottom - PROOF_BOTTOM_INSET_PX - max_settle_height_px;
    anchor_y.min(justified)
}

/// Bottom edge of a settled panel anchored via [`proof_anchor_y`].
#[cfg(test)]
fn proof_settled_bottom(
    anchor_y: i32,
    settled_height: i32,
    work_bottom: i32,
    max_settle_height_px: i32,
) -> i32 {
    proof_anchor_y(anchor_y, work_bottom, max_settle_height_px) + settled_height
}

/// Calculate a predictable window position for proof captures.
///
/// Proof mode needs a reliable on-screen position. We skip
/// `inferred_tray_panel_position` because its DPI-scaled maths can
/// produce off-screen coords on high-DPI setups.
fn proof_window_position(app: &AppHandle) -> Option<(i32, i32)> {
    let (x, y) = proof_window_position_unjustified(app)?;

    // #265: justify above the taskbar when the tallest legitimate auto-fit
    // settle (the frontend's 920px cap) would otherwise push the bottom edge
    // under it — the harness anchors `main` once and never re-anchors, so
    // the anchor itself must reserve the settle room.
    let Some(window) = app.get_webview_window("main") else {
        return Some((x, y));
    };
    let Some(m) = window
        .primary_monitor()
        .ok()
        .flatten()
        .or_else(|| window.available_monitors().ok()?.into_iter().next())
    else {
        return Some((x, y));
    };
    let work_area = m.work_area();
    let work_bottom = work_area.position.y + work_area.size.height as i32;
    let scale = m.scale_factor().max(1.0);
    let max_settle = (PROOF_MAX_SETTLE_HEIGHT_LOGICAL * scale) as i32;
    let justified_y = proof_anchor_y(y, work_bottom, max_settle);
    if justified_y != y {
        tracing::info!(
            "proof-pos: #265 justified anchor y={y} → {justified_y} \
             (work_bottom={work_bottom} max_settle={max_settle})"
        );
    }
    Some((x, justified_y))
}

/// Raw proof-capture anchor, before the #265 above-taskbar justification.
fn proof_window_position_unjustified(app: &AppHandle) -> Option<(i32, i32)> {
    if let Some(pos) = shell::tray_panel_position(app) {
        return Some(pos);
    }
    let window = app.get_webview_window("main")?;
    let monitor = window
        .primary_monitor()
        .ok()
        .flatten()
        .or_else(|| window.available_monitors().ok()?.into_iter().next());
    if let Some(m) = monitor {
        // Return coordinates in Tauri physical space (same as
        // monitor.size/work_area). transition.rs divides by scale before
        // calling set_position.
        let screen_w = m.size().width as i32;
        let work_area = m.work_area();
        let work_bottom = work_area.position.y + work_area.size.height as i32;
        let scale = m.scale_factor().max(1.0);
        let props = SurfaceMode::TrayPanel.window_properties();
        let panel_w = (props.width * scale) as i32;
        let panel_h = (props.height * scale) as i32;
        let margin = (12.0 * scale) as i32;
        let x = screen_w - panel_w - margin;
        let y = work_bottom - panel_h - margin;
        tracing::info!(
            "proof-pos: screen_w={screen_w} work_bottom={work_bottom} \
             panel={}x{} scale={scale} → ({x},{y})",
            panel_w,
            panel_h,
        );
        return Some((x, y));
    }
    Some((800, 25))
}

/// Returns `true` when proof mode is active in the shared state.
pub fn is_proof_mode(app: &AppHandle) -> bool {
    app.try_state::<Mutex<AppState>>()
        .map(|st| st.lock().unwrap().proof_config.is_some())
        .unwrap_or(false)
}

fn proof_payload_is_supported(surface_mode: SurfaceMode, payload: Option<&str>) -> bool {
    match (surface_mode, payload) {
        (SurfaceMode::Hidden | SurfaceMode::TrayPanel, None) => true,
        (SurfaceMode::Hidden | SurfaceMode::TrayPanel, Some(_)) => false,
        (SurfaceMode::Settings, None) => true,
        (SurfaceMode::Settings, Some(tab)) => is_supported_settings_tab(tab),
        (SurfaceMode::PopOut, None) => true,
        (SurfaceMode::PopOut, Some(raw_target)) => {
            let Some(target) = SurfaceTarget::parse(raw_target) else {
                return false;
            };

            match target {
                SurfaceTarget::Dashboard => true,
                SurfaceTarget::Provider { provider_id } => is_supported_provider_id(&provider_id),
                _ => false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::LazyLock;

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    #[test]
    fn tall_panel_bottom_edge_never_passes_work_bottom_minus_inset() {
        // 1920x1080 proof rig at 100% display scale: historical anchor y=252
        // (1040 work bottom - 776 default height - 12 margin); the tallest
        // legitimate settle is the frontend auto-fit cap, 920px.
        let max_settle = 920;
        // y is justified up to 112 so the capped settle bottoms out exactly
        // on the inset boundary...
        assert_eq!(proof_anchor_y(252, 1040, max_settle), 112);
        let bottom = proof_settled_bottom(252, max_settle, 1040, max_settle);
        assert!(bottom <= 1040 - PROOF_BOTTOM_INSET_PX);
        assert_eq!(bottom, 1032);
        // ...and any shorter settle keeps its bottom edge on screen too.
        assert!(proof_settled_bottom(252, 780, 1040, max_settle) <= 1040 - PROOF_BOTTOM_INSET_PX);
        assert!(proof_settled_bottom(252, 568, 1040, max_settle) <= 1040 - PROOF_BOTTOM_INSET_PX);
        // Tall displays where even the capped settle already fits keep the
        // historical anchor unchanged.
        assert_eq!(proof_anchor_y(252, 2000, max_settle), 252);
        // Degenerate anchors below the work bottom move fully above it.
        assert_eq!(proof_anchor_y(1500, 1040, max_settle), 112);
    }

    fn with_proof_mode_env(value: Option<&str>, test: impl FnOnce()) {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("CODEXBAR_PROOF_MODE").ok();

        match value {
            Some(value) => unsafe { std::env::set_var("CODEXBAR_PROOF_MODE", value) },
            None => unsafe { std::env::remove_var("CODEXBAR_PROOF_MODE") },
        }

        test();

        match prev {
            Some(prev) => unsafe { std::env::set_var("CODEXBAR_PROOF_MODE", prev) },
            None => unsafe { std::env::remove_var("CODEXBAR_PROOF_MODE") },
        }
    }

    #[test]
    fn parse_simple_surface() {
        with_proof_mode_env(Some("trayPanel"), || {
            let cfg = ProofConfig::from_env().unwrap();
            assert_eq!(cfg.target_surface, "trayPanel");
            assert!(cfg.settings_tab.is_none());
            assert_eq!(cfg.surface_mode(), SurfaceMode::TrayPanel);
        });
    }

    #[test]
    fn parse_settings_with_tab() {
        with_proof_mode_env(Some("settings:menuBar"), || {
            let cfg = ProofConfig::from_env().unwrap();
            assert_eq!(cfg.target_surface, "settings");
            assert_eq!(cfg.settings_tab.as_deref(), Some("menuBar"));
            assert_eq!(cfg.surface_mode(), SurfaceMode::Settings);
            assert_eq!(
                cfg.surface_target(),
                SurfaceTarget::Settings {
                    tab: "menuBar".into()
                }
            );
        });
    }

    #[test]
    fn parse_settings_about_proof_target() {
        with_proof_mode_env(Some("settings:about"), || {
            let cfg = ProofConfig::from_env().unwrap();
            assert_eq!(cfg.target_surface, "settings");
            assert_eq!(cfg.settings_tab.as_deref(), Some("about"));
        });
    }

    #[test]
    fn parse_provider_popout_proof_target() {
        with_proof_mode_env(Some("popOut:provider:codex"), || {
            let cfg = ProofConfig::from_env().unwrap();
            assert_eq!(cfg.target_surface, "popOut");
            assert_eq!(cfg.target_payload.as_deref(), Some("provider:codex"));
            assert_eq!(
                cfg.surface_target(),
                SurfaceTarget::Provider {
                    provider_id: "codex".into()
                }
            );
        });
    }

    #[test]
    fn empty_env_returns_none() {
        with_proof_mode_env(Some(""), || {
            assert!(ProofConfig::from_env().is_none());
        });
    }

    #[test]
    fn unset_env_returns_none() {
        with_proof_mode_env(None, || {
            assert!(ProofConfig::from_env().is_none());
        });
    }

    #[test]
    fn invalid_surface_returns_none() {
        with_proof_mode_env(Some("bogus"), || {
            assert!(ProofConfig::from_env().is_none());
        });
    }

    #[test]
    fn invalid_settings_tab_returns_none() {
        with_proof_mode_env(Some("settings:security"), || {
            assert!(ProofConfig::from_env().is_none());
        });
    }

    #[test]
    fn invalid_provider_target_returns_none() {
        with_proof_mode_env(Some("popOut:provider:not-a-provider"), || {
            assert!(ProofConfig::from_env().is_none());
        });
    }

    #[test]
    fn pop_out_surface() {
        with_proof_mode_env(Some("popOut"), || {
            let cfg = ProofConfig::from_env().unwrap();
            assert_eq!(cfg.surface_mode(), SurfaceMode::PopOut);
            assert_eq!(cfg.surface_target(), SurfaceTarget::Dashboard);
        });
    }
}
