use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Local, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags, types::ValueRef};

use super::local_proto::{ParsedTurn, parse_turn};
use super::local_sessions::{LocalHistoryCoverage, LocalSessionSummary};

const MAX_DATABASES: usize = 500;
const MAX_DIRECTORY_ENTRIES: usize = 10_000;
const MAX_ROWS_PER_DATABASE: usize = 10_000;
const MAX_ROWS: usize = 50_000;
const MAX_BLOB_BYTES: usize = 16 * 1024 * 1024;
const MAX_DATABASE_BYTES: usize = 64 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug)]
pub(super) enum SQLiteScan {
    NoDatabases,
    Summary(LocalSessionSummary),
}

#[derive(Default)]
struct Budget {
    directory_entries: usize,
    databases: usize,
    rows: usize,
    bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Event {
    session: String,
    row: i64,
    turn: ParsedTurn,
    total: u64,
}

pub(super) fn summarize(home: &Path, now: DateTime<Utc>, days: u32) -> SQLiteScan {
    let (paths, discovery_complete) = discover_databases(home);
    if paths.is_empty() && discovery_complete {
        return SQLiteScan::NoDatabases;
    }

    let first_day = now.with_timezone(&Local).date_naive()
        - Duration::days(i64::from(days.clamp(1, 365).saturating_sub(1)));
    let mut budget = Budget::default();
    let mut complete = discovery_complete;
    let mut events = Vec::new();

    for path in paths.iter().take(MAX_DATABASES) {
        budget.databases += 1;
        if budget.databases > MAX_DATABASES {
            complete = false;
            break;
        }
        match read_database(path, &mut budget) {
            Ok((mut rows, is_complete)) => {
                events.append(&mut rows);
                complete &= is_complete;
            }
            Err(_) => complete = false,
        }
        if budget.rows >= MAX_ROWS || budget.bytes >= MAX_TOTAL_BYTES {
            complete = false;
            break;
        }
    }

    let mut total_tokens = 0_u64;
    let mut sessions = HashSet::new();
    let mut rows: HashMap<(String, i64), Event> = HashMap::new();
    let mut responses: HashMap<(String, String), Event> = HashMap::new();

    for event in events {
        let row_key = (event.session.clone(), event.row);
        if let Some(prior) = rows.get(&row_key) {
            if prior != &event {
                complete = false;
            }
            continue;
        }

        if let Some(response_id) = event
            .turn
            .usage
            .as_ref()
            .and_then(|usage| usage.response_id.as_ref())
        {
            let response_key = (event.session.clone(), response_id.clone());
            if let Some(prior) = responses.get(&response_key) {
                if prior.turn != event.turn {
                    complete = false;
                } else {
                    rows.insert(row_key, event);
                }
                continue;
            }
            responses.insert(response_key, event.clone());
        }

        let Some(timestamp_ms) = event.turn.timestamp_ms else {
            complete = false;
            continue;
        };
        let Some(at) = Utc.timestamp_millis_opt(timestamp_ms).single() else {
            complete = false;
            continue;
        };
        rows.insert(row_key, event.clone());
        if at > now || at.with_timezone(&Local).date_naive() < first_day {
            continue;
        }
        match total_tokens.checked_add(event.total) {
            Some(total) => total_tokens = total,
            None => {
                complete = false;
                continue;
            }
        }
        sessions.insert(event.session);
    }

    SQLiteScan::Summary(LocalSessionSummary {
        total_tokens,
        session_count: sessions.len(),
        coverage: if complete {
            LocalHistoryCoverage::Complete
        } else {
            LocalHistoryCoverage::Partial
        },
    })
}

fn discover_databases(home: &Path) -> (Vec<PathBuf>, bool) {
    let gemini = home.join(".gemini");
    let roots = [
        gemini.join("antigravity-cli").join("conversations"),
        gemini.join("antigravity"),
        gemini.join("antigravity").join("conversations"),
    ];
    let mut paths = Vec::new();
    let mut complete = true;
    let mut entries_seen = 0usize;

    for root in roots {
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                complete = false;
                continue;
            }
        };
        for entry in entries {
            entries_seen += 1;
            if entries_seen > MAX_DIRECTORY_ENTRIES {
                complete = false;
                break;
            }
            let Ok(entry) = entry else {
                complete = false;
                continue;
            };
            let path = entry.path();
            let hidden = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.'));
            if hidden || path.extension().and_then(|value| value.to_str()) != Some("db") {
                continue;
            }
            match entry.file_type() {
                Ok(kind) if kind.is_file() => paths.push(path),
                Ok(_) => complete = false,
                Err(_) => complete = false,
            }
            if paths.len() >= MAX_DATABASES {
                complete = false;
                break;
            }
        }
    }
    paths.sort();
    paths.dedup();
    (paths, complete)
}

fn read_database(path: &Path, budget: &mut Budget) -> rusqlite::Result<(Vec<Event>, bool)> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !supported_schema(&conn)? {
        return Ok((Vec::new(), false));
    }

    let session = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("unknown")
        .to_string();
    let mut statement = conn.prepare(
        "SELECT idx, CASE WHEN typeof(data) = 'blob' THEN length(data) END, data FROM main.gen_metadata NOT INDEXED LIMIT ?1",
    )?;
    let mut query =
        statement.query([i64::try_from(MAX_ROWS_PER_DATABASE + 1).unwrap_or(i64::MAX)])?;
    let mut database_bytes = 0usize;
    let mut database_rows = 0usize;
    let mut complete = true;
    let mut events = Vec::new();

    while let Some(row) = query.next()? {
        database_rows += 1;
        budget.rows += 1;
        if database_rows > MAX_ROWS_PER_DATABASE || budget.rows > MAX_ROWS {
            complete = false;
            break;
        }

        let idx: i64 = match row.get(0) {
            Ok(value) if value >= 0 => value,
            _ => {
                complete = false;
                continue;
            }
        };
        let declared: Option<i64> = row.get(1).ok();
        let Some(declared) = declared.and_then(|value| usize::try_from(value).ok()) else {
            complete = false;
            continue;
        };
        database_bytes = match database_bytes.checked_add(declared) {
            Some(value) if value <= MAX_DATABASE_BYTES => value,
            _ => {
                complete = false;
                break;
            }
        };
        budget.bytes = match budget.bytes.checked_add(declared) {
            Some(value) if value <= MAX_TOTAL_BYTES => value,
            _ => {
                complete = false;
                break;
            }
        };
        if declared == 0 || declared > MAX_BLOB_BYTES {
            complete = false;
            continue;
        }

        let blob = match row.get_ref(2)? {
            ValueRef::Blob(bytes) if bytes.len() == declared => bytes,
            _ => {
                complete = false;
                continue;
            }
        };
        let Some(turn) = parse_turn(blob) else {
            complete = false;
            continue;
        };
        let Some(usage) = turn.usage.as_ref() else {
            complete = false;
            continue;
        };
        if turn.timestamp_ms.is_none() {
            complete = false;
            continue;
        }
        let Some(input) = usage.system_prompt.checked_add(usage.new_input) else {
            complete = false;
            continue;
        };
        let Some(total) = input
            .checked_add(usage.output)
            .and_then(|value| value.checked_add(usage.cache_read))
            .and_then(|value| value.checked_add(usage.reasoning))
        else {
            complete = false;
            continue;
        };
        events.push(Event {
            session: session.clone(),
            row: idx,
            turn,
            total,
        });
    }

    Ok((events, complete))
}

fn supported_schema(conn: &Connection) -> rusqlite::Result<bool> {
    let mut statement = conn.prepare("SELECT name, type, rootpage FROM main.sqlite_master WHERE lower(name)='gen_metadata' LIMIT 2")?;
    let mut rows = statement.query([])?;
    let Some(row) = rows.next()? else {
        return Ok(false);
    };
    let name: String = row.get(0)?;
    let kind: String = row.get(1)?;
    let rootpage: i64 = row.get(2)?;
    if !name.eq_ignore_ascii_case("gen_metadata") || kind != "table" || rootpage <= 0 {
        return Ok(false);
    }
    if rows.next()?.is_some() {
        return Ok(false);
    }

    let mut columns = HashSet::new();
    let mut info = conn.prepare("PRAGMA main.table_xinfo('gen_metadata')")?;
    let mut rows = info.query([])?;
    while let Some(row) = rows.next()? {
        let hidden: i64 = row.get(6)?;
        if hidden != 0 {
            return Ok(false);
        }
        let name: String = row.get(1)?;
        columns.insert(name.to_ascii_lowercase());
    }
    Ok(columns.contains("idx") && columns.contains("data"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    #[test]
    fn missing_databases_falls_through() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            summarize(dir.path(), Utc::now(), 30),
            SQLiteScan::NoDatabases
        ));
    }

    #[test]
    fn unsupported_database_is_partial_not_zero() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".gemini/antigravity-cli/conversations");
        fs::create_dir_all(&root).unwrap();
        let conn = Connection::open(root.join("one.db")).unwrap();
        conn.execute("CREATE TABLE wrong(idx INTEGER, data BLOB)", [])
            .unwrap();
        drop(conn);
        let SQLiteScan::Summary(summary) = summarize(dir.path(), Utc::now(), 30) else {
            panic!("database should be attempted");
        };
        assert_eq!(summary.coverage, LocalHistoryCoverage::Partial);
        assert_eq!(summary.total_tokens, 0);
    }

    #[test]
    fn empty_supported_database_is_confirmed_zero() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".gemini/antigravity-cli/conversations");
        fs::create_dir_all(&root).unwrap();
        let conn = Connection::open(root.join("one.db")).unwrap();
        conn.execute("CREATE TABLE gen_metadata(idx INTEGER, data BLOB)", [])
            .unwrap();
        drop(conn);
        let SQLiteScan::Summary(summary) = summarize(dir.path(), Utc::now(), 30) else {
            panic!("supported database should produce coverage");
        };
        assert_eq!(summary.coverage, LocalHistoryCoverage::Complete);
        assert_eq!(summary.total_tokens, 0);
        assert_eq!(summary.session_count, 0);
    }

    #[test]
    fn non_blob_rows_make_coverage_partial() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".gemini/antigravity-cli/conversations");
        fs::create_dir_all(&root).unwrap();
        let conn = Connection::open(root.join("one.db")).unwrap();
        conn.execute("CREATE TABLE gen_metadata(idx INTEGER, data BLOB)", [])
            .unwrap();
        conn.execute(
            "INSERT INTO gen_metadata(idx,data) VALUES(?1,?2)",
            params![1_i64, "not-a-blob"],
        )
        .unwrap();
        drop(conn);
        let SQLiteScan::Summary(summary) = summarize(dir.path(), Utc::now(), 30) else {
            panic!("supported database should produce coverage");
        };
        assert_eq!(summary.coverage, LocalHistoryCoverage::Partial);
    }
}
