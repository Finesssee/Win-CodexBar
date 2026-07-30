//! JSONL Scanner with Caching
//!
//! Incremental log file parsing for Codex and Claude session logs.
//! Supports file-level caching to avoid re-parsing unchanged files.

#![allow(dead_code)]

use crate::core::{CostUsagePricing, ProviderId};
use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Maximum retained Codex JSONL line size (upstream session-metadata bound).
const CODEX_JSONL_MAX_LINE_BYTES: usize = 256 * 1024;

/// Default scanner-side refresh debounce (upstream CostUsageScanner).
pub const DEFAULT_COST_SCAN_REFRESH_MIN_INTERVAL_SECS: u64 = 60;

/// Options for a cost scan pass (disk-cache-backed full inspections).
///
/// Default debounce is 60s between full disk inspections when a
/// [`CostUsageCache`] is present. Pass [`CostScanOptions::app_driven`] (interval 0)
/// for explicit/CLI refreshes. Production [`crate::cost_scanner::CostScanner`]
/// honors these options and persists cache under `{cache}/CodexBar/cost-usage/`.
#[derive(Debug, Clone, Copy)]
pub struct CostScanOptions {
    /// Minimum seconds between disk-cache-backed full inspections.
    /// Set to 0 to force a fresh scan (app-driven / forceRefresh).
    pub refresh_min_interval_secs: u64,
}

impl Default for CostScanOptions {
    fn default() -> Self {
        Self {
            refresh_min_interval_secs: DEFAULT_COST_SCAN_REFRESH_MIN_INTERVAL_SECS,
        }
    }
}

impl CostScanOptions {
    /// App-driven or forced refresh: skip the scanner debounce entirely.
    pub fn app_driven() -> Self {
        Self {
            refresh_min_interval_secs: 0,
        }
    }

    /// Whether a prior scan at `last_scan_unix_ms` is still within the debounce window.
    pub fn should_skip_scan(&self, last_scan_unix_ms: i64, now_unix_ms: i64) -> bool {
        let refresh_ms = (self.refresh_min_interval_secs as i64).saturating_mul(1000);
        refresh_ms > 0
            && last_scan_unix_ms > 0
            && now_unix_ms.saturating_sub(last_scan_unix_ms) <= refresh_ms
    }
}

/// Cache for scanned file data
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CostUsageCache {
    /// Last scan timestamp in milliseconds
    pub last_scan_unix_ms: i64,
    /// Per-file usage data
    pub files: HashMap<String, CostUsageFileUsage>,
    /// Aggregated daily data: day_key -> model -> [input, cached, output]
    pub days: HashMap<String, HashMap<String, Vec<i32>>>,
    /// Inclusive range covered by the last successful full inspection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_since_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_until_key: Option<String>,
}

/// Per-file usage tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostUsageFileUsage {
    /// File modification time in milliseconds
    pub mtime_unix_ms: i64,
    /// File size in bytes
    pub size: i64,
    /// Daily usage data extracted from this file
    pub days: HashMap<String, HashMap<String, Vec<i32>>>,
    /// Bytes parsed so far (for incremental parsing)
    pub parsed_bytes: Option<i64>,
    /// Last model seen (for delta calculations)
    pub last_model: Option<String>,
    /// Last token totals (for delta calculations)
    pub last_totals: Option<CodexTotals>,
}

/// Running totals for Codex token counting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexTotals {
    pub input: i32,
    pub cached: i32,
    pub output: i32,
}

/// Result of parsing a Codex file
#[derive(Debug)]
pub struct CodexParseResult {
    /// Individual token-count deltas used for per-request pricing.
    pub records: Vec<CodexUsageRecord>,
    /// Bytes parsed
    pub parsed_bytes: i64,
    /// Last model seen
    pub last_model: Option<String>,
    /// Last totals seen
    pub last_totals: Option<CodexTotals>,
}

/// A billable Codex token-count delta.
#[derive(Debug, Clone)]
pub struct CodexUsageRecord {
    pub day_key: String,
    pub model: String,
    pub input: i32,
    pub cached: i32,
    pub output: i32,
}

/// Day range for scanning
pub struct CostUsageDayRange {
    pub since_key: String,
    pub until_key: String,
    pub scan_since_key: String,
    pub scan_until_key: String,
}

impl CostUsageDayRange {
    pub fn new(since: NaiveDate, until: NaiveDate) -> Self {
        let since_minus_one = since - chrono::Duration::days(1);
        let until_plus_one = until + chrono::Duration::days(1);

        Self {
            since_key: Self::day_key(since),
            until_key: Self::day_key(until),
            scan_since_key: Self::day_key(since_minus_one),
            scan_until_key: Self::day_key(until_plus_one),
        }
    }

    pub fn day_key(date: NaiveDate) -> String {
        date.format("%Y-%m-%d").to_string()
    }

    pub fn is_in_range(day_key: &str, since: &str, until: &str) -> bool {
        day_key >= since && day_key <= until
    }

    pub fn parse_day_key(key: &str) -> Option<NaiveDate> {
        NaiveDate::parse_from_str(key, "%Y-%m-%d").ok()
    }
}

/// JSONL Scanner for cost/usage logs
pub struct JsonlScanner;

struct CodexParserState {
    current_model: Option<String>,
    previous_totals: Option<CodexTotals>,
    /// High watermark of observed cumulative totals (never lowered). Used for
    /// Ultra interleaved-lineage containment (issue #2037 Phase 1).
    totals_watermark: Option<CodexTotals>,
    /// Latched once any cumulative component drops below the watermark.
    saw_interleaved_totals: bool,
    records: Vec<CodexUsageRecord>,
}

#[derive(Debug, Deserialize)]
struct CodexFastLine<'a> {
    #[serde(rename = "type", borrow)]
    event_type: Option<&'a str>,
    #[serde(default, borrow)]
    timestamp: Option<&'a str>,
    #[serde(default, borrow)]
    payload: Option<CodexFastPayload<'a>>,
    #[serde(default, borrow)]
    event_msg: Option<CodexFastPayload<'a>>,
    #[serde(default, borrow)]
    model: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
struct CodexFastPayload<'a> {
    #[serde(rename = "type", borrow)]
    payload_type: Option<&'a str>,
    #[serde(default, borrow)]
    model: Option<&'a str>,
    #[serde(default, borrow)]
    model_name: Option<&'a str>,
    #[serde(default, borrow)]
    info: Option<CodexFastInfo<'a>>,
    #[serde(default)]
    input_tokens: Option<i32>,
    #[serde(default)]
    cached_input_tokens: Option<i32>,
    #[serde(default)]
    cache_read_input_tokens: Option<i32>,
    #[serde(default)]
    output_tokens: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct CodexFastInfo<'a> {
    #[serde(default, borrow)]
    model: Option<&'a str>,
    #[serde(default, borrow)]
    model_name: Option<&'a str>,
    #[serde(default)]
    total_token_usage: Option<CodexFastTotals>,
    #[serde(default)]
    last_token_usage: Option<CodexFastTotals>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct CodexFastTotals {
    #[serde(default)]
    input_tokens: i32,
    #[serde(default)]
    cached_input_tokens: Option<i32>,
    #[serde(default)]
    cache_read_input_tokens: Option<i32>,
    #[serde(default)]
    output_tokens: i32,
}

enum CodexFastEvent<'a> {
    TurnContext {
        model: Option<&'a str>,
    },
    TokenCount {
        timestamp: &'a str,
        payload: CodexFastPayload<'a>,
    },
}

impl CodexParserState {
    fn new(initial_model: Option<String>, initial_totals: Option<CodexTotals>) -> Self {
        Self {
            current_model: initial_model,
            previous_totals: initial_totals.clone(),
            totals_watermark: initial_totals,
            saw_interleaved_totals: false,
            records: Vec::new(),
        }
    }

    fn process_line(&mut self, line: &str, range: &CostUsageDayRange) {
        if !is_candidate_codex_line(line) {
            return;
        }

        if let Some(event) = parse_codex_fast_event(line) {
            self.process_fast_event(event, range);
            return;
        }

        let Ok(obj) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let Some(day_key) = codex_line_day_key(&obj, range) else {
            return;
        };

        if obj.get("type").and_then(|v| v.as_str()) == Some("turn_context") {
            self.update_current_model(&obj);
        }

        if token_count_payload(&obj).is_some() {
            self.record_token_count(&obj, day_key);
        }
    }

    fn process_fast_event(&mut self, event: CodexFastEvent<'_>, range: &CostUsageDayRange) {
        match event {
            CodexFastEvent::TurnContext { model } => {
                // Explicit blank model evidence clears stale turn context.
                if let Some(raw) = model {
                    self.current_model = model_evidence(raw).map(str::to_string);
                }
            }
            CodexFastEvent::TokenCount { timestamp, payload } => {
                let Some(day_key) = codex_timestamp_day_key(timestamp) else {
                    return;
                };
                if !CostUsageDayRange::is_in_range(
                    &day_key,
                    &range.scan_since_key,
                    &range.scan_until_key,
                ) {
                    return;
                }
                self.record_fast_token_count(payload, day_key);
            }
        }
    }

    fn update_current_model(&mut self, obj: &Value) {
        let candidates = [
            obj.get("model").and_then(|v| v.as_str()),
            obj.get("payload")
                .and_then(|payload| payload.get("model"))
                .and_then(|v| v.as_str()),
            obj.get("payload")
                .and_then(|payload| payload.get("model_name"))
                .and_then(|v| v.as_str()),
            obj.get("payload")
                .and_then(|payload| payload.get("info"))
                .and_then(|info| info.get("model"))
                .and_then(|v| v.as_str()),
            obj.get("payload")
                .and_then(|payload| payload.get("info"))
                .and_then(|info| info.get("model_name"))
                .and_then(|v| v.as_str()),
        ];
        // Only rewrite current_model when the turn_context actually carries a
        // model field (including blank, which clears stale attribution).
        let has_key = candidates.iter().any(|c| c.is_some());
        if !has_key {
            return;
        }
        self.current_model = candidates
            .into_iter()
            .flatten()
            .find_map(model_evidence)
            .map(str::to_string);
    }

    fn record_token_count(&mut self, obj: &Value, day_key: String) {
        let Some(payload) = token_count_payload(obj) else {
            return;
        };
        let Some((delta_input, delta_cached, delta_output)) = self.token_deltas(payload) else {
            return;
        };
        if delta_input == 0 && delta_cached == 0 && delta_output == 0 {
            return;
        }

        let info = payload.get("info");
        let model = self.resolve_token_model(info, payload, obj);
        self.record_usage(day_key, &model, delta_input, delta_cached, delta_output);
    }

    fn record_fast_token_count(&mut self, payload: CodexFastPayload<'_>, day_key: String) {
        let Some((delta_input, delta_cached, delta_output)) = self.fast_token_deltas(&payload)
        else {
            return;
        };
        if delta_input == 0 && delta_cached == 0 && delta_output == 0 {
            return;
        }

        let event_model = payload
            .info
            .as_ref()
            .and_then(|info| info.model.or(info.model_name))
            .or(payload.model)
            .and_then(model_evidence);
        // Prefer current turn_context model over a conflicting event model,
        // matching upstream precedence. Fall back to unattributed (not gpt-5).
        let model = self
            .current_model
            .as_deref()
            .and_then(model_evidence)
            .or(event_model)
            .unwrap_or(CostUsagePricing::CODEX_UNATTRIBUTED_MODEL)
            .to_string();
        self.record_usage(day_key, &model, delta_input, delta_cached, delta_output);
    }

    fn record_usage(&mut self, day_key: String, model: &str, input: i32, cached: i32, output: i32) {
        self.records.push(CodexUsageRecord {
            day_key,
            model: CostUsagePricing::normalize_codex_model(model),
            input,
            cached: cached.min(input),
            output,
        });
    }

    fn resolve_token_model(&self, info: Option<&Value>, payload: &Value, obj: &Value) -> String {
        let event_model = info
            .and_then(|i| i.get("model").or(i.get("model_name")))
            .or_else(|| payload.get("model"))
            .or_else(|| obj.get("model"))
            .and_then(|v| v.as_str())
            .and_then(model_evidence);
        self.current_model
            .as_deref()
            .and_then(model_evidence)
            .or(event_model)
            .unwrap_or(CostUsagePricing::CODEX_UNATTRIBUTED_MODEL)
            .to_string()
    }

    fn token_deltas(&mut self, payload: &Value) -> Option<(i32, i32, i32)> {
        let info = payload.get("info");
        if let Some(total) = info.and_then(|i| i.get("total_token_usage")) {
            return Some(self.total_usage_delta(total));
        }

        if let Some(last) = info.and_then(|i| i.get("last_token_usage")) {
            return Some(last_usage_delta(last));
        }

        let direct = read_token_totals(payload);
        (direct.input != 0 || direct.cached != 0 || direct.output != 0).then_some((
            direct.input.max(0),
            direct.cached.max(0),
            direct.output.max(0),
        ))
    }

    fn fast_token_deltas(&mut self, payload: &CodexFastPayload<'_>) -> Option<(i32, i32, i32)> {
        if let Some(total) = payload
            .info
            .as_ref()
            .and_then(|info| info.total_token_usage)
        {
            return Some(self.fast_total_usage_delta(total));
        }

        if let Some(last) = payload.info.as_ref().and_then(|info| info.last_token_usage) {
            return Some(fast_last_usage_delta(last));
        }

        let direct = fast_totals_from_payload(payload);
        (direct.input != 0 || direct.cached != 0 || direct.output != 0).then_some((
            direct.input.max(0),
            direct.cached.max(0),
            direct.output.max(0),
        ))
    }

    fn total_usage_delta(&mut self, total: &Value) -> (i32, i32, i32) {
        let totals = read_token_totals(total);
        self.apply_totals_delta(totals)
    }

    fn fast_total_usage_delta(&mut self, total: CodexFastTotals) -> (i32, i32, i32) {
        let totals = codex_totals_from_fast(total);
        self.apply_totals_delta(totals)
    }

    fn apply_totals_delta(&mut self, totals: CodexTotals) -> (i32, i32, i32) {
        self.latch_if_below_watermark(&totals);

        let delta = if self.saw_interleaved_totals {
            contained_total_delta(
                self.totals_watermark.as_ref(),
                self.previous_totals.as_ref(),
                &totals,
            )
        } else {
            let previous = self.previous_totals.as_ref();
            CodexTotals {
                input: (totals.input - previous.map_or(0, |t| t.input)).max(0),
                cached: (totals.cached - previous.map_or(0, |t| t.cached)).max(0),
                output: (totals.output - previous.map_or(0, |t| t.output)).max(0),
            }
        };

        self.previous_totals = Some(totals.clone());
        self.raise_watermark(&totals);
        (delta.input, delta.cached, delta.output)
    }

    fn latch_if_below_watermark(&mut self, totals: &CodexTotals) {
        let Some(water) = self.totals_watermark.as_ref() else {
            return;
        };
        if totals.input < water.input
            || totals.cached < water.cached
            || totals.output < water.output
        {
            self.saw_interleaved_totals = true;
        }
    }

    fn raise_watermark(&mut self, totals: &CodexTotals) {
        self.totals_watermark = Some(match self.totals_watermark.as_ref() {
            Some(water) => CodexTotals {
                input: water.input.max(totals.input),
                cached: water.cached.max(totals.cached),
                output: water.output.max(totals.output),
            },
            None => totals.clone(),
        });
    }
}

fn model_evidence(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// When interleaved Ultra lineages reset cumulative counters, only count growth
/// above the historical high watermark so rewound branches do not re-add work.
fn contained_total_delta(
    watermark: Option<&CodexTotals>,
    counted: Option<&CodexTotals>,
    current: &CodexTotals,
) -> CodexTotals {
    let water = watermark.cloned().unwrap_or(CodexTotals {
        input: 0,
        cached: 0,
        output: 0,
    });
    let counted = counted.cloned().unwrap_or(CodexTotals {
        input: 0,
        cached: 0,
        output: 0,
    });

    let component = |water: i32, counted: i32, current: i32| -> i32 {
        if current >= water {
            // Only growth above the historical high watermark counts.
            (current - water.max(counted)).max(0)
        } else {
            // Below watermark: rewind / interleaved lineage — do not re-add
            // mid-range climbs that would inflate totals after a fork reset.
            0
        }
    };

    CodexTotals {
        input: component(water.input, counted.input, current.input),
        cached: component(water.cached, counted.cached, current.cached),
        output: component(water.output, counted.output, current.output),
    }
}

/// Read one JSONL line, discarding content when it exceeds `max_bytes`.
/// Returns `(line_without_newline, bytes_consumed_including_newline)`.
fn read_bounded_jsonl_line<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
) -> std::io::Result<Option<(Vec<u8>, usize)>> {
    let mut line = Vec::new();
    let mut saw_bytes = false;
    let mut discarding = false;
    let mut consumed_total = 0;

    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            return Ok(
                saw_bytes.then_some((if discarding { Vec::new() } else { line }, consumed_total))
            );
        }
        let newline = chunk.iter().position(|byte| *byte == b'\n');
        let segment_end = newline.unwrap_or(chunk.len());
        let segment = &chunk[..segment_end];
        saw_bytes = true;

        if !discarding {
            let remaining = max_bytes.saturating_sub(line.len());
            if segment.len() <= remaining {
                line.extend_from_slice(segment);
            } else {
                line.clear();
                discarding = true;
            }
        }

        let consumed = segment_end + usize::from(newline.is_some());
        reader.consume(consumed);
        consumed_total += consumed;
        if newline.is_some() {
            return Ok(Some((
                if discarding { Vec::new() } else { line },
                consumed_total,
            )));
        }
    }
}

fn parse_codex_fast_event(line: &str) -> Option<CodexFastEvent<'_>> {
    let parsed: CodexFastLine<'_> = serde_json::from_str(line).ok()?;
    match parsed.event_type? {
        "turn_context" => {
            let model = parsed
                .payload
                .as_ref()
                .and_then(|payload| {
                    payload.model.or(payload.model_name).or_else(|| {
                        payload
                            .info
                            .as_ref()
                            .and_then(|info| info.model.or(info.model_name))
                    })
                })
                .or(parsed.model);
            Some(CodexFastEvent::TurnContext { model })
        }
        "event_msg" => {
            let payload = parsed.payload.or(parsed.event_msg)?;
            (payload.payload_type == Some("token_count")).then_some(CodexFastEvent::TokenCount {
                timestamp: parsed.timestamp?,
                payload,
            })
        }
        _ => None,
    }
}

fn is_candidate_codex_line(line: &str) -> bool {
    if !line.contains("\"type\":\"event_msg\"")
        && !line.contains("\"type\":\"turn_context\"")
        && !line.contains("\"event_msg\"")
    {
        return false;
    }

    !line.contains("\"type\":\"event_msg\"") || line.contains("\"token_count\"")
}

fn codex_line_day_key(obj: &Value, range: &CostUsageDayRange) -> Option<String> {
    let ts = obj.get("timestamp").and_then(|v| v.as_str())?;
    let day_key = codex_timestamp_day_key(ts)?;

    CostUsageDayRange::is_in_range(&day_key, &range.scan_since_key, &range.scan_until_key)
        .then_some(day_key)
}

fn codex_timestamp_day_key(timestamp: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|ts| {
            ts.with_timezone(&Local)
                .date_naive()
                .format("%Y-%m-%d")
                .to_string()
        })
        .or_else(|| timestamp.get(..10).map(str::to_string))
}

fn token_count_payload(obj: &Value) -> Option<&Value> {
    if let Some(payload) = obj.get("payload")
        && payload.get("type").and_then(|v| v.as_str()) == Some("token_count")
    {
        return Some(payload);
    }

    let event_msg = obj.get("event_msg")?;
    (event_msg.get("type").and_then(|v| v.as_str()) == Some("token_count")).then_some(event_msg)
}

fn read_token_totals(value: &Value) -> CodexTotals {
    CodexTotals {
        input: token_i32(value, "input_tokens"),
        cached: value
            .get("cached_input_tokens")
            .or_else(|| value.get("cache_read_input_tokens"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32,
        output: token_i32(value, "output_tokens"),
    }
}

fn codex_totals_from_fast(value: CodexFastTotals) -> CodexTotals {
    CodexTotals {
        input: value.input_tokens,
        cached: value
            .cached_input_tokens
            .or(value.cache_read_input_tokens)
            .unwrap_or(0),
        output: value.output_tokens,
    }
}

fn fast_totals_from_payload(value: &CodexFastPayload<'_>) -> CodexTotals {
    CodexTotals {
        input: value.input_tokens.unwrap_or(0),
        cached: value
            .cached_input_tokens
            .or(value.cache_read_input_tokens)
            .unwrap_or(0),
        output: value.output_tokens.unwrap_or(0),
    }
}

fn token_i32(value: &Value, key: &str) -> i32 {
    value.get(key).and_then(|v| v.as_i64()).unwrap_or(0) as i32
}

fn last_usage_delta(last: &Value) -> (i32, i32, i32) {
    let totals = read_token_totals(last);
    (
        totals.input.max(0),
        totals.cached.max(0),
        totals.output.max(0),
    )
}

fn fast_last_usage_delta(last: CodexFastTotals) -> (i32, i32, i32) {
    let totals = codex_totals_from_fast(last);
    (
        totals.input.max(0),
        totals.cached.max(0),
        totals.output.max(0),
    )
}

impl JsonlScanner {
    /// Get default Codex sessions root directory
    pub fn default_codex_sessions_root() -> Option<PathBuf> {
        // Check CODEX_HOME environment variable
        if let Ok(home) = std::env::var("CODEX_HOME") {
            let home = home.trim();
            if !home.is_empty() {
                return Some(PathBuf::from(home).join("sessions"));
            }
        }

        // Default to ~/.codex/sessions
        dirs::home_dir().map(|h| h.join(".codex").join("sessions"))
    }

    /// Get default Claude projects roots
    pub fn default_claude_projects_roots() -> Vec<PathBuf> {
        let mut roots = Vec::new();

        // Check CLAUDE_CONFIG_DIR
        if let Ok(config_dir) = std::env::var("CLAUDE_CONFIG_DIR") {
            let path = PathBuf::from(config_dir.trim()).join("projects");
            if path.exists() {
                roots.push(path);
            }
        }

        // Default locations
        if let Some(home) = dirs::home_dir() {
            let default_path = home.join(".claude").join("projects");
            if default_path.exists() && !roots.contains(&default_path) {
                roots.push(default_path);
            }
        }

        roots
    }

    /// List Codex session files in the given date range
    pub fn list_codex_session_files(
        root: &Path,
        scan_since_key: &str,
        scan_until_key: &str,
    ) -> Vec<PathBuf> {
        let mut files = Vec::new();

        let Some(mut date) = CostUsageDayRange::parse_day_key(scan_since_key) else {
            return files;
        };
        let Some(until_date) = CostUsageDayRange::parse_day_key(scan_until_key) else {
            return files;
        };

        while date <= until_date {
            let year = format!("{:04}", date.year());
            let month = format!("{:02}", date.month());
            let day = format!("{:02}", date.day());

            let day_dir = root.join(&year).join(&month).join(&day);

            if let Ok(entries) = fs::read_dir(&day_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("jsonl"))
                    {
                        files.push(path);
                    }
                }
            }

            date += chrono::Duration::days(1);
        }

        files
    }

    /// Parse a Codex JSONL file
    pub fn parse_codex_file(
        file_path: &Path,
        range: &CostUsageDayRange,
        start_offset: i64,
        initial_model: Option<String>,
        initial_totals: Option<CodexTotals>,
    ) -> std::io::Result<CodexParseResult> {
        let file = File::open(file_path)?;
        let file_size = file.metadata()?.len() as i64;

        let mut reader = BufReader::new(file);
        if start_offset > 0 {
            reader.seek(SeekFrom::Start(start_offset as u64))?;
        }

        let mut parser = CodexParserState::new(initial_model, initial_totals);
        let mut parsed_bytes = start_offset;

        while let Some((line_bytes, consumed)) =
            read_bounded_jsonl_line(&mut reader, CODEX_JSONL_MAX_LINE_BYTES)?
        {
            parsed_bytes += consumed as i64;
            if line_bytes.is_empty() {
                continue;
            }
            let Ok(line) = std::str::from_utf8(&line_bytes) else {
                continue;
            };
            let line = line.strip_suffix('\r').unwrap_or(line);
            parser.process_line(line, range);
        }

        Ok(CodexParseResult {
            records: parser.records,
            parsed_bytes: file_size.max(parsed_bytes),
            last_model: parser.current_model,
            last_totals: parser.previous_totals,
        })
    }

    /// Whether a cached scan should be reused under `options` (issue #2089).
    pub fn should_skip_cached_scan(
        cache: &CostUsageCache,
        options: CostScanOptions,
        now_unix_ms: i64,
    ) -> bool {
        options.should_skip_scan(cache.last_scan_unix_ms, now_unix_ms)
    }

    /// Load cache from disk
    pub fn load_cache(provider: ProviderId, cache_root: Option<&Path>) -> CostUsageCache {
        let cache_path = Self::cache_path(provider, cache_root);

        if let Ok(contents) = fs::read_to_string(&cache_path)
            && let Ok(cache) = serde_json::from_str(&contents)
        {
            return cache;
        }

        CostUsageCache::default()
    }

    /// Save cache to disk (temp sibling + copy into place).
    pub fn save_cache(provider: ProviderId, cache: &CostUsageCache, cache_root: Option<&Path>) {
        let cache_path = Self::cache_path(provider, cache_root);

        let Some(parent) = cache_path.parent() else {
            return;
        };
        let _ = fs::create_dir_all(parent);

        let Ok(json) = serde_json::to_string(cache) else {
            return;
        };

        let tmp_name = format!(
            ".{}.{}-{}.tmp",
            provider.cli_name(),
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let tmp_path = parent.join(tmp_name);
        if fs::write(&tmp_path, json.as_bytes()).is_err() {
            return;
        }
        // `copy` replaces an existing target on Windows; prefer it over rename.
        if fs::copy(&tmp_path, &cache_path).is_err() {
            let _ = fs::write(&cache_path, json.as_bytes());
        }
        // Best-effort temp cleanup (ignore errors — unique name avoids clashes).
        let _ = fs::File::create(&tmp_path).and_then(|f| f.set_len(0));
    }

    /// Default on-disk cache root: `%LOCALAPPDATA%\CodexBar` (via `dirs::cache_dir`).
    pub fn default_cache_root() -> Option<PathBuf> {
        dirs::cache_dir().map(|d| d.join("CodexBar"))
    }

    fn cache_path(provider: ProviderId, cache_root: Option<&Path>) -> PathBuf {
        let root = cache_root
            .map(|p| p.to_path_buf())
            .or_else(Self::default_cache_root)
            .unwrap_or_else(|| PathBuf::from("."));

        // Mirror upstream layout: {cacheRoot}/cost-usage/{provider}-v1.json
        root.join("cost-usage")
            .join(format!("{}-v1.json", provider.cli_name()))
    }

    /// Whether `cache` covers the requested day window (for debounce short-circuit).
    pub fn cache_covers_range(cache: &CostUsageCache, range: &CostUsageDayRange) -> bool {
        match (&cache.scan_since_key, &cache.scan_until_key) {
            (Some(since), Some(until)) => {
                since.as_str() <= range.since_key.as_str()
                    && until.as_str() >= range.until_key.as_str()
            }
            _ => !cache.days.is_empty() || !cache.files.is_empty(),
        }
    }
}

use chrono::Datelike;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::io::Write;

    #[test]
    fn test_day_range() {
        let since = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let until = NaiveDate::from_ymd_opt(2026, 1, 20).unwrap();
        let range = CostUsageDayRange::new(since, until);

        assert_eq!(range.since_key, "2026-01-15");
        assert_eq!(range.until_key, "2026-01-20");
        assert_eq!(range.scan_since_key, "2026-01-14");
        assert_eq!(range.scan_until_key, "2026-01-21");
    }

    #[test]
    fn test_is_in_range() {
        assert!(CostUsageDayRange::is_in_range(
            "2026-01-15",
            "2026-01-10",
            "2026-01-20"
        ));
        assert!(!CostUsageDayRange::is_in_range(
            "2026-01-05",
            "2026-01-10",
            "2026-01-20"
        ));
        assert!(!CostUsageDayRange::is_in_range(
            "2026-01-25",
            "2026-01-10",
            "2026-01-20"
        ));
    }

    #[test]
    fn test_parse_day_key() {
        let date = CostUsageDayRange::parse_day_key("2026-01-15");
        assert!(date.is_some());
        let date = date.unwrap();
        assert_eq!(date.year(), 2026);
        assert_eq!(date.month(), 1);
        assert_eq!(date.day(), 15);
    }

    #[test]
    fn codex_timestamp_day_key_uses_local_calendar_day() {
        let today = Local::now().date_naive();
        let local_midnight = today.and_hms_opt(0, 30, 0).unwrap();
        let Some(local_time) = Local.from_local_datetime(&local_midnight).earliest() else {
            return;
        };
        let utc_timestamp = local_time.with_timezone(&chrono::Utc).to_rfc3339();
        let expected = today.format("%Y-%m-%d").to_string();

        assert_eq!(
            codex_timestamp_day_key(&utc_timestamp).as_deref(),
            Some(expected.as_str())
        );
    }

    #[test]
    fn test_fast_codex_parser_reads_last_usage_from_payload() {
        let range = CostUsageDayRange::new(
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
        );
        let mut parser = CodexParserState::new(None, None);

        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:00:00.000Z","type":"turn_context","payload":{"info":{"model":"gpt-5.5"}}}"#,
            &range,
        );
        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:00:02.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":120,"cache_read_input_tokens":40,"output_tokens":9}}}}"#,
            &range,
        );

        assert_eq!(parser.records.len(), 1);
        let record = &parser.records[0];
        assert_eq!(record.day_key, "2026-05-31");
        assert_eq!(record.model, "gpt-5.5");
        assert_eq!((record.input, record.cached, record.output), (120, 40, 9));
        assert_eq!(parser.current_model.as_deref(), Some("gpt-5.5"));
    }

    #[test]
    fn test_fast_codex_parser_diffs_total_usage() {
        let range = CostUsageDayRange::new(
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
        );
        let mut parser = CodexParserState::new(Some("gpt-5".to_string()), None);

        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:00:01.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":200,"output_tokens":50}}}}"#,
            &range,
        );
        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:00:02.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1250,"cached_input_tokens":260,"output_tokens":90}}}}"#,
            &range,
        );

        assert_eq!(parser.records.len(), 2);
        assert_eq!(
            parser
                .records
                .iter()
                .map(|record| (record.input, record.cached, record.output))
                .collect::<Vec<_>>(),
            vec![(1_000, 200, 50), (250, 60, 40)]
        );
        let totals = parser.previous_totals.expect("last totals");
        assert_eq!(totals.input, 1250);
        assert_eq!(totals.cached, 260);
        assert_eq!(totals.output, 90);
    }

    #[test]
    fn test_fast_codex_parser_reads_legacy_event_msg_shape() {
        let range = CostUsageDayRange::new(
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
        );
        let mut parser = CodexParserState::new(Some("gpt-5".to_string()), None);

        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:00:02.000Z","type":"event_msg","event_msg":{"type":"token_count","input_tokens":20,"cached_input_tokens":5,"output_tokens":3}}"#,
            &range,
        );

        assert_eq!(parser.records.len(), 1);
        let record = &parser.records[0];
        assert_eq!(record.model, "gpt-5");
        assert_eq!((record.input, record.cached, record.output), (20, 5, 3));
    }

    #[test]
    fn test_parse_codex_file_uses_fast_parser_for_current_logs() {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        writeln!(
            file,
            r#"{{"timestamp":"2026-05-31T10:00:00.000Z","type":"turn_context","payload":{{"model":"gpt-5.5"}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"timestamp":"2026-05-31T10:00:01.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":45,"cached_input_tokens":12,"output_tokens":8}}}}}}}}"#
        )
        .unwrap();

        let range = CostUsageDayRange::new(
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
        );
        let parsed =
            JsonlScanner::parse_codex_file(file.path(), &range, 0, None, None).expect("parse");

        assert_eq!(parsed.last_model.as_deref(), Some("gpt-5.5"));
        assert_eq!(parsed.records.len(), 1);
        let record = &parsed.records[0];
        assert_eq!(record.day_key, "2026-05-31");
        assert_eq!(record.model, "gpt-5.5");
        assert_eq!((record.input, record.cached, record.output), (45, 12, 8));
    }

    #[test]
    fn codex_parser_discards_oversized_line_and_recovers_next_record() {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        let padding = "x".repeat(CODEX_JSONL_MAX_LINE_BYTES);
        writeln!(
            file,
            r#"{{"timestamp":"2026-05-31T10:00:00Z","type":"turn_context","payload":{{"model":"{padding}"}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"timestamp":"2026-05-31T10:00:01Z","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":9,"cached_input_tokens":2,"output_tokens":1}}}}}}}}"#
        )
        .unwrap();

        let day = NaiveDate::from_ymd_opt(2026, 5, 31).unwrap();
        let parsed = JsonlScanner::parse_codex_file(
            file.path(),
            &CostUsageDayRange::new(day, day),
            0,
            None,
            None,
        )
        .expect("parse");

        assert_eq!(parsed.records.len(), 1);
        assert_eq!(
            parsed.records[0].model,
            CostUsagePricing::CODEX_UNATTRIBUTED_MODEL
        );
        assert_eq!(
            (
                parsed.records[0].input,
                parsed.records[0].cached,
                parsed.records[0].output
            ),
            (9, 2, 1)
        );
    }

    #[test]
    fn bounded_jsonl_reader_accepts_exact_limit_without_retaining_larger_input() {
        let mut input = vec![b'x'; CODEX_JSONL_MAX_LINE_BYTES];
        input.push(b'\n');
        input.extend_from_slice(b"{\"type\":\"event_msg\"}\n");
        let mut reader = BufReader::with_capacity(64 * 1024, std::io::Cursor::new(input));

        let (exact, _) = read_bounded_jsonl_line(&mut reader, CODEX_JSONL_MAX_LINE_BYTES)
            .expect("read")
            .expect("line");
        let (later, _) = read_bounded_jsonl_line(&mut reader, CODEX_JSONL_MAX_LINE_BYTES)
            .expect("read")
            .expect("line");

        assert_eq!(exact.len(), CODEX_JSONL_MAX_LINE_BYTES);
        assert_eq!(later, br#"{"type":"event_msg"}"#);
    }

    #[test]
    fn codex_turn_context_wins_over_conflicting_event_model() {
        let day = NaiveDate::from_ymd_opt(2026, 5, 31).unwrap();
        let range = CostUsageDayRange::new(day, day);
        let mut parser = CodexParserState::new(None, None);
        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:00:00Z","type":"turn_context","payload":{"model":"gpt-5.5"}}"#,
            &range,
        );
        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:00:01Z","type":"event_msg","payload":{"type":"token_count","model":"gpt-5.6-sol","info":{"last_token_usage":{"input_tokens":5,"cached_input_tokens":1,"output_tokens":2}}}}"#,
            &range,
        );

        assert_eq!(parser.records[0].model, "gpt-5.5");
    }

    #[test]
    fn codex_blank_context_clears_stale_model_and_emits_unattributed_usage() {
        let day = NaiveDate::from_ymd_opt(2026, 5, 31).unwrap();
        let range = CostUsageDayRange::new(day, day);
        let mut parser = CodexParserState::new(Some("gpt-5.5".to_string()), None);
        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:00:00Z","type":"turn_context","payload":{"model":" "}}"#,
            &range,
        );
        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":5,"cached_input_tokens":1,"output_tokens":2}}}}"#,
            &range,
        );

        assert_eq!(
            parser.records[0].model,
            CostUsagePricing::CODEX_UNATTRIBUTED_MODEL
        );
    }

    #[test]
    fn codex_model_less_token_event_uses_unpriced_sentinel() {
        let day = NaiveDate::from_ymd_opt(2026, 5, 31).unwrap();
        let range = CostUsageDayRange::new(day, day);
        let mut parser = CodexParserState::new(None, None);
        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":10,"cached_input_tokens":0,"output_tokens":2}}}}"#,
            &range,
        );

        assert_eq!(parser.records.len(), 1);
        assert_eq!(
            parser.records[0].model,
            CostUsagePricing::CODEX_UNATTRIBUTED_MODEL
        );
    }

    #[test]
    fn interleaved_lineage_totals_never_exceed_high_watermark_growth() {
        let day = NaiveDate::from_ymd_opt(2026, 5, 31).unwrap();
        let range = CostUsageDayRange::new(day, day);
        let mut parser = CodexParserState::new(Some("gpt-5.6-sol".to_string()), None);

        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":20}}}}"#,
            &range,
        );
        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":5,"cached_input_tokens":0,"output_tokens":1}}}}"#,
            &range,
        );
        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":101,"cached_input_tokens":0,"output_tokens":21}}}}"#,
            &range,
        );

        let total_input: i32 = parser.records.iter().map(|r| r.input).sum();
        let total_output: i32 = parser.records.iter().map(|r| r.output).sum();
        assert!(
            total_input <= 101,
            "input inflated to {total_input}, expected <= 101"
        );
        assert!(
            total_output <= 21,
            "output inflated to {total_output}, expected <= 21"
        );
    }

    #[test]
    fn interleaved_lineage_mid_range_climb_below_watermark_does_not_readd() {
        // 100 → 5 (rewind) → 80 (mid-range below water) → 101 (above water).
        // Phase-1 containment: do not re-add the 5→80 climb; only growth above
        // the historical high watermark counts.
        let day = NaiveDate::from_ymd_opt(2026, 5, 31).unwrap();
        let range = CostUsageDayRange::new(day, day);
        let mut parser = CodexParserState::new(Some("gpt-5.6-sol".to_string()), None);

        for (input, output) in [(100, 20), (5, 1), (80, 10), (101, 21)] {
            parser.process_line(
                &format!(
                    r#"{{"timestamp":"2026-05-31T10:00:0{input}Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":0,"output_tokens":{output}}}}}}}"#
                ),
                &range,
            );
        }

        let total_input: i32 = parser.records.iter().map(|r| r.input).sum();
        let total_output: i32 = parser.records.iter().map(|r| r.output).sum();
        assert!(
            total_input <= 101,
            "mid-range climb re-added input to {total_input}, expected <= 101"
        );
        assert!(
            total_output <= 21,
            "mid-range climb re-added output to {total_output}, expected <= 21"
        );
    }

    #[test]
    fn cost_scan_options_app_driven_bypasses_debounce() {
        let debounced = CostScanOptions::default();
        let forced = CostScanOptions::app_driven();
        let last = 1_000_000_i64;
        let now = last + 1_000; // 1s later, within 60s window

        assert!(debounced.should_skip_scan(last, now));
        assert!(!forced.should_skip_scan(last, now));
        assert!(!debounced.should_skip_scan(last, last + 61_000));

        let cache = CostUsageCache {
            last_scan_unix_ms: last,
            ..Default::default()
        };
        assert!(JsonlScanner::should_skip_cached_scan(
            &cache,
            CostScanOptions::default(),
            now
        ));
        assert!(!JsonlScanner::should_skip_cached_scan(
            &cache,
            CostScanOptions::app_driven(),
            now
        ));
    }
}
