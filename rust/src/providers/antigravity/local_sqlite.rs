use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration as StdDuration, Instant};

use chrono::{DateTime, Duration, Local, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags, TransactionBehavior, types::ValueRef};

use super::local_proto::{ParsedTurn, parse_turn};
use super::local_sessions::{LocalHistoryCoverage, LocalSessionSummary};

const MAX_DATABASES: usize = 500;
const MAX_DIRECTORY_ENTRIES: usize = 10_000;
const MAX_ROWS_PER_DATABASE: usize = 10_000;
const MAX_ROWS: usize = 50_000;
const MAX_BLOB_BYTES: usize = 16 * 1024 * 1024;
const MAX_DATABASE_BYTES: usize = 64 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 128 * 1024 * 1024;
const MAX_SCHEMA_ENTRIES: usize = 128;
const MAX_SCHEMA_COLUMNS: usize = 64;
const MAX_SCHEMA_BYTES: usize = 64 * 1024;
const MAX_SCAN_DURATION: StdDuration = StdDuration::from_secs(5);

#[derive(Debug)]
pub(super) enum SQLiteScan {
    NoDatabases,
    Summary(LocalSessionSummary),
}

struct Budget {
    directory_entries: usize,
    databases: usize,
    rows: usize,
    bytes: usize,
    schema_bytes: usize,
    deadline: Instant,
}

impl Budget {
    fn new() -> Self {
        Self::with_deadline(Instant::now() + MAX_SCAN_DURATION)
    }

    fn with_deadline(deadline: Instant) -> Self {
        Self {
            directory_entries: 0,
            databases: 0,
            rows: 0,
            bytes: 0,
            schema_bytes: 0,
            deadline,
        }
    }

    fn check(&self) -> bool {
        Instant::now() < self.deadline
    }

    fn charge_schema_text(&mut self, value: &str) -> bool {
        let Some(next) = self.schema_bytes.checked_add(value.len()) else {
            return false;
        };
        self.schema_bytes = next;
        next <= MAX_SCHEMA_BYTES
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Event {
    session: String,
    row: i64,
    turn: ParsedTurn,
    total: u64,
}

pub(super) fn database_roots(gemini_base: &Path) -> [PathBuf; 3] {
    [
        gemini_base.join("antigravity-cli").join("conversations"),
        gemini_base.join("antigravity"),
        gemini_base.join("antigravity").join("conversations"),
    ]
}

pub(super) fn summarize(roots: &[PathBuf], now: DateTime<Utc>, days: u32) -> SQLiteScan {
    let mut budget = Budget::new();
    let (paths, discovery_complete) = discover_databases(roots, &mut budget);
    if paths.is_empty() && discovery_complete {
        return SQLiteScan::NoDatabases;
    }

    let first_day = now.with_timezone(&Local).date_naive()
        - Duration::days(i64::from(days.clamp(1, 365).saturating_sub(1)));
    let mut complete = discovery_complete && budget.check();
    let mut events = Vec::new();

    for path in &paths {
        if !budget.check() {
            complete = false;
            break;
        }
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

fn discover_databases(roots: &[PathBuf], budget: &mut Budget) -> (Vec<PathBuf>, bool) {
    let mut paths = Vec::new();
    let mut complete = true;

    for root in roots {
        if !budget.check() {
            return (paths, false);
        }
        let resolved_root = match fs::canonicalize(root) {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                complete = false;
                continue;
            }
        };
        match fs::metadata(&resolved_root) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) | Err(_) => {
                complete = false;
                continue;
            }
        }
        let entries = match fs::read_dir(&resolved_root) {
            Ok(entries) => entries,
            Err(_) => {
                complete = false;
                continue;
            }
        };
        for entry in entries {
            if !budget.check() {
                return (paths, false);
            }
            budget.directory_entries += 1;
            if budget.directory_entries > MAX_DIRECTORY_ENTRIES {
                return (paths, false);
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    complete = false;
                    continue;
                }
            };
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                complete = false;
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            let is_db = path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("db"));
            if !is_db {
                continue;
            }
            let resolved = match fs::canonicalize(&path) {
                Ok(path) => path,
                Err(_) => {
                    complete = false;
                    continue;
                }
            };
            match fs::metadata(&resolved) {
                Ok(metadata) if metadata.is_file() => {}
                Ok(_) | Err(_) => {
                    complete = false;
                    continue;
                }
            }
            if paths.len() >= MAX_DATABASES {
                return (paths, false);
            }
            paths.push(resolved);
        }
    }
    paths.sort();
    paths.dedup();
    (paths, complete)
}

fn read_database(path: &Path, budget: &mut Budget) -> rusqlite::Result<(Vec<Event>, bool)> {
    if !budget.check() {
        return Ok((Vec::new(), false));
    }
    let mut conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
    if !supported_schema(&tx, budget)? {
        return Ok((Vec::new(), false));
    }

    let session = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("unknown")
        .to_string();
    let mut statement = tx.prepare(
        "SELECT idx, CASE WHEN typeof(data) = 'blob' THEN length(data) END, CASE WHEN typeof(data) = 'blob' AND length(data) <= ?2 THEN data END FROM main.gen_metadata NOT INDEXED LIMIT ?1",
    )?;
    let row_limit = i64::try_from(MAX_ROWS_PER_DATABASE + 1).unwrap_or(i64::MAX);
    let blob_limit = i64::try_from(MAX_BLOB_BYTES).unwrap_or(i64::MAX);
    let mut query = statement.query(rusqlite::params![row_limit, blob_limit])?;
    let mut database_bytes = 0usize;
    let mut database_rows = 0usize;
    let mut complete = true;
    let mut events = Vec::new();

    while let Some(row) = query.next()? {
        if !budget.check() {
            complete = false;
            break;
        }
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

fn supported_schema(conn: &Connection, budget: &mut Budget) -> rusqlite::Result<bool> {
    let mut statement =
        conn.prepare("SELECT name, type, rootpage FROM main.sqlite_master LIMIT ?1")?;
    let mut rows = statement.query([i64::try_from(MAX_SCHEMA_ENTRIES + 1).unwrap_or(i64::MAX)])?;
    let mut found = false;
    let mut schema_entries = 0usize;
    while let Some(row) = rows.next()? {
        if !budget.check() {
            return Ok(false);
        }
        schema_entries += 1;
        if schema_entries > MAX_SCHEMA_ENTRIES {
            return Ok(false);
        }
        let name: String = row.get(0)?;
        let kind: String = row.get(1)?;
        if !budget.charge_schema_text(&name) || !budget.charge_schema_text(&kind) {
            return Ok(false);
        }
        if !name.eq_ignore_ascii_case("gen_metadata") {
            continue;
        }
        let rootpage: i64 = row.get(2)?;
        if kind != "table" || rootpage <= 0 || found {
            return Ok(false);
        }
        found = true;
    }
    if !found {
        return Ok(false);
    }

    let mut columns = HashSet::new();
    let mut schema_columns = 0usize;
    let mut info = conn.prepare("PRAGMA main.table_xinfo('gen_metadata')")?;
    let mut rows = info.query([])?;
    while let Some(row) = rows.next()? {
        if !budget.check() {
            return Ok(false);
        }
        schema_columns += 1;
        if schema_columns > MAX_SCHEMA_COLUMNS {
            return Ok(false);
        }
        let hidden: i64 = row.get(6)?;
        if hidden != 0 {
            return Ok(false);
        }
        let name: String = row.get(1)?;
        let column_type: String = row.get(2).unwrap_or_default();
        let default_value: Option<String> = row.get(4).ok();
        if !budget.charge_schema_text(&name) || !budget.charge_schema_text(&column_type) {
            return Ok(false);
        }
        if let Some(default_value) = default_value.as_deref()
            && !budget.charge_schema_text(default_value)
        {
            return Ok(false);
        }
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
            summarize(&database_roots(&dir.path().join(".gemini")), Utc::now(), 30),
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
        let SQLiteScan::Summary(summary) =
            summarize(&database_roots(&dir.path().join(".gemini")), Utc::now(), 30)
        else {
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
        let SQLiteScan::Summary(summary) =
            summarize(&database_roots(&dir.path().join(".gemini")), Utc::now(), 30)
        else {
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
        let SQLiteScan::Summary(summary) =
            summarize(&database_roots(&dir.path().join(".gemini")), Utc::now(), 30)
        else {
            panic!("supported database should produce coverage");
        };
        assert_eq!(summary.coverage, LocalHistoryCoverage::Partial);
    }
    #[test]
    fn discovery_allows_exactly_500_databases_but_marks_501_partial() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dbs");
        fs::create_dir_all(&root).unwrap();
        for index in 0..MAX_DATABASES {
            fs::write(root.join(format!("{index:03}.db")), b"").unwrap();
        }
        let mut budget = Budget::new();
        let (paths, complete) = discover_databases(std::slice::from_ref(&root), &mut budget);
        assert_eq!(paths.len(), MAX_DATABASES);
        assert!(complete);

        fs::write(root.join("overflow.db"), b"").unwrap();
        let mut budget = Budget::new();
        let (paths, complete) = discover_databases(std::slice::from_ref(&root), &mut budget);
        assert_eq!(paths.len(), MAX_DATABASES);
        assert!(!complete);
    }

    #[test]
    fn expired_budget_marks_discovery_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dbs");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("one.db"), b"").unwrap();
        let mut budget = Budget::with_deadline(Instant::now());
        let (_, complete) = discover_databases(std::slice::from_ref(&root), &mut budget);
        assert!(!complete);
    }

    #[test]
    fn extra_columns_and_without_rowid_schema_is_supported() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE gen_metadata(idx INTEGER PRIMARY KEY, data BLOB, extra TEXT) WITHOUT ROWID",
            [],
        )
        .unwrap();
        let mut budget = Budget::new();
        assert!(supported_schema(&conn, &mut budget).unwrap());
    }

    #[test]
    fn generated_columns_are_rejected() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE gen_metadata(idx INTEGER, data BLOB, derived TEXT GENERATED ALWAYS AS (idx || 'x') VIRTUAL)",
            [],
        )
        .unwrap();
        let mut budget = Budget::new();
        assert!(!supported_schema(&conn, &mut budget).unwrap());
    }
}
