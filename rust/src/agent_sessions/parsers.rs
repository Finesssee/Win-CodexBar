use super::*;

impl TailscaleStatusParser {
    pub fn hosts(json: &str) -> Result<Vec<String>, String> {
        let status: serde_json::Value =
            serde_json::from_str(json).map_err(|_| "invalid Tailscale status JSON".to_string())?;
        let mut hosts = status
            .get("Peer")
            .and_then(serde_json::Value::as_object)
            .into_iter()
            .flat_map(|peers| peers.values())
            .filter(|peer| peer.get("Online").and_then(serde_json::Value::as_bool) == Some(true))
            .filter_map(|peer| peer.get("DNSName").and_then(serde_json::Value::as_str))
            .map(|host| host.trim().trim_end_matches('.').to_string())
            .filter(|host| !host.is_empty())
            .filter(|host| RemoteSessionFetcher::validate_host(host).is_ok())
            .collect::<Vec<_>>();
        hosts.sort_by_key(|host| host.to_ascii_lowercase());
        Ok(RemoteSessionFetcher::sanitized_hosts(&hosts))
    }
}

impl AgentPSOutputParser {
    pub fn parse(output: &str) -> Vec<AgentProcessRecord> {
        let mut seen_pids = HashSet::new();
        output
            .lines()
            .filter_map(|line| Self::parse_line(line, &mut seen_pids))
            .collect()
    }

    pub fn agent_processes(records: &[AgentProcessRecord]) -> Vec<AgentProcessRecord> {
        let mut seen = HashSet::new();
        records
            .iter()
            .filter(|record| record.is_agent())
            .filter_map(|record| {
                if seen.insert(record.pid) {
                    Some(record.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn provider(record: &AgentProcessRecord) -> Option<AgentSessionProvider> {
        record.provider
    }

    pub fn source(record: &AgentProcessRecord) -> AgentSessionSource {
        record.source
    }

    pub fn has_codex_app_server(records: &[AgentProcessRecord]) -> bool {
        records.iter().any(|record| {
            record.kind == AgentProcessKind::AppServer
                && record.provider == Some(AgentSessionProvider::Codex)
        })
    }

    fn parse_line(line: &str, seen_pids: &mut HashSet<u32>) -> Option<AgentProcessRecord> {
        let mut fields = line.split_whitespace();
        let pid = fields.next()?.parse::<u32>().ok()?;
        let ppid = fields.next()?.parse::<u32>().ok()?;
        let weekday = fields.next()?;
        let month = fields.next()?;
        let day = fields.next()?;
        let time = fields.next()?;
        let year = fields.next()?;
        if !seen_pids.insert(pid) {
            return None;
        }

        let started_at = Self::parse_started_at(weekday, month, day, time, year)?;
        let command = fields.collect::<Vec<_>>().join(" ");
        let classification = classify_process_command(&command);
        Some(AgentProcessRecord {
            pid,
            ppid,
            started_at: Some(started_at),
            provider: classification.provider,
            source: classification.source,
            executable: classification.executable,
            kind: classification.kind,
            command: Some(command),
        })
    }

    fn parse_started_at(
        weekday: &str,
        month: &str,
        day: &str,
        time: &str,
        year: &str,
    ) -> Option<DateTime<Utc>> {
        let text = format!("{weekday} {month} {day} {time} {year}");
        chrono::NaiveDateTime::parse_from_str(&text, "%a %b %e %H:%M:%S %Y")
            .ok()
            .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WindowsProcessMetadata {
    process_id: u32,
    #[serde(default)]
    parent_process_id: u32,
    creation_date: Option<String>,
    name: Option<String>,
    executable_path: Option<String>,
}

impl WindowsProcessOutputParser {
    pub fn parse(output: &str) -> Vec<AgentProcessRecord> {
        let Ok(value) = serde_json::from_str::<Value>(output.trim()) else {
            return Vec::new();
        };
        let values = match value {
            Value::Array(values) => values,
            Value::Object(_) => vec![value],
            _ => return Vec::new(),
        };
        let mut seen = HashSet::new();

        values
            .into_iter()
            .filter_map(|value| serde_json::from_value::<WindowsProcessMetadata>(value).ok())
            .filter(|process| process.process_id > 0 && seen.insert(process.process_id))
            .map(|process| {
                let display_name = process
                    .name
                    .as_deref()
                    .filter(|name| !name.trim().is_empty())
                    .or(process.executable_path.as_deref())
                    .unwrap_or_default();
                let classification = classify_process_command(display_name);
                AgentProcessRecord {
                    pid: process.process_id,
                    ppid: process.parent_process_id,
                    started_at: process
                        .creation_date
                        .as_deref()
                        .and_then(parse_windows_creation_date),
                    provider: classification.provider,
                    source: classification.source,
                    executable: process.name.unwrap_or(classification.executable),
                    kind: classification.kind,
                    // Windows deliberately does not harvest other processes'
                    // command lines (privacy); ps-based remote scans carry
                    // them, and tests inject them.
                    command: None,
                }
            })
            .collect()
    }
}

fn parse_windows_creation_date(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.with_timezone(&Utc))
        .or_else(|| {
            let value = value.strip_prefix("/Date(")?;
            let milliseconds = value
                .trim_end_matches(")/")
                .split(['+', '-'])
                .next()?
                .parse()
                .ok()?;
            DateTime::<Utc>::from_timestamp_millis(milliseconds)
        })
        .or_else(|| {
            let core = value.get(..21)?;
            chrono::NaiveDateTime::parse_from_str(core, "%Y%m%d%H%M%S%.f")
                .ok()
                .map(|date| DateTime::<Utc>::from_naive_utc_and_offset(date, Utc))
        })
}

struct ProcessClassification {
    provider: Option<AgentSessionProvider>,
    source: AgentSessionSource,
    executable: String,
    kind: AgentProcessKind,
}

fn classify_process_command(command: &str) -> ProcessClassification {
    let lower = command.to_ascii_lowercase();
    let executable = executable_basename(command);

    if lower.contains("app-server") && lower.contains("codex") {
        return ProcessClassification {
            provider: Some(AgentSessionProvider::Codex),
            source: AgentSessionSource::DesktopApp,
            executable,
            kind: AgentProcessKind::AppServer,
        };
    }

    // Pi-family CLIs (upstream 0.48.0 #2626): `pi` and OMP share one session
    // family; the dialect comes from the invocation, obvious helpers
    // (`--help`, `--version`, `--smoke-test`, `__omp_worker_`) are filtered
    // inside the family module instead of the generic helper block.
    if crate::agent_sessions::pi_family::pi_family_dialect(command).is_some()
        && !crate::agent_sessions::pi_family::is_pi_family_helper(command)
    {
        return ProcessClassification {
            provider: Some(AgentSessionProvider::Pi),
            source: AgentSessionSource::Cli,
            executable: crate::agent_sessions::pi_family::pi_family_executable(command)
                .unwrap_or(executable),
            kind: AgentProcessKind::Agent,
        };
    }

    if lower.contains("codex (renderer)")
        || lower.contains("claude-code-acp")
        || lower.contains("--help")
        || lower.contains("--version")
        || lower.contains("--type=renderer")
        || lower.contains("disclaimer")
        || executable.eq_ignore_ascii_case("disclaimer")
    {
        return ProcessClassification {
            provider: None,
            source: AgentSessionSource::Unknown,
            executable,
            kind: AgentProcessKind::Helper,
        };
    }

    if lower.contains("application support/claude/claude-code/claude")
        || lower.contains("claude.app")
        || lower.contains("claude.exe")
        || executable.eq_ignore_ascii_case("claude")
    {
        return ProcessClassification {
            provider: Some(AgentSessionProvider::Claude),
            source: if lower.contains("application support/claude/claude-code")
                || lower.contains("claude.app")
            {
                AgentSessionSource::DesktopApp
            } else {
                AgentSessionSource::Cli
            },
            executable: if executable.eq_ignore_ascii_case("claude") {
                "claude".to_string()
            } else {
                executable
            },
            kind: AgentProcessKind::Agent,
        };
    }

    if lower.contains("codex.exe")
        || lower.contains("codex.app")
        || lower.contains("codex desktop")
        || executable.eq_ignore_ascii_case("codex")
    {
        return ProcessClassification {
            provider: Some(AgentSessionProvider::Codex),
            source: if lower.contains("codex.app") || lower.contains("codex desktop") {
                AgentSessionSource::DesktopApp
            } else {
                AgentSessionSource::Cli
            },
            executable: if executable.eq_ignore_ascii_case("codex") {
                "codex".to_string()
            } else {
                executable
            },
            kind: AgentProcessKind::Agent,
        };
    }

    ProcessClassification {
        provider: None,
        source: AgentSessionSource::Unknown,
        executable,
        kind: AgentProcessKind::Other,
    }
}

fn executable_basename(command: &str) -> String {
    let normalized = command.replace('\\', "/").to_ascii_lowercase();
    for (needle, name) in [
        ("claude-code-acp", "claude-code-acp"),
        ("application support/claude/claude-code/claude", "claude"),
        ("codex app-server", "codex"),
        ("codex (renderer)", "codex"),
        ("codex.app", "codex"),
        ("claude.app", "claude"),
        ("claude.exe", "claude"),
        ("codex.exe", "codex"),
        ("disclaimer", "disclaimer"),
    ] {
        if normalized.contains(needle) {
            return name.to_string();
        }
    }

    let first_token = command.split_whitespace().next().unwrap_or_default();
    if first_token.is_empty() {
        return String::new();
    }

    Path::new(first_token)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(first_token)
        .to_string()
}

impl LSOFCWDOutputParser {
    pub fn parse(output: &str) -> HashMap<u32, String> {
        let mut result = HashMap::new();
        let mut current_pid = None;

        for line in output.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            match line.chars().next() {
                Some('p') => {
                    current_pid = line[1..].trim().parse::<u32>().ok();
                }
                Some('n') => {
                    if let Some(pid) = current_pid {
                        result.insert(pid, line[1..].to_string());
                    }
                }
                _ => {}
            }
        }

        result
    }
}

impl ClaudeSessionProjectMapper {
    pub fn escaped_cwd(cwd: &str) -> String {
        cwd.chars()
            .map(|scalar| {
                if scalar.is_ascii_alphanumeric() {
                    scalar
                } else {
                    '-'
                }
            })
            .collect()
    }

    pub fn project_directories(cwd: &str, home_directory: &Path) -> Vec<PathBuf> {
        if cwd.trim().is_empty() {
            return Vec::new();
        }

        vec![
            home_directory
                .join(".claude")
                .join("projects")
                .join(Self::escaped_cwd(cwd)),
        ]
    }

    pub fn transcripts(cwd: &str, home_directory: &Path) -> Vec<ClaudeTranscript> {
        let mut transcripts = Vec::new();

        for directory in Self::project_directories(cwd, home_directory) {
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                    continue;
                }

                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                let Ok(modified) = metadata.modified() else {
                    continue;
                };

                transcripts.push(ClaudeTranscript::new(path, modified.into()));
            }
        }

        transcripts.sort_by(|lhs, rhs| {
            rhs.modified_at
                .cmp(&lhs.modified_at)
                .then_with(|| rhs.url.cmp(&lhs.url))
        });
        transcripts
    }

    pub fn newest_transcript(cwd: &str, home_directory: &Path) -> Option<ClaudeTranscript> {
        Self::transcripts(cwd, home_directory).into_iter().next()
    }
}

impl ClaudeTranscriptMetadataParser {
    const MAX_LINES: usize = 32;
    const MAX_BYTES: u64 = 64 * 1024;

    pub fn parse(reader: impl Read) -> Option<ClaudeTranscriptMetadata> {
        let mut session_id = None;
        let mut cwd = None;
        let reader = BufReader::new(reader.take(Self::MAX_BYTES));

        for line in reader.lines().take(Self::MAX_LINES).map_while(Result::ok) {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if session_id.is_none() {
                session_id = value
                    .get("sessionId")
                    .or_else(|| value.get("session_id"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            if cwd.is_none() {
                cwd = value.get("cwd").and_then(Value::as_str).map(str::to_owned);
            }
            if session_id.is_some() && cwd.is_some() {
                break;
            }
        }

        (session_id.is_some() || cwd.is_some())
            .then_some(ClaudeTranscriptMetadata { session_id, cwd })
    }
}

impl CodexRolloutFirstLineParser {
    pub fn parse(line: &str) -> Option<CodexRolloutMetadata> {
        let value: Value = serde_json::from_str(line).ok()?;
        if value.get("type")?.as_str()? != "session_meta" {
            return None;
        }

        let payload = value.get("payload")?.as_object()?;
        let session_id = payload
            .get("session_id")
            .or_else(|| payload.get("id"))?
            .as_str()?;
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return None;
        }

        Some(CodexRolloutMetadata {
            session_id: session_id.to_string(),
            cwd: payload
                .get("cwd")
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned),
            originator: payload
                .get("originator")
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned),
            source: payload
                .get("source")
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned),
        })
    }

    pub fn read_first_line(path: &Path) -> Option<String> {
        let file = File::open(path).ok()?;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).ok()?;
        if bytes == 0 {
            return None;
        }
        while line.ends_with(['\n', '\r']) {
            line.pop();
        }
        Some(line)
    }
}

impl AgentSessionCorrelation {
    pub fn project_name(cwd: Option<&str>) -> Option<String> {
        cwd.and_then(project_name_from_cwd)
    }
}
