use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{OpenCodexEntry, parse_line};

const CACHE_SCHEMA_VERSION: i64 = 2;
const PREFIX_DIGEST_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct ParseCursor {
    source_path: String,
    file_identity: String,
    pub(super) parsed_offset: u64,
    prefix_digest: String,
}

#[derive(Debug, Clone)]
pub(super) struct CacheState {
    pub(super) cursor: ParseCursor,
    entries: Vec<OpenCodexEntry>,
}

#[derive(Debug)]
struct LogIdentity {
    source_path: String,
    file_identity: String,
    size: u64,
}

#[derive(Debug)]
struct ParsedSegment {
    committed: Vec<OpenCodexEntry>,
    pending: Vec<OpenCodexEntry>,
    next_offset: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CacheWrite {
    Applied,
    Stale,
}

pub(super) fn load_entries(source_path: &Path) -> Option<Vec<OpenCodexEntry>> {
    let cache = cache_path()?;
    load_entries_with_cache(source_path, &cache)
}

pub(super) fn load_entries_with_cache(
    source_path: &Path,
    cache_path: &Path,
) -> Option<Vec<OpenCodexEntry>> {
    for _ in 0..2 {
        let identity = log_identity(source_path)?;
        let state = read_cache(cache_path);
        if let Some(state) = state.as_ref()
            && cursor_matches_source(&state.cursor, &identity, source_path)
        {
            if state.cursor.parsed_offset == identity.size {
                return Some(state.entries.clone());
            }
            if identity.size > state.cursor.parsed_offset {
                let parsed = parse_segment(source_path, state.cursor.parsed_offset, identity.size)?;
                let mut visible = state.entries.clone();
                visible.extend(parsed.committed.iter().cloned());
                visible.extend(parsed.pending.iter().cloned());
                let visible = dedup_entries(visible);

                let next_cursor = ParseCursor {
                    source_path: identity.source_path.clone(),
                    file_identity: identity.file_identity.clone(),
                    parsed_offset: parsed.next_offset,
                    prefix_digest: prefix_digest(source_path, parsed.next_offset)?,
                };
                match write_incremental_cache(
                    cache_path,
                    &state.cursor,
                    &next_cursor,
                    &parsed.committed,
                ) {
                    CacheWrite::Applied => return Some(visible),
                    CacheWrite::Stale => continue,
                }
            }
        }

        let parsed = parse_segment(source_path, 0, identity.size)?;
        let mut visible = parsed.committed.clone();
        visible.extend(parsed.pending.iter().cloned());
        let visible = dedup_entries(visible);
        let cursor = ParseCursor {
            source_path: identity.source_path.clone(),
            file_identity: identity.file_identity.clone(),
            parsed_offset: parsed.next_offset,
            prefix_digest: prefix_digest(source_path, parsed.next_offset)?,
        };
        let current = log_identity(source_path)?;
        if current.file_identity != identity.file_identity || current.size < identity.size {
            continue;
        }
        write_full_cache(cache_path, &cursor, &parsed.committed);
        return Some(visible);
    }
    None
}

fn parse_segment(
    source_path: &Path,
    start_offset: u64,
    snapshot_len: u64,
) -> Option<ParsedSegment> {
    if start_offset > snapshot_len {
        return None;
    }
    let mut file = File::open(source_path).ok()?;
    file.seek(SeekFrom::Start(start_offset)).ok()?;
    let expected = snapshot_len.saturating_sub(start_offset);
    let mut bytes = Vec::with_capacity(usize::try_from(expected).ok()?);
    file.take(expected).read_to_end(&mut bytes).ok()?;
    if u64::try_from(bytes.len()).ok()? != expected {
        return None;
    }

    let mut committed = Vec::new();
    let mut line_start = 0usize;
    let mut next_offset = start_offset;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        if let Ok(line) = std::str::from_utf8(&bytes[line_start..index])
            && let Some(entry) = parse_line(line.trim_end_matches('\r'))
        {
            committed.push(entry);
        }
        line_start = index + 1;
        next_offset = start_offset.saturating_add(u64::try_from(line_start).ok()?);
    }
    let mut pending = Vec::new();
    if line_start < bytes.len()
        && let Ok(line) = std::str::from_utf8(&bytes[line_start..])
        && let Some(entry) = parse_line(line.trim_end_matches('\r'))
    {
        pending.push(entry);
    }
    Some(ParsedSegment {
        committed,
        pending,
        next_offset,
    })
}

fn cursor_matches_source(cursor: &ParseCursor, identity: &LogIdentity, source_path: &Path) -> bool {
    cursor.source_path == identity.source_path
        && cursor.file_identity == identity.file_identity
        && identity.size >= cursor.parsed_offset
        && prefix_digest(source_path, cursor.parsed_offset)
            .is_some_and(|digest| digest == cursor.prefix_digest)
}

fn dedup_entries(entries: Vec<OpenCodexEntry>) -> Vec<OpenCodexEntry> {
    let mut unique = HashMap::new();
    for entry in entries {
        unique.insert(entry.request_id.clone(), entry);
    }
    let mut entries: Vec<_> = unique.into_values().collect();
    entries.sort_by(|left, right| {
        left.timestamp
            .cmp(&right.timestamp)
            .then_with(|| left.request_id.cmp(&right.request_id))
    });
    entries
}

fn cache_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|root| {
        root.join("openCodexBar")
            .join("opencodex")
            .join("usage-cache-v2.sqlite")
    })
}

fn open_cache(path: &Path) -> Option<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok()?;
    }
    let conn = Connection::open(path).ok()?;
    conn.busy_timeout(std::time::Duration::from_secs(2)).ok()?;
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .ok()?;
    if version != CACHE_SCHEMA_VERSION {
        conn.execute_batch(
            "DROP TABLE IF EXISTS entries;
             DROP TABLE IF EXISTS meta;
             CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE entries (request_id TEXT PRIMARY KEY, payload TEXT NOT NULL);",
        )
        .ok()?;
        conn.pragma_update(None, "user_version", CACHE_SCHEMA_VERSION)
            .ok()?;
    }
    Some(conn)
}

pub(super) fn read_cache(path: &Path) -> Option<CacheState> {
    if !path.exists() {
        return None;
    }
    let conn = open_cache(path)?;
    read_cache_state(&conn)
}

fn read_cache_state(conn: &Connection) -> Option<CacheState> {
    let cursor_json: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'parseCursor'",
            [],
            |row| row.get(0),
        )
        .optional()
        .ok()??;
    let cursor: ParseCursor = serde_json::from_str(&cursor_json).ok()?;
    let mut statement = conn
        .prepare("SELECT payload FROM entries ORDER BY request_id")
        .ok()?;
    let entries = statement
        .query_map([], |row| row.get::<_, String>(0))
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|payload| serde_json::from_str::<OpenCodexEntry>(&payload).ok())
        .collect();
    Some(CacheState { cursor, entries })
}

fn write_full_cache(path: &Path, cursor: &ParseCursor, entries: &[OpenCodexEntry]) {
    let Some(mut conn) = open_cache(path) else {
        return;
    };
    let Ok(tx) = conn.transaction_with_behavior(TransactionBehavior::Immediate) else {
        return;
    };
    if tx.execute("DELETE FROM entries", []).is_err()
        || set_cursor(&tx, cursor).is_err()
        || !upsert_entries(&tx, entries)
    {
        return;
    }
    let _committed = tx.commit();
}

fn write_incremental_cache(
    path: &Path,
    base_cursor: &ParseCursor,
    next_cursor: &ParseCursor,
    entries: &[OpenCodexEntry],
) -> CacheWrite {
    let Some(mut conn) = open_cache(path) else {
        return CacheWrite::Applied;
    };
    let Ok(tx) = conn.transaction_with_behavior(TransactionBehavior::Immediate) else {
        return CacheWrite::Stale;
    };
    let durable_cursor = tx
        .query_row(
            "SELECT value FROM meta WHERE key = 'parseCursor'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_str::<ParseCursor>(&value).ok());
    if durable_cursor.as_ref() != Some(base_cursor) {
        return CacheWrite::Stale;
    }
    if set_cursor(&tx, next_cursor).is_err() || !upsert_entries(&tx, entries) {
        return CacheWrite::Stale;
    }
    if tx.commit().is_ok() {
        CacheWrite::Applied
    } else {
        CacheWrite::Stale
    }
}

fn set_cursor(tx: &rusqlite::Transaction<'_>, cursor: &ParseCursor) -> rusqlite::Result<()> {
    let value = serde_json::to_string(cursor)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES('parseCursor', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![value],
    )?;
    Ok(())
}

fn upsert_entries(tx: &rusqlite::Transaction<'_>, entries: &[OpenCodexEntry]) -> bool {
    let Ok(mut statement) = tx.prepare(
        "INSERT INTO entries(request_id, payload) VALUES(?1, ?2)
         ON CONFLICT(request_id) DO UPDATE SET payload = excluded.payload",
    ) else {
        return false;
    };
    for entry in entries {
        let Ok(payload) = serde_json::to_string(entry) else {
            return false;
        };
        if statement
            .execute(params![entry.request_id, payload])
            .is_err()
        {
            return false;
        }
    }
    true
}

fn prefix_digest(source_path: &Path, parsed_offset: u64) -> Option<String> {
    let file = File::open(source_path).ok()?;
    let byte_count = parsed_offset.min(PREFIX_DIGEST_BYTES);
    let mut bytes = Vec::with_capacity(usize::try_from(byte_count).ok()?);
    file.take(byte_count).read_to_end(&mut bytes).ok()?;
    if u64::try_from(bytes.len()).ok()? != byte_count {
        return None;
    }
    let digest = Sha256::digest(&bytes);
    Some(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn log_identity(source_path: &Path) -> Option<LogIdentity> {
    let metadata = fs::metadata(source_path).ok()?;

    Some(LogIdentity {
        source_path: source_path.to_string_lossy().to_string(),
        file_identity: platform_file_identity(source_path, &metadata)?,
        size: metadata.len(),
    })
}

#[cfg(windows)]
fn platform_file_identity(source_path: &Path, _metadata: &fs::Metadata) -> Option<String> {
    use std::os::windows::io::AsRawHandle;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let file = File::open(source_path).ok()?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `file` is an open file handle and `info` is valid for writes for
    // the duration of the call.
    let ok = unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) };
    if ok.is_err() {
        return None;
    }
    let file_index = ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64;
    Some(format!("{}:{}", info.dwVolumeSerialNumber, file_index))
}

#[cfg(unix)]
fn platform_file_identity(_source_path: &Path, metadata: &fs::Metadata) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    Some(format!("{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(not(any(windows, unix)))]
fn platform_file_identity(_source_path: &Path, metadata: &fs::Metadata) -> Option<String> {
    metadata
        .created()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_nanos().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_path_replacement_forces_reparse() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_path = dir.path().join("sessions.jsonl");
        std::fs::write(&log_path, b"{\"a\":1}\n").expect("write log");

        let identity_before = log_identity(&log_path).expect("identity before");

        // Simulate replacement of the file at the same path (log rotation /
        // re-creation by the provider CLI): move the old file aside, then
        // re-create a fresh file at the original path.
        std::fs::rename(&log_path, dir.path().join("sessions.jsonl.old")).expect("rotate log");
        std::fs::write(&log_path, b"{\"a\":1}\n{\"b\":2}\n").expect("rewrite log");

        let identity_after = log_identity(&log_path).expect("identity after");
        assert_ne!(
            identity_before.file_identity, identity_after.file_identity,
            "re-created file must get a new file identity so the cache cursor is invalidated"
        );
        assert!(identity_after.size >= identity_before.size);
    }
}
