//! Pi-family session root resolution: budgets, canonical paths, OMP
//! profiles/config dirs, plain-pi settings, and per-process root selection
//! (upstream `OMPSessionRootResolver` + `sessionRoots(for:)` + `records(in:)`).

use super::AgentProcessRecord;
use super::command_line_value;
use super::parser::{PiFamilySessionRecord, parse_session_file};
use super::{MAX_PROFILE_ROOTS, MAX_SETTINGS_BYTES, PiSessionDialect};

use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

// ---------------------------------------------------------------------------

/// Hard bounds over directory metadata walks (entries, depth, wall clock).
pub struct DirectoryScanBudget {
    max_entry_count: usize,
    max_depth: usize,
    entries_seen: usize,
    deadline: Instant,
}

impl DirectoryScanBudget {
    pub fn new(max_entry_count: usize, max_depth: usize, budget: std::time::Duration) -> Self {
        Self {
            max_entry_count,
            max_depth,
            entries_seen: 0,
            deadline: Instant::now() + budget,
        }
    }

    pub fn has_time_remaining(&self) -> bool {
        Instant::now() < self.deadline
    }

    pub fn visit_entry(&mut self) -> bool {
        if !self.has_time_remaining() {
            return false;
        }
        self.entries_seen += 1;
        self.entries_seen <= self.max_entry_count
    }

    fn allowed_depth(&self, depth: usize) -> bool {
        depth <= self.max_depth
    }
}

// ---------------------------------------------------------------------------
// Canonical path helpers (upstream `canonicalURL` / `isWithin` / pathURL)
// ---------------------------------------------------------------------------

/// Lexically resolve `..`/`.` without requiring the target to exist
/// (upstream `standardizedFileURL` on top of canonicalize-when-possible).
pub fn canonicalize_for_scan(path: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| lexically_normalize(path));
    strip_windows_verbatim_prefix(canonical)
}

fn lexically_normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    let stripped = raw
        .strip_prefix(r"\\?\")
        .or_else(|| raw.strip_prefix("//?/"))
        .unwrap_or(raw.as_ref());
    PathBuf::from(stripped)
}

/// Case-insensitive containment on Windows (NTFS), case-sensitive elsewhere.
pub fn path_is_within(root: &Path, candidate: &Path) -> bool {
    let root_s = root.to_string_lossy();
    let candidate_s = candidate.to_string_lossy();
    if candidate_s == root_s {
        return true;
    }
    #[cfg(windows)]
    {
        let sep = if root_s.ends_with(['/', '\\']) {
            ""
        } else {
            "\\"
        };
        let prefix = format!("{root_s}{sep}").to_ascii_lowercase();
        candidate_s.to_ascii_lowercase().starts_with(&prefix)
    }
    #[cfg(not(windows))]
    {
        if root_s.as_ref() == "/" {
            return candidate_s.starts_with('/');
        }
        let prefix = format!("{}/", root_s.trim_end_matches('/'));
        candidate_s.starts_with(&prefix)
    }
}

/// Upstream `pathURL`: `~` expansion, absolute vs cwd-relative resolution.
fn resolve_path_url(path: &str, cwd: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return None;
    }
    let expanded: String = if trimmed == "~" {
        home?.to_string_lossy().into_owned()
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        home?.join(rest).to_string_lossy().into_owned()
    } else {
        trimmed.to_string()
    };
    let expanded = PathBuf::from(&expanded);
    let resolved = if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    };
    Some(canonicalize_for_scan(&resolved))
}

// ---------------------------------------------------------------------------
// OMP session root resolution (upstream `OMPSessionRootResolver`)
// ---------------------------------------------------------------------------

/// Validated profile selector: `default` resolves to the default profile;
/// invalid values fail closed (upstream `normalizedProfile`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PiProfile {
    Default,
    Named(String),
    Invalid,
}

/// Upstream profile-name policy: ≤64 scalars, leading ASCII digit/lowercase
/// letter, tail of ASCII alnum/.-_ , not `.`/`..`, no trailing dot, not a
/// Windows reserved device name — re-evaluated even on Windows (OMP profile
/// folders are shared across platforms).
pub fn normalize_profile(value: Option<&str>) -> PiProfile {
    let normalized = value.map(str::trim).unwrap_or_default();
    if normalized.is_empty() || normalized == "default" {
        return PiProfile::Default;
    }
    let chars: Vec<char> = normalized.chars().collect();
    let first_ok = chars
        .first()
        .is_some_and(|c| c.is_ascii_digit() || c.is_ascii_lowercase());
    let tail_ok = chars
        .iter()
        .skip(1)
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if chars.len() > 64
        || !first_ok
        || !tail_ok
        || normalized == "."
        || normalized == ".."
        || normalized.ends_with('.')
        || is_windows_reserved_profile_name(normalized)
    {
        return PiProfile::Invalid;
    }
    PiProfile::Named(normalized.to_string())
}

fn is_windows_reserved_profile_name(value: &str) -> bool {
    let upper = value.to_ascii_uppercase();
    let base = upper.split('.').next().unwrap_or("");
    if matches!(base, "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    (base.starts_with("COM") || base.starts_with("LPT"))
        && base.len() == 4
        && base.chars().last().is_some_and(|c| c.is_ascii_digit())
}

pub type EnvMap = HashMap<String, String>;

/// Upstream `OMPSessionRootResolver.sessionRoots` for the DEFAULT profile:
/// `PI_CONFIG_DIR` (relative, must stay under home) → `agent/sessions`,
/// plus the optional `PI_CODING_AGENT_DIR` override + XDG fallback (unix).
pub fn omp_default_profile_root(environment: &EnvMap, cwd: &Path, home: &Path) -> Option<PathBuf> {
    let canonical_home = canonicalize_for_scan(home);
    let config_root = omp_config_root(environment, &canonical_home)?;
    let agent_root = match omp_custom_agent_root(environment, cwd) {
        Some(custom) => custom,
        None => {
            let canonical_agent = canonicalize_for_scan(&config_root.join("agent"));
            if !path_is_within(&canonical_home, &canonical_agent) {
                return None;
            }
            canonical_agent
        }
    };
    let root = session_root_under(&agent_root)?;
    #[cfg(unix)]
    {
        if !environment.contains_key("PI_CODING_AGENT_DIR")
            && let Some(xdg) = env_path(environment.get("XDG_DATA_HOME"), cwd)
        {
            let xdg_sessions = xdg.join("omp").join("sessions");
            if xdg_sessions.is_dir() {
                return session_root_under(&xdg.join("omp"));
            }
        }
    }
    Some(root)
}

/// Upstream `sessionRoots` for a NAMED profile:
/// `<config>/profiles/<name>/agent/sessions` (+ XDG on unix).
pub fn omp_named_profile_root(
    profile: &str,
    environment: &EnvMap,
    cwd: &Path,
    home: &Path,
) -> Option<PathBuf> {
    // `cwd` only feeds the unix XDG probe; on Windows it is intentionally idle.
    #[cfg(windows)]
    let _ = cwd;
    let canonical_home = canonicalize_for_scan(home);
    let config_root = omp_config_root(environment, &canonical_home)?;
    let profile_root = config_root.join("profiles").join(profile);
    let canonical_agent = canonicalize_for_scan(&profile_root.join("agent"));
    if !path_is_within(&canonical_home, &canonical_agent) {
        return None;
    }
    let root = session_root_under(&canonical_agent)?;
    #[cfg(unix)]
    {
        if let Some(xdg) = env_path(environment.get("XDG_DATA_HOME"), cwd) {
            let xdg_profile = xdg.join("omp").join("profiles").join(profile);
            if xdg_profile.join("sessions").is_dir() {
                return session_root_under(&xdg_profile);
            }
        }
    }
    Some(root)
}

/// Every named profile root (upstream `profileSessionRoots(in:)`): enumerate
/// `<config>/profiles/*` dirs; probe `<profile>/sessions` (XDG layout) else
/// `<profile>/agent/sessions`. Bounded to [`MAX_PROFILE_ROOTS`], sorted.
pub fn omp_profile_parents(environment: &EnvMap, home: &Path) -> Vec<PathBuf> {
    let canonical_home = canonicalize_for_scan(home);
    let mut parents = Vec::new();
    if let Some(config_root) = omp_config_root(environment, &canonical_home) {
        parents.push(config_root.join("profiles"));
    }
    #[cfg(unix)]
    {
        if let Some(xdg) = env_path(environment.get("XDG_DATA_HOME"), &canonical_home) {
            parents.push(xdg.join("omp").join("profiles"));
        } else {
            parents.push(
                canonical_home
                    .join(".local")
                    .join("share")
                    .join("omp")
                    .join("profiles"),
            );
        }
    }
    parents
}

/// Upstream `profileSessionRoots(in:)` for one profiles directory.
fn omp_profile_roots_in(profiles_directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(profiles_directory) else {
        return Vec::new();
    };
    let canonical_parent = canonicalize_for_scan(profiles_directory);
    let mut roots = Vec::new();
    for entry in entries.flatten() {
        if roots.len() >= MAX_PROFILE_ROOTS {
            break;
        }
        let path = canonicalize_for_scan(&entry.path());
        if !path.is_dir() || !path_is_within(&canonical_parent, &path) {
            continue;
        }
        let xdg_layout = path.join("sessions");
        if xdg_layout.is_dir() {
            roots.push(xdg_layout);
        } else {
            roots.push(path.join("agent").join("sessions"));
        }
    }
    roots.sort();
    roots
}

/// All OMP profile roots for a no-profile process (upstream appends these
/// after the default root).
pub fn omp_all_profile_roots(environment: &EnvMap, home: &Path) -> Vec<PathBuf> {
    omp_profile_parents(environment, home)
        .iter()
        .flat_map(|parent| omp_profile_roots_in(parent))
        .collect()
}

fn omp_config_root(environment: &EnvMap, canonical_home: &Path) -> Option<PathBuf> {
    let name = environment
        .get("PI_CONFIG_DIR")
        .map(String::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(".omp");
    // Upstream rejects absolute PI_CONFIG_DIR (must resolve under home) —
    // the root is always home-relative, never cwd-relative.
    if PathBuf::from(name).is_absolute() || name.starts_with('/') || name.starts_with('~') {
        return None;
    }
    let configured = resolve_path_url(name, canonical_home, Some(canonical_home))?;
    if !path_is_within(canonical_home, &configured) {
        return None;
    }
    Some(configured)
}

fn omp_custom_agent_root(environment: &EnvMap, cwd: &Path) -> Option<PathBuf> {
    env_path(environment.get("PI_CODING_AGENT_DIR"), cwd)
}

fn env_path(value: Option<&String>, cwd: &Path) -> Option<PathBuf> {
    let value = value?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = PathBuf::from(trimmed);
    let resolved = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };
    Some(canonicalize_for_scan(&resolved))
}

fn session_root_under(agent_root: &Path) -> Option<PathBuf> {
    let canonical_agent = canonicalize_for_scan(agent_root);
    let candidate = canonicalize_for_scan(&agent_root.join("sessions"));
    path_is_within(&canonical_agent, &candidate).then_some(candidate)
}

// ---------------------------------------------------------------------------
// Settings-driven session dirs (upstream `piSettingsSessionDirectory`)
// ---------------------------------------------------------------------------

/// Session directory configured by plain pi settings: project
/// `<cwd>/.pi/settings.json` first, then `<home>/.pi/agent/settings.json`.
pub fn pi_settings_session_directory(cwd: &Path, home: &Path) -> Option<PathBuf> {
    let global = home.join(".pi").join("agent").join("settings.json");
    let project = cwd.join(".pi").join("settings.json");
    let configured = session_dir_in(&project).or_else(|| session_dir_in(&global))?;
    resolve_path_url(&configured, cwd, Some(home))
}

pub fn session_dir_in(settings_path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(settings_path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_SETTINGS_BYTES {
        return None;
    }
    let data = std::fs::read(settings_path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&data).ok()?;
    let session_dir = value.get("sessionDir")?.as_str()?.trim();
    if session_dir.is_empty() {
        None
    } else {
        Some(session_dir.to_string())
    }
}

// ---------------------------------------------------------------------------
// Session root selection per process (upstream `sessionRoots(for:)`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RootLayout {
    ProjectDirectories,
    Direct,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionRoot {
    pub path: PathBuf,
    pub layout: RootLayout,
}

pub fn session_roots_for_process(
    process: &AgentProcessRecord,
    dialect: PiSessionDialect,
    cwd: &Path,
    environment: &EnvMap,
    home: Option<&Path>,
) -> Vec<SessionRoot> {
    // 1. `--session-dir <path>` from the invocation wins (upstream first).
    if let Some(command) = process.command.as_deref()
        && let Some(explicit) = command_line_value("--session-dir", command)
        && let Some(url) = resolve_path_url(explicit, cwd, home)
    {
        return vec![SessionRoot {
            path: url,
            layout: RootLayout::Direct,
        }];
    }
    // 2. `PI_CODING_AGENT_SESSION_DIR` env.
    if let Some(url) = env_path(environment.get("PI_CODING_AGENT_SESSION_DIR"), cwd) {
        return vec![SessionRoot {
            path: url,
            layout: RootLayout::Direct,
        }];
    }

    match dialect {
        PiSessionDialect::Pi => pi_roots(environment, cwd, home),
        PiSessionDialect::Omp => omp_roots(process, cwd, environment, home),
    }
}

fn pi_roots(environment: &EnvMap, cwd: &Path, home: Option<&Path>) -> Vec<SessionRoot> {
    if let Some(agent_root) = env_path(environment.get("PI_CODING_AGENT_DIR"), cwd) {
        return vec![SessionRoot {
            path: agent_root.join("sessions"),
            layout: RootLayout::ProjectDirectories,
        }];
    }
    let home = match home {
        Some(home) => home,
        None => return Vec::new(),
    };
    if let Some(configured) = pi_settings_session_directory(cwd, home) {
        return vec![SessionRoot {
            path: configured,
            layout: RootLayout::Direct,
        }];
    }
    vec![SessionRoot {
        path: home.join(".pi").join("agent").join("sessions"),
        layout: RootLayout::ProjectDirectories,
    }]
}

/// Upstream `ompSessionRoots`: sanitized environment slice, `--profile` from
/// the command line, plus all profile roots when none is selected.
fn omp_roots(
    process: &AgentProcessRecord,
    cwd: &Path,
    environment: &EnvMap,
    home: Option<&Path>,
) -> Vec<SessionRoot> {
    let home = match home {
        Some(home) => home,
        None => return Vec::new(),
    };
    let mut safe_env: EnvMap = EnvMap::new();
    safe_env.insert("HOME".to_string(), home.to_string_lossy().into_owned());
    for key in [
        "PI_CONFIG_DIR",
        "PI_CODING_AGENT_DIR",
        "XDG_DATA_HOME",
        "OMP_PROFILE",
        "PI_PROFILE",
    ] {
        if let Some(value) = environment.get(key) {
            safe_env.insert(key.to_string(), value.clone());
        }
    }
    if let Some(command) = process.command.as_deref()
        && let Some(profile) = command_line_value("--profile", command)
    {
        safe_env.insert("OMP_PROFILE".to_string(), profile.to_string());
    }

    let profile_value = omp_profile_selector(&safe_env);
    // Upstream: profile parents are appended only when no explicit profile
    // was selected (default profile case); invalid selectors fail closed.
    let named_selected = matches!(profile_value, PiProfile::Named(_));
    let is_invalid = profile_value == PiProfile::Invalid;
    let mut urls: Vec<PathBuf> = match &profile_value {
        PiProfile::Invalid => Vec::new(),
        PiProfile::Named(profile) => omp_named_profile_root(profile, &safe_env, cwd, home)
            .into_iter()
            .collect(),
        PiProfile::Default => omp_default_profile_root(&safe_env, cwd, home)
            .into_iter()
            .collect(),
    };

    if !is_invalid && !named_selected {
        urls.extend(omp_all_profile_roots(&safe_env, home));
    }

    let mut seen = HashSet::new();
    urls.into_iter()
        .filter_map(|url| {
            let canonical = canonicalize_for_scan(&url);
            seen.insert(canonical.clone()).then_some(SessionRoot {
                path: canonical,
                layout: RootLayout::ProjectDirectories,
            })
        })
        .collect()
}

/// Unified profile selector (`OMP_PROFILE` wins over `PI_PROFILE`, upstream).
pub fn omp_profile_selector(environment: &EnvMap) -> PiProfile {
    let value = environment
        .get("OMP_PROFILE")
        .or_else(|| environment.get("PI_PROFILE"));
    normalize_profile(value.map(String::as_str))
}

// ---------------------------------------------------------------------------
// Directory content collection (upstream `records(in:…)`)
// ---------------------------------------------------------------------------

pub fn records_in_root(
    root: &Path,
    now: DateTime<Utc>,
    dialect: PiSessionDialect,
    layout: RootLayout,
    budget: &mut DirectoryScanBudget,
) -> Vec<PiFamilySessionRecord> {
    if !budget.has_time_remaining() {
        return Vec::new();
    }
    let canonical_root = canonicalize_for_scan(root);
    let project_directories: Vec<PathBuf> = match layout {
        RootLayout::Direct => vec![canonical_root.clone()],
        RootLayout::ProjectDirectories => {
            let Ok(entries) = std::fs::read_dir(&canonical_root) else {
                return Vec::new();
            };
            let mut dirs: Vec<PathBuf> = entries
                .flatten()
                .filter(|_entry| budget.visit_entry())
                .map(|entry| canonicalize_for_scan(&entry.path()))
                .filter(|path| path.is_dir() && path_is_within(&canonical_root, path))
                .collect();
            dirs.sort();
            dirs
        }
    };

    if !budget.allowed_depth(1) {
        return Vec::new();
    }

    let mut records = Vec::new();
    let mut visible = HashSet::new();
    for project_dir in project_directories {
        if !budget.has_time_remaining() {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&project_dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .filter(|_entry| budget.visit_entry())
            .map(|entry| canonicalize_for_scan(&entry.path()))
            .filter(|path| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
                    && path_is_within(&canonical_root, path)
                    && path.parent() == Some(project_dir.as_path())
            })
            .collect();
        files.sort();
        for path in files {
            if !budget.has_time_remaining() {
                break;
            }
            let Ok(metadata) = path.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let Some(modified_at) = metadata.modified().ok().map(DateTime::<Utc>::from) else {
                continue;
            };
            if let Some(record) = parse_session_file(&path, dialect, modified_at, now)
                && visible.insert(record.id.clone())
            {
                records.push(record);
            }
        }
    }

    records.sort_by(|lhs, rhs| {
        rhs.modified_at
            .cmp(&lhs.modified_at)
            .then(lhs.id.cmp(&rhs.id))
            .then(lhs.path.cmp(&rhs.path))
    });
    let mut seen_paths = HashSet::new();
    records.retain(|record| seen_paths.insert(canonicalize_for_scan(&record.path)));
    records
}

// ---------------------------------------------------------------------------
// Scanner (upstream `PiFamilySessionScanner.scan`)
