//! OpenCode Go local usage reader (SQLite).
//!
//! Mirrors upstream `OpenCodeGoLocalUsageReader`: sums `opencode-go` assistant
//! message / step-finish costs from the local OpenCode database and maps them
//! onto session ($12 / 5h), weekly ($30), and monthly ($60) windows.

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone, Timelike, Utc};
use rusqlite::Connection;
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
  CAST(json_extract(data, '$.cost') AS REAL) AS cost,
  1 AS requestCount,
  COALESCE(json_extract(data, '$.modelID'), '') AS modelID
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
    json_type(data, '$.cost') IN ('integer', 'real') AS hasCost,
    COALESCE(json_extract(data, '$.modelID'), '') AS modelID
  FROM message
  WHERE json_valid(data)
    AND json_extract(data, '$.providerID') = 'opencode-go'
    AND json_extract(data, '$.role') = 'assistant'
)
SELECT
  CAST(COALESCE(json_extract(p.data, '$.time.created'), p.time_created, m.createdMs) AS INTEGER)
    AS createdMs,
  CAST(json_extract(p.data, '$.cost') AS REAL) AS cost,
  1 AS requestCount,
  m.modelID AS modelID
FROM part p
JOIN provider_messages m ON m.messageID = p.message_id
WHERE json_valid(p.data)
  AND json_extract(p.data, '$.type') = 'step-finish'
  AND json_type(p.data, '$.cost') IN ('integer', 'real')
UNION ALL
SELECT createdMs, cost, 1 AS requestCount, modelID
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

#[derive(Debug, Clone)]
pub(crate) struct UsageRow {
    created_ms: i64,
    cost: f64,
    /// One provider invocation per step-finish part; message-only databases fall back to one.
    request_count: u32,
    /// The underlying model behind the `opencode-go` Zen proxy; empty when unattributed.
    model: String,
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
        let monthly_reset = now + Duration::seconds(self.monthly_reset_in_sec);
        snap = snap.with_tertiary(RateWindow::with_details(
            self.monthly_usage_percent,
            RateWindow::monthly_window_minutes(Some(monthly_reset)).or(Some(43200)),
            Some(monthly_reset),
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
                request_count: row.get::<_, i64>(2).map(|n| n.max(1) as u32).unwrap_or(1),
                model: row.get::<_, String>(3).unwrap_or_default(),
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
    crate::core::open_readonly_sqlite_connection(db_path, std::time::Duration::from_millis(250))
        .map_err(|e| ProviderError::Other(format!("SQLite error reading OpenCode Go usage: {e}")))
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

/// Bucket label for rows whose local `modelID` is missing or blank (upstream #2649).
const UNKNOWN_MODEL_NAME: &str = "unknown";

/// One (day, model) cost bucket for the daily per-model breakdown (upstream #2649).
///
/// Mirrors `CostUsageDailyReport.ModelBreakdown` plus the day key, so the shared
/// cost-history chart can render OpenCode Go the same way it renders Codex/Claude
/// without a bespoke chart surface. Entries are sorted by `(day_key, model)`.
#[derive(Debug, Clone, PartialEq)]
pub struct DailyModelCost {
    /// `yyyy-MM-dd` local calendar day (matches Codex/Claude cost-history keying).
    pub day_key: String,
    /// Trimmed model id, or `UNKNOWN_MODEL_NAME` when the row had none.
    pub model: String,
    /// Cost in USD accumulated for this (day, model) bucket.
    pub cost: f64,
    /// Number of provider invocations (step-finish parts, or one per message).
    pub request_count: u32,
}

/// Provider-local cost summary reusing the shared `CostSummary` fields the chart
/// already consumes (`total_cost_usd`, `by_model`, `period_start/end`). Built
/// from the same local rows as the daily breakdown so the two surfaces agree.
#[derive(Debug, Clone, Default)]
pub struct ModelCostSummary {
    pub total_cost_usd: f64,
    pub by_model: std::collections::HashMap<String, f64>,
    pub request_count: u32,
    pub period_start: Option<NaiveDate>,
    pub period_end: Option<NaiveDate>,
}

/// Local calendar-day key (`yyyy-MM-dd`) for a UTC millisecond timestamp,
/// matching how Codex/Claude cost history is keyed.
fn day_key_local(ms: i64) -> Option<String> {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.date_naive().format("%Y-%m-%d").to_string())
}

/// Local "today" derived from a UTC instant, so the day window is deterministic
/// under test rather than snapping to wall-clock `Local::now()`.
fn local_today_from_utc(now: DateTime<Utc>) -> NaiveDate {
    Local.from_utc_datetime(&now.naive_utc()).date_naive()
}

/// Group rows into `(day, model)` cost buckets (upstream #2649).
///
/// Rows outside the `[since, now]` window are dropped; model ids are trimmed and
/// blanks collapse to `UNKNOWN_MODEL_NAME`. The result is sorted by
/// `(day_key, model)` for deterministic ordering.
pub fn daily_model_costs(
    rows: &[UsageRow],
    now: DateTime<Utc>,
    history_days: u32,
) -> Vec<DailyModelCost> {
    let clamped = history_days.clamp(1, 365);
    let today = local_today_from_utc(now);
    let since = today - Duration::days(clamped as i64 - 1);
    let since_ms = Local
        .from_local_datetime(&since.and_hms_opt(0, 0, 0).unwrap_or_default())
        .single()
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(0);
    let now_ms = now.timestamp_millis();

    let mut by_day_model: std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, (f64, u32)>,
    > = std::collections::BTreeMap::new();
    for row in rows {
        if row.created_ms < since_ms || row.created_ms > now_ms {
            continue;
        }
        let Some(key) = day_key_local(row.created_ms) else {
            continue;
        };
        let trimmed = row.model.trim();
        let model = if trimmed.is_empty() {
            UNKNOWN_MODEL_NAME
        } else {
            trimmed
        };
        let entry = by_day_model.entry(key).or_default();
        let bucket = entry.entry(model.to_string()).or_insert((0.0, 0));
        bucket.0 += row.cost;
        bucket.1 = bucket.1.saturating_add(row.request_count);
    }

    let mut out = Vec::new();
    for (day_key, models) in by_day_model {
        for (model, (cost, request_count)) in models {
            out.push(DailyModelCost {
                day_key: day_key.clone(),
                model,
                cost,
                request_count,
            });
        }
    }
    out
}

/// Build the provider-local cost summary for the last `days` days.
pub fn model_cost_summary_from_rows(
    rows: &[UsageRow],
    now: DateTime<Utc>,
    days: u32,
) -> ModelCostSummary {
    let clamped = days.clamp(1, 365);
    let today = local_today_from_utc(now);
    let since = today - Duration::days(clamped as i64 - 1);
    let since_ms = Local
        .from_local_datetime(&since.and_hms_opt(0, 0, 0).unwrap_or_default())
        .single()
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(0);
    let now_ms = now.timestamp_millis();

    let mut total = 0.0;
    let mut request_count = 0u32;
    let mut by_model: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    let mut earliest: Option<NaiveDate> = None;
    let mut latest: Option<NaiveDate> = None;
    for row in rows {
        if row.created_ms < since_ms || row.created_ms > now_ms {
            continue;
        }
        total += row.cost;
        request_count = request_count.saturating_add(row.request_count);
        let trimmed = row.model.trim();
        let model = if trimmed.is_empty() {
            UNKNOWN_MODEL_NAME
        } else {
            trimmed
        };
        *by_model.entry(model.to_string()).or_insert(0.0) += row.cost;
        if let Some(day) = day_key_local(row.created_ms)
            .and_then(|k| NaiveDate::parse_from_str(&k, "%Y-%m-%d").ok())
        {
            earliest = Some(earliest.map(|e| e.min(day)).unwrap_or(day));
            latest = Some(latest.map(|l| l.max(day)).unwrap_or(day));
        }
    }
    ModelCostSummary {
        total_cost_usd: total,
        by_model,
        request_count,
        period_start: earliest,
        period_end: latest,
    }
}

/// Per-day cost series (`yyyy-MM-dd`, cost USD) for the shared cost-history chart,
/// summed across all models. Reads the first available local OpenCode install.
/// Empty when no install is detected.
pub fn daily_cost_series(now: DateTime<Utc>, history_days: u32) -> Vec<(String, f64)> {
    let Some(rows) = read_available_rows() else {
        return Vec::new();
    };
    let buckets = daily_model_costs(&rows, now, history_days);
    let mut by_day: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();
    for b in &buckets {
        *by_day.entry(b.day_key.clone()).or_insert(0.0) += b.cost;
    }
    by_day.into_iter().collect()
}

/// Provider-local cost summary for the chart's local-usage panel. `None` when no
/// local OpenCode install is detected.
pub fn model_cost_summary_scan(now: DateTime<Utc>, days: u32) -> Option<ModelCostSummary> {
    let rows = read_available_rows()?;
    Some(model_cost_summary_from_rows(&rows, now, days))
}

/// Read usage rows from the first candidate install that yields any. Returns
/// `None` when no install is reachable (auth+db both absent) rather than
/// propagating `NotInstalled`, since the cost surfaces treat "no data" as empty.
fn read_available_rows() -> Option<Vec<UsageRow>> {
    for (auth, db) in candidate_paths() {
        if !db.exists() {
            continue;
        }
        match read_rows(&db) {
            Ok(rows) if !rows.is_empty() || has_auth_key(&auth) => return Some(rows),
            Ok(_) => continue,
            Err(_) => continue,
        }
    }
    None
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

    /// Build a message-only DB with optional per-row `modelID` (upstream #2649 fixtures).
    fn write_message_db_with_model(path: &Path, rows: &[(i64, f64, Option<&str>)]) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE message (
                id TEXT PRIMARY KEY,
                data TEXT,
                time_created INTEGER
            );",
        )
        .unwrap();
        for (i, (created_ms, cost, model)) in rows.iter().enumerate() {
            let model_json = match model {
                Some(m) => format!(r#","modelID":"{m}""#),
                None => String::new(),
            };
            let data = format!(
                r#"{{"providerID":"opencode-go","role":"assistant","cost":{cost}{model_json},"time":{{"created":{created_ms}}}}}"#
            );
            conn.execute(
                "INSERT INTO message (id, data, time_created) VALUES (?1, ?2, ?3)",
                rusqlite::params![format!("m{i}"), data, created_ms],
            )
            .unwrap();
        }
    }

    /// Insert one assistant message and return its id, optionally with a modelID.
    fn insert_message(
        conn: &Connection,
        id: &str,
        created_ms: i64,
        cost: Option<f64>,
        model: Option<&str>,
    ) {
        let cost_json = match cost {
            Some(c) => format!(r#","cost":{c}"#),
            None => String::new(),
        };
        let model_json = match model {
            Some(m) => format!(r#","modelID":"{m}""#),
            None => String::new(),
        };
        let data = format!(
            r#"{{"providerID":"opencode-go","role":"assistant"{cost_json}{model_json},"time":{{"created":{created_ms}}}}}"#
        );
        conn.execute(
            "INSERT INTO message (id, data, time_created) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, data, created_ms],
        )
        .unwrap();
    }

    /// Insert a step-finish part carrying a cost, attached to `message_id`.
    fn insert_step_finish_part(
        conn: &Connection,
        id: &str,
        message_id: &str,
        created_ms: i64,
        cost: f64,
    ) {
        let data =
            format!(r#"{{"type":"step-finish","cost":{cost},"time":{{"created":{created_ms}}}}}"#);
        conn.execute(
            "INSERT INTO part (id, message_id, data, time_created) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, message_id, data, created_ms],
        )
        .unwrap();
    }

    fn iso_ms(iso: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(iso)
            .unwrap()
            .timestamp_millis()
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
        let wal = crate::core::sqlite_sidecar_path(&db, "-wal");
        let shm = crate::core::sqlite_sidecar_path(&db, "-shm");
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

    // ---- A14: per-model daily cost breakdown (upstream #2649) -------------

    fn a14_now() -> DateTime<Utc> {
        Utc.timestamp_opt(1_772_798_400, 0).unwrap()
    }

    fn a14_now_afternoon() -> DateTime<Utc> {
        Utc.timestamp_opt(1_772_798_400 + 4 * 3600, 0).unwrap()
    }

    #[test]
    fn daily_entries_group_cost_by_model_within_a_day() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        let now = a14_now_afternoon();
        write_message_db_with_model(
            &db,
            &[
                (
                    iso_ms("2026-03-06T11:00:00.000Z"),
                    3.0,
                    Some("claude-sonnet-4-5"),
                ),
                (
                    iso_ms("2026-03-06T12:00:00.000Z"),
                    2.0,
                    Some("gpt-5.1-codex"),
                ),
                (
                    iso_ms("2026-03-06T13:00:00.000Z"),
                    1.0,
                    Some("claude-sonnet-4-5"),
                ),
            ],
        );
        let rows = read_rows(&db).unwrap();
        let buckets = daily_model_costs(&rows, now, 30);

        // Day key is local-calendar; assert on tz-independent model aggregation.
        let total: f64 = buckets.iter().map(|b| b.cost).sum();
        assert!((total - 6.0).abs() < 1e-6, "total {total}");
        assert_eq!(buckets.iter().map(|b| b.request_count).sum::<u32>(), 3);
        let by_model: std::collections::HashMap<&str, f64> =
            buckets.iter().map(|b| (b.model.as_str(), b.cost)).collect();
        assert!((by_model["claude-sonnet-4-5"] - 4.0).abs() < 1e-6);
        assert!((by_model["gpt-5.1-codex"] - 2.0).abs() < 1e-6);
    }

    #[test]
    fn step_finish_parts_inherit_their_model_from_the_parent_message() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE message (id TEXT PRIMARY KEY, data TEXT, time_created INTEGER);
             CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, data TEXT, time_created INTEGER);",
        )
        .unwrap();
        let created = iso_ms("2026-03-06T11:00:00.000Z");
        insert_message(&conn, "m1", created, None, Some("grok-code-fast-1"));
        insert_step_finish_part(&conn, "p1", "m1", created, 3.0);
        drop(conn);

        let rows = read_rows(&db).unwrap();
        let buckets = daily_model_costs(&rows, a14_now(), 30);
        assert_eq!(buckets.len(), 1, "{buckets:?}");
        assert_eq!(buckets[0].model, "grok-code-fast-1");
        assert!((buckets[0].cost - 3.0).abs() < 1e-6);
    }

    #[test]
    fn messages_without_a_model_fall_back_to_the_unknown_bucket() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        write_message_db_with_model(&db, &[(iso_ms("2026-03-06T11:00:00.000Z"), 4.0, None)]);
        let rows = read_rows(&db).unwrap();
        let buckets = daily_model_costs(&rows, a14_now(), 30);
        assert_eq!(buckets.len(), 1, "{buckets:?}");
        assert_eq!(buckets[0].model, UNKNOWN_MODEL_NAME);
        assert!((buckets[0].cost - 4.0).abs() < 1e-6);
    }

    #[test]
    fn whitespace_only_model_ids_fall_back_to_the_unknown_bucket() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        write_message_db_with_model(
            &db,
            &[(iso_ms("2026-03-06T11:00:00.000Z"), 5.0, Some("   "))],
        );
        let rows = read_rows(&db).unwrap();
        let buckets = daily_model_costs(&rows, a14_now(), 30);
        assert_eq!(buckets.len(), 1, "{buckets:?}");
        assert_eq!(buckets[0].model, UNKNOWN_MODEL_NAME);
        assert!((buckets[0].cost - 5.0).abs() < 1e-6);
    }

    #[test]
    fn model_ids_with_incidental_whitespace_merge_with_the_trimmed_bucket() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        write_message_db_with_model(
            &db,
            &[
                (
                    iso_ms("2026-03-06T11:00:00.000Z"),
                    2.0,
                    Some("claude-sonnet-4-5"),
                ),
                (
                    iso_ms("2026-03-06T12:00:00.000Z"),
                    3.0,
                    Some("  claude-sonnet-4-5  "),
                ),
            ],
        );
        let rows = read_rows(&db).unwrap();
        let buckets = daily_model_costs(&rows, a14_now_afternoon(), 30);
        assert_eq!(buckets.len(), 1, "{buckets:?}");
        assert_eq!(buckets[0].model, "claude-sonnet-4-5");
        assert!((buckets[0].cost - 5.0).abs() < 1e-6);
        assert_eq!(buckets[0].request_count, 2);
    }

    #[test]
    fn multiple_days_bucket_separately_and_sort_deterministically() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        write_message_db_with_model(
            &db,
            &[
                (iso_ms("2026-03-05T11:00:00.000Z"), 1.0, Some("a")),
                (iso_ms("2026-03-06T11:00:00.000Z"), 2.0, Some("b")),
                (iso_ms("2026-03-07T11:00:00.000Z"), 3.0, Some("a")),
            ],
        );
        let rows = read_rows(&db).unwrap();
        let buckets = daily_model_costs(&rows, a14_now_afternoon(), 30);
        assert!(
            buckets
                .windows(2)
                .all(|w| (w[0].day_key.as_str(), w[0].model.as_str())
                    <= (w[1].day_key.as_str(), w[1].model.as_str())),
            "not sorted: {buckets:?}"
        );
        assert!(
            buckets
                .iter()
                .map(|b| b.day_key.as_str())
                .collect::<std::collections::HashSet<_>>()
                .len()
                >= 2
        );
    }

    #[test]
    fn zero_cost_rows_are_kept_and_aggregated() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        write_message_db_with_model(
            &db,
            &[
                (iso_ms("2026-03-06T11:00:00.000Z"), 0.0, Some("a")),
                (iso_ms("2026-03-06T12:00:00.000Z"), 4.0, Some("a")),
            ],
        );
        let rows = read_rows(&db).unwrap();
        let buckets = daily_model_costs(&rows, a14_now_afternoon(), 30);
        assert_eq!(buckets.len(), 1, "{buckets:?}");
        assert!((buckets[0].cost - 4.0).abs() < 1e-6);
        assert_eq!(buckets[0].request_count, 2);
    }

    #[test]
    fn malformed_rows_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE message (id TEXT PRIMARY KEY, data TEXT, time_created INTEGER);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, data, time_created) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                "m1",
                r#"{"providerID":"opencode-go","role":"user","cost":9,"time":{"created":1772798400000}}"#,
                1772798400000i64
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, data, time_created) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                "m2",
                r#"{"providerID":"opencode-go","role":"assistant","cost":null,"modelID":"x","time":{"created":1772798400000}}"#,
                1772798400000i64
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, data, time_created) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                "m3",
                r#"{"providerID":"opencode-go","role":"assistant","cost":7,"modelID":"good","time":{"created":1772798400000}}"#,
                1772798400000i64
            ],
        )
        .unwrap();
        drop(conn);

        let rows = read_rows(&db).unwrap();
        assert_eq!(rows.len(), 1, "only the valid assistant+cost row survives");
        assert_eq!(rows[0].model, "good");
    }

    #[test]
    fn rows_outside_history_window_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        let far_past = iso_ms("2025-01-01T00:00:00.000Z");
        let recent = a14_now().timestamp_millis();
        write_message_db_with_model(
            &db,
            &[(far_past, 1.0, Some("old")), (recent, 2.0, Some("new"))],
        );
        let rows = read_rows(&db).unwrap();
        let buckets = daily_model_costs(&rows, a14_now(), 1);
        let models: Vec<&str> = buckets.iter().map(|b| b.model.as_str()).collect();
        assert!(
            !models.contains(&"old"),
            "old row should be outside the 1-day window: {buckets:?}"
        );
    }

    #[test]
    fn day_boundary_keys_by_local_calendar_day() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        let just_after_utc_midnight = iso_ms("2026-03-06T00:30:00.000Z");
        write_message_db_with_model(&db, &[(just_after_utc_midnight, 1.5, Some("edge"))]);
        let rows = read_rows(&db).unwrap();
        let buckets = daily_model_costs(&rows, a14_now_afternoon(), 30);
        assert_eq!(buckets.len(), 1, "{buckets:?}");
        assert!(
            NaiveDate::parse_from_str(&buckets[0].day_key, "%Y-%m-%d").is_ok(),
            "day_key not yyyy-MM-dd: {}",
            buckets[0].day_key
        );
    }

    #[test]
    fn model_cost_summary_aggregates_total_and_by_model() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        write_message_db_with_model(
            &db,
            &[
                (iso_ms("2026-03-06T11:00:00.000Z"), 3.0, Some("a")),
                (iso_ms("2026-03-06T12:00:00.000Z"), 1.0, Some("b")),
                (iso_ms("2026-03-06T13:00:00.000Z"), 2.0, None),
            ],
        );
        let rows = read_rows(&db).unwrap();
        let summary = model_cost_summary_from_rows(&rows, a14_now_afternoon(), 30);
        assert!((summary.total_cost_usd - 6.0).abs() < 1e-6, "{summary:?}");
        assert_eq!(summary.request_count, 3);
        assert!((summary.by_model["a"] - 3.0).abs() < 1e-6);
        assert!((summary.by_model["b"] - 1.0).abs() < 1e-6);
        assert!((summary.by_model[UNKNOWN_MODEL_NAME] - 2.0).abs() < 1e-6);
        assert!(summary.period_start.is_some() && summary.period_end.is_some());
    }

    #[test]
    fn daily_series_sums_models_per_day_via_pure_aggregation() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        write_message_db_with_model(
            &db,
            &[
                (iso_ms("2026-03-06T11:00:00.000Z"), 3.0, Some("a")),
                (iso_ms("2026-03-06T12:00:00.000Z"), 2.0, Some("b")),
            ],
        );
        let rows = read_rows(&db).unwrap();
        let buckets = daily_model_costs(&rows, a14_now_afternoon(), 30);
        let mut by_day: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();
        for b in &buckets {
            *by_day.entry(b.day_key.clone()).or_insert(0.0) += b.cost;
        }
        let series: Vec<(String, f64)> = by_day.into_iter().collect();
        assert_eq!(series.len(), 1, "{series:?}");
        assert!((series[0].1 - 5.0).abs() < 1e-6);
    }

    #[test]
    fn daily_aggregation_is_independent_of_zen_wait() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        write_message_db_with_model(&db, &[(iso_ms("2026-03-06T11:00:00.000Z"), 2.0, Some("a"))]);
        let rows = read_rows(&db).unwrap();
        let now = a14_now();
        let b1 = daily_model_costs(&rows, now, 30);
        let b2 = daily_model_costs(&rows, now, 30);
        assert_eq!(b1, b2, "pure aggregation must be deterministic");
        assert_eq!(b1.len(), 1);
    }
}
