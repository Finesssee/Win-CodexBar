//! Pi-family session-file parsing (upstream `PiFamilySessionFileParser`).

use super::{MAX_PREFIX_READ, MAX_TITLE_SCALARS, PiSessionDialect, TAIL_READ};
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------

/// One parsed Pi-family session file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiFamilySessionRecord {
    pub id: String,
    pub cwd: Option<String>,
    pub session_name: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub modified_at: DateTime<Utc>,
    pub path: PathBuf,
}

/// Parse a session file in the given dialect (upstream `parse(url:dialect:…)`).
///
/// `modified_at` is the file's content-modification stamp; the record's
/// `modified_at` is clamped to `<= now` exactly like upstream. Malformed,
/// wrong-version, empty, or truncated-headed files return `None`.
pub fn parse_session_file(
    path: &Path,
    dialect: PiSessionDialect,
    modified_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Option<PiFamilySessionRecord> {
    let prefix = read_prefix(path)?;
    let lines = complete_lines(&prefix)?;
    let mut non_empty: Vec<&[u8]> = lines
        .iter()
        .map(Vec::as_slice)
        .filter(|l| !l.is_empty())
        .collect();
    if non_empty.is_empty() {
        return None;
    }

    let mut title_slot_was_present = false;
    let mut title_slot: Option<String> = None;
    if dialect == PiSessionDialect::Omp
        && let Some(first) = json_object(non_empty[0])
        && first.get("type").and_then(serde_json::Value::as_str) == Some("title")
    {
        title_slot_was_present = true;
        title_slot = first
            .get("title")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        non_empty.remove(0);
    }

    let header_data = *non_empty.first()?;
    let header = json_object(header_data)?;
    if header.get("type").and_then(serde_json::Value::as_str) != Some("session") {
        return None;
    }
    let id = header.get("id").and_then(serde_json::Value::as_str)?;
    if dialect == PiSessionDialect::Pi
        && header.get("version").and_then(serde_json::Value::as_i64) != Some(3)
    {
        return None;
    }

    let raw_title = match dialect {
        PiSessionDialect::Pi => latest_pi_session_name(path, non_empty.as_slice()),
        PiSessionDialect::Omp => {
            if title_slot_was_present {
                title_slot
            } else {
                header
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            }
        }
    };
    let session_name = raw_title.as_deref().and_then(sanitized_title);
    let started_at = header
        .get("timestamp")
        .and_then(serde_json::Value::as_str)
        .and_then(parse_iso_date);

    Some(PiFamilySessionRecord {
        id: id.to_string(),
        cwd: header
            .get("cwd")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        session_name,
        started_at,
        modified_at: modified_at.min(now),
        path: path.to_path_buf(),
    })
}

/// Latest `session_info.name` for pi files — prefix scan, then a bounded
/// tail scan when the file is longer than the prefix (upstream
/// `latestPiSessionName(in:prefixLines:)`).
fn latest_pi_session_name(path: &Path, prefix_lines: &[&[u8]]) -> Option<String> {
    let latest = latest_pi_session_name_in(prefix_lines);
    let Ok(metadata) = std::fs::metadata(path) else {
        return latest;
    };
    let size = metadata.len();
    if size <= MAX_PREFIX_READ as u64 {
        return latest;
    }

    let Ok(mut file) = std::fs::File::open(path) else {
        return latest;
    };
    use std::io::{Read, Seek, SeekFrom};
    let offset = size.saturating_sub(TAIL_READ as u64);
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return latest;
    }
    let mut tail = vec![0_u8; TAIL_READ.min(size as usize)];
    let mut read_total = 0_usize;
    while read_total < tail.len() {
        match file.read(&mut tail[read_total..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => read_total += n,
        }
    }
    tail.truncate(read_total);
    if tail.is_empty() {
        return latest;
    }
    // A mid-read offset may cut a record: drop the first partial line.
    let cut = offset > 0 && !tail.starts_with(&[b'\n'][..]);
    let mut tail_lines: Vec<&[u8]> = tail
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .collect();
    if cut && !tail_lines.is_empty() {
        tail_lines.remove(0);
    }
    latest_pi_session_name_in(&tail_lines).or(latest)
}

fn latest_pi_session_name_in(lines: &[&[u8]]) -> Option<String> {
    lines.iter().rev().find_map(|line| {
        let entry = json_object(line)?;
        if entry.get("type").and_then(serde_json::Value::as_str) != Some("session_info") {
            return None;
        }
        entry
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    })
}

fn read_prefix(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    let limit = (metadata.len() as usize).min(MAX_PREFIX_READ);
    let mut data = vec![0_u8; limit];
    let mut read_total = 0_usize;
    while read_total < limit {
        match file.read(&mut data[read_total..]) {
            Ok(0) => break,
            Ok(n) => read_total += n,
            Err(_) => return None,
        }
    }
    data.truncate(read_total);
    Some(data)
}

/// Upstream `completeLines`: split on LF; a trailing partial line is kept
/// only when the read stopped short of the cap (not a truncated record).
fn complete_lines(data: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut lines: Vec<Vec<u8>> = Vec::new();
    let mut line_start = 0_usize;
    for (index, byte) in data.iter().enumerate() {
        if *byte == b'\n' {
            lines.push(data[line_start..index].to_vec());
            line_start = index + 1;
        }
    }
    if line_start < data.len() && data.len() < MAX_PREFIX_READ {
        lines.push(data[line_start..].to_vec());
    }
    if line_start == data.len() || !lines.is_empty() {
        Some(lines)
    } else {
        None
    }
}

fn json_object(line: &[u8]) -> Option<serde_json::Map<String, serde_json::Value>> {
    let value: serde_json::Value = serde_json::from_slice(line).ok()?;
    value.as_object().cloned()
}

/// Upstream `parseDate`: ISO 8601 with or without fractional seconds
/// (`DateTime::parse_from_rfc3339` covers both forms).
pub fn parse_iso_date(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Upstream `sanitizedTitle`: drop control + newline scalars, cap at 64
/// scalars; empty results collapse to `None`.
pub fn sanitized_title(value: &str) -> Option<String> {
    let mut result = String::new();
    for ch in value.chars() {
        if ch.is_control() || ch == '\n' || ch == '\r' {
            continue;
        }
        if result.chars().count() >= MAX_TITLE_SCALARS {
            break;
        }
        result.push(ch);
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}
