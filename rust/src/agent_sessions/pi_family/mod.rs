//! Pi-family agent session scanner (upstream 0.48.0 #2626
//! `PiFamilySessionScanner` + `OMPSessionRootResolver`).
//!
//! One scanner discovers live **pi** and **OMP** sessions and correlates each
//! process with its JSONL session file, in a dialect-aware way:
//!
//! - **pi**: session headers require `version: 3`; the current title is the
//!   latest `session_info` record (prefix scan, then a bounded tail scan for
//!   long transcripts).
//! - **omp**: headers tolerate no `version` (legacy) and the title comes from
//!   the leading `{"type":"title"}` slot or the header `title` field.
//!
//! Pi-family sessions are process-backed (upstream semantics): a session never
//! materializes from files alone. When a process cannot be correlated (no
//! readable cwd, no matching transcript, or a fresh startup), the scanner
//! emits a **PID-only row** (`pid:<pid>`) instead of guessing.
//!
//! Windows mapping notes (documented divergences):
//! - There is no cheap per-process cwd API like upstream's `lsof`; the
//!   Windows host passes an empty cwd map, so correlate-on-cwd records yield
//!   PID-only rows until cwd capture exists. The pure scanner path used by
//!   tests accepts an injected cwd map and implements full upstream semantics.
//! - XDG fallback roots are Linux/macOS-only upstream (`#if os(macOS) ||
//!   os(Linux)`); they are compiled out on Windows here exactly like upstream.
//! - Path canonicalization/`\\?\` prefix handling is Windows-flavored;
//!   relative `PI_CONFIG_DIR` roots still must resolve within home.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::{
    AgentProcessRecord, AgentSession, AgentSessionActivity, AgentSessionFocusTarget,
    AgentSessionProvider, AgentSessionSource, AgentSessionState, AgentSessionWorkspace,
    PiSessionDialect, SessionScanConfig,
};

/// Bounded transcript prefix used for header/first-title parsing (upstream
/// `maximumReadSize`).
const MAX_PREFIX_READ: usize = 16 * 1024;
/// Bounded tail window scanned for late `session_info` names on long files.
const TAIL_READ: usize = 64 * 1024;
/// Upstream title bound: 64 unicode scalars, control/newline stripped.
const MAX_TITLE_SCALARS: usize = 64;
/// `settings.json` reads are size-bounded like upstream (1 MiB).
const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;
/// Cap on profile directory enumeration (upstream `roots.count < 64`).
const MAX_PROFILE_ROOTS: usize = 64;

// ---------------------------------------------------------------------------
// Process-level dialect detection (upstream `AgentPSOutputParser.piDialect`)
// ---------------------------------------------------------------------------

/// Windows install shims (`.cmd`, `.bat`, `.ps1`, `.exe`) stripped for
/// basename comparison.
fn basename_without_windows_ext(token: &str) -> String {
    let name = Path::new(token)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    for ext in [".exe", ".cmd", ".bat", ".ps1"] {
        if let Some(base) = name.strip_suffix(ext) {
            return base.to_string();
        }
    }
    name
}

/// Upstream `piDialect(for:)`: detects `pi` / `omp` from the invocation:
/// first token `pi` → pi, `omp` → omp, `bun <…>/omp` → omp.
pub fn pi_family_dialect(command: &str) -> Option<PiSessionDialect> {
    let mut tokens = command.split_whitespace();
    let first = basename_without_windows_ext(tokens.next()?);
    match first.as_str() {
        "pi" => Some(PiSessionDialect::Pi),
        "omp" => Some(PiSessionDialect::Omp),
        "bun" => tokens
            .any(|token| basename_without_windows_ext(token) == "omp")
            .then_some(PiSessionDialect::Omp),
        _ => None,
    }
}

/// Stable executable label for a Pi-family process (first-token basename).
pub fn pi_family_executable(command: &str) -> Option<String> {
    command
        .split_whitespace()
        .next()
        .map(basename_without_windows_ext)
        .filter(|name| !name.is_empty())
}

/// Upstream `isObviousPiFamilyHelper`.
pub fn is_pi_family_helper(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    lower.contains("--help")
        || lower.contains("--version")
        || lower.contains("--smoke-test")
        || lower.contains("__omp_worker_")
}

/// Upstream `commandLineValue`: `--flag value` or `--flag=value` scanning.
fn command_line_value<'a>(flag: &str, command: &'a str) -> Option<&'a str> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    for (index, token) in tokens.iter().enumerate() {
        if *token == flag {
            let value = tokens.get(index + 1)?;
            return (!value.starts_with('-')).then_some(*value);
        }
        if let Some(value) = token.strip_prefix(&format!("{flag}=")) {
            return (!value.is_empty()).then_some(value);
        }
    }
    None
}

pub mod parser;
pub mod roots;

pub use parser::*;
pub use roots::*;

type CwdByPid = HashMap<u32, String>;

/// Inputs for one host-local scan.
pub struct PiFamilyScanInput<'a> {
    /// Candidate process list (pre-classification records).
    pub processes: &'a [AgentProcessRecord],
    /// Per-PID current working directories (empty on Windows today — PID-only
    /// rows result until cwd capture lands).
    pub cwd_by_pid: CwdByPid,
    /// Environment slice (HOME/USERPROFILE/PI_*/OMP_*).
    pub environment: EnvMap,
    pub now: DateTime<Utc>,
    pub host: String,
    pub config: SessionScanConfig,
}

/// One bounded, dialect-aware correlation pass over live Pi-family processes.
pub struct PiFamilySessionScanner;

impl PiFamilySessionScanner {
    /// Environment for one scan: HOME/USERPROFILE + pi/OMP selector env keys.
    pub fn scan_environment() -> EnvMap {
        let mut map: EnvMap = EnvMap::new();
        if let Some(home) = dirs::home_dir() {
            map.insert("HOME".to_string(), home.to_string_lossy().into_owned());
        }
        for key in [
            "PI_CONFIG_DIR",
            "PI_CODING_AGENT_DIR",
            "PI_CODING_AGENT_SESSION_DIR",
            "OMP_PROFILE",
            "PI_PROFILE",
            "XDG_DATA_HOME",
        ] {
            if let Ok(value) = std::env::var(key) {
                map.insert(key.to_string(), value);
            }
        }
        map
    }

    /// Home from the scan environment (HOME, then USERPROFILE).
    pub fn home_from(environment: &EnvMap) -> Option<PathBuf> {
        environment
            .get("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                environment
                    .get("USERPROFILE")
                    .filter(|home| !home.is_empty())
                    .map(PathBuf::from)
            })
    }

    /// Upstream `scan(input:directoryBudget:)`; returns session rows for all
    /// live Pi-family agent processes (never file-only).
    pub fn scan(input: &PiFamilyScanInput, budget: &mut DirectoryScanBudget) -> Vec<AgentSession> {
        let now = input.now;
        let home = Self::home_from(&input.environment);

        let mut live: Vec<&AgentProcessRecord> = input
            .processes
            .iter()
            .filter(|process| process.provider == Some(AgentSessionProvider::Pi))
            .filter(|process| {
                process
                    .command
                    .as_deref()
                    .map(|command| !is_pi_family_helper(command))
                    .unwrap_or(true)
            })
            .collect();
        live.sort_by_key(|process| std::cmp::Reverse(process.started_at));
        live.truncate(input.config.max_process_count);

        // Upstream: Pi-family sessions are process-backed. Never turn an old
        // session file into a file-only AgentSession.
        if live.is_empty() {
            return Vec::new();
        }

        let mut records_by_root: HashMap<String, Vec<PiFamilySessionRecord>> = HashMap::new();
        let mut used_record_paths: HashSet<PathBuf> = HashSet::new();
        let mut sessions = Vec::new();

        for process in live {
            let dialect = match process
                .command
                .as_deref()
                .and_then(pi_family_dialect)
                .or_else(|| pi_family_dialect_from_executable(&process.executable))
            {
                Some(dialect) => dialect,
                None => continue,
            };
            let process_cwd = input.cwd_by_pid.get(&process.pid);
            let standardized_cwd = process_cwd
                .filter(|cwd| !cwd.is_empty())
                .map(|cwd| standardized_cwd_string(cwd));

            let mut record: Option<PiFamilySessionRecord> = None;
            if let (Some(started_at), Some(standardized), Some(cwd)) =
                (process.started_at, standardized_cwd.as_ref(), process_cwd)
            {
                let cwd_value = cwd.clone();
                let cwd_path = PathBuf::from(&cwd_value);
                for root in session_roots_for_process(
                    process,
                    dialect,
                    &cwd_path,
                    &input.environment,
                    home.as_deref(),
                ) {
                    if !budget.has_time_remaining() {
                        break;
                    }
                    let canonical_root = canonicalize_for_scan(&root.path);
                    let root_key = format!(
                        "{:?}:{:?}:{}",
                        dialect,
                        root.layout,
                        canonical_root.display()
                    );
                    let root_records = records_by_root.entry(root_key).or_insert_with(|| {
                        records_in_root(&canonical_root, now, dialect, root.layout, budget)
                    });
                    if let Some(candidate) = root_records.iter().find(|candidate| {
                        candidate.modified_at >= started_at
                            && candidate
                                .cwd
                                .as_deref()
                                .filter(|cwd| !cwd.is_empty())
                                .is_some_and(|record_cwd| {
                                    standardized_cwd_string(record_cwd) == *standardized
                                })
                            && !used_record_paths.contains(&canonicalize_for_scan(&candidate.path))
                    }) {
                        used_record_paths.insert(canonicalize_for_scan(&candidate.path));
                        record = Some(candidate.clone());
                        break;
                    }
                }
            }

            let cwd = process_cwd
                .cloned()
                .or_else(|| record.as_ref().and_then(|r| r.cwd.clone()));
            let id = record
                .as_ref()
                .map(|record| record.id.clone())
                .unwrap_or_else(|| format!("pid:{}", process.pid));
            let session = AgentSession {
                id,
                provider: AgentSessionProvider::Pi,
                dialect: Some(dialect),
                session_name: record
                    .as_ref()
                    .and_then(|record| record.session_name.clone()),
                source: AgentSessionSource::Cli,
                state: input.config.state(
                    record.as_ref().map(|record| record.modified_at),
                    now,
                    true,
                ),
                pid: Some(process.pid),
                transcript_path: record
                    .as_ref()
                    .map(|record| record.path.to_string_lossy().into_owned()),
                host: input.host.clone(),
                workspace: AgentSessionWorkspace {
                    project_name: cwd.as_deref().and_then(super::project_name_from_cwd),
                    cwd,
                },
                activity: AgentSessionActivity {
                    started_at: record
                        .as_ref()
                        .and_then(|record| record.started_at)
                        .or(process.started_at),
                    last_activity_at: record.as_ref().map(|record| record.modified_at),
                },
                focus_target: AgentSessionFocusTarget::Process { pid: process.pid },
            };
            sessions.push(session);
        }

        // Upstream ordering: active first, then most-recent activity, pid desc.
        sessions.sort_by(|lhs, rhs| {
            (rhs.state == AgentSessionState::Active)
                .cmp(&(lhs.state == AgentSessionState::Active))
                .then_with(|| {
                    rhs.activity
                        .last_activity_at
                        .or(rhs.activity.started_at)
                        .cmp(&lhs.activity.last_activity_at.or(lhs.activity.started_at))
                })
                .then_with(|| rhs.pid.unwrap_or(0).cmp(&lhs.pid.unwrap_or(0)))
        });
        let mut seen = HashSet::new();
        sessions.retain(|session| seen.insert(format!("{}:{}", session.host, session.id)));
        sessions
    }
}

/// Dialect from a bare executable name when no command line was captured.
fn pi_family_dialect_from_executable(executable: &str) -> Option<PiSessionDialect> {
    match basename_without_windows_ext(executable).as_str() {
        "pi" => Some(PiSessionDialect::Pi),
        "omp" => Some(PiSessionDialect::Omp),
        _ => None,
    }
}

/// Upstream `standardizedPath` comparison shape: canonicalized, separators
/// normalized; case-insensitive on NTFS.
pub fn standardized_cwd_string(cwd: &str) -> String {
    let canonical = canonicalize_for_scan(Path::new(cwd));
    let value = canonical.to_string_lossy().replace('/', "\\");
    let trimmed = value.trim_end_matches('\\').to_string();
    #[cfg(windows)]
    {
        trimmed.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        trimmed
    }
}

/// Build a bounded budget from the scan config (upstream defaults 512 entries,
/// depth 1, ≤0.25 s wall-clock).
pub fn budget_for(config: &SessionScanConfig) -> DirectoryScanBudget {
    DirectoryScanBudget::new(
        config.max_directory_entry_count,
        config.max_directory_depth,
        config.directory_scan_budget,
    )
}

include!("../pi_family_tests.rs");
