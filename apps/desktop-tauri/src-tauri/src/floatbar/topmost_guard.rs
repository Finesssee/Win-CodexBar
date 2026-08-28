//! Keep a taskbar-overlapping floatbar above the Windows taskbar.
//!
//! The taskbar is itself topmost. Activating it can reorder the topmost band
//! without clearing `WS_EX_TOPMOST` on the floatbar. While the floatbar is
//! shown we run a short visibility-scoped timer and reassert topmost only when
//! the bar is visible and overlaps a taskbar. Move/resize always reasserts in
//! `floatbar::handle_window_event` (not overlap-gated).

#[cfg(any(windows, test))]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[cfg(any(windows, test))]
fn rects_overlap(a: Rect, b: Rect) -> bool {
    a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top
}

#[cfg(windows)]
mod platform {
    use super::{Rect, rects_overlap};
    use raw_window_handle::HasWindowHandle;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tauri::Manager;

    use super::super::window::{self, FLOATBAR_LABEL};

    /// Interval while the floatbar is active. Matches the ~150ms recovery
    /// budget validated for the original hook-based fix.
    const POLL_INTERVAL: Duration = Duration::from_millis(120);

    static APP_HANDLE: OnceLock<tauri::AppHandle> = OnceLock::new();
    static FLOATBAR_ACTIVE: AtomicBool = AtomicBool::new(false);
    static LOOP_RUNNING: AtomicBool = AtomicBool::new(false);

    static PRIMARY_CLASS: OnceLock<Vec<u16>> = OnceLock::new();
    static SECONDARY_CLASS: OnceLock<Vec<u16>> = OnceLock::new();

    pub fn install(app: &tauri::AppHandle) {
        // One-time install; a redundant second install is intentionally
        // ignored (the first handle wins).
        let _install = APP_HANDLE.set(app.clone());
    }

    pub fn set_active(active: bool) {
        FLOATBAR_ACTIVE.store(active, Ordering::Release);
        if active {
            ensure_loop();
        }
    }

    fn ensure_loop() {
        if LOOP_RUNNING.swap(true, Ordering::AcqRel) {
            return;
        }
        let Some(app) = APP_HANDLE.get().cloned() else {
            LOOP_RUNNING.store(false, Ordering::Release);
            return;
        };

        tauri::async_runtime::spawn(async move {
            while FLOATBAR_ACTIVE.load(Ordering::Acquire) {
                tokio::time::sleep(POLL_INTERVAL).await;
                if !FLOATBAR_ACTIVE.load(Ordering::Acquire) {
                    break;
                }
                let dispatcher = app.clone();
                let app_for_work = app.clone();
                // Best-effort main-thread dispatch; on failure the poll
                // loop re-evaluates on the next tick, so the result is
                // discarded.
                let _dispatch = dispatcher.run_on_main_thread(move || {
                    reassert_if_needed(&app_for_work);
                });
            }
            LOOP_RUNNING.store(false, Ordering::Release);
            // If set_active(true) raced with loop exit, start again.
            if FLOATBAR_ACTIVE.load(Ordering::Acquire) {
                ensure_loop();
            }
        });
    }

    fn reassert_if_needed(app: &tauri::AppHandle) {
        let Some(floatbar) = app.get_webview_window(FLOATBAR_LABEL) else {
            // Webview gone without hide — stop the guard.
            FLOATBAR_ACTIVE.store(false, Ordering::Release);
            return;
        };
        if !floatbar.is_visible().unwrap_or(false) {
            return;
        }
        if overlaps_taskbar(&floatbar) {
            window::apply_always_on_top(&floatbar);
        }
    }

    fn overlaps_taskbar(window: &tauri::WebviewWindow) -> bool {
        let Ok(handle) = window.window_handle() else {
            return false;
        };
        let raw_window_handle::RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return false;
        };

        let mut floatbar_rect = Rect::default();
        // SAFETY: `handle.hwnd.get()` is a valid HWND owned by the live
        // floatbar window, and `floatbar_rect` is a `Rect` out-param that
        // outlives the call. GetWindowRect only writes through the pointer.
        if unsafe { GetWindowRect(handle.hwnd.get(), &mut floatbar_rect) } == 0 {
            return false;
        }
        taskbar_rects()
            .into_iter()
            .any(|taskbar_rect| rects_overlap(floatbar_rect, taskbar_rect))
    }

    fn taskbar_rects() -> Vec<Rect> {
        let primary_class = PRIMARY_CLASS.get_or_init(|| wide("Shell_TrayWnd"));
        let secondary_class = SECONDARY_CLASS.get_or_init(|| wide("Shell_SecondaryTrayWnd"));
        let mut rects = Vec::new();

        // SAFETY: `primary_class` is a NUL-terminated UTF-16 buffer from
        // `wide()` that outlives the call; a null window-name pointer is
        // valid. The returned HWND is treated as an opaque handle (0 = not
        // found), never dereferenced unsafely.
        let primary = unsafe { FindWindowW(primary_class.as_ptr(), std::ptr::null()) };
        push_window_rect(primary, &mut rects);

        let mut previous = 0;
        loop {
            // SAFETY: 0 and `previous` (a prior FindWindowExW handle or 0)
            // are valid HWND arguments; `secondary_class` is a
            // NUL-terminated UTF-16 buffer outliving the call, and a null
            // window name is valid. The handle is opaque (0 = none).
            let taskbar =
                unsafe { FindWindowExW(0, previous, secondary_class.as_ptr(), std::ptr::null()) };
            if taskbar == 0 {
                break;
            }
            push_window_rect(taskbar, &mut rects);
            previous = taskbar;
        }

        rects
    }
    fn push_window_rect(hwnd: isize, rects: &mut Vec<Rect>) {
        // SAFETY: `hwnd` is an opaque HWND (possibly 0); IsWindowVisible
        // safely returns 0 for invalid handles without dereferencing.
        if hwnd == 0 || unsafe { IsWindowVisible(hwnd) } == 0 {
            return;
        }
        let mut rect = Rect::default();
        // SAFETY: `hwnd` is a nonzero, possibly-stale handle; GetWindowRect
        // only writes through the `rect` out-param that outlives the call.
        if unsafe { GetWindowRect(hwnd, &mut rect) } != 0 {
            rects.push(rect);
        }
    }
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn FindWindowW(class_name: *const u16, window_name: *const u16) -> isize;
        fn FindWindowExW(
            parent: isize,
            child_after: isize,
            class_name: *const u16,
            window_name: *const u16,
        ) -> isize;
        fn GetWindowRect(hwnd: isize, rect: *mut Rect) -> i32;
        fn IsWindowVisible(hwnd: isize) -> i32;
    }
}

#[cfg(windows)]
pub fn install(app: &tauri::AppHandle) {
    platform::install(app);
}

#[cfg(windows)]
pub fn set_active(active: bool) {
    platform::set_active(active);
}

#[cfg(not(windows))]
pub fn install(_app: &tauri::AppHandle) {}

#[cfg(not(windows))]
pub fn set_active(_active: bool) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_requires_positive_area() {
        let taskbar = Rect {
            left: 0,
            top: 1040,
            right: 1920,
            bottom: 1080,
        };
        assert!(rects_overlap(
            Rect {
                left: 800,
                top: 1044,
                right: 1120,
                bottom: 1076,
            },
            taskbar
        ));
        assert!(!rects_overlap(
            Rect {
                left: 800,
                top: 1000,
                right: 1120,
                bottom: 1040,
            },
            taskbar
        ));
    }
}
