use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Local, TimeZone, Utc};
use serde_json::Value;

const MAX_SESSION_FILES: usize = 2048;
const MAX_SESSION_FILE_BYTES: usize = 32 * 1024 * 1024;
const MAX_SESSION_FILE_BYTES_U64: u64 = 32 * 1024 * 1024;
const MAX_JSONL_LINE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LocalHistoryCoverage {
    Complete,
    Partial,
    #[default]
    Unavailable,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LocalSessionSummary {
    pub total_tokens: u64,
    pub session_count: usize,
    pub coverage: LocalHistoryCoverage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScanContext {
    database_roots: [PathBuf; 3],
    tokscale_sessions: PathBuf,
}

impl ScanContext {
    fn from_values(
        home: &Path,
        gemini_cli_home: Option<&str>,
        tokscale_config_dir: Option<&str>,
    ) -> Self {
        let gemini_base = clean_env_path(gemini_cli_home).unwrap_or_else(|| home.join(".gemini"));
        let tokscale_base = clean_env_path(tokscale_config_dir)
            .unwrap_or_else(|| home.join(".config").join("tokscale"));
        Self {
            database_roots: super::local_sqlite::database_roots(&gemini_base),
            tokscale_sessions: tokscale_base.join("antigravity-cache").join("sessions"),
        }
    }

    fn capture() -> Option<Self> {
        let home = dirs::home_dir()?;
        let gemini = std::env::var("GEMINI_CLI_HOME").ok();
        let tokscale = std::env::var("TOKSCALE_CONFIG_DIR").ok();
        Some(Self::from_values(
            &home,
            gemini.as_deref(),
            tokscale.as_deref(),
        ))
    }
}

fn clean_env_path(value: Option<&str>) -> Option<PathBuf> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub fn summarize(days: u32) -> LocalSessionSummary {
    let now = Utc::now();
    let Some(context) = ScanContext::capture() else {
        return LocalSessionSummary::default();
    };
    match super::local_sqlite::summarize(&context.database_roots, now, days) {
        super::local_sqlite::SQLiteScan::Summary(summary) => summary,
        super::local_sqlite::SQLiteScan::NoDatabases => {
            let (paths, truncated) = tokscale_paths(&context.tokscale_sessions);
            if paths.is_empty() {
                LocalSessionSummary::default()
            } else {
                summarize_paths(&paths, now, days, truncated)
            }
        }
    }
}

/// Count local Antigravity conversation artifacts for the quota provider's
/// offline fallback. Mirrors upstream #3119 without opening SQLite files.
pub fn offline_conversation_count() -> usize {
    let Some(context) = ScanContext::capture() else {
        return 0;
    };
    offline_conversation_count_context(&context)
}

fn offline_conversation_count_in(home: &Path) -> usize {
    offline_conversation_count_context(&ScanContext::from_values(home, None, None))
}

fn offline_conversation_count_context(context: &ScanContext) -> usize {
    let db_count = context
        .database_roots
        .iter()
        .map(|root| count_extension(root, "db"))
        .sum::<usize>();
    if db_count > 0 {
        return db_count;
    }
    tokscale_paths(&context.tokscale_sessions).0.len()
}

fn count_extension(root: &Path, extension: &str) -> usize {
    fs::read_dir(root)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some(extension))
        .count()
}

fn tokscale_paths(base: &Path) -> (Vec<PathBuf>, bool) {
    let Ok(entries) = fs::read_dir(base) else {
        return (Vec::new(), false);
    };
    let mut paths: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("jsonl"))
        })
        .collect();
    paths.sort();
    let truncated = paths.len() > MAX_SESSION_FILES;
    if truncated {
        paths.drain(..paths.len() - MAX_SESSION_FILES);
    }
    (paths, truncated)
}

fn summarize_paths(
    paths: &[PathBuf],
    now: DateTime<Utc>,
    days: u32,
    truncated: bool,
) -> LocalSessionSummary {
    let first_day = now.with_timezone(&Local).date_naive()
        - Duration::days(i64::from(days.clamp(1, 365).saturating_sub(1)));
    let mut total_tokens = 0_u64;
    let mut sessions_with_usage = HashSet::new();
    let mut seen_response_ids = HashSet::new();
    let mut complete = !truncated;

    for path in paths.iter().take(MAX_SESSION_FILES) {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(_) => {
                complete = false;
                continue;
            }
        };
        match file.metadata() {
            Ok(metadata) if metadata.len() > MAX_SESSION_FILE_BYTES_U64 => complete = false,
            Ok(_) => {}
            Err(_) => complete = false,
        }
        let mut reader = BufReader::new(file);
        let mut remaining = MAX_SESSION_FILE_BYTES;
        let mut path_had_usage = false;
        loop {
            let line = match read_bounded_jsonl_line(&mut reader, &mut remaining) {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(_) => {
                    complete = false;
                    break;
                }
            };
            if line.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_slice::<Value>(&line) else {
                continue;
            };
            let kind = value.get("type").and_then(Value::as_str);
            if kind != Some("usage") && value.get("input").is_none() {
                continue;
            }
            if let Some(response_id) = value
                .get("responseId")
                .or_else(|| value.get("response_id"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                && !seen_response_ids.insert(response_id.to_string())
            {
                continue;
            }

            let timestamp_ms = value
                .get("timestamp")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let Some(at) = Utc.timestamp_millis_opt(timestamp_ms).single() else {
                continue;
            };
            if at > now || at.with_timezone(&Local).date_naive() < first_day {
                continue;
            }

            let input = token_field(&value, &["input"]);
            let output = token_field(&value, &["output"]);
            let cache_read = token_field(&value, &["cacheRead", "cache_read"]);
            let cache_write = token_field(&value, &["cacheWrite", "cache_write"]);
            let total = input
                .saturating_add(output)
                .saturating_add(cache_read)
                .saturating_add(cache_write);
            if total == 0 {
                continue;
            }
            total_tokens = total_tokens.saturating_add(total);
            path_had_usage = true;
        }
        if path_had_usage {
            sessions_with_usage.insert(path.clone());
        }
    }

    LocalSessionSummary {
        total_tokens,
        session_count: sessions_with_usage.len(),
        coverage: if paths.is_empty() {
            LocalHistoryCoverage::Unavailable
        } else if complete {
            LocalHistoryCoverage::Complete
        } else {
            LocalHistoryCoverage::Partial
        },
    }
}

fn read_bounded_jsonl_line<R: BufRead>(
    reader: &mut R,
    remaining_file_bytes: &mut usize,
) -> std::io::Result<Option<Vec<u8>>> {
    if *remaining_file_bytes == 0 {
        return Ok(None);
    }
    let mut line = Vec::new();
    let mut saw_input = false;
    let mut discarding = false;

    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            return Ok(saw_input.then_some(if discarding { Vec::new() } else { line }));
        }
        let bounded_len = chunk.len().min(*remaining_file_bytes);
        if bounded_len == 0 {
            return Ok(None);
        }
        let bounded = &chunk[..bounded_len];
        let newline = bounded.iter().position(|byte| *byte == b'\n');
        let segment_end = newline.unwrap_or(bounded.len());
        let segment = &bounded[..segment_end];
        saw_input = saw_input || !segment.is_empty() || newline.is_some();
        if !discarding {
            if line.len().saturating_add(segment.len()) <= MAX_JSONL_LINE_BYTES {
                line.extend_from_slice(segment);
            } else {
                line.clear();
                discarding = true;
            }
        }
        let consumed = segment_end + usize::from(newline.is_some());
        reader.consume(consumed);
        *remaining_file_bytes = remaining_file_bytes.saturating_sub(consumed);
        if newline.is_some() {
            return Ok(Some(if discarding { Vec::new() } else { line }));
        }
        if *remaining_file_bytes == 0 {
            return Ok(Some(Vec::new()));
        }
    }
}

fn token_field(value: &Value, keys: &[&str]) -> u64 {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_u64))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_context_honors_non_empty_root_overrides() {
        let home = Path::new(r"C:\Users\test");
        let context =
            ScanContext::from_values(home, Some(r"D:\gemini-root"), Some(r"E:\tokscale-root"));
        assert_eq!(
            context.database_roots[0],
            PathBuf::from(r"D:\gemini-root")
                .join("antigravity-cli")
                .join("conversations")
        );
        assert_eq!(
            context.tokscale_sessions,
            PathBuf::from(r"E:\tokscale-root")
                .join("antigravity-cache")
                .join("sessions")
        );
        let defaults = ScanContext::from_values(home, Some("  "), Some(""));
        assert_eq!(
            defaults.database_roots[1],
            home.join(".gemini").join("antigravity")
        );
        assert_eq!(
            defaults.tokscale_sessions,
            home.join(".config")
                .join("tokscale")
                .join("antigravity-cache")
                .join("sessions")
        );
    }

    #[test]
    fn summarizes_tokscale_jsonl_and_deduplicates_response_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-a.jsonl");
        fs::write(&path, concat!(
            "{\"type\":\"session_meta\",\"modelId\":\"test-model-antigravity-a\"}\n",
            "{\"type\":\"usage\",\"responseId\":\"r1\",\"timestamp\":1787572800000,\"input\":100,\"output\":20,\"cacheRead\":10,\"cacheWrite\":5}\n",
            "{\"type\":\"usage\",\"response_id\":\"r1\",\"timestamp\":1787572800000,\"input\":100,\"output\":20}\n"
        )).unwrap();
        let now = Utc.timestamp_millis_opt(1787576400000).single().unwrap();
        let summary = summarize_paths(&[path], now, 7, false);
        assert_eq!(summary.total_tokens, 135);
        assert_eq!(summary.session_count, 1);
    }

    #[test]
    fn truncated_or_unreadable_tokscale_history_is_partial() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-a.jsonl");
        fs::write(
            &path,
            b"{\"type\":\"usage\",\"timestamp\":1787572800000,\"input\":10}\n",
        )
        .unwrap();
        let now = Utc.timestamp_millis_opt(1787576400000).single().unwrap();
        let truncated = summarize_paths(std::slice::from_ref(&path), now, 7, true);
        assert_eq!(truncated.coverage, LocalHistoryCoverage::Partial);

        let missing = summarize_paths(&[dir.path().join("missing.jsonl")], now, 7, false);
        assert_eq!(missing.coverage, LocalHistoryCoverage::Partial);
    }
    #[test]
    fn offline_count_prefers_cli_and_app_db_artifacts_then_tokscale() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir
            .path()
            .join(".gemini")
            .join("antigravity")
            .join("conversations");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("a.db"), b"").unwrap();
        fs::write(app.join("a.db-wal"), b"").unwrap();
        assert_eq!(offline_conversation_count_in(dir.path()), 1);

        fs::remove_file(app.join("a.db")).unwrap();
        let cache = dir
            .path()
            .join(".config")
            .join("tokscale")
            .join("antigravity-cache")
            .join("sessions");
        fs::create_dir_all(&cache).unwrap();
        fs::write(
            cache.join("one.jsonl"),
            b"{}
",
        )
        .unwrap();
        assert_eq!(offline_conversation_count_in(dir.path()), 1);
    }

    #[test]
    fn excludes_usage_outside_requested_window() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-a.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"usage\",\"timestamp\":1787572800000,\"input\":10,\"output\":5}\n",
                "{\"type\":\"usage\",\"timestamp\":1784894400000,\"input\":99,\"output\":99}\n"
            ),
        )
        .unwrap();
        let now = Utc.timestamp_millis_opt(1787576400000).single().unwrap();
        let summary = summarize_paths(&[path], now, 7, false);
        assert_eq!(summary.total_tokens, 15);
        assert_eq!(summary.session_count, 1);
    }

    #[test]
    fn oversized_jsonl_line_is_discarded_and_next_usage_row_is_counted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-a.jsonl");
        let mut text = format!(
            r#"{{"type":"usage","padding":"{}"}}"#,
            "x".repeat(MAX_JSONL_LINE_BYTES + 32)
        );
        text.push('\n');
        text.push_str(r#"{"type":"usage","timestamp":1787572800000,"input":10,"output":5}"#);
        text.push('\n');
        fs::write(&path, text).unwrap();
        let now = Utc.timestamp_millis_opt(1787576400000).single().unwrap();
        let summary = summarize_paths(&[path], now, 7, false);
        assert_eq!(summary.total_tokens, 15);
        assert_eq!(summary.session_count, 1);
    }
}
