#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_sessions::{
        AgentSession, AgentSessionActivity, AgentSessionFocusTarget, AgentSessionProvider,
        AgentSessionSource, AgentSessionState, AgentSessionWorkspace,
    };

    fn sample() -> AgentSession {
        AgentSession {
            id: "session-1".into(),
            provider: AgentSessionProvider::Codex,
            dialect: None,
            session_name: None,
            source: AgentSessionSource::Cli,
            state: AgentSessionState::Active,
            pid: Some(42),
            transcript_path: None,
            host: "DESKTOP".into(),
            workspace: AgentSessionWorkspace {
                cwd: Some(r"C:\work\demo".into()),
                project_name: Some("demo".into()),
            },
            activity: AgentSessionActivity {
                started_at: None,
                last_activity_at: None,
            },
            focus_target: AgentSessionFocusTarget::Process { pid: 42 },
        }
    }

    #[test]
    fn brief_output_is_one_safe_line() {
        let line = render_brief(&sample());
        assert_eq!(line, "active codex cli DESKTOP demo (session-1)");
        assert!(!line.contains("C:\\work"));
    }

    #[test]
    fn json_output_contains_typed_session_fields() {
        let output = serde_json::to_string(&sample()).expect("session JSON");
        assert!(output.contains("\"provider\":\"codex\""));
        assert!(output.contains("\"focusTarget\""));
        assert!(!output.contains("rawCommand"));
    }

    #[test]
    fn legacy_json_filters_pi_family_and_v2_keeps_them() {
        let mut pi_session = sample();
        pi_session.id = "pid:7".into();
        pi_session.provider = crate::agent_sessions::AgentSessionProvider::Pi;
        pi_session.dialect = Some(crate::agent_sessions::PiSessionDialect::Omp);
        let sessions = vec![sample(), pi_session];

        let legacy = sessions_for_json(&sessions, false);
        let v2 = sessions_for_json(&sessions, true);

        assert_eq!(legacy.len(), 1);
        assert_eq!(v2.len(), 2);
        assert_eq!(
            v2[1].dialect,
            Some(crate::agent_sessions::PiSessionDialect::Omp)
        );
        // v2 rows carry dialect; v1 rows never emit it.
        let legacy_output = serde_json::to_string(legacy[0]).unwrap();
        assert!(!legacy_output.contains("dialect"));
        let v2_output = serde_json::to_string(v2[1]).unwrap();
        assert!(v2_output.contains("\"dialect\":\"omp\""));
    }

    #[test]
    fn legacy_v1_remote_array_still_decodes() {
        // An older host's `--json` (v1) array has no dialect/sessionName keys.
        let legacy_json = serde_json::to_string(&sample()).unwrap();
        let decoded: AgentSession = serde_json::from_str(&legacy_json).unwrap();
        assert_eq!(decoded.dialect, None);
        assert_eq!(decoded.session_name, None);
    }
}
use crate::agent_sessions::{
    AgentSession, AgentSessionDiscovery, AgentSessionDiscoveryMode, AgentSessionDiscoveryResult,
    SessionFocusResult, focus_session,
};
use clap::Args;
use serde::Serialize;

#[derive(Args, Debug, Default)]
pub struct SessionsArgs {
    /// Emit machine-readable session data (legacy v1: Codex and Claude only).
    #[arg(long)]
    pub json: bool,

    /// Emit complete JSON, including Pi-family sessions (upstream 0.48.0 v2).
    #[arg(long = "json-v2")]
    pub json_v2: bool,

    /// Pretty-print JSON output.
    #[arg(long)]
    pub pretty: bool,

    /// Print one compact line per session.
    #[arg(long)]
    pub brief: bool,

    /// Discover sessions on a configured SSH host (repeatable or comma-separated).
    #[arg(long = "ssh-host", value_delimiter = ',')]
    pub ssh_hosts: Vec<String>,

    /// Focus one session by its stable id.
    #[arg(long)]
    pub focus: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionsOutput<'a> {
    sessions: Vec<&'a AgentSession>,
    errors: Vec<&'a str>,
}

pub async fn run(args: SessionsArgs) -> anyhow::Result<()> {
    let discovery = AgentSessionDiscovery::default();
    let result = discovery
        .scan(AgentSessionDiscoveryMode::Enabled {
            ssh_hosts: args.ssh_hosts,
        })
        .await;
    let sessions = match &result {
        AgentSessionDiscoveryResult::Disabled => Vec::new(),
        AgentSessionDiscoveryResult::Hosts(hosts) => hosts
            .iter()
            .flat_map(|host| host.sessions.iter().cloned())
            .collect::<Vec<_>>(),
    };
    let errors = match &result {
        AgentSessionDiscoveryResult::Disabled => Vec::new(),
        AgentSessionDiscoveryResult::Hosts(hosts) => hosts
            .iter()
            .filter_map(|host| host.error.as_deref())
            .collect::<Vec<_>>(),
    };

    if let Some(id) = args.focus.as_deref() {
        let outcome = sessions
            .iter()
            .find(|session| session.id == id)
            .map(focus_session)
            .unwrap_or_else(|| SessionFocusResult::failed("Session was not found."));
        if args.json || args.json_v2 {
            println!(
                "{}",
                if args.pretty {
                    serde_json::to_string_pretty(&outcome)?
                } else {
                    serde_json::to_string(&outcome)?
                }
            );
        } else {
            print_focus_result(id, &outcome);
        }
        return Ok(());
    }

    if args.json || args.json_v2 {
        let output = SessionsOutput {
            sessions: sessions_for_json(&sessions, args.json_v2),
            errors,
        };
        println!(
            "{}",
            if args.pretty {
                serde_json::to_string_pretty(&output)?
            } else {
                serde_json::to_string(&output)?
            }
        );
    } else if args.brief {
        for session in &sessions {
            println!("{}", render_brief(session));
        }
        for error in errors {
            eprintln!("session discovery: {error}");
        }
    } else {
        for session in &sessions {
            println!(
                "{} {} on {} ({})",
                provider_label(session),
                session
                    .workspace
                    .project_name
                    .as_deref()
                    .unwrap_or("unknown workspace"),
                session.host,
                session.id
            );
        }
        for error in errors {
            eprintln!("session discovery: {error}");
        }
    }

    Ok(())
}

fn print_focus_result(id: &str, result: &SessionFocusResult) {
    match result {
        SessionFocusResult::Focused => println!("focused {id}"),
        SessionFocusResult::Unsupported { message } | SessionFocusResult::Failed { message } => {
            eprintln!("unable to focus {id}: {message}")
        }
    }
}

/// Upstream 0.48.0 #2626 protocol split: legacy `--json` (v1) arrays carry
/// only Codex and Claude sessions (older remote clients keep decoding);
/// `--json-v2` emits the complete set including Pi-family dialects.
fn sessions_for_json(sessions: &[AgentSession], include_pi_family: bool) -> Vec<&AgentSession> {
    if include_pi_family {
        sessions.iter().collect()
    } else {
        sessions
            .iter()
            .filter(|session| {
                !matches!(
                    session.provider,
                    crate::agent_sessions::AgentSessionProvider::Pi
                )
            })
            .collect()
    }
}

fn render_brief(session: &AgentSession) -> String {
    format!(
        "{} {} {} {} {} ({})",
        format!("{:?}", session.state).to_ascii_lowercase(),
        provider_label(session),
        format!("{:?}", session.source).to_ascii_lowercase(),
        session.host,
        session
            .workspace
            .project_name
            .as_deref()
            .unwrap_or("unknown"),
        session.id
    )
}

fn provider_label(session: &AgentSession) -> &'static str {
    match session.provider {
        crate::agent_sessions::AgentSessionProvider::Codex => "codex",
        crate::agent_sessions::AgentSessionProvider::Claude => "claude",
        crate::agent_sessions::AgentSessionProvider::Pi => "pi",
    }
}
