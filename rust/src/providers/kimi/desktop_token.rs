//! Read-only access to the official Kimi Desktop (Chromium) cookie store.
//!
//! Port of upstream 0.48.0 `KimiDesktopAuthToken` (#2622): enrich Kimi Code
//! API / CLI usage with the monthly membership pool from a signed-in Kimi
//! Desktop session. Windows adaptations:
//!
//! - The cookie database lives at `%APPDATA%\kimi-desktop\Cookies`
//!   (Roaming AppData — the Electron `userData` path; upstream reads
//!   `~/Library/Application Support/kimi-desktop/Cookies`, same layout).
//! - WAL-safe, read-only, no-copy: `core::open_readonly_sqlite_connection`
//!   implements upstream's exact open policy (plain read-only first,
//!   `immutable=1` fallback when WAL sidecars are absent after a clean
//!   shutdown) and never creates sidecar files next to the real store.
//! - Chromium on Windows encrypts cookie values (`encrypted_value`,
//!   AES-256-GCM keyed from `Local State`, DPAPI-wrapped). Plaintext `value`
//!   is preferred (upstream reads only `value`); the encrypted form falls
//!   back to the existing `browser::cookies` decryption helpers.
//!
//! Auth cookies are secrets: token values are never logged.

use std::path::{Path, PathBuf};

pub struct KimiDesktopAuthToken;

/// `userData` subdirectory written by the Kimi Desktop app.
const DESKTOP_APP_DIR: &str = "kimi-desktop";
const COOKIES_FILE: &str = "Cookies";
const LOCAL_STATE_FILE: &str = "Local State";
const AUTH_COOKIE_NAME: &str = "kimi-auth";
const AUTH_COOKIE_HOSTS: [&str; 4] = ["www.kimi.com", ".www.kimi.com", ".kimi.com", "kimi.com"];

impl KimiDesktopAuthToken {
    /// Cookies database inside a caller-provided `data_root` (upstream
    /// `cookiesDatabaseURL(homeDirectory:)` shape for test injection).
    pub fn cookies_database_path(data_root: &Path) -> PathBuf {
        data_root.join(DESKTOP_APP_DIR).join(COOKIES_FILE)
    }

    /// `Local State` file carrying the DPAPI-wrapped AES-GCM key.
    pub fn local_state_path(data_root: &Path) -> PathBuf {
        data_root.join(DESKTOP_APP_DIR).join(LOCAL_STATE_FILE)
    }

    /// Most recently accessed `kimi-auth` token from the signed-in Kimi
    /// Desktop session, or `None` when the app/database/cookie is absent or
    /// unreadable. Production entry point.
    pub fn load() -> Option<String> {
        let data_root = dirs::data_dir()?;
        Self::load_from(&data_root)
    }

    /// Read from an explicit `data_root` (Electron `userData` parent).
    pub fn load_from(data_root: &Path) -> Option<String> {
        let aes_key = crate::browser::cookies::CookieExtractor::get_chromium_encryption_key(
            &Self::local_state_path(data_root),
        )
        .inspect_err(|err| {
            tracing::debug!(
                error = %err,
                "Kimi Desktop encryption key unavailable; only plaintext cookies are readable"
            );
        })
        .ok();
        Self::load_token(&Self::cookies_database_path(data_root), aes_key.as_deref())
    }

    /// Core read (upstream `read(databaseURL:immutable:)`): WAL-safe
    /// read-only open → newest `kimi-auth` row → decode. `aes_key` is the
    /// Chromium app cookie key; `None` restricts reads to plaintext rows.
    fn load_token(database_path: &Path, aes_key: Option<&[u8]>) -> Option<String> {
        if !database_path.is_file() {
            return None;
        }
        let conn = crate::core::open_readonly_sqlite_connection(
            database_path,
            crate::core::DEFAULT_SQLITE_BUSY_TIMEOUT,
        )
        .inspect_err(|err| {
            tracing::debug!(error = %err, "Kimi Desktop Cookies open failed");
        })
        .ok()?;
        read_newest_auth_cookie(&conn)
            .inspect_err(|err| {
                tracing::debug!(error = %err, "Kimi Desktop cookies read failed");
            })
            .ok()
            .and_then(|row| decode_cookie_value(row, aes_key))
    }
}

/// Decode a `(value, encrypted_value)` pair: plaintext first, AES-256-GCM
/// fallback (upstream reads `value` only; Windows Chromium rows are usually
/// encrypted).
fn decode_cookie_value(row: (String, Vec<u8>), aes_key: Option<&[u8]>) -> Option<String> {
    let (value, encrypted_value) = row;
    let trimmed = value.trim();
    if !trimmed.is_empty() {
        return Some(trimmed.to_string());
    }
    let aes_key = aes_key?;
    if encrypted_value.is_empty() {
        return None;
    }
    crate::browser::cookies::CookieExtractor::decrypt_chromium_cookie(&encrypted_value, aes_key)
        .map(|plain| plain.trim().to_string())
        .ok()
        .filter(|plain| !plain.is_empty())
}

fn read_newest_auth_cookie(conn: &rusqlite::Connection) -> rusqlite::Result<(String, Vec<u8>)> {
    // Upstream query verbatim: newest `kimi-auth` across the registered
    // kimi.com cookie scopes by last access.
    let mut statement = conn.prepare(
        "SELECT value, encrypted_value
         FROM cookies
         WHERE name = ?1
           AND host_key IN (?2, ?3, ?4, ?5)
         ORDER BY last_access_utc DESC
         LIMIT 1",
    )?;
    statement.query_row(
        rusqlite::params![
            AUTH_COOKIE_NAME,
            AUTH_COOKIE_HOSTS[0],
            AUTH_COOKIE_HOSTS[1],
            AUTH_COOKIE_HOSTS[2],
            AUTH_COOKIE_HOSTS[3],
        ],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn make_environment() -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join(DESKTOP_APP_DIR);
        std::fs::create_dir_all(&dir).expect("mkdir");
        (root, dir.join(COOKIES_FILE))
    }

    fn create_database(path: &Path) {
        let conn = Connection::open(path).expect("create db");
        create_schema(&conn);
    }

    fn create_schema(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE cookies (
                host_key TEXT NOT NULL,
                name TEXT NOT NULL,
                value TEXT NOT NULL,
                encrypted_value BLOB NOT NULL DEFAULT X'',
                last_access_utc INTEGER NOT NULL
            );",
        )
        .expect("create schema");
    }

    fn insert_cookie(db: &Connection, host: &str, value: &str, last_access: i64) {
        insert_cookie_row(db, host, value, &[], last_access);
    }

    fn insert_cookie_row(
        db: &Connection,
        host: &str,
        value: &str,
        encrypted_value: &[u8],
        last_access: i64,
    ) {
        db.execute(
            "INSERT INTO cookies (host_key, name, value, encrypted_value, last_access_utc)
             VALUES (?1, 'kimi-auth', ?2, ?3, ?4)",
            rusqlite::params![host, value, encrypted_value, last_access],
        )
        .expect("insert cookie");
    }

    #[test]
    fn reads_newest_plaintext_kimi_auth_token() {
        let (root, database) = make_environment();
        create_database(&database);
        let conn = Connection::open(&database).expect("open");
        insert_cookie(&conn, "www.kimi.com", "older-token", 1);
        insert_cookie(&conn, ".kimi.com", "newer-token", 2);

        assert_eq!(
            KimiDesktopAuthToken::load_from(root.path()).as_deref(),
            Some("newer-token")
        );
    }

    #[test]
    fn reads_active_wal_without_mutating_the_database() {
        let (root, database) = make_environment();
        let conn = Connection::open(&database).expect("open");
        create_schema(&conn);
        conn.execute_batch("PRAGMA journal_mode = WAL;")
            .expect("wal");
        insert_cookie(&conn, "www.kimi.com", "active-wal-token", 3);

        let wal_path = crate::core::sqlite_sidecar_path(&database, "-wal");
        assert!(wal_path.exists(), "WAL sidecar should exist pre-read");
        assert_eq!(
            KimiDesktopAuthToken::load_from(root.path()).as_deref(),
            Some("active-wal-token")
        );
        assert!(wal_path.exists(), "reading must not checkpoint the WAL");
    }

    #[test]
    fn reads_idle_wal_database_without_creating_sidecars() {
        let (root, database) = make_environment();
        create_database(&database);
        let conn = Connection::open(&database).expect("open");
        conn.execute_batch("PRAGMA journal_mode = WAL;")
            .expect("wal");
        insert_cookie(&conn, "www.kimi.com", "idle-wal-token", 4);
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .expect("checkpoint");
        assert!(conn.close().is_ok(), "close failed");

        // Recreate the post-clean-shutdown state (WAL header, no sidecars)
        // by copying the checkpointed main DB into a fresh directory layout.
        let copied_root = root.path().join("copied-home");
        let copied_dir = copied_root.join(DESKTOP_APP_DIR);
        std::fs::create_dir_all(&copied_dir).expect("mkdir copied");
        let copy = copied_dir.join(COOKIES_FILE);
        std::fs::copy(&database, &copy).expect("copy main db");
        assert!(crate::core::sqlite_wal_sidecars_missing(&copy));

        assert_eq!(
            KimiDesktopAuthToken::load_from(&copied_root).as_deref(),
            Some("idle-wal-token")
        );
        assert!(
            crate::core::sqlite_wal_sidecars_missing(&copy),
            "read must not create -wal/-shm"
        );
    }

    #[test]
    fn ignores_tokens_from_unrelated_hosts() {
        let (root, database) = make_environment();
        create_database(&database);
        let conn = Connection::open(&database).expect("open");
        insert_cookie(&conn, "example.com", "wrong-host-token", 5);

        assert_eq!(KimiDesktopAuthToken::load_from(root.path()), None);
    }

    #[test]
    fn empty_or_whitespace_tokens_are_rejected() {
        let (root, database) = make_environment();
        create_database(&database);
        let conn = Connection::open(&database).expect("open");
        insert_cookie(&conn, "kimi.com", "   \n ", 1);
        insert_cookie(&conn, "kimi.com", "", 2);

        assert_eq!(KimiDesktopAuthToken::load_from(root.path()), None);
    }

    #[test]
    fn malformed_database_returns_none() {
        let (root, database) = make_environment();
        std::fs::write(&database, "definitely not sqlite").expect("write junk");

        assert_eq!(KimiDesktopAuthToken::load_from(root.path()), None);
    }

    #[test]
    fn missing_database_returns_none() {
        let root = tempfile::tempdir().expect("tempdir");
        assert_eq!(KimiDesktopAuthToken::load_from(root.path()), None);
    }

    #[test]
    fn encrypted_cookie_values_decrypt_with_injected_key() {
        use aes_gcm::Aes256Gcm;
        use aes_gcm::aead::{Aead, KeyInit, Payload};

        let key: [u8; 32] = [7; 32];
        let nonce_bytes: [u8; 12] = [9; 12];
        let cipher = Aes256Gcm::new_from_slice(&key).expect("key length");
        let mut encrypted = b"v10".to_vec();
        encrypted.extend_from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(
                aes_gcm::Nonce::from_slice(&nonce_bytes),
                Payload {
                    msg: b"encrypted-kimi-token",
                    aad: &[],
                },
            )
            .expect("encrypt");
        encrypted.extend_from_slice(&ciphertext);

        let (root, database) = make_environment();
        create_database(&database);
        let conn = Connection::open(&database).expect("open");
        insert_cookie_row(&conn, "www.kimi.com", "", &encrypted, 1);

        assert_eq!(
            KimiDesktopAuthToken::load_token(&database, Some(key.as_slice())).as_deref(),
            Some("encrypted-kimi-token")
        );

        // Without a key the encrypted row cannot be used.
        assert_eq!(KimiDesktopAuthToken::load_token(&database, None), None);
        // `load_from` without a usable `Local State` reads plaintext only and
        // yields nothing (no panic, no secret in logs).
        assert_eq!(KimiDesktopAuthToken::load_from(root.path()), None);
    }
}
