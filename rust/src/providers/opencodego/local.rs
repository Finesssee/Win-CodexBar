//! OpenCode Go local usage reader (SQLite).
//!
//! Mirrors upstream `OpenCodeGoLocalUsageReader`: sums `opencode-go` assistant
//! message / step-finish costs from the local OpenCode database and maps them
//! onto session ($12 / 5h), weekly ($30), and monthly ($60) windows.

use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Timelike, Utc};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

use crate::core::{ProviderError, ProviderFetchResult, RateWindow, UsageSnapshot};

const FIVE_HOURS_MS: i64 = 5 * 60 * 60 * 1000;
const WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const SESSION_LIMIT_USD: f64 = 12.0;
const WEEKLY_LIMIT_USD: f64 = 30.0;
const MONTHLY_LIMIT_USD: f64 = 60.0;
const PROVIDER_ID: &str = "opencode-go";

const MESSAGE_USAGE_SQL: &str = r#"
SELECT
  CAST(COALESCE(json_extract(data, '$.time.created'), time_created) AS INTEGER) AS createdMs,
  CAST(json_extract(data, '$.cost') AS REAL) AS cost
FROM message
WHERE json_valid(data)
  AND json_extract(data, '$.providerID') = 'opencode-go'
  AND json_extract(data, '$.role') = 'assistant'
  AND json_type(data, '$.cost') IN ('integer', 'real')
"#;

const MESSAGE_AND_PART_USAGE_SQL: &str = r#"
WITH provider_messages AS (
  SELECT
    id AS messageID,
    CAST(COALESCE(json_extract(data, '$.time.created'), time_created) AS INTEGER) AS createdMs,
    CAST(json_extract(data, '$.cost') AS REAL) AS cost,
    json_type(data, '$.cost') IN ('integer', 'real') AS hasCost
  FROM message
  WHERE json_valid(data)
    AND json_extract(data, '$.providerID') = 'opencode-go'
    AND json_extract(data, '$.role') = 'assistant'
)
SELECT
  CAST(COALESCE(json_extract(p.data, '$.time.created'), p.time_created, m.createdMs) AS INTEGER)
    AS createdMs,
  CAST(json_extract(p.data, '$.cost') AS REAL) AS cost
FROM part p
JOIN provider_messages m ON m.messageID = p.message_id
WHERE json_valid(p.data)
  AND json_extract(p.data, '$.type') = 'step-finish'
  AND json_type(p.data, '$.cost') IN ('integer', 'real')
UNION ALL
SELECT createdMs, cost
FROM provider_messages m
WHERE hasCost
  AND NOT EXISTS (
    SELECT 1
    FROM part p
    WHERE p.message_id = m.messageID
      AND json_valid(p.data)
      AND json_extract(p.data, '$.type') = 'step-finish'
      AND json_type(p.data, '$.cost') IN ('integer', 'real')
  )
"#;

#[derive(Debug, Clone, Copy)]
struct UsageRow {
    created_ms: i64,
    cost: f64,
}

#[derive(Debug, Clone)]
pub struct LocalUsageSnapshot {
    pub rolling_usage_percent: f64,
    pub weekly_usage_percent: f64,
    pub monthly_usage_percent: f64,
    pub rolling_reset_in_sec: i64,
    pub weekly_reset_in_sec: i64,
    pub monthly_reset_in_sec: i64,
}

impl LocalUsageSnapshot {
    pub fn to_fetch_result(&self) -> ProviderFetchResult {
        let now = Utc::now();
        let primary = RateWindow::with_details(
            self.rolling_usage_percent,
            Some(300),
            Some(now + Duration::seconds(self.rolling_reset_in_sec)),
            None,
        );
        let mut snap = UsageSnapshot::new(primary).with_login_method("OpenCode Go");
        snap = snap.with_secondary(RateWindow::with_details(
            self.weekly_usage_percent,
            Some(10080),
            Some(now + Duration::seconds(self.weekly_reset_in_sec)),
            None,
        ));
        snap = snap.with_tertiary(RateWindow::with_details(
            self.monthly_usage_percent,
            Some(43200),
            Some(now + Duration::seconds(self.monthly_reset_in_sec)),
            None,
        ));
        ProviderFetchResult::new(snap, "local")
    }
}

/// Candidate (auth.json, opencode.db) pairs for local OpenCode installs.
pub fn candidate_paths() -> Vec<(PathBuf, PathBuf)> {
    let mut out = Vec::new();

    if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
        let base = PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("opencode");
        out.push((base.join("auth.json"), base.join("opencode.db")));
    }

    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let base = PathBuf::from(local).join("opencode");
        out.push((base.join("auth.json"), base.join("opencode.db")));
    }

    if let Some(home) = dirs::home_dir() {
        let base = home.join(".local").join("share").join("opencode");
        let pair = (base.join("auth.json"), base.join("opencode.db"));
        if !out.iter().any(|existing| existing.1 == pair.1) {
            out.push(pair);
        }
    }

    out
}

pub fn fetch_local_usage(now: DateTime<Utc>) -> Result<LocalUsageSnapshot, ProviderError> {
    let mut last_err: Option<ProviderError> = None;
    for (auth, db) in candidate_paths() {
        match fetch_from_paths(&auth, &db, now) {
            Ok(snap) => return Ok(snap),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| {
        ProviderError::NotInstalled(
            "OpenCode Go not detected. Log in with OpenCode Go or use it locally first.".into(),
        )
    }))
}

pub fn fetch_from_paths(
    auth_path: &Path,
    db_path: &Path,
    now: DateTime<Utc>,
) -> Result<LocalUsageSnapshot, ProviderError> {
    let has_auth = has_auth_key(auth_path);
    if !db_path.exists() {
        return Err(if has_auth {
            ProviderError::Other(
                "OpenCode Go local usage history is unavailable: database not found".into(),
            )
        } else {
            ProviderError::NotInstalled(
                "OpenCode Go not detected. Log in with OpenCode Go or use it locally first.".into(),
            )
        });
    }

    let rows = read_rows(db_path)?;
    if !has_auth && rows.is_empty() {
        return Err(ProviderError::NotInstalled(
            "OpenCode Go not detected. Log in with OpenCode Go or use it locally first.".into(),
        ));
    }
    if rows.is_empty() {
        return Err(ProviderError::Other(
            "OpenCode Go local usage history is unavailable: no local usage rows".into(),
        ));
    }

    Ok(snapshot_from_rows(&rows, now))
}

fn has_auth_key(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    value
        .get(PROVIDER_ID)
        .and_then(|entry| entry.get("key"))
        .and_then(|key| key.as_str())
        .is_some_and(|key| !key.trim().is_empty())
}

fn read_rows(db_path: &Path) -> Result<Vec<UsageRow>, ProviderError> {
    let conn = open_readonly_connection(db_path)?;
    conn.busy_timeout(std::time::Duration::from_millis(250))
        .map_err(|e| {
            ProviderError::Other(format!("SQLite error reading OpenCode Go usage: {e}"))
        })?;

    let sql = if has_table(&conn, "part") {
        MESSAGE_AND_PART_USAGE_SQL
    } else {
        MESSAGE_USAGE_SQL
    };

    let mut stmt = conn.prepare(sql).map_err(|e| {
        ProviderError::Other(format!("SQLite error reading OpenCode Go usage: {e}"))
    })?;
    let rows = stmt
        .query_map([], |row| {
            Ok(UsageRow {
                created_ms: row.get::<_, i64>(0)?,
                cost: row.get::<_, f64>(1)?,
            })
        })
        .map_err(|e| {
            ProviderError::Other(format!("SQLite error reading OpenCode Go usage: {e}"))
        })?;

    let mut out = Vec::new();
    for row in rows {
        let row = row.map_err(|e| {
            ProviderError::Other(format!("SQLite error reading OpenCode Go usage: {e}"))
        })?;
        if row.created_ms > 0 && row.cost.is_finite() && row.cost >= 0.0 {
            out.push(row);
        }
    }
    Ok(out)
}

/// Open a read-only connection without creating `-wal`/`-shm` sidecars for idle
/// WAL-mode databases (upstream #2544).
fn open_readonly_connection(db_path: &Path) -> Result<Connection, ProviderError> {
    let map_err = |e: rusqlite::Error| {
        ProviderError::Other(format!("SQLite error reading OpenCode Go usage: {e}"))
    };

    // Prefer immutable URI when sidecars are absent so a clean WAL shutdown
    // (header still WAL, no -wal/-shm) does not recreate them on open.
    if wal_sidecars_missing(db_path)
        && let Ok(conn) = open_immutable(db_path)
    {
        return Ok(conn);
    }

    match Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(conn) => Ok(conn),
        Err(err) if is_cant_open(&err) && wal_sidecars_missing(db_path) => {
            open_immutable(db_path).map_err(map_err)
        }
        Err(err) => Err(map_err(err)),
    }
}

fn open_immutable(db_path: &Path) -> Result<Connection, rusqlite::Error> {
    let uri = sqlite_immutable_uri(db_path);
    Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
}

fn sqlite_immutable_uri(db_path: &Path) -> String {
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

fn wal_sidecars_missing(db_path: &Path) -> bool {
    let wal = sidecar_path(db_path, "-wal");
    let shm = sidecar_path(db_path, "-shm");
    !wal.exists() && !shm.exists()
}

fn sidecar_path(db_path: &Path, suffix: &str) -> PathBuf {
    let mut s = db_path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

fn is_cant_open(err: &rusqlite::Error) -> bool {
    matches!(
        err.sqlite_error_code(),
        Some(rusqlite::ErrorCode::CannotOpen)
    ) || err.to_string().to_ascii_lowercase().contains("unable to open")
}

fn has_table(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1",
        [name],
        |_| Ok(()),
    )
    .is_ok()
}

fn snapshot_from_rows(rows: &[UsageRow], now: DateTime<Utc>) -> LocalUsageSnapshot {
    let now_ms = now.timestamp_millis();
    let session_start = now_ms - FIVE_HOURS_MS;
    let week_start_ms = start_of_utc_iso_week_ms(now);
    let week_end_ms = week_start_ms + WEEK_MS;
    let earliest_ms = rows.iter().map(|r| r.created_ms).min();
    let (month_start_ms, month_end_ms) = month_bounds_ms(now, earliest_ms);

    let mut session_cost = 0.0;
    let mut weekly_cost = 0.0;
    let mut monthly_cost = 0.0;
    let mut oldest_session_ms: Option<i64> = None;

    for row in rows {
        if row.created_ms >= session_start && row.created_ms < now_ms {
            session_cost += row.cost;
            oldest_session_ms = Some(match oldest_session_ms {
                Some(prev) => prev.min(row.created_ms),
                None => row.created_ms,
            });
        }
        if row.created_ms >= week_start_ms && row.created_ms < week_end_ms {
            weekly_cost += row.cost;
        }
        if row.created_ms >= month_start_ms && row.created_ms < month_end_ms {
            monthly_cost += row.cost;
        }
    }

    let oldest = oldest_session_ms.unwrap_or(now_ms);
    let rolling_reset_in_sec = ((oldest + FIVE_HOURS_MS - now_ms) / 1000).max(0);

    LocalUsageSnapshot {
        rolling_usage_percent: percent(session_cost, SESSION_LIMIT_USD),
        weekly_usage_percent: percent(weekly_cost, WEEKLY_LIMIT_USD),
        monthly_usage_percent: percent(monthly_cost, MONTHLY_LIMIT_USD),
        rolling_reset_in_sec,
        weekly_reset_in_sec: ((week_end_ms - now_ms) / 1000).max(0),
        monthly_reset_in_sec: ((month_end_ms - now_ms) / 1000).max(0),
    }
}

fn percent(used: f64, limit: f64) -> f64 {
    if !used.is_finite() || limit <= 0.0 {
        return 0.0;
    }
    let value = (used / limit * 100.0).clamp(0.0, 100.0);
    (value * 10.0).round() / 10.0
}

/// ISO week start (Monday 00:00 UTC), matching upstream calendar settings.
fn start_of_utc_iso_week_ms(now: DateTime<Utc>) -> i64 {
    let date = now.date_naive();
    let days_from_monday = date.weekday().num_days_from_monday() as i64;
    let monday = date - Duration::days(days_from_monday);
    Utc.from_utc_datetime(&monday.and_hms_opt(0, 0, 0).unwrap_or_default())
        .timestamp_millis()
}

fn month_bounds_ms(now: DateTime<Utc>, anchor_ms: Option<i64>) -> (i64, i64) {
    let Some(anchor_ms) = anchor_ms else {
        let start = NaiveDate::from_ymd_opt(now.year(), now.month(), 1)
            .unwrap_or_else(|| now.date_naive())
            .and_hms_opt(0, 0, 0)
            .unwrap_or_default();
        let start_dt = Utc.from_utc_datetime(&start);
        let end_dt = if now.month() == 12 {
            Utc.with_ymd_and_hms(now.year() + 1, 1, 1, 0, 0, 0)
                .single()
                .unwrap_or(start_dt)
        } else {
            Utc.with_ymd_and_hms(now.year(), now.month() + 1, 1, 0, 0, 0)
                .single()
                .unwrap_or(start_dt)
        };
        return (start_dt.timestamp_millis(), end_dt.timestamp_millis());
    };

    let anchor = DateTime::<Utc>::from_timestamp_millis(anchor_ms).unwrap_or(now);
    let mut year = now.year();
    let mut month = now.month();
    let mut start = anchored_month(year, month, &anchor);
    if start > now {
        if month == 1 {
            year -= 1;
            month = 12;
        } else {
            month -= 1;
        }
        start = anchored_month(year, month, &anchor);
    }
    let (end_year, end_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let end = anchored_month(end_year, end_month, &anchor);
    (start.timestamp_millis(), end.timestamp_millis())
}

fn anchored_month(year: i32, month: u32, anchor: &DateTime<Utc>) -> DateTime<Utc> {
    let day = anchor.day();
    let (hour, min, sec, nano) = (
        anchor.hour(),
        anchor.minute(),
        anchor.second(),
        anchor.nanosecond(),
    );
    if let Some(date) = NaiveDate::from_ymd_opt(year, month, day)
        && let Some(ndt) = date.and_hms_nano_opt(hour, min, sec, nano)
    {
        return Utc.from_utc_datetime(&ndt);
    }
    // Clamp to last day of month when anchor day overflows (e.g. 31 → Feb).
    let last_day = NaiveDate::from_ymd_opt(year, month, 1)
        .map(|d| {
            if month == 12 {
                NaiveDate::from_ymd_opt(year + 1, 1, 1)
            } else {
                NaiveDate::from_ymd_opt(year, month + 1, 1)
            }
            .unwrap_or(d)
                - Duration::days(1)
        })
        .map(|d| d.day())
        .unwrap_or(28);
    let date = NaiveDate::from_ymd_opt(year, month, last_day).unwrap_or_else(|| {
        NaiveDate::from_ymd_opt(year, month, 1).unwrap_or_else(|| Utc::now().date_naive())
    });
    let ndt = date
        .and_hms_nano_opt(hour, min, sec, nano)
        .or_else(|| date.and_hms_opt(0, 0, 0))
        .unwrap_or_default();
    Utc.from_utc_datetime(&ndt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Weekday;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db_path(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("opencodego-local-{label}-{nanos}.db"))
    }

    fn write_message_db(path: &Path, rows: &[(i64, f64)]) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE message (
                id TEXT PRIMARY KEY,
                data TEXT,
                time_created INTEGER
            );",
        )
        .unwrap();
        for (i, (created_ms, cost)) in rows.iter().enumerate() {
            let data = format!(
                r#"{{"providerID":"opencode-go","role":"assistant","cost":{cost},"time":{{"created":{created_ms}}}}}"#
            );
            conn.execute(
                "INSERT INTO message (id, data, time_created) VALUES (?1, ?2, ?3)",
                rusqlite::params![format!("m{i}"), data, created_ms],
            )
            .unwrap();
        }
    }

    #[test]
    fn not_detected_without_db_or_auth() {
        let dir = std::env::temp_dir().join(format!(
            "opencodego-missing-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let auth = dir.join("auth.json");
        let db = dir.join("opencode.db");
        let err = fetch_from_paths(&auth, &db, Utc::now()).unwrap_err();
        assert!(matches!(err, ProviderError::NotInstalled(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sums_session_weekly_monthly_costs() {
        let db = temp_db_path("sums");
        let now = Utc.with_ymd_and_hms(2026, 3, 18, 12, 0, 0).unwrap(); // Wednesday
        let now_ms = now.timestamp_millis();
        // $6 in the rolling 5h window → 50% of $12
        // $15 in ISO week → 50% of $30
        // $30 in anchored month → 50% of $60
        let session_ms = now_ms - 60_000;
        let week_ms = start_of_utc_iso_week_ms(now) + 3_600_000;
        let month_anchor_ms = now_ms - 10 * 24 * 60 * 60 * 1000;
        write_message_db(
            &db,
            &[
                (session_ms, 6.0),
                (week_ms, 9.0), // plus session = 15 in week if session also in week
                (month_anchor_ms, 15.0),
            ],
        );

        // auth present so empty-rows path is not used; auth not required when rows exist
        let auth = db.with_extension("auth.json");
        let _ = std::fs::write(&auth, r#"{"opencode-go":{"key":"test-key"}}"#);

        let snap = fetch_from_paths(&auth, &db, now).unwrap();
        assert!((snap.rolling_usage_percent - 50.0).abs() < 0.05, "{snap:?}");
        // session 6 + week-only 9 = 15 → 50%
        assert!((snap.weekly_usage_percent - 50.0).abs() < 0.05, "{snap:?}");
        // session 6 + week 9 + month 15 = 30 → 50%
        assert!((snap.monthly_usage_percent - 50.0).abs() < 0.05, "{snap:?}");

        let _ = std::fs::remove_file(&db);
        let _ = std::fs::remove_file(&auth);
    }

    #[test]
    fn prefers_step_finish_parts_when_present() {
        let db = temp_db_path("parts");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE message (id TEXT PRIMARY KEY, data TEXT, time_created INTEGER);
             CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, data TEXT, time_created INTEGER);",
        )
        .unwrap();
        let now = Utc.with_ymd_and_hms(2026, 3, 18, 12, 0, 0).unwrap();
        let created = now.timestamp_millis() - 1_000;
        // Message cost would be $12 (100%), but step-finish parts sum to $3 (25%).
        conn.execute(
            "INSERT INTO message (id, data, time_created) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                "m1",
                format!(
                    r#"{{"providerID":"opencode-go","role":"assistant","cost":12,"time":{{"created":{created}}}}}"#
                ),
                created
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, data, time_created) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                "p1",
                "m1",
                format!(r#"{{"type":"step-finish","cost":3,"time":{{"created":{created}}}}}"#),
                created
            ],
        )
        .unwrap();
        drop(conn);

        let auth = db.with_extension("auth.json");
        let _ = std::fs::write(&auth, r#"{"opencode-go":{"key":"k"}}"#);
        let snap = fetch_from_paths(&auth, &db, now).unwrap();
        assert!(
            (snap.rolling_usage_percent - 25.0).abs() < 0.05,
            "expected step-finish cost only, got {snap:?}"
        );
        let _ = std::fs::remove_file(&db);
        let _ = std::fs::remove_file(&auth);
    }

    #[test]
    fn percent_rounds_to_one_decimal() {
        assert!((percent(1.0, 12.0) - 8.3).abs() < 0.05);
        assert_eq!(percent(0.0, 12.0), 0.0);
        assert_eq!(percent(f64::NAN, 12.0), 0.0);
    }

    #[test]
    fn iso_week_starts_monday_utc() {
        // 2026-03-18 is a Wednesday; week start should be 2026-03-16 00:00 UTC.
        let wed = Utc.with_ymd_and_hms(2026, 3, 18, 15, 0, 0).unwrap();
        let start = start_of_utc_iso_week_ms(wed);
        let expected = Utc
            .with_ymd_and_hms(2026, 3, 16, 0, 0, 0)
            .unwrap()
            .timestamp_millis();
        assert_eq!(start, expected);
        assert_eq!(wed.weekday(), Weekday::Wed);
    }

    #[test]
    fn idle_wal_mode_read_creates_no_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        let auth = dir.path().join("auth.json");
        std::fs::write(&auth, r#"{"opencode-go":{"key":"k"}}"#).unwrap();

        // Build a WAL-mode DB, insert a row, leave journal_mode=WAL, then drop
        // any writer-created sidecars so the main file is an idle WAL header.
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
            conn.execute_batch(
                "CREATE TABLE message (
                    id TEXT PRIMARY KEY,
                    data TEXT,
                    time_created INTEGER
                );",
            )
            .unwrap();
            let now = Utc::now();
            let created = now.timestamp_millis() - 1_000;
            let data = format!(
                r#"{{"providerID":"opencode-go","role":"assistant","cost":3,"time":{{"created":{created}}}}}"#
            );
            conn.execute(
                "INSERT INTO message (id, data, time_created) VALUES ('m1', ?1, ?2)",
                rusqlite::params![data, created],
            )
            .unwrap();
            // Truncate empties WAL content before close; journal_mode stays WAL.
            let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
            drop(conn);
        }

        // Ensure idle-WAL case: header says WAL, no live sidecars.
        let wal = sidecar_path(&db, "-wal");
        let shm = sidecar_path(&db, "-shm");
        // Prefer rename-away over delete if OS still holds handles.
        for side in [&wal, &shm] {
            if side.exists() {
                let parked = side.with_extension("parked");
                let _ = std::fs::rename(side, parked);
            }
        }
        assert!(!wal.exists(), "precondition: no -wal");
        assert!(!shm.exists(), "precondition: no -shm");

        let snap = fetch_from_paths(&auth, &db, Utc::now()).expect("read idle WAL db");
        assert!(snap.rolling_usage_percent > 0.0, "{snap:?}");

        assert!(
            !wal.exists() && !shm.exists(),
            "reader must not create -wal/-shm sidecars"
        );
    }
}
