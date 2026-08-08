//! WAL-safe read-only SQLite access for app-owned databases.
//!
//! Port of the upstream pattern used by `KimiDesktopAuthToken` (#2622) and
//! OpenCode Go (#2544): opening a Chromium/Electron-owned SQLite database
//! read-only must never create `-wal`/`-shm` sidecar files next to the real
//! database (an "idle WAL" database whose sidecars were removed at clean
//! shutdown would otherwise have them recreated).
//!
//! Strategy:
//! 1. If the WAL sidecars are missing, prefer an `immutable=1` URI open
//!    (never creates sidecars; the database is read as-checkpointed).
//! 2. Otherwise open plain read-only with a short busy timeout.
//! 3. On `SQLITE_CANTOPEN` with missing sidecars, fall back to immutable.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

/// Default busy timeout for app-database reads — long enough to ride out a
/// checkpoint, short enough not to stall provider refresh.
pub const DEFAULT_SQLITE_BUSY_TIMEOUT: Duration = Duration::from_millis(250);

/// Open `db_path` read-only without creating `-wal`/`-shm` sidecars.
pub fn open_readonly_sqlite_connection(
    db_path: &Path,
    busy_timeout: Duration,
) -> Result<Connection, rusqlite::Error> {
    // Prefer immutable URI when sidecars are absent so a clean WAL shutdown
    // (header still WAL, no -wal/-shm) does not recreate them on open.
    if sqlite_wal_sidecars_missing(db_path)
        && let Ok(conn) = open_immutable_sqlite_connection(db_path, busy_timeout)
    {
        return Ok(conn);
    }

    match open_plain_readonly_sqlite_connection(db_path, busy_timeout) {
        Ok(conn) => Ok(conn),
        Err(err) if is_sqlite_cant_open(&err) && sqlite_wal_sidecars_missing(db_path) => {
            open_immutable_sqlite_connection(db_path, busy_timeout)
        }
        Err(err) => Err(err),
    }
}

fn open_plain_readonly_sqlite_connection(
    db_path: &Path,
    busy_timeout: Duration,
) -> Result<Connection, rusqlite::Error> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.busy_timeout(busy_timeout)?;
    Ok(conn)
}

fn open_immutable_sqlite_connection(
    db_path: &Path,
    busy_timeout: Duration,
) -> Result<Connection, rusqlite::Error> {
    let uri = sqlite_immutable_uri(db_path);
    let conn = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.busy_timeout(busy_timeout)?;
    Ok(conn)
}

/// `file:<absolute>?immutable=1` URI; strips the Windows `\\?\` prefix and
/// normalizes separators so SQLite's URI parser accepts drive letters.
pub fn sqlite_immutable_uri(db_path: &Path) -> String {
    let abs = db_path
        .canonicalize()
        .unwrap_or_else(|_| db_path.to_path_buf());
    let raw = abs.to_string_lossy();
    // Windows canonicalize() yields `\\?\C:\...`; strip that for SQLite URIs.
    let stripped = raw
        .strip_prefix(r"\\?\")
        .or_else(|| raw.strip_prefix("//?/"))
        .unwrap_or(raw.as_ref());
    let path = stripped.replace('\\', "/");
    // Prefer `file:` (no authority) so drive letters stay valid on Windows.
    format!("file:{path}?immutable=1")
}

/// Both WAL sidecars absent (either the database is not in WAL mode, or its
/// WAL was fully checkpointed at clean shutdown).
pub fn sqlite_wal_sidecars_missing(db_path: &Path) -> bool {
    let wal = sqlite_sidecar_path(db_path, "-wal");
    let shm = sqlite_sidecar_path(db_path, "-shm");
    !wal.exists() && !shm.exists()
}

/// `<path>-wal` / `<path>-shm` style sidecar paths.
pub fn sqlite_sidecar_path(db_path: &Path, suffix: &str) -> PathBuf {
    let mut s = db_path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

/// SQLITE_CANTOPEN detection (rusqlite may wrap it textually on some builds).
fn is_sqlite_cant_open(err: &rusqlite::Error) -> bool {
    matches!(
        err.sqlite_error_code(),
        Some(rusqlite::ErrorCode::CannotOpen)
    ) || err
        .to_string()
        .to_ascii_lowercase()
        .contains("unable to open")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immutable_uri_has_no_backslashes_and_keeps_drive_letters() {
        // Relative paths must canonicalize into absolute ones.
        let path = Path::new("some dir/cookies.db");
        let uri = sqlite_immutable_uri(path);
        assert!(uri.starts_with("file:"), "uri must be a file: URI: {uri}");
        assert!(uri.ends_with("?immutable=1"), "immutable flag: {uri}");
        assert!(!uri.contains('\\'), "separators normalized: {uri}");
        assert!(!uri.contains(r"\\?\"), "UNC prefix stripped: {uri}");
    }

    /// Build an "idle WAL, no sidecars" database by copying a checkpointed
    /// main database into a sidecar-free directory (mirrors how the app ends
    /// up after a clean shutdown removes sidecars).
    fn make_idle_wal_db(dir: &Path) -> PathBuf {
        let source_dir = dir.join("source");
        std::fs::create_dir_all(&source_dir).expect("mkdir source");
        let source = source_dir.join("cookies.db");
        {
            let conn = Connection::open(&source).expect("create");
            conn.execute_batch(
                "PRAGMA journal_mode = WAL; CREATE TABLE t (v TEXT); INSERT INTO t VALUES ('a');",
            )
            .expect("init");
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                .expect("checkpoint");
        }
        let target = dir.join("cookies.db");
        std::fs::copy(&source, &target).expect("copy main db");
        target
    }

    #[test]
    fn read_only_open_does_not_create_wal_sidecars() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = make_idle_wal_db(dir.path());
        assert!(sqlite_wal_sidecars_missing(&db));

        let conn = open_readonly_sqlite_connection(&db, Duration::from_millis(10))
            .expect("read-only open");
        let value: String = conn
            .query_row("SELECT v FROM t", [], |row| row.get(0))
            .expect("read");
        assert_eq!(value, "a");
        drop(conn);
        assert!(
            sqlite_wal_sidecars_missing(&db),
            "read-only open must not create -wal/-shm sidecars"
        );
    }

    #[test]
    fn active_wal_readable_without_checking_it_out() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("cookies.db");
        let conn = Connection::open(&db).expect("create");
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; CREATE TABLE t (v TEXT); INSERT INTO t VALUES ('in-wal');",
        )
        .expect("init");
        let wal = sqlite_sidecar_path(&db, "-wal");
        assert!(wal.exists(), "active WAL sidecar should exist");

        let read = open_readonly_sqlite_connection(&db, Duration::from_millis(50)).expect("read");
        let value: String = read
            .query_row("SELECT v FROM t", [], |row| row.get(0))
            .expect("read");
        assert_eq!(value, "in-wal");
        drop(read);
        assert!(wal.exists(), "reading must not checkpoint the active WAL");
    }

    #[test]
    fn missing_path_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("nope.db");
        assert!(open_readonly_sqlite_connection(&db, Duration::ZERO).is_err());
    }
}
