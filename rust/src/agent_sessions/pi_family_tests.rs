#[cfg(test)]
mod pi_family_tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    // -------------------------------------------------------------------
    // Fixture + scaffolding helpers
    // -------------------------------------------------------------------

    fn fixture_file(relative: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/agent_sessions/fixtures/pi_family")
            .join(relative)
    }

    fn record(
        url: &Path,
        dialect: PiSessionDialect,
        modified_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> PiFamilySessionRecord {
        parse_session_file(url, dialect, modified_at, now).expect("record parses")
    }

    fn utc_ts(secs: i64) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(secs, 0).unwrap()
    }

    fn agent_process(pid: u32, started_at: Option<DateTime<Utc>>, command: &str) -> AgentProcessRecord {
        AgentProcessRecord {
            pid,
            ppid: 1,
            started_at,
            provider: Some(AgentSessionProvider::Pi),
            source: crate::agent_sessions::AgentSessionSource::Cli,
            executable: command.split_whitespace().next().unwrap_or(command).to_string(),
            kind: crate::agent_sessions::AgentProcessKind::Agent,
            command: Some(command.to_string()),
        }
    }

    fn env_map(pairs: &[(&str, &str)]) -> EnvMap {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    fn budget() -> DirectoryScanBudget {
        DirectoryScanBudget::new(512, 1, std::time::Duration::from_secs(5))
    }

    fn scan_helper(
        processes: &[AgentProcessRecord],
        cwd_by_pid: CwdByPid,
        environment: EnvMap,
        now: DateTime<Utc>,
    ) -> Vec<AgentSession> {
        let input = PiFamilyScanInput {
            processes,
            cwd_by_pid,
            environment,
            now,
            host: "fixture-host".to_string(),
            config: SessionScanConfig::default(),
        };
        let mut budget = budget();
        PiFamilySessionScanner::scan(&input, &mut budget)
    }

    fn copy_fixture_tree(relative: &str, destination: &Path) {
        let source = fixture_file(relative);
        // Copy every bucket directory (e.g. `--tmp-pi-family-project--/`) and
        // its jsonl files under the destination root.
        for bucket in fs::read_dir(&source).unwrap().flatten() {
            if !bucket.path().is_dir() {
                continue;
            }
            let dest_bucket = destination.join(bucket.file_name());
            fs::create_dir_all(&dest_bucket).unwrap();
            for entry in fs::read_dir(bucket.path()).unwrap().flatten() {
                if entry.path().is_file() {
                    fs::copy(entry.path(), dest_bucket.join(entry.file_name())).unwrap();
                }
            }
        }
    }

    /// Set every *.jsonl mtime under `root` recursively (metadata writes only).
    fn touch_jsonl(root: &Path, time: std::time::SystemTime) {
        for entry in fs::read_dir(root).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                touch_jsonl(&path, time);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                fs::File::options()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_modified(time)
                    .unwrap();
            }
        }
    }

    /// Write a jsonl session file with `id`/`cwd` recorded at `modified_at`.
    fn write_jsonl_session(
        path: &Path,
        dialect: PiSessionDialect,
        id: &str,
        cwd: &Path,
        modified_at: std::time::SystemTime,
    ) {
        let parent = path.parent().unwrap();
        fs::create_dir_all(parent).unwrap();
        let body = match dialect {
            PiSessionDialect::Pi => format!(
                "{{\"type\":\"session\",\"version\":3,\"id\":\"{id}\",\"timestamp\":\"2026-08-03T12:00:00.000Z\",\"cwd\":{}}}\n",
                serde_json::to_string(&cwd.to_string_lossy()).unwrap()
            ),
            PiSessionDialect::Omp => format!(
                "{{\"type\":\"session\",\"id\":\"{id}\",\"timestamp\":\"2026-08-03T11:00:00.000Z\",\"cwd\":{}}}\n",
                serde_json::to_string(&cwd.to_string_lossy()).unwrap()
            ),
        };
        fs::write(path, body).unwrap();
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(modified_at)
            .unwrap();
    }

    fn json_line(value: &serde_json::Value) -> String {
        format!("{}\n", serde_json::to_string(value).unwrap())
    }

    // -------------------------------------------------------------------
    // Parser coverage (upstream: fixture parsers cover plain pi, omp title,
    // and legacy header dialects)
    // -------------------------------------------------------------------

    #[test]
    fn fixture_parsers_cover_plain_pi_omp_title_and_legacy_header_dialects() {
        let now = utc_ts(1_900_000_000);
        let pi_url = fixture_file("pi/--tmp-pi-family-project--/2026-08-03T12-00-00-000Z_pi-fixture.jsonl");
        let omp_url = fixture_file("omp/abs-pi-family-project-0bff77ccc1794123b5c69216e8a176e470093f8ebe392db0e42a2df5b9f5d17a/2026-08-03T12-00-00-000Z_omp-fixture.jsonl");
        let legacy_url = fixture_file("omp-legacy/--tmp-pi-family-project--/2026-08-03T11-00-00-000Z_omp-legacy.jsonl");
        assert!(pi_url.exists(), "{} missing", pi_url.display());
        assert!(omp_url.exists(), "{} missing", omp_url.display());
        assert!(legacy_url.exists(), "{} missing", legacy_url.display());

        let pi = record(&pi_url, PiSessionDialect::Pi, now, now);
        assert_eq!(pi.id, "pi-fixture");
        assert_eq!(pi.cwd.as_deref(), Some("/tmp/pi-family-project"));
        assert_eq!(pi.session_name.as_deref(), Some("Plain pi fixture"));

        let omp = record(&omp_url, PiSessionDialect::Omp, now, now);
        assert_eq!(omp.id, "omp-fixture");
        assert_eq!(omp.session_name.as_deref(), Some("OMP fixture"));

        let legacy = record(&legacy_url, PiSessionDialect::Omp, now, now);
        assert_eq!(legacy.id, "omp-legacy");
        assert_eq!(legacy.session_name.as_deref(), Some("OMP legacy fixture"));
    }

    #[test]
    fn plain_pi_session_info_reads_from_the_tail_and_bounds_labels_to_64_scalars() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("session.jsonl");
        let title = format!("{}\nignored", "🙂".repeat(70));
        let mut content = json_line(&serde_json::json!({
            "type": "session",
            "version": 3,
            "id": "tail-title",
            "timestamp": "2026-08-03T12:00:00.000Z",
            "cwd": "/tmp/project"
        }));
        for _ in 0..2000 {
            content.push_str("{\"type\":\"custom\",\"data\":\"padding-padding-padding\"}\n");
        }
        content.push_str(&json_line(&serde_json::json!({
            "type": "session_info",
            "name": title
        })));
        fs::write(&file, content).unwrap();

        let now = utc_ts(1_900_000_000);
        let record = record(&file, PiSessionDialect::Pi, now + chrono::Duration::seconds(30), now);
        assert_eq!(record.session_name.as_deref().map(|n| n.chars().count()), Some(64));
        assert_eq!(record.session_name.as_deref(), Some(&"🙂".repeat(64)[..]));
        // modified_at is clamped to `now` (never the future).
        assert_eq!(record.modified_at, now);
    }

    // -------------------------------------------------------------------
    // Process classification (upstream: recognizes both pi dialects,
    // excludes helpers)
    // -------------------------------------------------------------------

    #[test]
    fn process_classification_recognizes_both_pi_dialects_and_excludes_helpers() {
        let records = [
            agent_process(1, None, "pi"),
            agent_process(2, None, "/usr/local/bin/pi --model test"),
            agent_process(3, None, "omp --profile work"),
            agent_process(4, None, "bun /tools/oh-my-pi/omp"),
            agent_process(5, None, "pi --help"),
            agent_process(6, None, "omp --version"),
            agent_process(7, None, "bun /tools/unrelated.js"),
        ];

        assert_eq!(pi_family_dialect(records[0].command.as_deref().unwrap()), Some(PiSessionDialect::Pi));
        assert_eq!(pi_family_dialect(records[1].command.as_deref().unwrap()), Some(PiSessionDialect::Pi));
        assert_eq!(pi_family_dialect(records[2].command.as_deref().unwrap()), Some(PiSessionDialect::Omp));
        assert_eq!(pi_family_dialect(records[3].command.as_deref().unwrap()), Some(PiSessionDialect::Omp));
        assert!(is_pi_family_helper(records[4].command.as_deref().unwrap()));
        assert!(is_pi_family_helper(records[5].command.as_deref().unwrap()));
        assert_eq!(pi_family_dialect(records[6].command.as_deref().unwrap()), None);
    }

    #[test]
    fn windows_shims_and_paths_are_normalized_for_dialect() {
        assert_eq!(
            pi_family_dialect(r"C:\Tools\pi.exe --model test"),
            Some(PiSessionDialect::Pi)
        );
        assert_eq!(
            pi_family_dialect(r"omp.cmd"),
            Some(PiSessionDialect::Omp)
        );
        assert_eq!(
            pi_family_dialect(r"C:\Users\me\AppData\Roaming\npm\omp.cmd run"),
            Some(PiSessionDialect::Omp)
        );
        assert_eq!(
            pi_family_dialect(r"bun C:\src\oh-my-pi\omp.cmd"),
            Some(PiSessionDialect::Omp)
        );
        assert!(is_pi_family_helper("omp --smoke-test"));
        assert!(is_pi_family_helper("bun omp __omp_worker_boot"));
    }

    // -------------------------------------------------------------------
    // Scanner end-to-end (upstream: correlates fixture dirs for both
    // dialects; legacy omp buckets + xdg roots — xdg gated unix)
    // -------------------------------------------------------------------

    #[test]
    fn scanner_correlates_fixture_directories_for_both_dialects() {
        let root_dir = tempfile::tempdir().expect("tempdir");
        let home = root_dir.path().join("home");
        let pi_root = home.join(".pi").join("agent").join("sessions");
        let omp_root = home.join(".omp").join("agent").join("sessions");
        copy_fixture_tree("pi", &pi_root);
        copy_fixture_tree("omp", &omp_root);
        fs::create_dir_all(&home).ok();
        let now = utc_ts(1_900_000_000);
        touch_jsonl(&home, (now - chrono::Duration::seconds(5)).into());

        let processes = vec![
            agent_process(11, Some(now - chrono::Duration::seconds(60)), "pi"),
            agent_process(12, Some(now - chrono::Duration::seconds(60)), "omp"),
        ];
        let cwd_by_pid: CwdByPid = [
            (11, "/tmp/pi-family-project".to_string()),
            (12, "/tmp/pi-family-project".to_string()),
        ]
        .into_iter()
        .collect();
        let environment = env_map(&[("HOME", home.to_string_lossy().as_ref())]);
        let sessions = scan_helper(&processes, cwd_by_pid, environment, now);

        assert_eq!(sessions.len(), 2);
        let pi = sessions.iter().find(|s| s.dialect == Some(PiSessionDialect::Pi)).expect("pi session");
        let omp = sessions.iter().find(|s| s.dialect == Some(PiSessionDialect::Omp)).expect("omp session");
        assert_eq!(pi.id, "pi-fixture");
        assert_eq!(pi.session_name.as_deref(), Some("Plain pi fixture"));
        assert_eq!(omp.id, "omp-fixture");
        assert_eq!(omp.session_name.as_deref(), Some("OMP fixture"));
        assert!(sessions.iter().all(|s| s.provider == AgentSessionProvider::Pi));
        assert!(sessions.iter().all(|s| s.transcript_path.is_some()));
    }

    #[cfg(unix)]
    #[test]
    fn scanner_uses_legacy_omp_buckets_and_xdg_roots() {
        let root_dir = tempfile::tempdir().expect("tempdir");
        let home = root_dir.path().join("home");
        let xdg = root_dir.path().join("xdg");
        let sessions_root = xdg.join("omp").join("sessions");
        copy_fixture_tree("omp-legacy", &sessions_root);
        let now = utc_ts(1_900_000_000);
        touch_jsonl(&xdg, (now - chrono::Duration::seconds(5)).into());

        let processes = vec![agent_process(20, Some(now - chrono::Duration::seconds(60)), "omp")];
        let cwd_by_pid: CwdByPid = [(20, "/tmp/pi-family-project".to_string())].into_iter().collect();
        let environment = env_map(&[
            ("HOME", home.to_string_lossy().as_ref()),
            ("XDG_DATA_HOME", xdg.to_string_lossy().as_ref()),
        ]);
        let sessions = scan_helper(&processes, cwd_by_pid, environment, now);

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "omp-legacy");
        assert_eq!(sessions[0].dialect, Some(PiSessionDialect::Omp));
        assert_eq!(sessions[0].session_name.as_deref(), Some("OMP legacy fixture"));
    }

    // -------------------------------------------------------------------
    // Custom roots (upstream: cli --session-dir and pi settings.json)
    // -------------------------------------------------------------------

    #[test]
    fn custom_session_directories_resolve_from_cli_and_plain_pi_settings() {
        let root_dir = tempfile::tempdir().expect("tempdir");
        let home = root_dir.path().join("home");
        let cwd = root_dir.path().join("project");
        fs::create_dir_all(cwd.join(".pi")).expect("mk pi dir");
        let cli_sessions = root_dir.path().join("cli-sessions");
        let settings_sessions = root_dir.path().join("settings-sessions");
        fs::create_dir_all(&cli_sessions).unwrap();
        fs::create_dir_all(&settings_sessions).unwrap();
        fs::write(
            cwd.join(".pi").join("settings.json"),
            format!("{{\"sessionDir\":{}}}\n", serde_json::to_string(&settings_sessions.to_string_lossy()).unwrap()),
        )
        .unwrap();

        let now = utc_ts(1_900_000_000);
        write_jsonl_session(
            &cli_sessions.join("omp.jsonl"),
            PiSessionDialect::Omp,
            "omp-custom",
            &cwd,
            (now - chrono::Duration::seconds(5)).into(),
        );
        write_jsonl_session(
            &settings_sessions.join("pi.jsonl"),
            PiSessionDialect::Pi,
            "pi-settings",
            &cwd,
            (now - chrono::Duration::seconds(5)).into(),
        );

        let processes = vec![
            agent_process(
                31,
                Some(now - chrono::Duration::seconds(60)),
                &format!("omp --session-dir {}", cli_sessions.display()),
            ),
            agent_process(32, Some(now - chrono::Duration::seconds(60)), "pi"),
        ];
        let cwd_by_pid: CwdByPid = [
            (31, cwd.to_string_lossy().into_owned()),
            (32, cwd.to_string_lossy().into_owned()),
        ]
        .into_iter()
        .collect();
        let environment = env_map(&[("HOME", home.to_string_lossy().as_ref())]);
        let sessions = scan_helper(&processes, cwd_by_pid, environment, now);

        let ids: HashSet<String> = sessions.iter().map(|s| s.id.clone()).collect();
        assert_eq!(ids, HashSet::from(["omp-custom".to_string(), "pi-settings".to_string()]));
        assert_eq!(
            sessions.iter().find(|s| s.id == "omp-custom").unwrap().dialect,
            Some(PiSessionDialect::Omp)
        );
        assert_eq!(
            sessions.iter().find(|s| s.id == "pi-settings").unwrap().dialect,
            Some(PiSessionDialect::Pi)
        );
    }

    // -------------------------------------------------------------------
    // PID-only fallback (upstream: missing jsonl and unresolved custom
    // roots retain pid only rows)
    // -------------------------------------------------------------------

    #[test]
    fn missing_jsonl_and_unresolved_custom_roots_retain_pid_only_rows() {
        let root_dir = tempfile::tempdir().expect("tempdir");
        let now = utc_ts(1_900_000_000);
        let processes = vec![
            agent_process(41, Some(now - chrono::Duration::seconds(10)), "pi"),
            agent_process(42, Some(now - chrono::Duration::seconds(10)), "omp --profile missing"),
        ];
        let cwd_by_pid: CwdByPid = [
            (41, "/tmp/no-jsonl-pi".to_string()),
            (42, "/tmp/no-jsonl-omp".to_string()),
        ]
        .into_iter()
        .collect();
        let environment = env_map(&[("HOME", root_dir.path().to_string_lossy().as_ref())]);
        let sessions = scan_helper(&processes, cwd_by_pid, environment, now);

        let ids: HashSet<String> = sessions.iter().map(|s| s.id.clone()).collect();
        assert_eq!(ids, HashSet::from(["pid:41".to_string(), "pid:42".to_string()]));
        assert!(sessions.iter().all(|s| s.transcript_path.is_none()));
        assert!(sessions.iter().all(|s| s.state == AgentSessionState::Active));
        let dialects: HashSet<_> = sessions.iter().filter_map(|s| s.dialect).collect();
        assert_eq!(dialects, HashSet::from([PiSessionDialect::Pi, PiSessionDialect::Omp]));
    }

    // -------------------------------------------------------------------
    // Correlation uniqueness (upstream: assigns each transcript once and
    // leaves unmatched processes visible)
    // -------------------------------------------------------------------

    #[test]
    fn correlation_assigns_each_transcript_once_and_leaves_unmatched_processes_visible() {
        let root_dir = tempfile::tempdir().expect("tempdir");
        let home = root_dir.path().join("home");
        let now = utc_ts(1_900_000_000);
        write_jsonl_session(
            &home
                .join(".pi")
                .join("agent")
                .join("sessions")
                .join("--tmp-correlation--")
                .join("one.jsonl"),
            PiSessionDialect::Pi,
            "one-session",
            Path::new("/tmp/correlation"),
            (now - chrono::Duration::seconds(5)).into(),
        );

        let processes = vec![
            agent_process(51, Some(now - chrono::Duration::seconds(30)), "pi"),
            agent_process(52, Some(now - chrono::Duration::seconds(20)), "pi"),
        ];
        let cwd_by_pid: CwdByPid = [
            (51, "/tmp/correlation".to_string()),
            (52, "/tmp/correlation".to_string()),
        ]
        .into_iter()
        .collect();
        let environment = env_map(&[("HOME", home.to_string_lossy().as_ref())]);
        let sessions = scan_helper(&processes, cwd_by_pid, environment, now);

        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions.iter().filter(|s| s.id == "one-session").count(), 1);
        assert_eq!(sessions.iter().filter(|s| s.id.starts_with("pid:")).count(), 1);
        let transcripts: HashSet<_> = sessions.iter().filter_map(|s| s.transcript_path.as_ref()).collect();
        assert_eq!(transcripts.len(), 1);
    }

    // -------------------------------------------------------------------
    // Malformed / empty parsing boundaries
    // -------------------------------------------------------------------

    #[test]
    fn malformed_files_yield_no_record() {
        let now = utc_ts(1_900_000_000);
        let cases: Vec<(&str, &[u8], PiSessionDialect)> = vec![
            ("empty", b"", PiSessionDialect::Pi),
            ("whitespace-only", b"\n\n  \n", PiSessionDialect::Omp),
            ("not-json", b"hello world\n", PiSessionDialect::Pi),
            ("json-array", b"[1,2]\n", PiSessionDialect::Omp),
            ("wrong-first-type", b"{\"type\":\"message\",\"id\":\"x\"}\n", PiSessionDialect::Pi),
            ("missing-id", b"{\"type\":\"session\",\"timestamp\":\"2026-08-03T12:00:00.000Z\"}\n", PiSessionDialect::Omp),
            ("pi-wrong-version", b"{\"type\":\"session\",\"version\":2,\"id\":\"v2\"}\n", PiSessionDialect::Pi),
            ("pi-missing-version", b"{\"type\":\"session\",\"id\":\"nov\"}\n", PiSessionDialect::Pi),
            ("omp-no-version-ok", b"{\"type\":\"session\",\"id\":\"legacy\"}\n", PiSessionDialect::Omp),
            ("truncated-header", b"{\"type\":\"session\",\"id\":\"tru", PiSessionDialect::Omp),
        ];
        for (name, body, dialect) in cases {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join(format!("{name}.jsonl"));
            fs::write(&path, body).unwrap();
            let result = parse_session_file(&path, dialect, now, now);
            match name {
                "omp-no-version-ok" => assert_eq!(result.map(|r| r.id).as_deref(), Some("legacy")),
                // The truncated header has no trailing newline; below the
                // prefix cap it is kept as a partial line and fails JSON.
                _ => assert!(result.is_none(), "{name} must not parse"),
            }
        }
    }

    #[test]
    fn prefix_cap_drops_a_partial_header_line() {
        // A 16 KiB+ header with no newline is truncated by the cap and must
        // not be half-parsed into a record.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("huge.jsonl");
        let mut body = b"{\"type\":\"session\",\"id\":\"huge\"".to_vec();
        body.extend(std::iter::repeat_n(b' ', MAX_PREFIX_READ + 4096));
        fs::write(&path, body).unwrap();
        let now = utc_ts(1_900_000_000);
        assert!(parse_session_file(&path, PiSessionDialect::Omp, now, now).is_none());
    }

    #[test]
    fn title_sanitization_strips_controls_and_bounds_scalars() {
        assert_eq!(sanitized_title("hello"), Some("hello".to_string()));
        assert_eq!(sanitized_title(""), None);
        assert_eq!(sanitized_title("\u{0}\u{7}\n"), None);
        assert_eq!(sanitized_title("a\nb"), Some("ab".to_string()));
        let huge = "x".repeat(500);
        assert_eq!(
            sanitized_title(&huge).map(|t| t.chars().count()),
            Some(MAX_TITLE_SCALARS)
        );
    }

    #[test]
    fn timestamps_parse_with_and_without_fractional_seconds() {
        assert!(parse_iso_date("2026-08-03T12:00:00.000Z").is_some());
        assert!(parse_iso_date("2026-08-03T12:00:00Z").is_some());
        assert!(parse_iso_date("2026-08-03T12:00:00+07:00").is_some());
        assert!(parse_iso_date("not-a-date").is_none());
        assert!(parse_iso_date("").is_none());
    }

    // -------------------------------------------------------------------
    // Root resolution + profile policy
    // -------------------------------------------------------------------

    #[test]
    fn profile_names_validate_and_fail_closed() {
        assert_eq!(normalize_profile(None), PiProfile::Default);
        assert_eq!(normalize_profile(Some("")), PiProfile::Default);
        assert_eq!(normalize_profile(Some("default")), PiProfile::Default);
        assert_eq!(normalize_profile(Some(" work ")), PiProfile::Named("work".to_string()));
        assert_eq!(normalize_profile(Some("a.b-c_d")), PiProfile::Named("a.b-c_d".to_string()));
        assert_eq!(normalize_profile(Some("1abc")), PiProfile::Named("1abc".to_string()));
        assert_eq!(normalize_profile(Some(".")), PiProfile::Invalid);
        assert_eq!(normalize_profile(Some("..")), PiProfile::Invalid);
        assert_eq!(normalize_profile(Some("bad.")), PiProfile::Invalid);
        assert_eq!(normalize_profile(Some("UPPER")), PiProfile::Invalid);
        assert_eq!(normalize_profile(Some("con")), PiProfile::Invalid);
        assert_eq!(normalize_profile(Some("COM1")), PiProfile::Invalid);
        assert_eq!(normalize_profile(Some("LPT9")), PiProfile::Invalid);
        assert_eq!(normalize_profile(Some(&"x".repeat(65))), PiProfile::Invalid);
        assert_eq!(normalize_profile(Some("bad name")), PiProfile::Invalid);
    }

    #[test]
    fn invalid_profile_selects_no_roots_and_valid_profiles_resolve() {
        let home = tempfile::tempdir().expect("tempdir");
        let env = env_map(&[
            ("HOME", home.path().to_string_lossy().as_ref()),
            ("OMP_PROFILE", ".."),
        ]);
        let cwd = home.path().to_path_buf();
        assert!(omp_named_or_default_root_probe(&env, &cwd, home.path()).is_none());

        let env = env_map(&[
            ("HOME", home.path().to_string_lossy().as_ref()),
            ("OMP_PROFILE", "work"),
        ]);
        let root = omp_named_profile_root("work", &env, &cwd, home.path())
            .expect("named profile root");
        assert!(root.ends_with(Path::new("profiles").join("work").join("agent").join("sessions")));
    }

    fn omp_named_or_default_root_probe(environment: &EnvMap, cwd: &Path, home: &Path) -> Option<PathBuf> {
        match omp_profile_selector(environment) {
            PiProfile::Invalid => None,
            PiProfile::Named(profile) => omp_named_profile_root(&profile, environment, cwd, home),
            PiProfile::Default => omp_default_profile_root(environment, cwd, home),
        }
    }

    #[test]
    fn config_dir_must_stay_within_home() {
        let home = tempfile::tempdir().expect("tempdir");
        let cwd = home.path().to_path_buf();
        for bad in ["/abs/path", "..", "../escape", "~/x", "C:\\escape"] {
            let env = env_map(&[("PI_CONFIG_DIR", bad)]);
            assert!(
                omp_default_profile_root(&env, &cwd, home.path()).is_none(),
                "{bad} must fail closed"
            );
        }
        let env = env_map(&[("PI_CONFIG_DIR", "custom-omp")]);
        let root = omp_default_profile_root(&env, &cwd, home.path()).expect("custom root");
        let canonical_home = canonicalize_for_scan(home.path());
        assert!(path_is_within(&canonical_home, &root));
        assert!(root.ends_with(Path::new("custom-omp").join("agent").join("sessions")));
    }

    #[test]
    fn custom_agent_root_and_session_dir_env_win() {
        let home = tempfile::tempdir().expect("tempdir");
        let cwd = home.path().to_path_buf();
        let agent_dir = root_dir_agent_dir(home.path());
        let env = env_map(&[("PI_CODING_AGENT_DIR", agent_dir.to_string_lossy().as_ref())]);
        let roots = session_roots_for_process(
            &agent_process(1, None, "pi"),
            PiSessionDialect::Pi,
            &cwd,
            &env,
            Some(home.path()),
        );
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].path, canonicalize_for_scan(&agent_dir.join("sessions")));
    }

    fn root_dir_agent_dir(home: &Path) -> PathBuf {
        home.join("modes").join("custom-agent")
    }

    #[test]
    fn session_dir_flag_beats_env_and_paths_are_resolved() {
        let home = tempfile::tempdir().expect("tempdir");
        let cwd = home.path().to_path_buf();
        let sessions = home.path().join("flag-sessions");
        let process = agent_process(1, None, &format!("omp --session-dir {}", sessions.display()));
        let env = env_map(&[("PI_CODING_AGENT_SESSION_DIR", home.path().join("env-sessions").to_string_lossy().as_ref())]);
        let roots = session_roots_for_process(&process, PiSessionDialect::Omp, &cwd, &env, Some(home.path()));
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].layout, RootLayout::Direct);
        assert_eq!(roots[0].path, canonicalize_for_scan(&sessions));
    }

    #[test]
    fn pi_settings_project_file_wins_over_global() {
        let home = tempfile::tempdir().expect("tempdir");
        let cwd = home.path().join("proj");
        fs::create_dir_all(cwd.join(".pi")).unwrap();
        fs::create_dir_all(home.path().join(".pi").join("agent")).unwrap();
        let project_dir = home.path().join("project-sessions");
        let global_dir = home.path().join("global-sessions");
        fs::write(
            cwd.join(".pi").join("settings.json"),
            format!("{{\"sessionDir\":{}}}", serde_json::to_string(&project_dir.to_string_lossy()).unwrap()),
        )
        .unwrap();
        fs::write(
            home.path().join(".pi").join("agent").join("settings.json"),
            format!("{{\"sessionDir\":{}}}", serde_json::to_string(&global_dir.to_string_lossy()).unwrap()),
        )
        .unwrap();
        let resolved = pi_settings_session_directory(&cwd, home.path()).expect("project wins");
        assert_eq!(resolved, canonicalize_for_scan(&project_dir));
    }

    #[test]
    fn malformed_settings_session_dir_is_ignored() {
        let home = tempfile::tempdir().expect("tempdir");
        let cwd = home.path().to_path_buf();
        fs::create_dir_all(cwd.join(".pi")).unwrap();
        fs::write(cwd.join(".pi").join("settings.json"), "{{broken json").unwrap();
        assert!(session_dir_in(&cwd.join(".pi").join("settings.json")).is_none());
        fs::write(cwd.join(".pi").join("settings.json"), "{\"sessionDir\":\"   \"}").unwrap();
        assert!(session_dir_in(&cwd.join(".pi").join("settings.json")).is_none());
    }

    // -------------------------------------------------------------------
    // Command-line flag parsing
    // -------------------------------------------------------------------

    #[test]
    fn command_line_value_parses_split_and_equals_forms() {
        assert_eq!(command_line_value("--session-dir", "pi --session-dir /tmp/x"), Some("/tmp/x"));
        assert_eq!(command_line_value("--session-dir", "pi --session-dir=/tmp/x --model y"), Some("/tmp/x"));
        assert_eq!(command_line_value("--session-dir", "pi --session-dir"), None);
        assert_eq!(command_line_value("--session-dir", "pi --session-dir --other"), None);
        assert_eq!(command_line_value("--profile", "omp --profile work"), Some("work"));
        assert_eq!(command_line_value("--profile", "omp run --profile=work"), Some("work"));
    }

    // -------------------------------------------------------------------
    // Record ordering within a root
    // -------------------------------------------------------------------

    #[test]
    fn records_sort_by_modified_then_id_and_dedup_both() {
        let home = tempfile::tempdir().expect("tempdir");
        let bucket = home.path().join("sessions").join("proj");
        fs::create_dir_all(&bucket).unwrap();
        let now = utc_ts(1_900_000_000);
        write_jsonl_session(&bucket.join("b.jsonl"), PiSessionDialect::Pi, "same-id", Path::new("/tmp/p"), (now - chrono::Duration::seconds(10)).into());
        write_jsonl_session(&bucket.join("a.jsonl"), PiSessionDialect::Pi, "same-id", Path::new("/tmp/p"), (now - chrono::Duration::seconds(5)).into());
        let mut budget = budget();
        let records = records_in_root(&bucket, now, PiSessionDialect::Pi, RootLayout::Direct, &mut budget);
        assert_eq!(records.len(), 1, "duplicate ids collapse");
        assert_eq!(records[0].id, "same-id");
        assert_eq!(records[0].modified_at, now - chrono::Duration::seconds(5));
    }
}
