use crate::host::{CommandError, CommandOptions, CommandRunner};
use chrono::{DateTime, Duration as ChronoDuration, Local, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::future::Future;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentSessionProvider {
    Codex,
    Claude,
    Pi,
}

/// Pi-family CLI dialect (upstream `AgentSession.Dialect`).
///
/// `pi` is the standalone coding agent; `omp` (Oh My Pi) wraps the same
/// session file family with its own root/profile resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PiSessionDialect {
    Pi,
    Omp,
}

impl PiSessionDialect {
    pub fn as_str(self) -> &'static str {
        match self {
            PiSessionDialect::Pi => "pi",
            PiSessionDialect::Omp => "omp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentSessionSource {
    Cli,
    DesktopApp,
    Ide,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentSessionState {
    Active,
    Idle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionWorkspace {
    pub cwd: Option<String>,
    pub project_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionActivity {
    pub started_at: Option<DateTime<Utc>>,
    pub last_activity_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum AgentSessionFocusTarget {
    Process { pid: u32 },
    Transcript { transcript_path: String },
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSession {
    pub id: String,
    pub provider: AgentSessionProvider,
    /// Pi-family dialect; absent for Codex/Claude sessions (kept optional for
    /// the legacy v1 remote JSON sniffed by older hosts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialect: Option<PiSessionDialect>,
    /// Optional human title (Pi-family `session_info`/`title` records).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    pub source: AgentSessionSource,
    pub state: AgentSessionState,
    pub pid: Option<u32>,
    pub transcript_path: Option<String>,
    pub host: String,
    pub workspace: AgentSessionWorkspace,
    pub activity: AgentSessionActivity,
    pub focus_target: AgentSessionFocusTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionHostResult {
    pub host: String,
    pub sessions: Vec<AgentSession>,
    pub error: Option<String>,
}

impl AgentSessionHostResult {
    pub fn success(host: impl Into<String>, sessions: Vec<AgentSession>) -> Self {
        Self {
            host: host.into(),
            sessions,
            error: None,
        }
    }

    pub fn failed(host: impl Into<String>, message: impl std::fmt::Display) -> Self {
        Self {
            host: host.into(),
            sessions: Vec::new(),
            error: Some(crate::logging::safe_error_message(message)),
        }
    }

    pub fn from_json(body: &str) -> Result<Self, String> {
        RemoteSessionFetcher::decode_host_result(body)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionScanConfig {
    pub active_window: Duration,
    pub file_only_window: Duration,
    /// Upstream: cap on live agent processes correlated per host.
    pub max_process_count: usize,
    /// Directory-scan guard rails for session roots (pi/OMP family).
    pub max_directory_entry_count: usize,
    pub max_directory_depth: usize,
    pub directory_scan_budget: Duration,
    pub adaptive_directory_scan_budget: Duration,
}

impl Default for SessionScanConfig {
    fn default() -> Self {
        Self {
            active_window: Duration::from_secs(120),
            file_only_window: Duration::from_secs(30 * 60),
            max_process_count: 64,
            max_directory_entry_count: 512,
            max_directory_depth: 1,
            directory_scan_budget: Duration::from_millis(250),
            adaptive_directory_scan_budget: Duration::from_millis(150),
        }
    }
}

impl SessionScanConfig {
    pub fn state(
        &self,
        last_activity_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
        has_live_process: bool,
    ) -> AgentSessionState {
        match last_activity_at {
            Some(last_activity_at) => {
                let age = now.signed_duration_since(last_activity_at);
                let active_window = ChronoDuration::from_std(self.active_window)
                    .unwrap_or_else(|_| ChronoDuration::seconds(120));
                if age <= active_window {
                    AgentSessionState::Active
                } else {
                    AgentSessionState::Idle
                }
            }
            None if has_live_process => AgentSessionState::Active,
            None => AgentSessionState::Idle,
        }
    }

    pub fn file_only_session_allowed(
        &self,
        modified_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> bool {
        let age = now.signed_duration_since(modified_at);
        let file_window = ChronoDuration::from_std(self.file_only_window)
            .unwrap_or_else(|_| ChronoDuration::seconds(30 * 60));
        age <= file_window
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentProcessKind {
    Agent,
    Helper,
    AppServer,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProcessRecord {
    pub pid: u32,
    pub ppid: u32,
    pub started_at: Option<DateTime<Utc>>,
    pub provider: Option<AgentSessionProvider>,
    pub source: AgentSessionSource,
    pub executable: String,
    pub kind: AgentProcessKind,
    /// Full command line when the platform surfaces it (Win32 CIM / ps).
    /// Pi-family dialect detection and `--session-dir`/`--profile` flags
    /// live here rather than in the display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

impl AgentProcessRecord {
    pub fn is_agent(&self) -> bool {
        self.kind == AgentProcessKind::Agent
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeTranscript {
    pub url: PathBuf,
    pub modified_at: DateTime<Utc>,
}

impl ClaudeTranscript {
    pub fn new(url: PathBuf, modified_at: DateTime<Utc>) -> Self {
        Self { url, modified_at }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexRolloutMetadata {
    pub session_id: String,
    pub cwd: Option<String>,
    pub originator: Option<String>,
    pub source: Option<String>,
}

impl CodexRolloutMetadata {
    pub fn session_source(&self) -> AgentSessionSource {
        let value = [self.originator.as_deref(), self.source.as_deref()]
            .into_iter()
            .flatten()
            .map(|part| part.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join(" ");

        if value.contains("desktop") || value.contains("app-server") {
            AgentSessionSource::DesktopApp
        } else if value.contains("ide")
            || value.contains("vscode")
            || value.contains("cursor")
            || value.contains("zed")
        {
            AgentSessionSource::Ide
        } else if value.contains("codex_exec") || value.contains("exec") || value.contains("cli") {
            AgentSessionSource::Cli
        } else {
            AgentSessionSource::Unknown
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum SessionFocusResult {
    Focused,
    Unsupported { message: String },
    Failed { message: String },
}

impl SessionFocusResult {
    pub fn focused() -> Self {
        Self::Focused
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::Unsupported {
            message: crate::logging::safe_error_message(message.into()),
        }
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self::Failed {
            message: crate::logging::safe_error_message(message.into()),
        }
    }
}

pub struct AgentPSOutputParser;
pub struct WindowsProcessOutputParser;
pub struct LSOFCWDOutputParser;
pub struct ClaudeSessionProjectMapper;
pub struct ClaudeTranscriptMetadataParser;
pub struct CodexRolloutFirstLineParser;
pub struct AgentSessionCorrelation;
#[derive(Debug, Clone)]
pub struct RemoteSessionFetcher {
    pub per_host_timeout: Duration,
}
pub struct TailscaleStatusParser;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeTranscriptMetadata {
    pub session_id: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSessionDiscoveryMode {
    Disabled,
    Enabled { ssh_hosts: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "status", content = "hosts")]
pub enum AgentSessionDiscoveryResult {
    Disabled,
    Hosts(Vec<AgentSessionHostResult>),
}

#[derive(Debug, Clone)]
pub struct LocalAgentSessionScanner {
    pub config: SessionScanConfig,
    pub command_timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct AgentSessionDiscovery {
    local: LocalAgentSessionScanner,
    remote: RemoteSessionFetcher,
}

mod focus;
mod parsers;
pub mod pi_family;
mod remote;
pub use focus::focus_session;

struct CodexRollout {
    path: PathBuf,
    modified_at: DateTime<Utc>,
    metadata: CodexRolloutMetadata,
}

struct ClaudeTranscriptCandidate {
    path: PathBuf,
    modified_at: DateTime<Utc>,
    metadata: ClaudeTranscriptMetadata,
}

impl Default for LocalAgentSessionScanner {
    fn default() -> Self {
        Self {
            config: SessionScanConfig::default(),
            command_timeout: Duration::from_secs(5),
        }
    }
}

impl LocalAgentSessionScanner {
    pub fn new(config: SessionScanConfig, command_timeout: Duration) -> Self {
        Self {
            config,
            command_timeout,
        }
    }

    pub async fn scan(&self) -> AgentSessionHostResult {
        let host = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "localhost".to_string());
        let options = Self::process_options(self.command_timeout);
        let process_result = CommandRunner::new()
            .run_async("powershell.exe", None, &options)
            .await;
        let (processes, error) = match process_result {
            Ok(result) if result.timed_out => (
                Vec::new(),
                Some("Windows process discovery timed out; file-only sessions may still appear."),
            ),
            Ok(result) if result.exit_code == Some(0) => {
                (WindowsProcessOutputParser::parse(&result.text), None)
            }
            Ok(_) => (
                Vec::new(),
                Some(
                    "Windows process discovery failed; verify PowerShell and CIM access. File-only sessions may still appear.",
                ),
            ),
            Err(_) => (
                Vec::new(),
                Some(
                    "Unable to launch PowerShell for process discovery; file-only sessions may still appear.",
                ),
            ),
        };

        let now = Utc::now();
        let sessions = self.scan_files(
            &host,
            now,
            &Self::codex_sessions_root(),
            &Self::claude_projects_roots(),
            &processes,
        );
        AgentSessionHostResult {
            host,
            sessions,
            error: error.map(crate::logging::safe_error_message),
        }
    }

    fn process_options(timeout: Duration) -> CommandOptions {
        CommandOptions {
            timeout,
            initial_delay: Duration::ZERO,
            extra_args: vec![
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-Command".to_string(),
                concat!(
                    "$ErrorActionPreference='Stop';",
                    "$processes=@(Get-CimInstance Win32_Process | ",
                    "Select-Object ProcessId,ParentProcessId,CreationDate,Name,ExecutablePath);",
                    "ConvertTo-Json -Compress -InputObject $processes"
                )
                .to_string(),
            ],
            ..CommandOptions::default()
        }
    }

    fn scan_files(
        &self,
        host: &str,
        now: DateTime<Utc>,
        codex_root: &Path,
        claude_roots: &[PathBuf],
        processes: &[AgentProcessRecord],
    ) -> Vec<AgentSession> {
        let mut agents = AgentPSOutputParser::agent_processes(processes);
        agents.sort_by_key(|process| std::cmp::Reverse(process.started_at));
        // Pi-family processes are correlated in a dedicated dialect-aware
        // pass (upstream 0.48.0 #2626); split them out before the
        // codex/claude loop consumes its records.
        let (pi_processes, agents): (Vec<_>, Vec<_>) = agents
            .into_iter()
            .partition(|process| process.provider == Some(AgentSessionProvider::Pi));
        let mut rollouts = VecDeque::from(Self::codex_rollouts(
            codex_root,
            now.with_timezone(&Local).date_naive(),
        ));
        let claude_count = agents
            .iter()
            .filter(|process| process.provider == Some(AgentSessionProvider::Claude))
            .count();
        let mut claude_transcripts =
            VecDeque::from(Self::claude_transcripts(claude_roots, claude_count));
        let mut sessions = Vec::new();

        for process in agents {
            match process.provider {
                Some(AgentSessionProvider::Codex) => sessions.push(self.codex_process_session(
                    host,
                    now,
                    process,
                    rollouts.pop_front(),
                )),
                Some(AgentSessionProvider::Claude) => sessions.push(self.claude_process_session(
                    host,
                    now,
                    process,
                    claude_transcripts.pop_front(),
                )),
                Some(AgentSessionProvider::Pi) | None => {}
            }
        }

        // Pi-family discovery gets its own bounded directory budget so a
        // malformed/unreadable OMP root can never starve Codex/Claude
        // correlation (upstream #2626 shared-provider regression guard).
        let pi_input = pi_family::PiFamilyScanInput {
            processes: &pi_processes,
            cwd_by_pid: std::collections::HashMap::new(),
            environment: pi_family::PiFamilySessionScanner::scan_environment(),
            now,
            host: host.to_string(),
            config: self.config,
        };
        let mut pi_budget = pi_family::budget_for(&self.config);
        sessions.extend(pi_family::PiFamilySessionScanner::scan(
            &pi_input,
            &mut pi_budget,
        ));

        sessions.extend(
            rollouts
                .into_iter()
                .filter_map(|rollout| self.codex_file_session(host, now, rollout)),
        );
        sessions.sort_by(|lhs, rhs| {
            (rhs.state == AgentSessionState::Active)
                .cmp(&(lhs.state == AgentSessionState::Active))
                .then_with(|| {
                    rhs.activity
                        .last_activity_at
                        .or(rhs.activity.started_at)
                        .cmp(&lhs.activity.last_activity_at.or(lhs.activity.started_at))
                })
        });
        let mut seen = HashSet::new();
        sessions.retain(|session| seen.insert(format!("{}:{}", session.host, session.id)));
        sessions
    }

    fn codex_process_session(
        &self,
        host: &str,
        now: DateTime<Utc>,
        process: AgentProcessRecord,
        rollout: Option<CodexRollout>,
    ) -> AgentSession {
        let cwd = rollout
            .as_ref()
            .and_then(|rollout| rollout.metadata.cwd.clone());
        let source = rollout
            .as_ref()
            .map(|rollout| rollout.metadata.session_source())
            .filter(|source| *source != AgentSessionSource::Unknown)
            .unwrap_or(process.source);
        let modified_at = rollout.as_ref().map(|rollout| rollout.modified_at);
        let transcript_path = rollout
            .as_ref()
            .map(|rollout| rollout.path.to_string_lossy().to_string());
        AgentSession {
            id: rollout
                .as_ref()
                .map(|rollout| rollout.metadata.session_id.clone())
                .unwrap_or_else(|| format!("pid:{}", process.pid)),
            provider: AgentSessionProvider::Codex,
            dialect: None,
            session_name: None,
            source,
            state: self.config.state(modified_at, now, true),
            pid: Some(process.pid),
            transcript_path,
            host: host.to_string(),
            workspace: AgentSessionWorkspace {
                project_name: cwd.as_deref().and_then(project_name_from_cwd),
                cwd,
            },
            activity: AgentSessionActivity {
                started_at: process.started_at,
                last_activity_at: modified_at,
            },
            focus_target: AgentSessionFocusTarget::Process { pid: process.pid },
        }
    }

    fn claude_process_session(
        &self,
        host: &str,
        now: DateTime<Utc>,
        process: AgentProcessRecord,
        transcript: Option<ClaudeTranscriptCandidate>,
    ) -> AgentSession {
        let cwd = transcript
            .as_ref()
            .and_then(|transcript| transcript.metadata.cwd.clone());
        let modified_at = transcript.as_ref().map(|transcript| transcript.modified_at);
        let transcript_path = transcript
            .as_ref()
            .map(|transcript| transcript.path.to_string_lossy().to_string());
        let id = transcript
            .as_ref()
            .and_then(|transcript| transcript.metadata.session_id.clone())
            .or_else(|| {
                transcript.as_ref().and_then(|transcript| {
                    transcript
                        .path
                        .file_stem()
                        .and_then(|name| name.to_str())
                        .map(str::to_owned)
                })
            })
            .unwrap_or_else(|| format!("pid:{}", process.pid));
        AgentSession {
            id,
            provider: AgentSessionProvider::Claude,
            dialect: None,
            session_name: None,
            source: process.source,
            state: self.config.state(modified_at, now, true),
            pid: Some(process.pid),
            transcript_path,
            host: host.to_string(),
            workspace: AgentSessionWorkspace {
                project_name: cwd.as_deref().and_then(project_name_from_cwd),
                cwd,
            },
            activity: AgentSessionActivity {
                started_at: process.started_at,
                last_activity_at: modified_at,
            },
            focus_target: AgentSessionFocusTarget::Process { pid: process.pid },
        }
    }

    fn codex_file_session(
        &self,
        host: &str,
        now: DateTime<Utc>,
        rollout: CodexRollout,
    ) -> Option<AgentSession> {
        self.config
            .file_only_session_allowed(rollout.modified_at, now)
            .then(|| {
                let source = rollout.metadata.session_source();
                let cwd = rollout.metadata.cwd;
                let transcript_path = rollout.path.to_string_lossy().to_string();
                AgentSession {
                    id: rollout.metadata.session_id,
                    provider: AgentSessionProvider::Codex,
                    dialect: None,
                    session_name: None,
                    source,
                    state: self.config.state(Some(rollout.modified_at), now, false),
                    pid: None,
                    transcript_path: Some(transcript_path.clone()),
                    host: host.to_string(),
                    workspace: AgentSessionWorkspace {
                        project_name: cwd.as_deref().and_then(project_name_from_cwd),
                        cwd,
                    },
                    activity: AgentSessionActivity {
                        started_at: None,
                        last_activity_at: Some(rollout.modified_at),
                    },
                    focus_target: AgentSessionFocusTarget::Transcript { transcript_path },
                }
            })
    }

    fn codex_sessions_root() -> PathBuf {
        if let Ok(path) = std::env::var("CODEX_HOME")
            && !path.trim().is_empty()
        {
            let path = PathBuf::from(path.trim());
            if path
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("sessions"))
            {
                return path;
            }
            return path.join("sessions");
        }
        dirs::home_dir()
            .unwrap_or_default()
            .join(".codex")
            .join("sessions")
    }

    fn claude_projects_roots() -> Vec<PathBuf> {
        if let Ok(path) = std::env::var("CLAUDE_CONFIG_DIR")
            && !path.trim().is_empty()
        {
            return vec![PathBuf::from(path.trim()).join("projects")];
        }
        dirs::home_dir()
            .map(|home| vec![home.join(".claude").join("projects")])
            .unwrap_or_default()
    }

    fn codex_day_directories(root: &Path, today: NaiveDate) -> Vec<PathBuf> {
        [today, today - ChronoDuration::days(1)]
            .into_iter()
            .map(|date| {
                root.join(date.format("%Y").to_string())
                    .join(date.format("%m").to_string())
                    .join(date.format("%d").to_string())
            })
            .collect()
    }

    fn codex_rollouts(root: &Path, today: NaiveDate) -> Vec<CodexRollout> {
        let mut rollouts = Vec::new();
        for directory in Self::codex_day_directories(root, today) {
            let Ok(entries) = fs::read_dir(directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let is_rollout = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("rollout-"));
                if !is_rollout || path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                    continue;
                }
                let Some(line) = CodexRolloutFirstLineParser::read_first_line(&path) else {
                    continue;
                };
                let Some(metadata) = CodexRolloutFirstLineParser::parse(&line) else {
                    continue;
                };
                let Some(modified_at) = entry
                    .metadata()
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .map(DateTime::<Utc>::from)
                else {
                    continue;
                };
                rollouts.push(CodexRollout {
                    path,
                    modified_at,
                    metadata,
                });
            }
        }
        rollouts.sort_by_key(|rollout| std::cmp::Reverse(rollout.modified_at));
        rollouts
    }

    fn claude_transcripts(
        roots: &[PathBuf],
        live_process_count: usize,
    ) -> Vec<ClaudeTranscriptCandidate> {
        if live_process_count == 0 {
            return Vec::new();
        }
        let mut files = Vec::new();
        for root in roots {
            let Ok(projects) = fs::read_dir(root) else {
                continue;
            };
            for project in projects.flatten() {
                let Ok(entries) = fs::read_dir(project.path()) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                        continue;
                    }
                    let Some(modified_at) = entry
                        .metadata()
                        .ok()
                        .and_then(|metadata| metadata.modified().ok())
                        .map(DateTime::<Utc>::from)
                    else {
                        continue;
                    };
                    files.push((path, modified_at));
                }
            }
        }
        files.sort_by(|lhs, rhs| rhs.1.cmp(&lhs.1).then_with(|| rhs.0.cmp(&lhs.0)));
        files
            .into_iter()
            .take(live_process_count)
            .filter_map(|(path, modified_at)| {
                let file = File::open(&path).ok()?;
                let metadata = ClaudeTranscriptMetadataParser::parse(file)?;
                Some(ClaudeTranscriptCandidate {
                    path,
                    modified_at,
                    metadata,
                })
            })
            .collect()
    }
}

impl AgentSessionDiscovery {
    pub fn new(local: LocalAgentSessionScanner, remote: RemoteSessionFetcher) -> Self {
        Self { local, remote }
    }

    pub async fn scan(&self, mode: AgentSessionDiscoveryMode) -> AgentSessionDiscoveryResult {
        let AgentSessionDiscoveryMode::Enabled { ssh_hosts } = mode else {
            return AgentSessionDiscoveryResult::Disabled;
        };
        let (local, automatic) =
            tokio::join!(self.local.scan(), RemoteSessionFetcher::tailscale_hosts());
        let (automatic_hosts, tailscale_error) = match automatic {
            Ok(hosts) => (hosts, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        let merged_hosts = RemoteSessionFetcher::merge_hosts(&ssh_hosts, &automatic_hosts);
        let mut remote = self.remote.fetch(&merged_hosts).await;
        if let Some(error) = tailscale_error {
            remote.push(AgentSessionHostResult::failed("tailscale", error));
        }
        let mut hosts = Vec::with_capacity(remote.len() + 1);
        hosts.push(local);
        hosts.extend(remote);
        AgentSessionDiscoveryResult::Hosts(hosts)
    }
}

impl Default for AgentSessionDiscovery {
    fn default() -> Self {
        Self::new(
            LocalAgentSessionScanner::default(),
            RemoteSessionFetcher::default(),
        )
    }
}

fn actionable_message(label: &str, err: impl std::fmt::Display) -> String {
    crate::logging::safe_error_message(format!("{label}: {err}"))
}

fn is_safe_host_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']' | '_' | '@')
}

fn project_name_from_cwd(cwd: &str) -> Option<String> {
    let trimmed = cwd.trim().trim_end_matches(['\\', '/']);
    let path = Path::new(trimmed);
    let name = path.file_name()?.to_str()?.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

include!("agent_sessions/tests.rs");
