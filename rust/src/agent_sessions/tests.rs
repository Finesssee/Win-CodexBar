#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::io;
    use std::sync::Arc;

    #[test]
    fn process_parser_filters_helpers_app_server_duplicates_and_malformed_lines() {
        let output = "\
101   1 Mon Jul  6 09:00:00 2026 /Applications/Claude.app/Contents/Resources/disclaimer /Users/test/Library/Application Support/Claude/claude-code/claude --dangerously-skip-permissions
102 101 Mon Jul  6 09:00:01 2026 /Users/test/Library/Application Support/Claude/claude-code/claude --dangerously-skip-permissions
102 101 Mon Jul  6 09:00:01 2026 /Users/test/Library/Application Support/Claude/claude-code/claude --dangerously-skip-permissions
201   1 Mon Jul  6 09:01:00 2026 /opt/homebrew/bin/codex exec --full-auto strange argv here
202   1 Mon Jul  6 09:02:00 2026 /Applications/Codex.app/Contents/Resources/codex app-server --listen stdio
203   1 Mon Jul  6 09:03:00 2026 /usr/local/bin/codex --help
301   1 Mon Jul  6 09:04:00 2026 /Users/test/.local/bin/claude-code-acp --stdio
401   1 Mon Jul  6 09:05:00 2026 /Applications/Codex.app/Contents/Frameworks/Codex Framework.framework/Helpers/Codex (Renderer) --type=renderer
bad line
";

        let records = AgentPSOutputParser::parse(output);
        let agents = AgentPSOutputParser::agent_processes(&records);

        assert_eq!(agents.len(), 2);
    }

    #[test]
    fn session_scan_config_cuts_off_active_and_file_only_windows() {
        let config = SessionScanConfig::default();
        let now = Utc.with_ymd_and_hms(2026, 7, 12, 0, 0, 0).unwrap();

        assert_eq!(
            config.state(Some(now - chrono::Duration::seconds(119)), now, true),
            AgentSessionState::Active
        );
        assert_eq!(
            config.state(Some(now - chrono::Duration::seconds(121)), now, true),
            AgentSessionState::Idle
        );
        assert!(config.file_only_session_allowed(now - chrono::Duration::minutes(29), now));
        assert!(!config.file_only_session_allowed(now - chrono::Duration::minutes(31), now));
    }

    #[test]
    fn agent_session_round_trips_json() {
        let session = AgentSession {
            id: "session-1".to_string(),
            provider: AgentSessionProvider::Codex,
            dialect: None,
            session_name: None,
            source: AgentSessionSource::DesktopApp,
            state: AgentSessionState::Active,
            pid: Some(1234),
            transcript_path: Some("C:\\sessions\\rollout.jsonl".to_string()),
            host: "devbox".to_string(),
            workspace: AgentSessionWorkspace {
                cwd: Some("C:\\work\\proj".to_string()),
                project_name: Some("proj".to_string()),
            },
            activity: AgentSessionActivity {
                started_at: Some(Utc.with_ymd_and_hms(2026, 7, 12, 0, 0, 0).unwrap()),
                last_activity_at: Some(Utc.with_ymd_and_hms(2026, 7, 12, 0, 1, 0).unwrap()),
            },
            focus_target: AgentSessionFocusTarget::Process { pid: 1234 },
        };

        let json = serde_json::to_string(&session).unwrap();
        let round_tripped: AgentSession = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped, session);
        assert!(json.contains("\"focusTarget\""));
    }

    #[test]
    fn focus_result_serializes_safely() {
        let focused = serde_json::to_value(&SessionFocusResult::Focused).unwrap();
        let unsupported = serde_json::to_value(&SessionFocusResult::Unsupported {
            message: "focus unavailable".to_string(),
        })
        .unwrap();
        let failed = serde_json::to_value(&SessionFocusResult::Failed {
            message: "failed to focus".to_string(),
        })
        .unwrap();

        assert!(focused.is_string() || focused.is_object());
        assert_eq!(unsupported["message"], "focus unavailable");
        assert_eq!(failed["message"], "failed to focus");
    }

    #[test]
    fn host_validation_dedupes_and_rejects_unsafe_values() {
        let hosts = RemoteSessionFetcher::sanitized_hosts(&[
            "".to_string(),
            " ".to_string(),
            "-bad".to_string(),
            "good".to_string(),
            "good".to_string(),
            "GOOD".to_string(),
            "bad host".to_string(),
            "bad\tcontrol".to_string(),
        ]);
        assert_eq!(hosts, vec!["good".to_string()]);
    }

    #[test]
    fn tailscale_parser_returns_online_peer_dns_names() {
        let json = r#"{
            "Self": {"DNSName": "this-pc.tailnet.ts.net."},
            "Peer": {
                "one": {"DNSName": "devbox.tailnet.ts.net.", "Online": true},
                "two": {"DNSName": "offline.tailnet.ts.net.", "Online": false},
                "three": {"DNSName": "", "Online": true}
            }
        }"#;

        assert_eq!(
            TailscaleStatusParser::hosts(json).unwrap(),
            vec!["devbox.tailnet.ts.net"]
        );
    }

    #[test]
    fn tailscale_parser_rejects_malformed_json() {
        assert!(TailscaleStatusParser::hosts("{").is_err());
    }

    #[test]
    fn automatic_and_manual_hosts_are_validated_and_deduplicated() {
        assert_eq!(
            RemoteSessionFetcher::merge_hosts(
                &["manual".into(), "DEVBOX.tailnet.ts.net".into()],
                &["devbox.tailnet.ts.net".into(), "-unsafe".into()],
            ),
            vec!["manual", "DEVBOX.tailnet.ts.net"]
        );
    }

    #[test]
    fn codex_rollout_parser_reads_first_line_metadata() {
        let metadata = CodexRolloutFirstLineParser::parse(
            r#"{"type":"session_meta","payload":{"session_id":"abc","cwd":"C:\\work\\proj","originator":"codex_exec","source":"cli"}}"#,
        )
        .unwrap();
        assert_eq!(metadata.session_id, "abc");
        assert_eq!(metadata.cwd.as_deref(), Some("C:\\work\\proj"));
    }

    #[test]
    fn claude_cwd_escape_is_stable() {
        assert_eq!(
            ClaudeSessionProjectMapper::escaped_cwd(r"C:\Users\me\My Project!"),
            "C--Users-me-My-Project-"
        );
    }

    #[test]
    fn remote_session_result_round_trips() {
        let result = AgentSessionHostResult {
            host: "devbox".to_string(),
            sessions: vec![AgentSession {
                id: "session-1".to_string(),
                provider: AgentSessionProvider::Claude,
                dialect: None,
                session_name: None,
                source: AgentSessionSource::Cli,
                state: AgentSessionState::Idle,
                pid: None,
                transcript_path: None,
                host: "devbox".to_string(),
                workspace: AgentSessionWorkspace {
                    cwd: None,
                    project_name: None,
                },
                activity: AgentSessionActivity {
                    started_at: None,
                    last_activity_at: None,
                },
                focus_target: AgentSessionFocusTarget::None,
            }],
            error: Some("ssh not found".to_string()),
        };

        let json = serde_json::to_string(&result).unwrap();
        let decoded: AgentSessionHostResult = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, result);
    }

    #[test]
    fn windows_process_parser_uses_metadata_and_ignores_command_lines() {
        let output = r#"[
            {
                "ProcessId": 101,
                "ParentProcessId": 1,
                "CreationDate": "/Date(1783773458193)/",
                "Name": "claude.exe",
                "ExecutablePath": "C:\\Tools\\claude.exe",
                "CommandLine": "claude.exe --api-key super-secret"
            },
            {
                "ProcessId": 202,
                "ParentProcessId": 1,
                "CreationDate": "2026-07-12T00:02:03Z",
                "Name": "codex.exe",
                "ExecutablePath": "C:\\Tools\\codex.exe"
            }
        ]"#;

        let records = WindowsProcessOutputParser::parse(output);

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].provider, Some(AgentSessionProvider::Claude));
        assert_eq!(records[1].provider, Some(AgentSessionProvider::Codex));
        assert!(records[0].started_at.is_some());
        assert_eq!(records[0].executable, "claude.exe");
        assert!(!records[0].executable.contains("super-secret"));
    }

    #[test]
    fn windows_process_query_never_requests_raw_command_lines() {
        let options = LocalAgentSessionScanner::process_options(Duration::from_secs(2));
        let script = options.extra_args.last().unwrap();

        assert!(script.contains("ProcessId"));
        assert!(script.contains("ExecutablePath"));
        assert!(!script.contains("CommandLine"));
    }

    #[test]
    fn codex_discovery_only_builds_today_and_yesterday_directories() {
        let now = Utc.with_ymd_and_hms(2026, 7, 12, 1, 2, 3).unwrap();
        let root = Path::new(r"C:\Users\me\.codex\sessions");

        let directories = LocalAgentSessionScanner::codex_day_directories(root, now.date_naive());

        assert_eq!(
            directories,
            vec![
                root.join("2026").join("07").join("12"),
                root.join("2026").join("07").join("11"),
            ]
        );
    }

    #[test]
    fn claude_metadata_parser_stops_after_session_and_cwd_are_known() {
        struct FirstLineOnly {
            bytes: &'static [u8],
            offset: usize,
        }

        impl Read for FirstLineOnly {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                assert!(
                    self.offset < self.bytes.len(),
                    "parser read past complete metadata"
                );
                buffer[0] = self.bytes[self.offset];
                self.offset += 1;
                Ok(1)
            }
        }

        let input = FirstLineOnly {
            bytes:
                b"{\"type\":\"user\",\"sessionId\":\"session-1\",\"cwd\":\"C:\\\\work\\\\proj\"}\n",
            offset: 0,
        };

        let metadata = ClaudeTranscriptMetadataParser::parse(input).unwrap();

        assert_eq!(metadata.session_id.as_deref(), Some("session-1"));
        assert_eq!(metadata.cwd.as_deref(), Some(r"C:\work\proj"));
    }

    #[test]
    fn focus_rejects_remote_and_file_only_targets_explicitly() {
        let remote = AgentSession {
            id: "remote".to_string(),
            provider: AgentSessionProvider::Codex,
            dialect: None,
            session_name: None,
            source: AgentSessionSource::Cli,
            state: AgentSessionState::Active,
            pid: Some(42),
            transcript_path: None,
            host: "other-host".to_string(),
            workspace: AgentSessionWorkspace {
                cwd: None,
                project_name: None,
            },
            activity: AgentSessionActivity {
                started_at: None,
                last_activity_at: None,
            },
            focus_target: AgentSessionFocusTarget::Process { pid: 42 },
        };
        let file_only = AgentSession {
            host: "localhost".to_string(),
            focus_target: AgentSessionFocusTarget::Transcript {
                transcript_path: "session.jsonl".to_string(),
            },
            ..remote.clone()
        };

        assert!(matches!(
            focus_session(&remote),
            SessionFocusResult::Unsupported { .. }
        ));
        assert!(matches!(
            focus_session(&file_only),
            SessionFocusResult::Unsupported { .. }
        ));
    }

    #[test]
    fn ssh_fetch_uses_noninteractive_options_and_a_strict_timeout() {
        let options =
            RemoteSessionFetcher::ssh_options("user@devbox", Duration::from_secs(5)).unwrap();

        assert_eq!(options.timeout, Duration::from_secs(5));
        assert!(
            options
                .extra_args
                .windows(2)
                .any(|pair| pair == ["-o", "BatchMode=yes"])
        );
        assert!(
            options
                .extra_args
                .windows(2)
                .any(|pair| pair == ["-o", "ConnectTimeout=3"])
        );
        assert!(options.extra_args.iter().any(|arg| arg == "user@devbox"));
    }

    #[test]
    fn ssh_remote_command_prefers_json_v2_with_legacy_fallback() {
        let options =
            RemoteSessionFetcher::ssh_options("user@devbox", Duration::from_secs(5)).unwrap();
        let shell = options.extra_args.last().expect("shell string").as_str();

        let v2 = shell.find("codexbar sessions --json-v2").expect("v2 command");
        let v1 = shell.find("codexbar sessions --json ||").expect("v1 fallback");
        let bundled_v2 = shell.rfind("sessions --json-v2").expect("bundled v2 fallback");
        assert!(v2 < v1, "v2 negotiated before legacy PATH fallback");
        assert!(v1 < bundled_v2, "PATH fallback before bundled CLI");
        assert!(shell.ends_with("sessions --json"), "bundled legacy fallback present");
    }

    #[tokio::test]
    async fn ssh_hosts_are_fetched_in_parallel_and_failures_are_isolated() {
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let results = tokio::time::timeout(
            Duration::from_secs(1),
            RemoteSessionFetcher::fetch_hosts_with(
                &[
                    "slow-a".to_string(),
                    "slow-b".to_string(),
                    "broken".to_string(),
                ],
                |host| {
                    let barrier = Arc::clone(&barrier);
                    async move {
                        if host == "broken" {
                            return AgentSessionHostResult::failed(host, "unreachable");
                        }
                        barrier.wait().await;
                        AgentSessionHostResult::success(host, Vec::new())
                    }
                },
            ),
        )
        .await
        .expect("hosts were fetched sequentially");

        assert_eq!(results.len(), 3);
        assert_eq!(
            results
                .iter()
                .filter(|result| result.error.is_some())
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| result.error.is_none())
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn disabled_discovery_returns_without_running_scanners() {
        let result = AgentSessionDiscovery::default()
            .scan(AgentSessionDiscoveryMode::Disabled)
            .await;

        assert_eq!(result, AgentSessionDiscoveryResult::Disabled);
    }
}
