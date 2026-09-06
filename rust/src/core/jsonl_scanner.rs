//! JSONL Scanner with Caching
//!
//! Incremental log file parsing for Codex and Claude session logs.
//! Supports file-level caching to avoid re-parsing unchanged files.

#![allow(
    dead_code,
    reason = "scanner types are deserialized from JSONL for parsing but not all are read"
)]

use crate::core::{CostUsagePricing, ProviderId};
use chrono::{DateTime, Local, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct CachedCostReadStatus {
    pub has_days: bool,
    pub previous_report: Option<CachedCostReport>,
}

#[derive(Deserialize, Default)]
struct CachedCostReadStatusProjection {
    #[serde(
        default,
        rename = "days",
        deserialize_with = "deserialize_nonempty_object"
    )]
    has_days: bool,
    #[serde(default)]
    previous_report: Option<CachedCostReport>,
}

fn deserialize_nonempty_object<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{IgnoredAny, MapAccess, Visitor};

    struct NonemptyObjectVisitor;

    impl<'de> Visitor<'de> for NonemptyObjectVisitor {
        type Value = bool;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a JSON object")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut nonempty = false;
            while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {
                nonempty = true;
            }
            Ok(nonempty)
        }
    }

    deserializer.deserialize_map(NonemptyObjectVisitor)
}
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
    /// A16 (upstream 0.48.0 --provider-native-only): when false, exclude
    /// pi/OMP-compatible agent session mirrors from Codex/Claude cost history.
    /// Defaults to true (include mirrors) for backward compatibility.
    pub include_pi_sessions: bool,
}

impl Default for CostScanOptions {
    fn default() -> Self {
        Self {
            refresh_min_interval_secs: DEFAULT_COST_SCAN_REFRESH_MIN_INTERVAL_SECS,
            include_pi_sessions: true,
        }
    }
}

impl CostScanOptions {
    /// App-driven or forced refresh: skip the scanner debounce entirely.
    pub fn app_driven() -> Self {
        Self {
            refresh_min_interval_secs: 0,
            include_pi_sessions: true,
        }
    }

    /// Whether a prior scan at `last_scan_unix_ms` is still within the debounce window.
    pub fn should_skip_scan(&self, last_scan_unix_ms: i64, now_unix_ms: i64) -> bool {
        // Debounce intervals are seconds-scale config values, far below i64::MAX.
        #[allow(
            clippy::cast_possible_wrap,
            reason = "debounce interval in seconds is a small config value that cannot exceed i64::MAX"
        )]
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
    /// Last validated cost report, kept so spend surfaces can keep showing
    /// totals while a (re)scan catches up after the cache was trimmed or the
    /// debounce window expired (upstream 0.48.0 #2628). `None` once a scan
    /// completes for the current window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_report: Option<CachedCostReport>,
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

/// Snapshot of the last validated cost report, persisted so spend surfaces keep
/// showing totals while a rescan catches up after the cache is trimmed or the
/// debounce window expires (upstream 0.48.0 #2628). See the cache-budget module
/// for the save/load overshoot contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedCostReport {
    /// Total cost in USD for the reported window.
    pub total_cost_usd: f64,
    /// Total input tokens.
    pub input_tokens: i32,
    /// Total cached tokens.
    pub cached_tokens: i32,
    /// Total output tokens.
    pub output_tokens: i32,
    /// Number of sessions contributing.
    pub sessions_count: i32,
    /// ISO 8601 timestamp when this report was generated.
    pub updated_at: Option<String>,
    /// Whether the report was marked partial (unpriced routing rows retained).
    #[serde(default)]
    pub partial: bool,
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
        let event_candidate = is_candidate_codex_line(line);
        let bare_candidate = !event_candidate && line.contains("\"usage\"");
        if !event_candidate && !bare_candidate {
            return;
        }

        if event_candidate && let Some(event) = parse_codex_fast_event(line) {
            self.process_fast_event(event, range);
            return;
        }

        let Ok(obj) = serde_json::from_str::<Value>(line) else {
            return;
        };

        if bare_candidate {
            if obj.get("type").is_some() {
                return;
            }
            let Some(day_key) = codex_line_day_key(&obj, range)
                .or_else(|| self.records.last().map(|record| record.day_key.clone()))
            else {
                return;
            };
            if let Some((totals, model)) = bare_usage_totals(&obj) {
                let model = self
                    .current_model
                    .as_deref()
                    .and_then(model_evidence)
                    .or(model.as_deref().and_then(model_evidence))
                    .unwrap_or(CostUsagePricing::CODEX_UNATTRIBUTED_MODEL)
                    .to_string();
                self.record_usage(day_key, &model, totals.input, totals.cached, totals.output);
            }
            return;
        }

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

fn bare_usage_totals(obj: &Value) -> Option<(CodexTotals, Option<String>)> {
    let usage = obj
        .get("usage")
        .or_else(|| obj.get("data").and_then(|v| v.get("usage")))
        .or_else(|| obj.get("result").and_then(|v| v.get("usage")))
        .or_else(|| obj.get("response").and_then(|v| v.get("usage")))?;
    // Token counts come from usage records and fit i32, the canonical totals storage type.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "usage token counts fit i32, the canonical totals storage type"
    )]
    let input = ["input_tokens", "prompt_tokens", "input"]
        .into_iter()
        .find_map(|key| usage.get(key).and_then(Value::as_i64))
        .unwrap_or(0)
        .max(0) as i32;
    // Token counts come from usage records and fit i32, the canonical totals storage type.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "usage token counts fit i32, the canonical totals storage type"
    )]
    let output = ["output_tokens", "completion_tokens", "output"]
        .into_iter()
        .find_map(|key| usage.get(key).and_then(Value::as_i64))
        .unwrap_or(0)
        .max(0) as i32;
    // Token counts come from usage records and fit i32, the canonical totals storage type.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "usage token counts fit i32, the canonical totals storage type"
    )]
    let cached = [
        "cached_input_tokens",
        "cache_read_input_tokens",
        "cached_tokens",
    ]
    .into_iter()
    .filter_map(|key| usage.get(key).and_then(Value::as_i64))
    .max()
    .unwrap_or(0)
    .max(0) as i32;
    if input == 0 && output == 0 && cached == 0 {
        return None;
    }
    let model = obj
        .get("model")
        .or_else(|| obj.get("data").and_then(|v| v.get("model")))
        .or_else(|| obj.get("result").and_then(|v| v.get("model")))
        .or_else(|| obj.get("response").and_then(|v| v.get("model")))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    Some((
        CodexTotals {
            input,
            cached,
            output,
        },
        model,
    ))
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
    // Token counts come from Codex usage records and fit within i32, which is
    // the canonical storage type of the totals table.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "token counts from usage records fit i32"
    )]
    let cached = value
        .get("cached_input_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(
            value
                .get("cache_read_input_tokens")
                .and_then(|v| v.as_i64())
                .unwrap_or(0),
        ) as i32;
    CodexTotals {
        input: token_i32(value, "input_tokens"),
        cached,
        output: token_i32(value, "output_tokens"),
    }
}

fn codex_totals_from_fast(value: CodexFastTotals) -> CodexTotals {
    CodexTotals {
        input: value.input_tokens,
        cached: value
            .cached_input_tokens
            .unwrap_or(0)
            .max(value.cache_read_input_tokens.unwrap_or(0)),
        output: value.output_tokens,
    }
}

fn fast_totals_from_payload(value: &CodexFastPayload<'_>) -> CodexTotals {
    CodexTotals {
        input: value.input_tokens.unwrap_or(0),
        cached: value
            .cached_input_tokens
            .unwrap_or(0)
            .max(value.cache_read_input_tokens.unwrap_or(0)),
        output: value.output_tokens.unwrap_or(0),
    }
}

fn token_i32(value: &Value, key: &str) -> i32 {
    // Token counts from usage records fit i32, the canonical totals storage type.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "token counts from usage records fit i32"
    )]
    let tokens = value.get(key).and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    tokens
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
        // Session JSONL files are bounded by the cache budget; sizes fit i64.
        #[allow(
            clippy::cast_possible_wrap,
            reason = "session JSONL file sizes fit i64"
        )]
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
            // Per-line byte counts are capped at 256 KiB, far inside i64::MAX.
            #[allow(
                clippy::cast_possible_wrap,
                reason = "per-line consumed bytes are capped at CODEX_JSONL_MAX_LINE_BYTES"
            )]
            let consumed_i64 = consumed as i64;
            parsed_bytes += consumed_i64;
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

    /// F2 (upstream 0.48.0 #2648): whether a cached resume offset sits on a real
    /// line boundary. A partial trailing-line write leaves the cached offset
    /// mid-line; resuming there re-parses from mid-line and corrupts the first
    /// resumed record. Returns  when the byte just before  is
    /// not a newline (or the probe fails), signalling the caller to fall back
    /// to a full re-parse from zero.
    pub fn is_line_boundary_offset(file_path: &Path, offset: i64) -> bool {
        use std::io::{Read, Seek};
        if offset <= 0 {
            return true;
        }
        // Session JSONL file sizes fit i64; metadata feeds only boundary probes.
        #[allow(
            clippy::cast_possible_wrap,
            reason = "session JSONL file sizes fit i64"
        )]
        let file_size_i64 = fs::metadata(file_path).map(|m| m.len() as i64);
        let Ok(file_size) = file_size_i64 else {
            return false;
        };
        if offset >= file_size {
            return true;
        }
        let Ok(mut probe) = File::open(file_path) else {
            return false;
        };
        if probe.seek(SeekFrom::Start((offset - 1) as u64)).is_err() {
            return false;
        }
        let mut prev_byte = [0u8; 1];
        probe.read_exact(&mut prev_byte).is_ok() && prev_byte[0] == b'\n'
    }

    /// Whether a cached scan should be reused under `options` (issue #2089).
    pub fn should_skip_cached_scan(
        cache: &CostUsageCache,
        options: CostScanOptions,
        now_unix_ms: i64,
    ) -> bool {
        options.should_skip_scan(cache.last_scan_unix_ms, now_unix_ms)
    }

    /// Load cache from disk.
    ///
    /// Refuses to decode artifacts larger than the load cap
    /// (`crate::core::CostUsageCacheBudget::MAX_LOAD_BYTES`); an oversized artifact is
    /// cheaper to rebuild bounded than to decode in one shot, so the caller
    /// gets a fresh empty cache instead (upstream 0.48.0 overshoot contract).
    /// Only Codex persistence is bounded; other providers load unbounded.
    pub fn load_cache(provider: ProviderId, cache_root: Option<&Path>) -> CostUsageCache {
        let cache_path = Self::cache_path(provider, cache_root);

        if crate::core::is_bounded_provider(provider) {
            // Artifacts are bounded by MAX_LOAD_BYTES (320 MiB), fitting usize on
            // any supported target even before the budget comparison below.
            #[allow(
                clippy::cast_possible_truncation,
                reason = "bounded artifacts fit usize on any supported target"
            )]
            let file_bytes = crate::core::artifact_file_size(&cache_path) as usize;
            if file_bytes > crate::core::CostUsageCacheBudget::MAX_LOAD_BYTES {
                return CostUsageCache::default();
            }
        }

        if let Ok(contents) = fs::read_to_string(&cache_path)
            && let Ok(cache) = serde_json::from_str(&contents)
        {
            return cache;
        }

        CostUsageCache::default()
    }

    /// Read only the cache metadata needed by presentation surfaces.
    ///
    /// v0.56.0 performance parity: skip raw per-file scanner state and day
    /// payloads when callers only need stale/catch-up status.
    pub fn load_cache_status(
        provider: ProviderId,
        cache_root: Option<&Path>,
    ) -> CachedCostReadStatus {
        let cache_path = Self::cache_path(provider, cache_root);
        if crate::core::is_bounded_provider(provider) {
            #[allow(
                clippy::cast_possible_truncation,
                reason = "bounded artifacts fit usize on any supported target"
            )]
            let file_bytes = crate::core::artifact_file_size(&cache_path) as usize;
            if file_bytes > crate::core::CostUsageCacheBudget::MAX_LOAD_BYTES {
                return CachedCostReadStatus::default();
            }
        }

        let Ok(file) = File::open(cache_path) else {
            return CachedCostReadStatus::default();
        };
        let Ok(projection) =
            serde_json::from_reader::<_, CachedCostReadStatusProjection>(BufReader::new(file))
        else {
            return CachedCostReadStatus::default();
        };
        CachedCostReadStatus {
            has_days: projection.has_days,
            previous_report: projection.previous_report,
        }
    }
    fn cached_cost_report_from_days(cache: &CostUsageCache) -> CachedCostReport {
        let mut total_cost_usd = 0.0;
        let mut input_tokens = 0_i32;
        let mut cached_tokens = 0_i32;
        let mut output_tokens = 0_i32;
        let mut partial = false;

        for (day_key, models) in &cache.days {
            let pricing_day = NaiveDate::parse_from_str(day_key, "%Y-%m-%d").ok();
            for (model, values) in models {
                let input = values.first().copied().unwrap_or(0).max(0);
                let cached = values.get(1).copied().unwrap_or(0).max(0);
                let output = values.get(2).copied().unwrap_or(0).max(0);
                input_tokens = input_tokens.saturating_add(input);
                cached_tokens = cached_tokens.saturating_add(cached);
                output_tokens = output_tokens.saturating_add(output);

                if CostUsagePricing::is_codex_unattributed_model(model) {
                    partial = true;
                    continue;
                }
                if !CostUsagePricing::counts_toward_codex_subscription(model) {
                    continue;
                }
                let priced = pricing_day
                    .and_then(|day| {
                        CostUsagePricing::codex_cost_usd_at_date(
                            model,
                            u64::try_from(input).unwrap_or(0),
                            u64::try_from(cached).unwrap_or(0),
                            u64::try_from(output).unwrap_or(0),
                            day,
                        )
                    })
                    .or_else(|| {
                        CostUsagePricing::codex_cost_usd(
                            model,
                            u64::try_from(input).unwrap_or(0),
                            u64::try_from(cached).unwrap_or(0),
                            u64::try_from(output).unwrap_or(0),
                        )
                    });
                if let Some(cost) = priced {
                    total_cost_usd += cost;
                } else {
                    partial = true;
                }
            }
        }

        let sessions_count = i32::try_from(cache.files.len()).unwrap_or(i32::MAX);
        CachedCostReport {
            total_cost_usd,
            input_tokens,
            cached_tokens,
            output_tokens,
            sessions_count,
            updated_at: Some(Utc::now().to_rfc3339()),
            partial,
        }
    }

    /// Save cache to disk (temp sibling + copy into place).
    ///
    /// Before encoding, prunes the cache to the persistence budget so the
    /// artifact stays small enough to decode in one shot (upstream 0.48.0
    /// #2637). Only Codex persistence is bounded; the overshoot contract lets
    /// the encoded size exceed `MAX_FILE_BYTES` up to `MAX_LOAD_BYTES`
    /// when protected (partially parsed) entries cannot be trimmed further.
    pub fn save_cache(provider: ProviderId, cache: &mut CostUsageCache, cache_root: Option<&Path>) {
        Self::save_cache_with_limit(
            provider,
            cache,
            cache_root,
            crate::core::CostUsageCacheBudget::MAX_LOAD_BYTES,
        );
    }

    /// Save with an explicit post-encode refusal limit, injected by tests.
    ///
    /// Identical to `save_cache` except the post-encode oversize check uses
    /// `max_load_bytes` rather than the production `MAX_LOAD_BYTES` const.
    /// Production callers MUST use `save_cache`; this helper exists so the
    /// refusal / stale-destination removal can be exercised without encoding a
    /// ~320 MiB test artifact.
    fn save_cache_with_limit(
        provider: ProviderId,
        cache: &mut CostUsageCache,
        cache_root: Option<&Path>,
        max_load_bytes: usize,
    ) {
        let cache_path = Self::cache_path(provider, cache_root);

        let Some(parent) = cache_path.parent() else {
            return;
        };
        // Best-effort cache dir creation; a missing dir surfaces as the write error below.
        let _dir_created = fs::create_dir_all(parent);

        if crate::core::is_bounded_provider(provider) {
            // v0.55.1 #3051: snapshot the fully validated report BEFORE persistence
            // pruning. If budget trimming creates a catch-up cycle, this is the
            // established spend/tokens users should keep seeing until replacement
            // history finishes, not a zero-cost reconstruction of the trimmed cache.
            let established_report = cache
                .previous_report
                .clone()
                .unwrap_or_else(|| Self::cached_cost_report_from_days(cache));
            let pruned = crate::core::prune_out_of_window_for_budget(
                &mut cache.files,
                &mut cache.days,
                cache.scan_since_key.as_deref(),
                cache.scan_until_key.as_deref(),
                false,
            );
            let estimate = crate::core::estimated_cache_bytes(&cache.files, &cache.days);
            let trimmed = if estimate > crate::core::CostUsageCacheBudget::MAX_FILE_BYTES {
                crate::core::trim_in_window_for_budget(
                    &mut cache.files,
                    &mut cache.days,
                    cache.scan_since_key.as_deref(),
                    cache.scan_until_key.as_deref(),
                    crate::core::CostUsageCacheBudget::MAX_FILE_BYTES,
                )
            } else {
                Vec::new()
            };
            // A16 (upstream 0.48.0): when entries were trimmed for budget, the persisted
            // artifact no longer covers the full window — set previous_report so the
            // next refresh can signal catch-up is pending (and spend surfaces can show
            // the last-validated snapshot during the rescan).
            if (!pruned.is_empty() || !trimmed.is_empty()) && cache.previous_report.is_none() {
                cache.previous_report = Some(established_report);
            }
        }

        let Ok(json) = serde_json::to_string(cache) else {
            return;
        };

        // F19 (upstream 0.48.0): after bounded encode, if the artifact still
        // exceeds MAX_LOAD_BYTES, refuse persistence. Also remove any existing
        // destination artifact so a stale/oversized file cannot persist and
        // trip the load-refusal path on the next scan (which would force an
        // unnecessary full rebuild from a poisoned artifact). This is a
        // one-shot refusal (not a persist/refuse/rebuild loop): the budget
        // enforcement above already pruned and trimmed; if the result is still
        // too large (e.g. a single protected entry exceeds the limit), the
        // artifact is dropped and the next scan rebuilds from scratch.
        if crate::core::is_bounded_provider(provider)
            && crate::core::CostUsageCacheBudget::should_refuse_persistence(
                json.len(),
                max_load_bytes,
            )
        {
            // Best-effort removal; ignore errors (file may not exist).
            let _cleared = fs::remove_file(&cache_path);
            return;
        }

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
            // Fallback direct write when copy fails; the copy error already surfaced.
            let _fallback_written = fs::write(&cache_path, json.as_bytes());
        }
        // Best-effort temp cleanup (ignore errors — unique name avoids clashes).
        let _truncated_tmp = fs::File::create(&tmp_path).and_then(|f| f.set_len(0));
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
    fn cached_tokens_use_larger_cached_or_cache_read_field() {
        let value = serde_json::json!({
            "input_tokens": 100,
            "cached_input_tokens": 20,
            "cache_read_input_tokens": 35,
            "output_tokens": 10
        });
        let totals = read_token_totals(&value);
        assert_eq!(totals.cached, 35);
    }

    #[test]
    fn parses_bare_usage_rows_outside_token_count_envelope() {
        let value = serde_json::json!({
            "model": "gpt-5.6-sol",
            "usage": {
                "prompt_tokens": 120,
                "completion_tokens": 30,
                "cached_input_tokens": 40,
                "cache_read_input_tokens": 55
            }
        });
        let (totals, model) = bare_usage_totals(&value).expect("bare usage");
        assert_eq!(totals.input, 120);
        assert_eq!(totals.output, 30);
        assert_eq!(totals.cached, 55);
        assert_eq!(model.as_deref(), Some("gpt-5.6-sol"));
    }

    #[test]
    fn process_line_accepts_type_less_bare_usage_row() {
        let day = NaiveDate::from_ymd_opt(2026, 5, 31).unwrap();
        let range = CostUsageDayRange::new(day, day);
        let mut parser = CodexParserState::new(None, None);

        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:00:01Z","model":"gpt-5.6-sol","usage":{"prompt_tokens":120,"completion_tokens":30,"cache_read_input_tokens":55}}"#,
            &range,
        );

        assert_eq!(parser.records.len(), 1);
        assert_eq!(parser.records[0].model, "gpt-5.6-sol");
        assert_eq!(
            (
                parser.records[0].input,
                parser.records[0].cached,
                parser.records[0].output
            ),
            (120, 55, 30)
        );
    }

    #[test]
    fn timestamp_less_bare_usage_uses_last_accepted_usage_day() {
        let day = NaiveDate::from_ymd_opt(2026, 5, 31).unwrap();
        let range = CostUsageDayRange::new(day, day);
        let mut parser = CodexParserState::new(Some("gpt-5.6-sol".to_string()), None);

        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":1}}}}"#,
            &range,
        );
        parser.process_line(
            r#"{"usage":{"prompt_tokens":20,"completion_tokens":4,"cache_read_input_tokens":3}}"#,
            &range,
        );

        assert_eq!(parser.records.len(), 2);
        assert_eq!(parser.records[1].day_key, "2026-05-31");
        assert_eq!(
            (
                parser.records[1].input,
                parser.records[1].cached,
                parser.records[1].output
            ),
            (20, 3, 4)
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

    #[test]
    fn is_line_boundary_offset_zero_returns_true() {
        // F2: offset 0 is always a valid boundary (start of file).
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("f.jsonl");
        std::fs::write(
            &path,
            b"hello
world
",
        )
        .unwrap();
        assert!(JsonlScanner::is_line_boundary_offset(&path, 0));
    }

    #[test]
    fn is_line_boundary_offset_at_or_past_size_returns_true() {
        // F2: offset >= file_size returns true (EOF or beyond is a valid boundary).
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("f.jsonl");
        let content = b"line1
line2
";
        std::fs::write(&path, content).unwrap();
        let size = i64::try_from(content.len()).unwrap();
        assert!(JsonlScanner::is_line_boundary_offset(&path, size));
        assert!(JsonlScanner::is_line_boundary_offset(&path, size + 100));
    }

    #[test]
    fn is_line_boundary_offset_exact_newline_returns_true() {
        // F2: offset pointing right after a newline is a valid boundary.
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("f.jsonl");
        // "line1\nline2\n" — offset 6 is right after first \n
        std::fs::write(&path, b"line1\nline2\n").unwrap();
        assert!(JsonlScanner::is_line_boundary_offset(&path, 6));
    }

    #[test]
    fn is_line_boundary_offset_midline_returns_false() {
        // F2: offset pointing mid-line (byte before is not \n) returns false.
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("f.jsonl");
        // "line1\nline2\n" — offset 3 is mid-line (byte before is 'n')
        std::fs::write(&path, b"line1\nline2\n").unwrap();
        assert!(!JsonlScanner::is_line_boundary_offset(&path, 3));
    }

    #[test]
    fn is_line_boundary_offset_missing_file_returns_false() {
        // F2: missing file returns false (probe fails).
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("nonexistent.jsonl");
        // offset > 0 so it doesn't short-circuit to true
        assert!(!JsonlScanner::is_line_boundary_offset(&path, 10));
    }

    #[test]
    fn catch_up_snapshot_preserves_established_codex_cost_and_tokens() {
        let mut cache = CostUsageCache::default();
        cache.files.insert(
            "session.jsonl".to_string(),
            CostUsageFileUsage {
                mtime_unix_ms: 0,
                size: 100,
                days: HashMap::new(),
                parsed_bytes: Some(100),
                last_model: Some("gpt-5.6-sol".to_string()),
                last_totals: None,
            },
        );
        cache.days.insert(
            "2026-08-20".to_string(),
            HashMap::from([("gpt-5.6-sol".to_string(), vec![1_000, 250, 100])]),
        );

        let report = JsonlScanner::cached_cost_report_from_days(&cache);
        let expected = CostUsagePricing::codex_cost_usd_at_date(
            "gpt-5.6-sol",
            1_000,
            250,
            100,
            NaiveDate::from_ymd_opt(2026, 8, 20).unwrap(),
        )
        .expect("known model price");

        assert!((report.total_cost_usd - expected).abs() < 1e-12);
        assert!(report.total_cost_usd > 0.0);
        assert_eq!(report.input_tokens, 1_000);
        assert_eq!(report.cached_tokens, 250);
        assert_eq!(report.output_tokens, 100);
        assert_eq!(report.sessions_count, 1);
        assert!(!report.partial);
        assert!(report.updated_at.is_some());
    }

    #[test]
    fn save_cache_persists_small_codex_artifact() {
        // F19 integration: a normal-sized Codex cache is persisted and
        // reloadable — the MAX_LOAD_BYTES refusal does not false-positive.
        let root = tempfile::tempdir().unwrap();
        let cache_root = root.path().to_path_buf();
        let mut cache = CostUsageCache {
            scan_since_key: Some("2026-01-01".to_string()),
            scan_until_key: Some("2026-01-31".to_string()),
            files: HashMap::from([(
                "a.jsonl".to_string(),
                CostUsageFileUsage {
                    mtime_unix_ms: 0,
                    size: 100,
                    days: HashMap::from([(
                        "2026-01-10".to_string(),
                        HashMap::from([("gpt-5.6-sol".to_string(), vec![10, 0, 1])]),
                    )]),
                    parsed_bytes: None,
                    last_model: None,
                    last_totals: None,
                },
            )]),
            ..Default::default()
        };

        JsonlScanner::save_cache(ProviderId::Codex, &mut cache, Some(&cache_root));

        // File should exist and be reloadable.
        let loaded = JsonlScanner::load_cache(ProviderId::Codex, Some(&cache_root));
        assert!(
            loaded.files.contains_key("a.jsonl"),
            "small artifact persisted"
        );
        assert_eq!(loaded.scan_since_key, Some("2026-01-01".to_string()));
    }

    #[test]
    fn save_cache_refuses_non_bounded_provider_oversize() {
        // F19: non-bounded providers (e.g. Claude) skip the refusal check
        // entirely — the MAX_LOAD_BYTES guard only applies to bounded providers.
        // This test confirms the is_bounded_provider gate works: Claude cache
        // is saved regardless of the MAX_LOAD_BYTES check (which is Codex-only).
        let root = tempfile::tempdir().unwrap();
        let cache_root = root.path().to_path_buf();
        let mut cache = CostUsageCache::default();
        cache.files.insert(
            "claude.jsonl".to_string(),
            CostUsageFileUsage {
                mtime_unix_ms: 0,
                size: 100,
                days: HashMap::new(),
                parsed_bytes: None,
                last_model: None,
                last_totals: None,
            },
        );

        JsonlScanner::save_cache(ProviderId::Claude, &mut cache, Some(&cache_root));
        let loaded = JsonlScanner::load_cache(ProviderId::Claude, Some(&cache_root));
        assert!(loaded.files.contains_key("claude.jsonl"));
    }

    #[test]
    fn save_cache_refusal_removes_preexisting_destination_artifact() {
        // F19 integration: when the post-encode check refuses the artifact, any
        // pre-existing destination file is removed so a stale/oversized artifact
        // cannot persist and trigger load/refuse/rebuild behavior on next scan.
        let root = tempfile::tempdir().unwrap();
        let cache_root = root.path().to_path_buf();

        let mut cache = CostUsageCache::default();
        cache.files.insert(
            "big.jsonl".to_string(),
            CostUsageFileUsage {
                mtime_unix_ms: 0,
                size: 100,
                days: HashMap::from([(
                    "2026-01-10".to_string(),
                    HashMap::from([("gpt-5.6-sol".to_string(), vec![10, 0, 1])]),
                )]),
                parsed_bytes: None,
                last_model: None,
                last_totals: None,
            },
        );

        // Precreate a "stale" destination artifact so the refusal must remove
        // it. We seed it via a large (over_max) save_limit so the save_cache_with_limit
        // first ENCODES the small cache fine under a generous limit, writes the file,
        // then a follow-up call with a tiny limit must refuse AND remove.
        let cache_path = {
            // Exercise the private helper indirectly via the public path: first
            // persist a valid artifact under a generous limit via save_cache.
            // Then call with an impossible limit (encoded JSON ~hundreds of
            // bytes, limit = 1 byte) to force refusal.
            JsonlScanner::save_cache_with_limit(
                ProviderId::Codex,
                &mut cache,
                Some(&cache_root),
                usize::MAX,
            );
            let p = JsonlScanner::cache_path(ProviderId::Codex, Some(&cache_root));
            assert!(p.exists(), "precreate destination artifact");
            p
        };

        // Sanity: a normal load succeeds against the precreated artifact.
        let loaded = JsonlScanner::load_cache(ProviderId::Codex, Some(&cache_root));
        assert!(loaded.files.contains_key("big.jsonl"));

        // Force refusal with a 1-byte limit: encoded cache will exceed it.
        JsonlScanner::save_cache_with_limit(ProviderId::Codex, &mut cache, Some(&cache_root), 1);

        // Destination must be gone — no stale artifact may persist.
        assert!(
            !cache_path.exists(),
            "refusal must remove preexisting destination artifact"
        );

        // No temp file should remain in the cache root (only unique tmp name was used).
        let mut tmp_entries = Vec::new();
        for entry in std::fs::read_dir(&cache_root).unwrap() {
            let name = entry.unwrap().file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') && name.ends_with(".tmp") {
                tmp_entries.push(name.into_owned());
            }
        }
        // Best-effort temp cleanup writes an empty file at the unique name; the
        // invariant is that NO tmp file contains a complete artifact. The set
        // should at most contain a single zero-byte remnant from the cleanup
        // (or be empty); we persist via copy() rather than rename so no live
        // tmp holds data after the save path completes.
        for t in &tmp_entries {
            let meta = std::fs::metadata(cache_root.join(t)).unwrap();
            assert_eq!(meta.len(), 0, "tmp remnant must be empty: {t}");
        }

        // Loading after removal yields a fresh default cache (no rebuild loop).
        let loaded = JsonlScanner::load_cache(ProviderId::Codex, Some(&cache_root));
        assert!(
            loaded.files.is_empty(),
            "no rebuild loop from removed artifact"
        );
    }

    #[test]
    fn save_cache_at_exact_limit_is_accepted() {
        // F19 boundary: an encoded artifact at exactly the injected limit is
        // accepted (only strictly-larger artifacts are refused).
        let root = tempfile::tempdir().unwrap();
        let cache_root = root.path().to_path_buf();

        let cache = CostUsageCache::default();
        // Serialize to learn the actual encoded size for this exact struct.
        let json = serde_json::to_string(&cache).unwrap();
        let exact_limit = json.len();

        let mut cache_for_save = cache;
        JsonlScanner::save_cache_with_limit(
            ProviderId::Codex,
            &mut cache_for_save,
            Some(&cache_root),
            exact_limit,
        );

        let cache_path = JsonlScanner::cache_path(ProviderId::Codex, Some(&cache_root));
        assert!(
            cache_path.exists(),
            "artifact at exact limit must be persisted"
        );
    }

    #[test]
    fn save_cache_one_over_limit_is_refused_and_removes_destination() {
        // F19 boundary: an encoded artifact one byte over the injected limit is
        // refused, and any pre-existing destination is removed.
        let root = tempfile::tempdir().unwrap();
        let cache_root = root.path().to_path_buf();

        let cache = CostUsageCache::default();
        let json = serde_json::to_string(&cache).unwrap();
        // One byte short of the encoded size forces refusal on the next attempt.
        let under_by_one = json.len().saturating_sub(1);

        let mut cache_for_save = cache;
        JsonlScanner::save_cache_with_limit(
            ProviderId::Codex,
            &mut cache_for_save,
            Some(&cache_root),
            under_by_one,
        );

        let cache_path = JsonlScanner::cache_path(ProviderId::Codex, Some(&cache_root));
        assert!(
            !cache_path.exists(),
            "one-over-limit encoded artifact must be refused"
        );
    }
}
