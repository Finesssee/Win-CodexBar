//! Scan Codex rollout JSONL + read-only catalog → project usage snapshot.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};

use crate::agent_sessions::CodexRolloutFirstLineParser;
use crate::codex_costs::codex_period_start;
use crate::core::{CostUsageDayRange, CostUsagePricing, JsonlScanner, sha256_hex};

use super::sidecar::{SidecarError, WorkspaceUsageSidecar};
use super::types::{
    CodexLocalProjectUsageSnapshot, CostEstimate, DailyPoint, Progress, ProgressPhase,
    ProjectUsage, SessionUsage, SourceStatus, UsageTotals,
};
use super::{CHATS_DISPLAY_NAME, CHATS_PROJECT_ID};

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("codex home could not be resolved")]
    HomeUnresolved,
    #[error(transparent)]
    Sidecar(#[from] SidecarError),
    #[error("workspaces index io failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Resolved local Codex sources for one home scope.
#[derive(Debug, Clone)]
pub struct CodexLocalDataScope {
    pub identifier: String,
    pub codex_home: PathBuf,
    pub sessions_root: PathBuf,
    pub archived_sessions_root: PathBuf,
    pub state_database: PathBuf,
}

impl CodexLocalDataScope {
    /// `CODEX_HOME` → `CODEX_SQLITE_HOME` → `~/.codex`.
    pub fn resolve() -> Option<Self> {
        let home = non_empty_env("CODEX_HOME")
            .or_else(|| non_empty_env("CODEX_SQLITE_HOME"))
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".codex")))?;
        Some(Self::from_home(home))
    }

    pub fn from_home(codex_home: PathBuf) -> Self {
        let codex_home = normalize_path(&codex_home);
        let path_str = codex_home.to_string_lossy();
        Self {
            identifier: format!("codex-workspaces:{}", scope_fingerprint(&path_str)),
            sessions_root: codex_home.join("sessions"),
            archived_sessions_root: codex_home.join("archived_sessions"),
            state_database: codex_home.join("state_5.sqlite"),
            codex_home,
        }
    }

    pub fn scope_signature(&self) -> &str {
        &self.identifier
    }
}

/// Public workspaces index API.
#[derive(Debug, Clone)]
pub struct CodexWorkspacesIndex {
    history_days: u32,
    codex_home_override: Option<PathBuf>,
    sidecar_path_override: Option<PathBuf>,
}

impl Default for CodexWorkspacesIndex {
    fn default() -> Self {
        Self::new(30)
    }
}

impl CodexWorkspacesIndex {
    pub fn new(history_days: u32) -> Self {
        Self {
            history_days: history_days.clamp(1, 365),
            codex_home_override: None,
            sidecar_path_override: None,
        }
    }

    pub fn with_codex_home(mut self, home: impl Into<PathBuf>) -> Self {
        self.codex_home_override = Some(home.into());
        self
    }

    pub fn with_sidecar_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.sidecar_path_override = Some(path.into());
        self
    }

    pub fn history_days(&self) -> u32 {
        self.history_days
    }

    fn scope(&self) -> Result<CodexLocalDataScope, IndexError> {
        if let Some(home) = &self.codex_home_override {
            return Ok(CodexLocalDataScope::from_home(home.clone()));
        }
        CodexLocalDataScope::resolve().ok_or(IndexError::HomeUnresolved)
    }

    fn sidecar(&self) -> Result<WorkspaceUsageSidecar, IndexError> {
        let path = if let Some(path) = &self.sidecar_path_override {
            path.clone()
        } else {
            WorkspaceUsageSidecar::default_path().ok_or(SidecarError::PathUnavailable)?
        };
        Ok(WorkspaceUsageSidecar::new(path))
    }

    pub fn load_cached_snapshot(
        &self,
    ) -> Result<Option<CodexLocalProjectUsageSnapshot>, IndexError> {
        let scope = self.scope()?;
        let sidecar = self.sidecar()?;
        Ok(sidecar.load_latest_snapshot(scope.scope_signature(), self.history_days)?)
    }

    pub fn clear_cached_snapshot(&self) -> Result<(), IndexError> {
        let sidecar = self.sidecar()?;
        sidecar.clear_snapshots()?;
        Ok(())
    }

    pub fn load_snapshot<F>(
        &self,
        force_refresh: bool,
        mut progress: F,
    ) -> Result<CodexLocalProjectUsageSnapshot, IndexError>
    where
        F: FnMut(Progress),
    {
        let scope = self.scope()?;
        let sidecar = self.sidecar()?;
        let source_status = read_catalog_status(&scope.state_database);

        if !force_refresh
            && let Ok(Some(cached)) =
                sidecar.load_latest_snapshot(scope.scope_signature(), self.history_days)
        {
            let mut snap = cached;
            snap.source_status = source_status;
            return Ok(snap);
        }

        progress(Progress::phase(ProgressPhase::ScanningLogs));
        let today = Local::now().date_naive();
        let since = codex_period_start(today, self.history_days);
        let range = CostUsageDayRange::new(since, today);

        let mut files = list_session_files(&scope.sessions_root, &range);
        files.extend(list_session_files(&scope.archived_sessions_root, &range));
        files.sort();
        files.dedup();

        let total_file_count = files.len() as u32;
        progress(Progress {
            phase: ProgressPhase::ScanningLogs,
            processed_file_count: Some(0),
            total_file_count: Some(total_file_count),
            indexed_file_count: 0,
            skipped_file_count: 0,
        });

        progress(Progress::phase(ProgressPhase::IndexingProjects));
        let mut session_buckets: HashMap<String, SessionBucket> = HashMap::new();
        let mut daily_acc: HashMap<String, DailyAcc> = HashMap::new();
        let mut indexed = 0u32;
        let mut skipped = 0u32;

        for (idx, path) in files.iter().enumerate() {
            match index_one_file(path, &range) {
                Some(parsed) => {
                    merge_daily(&mut daily_acc, &parsed.day_models);
                    let entry = session_buckets
                        .entry(parsed.session_id.clone())
                        .or_insert_with(|| SessionBucket::new(&parsed));
                    entry.merge(parsed);
                    indexed = indexed.saturating_add(1);
                }
                None => {
                    skipped = skipped.saturating_add(1);
                }
            }
            if idx == 0 || (idx + 1) % 25 == 0 || idx + 1 == files.len() {
                progress(Progress {
                    phase: ProgressPhase::IndexingProjects,
                    processed_file_count: Some((idx + 1) as u32),
                    total_file_count: Some(total_file_count),
                    indexed_file_count: indexed,
                    skipped_file_count: skipped,
                });
            }
        }

        let mut projects = build_projects(&session_buckets);
        projects.sort_by(|a, b| {
            b.latest_activity
                .cmp(&a.latest_activity)
                .then_with(|| a.display_name.cmp(&b.display_name))
        });

        let mut total = UsageTotals::default();
        for p in &projects {
            total.add_assign(&p.totals);
        }

        let mut daily: Vec<DailyPoint> = daily_acc
            .into_iter()
            .map(|(day, acc)| DailyPoint {
                day,
                total_tokens: acc.total_tokens,
                cached_input_tokens: acc.cached_input_tokens,
                estimated_cost_usd: if acc.unknown_tokens == 0 || acc.known_usd > 0.0 {
                    Some(acc.known_usd)
                } else {
                    None
                },
            })
            .collect();
        daily.sort_by(|a, b| a.day.cmp(&b.day));

        let snapshot = CodexLocalProjectUsageSnapshot {
            updated_at: Utc::now(),
            history_days: self.history_days,
            scope_signature: scope.scope_signature().to_string(),
            indexed_file_count: indexed,
            skipped_file_count: skipped,
            total,
            projects,
            daily,
            source_status,
        };

        progress(Progress::phase(ProgressPhase::Saving));
        sidecar.publish_snapshot(&snapshot)?;
        Ok(snapshot)
    }
}

#[derive(Debug, Clone)]
struct ParsedFile {
    session_id: String,
    cwd: Option<String>,
    title: Option<String>,
    project_id: String,
    project_display_name: String,
    project_path: Option<String>,
    totals: UsageTotals,
    cost: CostEstimate,
    model_tokens: HashMap<String, u64>,
    day_models: HashMap<String, HashMap<String, (u64, u64, u64)>>,
    started_at: Option<DateTime<Utc>>,
    latest_activity: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
struct SessionBucket {
    id: String,
    project_id: String,
    project_display_name: String,
    project_path: Option<String>,
    cwd: Option<String>,
    title: Option<String>,
    totals: UsageTotals,
    cost: CostEstimate,
    model_tokens: HashMap<String, u64>,
    started_at: Option<DateTime<Utc>>,
    latest_activity: Option<DateTime<Utc>>,
}

impl SessionBucket {
    fn new(parsed: &ParsedFile) -> Self {
        Self {
            id: parsed.session_id.clone(),
            project_id: parsed.project_id.clone(),
            project_display_name: parsed.project_display_name.clone(),
            project_path: parsed.project_path.clone(),
            cwd: parsed.cwd.clone(),
            title: parsed.title.clone(),
            totals: UsageTotals::default(),
            cost: CostEstimate::default(),
            model_tokens: HashMap::new(),
            started_at: None,
            latest_activity: None,
        }
    }

    fn merge(&mut self, parsed: ParsedFile) {
        self.totals.add_assign(&parsed.totals);
        self.cost.add_assign(&parsed.cost);
        if self.cwd.is_none() {
            self.cwd = parsed.cwd;
        }
        if self.title.is_none() {
            self.title = parsed.title;
        }
        if self.project_path.is_none() {
            self.project_path = parsed.project_path;
            self.project_display_name = parsed.project_display_name;
            self.project_id = parsed.project_id;
        }
        self.started_at = min_time(self.started_at, parsed.started_at);
        self.latest_activity = max_time(self.latest_activity, parsed.latest_activity);
        for (model, tokens) in parsed.model_tokens {
            *self.model_tokens.entry(model).or_default() = self
                .model_tokens
                .get(&model)
                .copied()
                .unwrap_or(0)
                .saturating_add(tokens);
        }
    }

    fn top_model(&self) -> Option<String> {
        self.model_tokens
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then_with(|| a.0.cmp(b.0)))
            .map(|(m, _)| m.clone())
    }

    fn to_session_usage(&self) -> SessionUsage {
        SessionUsage {
            id: self.id.clone(),
            project_id: self.project_id.clone(),
            display_title: self
                .title
                .clone()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| "Local Codex chat".to_string()),
            cwd: self.cwd.clone(),
            started_at: self.started_at,
            latest_activity: self.latest_activity,
            totals: self.totals,
            cost_estimate: self.cost,
            top_model: self.top_model(),
        }
    }
}

#[derive(Default)]
struct DailyAcc {
    total_tokens: u64,
    cached_input_tokens: u64,
    known_usd: f64,
    unknown_tokens: u64,
}

fn build_projects(sessions: &HashMap<String, SessionBucket>) -> Vec<ProjectUsage> {
    let mut by_project: HashMap<String, Vec<&SessionBucket>> = HashMap::new();
    for session in sessions.values() {
        by_project
            .entry(session.project_id.clone())
            .or_default()
            .push(session);
    }

    by_project
        .into_values()
        .map(|mut buckets| {
            buckets.sort_by(|a, b| {
                b.latest_activity
                    .cmp(&a.latest_activity)
                    .then_with(|| a.id.cmp(&b.id))
            });
            let first = buckets[0];
            let mut totals = UsageTotals::default();
            let mut cost = CostEstimate::default();
            let mut model_tokens: HashMap<String, u64> = HashMap::new();
            let mut latest = None;
            for b in &buckets {
                totals.add_assign(&b.totals);
                cost.add_assign(&b.cost);
                latest = max_time(latest, b.latest_activity);
                for (model, tokens) in &b.model_tokens {
                    *model_tokens.entry(model.clone()).or_default() += *tokens;
                }
            }
            let top_model = model_tokens
                .into_iter()
                .max_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
                .map(|(m, _)| m);
            let top_sessions = buckets
                .iter()
                .take(5)
                .map(|b| b.to_session_usage())
                .collect();
            ProjectUsage {
                id: first.project_id.clone(),
                display_name: first.project_display_name.clone(),
                path: first.project_path.clone(),
                totals,
                cost_estimate: cost,
                session_count: buckets.len() as u32,
                latest_activity: latest,
                top_model,
                top_sessions,
            }
        })
        .collect()
}

fn index_one_file(path: &Path, range: &CostUsageDayRange) -> Option<ParsedFile> {
    let first_line = CodexRolloutFirstLineParser::read_first_line(path)?;
    let meta = CodexRolloutFirstLineParser::parse(&first_line);
    let (session_id, cwd) = if let Some(meta) = meta {
        (meta.session_id, meta.cwd)
    } else {
        let stem = path.file_stem()?.to_string_lossy().into_owned();
        let id = stem
            .rsplit('-')
            .next()
            .filter(|s| s.len() >= 8)
            .map(|s| s.to_string())
            .unwrap_or(stem);
        (id, None)
    };

    let parsed = JsonlScanner::parse_codex_file(path, range, 0, None, None).ok()?;
    if parsed.records.is_empty() && parsed.last_model.is_none() {
        // Still index empty sessions with metadata when they fall in range by mtime/name.
        // Skip only when there is truly nothing usable.
        if cwd.is_none() && meta_day_out_of_range(path, range) {
            return None;
        }
    }

    let identity = project_identity(cwd.as_deref());
    let mut totals = UsageTotals::default();
    let mut cost = CostEstimate::default();
    let mut model_tokens: HashMap<String, u64> = HashMap::new();
    let mut day_models: HashMap<String, HashMap<String, (u64, u64, u64)>> = HashMap::new();

    for record in &parsed.records {
        let input = record.input.max(0) as u64;
        let cached = (record.cached.max(0) as u64).min(input);
        let output = record.output.max(0) as u64;
        let part = UsageTotals::from_parts(input, cached, output);
        totals.add_assign(&part);

        let model = CostUsagePricing::normalize_codex_model(&record.model);
        let tokens = input.saturating_add(output);
        *model_tokens.entry(model.clone()).or_default() += tokens;
        let day_entry = day_models
            .entry(record.day_key.clone())
            .or_default()
            .entry(model.clone())
            .or_default();
        day_entry.0 = day_entry.0.saturating_add(input);
        day_entry.1 = day_entry.1.saturating_add(cached);
        day_entry.2 = day_entry.2.saturating_add(output);

        match CostUsagePricing::codex_cost_usd(&model, input, cached, output) {
            Some(usd) => cost.known_usd += usd,
            None => cost.unknown_tokens = cost.unknown_tokens.saturating_add(tokens),
        }
    }

    if model_tokens.is_empty()
        && let Some(model) = parsed
            .last_model
            .as_deref()
            .map(CostUsagePricing::normalize_codex_model)
    {
        model_tokens.insert(model, 0);
    }

    let (started_at, latest_activity) = file_activity_bounds(path, &day_models);

    Some(ParsedFile {
        session_id,
        cwd,
        title: None,
        project_id: identity.0,
        project_display_name: identity.1,
        project_path: identity.2,
        totals,
        cost,
        model_tokens,
        day_models,
        started_at,
        latest_activity,
    })
}

fn merge_daily(
    daily: &mut HashMap<String, DailyAcc>,
    day_models: &HashMap<String, HashMap<String, (u64, u64, u64)>>,
) {
    for (day, models) in day_models {
        let acc = daily.entry(day.clone()).or_default();
        for (model, (input, cached, output)) in models {
            let tokens = input.saturating_add(*output);
            acc.total_tokens = acc.total_tokens.saturating_add(tokens);
            acc.cached_input_tokens = acc.cached_input_tokens.saturating_add(*cached);
            match CostUsagePricing::codex_cost_usd(model, *input, *cached, *output) {
                Some(usd) => acc.known_usd += usd,
                None => acc.unknown_tokens = acc.unknown_tokens.saturating_add(tokens),
            }
        }
    }
}

fn list_session_files(root: &Path, range: &CostUsageDayRange) -> Vec<PathBuf> {
    if !root.exists() {
        return Vec::new();
    }
    JsonlScanner::list_codex_session_files(root, &range.scan_since_key, &range.scan_until_key)
}

fn read_catalog_status(db_path: &Path) -> SourceStatus {
    if !db_path.exists() {
        return SourceStatus::CatalogMissing;
    }
    let open = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    );
    let conn = match open {
        Ok(c) => c,
        Err(e) => {
            let msg = e.to_string().to_ascii_lowercase();
            return if msg.contains("locked") || msg.contains("busy") {
                SourceStatus::CatalogLocked
            } else if msg.contains("not a database") || msg.contains("corrupt") {
                SourceStatus::CatalogCorrupt
            } else {
                SourceStatus::CatalogIncompatible
            };
        }
    };
    let _ = conn.busy_timeout(std::time::Duration::from_millis(250));
    let _ = conn.execute_batch("PRAGMA query_only = ON;");

    let has_threads: Result<bool, _> = conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'threads' LIMIT 1",
        [],
        |_| Ok(true),
    );
    match has_threads {
        Ok(true) => {}
        Ok(false) => return SourceStatus::CatalogIncompatible,
        Err(e) => {
            let msg = e.to_string().to_ascii_lowercase();
            return if msg.contains("locked") || msg.contains("busy") {
                SourceStatus::CatalogLocked
            } else if msg.contains("corrupt") || msg.contains("not a database") {
                SourceStatus::CatalogCorrupt
            } else {
                SourceStatus::CatalogIncompatible
            };
        }
    }

    // Probe a cheap read so locked/corrupt states surface.
    match conn.query_row("SELECT COUNT(*) FROM threads", [], |r| r.get::<_, i64>(0)) {
        Ok(_) => SourceStatus::Complete,
        Err(e) => {
            let msg = e.to_string().to_ascii_lowercase();
            if msg.contains("locked") || msg.contains("busy") {
                SourceStatus::CatalogLocked
            } else if msg.contains("corrupt") {
                SourceStatus::CatalogCorrupt
            } else {
                SourceStatus::CatalogIncompatible
            }
        }
    }
}

/// Project identity from cwd: `(id, display_name, path)`.
pub fn project_identity(cwd: Option<&str>) -> (String, String, Option<String>) {
    let Some(cwd) = cwd.map(str::trim).filter(|s| !s.is_empty()) else {
        return (
            CHATS_PROJECT_ID.to_string(),
            CHATS_DISPLAY_NAME.to_string(),
            None,
        );
    };
    let root = resolve_project_root(Path::new(cwd));
    let path = normalize_path(&root);
    let path_str = path.to_string_lossy().replace('\\', "/");
    let id = format!("project-{}", &sha256_hex(path_str.as_bytes())[..16]);
    let display = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| path_str.clone());
    (id, display, Some(path_str))
}

fn resolve_project_root(cwd: &Path) -> PathBuf {
    if !cwd.exists() {
        return cwd.to_path_buf();
    }
    let mut current = if cwd.is_file() {
        cwd.parent().unwrap_or(cwd).to_path_buf()
    } else {
        cwd.to_path_buf()
    };
    loop {
        if current.join(".git").exists() {
            return current;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => return cwd.to_path_buf(),
        }
    }
}

fn scope_fingerprint(value: &str) -> String {
    // FNV-1a 64-bit, matching upstream CodexLocalDataScope.
    let mut hash: u64 = 14_695_981_039_346_656_037;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    format!("{hash:x}")
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(|v| {
        let t = v.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    })
}

fn normalize_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn min_time(a: Option<DateTime<Utc>>, b: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

fn max_time(a: Option<DateTime<Utc>>, b: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

fn file_activity_bounds(
    path: &Path,
    day_models: &HashMap<String, HashMap<String, (u64, u64, u64)>>,
) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    let mut earliest: Option<NaiveDate> = None;
    let mut latest: Option<NaiveDate> = None;
    for day in day_models.keys() {
        if let Ok(d) = NaiveDate::parse_from_str(day, "%Y-%m-%d") {
            earliest = Some(earliest.map_or(d, |e| e.min(d)));
            latest = Some(latest.map_or(d, |e| e.max(d)));
        }
    }
    let started = earliest
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|n| Utc.from_utc_datetime(&n));
    let mut end = latest
        .and_then(|d| d.and_hms_opt(23, 59, 59))
        .map(|n| Utc.from_utc_datetime(&n));
    if end.is_none()
        && let Ok(meta) = fs::metadata(path)
        && let Ok(modified) = meta.modified()
    {
        end = Some(DateTime::<Utc>::from(modified));
    }
    (started.or(end), end)
}

fn meta_day_out_of_range(path: &Path, range: &CostUsageDayRange) -> bool {
    // Best-effort: parent folder name YYYY/MM/DD or mtime.
    if let Some(day) = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|d| d.to_str())
        .filter(|d| d.len() == 2)
    {
        // day folder — reconstruct from parents
        let parts: Vec<_> = path
            .components()
            .filter_map(|c| c.as_os_str().to_str())
            .collect();
        if parts.len() >= 3 {
            let y = parts[parts.len() - 3];
            let m = parts[parts.len() - 2];
            let d = day;
            if let Ok(date) = NaiveDate::parse_from_str(&format!("{y}-{m}-{d}"), "%Y-%m-%d") {
                let key = CostUsageDayRange::day_key(date);
                return !CostUsageDayRange::is_in_range(&key, &range.since_key, &range.until_key);
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_workspaces::types::SourceStatus;
    use rusqlite::Connection;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_session(
        dir: &Path,
        day: &str,
        name: &str,
        cwd: &str,
        model: &str,
        input: i32,
        output: i32,
    ) {
        // day = YYYY-MM-DD
        let parts: Vec<_> = day.split('-').collect();
        let folder = dir.join(parts[0]).join(parts[1]).join(parts[2]);
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join(name);
        let mut f = File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"timestamp":"{day}T10:00:00.000Z","type":"session_meta","payload":{{"session_id":"{id}","cwd":"{cwd}","originator":"codex_exec","source":"cli"}}}}"#,
            id = name.trim_end_matches(".jsonl"),
            cwd = cwd.replace('\\', "\\\\"),
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"timestamp":"{day}T10:00:01.000Z","type":"turn_context","payload":{{"model":"{model}"}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"timestamp":"{day}T10:00:02.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":{input},"cached_input_tokens":0,"output_tokens":{output}}}}}}}}}"#
        )
        .unwrap();
    }

    #[test]
    fn indexes_two_sessions_into_projects_and_daily() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("codex");
        let sessions = home.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let day = Local::now().date_naive().format("%Y-%m-%d").to_string();

        write_session(
            &sessions,
            &day,
            "sess-aaa.jsonl",
            &tmp.path().join("proj-a").to_string_lossy(),
            "gpt-5",
            1000,
            500,
        );
        write_session(
            &sessions,
            &day,
            "sess-bbb.jsonl",
            &tmp.path().join("proj-b").to_string_lossy(),
            "gpt-5",
            2000,
            100,
        );

        // Ensure project roots exist so resolve doesn't collapse oddly.
        fs::create_dir_all(tmp.path().join("proj-a")).unwrap();
        fs::create_dir_all(tmp.path().join("proj-b")).unwrap();

        let sidecar = tmp.path().join("sidecar.sqlite");
        let index = CodexWorkspacesIndex::new(30)
            .with_codex_home(&home)
            .with_sidecar_path(&sidecar);

        let snap = index.load_snapshot(true, |_| {}).expect("snapshot");
        assert_eq!(snap.indexed_file_count, 2);
        assert_eq!(snap.projects.len(), 2);
        assert!(!snap.daily.is_empty());
        assert_eq!(snap.source_status, SourceStatus::CatalogMissing);
        let total_sessions: u32 = snap.projects.iter().map(|p| p.session_count).sum();
        assert_eq!(total_sessions, 2);
        assert!(snap.total.input_tokens >= 3000);
        assert!(snap.total.output_tokens >= 600);

        // Cached load works.
        let cached = index.load_cached_snapshot().unwrap().expect("cached");
        assert_eq!(cached.indexed_file_count, 2);
    }

    #[test]
    fn foreign_user_version_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("bad.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "user_version", 99).unwrap();
        }
        let home = tmp.path().join("codex");
        fs::create_dir_all(home.join("sessions")).unwrap();
        let index = CodexWorkspacesIndex::new(7)
            .with_codex_home(home)
            .with_sidecar_path(path);
        let err = index.load_snapshot(true, |_| {}).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("incompatible") || msg.contains("user_version"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn missing_catalog_reports_catalog_missing() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("codex");
        fs::create_dir_all(home.join("sessions")).unwrap();
        let index = CodexWorkspacesIndex::new(7)
            .with_codex_home(&home)
            .with_sidecar_path(tmp.path().join("side.sqlite"));
        let snap = index.load_snapshot(true, |_| {}).unwrap();
        assert_eq!(snap.source_status, SourceStatus::CatalogMissing);
    }

    #[test]
    fn privacy_redaction_strips_paths_and_titles() {
        let mut snap = CodexLocalProjectUsageSnapshot {
            updated_at: Utc::now(),
            history_days: 30,
            scope_signature: "x".into(),
            indexed_file_count: 1,
            skipped_file_count: 0,
            total: UsageTotals::from_parts(10, 0, 5),
            projects: vec![ProjectUsage {
                id: "project-abc".into(),
                display_name: "secret-repo".into(),
                path: Some("/Users/me/secret-repo".into()),
                totals: UsageTotals::from_parts(10, 0, 5),
                cost_estimate: CostEstimate::default(),
                session_count: 1,
                latest_activity: None,
                top_model: Some("gpt-5".into()),
                top_sessions: vec![SessionUsage {
                    id: "s1".into(),
                    project_id: "project-abc".into(),
                    display_title: "do not leak".into(),
                    cwd: Some("/Users/me/secret-repo".into()),
                    started_at: None,
                    latest_activity: None,
                    totals: UsageTotals::from_parts(10, 0, 5),
                    cost_estimate: CostEstimate::default(),
                    top_model: Some("gpt-5".into()),
                }],
            }],
            daily: vec![],
            source_status: SourceStatus::Complete,
        };
        snap.redact_for_privacy();
        assert_eq!(snap.projects[0].display_name, "Workspace");
        assert!(snap.projects[0].path.is_none());
        assert_eq!(
            snap.projects[0].top_sessions[0].display_title,
            "Local Codex chat"
        );
        assert!(snap.projects[0].top_sessions[0].cwd.is_none());
    }
}
