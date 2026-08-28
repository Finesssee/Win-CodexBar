//! Windows DWM helpers for eliminating the non-client caption area.
//!
//! Even with `decorations(false)`, Windows keeps a thin caption strip
//! that DWM renders. We install a window subclass that intercepts
//! WM_NCCALCSIZE to zero the non-client area and WM_NCPAINT/WM_NCACTIVATE
//! to suppress DWM painting, making the window truly borderless.

#[cfg(windows)]
use std::ffi::c_void;

#[cfg(windows)]
#[link(name = "dwmapi")]
// SAFETY: FFI declarations for the DWM API; the functions are only ever
// called with live window handles and caller-owned buffers (see call sites).
unsafe extern "system" {
    fn DwmSetWindowAttribute(hwnd: isize, attr: u32, data: *const c_void, size: u32) -> i32;
    fn DwmExtendFrameIntoClientArea(hwnd: isize, margins: *const Margins) -> i32;
}

#[cfg(windows)]
#[repr(C)]
struct Margins {
    left: i32,
    right: i32,
    top: i32,
    bottom: i32,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct WinPoint {
    x: i32,
    y: i32,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct WinRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

/// Win32 `MINMAXINFO`. `lparam` of `WM_GETMINMAXINFO` points at one of these.
#[cfg(windows)]
#[repr(C)]
struct MinMaxInfo {
    reserved: WinPoint,
    max_size: WinPoint,
    max_position: WinPoint,
    min_track_size: WinPoint,
    max_track_size: WinPoint,
}

/// Win32 `MONITORINFO` (40 bytes). `cb_size` must be set before the call.
#[cfg(windows)]
#[repr(C)]
struct MonitorInfo {
    cb_size: u32,
    rc_monitor: WinRect,
    rc_work: WinRect,
    dw_flags: u32,
}

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetAncestor(hwnd: isize, flags: u32) -> isize;
    fn SetWindowLongPtrW(hwnd: isize, index: i32, new: isize) -> isize;
    fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
    fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, w: i32, h: i32, flags: u32) -> i32;
    fn DefSubclassProc(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn MonitorFromWindow(hwnd: isize, flags: u32) -> isize;
    fn GetMonitorInfoW(hmonitor: isize, info: *mut MonitorInfo) -> i32;
}

#[cfg(windows)]
#[link(name = "comctl32")]
unsafe extern "system" {
    fn SetWindowSubclass(
        hwnd: isize,
        pfn: unsafe extern "system" fn(isize, u32, usize, isize, usize, usize) -> isize,
        id: usize,
        data: usize,
    ) -> i32;
}

#[cfg(windows)]
#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateSolidBrush(color: u32) -> isize;
}

#[cfg(windows)]
static DARK_BRUSH: std::sync::OnceLock<isize> = std::sync::OnceLock::new();

#[cfg(windows)]
const WM_NCCALCSIZE: u32 = 0x0083;
#[cfg(windows)]
const WM_NCPAINT: u32 = 0x0085;
#[cfg(windows)]
const WM_NCACTIVATE: u32 = 0x0086;
#[cfg(windows)]
const WM_GETMINMAXINFO: u32 = 0x0024;
#[cfg(windows)]
const BORDERLESS_SUBCLASS_ID: usize = 0xC0DE_BA12;

#[cfg(windows)]
unsafe extern "system" fn borderless_subclass_proc(
    hwnd: isize,
    msg: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    _data: usize,
) -> isize {
    match msg {
        WM_NCCALCSIZE => {
            if wparam != 0 {
                // Returning 0 when wparam is TRUE tells Windows the
                // client area == the window area (no non-client area).
                return 0;
            }
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }
        WM_NCPAINT => {
            // Suppress DWM non-client painting entirely.
            0
        }
        WM_NCACTIVATE => {
            // Return TRUE to accept activation but skip DWM painting.
            1
        }
        WM_GETMINMAXINFO => {
            // A borderless window whose non-client area is zeroed maximizes to
            // cover the entire monitor, including the taskbar. Constrain the
            // maximized position/size to the monitor work area instead.
            const MONITOR_DEFAULTTONEAREST: u32 = 2;
            unsafe {
                let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                if hmon != 0 && lparam != 0 {
                    let mut mi = MonitorInfo {
                        cb_size: std::mem::size_of::<MonitorInfo>() as u32,
                        rc_monitor: WinRect::default(),
                        rc_work: WinRect::default(),
                        dw_flags: 0,
                    };
                    if GetMonitorInfoW(hmon, &mut mi) != 0 {
                        let mmi = lparam as *mut MinMaxInfo;
                        (*mmi).max_position = WinPoint {
                            x: mi.rc_work.left - mi.rc_monitor.left,
                            y: mi.rc_work.top - mi.rc_monitor.top,
                        };
                        (*mmi).max_size = WinPoint {
                            x: mi.rc_work.right - mi.rc_work.left,
                            y: mi.rc_work.bottom - mi.rc_work.top,
                        };
                        (*mmi).max_track_size = (*mmi).max_size;
                    }
                }
            }
            0
        }
        _ => unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) },
    }
}

/// Eliminate the DWM caption bar by subclassing the window to zero the
/// non-client area.  Safe to call on multiple windows — each gets its
/// own subclass via `SetWindowSubclass`.
///
/// When `resizable` is true, `WS_THICKFRAME` is preserved so the native
/// resize affordance still works.
#[cfg(windows)]
pub fn force_dark_caption(win: &tauri::WebviewWindow) {
    force_dark_caption_inner(win, false);
}

/// Same as [`force_dark_caption`] but keeps the resize frame.
#[cfg(windows)]
pub fn force_dark_caption_resizable(win: &tauri::WebviewWindow) {
    force_dark_caption_inner(win, true);
}

#[cfg(windows)]
fn force_dark_caption_inner(win: &tauri::WebviewWindow, keep_resize: bool) {
    use raw_window_handle::HasWindowHandle;

    let Ok(handle) = win.window_handle() else {
        tracing::warn!("dwm: couldn't get window handle");
        return;
    };
    let raw_window_handle::RawWindowHandle::Win32(h) = handle.as_raw() else {
        tracing::warn!("dwm: not a Win32 handle");
        return;
    };

    const GA_ROOT: u32 = 2;
    let inner = h.hwnd.get();
    let hwnd = unsafe { GetAncestor(inner, GA_ROOT) };
    let hwnd = if hwnd != 0 { hwnd } else { inner };
    tracing::info!("dwm: inner={inner:#x} root={hwnd:#x}");

    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    const DWMWA_CAPTION_COLOR: u32 = 35;
    let dark_mode: u32 = 1;
    let caption_color: u32 = 0x001C1C1E;

    unsafe {
        let r1 = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &raw const dark_mode as *const c_void,
            4,
        );
        let r2 = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            &raw const caption_color as *const c_void,
            4,
        );
        tracing::info!("dwm: dark_mode={r1:#x} caption_color={r2:#x}");

        // Extend DWM frame fully into client area
        let margins = Margins {
            left: -1,
            right: -1,
            top: -1,
            bottom: -1,
        };
        let r3 = DwmExtendFrameIntoClientArea(hwnd, &margins);
        tracing::info!("dwm: extend_frame={r3:#x}");

        // Install subclass proc (safe for multiple windows)
        let ok = SetWindowSubclass(hwnd, borderless_subclass_proc, BORDERLESS_SUBCLASS_ID, 0);
        tracing::info!("dwm: subclass installed={ok}");

        // Set background brush to dark (reuse a single GDI brush)
        const GCL_HBRBACKGROUND: i32 = -10;
        let brush = *DARK_BRUSH.get_or_init(|| CreateSolidBrush(0x001C1C1E));
        if brush != 0 {
            SetWindowLongPtrW(hwnd, GCL_HBRBACKGROUND, brush);
        }

        // Remove WS_CAPTION; only strip WS_THICKFRAME for non-resizable windows
        const GWL_STYLE: i32 = -16;
        const WS_CAPTION: isize = 0x00C00000;
        const WS_THICKFRAME: isize = 0x00040000;
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let new_style = if keep_resize {
            style & !WS_CAPTION
        } else {
            style & !WS_CAPTION & !WS_THICKFRAME
        };
        if new_style != style {
            SetWindowLongPtrW(hwnd, GWL_STYLE, new_style);
            if keep_resize {
                tracing::info!("dwm: stripped WS_CAPTION (kept WS_THICKFRAME for resize)");
            } else {
                tracing::info!("dwm: stripped WS_CAPTION/WS_THICKFRAME");
            }
        }

        // Force frame recalculation
        const SWP_FRAMECHANGED: u32 = 0x0020;
        const SWP_NOMOVE: u32 = 0x0002;
        const SWP_NOSIZE: u32 = 0x0001;
        const SWP_NOZORDER: u32 = 0x0004;
        SetWindowPos(
            hwnd,
            0,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
        );
    }
}

#[cfg(not(windows))]
pub fn force_dark_caption(_win: &tauri::WebviewWindow) {}

#[cfg(not(windows))]
pub fn force_dark_caption_resizable(_win: &tauri::WebviewWindow) {}
