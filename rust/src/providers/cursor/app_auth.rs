//! Cursor desktop app auth session (upstream 0.50.0 #2398).
//!
//! Reads Cursor's own read-only local session database
//! (`%APPDATA%\Cursor\User\globalStorage\state.vscdb`) and rebuilds the
//! `WorkosCursorSessionToken` cookie from the stored access token, so
//! Automatic mode prefers the signed-in app over browser cookies. The
//! database is only ever opened read-only; an idle WAL database whose
//! sidecars vanished is retried in SQLite immutable mode (never while a WAL
//! exists — that would ignore live uncheckpointed Cursor state).

use rusqlite::OpenFlags;

/// Default `state.vscdb` location. Windows: `%APPDATA%\Cursor\…`; the
/// upstream macOS/Linux layouts differ per OS.
pub fn app_auth_db_path() -> Option<std::path::PathBuf> {
    let base = dirs::config_dir()?;
    Some(
        base.join("Cursor")
            .join("User")
            .join("globalStorage")
            .join("state.vscdb"),
    )
}

/// Read the stored `cursorAuth/accessToken` from Cursor's app database.
pub fn load_app_auth_access_token() -> Option<String> {
    let db_path = app_auth_db_path()?;
    if !db_path.exists() {
        return None;
    }
    match read_item_table_value(&db_path, "cursorAuth/accessToken", false) {
        Ok(value) => value,
        Err(err) => {
            // Immutable retry only when both WAL sidecars are gone — an idle
            // WAL database can retain WAL mode in its header after the
            // sidecars disappear, and immutable mode reads the main file
            // without recreating them.
            let wal_missing = !wal_sidecar(&db_path).exists() && !shm_sidecar(&db_path).exists();
            if !wal_missing {
                tracing::debug!("Cursor app auth read failed: {err}");
                return None;
            }
            read_item_table_value(&db_path, "cursorAuth/accessToken", true)
                .ok()
                .flatten()
                .or_else(|| {
                    tracing::debug!("Cursor app auth immutable read failed: {err}");
                    None
                })
        }
    }
}

fn wal_sidecar(db_path: &std::path::Path) -> std::path::PathBuf {
    let mut name = db_path.as_os_str().to_os_string();
    name.push("-wal");
    std::path::PathBuf::from(name)
}

fn shm_sidecar(db_path: &std::path::Path) -> std::path::PathBuf {
    let mut name = db_path.as_os_str().to_os_string();
    name.push("-shm");
    std::path::PathBuf::from(name)
}

/// Rebuild the `WorkosCursorSessionToken` cookie header from the app's
/// access token (`{userID}::{token}`, URL-encoded separator).
pub fn app_session_cookie_header(access_token: &str) -> Option<String> {
    let token = access_token.trim();
    if token.is_empty() {
        return None;
    }
    let user_id = crate::codex_accounts::api::jwt_payload(token)
        .and_then(|payload| {
            payload
                .get("sub")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    Some(format!("WorkosCursorSessionToken={user_id}%3A%3A{token}"))
}

/// Read one `ItemTable` value from the app database.
fn read_item_table_value(
    db_path: &std::path::Path,
    key: &str,
    immutable: bool,
) -> rusqlite::Result<Option<String>> {
    let mut flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let opened_path = if immutable {
        // URI + immutable=1: never recreates WAL sidecars on a cold database.
        flags |= OpenFlags::SQLITE_OPEN_URI;
        format!(
            "file:{}?immutable=1",
            db_path
                .to_str()
                .map(|p| p.replace('\\', "/"))
                .unwrap_or_default()
        )
    } else {
        db_path.to_string_lossy().to_string()
    };
    let conn = rusqlite::Connection::open_with_flags(&opened_path, flags)?;
    conn.busy_timeout(std::time::Duration::from_millis(250))?;
    let mut stmt = conn.prepare("SELECT value FROM ItemTable WHERE key = ? LIMIT 1;")?;
    let mut rows = stmt.query([key])?;
    match rows.next()? {
        Some(row) => Ok(row.get_ref(0).ok().and_then(decode_sqlite_string)),
        None => Ok(None),
    }
}

/// `ItemTable` values arrive as text or (sometimes) UTF-8/UTF-16LE blobs.
/// UTF-16LE bytes misread as UTF-8 decode "successfully" with interleaved
/// NULs, so a NUL-riddled result routes to the UTF-16LE decoder.
fn decode_sqlite_string(value: rusqlite::types::ValueRef<'_>) -> Option<String> {
    match value {
        rusqlite::types::ValueRef::Text(bytes) => String::from_utf8(bytes.to_vec()).ok(),
        rusqlite::types::ValueRef::Blob(bytes) => String::from_utf8(bytes.to_vec())
            .ok()
            .filter(|decoded| !decoded.contains('\0'))
            .or_else(|| decode_utf16_le(bytes)),
        _ => None,
    }
}

fn decode_utf16_le(bytes: &[u8]) -> Option<String> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16(&units).ok()
}

/// Local app session preference for Automatic mode: the validated header
/// persisted after a successful app-session fetch, else a fresh read of the
/// app database.
pub fn preferred_auto_cookie_header() -> Option<String> {
    if let Some(cached) =
        crate::browser::cookie_cache::CookieHeaderCache::load(crate::core::ProviderId::Cursor)
        && cached.source_label == "cursor-app"
        && !cached.is_stale(APP_SESSION_MAX_AGE_SECS)
    {
        return Some(cached.cookie_header);
    }
    let access_token = load_app_auth_access_token()?;
    app_session_cookie_header(&access_token)
}

/// Persist a validated app-session header for reuse across refreshes.
pub fn store_validated_app_session(cookie_header: &str) {
    if let Err(err) = crate::browser::cookie_cache::CookieHeaderCache::store(
        crate::core::ProviderId::Cursor,
        cookie_header,
        "cursor-app",
    ) {
        tracing::debug!("Could not persist Cursor app session: {err}");
    }
}

/// Validated app sessions go stale faster than browser imports: the app
/// rotates its token itself, so re-read the database every 15 minutes.
const APP_SESSION_MAX_AGE_SECS: i64 = 15 * 60;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_header_rebuilds_from_jwt_subject() {
        use base64::Engine;
        let payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"sub":"user_123"}"#);
        let token = format!("header.{payload}.signature");
        let header = app_session_cookie_header(&token).expect("cookie header");
        assert_eq!(
            header,
            format!("WorkosCursorSessionToken=user_123%3A%3A{token}")
        );
    }

    #[test]
    fn empty_or_malformed_token_yields_no_cookie() {
        assert!(app_session_cookie_header("   ").is_none());
        // A non-JWT token still builds a header with an empty subject — the
        // server rejects it and the caller falls back to browser cookies.
        assert_eq!(
            app_session_cookie_header("opaque-token").as_deref(),
            Some("WorkosCursorSessionToken=%3A%3Aopaque-token")
        );
    }

    #[test]
    fn utf16_le_blob_decodes() {
        let bytes: Vec<u8> = "WorkosCursorSessionToken"
            .encode_utf16()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        assert_eq!(
            decode_sqlite_string(rusqlite::types::ValueRef::Blob(&bytes)).as_deref(),
            Some("WorkosCursorSessionToken")
        );
        // Unpaired surrogate (0xD800) is invalid UTF-16 → no value.
        assert!(
            decode_sqlite_string(rusqlite::types::ValueRef::Blob(&[0x00, 0xd8, 0x41, 0x00]))
                .is_none()
        );
    }

    #[test]
    fn db_path_points_at_cursor_global_storage() {
        let path = app_auth_db_path().expect("path");
        assert!(path.ends_with("state.vscdb"));
        assert!(path.to_string_lossy().contains("Cursor"));
    }
}
