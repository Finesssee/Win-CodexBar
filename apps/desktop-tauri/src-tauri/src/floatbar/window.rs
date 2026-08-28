//! Detached "FloatBar" window: a small always-on-top transparent strip
//! that shows remaining capacity per provider. Runs as an auxiliary
//! Tauri window labeled `floatbar`, independent of the main surface
//! state machine.

use tauri::{LogicalPosition, LogicalSize, Manager, PhysicalPosition, PhysicalSize, WebviewUrl};

use crate::geometry_store;

pub const FLOATBAR_LABEL: &str = "floatbar";
pub const FLOAT_BAR_CONFIG_CHANGED_EVENT: &str = "float-bar-config-changed";
const FLOATBAR_DEFAULT_WIDTH_H: f64 = 360.0;
const FLOATBAR_DEFAULT_HEIGHT_H: f64 = 36.0;
const FLOATBAR_DEFAULT_WIDTH_V: f64 = 80.0;
const FLOATBAR_DEFAULT_HEIGHT_V: f64 = 280.0;
const WINDOWS_MINIMIZED_COORDINATE: i32 = -32_000;
/// Stored geometry is logical. Physical park (-32000,-32000) becomes about
/// -25600 @1.25x or -16000 @2x after scale divide. Both axes at/below this
/// threshold are treated as unusable while multi-monitor origins like
/// (-3840, 0) or (-8000, -8000) still restore.
const STORED_LOGICAL_PARK_THRESHOLD: i32 = -15_000;

/// Windows parks minimized windows at the exact physical coordinate pair
/// (-32000, -32000). Keep this strict so legitimate negative monitor origins
/// are never mistaken for minimized geometry.
fn is_windows_minimized_position(x: i32, y: i32) -> bool {
    x == WINDOWS_MINIMIZED_COORDINATE && y == WINDOWS_MINIMIZED_COORDINATE
}

/// Reject stored (logical) positions that came from a Windows park coordinate
/// at any common DPI scale, not only exact logical (-32000, -32000).
fn is_unusable_stored_logical_position(x: i32, y: i32) -> bool {
    if is_windows_minimized_position(x, y) {
        return true;
    }
    x <= STORED_LOGICAL_PARK_THRESHOLD && y <= STORED_LOGICAL_PARK_THRESHOLD
}

fn should_remember_physical_position(is_minimized: bool, x: i32, y: i32) -> bool {
    !is_minimized && !is_windows_minimized_position(x, y)
}

fn physical_rects_intersect(
    first_position: PhysicalPosition<i32>,
    first_size: PhysicalSize<u32>,
    second_position: PhysicalPosition<i32>,
    second_size: PhysicalSize<u32>,
) -> bool {
    let first_left = i64::from(first_position.x);
    let first_top = i64::from(first_position.y);
    let first_right = first_left + i64::from(first_size.width);
    let first_bottom = first_top + i64::from(first_size.height);
    let second_left = i64::from(second_position.x);
    let second_top = i64::from(second_position.y);
    let second_right = second_left + i64::from(second_size.width);
    let second_bottom = second_top + i64::from(second_size.height);

    first_left < second_right
        && first_right > second_left
        && first_top < second_bottom
        && first_bottom > second_top
}

/// Default first-open / recovery placement: center-X, taskbar bottom or
/// floating top, with 8 logical px edge padding. `mon_*` and size are logical.
fn default_logical_origin(
    mon_x: f64,
    mon_y: f64,
    mon_w: f64,
    mon_h: f64,
    window_w: f64,
    window_h: f64,
    style: &str,
) -> (f64, f64) {
    let x = mon_x + (mon_w - window_w) / 2.0;
    let y = if style == "taskbar" {
        mon_y + mon_h - window_h - 8.0
    } else {
        mon_y + 8.0
    };
    (x.max(mon_x), y.max(mon_y))
}

/// Physical fallback placement on a monitor work area (safe recovery target).
fn fallback_physical_position(
    work_area_position: PhysicalPosition<i32>,
    work_area_size: PhysicalSize<u32>,
    window_size: PhysicalSize<u32>,
    scale_factor: f64,
    style: &str,
) -> PhysicalPosition<i32> {
    let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    let mon_x = work_area_position.x as f64 / scale_factor;
    let mon_y = work_area_position.y as f64 / scale_factor;
    let mon_w = work_area_size.width as f64 / scale_factor;
    let mon_h = work_area_size.height as f64 / scale_factor;
    let window_w = window_size.width as f64 / scale_factor;
    let window_h = window_size.height as f64 / scale_factor;
    let (x, y) = default_logical_origin(mon_x, mon_y, mon_w, mon_h, window_w, window_h, style);
    PhysicalPosition::new(
        (x * scale_factor).round() as i32,
        (y * scale_factor).round() as i32,
    )
}

/// True when the window rectangle intersects any connected display's full
/// bounds (not just the work area). Taskbar-style bars may sit in the
/// reserved taskbar strip and must still count as on-screen.
fn intersects_any_monitor(
    position: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
    monitors: &[tauri::Monitor],
) -> bool {
    monitors.iter().any(|monitor| {
        physical_rects_intersect(position, size, *monitor.position(), *monitor.size())
    })
}

/// Probe: live window sits on no connected display (parked, disconnected
/// monitor, etc.). Returns `false` when geometry cannot be read so callers
/// do not treat "unknown" as recovery-needed.
pub(super) fn is_off_all_monitors<R: tauri::Runtime, M: WindowGeometry<R>>(window: &M) -> bool {
    let Ok(position) = window.outer_position() else {
        return false;
    };
    let Ok(size) = window.outer_size() else {
        return false;
    };
    let Ok(monitors) = window.available_monitors() else {
        return false;
    };
    if monitors.is_empty() {
        return false;
    }
    !intersects_any_monitor(position, size, &monitors)
}

/// Recover: unminimize if needed, place on the primary work area using
/// `style` for top/bottom, and persist the recovered geometry. Callers must
/// supply style (settings stay out of this module).
pub(super) fn recover_onto_primary<R: tauri::Runtime, M: WindowGeometry<R>>(
    window: &M,
    style: &str,
) -> bool {
    let Ok(position) = window.outer_position() else {
        return false;
    };
    let Ok(size) = window.outer_size() else {
        return false;
    };
    let Ok(monitors) = window.available_monitors() else {
        return false;
    };
    let target_monitor = window
        .primary_monitor()
        .ok()
        .flatten()
        .or_else(|| monitors.into_iter().next());
    let Some(target_monitor) = target_monitor else {
        return false;
    };

    // Placement uses the work area so recovery lands in a usable desktop
    // region; visibility probes use full monitor bounds (see
    // `intersects_any_monitor`).
    let work_area = target_monitor.work_area();
    let target = fallback_physical_position(
        work_area.position,
        work_area.size,
        size,
        target_monitor.scale_factor(),
        style,
    );

    if window.is_minimized().unwrap_or(false)
        || is_windows_minimized_position(position.x, position.y)
    {
        let _ = window.unminimize();
    }
    if window.set_physical_position(target).is_err() {
        return false;
    }
    // Atomic: persist recovered geometry here so event handlers never need
    // a synthetic Moved to re-save, and never persist the old off-screen pos.
    remember_geometry(window);
    true
}

/// Move a FloatBar that no longer intersects an active monitor onto the
/// primary monitor. Returns `true` when the window was relocated.
///
/// Prefer calling [`is_off_all_monitors`] then [`recover_onto_primary`] when
/// style comes from settings so the hot path can skip `Settings::load`.
pub(super) fn ensure_visible_on_active_monitor<R: tauri::Runtime, M: WindowGeometry<R>>(
    window: &M,
    style: &str,
) -> bool {
    if !is_off_all_monitors(window) {
        return false;
    }
    recover_onto_primary(window, style)
}

/// Initial dimensions (logical pixels) for the floating bar given an
/// orientation string. Unknown values fall back to horizontal so callers
/// don't have to pre-validate.
pub fn initial_size(orientation: &str) -> (f64, f64) {
    match orientation {
        "vertical" => (FLOATBAR_DEFAULT_WIDTH_V, FLOATBAR_DEFAULT_HEIGHT_V),
        _ => (FLOATBAR_DEFAULT_WIDTH_H, FLOATBAR_DEFAULT_HEIGHT_H),
    }
}

/// Convert a 0..=100 opacity value to a Win32 SetLayeredWindowAttributes
/// alpha byte (0..=255). Values below 30 are clamped so the bar is never
/// fully invisible — that would be a usability footgun.
#[cfg_attr(
    not(windows),
    allow(
        dead_code,
        reason = "opacity_to_alpha drives the Win32 layered-window alpha byte and is unused on non-Windows targets"
    )
)]
pub fn opacity_to_alpha(opacity: u8) -> u8 {
    let clamped = opacity.clamp(30, 100);
    ((clamped as u32) * 255 / 100) as u8
}

/// Open the floating-bar window, or focus + reapply attributes if already
/// open. Position is restored from the geometry store keyed by
/// `floatbar`; on first launch the window is centered horizontally near
/// the top of the primary monitor.
pub fn show(
    app: &tauri::AppHandle,
    opacity: u8,
    orientation: &str,
    style: &str,
    click_through: bool,
) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(FLOATBAR_LABEL) {
        apply_opacity(&window, opacity);
        apply_click_through(&window, click_through);
        let _ = ensure_visible_on_active_monitor(&window, style);
        // Re-assert after possible unminimize/relocate so focus stays off.
        apply_no_activate(&window);
        apply_always_on_top(&window);
        window.show().map_err(|e| e.to_string())?;
        apply_always_on_top(&window);
        super::topmost_guard::set_active(true);
        return Ok(());
    }

    let (w, h) = initial_size(orientation);
    let url =
        WebviewUrl::App(format!("index.html?window=floatbar&orientation={orientation}").into());

    let builder = tauri::WebviewWindowBuilder::new(app, FLOATBAR_LABEL, url)
        .title("CodexBar Float Bar")
        .inner_size(w, h)
        .decorations(false)
        .shadow(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        // Pin Dark: the floatbar is the only window without a theme pin, and
        // WebView2 resolves prefers-color-scheme per shared process profile,
        // so an unpinned (light-default) webview flips the Settings window's
        // auto theme from dark to light (issue #240). The bar's own look is
        // CSS-driven, so pinning Dark does not change how it appears.
        .theme(Some(tauri::Theme::Dark));

    // WebView2 only honors an alpha (transparent) background when the native
    // window is itself created transparent. Tauri cfg-gates this builder API
    // off on macOS unless `macos-private-api` is enabled, so keep the Windows
    // fix out of the macOS validation path.
    #[cfg(windows)]
    let builder = builder.transparent(true);

    let win = builder
        .background_color(tauri::utils::config::Color(0, 0, 0, 0))
        .visible(false)
        .build()
        .map_err(|e| e.to_string())?;

    // Restore prior geometry if we have one. Otherwise, taskbar style opens
    // near the bottom while the original floating style keeps its top-center
    // placement.
    if let Some(g) = geometry_store::load_entry(FLOATBAR_LABEL)
        .filter(|g| !is_unusable_stored_logical_position(g.x, g.y))
    {
        let _ = win.set_position(LogicalPosition::new(g.x as f64, g.y as f64));
        if let (Some(w), Some(h)) = (g.width, g.height) {
            let _ = win.set_size(LogicalSize::new(w as f64, h as f64));
        }
    } else if let Ok(Some(monitor)) = win.primary_monitor() {
        let scale = win.scale_factor().unwrap_or(1.0);
        let mon_x = monitor.position().x as f64 / scale;
        let mon_y = monitor.position().y as f64 / scale;
        let mon_w = monitor.size().width as f64 / scale;
        let mon_h = monitor.size().height as f64 / scale;
        let (x, y) = default_logical_origin(mon_x, mon_y, mon_w, mon_h, w, h, style);
        let _ = win.set_position(LogicalPosition::new(x, y));
    }

    let _ = ensure_visible_on_active_monitor(&win, style);

    apply_opacity(&win, opacity);
    apply_click_through(&win, click_through);
    apply_no_activate(&win);
    apply_always_on_top(&win);
    win.show().map_err(|e| e.to_string())?;
    apply_always_on_top(&win);
    super::topmost_guard::set_active(true);
    Ok(())
}

/// Hide / destroy the floating bar.
pub fn hide(app: &tauri::AppHandle) -> Result<(), String> {
    super::topmost_guard::set_active(false);
    if let Some(window) = app.get_webview_window(FLOATBAR_LABEL) {
        // Persist position before closing so it reopens in place.
        remember_geometry(&window);
        if let Err(error) = window.close() {
            super::topmost_guard::set_active(true);
            return Err(error.to_string());
        }
    }
    Ok(())
}

/// Capture current position into the geometry store under the floatbar key.
///
/// Accepts any Tauri window handle (`Window` from event callbacks or
/// `WebviewWindow` from `get_webview_window`), since `WindowEvent`
/// callbacks deliver a `&Window` while imperative call sites have a
/// `&WebviewWindow`.
pub fn remember_geometry<R: tauri::Runtime, M: WindowGeometry<R>>(window: &M) {
    let Ok(pos) = window.outer_position() else {
        return;
    };
    let is_minimized = window.is_minimized().unwrap_or(false);
    if !should_remember_physical_position(is_minimized, pos.x, pos.y) {
        return;
    }
    let Ok(size) = window.outer_size() else {
        return;
    };
    // Never re-poison the store with a position that is off every display
    // (failed recovery, transient shell state, etc.).
    if let Ok(monitors) = window.available_monitors()
        && !monitors.is_empty()
        && !intersects_any_monitor(pos, size, &monitors)
    {
        return;
    }
    let scale = window.scale_factor().unwrap_or(1.0);
    geometry_store::save_entry(
        FLOATBAR_LABEL,
        geometry_store::StoredGeometry {
            x: (pos.x as f64 / scale).round() as i32,
            y: (pos.y as f64 / scale).round() as i32,
            width: Some((size.width as f64 / scale).round() as u32),
            height: Some((size.height as f64 / scale).round() as u32),
        },
    );
}

/// Subset of `tauri::WebviewWindow` / `tauri::Window` used by
/// [`remember_geometry`]. Both types implement the underlying methods, but
/// they don't share a public trait — this private trait bridges them so we
/// can be called from `WindowEvent` (which delivers `&Window`) and from
/// imperative paths (which hold `&WebviewWindow`).
pub trait WindowGeometry<R: tauri::Runtime> {
    fn outer_position(&self) -> tauri::Result<tauri::PhysicalPosition<i32>>;
    fn outer_size(&self) -> tauri::Result<tauri::PhysicalSize<u32>>;
    fn scale_factor(&self) -> tauri::Result<f64>;
    fn is_minimized(&self) -> tauri::Result<bool>;
    fn primary_monitor(&self) -> tauri::Result<Option<tauri::Monitor>>;
    fn available_monitors(&self) -> tauri::Result<Vec<tauri::Monitor>>;
    fn unminimize(&self) -> tauri::Result<()>;
    fn set_physical_position(&self, position: PhysicalPosition<i32>) -> tauri::Result<()>;
}

impl<R: tauri::Runtime> WindowGeometry<R> for tauri::WebviewWindow<R> {
    fn outer_position(&self) -> tauri::Result<tauri::PhysicalPosition<i32>> {
        tauri::WebviewWindow::outer_position(self)
    }
    fn outer_size(&self) -> tauri::Result<tauri::PhysicalSize<u32>> {
        tauri::WebviewWindow::outer_size(self)
    }
    fn scale_factor(&self) -> tauri::Result<f64> {
        tauri::WebviewWindow::scale_factor(self)
    }
    fn is_minimized(&self) -> tauri::Result<bool> {
        tauri::WebviewWindow::is_minimized(self)
    }
    fn primary_monitor(&self) -> tauri::Result<Option<tauri::Monitor>> {
        tauri::WebviewWindow::primary_monitor(self)
    }
    fn available_monitors(&self) -> tauri::Result<Vec<tauri::Monitor>> {
        tauri::WebviewWindow::available_monitors(self)
    }
    fn unminimize(&self) -> tauri::Result<()> {
        tauri::WebviewWindow::unminimize(self)
    }
    fn set_physical_position(&self, position: PhysicalPosition<i32>) -> tauri::Result<()> {
        tauri::WebviewWindow::set_position(self, position)
    }
}

impl<R: tauri::Runtime> WindowGeometry<R> for tauri::Window<R> {
    fn outer_position(&self) -> tauri::Result<tauri::PhysicalPosition<i32>> {
        tauri::Window::outer_position(self)
    }
    fn outer_size(&self) -> tauri::Result<tauri::PhysicalSize<u32>> {
        tauri::Window::outer_size(self)
    }
    fn scale_factor(&self) -> tauri::Result<f64> {
        tauri::Window::scale_factor(self)
    }
    fn is_minimized(&self) -> tauri::Result<bool> {
        tauri::Window::is_minimized(self)
    }
    fn primary_monitor(&self) -> tauri::Result<Option<tauri::Monitor>> {
        tauri::Window::primary_monitor(self)
    }
    fn available_monitors(&self) -> tauri::Result<Vec<tauri::Monitor>> {
        tauri::Window::available_monitors(self)
    }
    fn unminimize(&self) -> tauri::Result<()> {
        tauri::Window::unminimize(self)
    }
    fn set_physical_position(&self, position: PhysicalPosition<i32>) -> tauri::Result<()> {
        tauri::Window::set_position(self, position)
    }
}

/// Resize the floatbar to the given logical dimensions and re-assert the
/// native interaction invariants in the same step.
///
/// A resize goes through `SetWindowPos`/frame changes, which can drop the
/// extended window styles, so the no-activate and click-through flags must be
/// re-applied afterwards. Keeping both halves here gives callers (including the
/// webview) a single canonical "the bar changed size" entry point instead of
/// pairing a JS `setSize` with a separate native repair command.
pub fn resize(
    window: &tauri::WebviewWindow,
    width: f64,
    height: f64,
    click_through: bool,
) -> Result<(), String> {
    let width = width.ceil().clamp(1.0, u32::MAX as f64) as u32;
    let height = height.ceil().clamp(1.0, u32::MAX as f64) as u32;

    window
        .set_size(PhysicalSize::new(width, height))
        .map_err(|e| e.to_string())?;
    apply_no_activate(window);
    apply_click_through(window, click_through);
    apply_always_on_top(window);
    Ok(())
}

/// Re-assert native topmost ordering without activating the window.
///
/// Tauri's `always_on_top(true)` sets the initial intent, but on Windows
/// resize/style changes and competing topmost windows can still disturb z-order.
/// This Win32 pass keeps the floatbar visually above normal app windows while
/// preserving the current foreground app's input focus.
pub fn apply_always_on_top(window: &tauri::WebviewWindow) {
    let _ = window;
    #[cfg(windows)]
    {
        use raw_window_handle::HasWindowHandle;
        let Ok(handle) = window.window_handle() else {
            return;
        };
        let raw_window_handle::RawWindowHandle::Win32(h) = handle.as_raw() else {
            return;
        };
        unsafe {
            const HWND_TOPMOST: isize = -1;
            const SWP_NOSIZE: u32 = 0x0001;
            const SWP_NOMOVE: u32 = 0x0002;
            const SWP_NOACTIVATE: u32 = 0x0010;
            let flags = SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE;
            if SetWindowPos(h.hwnd.get(), HWND_TOPMOST, 0, 0, 0, 0, flags) == 0 {
                tracing::warn!(
                    error = %std::io::Error::last_os_error(),
                    "failed to restore floatbar topmost z-order"
                );
            }
        }
    }
}

/// Apply the current opacity setting to an existing floatbar window via
/// `SetLayeredWindowAttributes`. No-op on non-Windows platforms.
pub fn apply_opacity(window: &tauri::WebviewWindow, opacity: u8) {
    let _ = (window, opacity);
    #[cfg(windows)]
    {
        use raw_window_handle::HasWindowHandle;
        let alpha = opacity_to_alpha(opacity);
        let Ok(handle) = window.window_handle() else {
            return;
        };
        let raw_window_handle::RawWindowHandle::Win32(h) = handle.as_raw() else {
            return;
        };
        unsafe {
            // Ensure WS_EX_LAYERED is set so SetLayeredWindowAttributes works.
            const WS_EX_LAYERED: isize = 0x00080000;
            let ex = GetWindowLongPtrW(h.hwnd.get(), GWL_EXSTYLE);
            if ex & WS_EX_LAYERED == 0 {
                set_extended_style(h.hwnd.get(), ex | WS_EX_LAYERED);
            }
            const LWA_ALPHA: u32 = 0x00000002;
            SetLayeredWindowAttributes(h.hwnd.get(), 0, alpha, LWA_ALPHA);
        }
    }
}

/// Keep the floatbar from activating when it is shown or clicked. This makes
/// it behave like a desktop widget that visually sits above the taskbar without
/// stealing focus from the active app.
pub fn apply_no_activate(window: &tauri::WebviewWindow) {
    let _ = window;
    #[cfg(windows)]
    {
        use raw_window_handle::HasWindowHandle;
        let Ok(handle) = window.window_handle() else {
            return;
        };
        let raw_window_handle::RawWindowHandle::Win32(h) = handle.as_raw() else {
            return;
        };
        unsafe {
            const WS_EX_NOACTIVATE: isize = 0x08000000;
            let ex = GetWindowLongPtrW(h.hwnd.get(), GWL_EXSTYLE);
            if ex & WS_EX_NOACTIVATE == 0 {
                set_extended_style(h.hwnd.get(), ex | WS_EX_NOACTIVATE);
            }
        }
    }
}

/// Toggle click-through (`WS_EX_TRANSPARENT`). When enabled, mouse events
/// pass through to the window beneath — true overlay mode.
pub fn apply_click_through(window: &tauri::WebviewWindow, click_through: bool) {
    let _ = (window, click_through);
    #[cfg(windows)]
    {
        use raw_window_handle::HasWindowHandle;
        let Ok(handle) = window.window_handle() else {
            return;
        };
        let raw_window_handle::RawWindowHandle::Win32(h) = handle.as_raw() else {
            return;
        };
        unsafe {
            const WS_EX_LAYERED: isize = 0x00080000;
            const WS_EX_TRANSPARENT: isize = 0x00000020;
            let ex = GetWindowLongPtrW(h.hwnd.get(), GWL_EXSTYLE);
            let mut new_ex = ex | WS_EX_LAYERED;
            if click_through {
                new_ex |= WS_EX_TRANSPARENT;
            } else {
                new_ex &= !WS_EX_TRANSPARENT;
            }
            if new_ex != ex {
                set_extended_style(h.hwnd.get(), new_ex);
            }
        }
    }
}

#[cfg(windows)]
const GWL_EXSTYLE: i32 = -20;

#[cfg(windows)]
unsafe fn set_extended_style(hwnd: isize, ex_style: isize) {
    unsafe {
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style);
        const SWP_NOSIZE: u32 = 0x0001;
        const SWP_NOMOVE: u32 = 0x0002;
        const SWP_NOZORDER: u32 = 0x0004;
        const SWP_NOACTIVATE: u32 = 0x0010;
        const SWP_FRAMECHANGED: u32 = 0x0020;
        let flags = SWP_NOSIZE | SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED;
        SetWindowPos(hwnd, 0, 0, 0, 0, 0, flags);
    }
}

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
    fn SetWindowLongPtrW(hwnd: isize, index: i32, new: isize) -> isize;
    fn SetLayeredWindowAttributes(hwnd: isize, color_key: u32, alpha: u8, flags: u32) -> i32;
    fn SetWindowPos(
        hwnd: isize,
        hwnd_insert_after: isize,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opacity_to_alpha_clamps_low_values() {
        assert_eq!(opacity_to_alpha(0), opacity_to_alpha(30));
        assert_eq!(opacity_to_alpha(10), opacity_to_alpha(30));
    }

    #[test]
    fn opacity_to_alpha_full_is_255() {
        assert_eq!(opacity_to_alpha(100), 255);
    }

    #[test]
    fn opacity_to_alpha_is_monotonic() {
        let a = opacity_to_alpha(30);
        let b = opacity_to_alpha(60);
        let c = opacity_to_alpha(100);
        assert!(a < b);
        assert!(b < c);
    }

    #[test]
    fn opacity_to_alpha_midpoint() {
        // 50% should be roughly half of 255.
        let alpha = opacity_to_alpha(50);
        assert!((125..=130).contains(&alpha), "got {alpha}");
    }

    #[test]
    fn initial_size_picks_orientation() {
        assert_eq!(
            initial_size("horizontal"),
            (FLOATBAR_DEFAULT_WIDTH_H, FLOATBAR_DEFAULT_HEIGHT_H)
        );
        assert_eq!(
            initial_size("vertical"),
            (FLOATBAR_DEFAULT_WIDTH_V, FLOATBAR_DEFAULT_HEIGHT_V)
        );
        // Unknown values fall through to horizontal so a corrupted setting
        // can't yield an unreadable strip.
        assert_eq!(
            initial_size("diagonal"),
            (FLOATBAR_DEFAULT_WIDTH_H, FLOATBAR_DEFAULT_HEIGHT_H)
        );
    }

    #[test]
    fn windows_minimized_position_is_not_restored() {
        assert!(is_windows_minimized_position(-32_000, -32_000));
    }

    #[test]
    fn legitimate_negative_monitor_positions_are_preserved() {
        assert!(!is_windows_minimized_position(-3_840, 0));
        assert!(!is_windows_minimized_position(-8_000, -8_000));
        assert!(!is_windows_minimized_position(-16_000, -16_000));
    }

    #[test]
    fn minimized_or_parked_physical_positions_are_not_remembered() {
        assert!(!should_remember_physical_position(true, 100, 100));
        assert!(!should_remember_physical_position(false, -32_000, -32_000));
        assert!(should_remember_physical_position(false, -3_840, 100));
    }

    #[test]
    fn physical_multi_monitor_origin_is_remembered() {
        assert!(should_remember_physical_position(false, -8_000, -8_000));
    }

    #[test]
    fn window_must_intersect_an_active_monitor_to_be_visible() {
        let window_size = PhysicalSize::new(211, 40);
        let monitor_size = PhysicalSize::new(1_920, 1_032);

        assert!(physical_rects_intersect(
            PhysicalPosition::new(780, 8),
            window_size,
            PhysicalPosition::new(0, 0),
            monitor_size,
        ));
        assert!(!physical_rects_intersect(
            PhysicalPosition::new(-32_000, -32_000),
            window_size,
            PhysicalPosition::new(0, 0),
            monitor_size,
        ));
        assert!(physical_rects_intersect(
            PhysicalPosition::new(-3_840, 8),
            window_size,
            PhysicalPosition::new(-3_840, 0),
            monitor_size,
        ));
    }

    #[test]
    fn taskbar_strip_outside_work_area_still_counts_as_on_monitor() {
        // Full monitor 1920x1080; work area is 1920x1032 (48px taskbar strip).
        // A taskbar-style bar sitting fully in the strip is on-screen for
        // visibility even though it has zero work-area intersection.
        let window_pos = PhysicalPosition::new(800, 1_040);
        let window_size = PhysicalSize::new(211, 40);
        let monitor_pos = PhysicalPosition::new(0, 0);
        let monitor_size = PhysicalSize::new(1_920, 1_080);
        let work_area_pos = PhysicalPosition::new(0, 0);
        let work_area_size = PhysicalSize::new(1_920, 1_032);

        assert!(physical_rects_intersect(
            window_pos,
            window_size,
            monitor_pos,
            monitor_size,
        ));
        assert!(!physical_rects_intersect(
            window_pos,
            window_size,
            work_area_pos,
            work_area_size,
        ));
    }

    #[test]
    fn hidpi_parked_logical_geometry_is_rejected() {
        assert!(is_unusable_stored_logical_position(-32_000, -32_000));
        // -32000 physical at 1.25x / 1.5x / 2.0x scale factors.
        assert!(is_unusable_stored_logical_position(-25_600, -25_600));
        assert!(is_unusable_stored_logical_position(-21_333, -21_333));
        assert!(is_unusable_stored_logical_position(-16_000, -16_000));
        // Legitimate multi-monitor logical origins must still restore.
        assert!(!is_unusable_stored_logical_position(-3_840, 0));
        assert!(!is_unusable_stored_logical_position(-8_000, -8_000));
    }

    #[test]
    fn disconnected_monitor_position_falls_back_to_primary_work_area() {
        let work_area_position = PhysicalPosition::new(0, 0);
        let work_area_size = PhysicalSize::new(1_920, 1_032);
        let window_size = PhysicalSize::new(211, 40);

        assert_eq!(
            fallback_physical_position(
                work_area_position,
                work_area_size,
                window_size,
                1.0,
                "floating",
            ),
            PhysicalPosition::new(855, 8),
        );
        assert_eq!(
            fallback_physical_position(
                work_area_position,
                work_area_size,
                window_size,
                1.0,
                "taskbar",
            ),
            PhysicalPosition::new(855, 984),
        );
    }

    #[test]
    fn default_logical_origin_matches_first_open_policy() {
        assert_eq!(
            default_logical_origin(0.0, 0.0, 1_920.0, 1_080.0, 211.0, 40.0, "floating"),
            (854.5, 8.0),
        );
        assert_eq!(
            default_logical_origin(0.0, 0.0, 1_920.0, 1_080.0, 211.0, 40.0, "taskbar"),
            (854.5, 1_032.0),
        );
    }
}
