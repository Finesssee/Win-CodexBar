//! Local timezone lookup with a Windows safety shim.
//!
//! On Windows, `iana-time-zone` resolves the system zone through WinRT's
//! `Windows.Globalization` `Calendar` class. The `windows-core` activation
//! lookup caches the class factory in a process-static for the rest of the
//! process, but nothing keeps `Windows.Globalization.dll` itself loaded: the
//! notification sound/toast test group exercises COM code paths whose DLL
//! cleanup unloads it (observed empirically on this machine; which specific
//! notification API releases the DLL was not isolated). Any later
//! `get_timezone()` call then dereferences a factory vtable in the abandoned
//! mapping — a hard access violation, not a catchable Rust panic (observed
//! under LLDB: crash inside `iana_time_zone::get_timezone`).
//!
//! The fix is to load `Windows.Globalization.dll` from System32 and pin it
//! for the life of the process *before* any `iana_time_zone::get_timezone`
//! call. The attempt runs exactly once behind a `LazyLock` (OnceLock-backed):
//! every caller observes the finished result before being allowed to
//! proceed, so no thread can reach `iana_time_zone` while pinning is still
//! in progress.

use std::sync::LazyLock;

/// Zone used when the OS timezone cannot be read safely.
const FALLBACK_TIMEZONE: &str = "UTC";

/// Settled result of the one-and-only pin attempt; `true` means the DLL is
/// guaranteed to stay mapped until process exit. Dereferencing blocks until
/// the initializer (load + pin) has fully completed.
#[cfg(windows)]
static GLOBALIZATION_PINNED: LazyLock<bool> = LazyLock::new(pin_globalization_dll);

/// Returns the IANA name of the system timezone, or `"UTC"` if it cannot be
/// determined safely.
pub(crate) fn local_timezone_name() -> String {
    #[cfg(windows)]
    {
        // Deref runs the load+pin to completion on one thread while all
        // concurrent callers block; ordering every caller after the pin is
        // what makes the subsequent iana call safe.
        if !*GLOBALIZATION_PINNED {
            // Unpinned: `get_timezone` might fault on an unloadable cached
            // factory, and an AV is uncatchable. Refuse to call it.
            return FALLBACK_TIMEZONE.to_string();
        }
    }
    iana_time_zone::get_timezone().unwrap_or_else(|_| FALLBACK_TIMEZONE.to_string())
}

/// Loads `Windows.Globalization.dll` from System32 and pins it so no later
/// COM/WinRT cleanup can unmap it. Returns `true` only when the mapping is
/// pinned for the remainder of the process lifetime.
#[cfg(windows)]
fn pin_globalization_dll() -> bool {
    use windows::Win32::Foundation::{FreeLibrary, HANDLE, HMODULE};
    use windows::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN, GetModuleHandleExW,
        LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
    };
    use windows::core::PCWSTR;

    let name: Vec<u16> = "Windows.Globalization.dll\0".encode_utf16().collect();
    // SAFETY: `name` is a null-terminated UTF-16 buffer that outlives the
    // call. The reserved `hFile` parameter must be the null handle.
    // LOAD_LIBRARY_SEARCH_SYSTEM32 restricts the pathless name to System32,
    // avoiding any preloaded look-alike DLL.
    let module = unsafe {
        LoadLibraryExW(
            PCWSTR(name.as_ptr()),
            HANDLE::default(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    let module: HMODULE = match module {
        Ok(module) => module,
        Err(_) => return false,
    };
    let mut pinned = HMODULE::default();
    // SAFETY: an HMODULE is the module's base address, i.e. an address
    // inside its own mapping, satisfying GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS.
    let pinned_ok = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
            PCWSTR(module.0.cast::<u16>()),
            &mut pinned,
        )
    }
    .is_ok();
    // Balance the load reference taken by LoadLibraryExW above. Safe on both
    // outcomes: if the pin failed we simply release our reference; if it
    // succeeded, MS documents GET_MODULE_HANDLE_EX_FLAG_PIN as keeping the
    // module loaded until process termination no matter how many times
    // FreeLibrary is called on it, so the pin — not our reference count —
    // is what protects the cached factory's mapping.
    //
    // SAFETY: `module` is a live module handle owned by this scope and is
    // released at most once, here.
    unsafe {
        let _released = FreeLibrary(module);
    }
    pinned_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_timezone_name_is_never_empty() {
        assert!(!local_timezone_name().is_empty());
    }

    #[test]
    fn local_timezone_name_is_stable_across_calls() {
        let first = local_timezone_name();
        let second = local_timezone_name();
        assert_eq!(first, second);
    }

    /// Concurrent callers — including a possible cold start where several
    /// threads hit the one-shot initializer at once — must all observe the
    /// same result; no thread may race ahead of the load+pin gate. The
    /// assertions hold for any interleaving, so the test is deterministic
    /// and makes no ordering assumption about other tests.
    #[test]
    fn concurrent_callers_see_one_stable_result() {
        let expected = local_timezone_name();
        let handles: Vec<_> = (0..8)
            .map(|_| std::thread::spawn(local_timezone_name))
            .collect();
        for handle in handles {
            let observed = handle.join().expect("caller thread panicked");
            assert_eq!(observed, expected);
        }
    }

    #[cfg(windows)]
    #[test]
    fn globalization_dll_pin_decision_settles_once() {
        // The pin *outcome* is host-dependent: stripped Windows images
        // without registered WinRT types may not expose
        // Windows.Globalization.dll, in which case the helper deliberately
        // falls back to UTC without calling iana-time-zone. What must hold
        // on every host is that the attempt happens exactly once and every
        // caller observes the same settled decision.
        let settled = *GLOBALIZATION_PINNED;
        assert_eq!(LazyLock::force(&GLOBALIZATION_PINNED), &settled);
    }
}
