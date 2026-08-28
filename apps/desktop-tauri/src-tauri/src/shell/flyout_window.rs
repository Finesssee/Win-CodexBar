//! Detached "Pop Out Dashboard" flyout window: a resizable, always-on-top,
//! tray-anchored panel that auto-hides on click-outside (blur-dismiss).
//!
//! Runs as an auxiliary Tauri window labeled `flyout`, independent of the
//! `main` window's surface state machine — it coexists with "Show Window"
//! (`SurfaceMode::PopOut`, which stays on `main`) instead of being a
//! mutually-exclusive state of the same window.
//!
//! Structurally modeled on `crate::floatbar` (self-contained module owning
//! its window + a `handle_window_event` hook dispatched from `main.rs`
//! before the `main`-window-only handling); the window itself is built with
//! `settings_window.rs`'s builder recipe (async open, manual DWM dark-caption
//! pass, `WebviewUrl::App` with a `?window=` query marker).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, PhysicalPosition, WebviewUrl};

use crate::geometry_store::{self, StoredSize};
use crate::state::AppState;
use crate::surface::SurfaceMode;

pub const FLYOUT_LABEL: &str = "flyout";

/// Geometry-store key for the flyout's remembered SIZE (position is never
/// stored — the flyout always re-anchors above the tray on open). Kept as
/// its own key (distinct from the legacy `SurfaceMode::TrayPanel::as_str()`
/// `"trayPanel"` key) — `geometry_store::load_size` migrates a pre-existing
/// `"trayPanel"` entry into this key on first read, so upgrading users keep
/// their remembered flyout size.
const FLYOUT_SIZE_KEY: &str = "flyout";

/// Same window used to close a same-click blur-dismiss/reopen race as the
/// pre-split tray panel handling (formerly `shell::transition::handle_tray_panel_click`,
/// removed once its only caller — the tray-icon left-click handler — was
/// retargeted to call this module directly).
const BLUR_DISMISS_CLICK_WINDOW: Duration = Duration::from_millis(250);
/// Grace period after showing the flyout during which a spurious Windows
/// blur (tray click focus race) is ignored — mirrors `main.rs`'s 500ms
/// `was_tray_panel_recently_shown` guard for the old shared window.
const RECENTLY_SHOWN_GRACE: Duration = Duration::from_millis(500);

/// Read the remembered flyout size, if any (migrating a legacy
/// `"trayPanel"`-keyed size on first read — see `geometry_store::load_size`).
pub fn stored_size() -> Option<(u32, u32)> {
    geometry_store::load_size(FLYOUT_SIZE_KEY).map(|size| (size.width, size.height))
}

/// Persist a user-chosen flyout size. Size-only — no fabricated position.
pub fn save_stored_size(width: u32, height: u32) {
    geometry_store::save_size(FLYOUT_SIZE_KEY, StoredSize { width, height });
}

/// Whether the flyout window currently exists and is visible. Canonical
/// replacement for the pre-split `surface_machine.current() == TrayPanel`
/// check, now that the flyout is not a state of the shared machine.
pub fn is_open(app: &AppHandle) -> bool {
    app.get_webview_window(FLYOUT_LABEL)
        .is_some_and(|w| w.is_visible().unwrap_or(false))
}

/// Build (first open) or show + focus (subsequent opens) the flyout window at
/// `position`, if given, else the default tray-anchored position.
///
/// `WebviewWindowBuilder::build` deadlocks when called synchronously from a
/// Tauri command on Windows (see `commands/surface.rs::open_settings_window`
/// precedent) — callers must invoke this from an async context (an `async`
/// command, or `tauri::async_runtime::spawn`), never a sync command handler.
pub fn open_or_focus(app: &AppHandle, position: Option<(i32, i32)>) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(FLYOUT_LABEL) {
        if let Some((x, y)) = position {
            // Best-effort reposition before show; the subsequent show/focus
            // is what the user sees, so the position result is non-fatal.
            let _set_position = window.set_position(PhysicalPosition::new(x, y));
        } else {
            reanchor(app)?;
        }
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
        if show_grace_starts_now(false) {
            mark_shown(app);
        }
        return Ok(());
    }

    // Derive window properties from `SurfaceMode::TrayPanel.window_properties()`
    // — the historical single source of truth for the flyout's shape (size,
    // resizability, always-on-top, taskbar visibility). The variant is kept
    // specifically so this builder (and the geometry-store key, and
    // `default_surface_position`'s positioning branch) have one place to read
    // from, rather than duplicating these values as independent constants
    // that could silently drift from `surface.rs`.
    let props = SurfaceMode::TrayPanel.window_properties();
    let (width, height) = stored_size()
        .map(|(w, h)| (w as f64, h as f64))
        .unwrap_or((props.width, props.height));

    let url = WebviewUrl::App("index.html?window=flyout".into());

    let mut builder = tauri::WebviewWindowBuilder::new(app, FLYOUT_LABEL, url)
        .title("CodexBar")
        .inner_size(width, height)
        .decorations(props.decorations)
        .shadow(false)
        .resizable(props.resizable)
        .always_on_top(props.always_on_top)
        .skip_taskbar(props.skip_taskbar)
        .theme(Some(tauri::Theme::Dark))
        // CRITICAL: dynamically-built windows default to drag-drop ENABLED,
        // which intercepts the HTML5 draggable events the provider grid's
        // drag-reorder (ProviderGrid.tsx) relies on — see `main`'s
        // `dragDropEnabled: false` in tauri.conf.json for why this must be
        // disabled explicitly on every window that hosts that grid.
        .disable_drag_drop_handler()
        .visible(false);
    if let (Some(min_w), Some(min_h)) = (props.min_width, props.min_height) {
        builder = builder.min_inner_size(min_w, min_h);
    }
    let win = builder.build().map_err(|e| e.to_string())?;

    // Force DWM caption dark; keep WS_THICKFRAME (resizable) like the
    // Settings window.
    super::dwm::force_dark_caption_resizable(&win);

    let target_position =
        position.or_else(|| super::position::default_surface_position(app, SurfaceMode::TrayPanel));
    if let Some((x, y)) = target_position {
        // Best-effort initial placement; the frontend re-reveals the
        // window, so a failed set_position here is non-fatal.
        let _set_initial = win.set_position(PhysicalPosition::new(x, y));
    }

    // Left `.visible(false)` above — the frontend reveals the window itself
    // after its first layout pass, then `reveal_tray_panel_window` starts the
    // recently-shown blur grace at the real show/focus boundary.
    if show_grace_starts_now(true) {
        mark_shown(app);
    }
    arm_reveal(app)?;
    Ok(())
}

fn show_grace_starts_now(first_build_hidden: bool) -> bool {
    !first_build_hidden
}

fn arm_reveal(app: &AppHandle) -> Result<(), String> {
    let state = app
        .try_state::<Mutex<AppState>>()
        .ok_or_else(|| "app state unavailable".to_string())?;
    state.lock().map_err(|e| e.to_string())?.arm_flyout_reveal();
    Ok(())
}

/// Toggle the flyout: hide if open, open (or focus) otherwise. Consumes a
/// same-click blur-dismissal first (see `handle_window_event`'s
/// `Focused(false)` path) so a tray click that just blur-dismissed the
/// flyout cleanly closes it instead of instantly reopening.
///
/// Must be called from an async context — see [`open_or_focus`].
pub fn toggle_with_blur_consume(app: &AppHandle, position: Option<(i32, i32)>) {
    let consumed_blur_dismissal = {
        let st = app.state::<Mutex<AppState>>();
        st.lock()
            .unwrap()
            .take_recent_blur_dismissal(Instant::now(), BLUR_DISMISS_CLICK_WINDOW)
    };

    if consumed_blur_dismissal {
        return;
    }

    if is_open(app) {
        // Tray-toggle is fire-and-forget; a hide error leaves the window
        // closed from the user's perspective and is non-fatal.
        let _hide = hide(app);
    } else {
        // Tray-toggle is fire-and-forget; open_or_focus surfaces its own
        // errors via tracing, so the toggle call site discards the result.
        let _open = open_or_focus(app, position);
    }
}

/// Hide (never close) the flyout window. Hiding — rather than closing —
/// keeps the window's WebView2 instance alive across opens, matching
/// `settings_window::dismiss`'s rationale: closing risks Tauri's
/// process/window lifecycle treating it as an app-relevant close.
pub fn hide(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(FLYOUT_LABEL) {
        let state = app
            .try_state::<Mutex<AppState>>()
            .ok_or_else(|| "app state unavailable".to_string())?;
        state
            .lock()
            .map_err(|e| e.to_string())?
            .clear_flyout_reveal();
        window.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn mark_shown(app: &AppHandle) {
    if let Some(st) = app.try_state::<Mutex<AppState>>()
        && let Ok(mut guard) = st.lock()
    {
        guard.mark_tray_panel_shown(Instant::now());
    }
}

/// Handle a `WindowEvent` targeting the flyout window. Returns `true` when
/// the event was for the flyout (and was handled), `false` otherwise so the
/// caller (`main.rs`'s single `on_window_event` dispatcher) can fall through
/// to its own `main`-window-only handling.
///
/// Ports the same four-guard chain `main.rs` applies to the old shared
/// window's `Focused(false)` (proof-mode suppression / startup grace /
/// recently-shown 500ms grace / gesture blur guard) so every previously
/// shipped anti-flicker behavior survives the window split.
pub fn handle_window_event(window: &tauri::Window, event: &tauri::WindowEvent) -> bool {
    if window.label() != FLYOUT_LABEL {
        return false;
    }

    let app = window.app_handle();

    match event {
        tauri::WindowEvent::Focused(false) => {
            if crate::proof_harness::is_proof_mode(app) {
                return true;
            }
            let Some(st) = app.try_state::<Mutex<AppState>>() else {
                return true;
            };
            {
                let mut guard = st.lock().unwrap();
                if guard.take_startup_tray_blur_grace(Instant::now()) {
                    return true;
                }
                if guard.was_tray_panel_recently_shown(Instant::now(), RECENTLY_SHOWN_GRACE) {
                    return true;
                }
                if guard.is_gesture_blur_guard_active(Instant::now()) {
                    return true;
                }
            }
            if hide(app).is_ok() {
                st.lock().unwrap().mark_blur_dismissed(Instant::now());
            }
            true
        }
        tauri::WindowEvent::Focused(true) => {
            if let Some(st) = app.try_state::<Mutex<AppState>>() {
                st.lock()
                    .unwrap()
                    .clear_gesture_guard_on_refocus(Instant::now());
            }
            true
        }
        // Size persistence is entirely frontend-driven (genuine user
        // drag-resizes call `set_flyout_size`, auto-fit resizes never do) —
        // mirrors `shell::position::remember_current_geometry_if_eligible`
        // skipping TrayPanel for the same reason on the old shared window.
        // Position is never persisted (always re-anchored above the tray).
        tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => true,
        tauri::WindowEvent::CloseRequested { api, .. } => {
            // Hide-not-close on a native close request (Alt+F4-equivalent from
            // a screen reader): the flyout survives so it can be reopened without
            // rebuilding the WebView2 instance. A hide failure is non-fatal and
            // the window stays for reuse.
            api.prevent_close();
            let _hide_on_close = hide(app);
            true
        }
        _ => true,
    }
}

/// Reposition the flyout so its bottom-right corner stays anchored to the
/// system-tray area, using the window's CURRENT logical size (after a
/// frontend-driven resize). Canonical anchor-math implementation for the
/// flyout window; the `reanchor_tray_panel` Tauri command
/// (`commands/system.rs`) is a thin retarget onto this function.
pub fn reanchor(app: &AppHandle) -> Result<(), String> {
    use crate::window_positioner::{PanelSize, Rect};

    let window = app
        .get_webview_window(FLYOUT_LABEL)
        .ok_or_else(|| "flyout window unavailable".to_string())?;
    let scale = window.scale_factor().unwrap_or(1.0).max(1.0);

    let outer = window.outer_size().map_err(|e| e.to_string())?;
    let panel_size = PanelSize {
        // Rounded logical px fit u32 by design (whole physical px / scale).
        width: (outer.width as f64 / scale).round() as u32,
        height: (outer.height as f64 / scale).round() as u32,
    };

    let anchor = app
        .try_state::<Mutex<AppState>>()
        .and_then(|state| state.lock().ok()?.tray_anchor);
    let monitors = window.available_monitors().unwrap_or_default();
    let monitor = anchor
        .and_then(|anchor| crate::shell::geometry::monitor_for_anchor(&monitors, anchor))
        .cloned()
        .or_else(|| window.current_monitor().ok().flatten())
        .or_else(|| window.primary_monitor().ok().flatten())
        .ok_or_else(|| "no monitor".to_string())?;

    let work_area = crate::shell::geometry::monitor_work_area_rect(&monitor);
    let monitor_bounds = Rect {
        x: monitor.position().x,
        y: monitor.position().y,
        width: monitor.size().width,
        height: monitor.size().height,
    };

    let (x, y) = {
        if let Some(a) = anchor {
            crate::window_positioner::calculate_panel_position(
                &Rect {
                    x: a.x,
                    y: a.y,
                    width: a.width,
                    height: a.height,
                },
                &monitor_bounds,
                &work_area,
                &panel_size,
                scale,
            )
        } else {
            // No real click anchor yet: infer one from the taskbar side
            // (handles left/right/top-docked taskbars, not just bottom-right).
            crate::shell::inferred_tray_panel_position_for_monitor_size(&monitor, &panel_size)
        }
    };

    // Pass physical coordinates directly — tao converts PhysicalPosition to
    // OS logical internally by dividing by the window's scale factor.
    let pos = tauri::PhysicalPosition::new(x, y);
    tracing::debug!(
        "flyout_window::reanchor: panel={}x{} => ({},{})",
        panel_size.width,
        panel_size.height,
        pos.x,
        pos.y
    );
    // Best-effort re-anchor after resize; the position result is non-fatal
    // and the next geometry event re-anchors again.
    let _set_position = window.set_position(pos);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flyout_label_is_stable() {
        assert_eq!(FLYOUT_LABEL, "flyout");
    }

    #[test]
    fn flyout_size_key_is_distinct_from_legacy_tray_panel_key() {
        // The whole point of the migration in geometry_store::load_size is
        // that this key differs from the legacy SurfaceMode::TrayPanel key
        // ("trayPanel") — otherwise there'd be nothing to migrate FROM.
        assert_ne!(FLYOUT_SIZE_KEY, SurfaceMode::TrayPanel.as_str());
    }

    #[test]
    fn tray_panel_window_properties_still_the_single_source_for_flyout_shape() {
        // `open_or_focus`'s builder reads size/decorations/resizable/
        // always_on_top/skip_taskbar/min-size from
        // `SurfaceMode::TrayPanel.window_properties()` directly (not
        // independent duplicated constants) — this pins down the values that
        // relationship depends on, so a change to `surface.rs` shows up here
        // instead of silently drifting from what the flyout actually builds.
        let props = SurfaceMode::TrayPanel.window_properties();
        assert_eq!(props.width, 328.0);
        assert_eq!(props.height, 776.0);
        assert_eq!(props.min_width, Some(300.0));
        assert_eq!(props.min_height, Some(360.0));
        assert!(props.resizable);
        assert!(props.always_on_top);
        assert!(props.skip_taskbar);
        assert!(!props.decorations);
    }

    #[test]
    fn first_hidden_build_does_not_start_show_grace() {
        assert!(show_grace_starts_now(false));
        assert!(!show_grace_starts_now(true));
    }
}
