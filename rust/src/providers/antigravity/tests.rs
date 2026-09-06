use super::*;

#[test]
fn cadence_labels_are_owned_by_antigravity_snapshot() {
    let mut secondary = RateWindow::new(20.0);
    secondary.window_minutes = Some(7 * 24 * 60);
    let usage = UsageSnapshot::new(RateWindow::new(10.0)).with_secondary(secondary);
    let usage = AntigravityProvider::with_cadence_labels(usage);
    assert_eq!(usage.secondary_label.as_deref(), Some("Weekly"));
}

#[test]
fn test_classify_model_families() {
    assert_eq!(classify_model("Claude 3.5 Sonnet"), ModelFamily::Claude);
    assert_eq!(classify_model("claude-4-opus"), ModelFamily::Claude);
    assert_eq!(
        classify_model("Claude Thinking"),
        ModelFamily::ClaudeThinking
    );
    assert_eq!(
        classify_model("claude-3.5-sonnet-thinking"),
        ModelFamily::ClaudeThinking
    );
    assert_eq!(classify_model("Gemini 2.5 Pro Low"), ModelFamily::GeminiPro);
    assert_eq!(classify_model("gemini-pro-low"), ModelFamily::GeminiPro);
    assert_eq!(classify_model("Pro Low Latency"), ModelFamily::GeminiPro);
    assert_eq!(classify_model("Gemini 2.5 Flash"), ModelFamily::GeminiFlash);
    assert_eq!(classify_model("gemini-flash"), ModelFamily::GeminiFlash);
    assert_eq!(classify_model("Flash Model"), ModelFamily::GeminiFlash);
    assert_eq!(classify_model("GPT-4o"), ModelFamily::Other);
    assert_eq!(classify_model("unknown-model"), ModelFamily::Other);
}

#[test]
fn retired_flash_ids_collapse_to_current_wire_id() {
    for id in [
        "gemini-3.6-flash",
        "gemini-3.6-flash-high",
        "gemini-3.5-flash-extra-low",
        "gemini-3-flash-agent",
    ] {
        assert_eq!(canonical_model_id(id), "gemini-3.7-flash");
    }
    assert_eq!(canonical_model_id("gemini-3.7-flash"), "gemini-3.7-flash");
}

#[test]
fn parses_current_language_server_process() {
    let output = r"4242	C:\Users\test\AppData\Local\Programs\Antigravity\resources\bin\language_server.exe --csrf_token 11111111-2222-3333-4444-555555555555 --extension_server_port 54123";

    let process = AntigravityProvider::parse_process_info(output).expect("process info");

    assert_eq!(process.pid, Some(4242));
    assert_eq!(process.extension_port, Some(54123));
    assert_eq!(process.csrf_token, "11111111-2222-3333-4444-555555555555");
    assert_eq!(process.source, ProcessSource::Ide);
}

#[test]
fn parses_language_server_without_extension_server_port() {
    let output = "34564\tC:\\Users\\test\\AppData\\Local\\Programs\\Antigravity\\resources\\bin\\language_server.exe --standalone --override_ide_name antigravity --subclient_type hub --override_ide_version 2.0.11 --https_server_port 0 --csrf_token 68dda2fb-6b26-40c0-aeef-b9a628615714 --app_data_dir antigravity";

    let process =
        AntigravityProvider::parse_process_info(output).expect("process info should be detected");

    assert_eq!(process.pid, Some(34564));
    assert_eq!(process.extension_port, Some(0));
    assert_eq!(process.csrf_token, "68dda2fb-6b26-40c0-aeef-b9a628615714");
    assert_eq!(process.source, ProcessSource::Ide);
}

#[test]
fn parses_language_server_without_any_port_arg() {
    let output = "34564\tC:\\Users\\test\\AppData\\Local\\Programs\\Antigravity\\resources\\bin\\language_server.exe --standalone --csrf_token aabbccdd-1122-3344-5566-778899001122 --app_data_dir antigravity";

    let process =
        AntigravityProvider::parse_process_info(output).expect("process info should be detected");

    assert_eq!(process.pid, Some(34564));
    assert_eq!(process.extension_port, None);
    assert_eq!(process.csrf_token, "aabbccdd-1122-3344-5566-778899001122");
    assert_eq!(process.source, ProcessSource::Ide);
}

#[test]
fn parses_equals_form_args() {
    let output = "34564\tC:\\Users\\test\\AppData\\Local\\Programs\\Antigravity\\resources\\bin\\language_server.exe --csrf_token=68dda2fb-6b26-40c0-aeef-b9a628615714 --https_server_port=61999";

    let process =
        AntigravityProvider::parse_process_info(output).expect("process info should be detected");

    assert_eq!(process.pid, Some(34564));
    assert_eq!(process.extension_port, Some(61999));
    assert_eq!(process.csrf_token, "68dda2fb-6b26-40c0-aeef-b9a628615714");
    assert_eq!(process.source, ProcessSource::Ide);
}

fn make_response(models: Vec<(&str, f64)>) -> UserStatusResponse {
    let json = serde_json::json!({
        "userStatus": {
            "cascadeModelConfigData": {
                "clientModelConfigs": models.iter().map(|(label, remaining)| {
                    serde_json::json!({
                        "label": label,
                        "quotaInfo": {
                            "remainingFraction": remaining
                        }
                    })
                }).collect::<Vec<_>>()
            }
        }
    });
    serde_json::from_value(json).unwrap()
}

#[test]
fn antigravity_extra_windows_preserve_usage_known() {
    let json = serde_json::json!({
        "userStatus": {
            "cascadeModelConfigData": {
                "clientModelConfigs": [
                    {
                        "label": "Gemini 2.5 Pro",
                        "quotaInfo": {"remainingFraction": 0.8}
                    },
                    {
                        "label": "Claude 4 Sonnet",
                        "quotaInfo": {"remainingFraction": null}
                    }
                ]
            }
        }
    });
    let resp: UserStatusResponse = serde_json::from_value(json).unwrap();
    let snap = AntigravityProvider::new().parse_user_status(resp).unwrap();
    let gemini = snap
        .extra_rate_windows
        .iter()
        .find(|window| window.title.contains("Gemini"))
        .unwrap();
    let claude = snap
        .extra_rate_windows
        .iter()
        .find(|window| window.title.contains("Claude"))
        .unwrap();
    assert!(gemini.usage_known);
    assert!(!claude.usage_known);
    assert_eq!(claude.window.used_percent, 0.0);
}

#[test]
fn test_parse_user_status_standard() {
    let resp = make_response(vec![
        ("Claude 3.5 Sonnet", 0.8),
        ("Gemini 2.5 Pro Low", 0.5),
        ("Gemini 2.5 Flash", 0.9),
    ]);
    let provider = AntigravityProvider::new();
    let snap = provider.parse_user_status(resp).unwrap();

    assert!((snap.primary.used_percent - 20.0).abs() < 0.1);
    let sec = snap.secondary.unwrap();
    assert!((sec.used_percent - 50.0).abs() < 0.1);
    let ter = snap.model_specific.unwrap();
    assert!((ter.used_percent - 10.0).abs() < 0.1);
    assert_eq!(snap.extra_rate_windows.len(), 3);
    assert!(
        snap.extra_rate_windows
            .iter()
            .any(|window| window.title == "Gemini 2.5 Flash")
    );
}

#[test]
fn test_parse_user_status_thinking_skipped() {
    let resp = make_response(vec![
        ("Claude Thinking", 0.6),
        ("Claude 3.5 Sonnet", 0.7),
        ("Gemini 2.5 Flash", 0.5),
    ]);
    let provider = AntigravityProvider::new();
    let snap = provider.parse_user_status(resp).unwrap();

    assert!((snap.primary.used_percent - 30.0).abs() < 0.1);
}

#[test]
fn test_parse_user_status_fallback_first() {
    let resp = make_response(vec![("GPT-4o", 0.4), ("Mistral Large", 0.6)]);
    let provider = AntigravityProvider::new();
    let snap = provider.parse_user_status(resp).unwrap();

    assert!((snap.primary.used_percent - 60.0).abs() < 0.1);
    assert!(snap.secondary.is_none());
    assert!(snap.model_specific.is_none());
}

#[test]
fn test_noisy_models_do_not_drive_summary_windows() {
    let resp = make_response(vec![
        ("Gemini 2.5 Flash Image", 0.01),
        ("Gemini 2.5 Pro Lite", 0.02),
        ("Gemini autocomplete internal", 0.03),
        ("Claude 4 Sonnet", 0.8),
        ("Gemini 2.5 Pro Low", 0.6),
        ("Gemini 2.5 Flash", 0.7),
    ]);
    let provider = AntigravityProvider::new();
    let snap = provider.parse_user_status(resp).unwrap();

    assert!((snap.primary.used_percent - 20.0).abs() < 0.1);
    assert!((snap.secondary.unwrap().used_percent - 40.0).abs() < 0.1);
    assert!((snap.model_specific.unwrap().used_percent - 30.0).abs() < 0.1);
    assert!(
        snap.extra_rate_windows
            .iter()
            .any(|window| window.title == "Gemini 2.5 Flash Image")
    );
}

#[test]
fn not_running_error_tells_user_how_to_start() {
    let error = ProviderError::NotInstalled(NOT_RUNNING_MESSAGE.to_string()).to_string();

    assert!(error.contains("Start Google Antigravity and sign in"));
}

// ── agy CLI process matching ───────────────────────────────────────

#[test]
fn detects_agy_exe_cli_process_with_empty_csrf() {
    // agy.exe hosts the language server in-process with no --csrf_token.
    let output =
        "7777\tC:\\Users\\test\\AppData\\Local\\agy\\bin\\agy.exe session --model gemini-2.5-pro";

    let process = AntigravityProvider::parse_process_info(output)
        .expect("agy CLI process should be detected");

    assert_eq!(process.pid, Some(7777));
    assert_eq!(process.source, ProcessSource::Cli);
    assert_eq!(process.csrf_token, "");
    assert!(
        process.csrf_token.is_empty(),
        "agy CLI requires no CSRF token"
    );
    assert_eq!(process.extension_server_csrf_token, None);
    assert_eq!(process.extension_port, None);
}

#[test]
fn detects_quoted_agy_exe_cli_process() {
    // Windows CIM quotes an executable path that contains path separators.
    let output = "7777\t\"C:\\Users\\user\\AppData\\Local\\agy\\bin\\agy.exe\" --model gemini-3.7-flash-high";

    let process = AntigravityProvider::parse_process_info(output)
        .expect("quoted agy CLI process should be detected");

    assert_eq!(process.pid, Some(7777));
    assert_eq!(process.source, ProcessSource::Cli);
    assert!(process.csrf_token.is_empty());
}

#[test]
fn detects_bare_agy_command() {
    // The CLI may appear under the bare `agy` name (no .exe suffix).
    let output = "8888\tagy serve";

    let process = AntigravityProvider::parse_process_info(output)
        .expect("bare agy command should be detected");

    assert_eq!(process.pid, Some(8888));
    assert_eq!(process.source, ProcessSource::Cli);
    assert!(process.csrf_token.is_empty());
}

#[test]
fn detects_antigravity_cli_command() {
    // Upstream also matches antigravity-cli / antigravity_cli.
    let output = "9999\t/opt/homebrew/bin/antigravity-cli status";

    let process = AntigravityProvider::parse_process_info(output)
        .expect("antigravity-cli command should be detected");

    assert_eq!(process.pid, Some(9999));
    assert_eq!(process.source, ProcessSource::Cli);
    assert!(process.csrf_token.is_empty());
}

#[test]
fn ide_match_preferred_over_agy_cli_when_both_running() {
    // When the desktop IDE server and the agy CLI are both running, the
    // CSRF-protected IDE match wins (mirrors upstream process-kind precedence).
    let output = "4242\tC:\\Antigravity\\language_server.exe --csrf_token deadbeef-aaaa-bbbb-cccc-dddddddddddd --extension_server_port 54123\n\
                  7777\tC:\\Users\\test\\AppData\\Local\\agy\\bin\\agy.exe session";

    let process =
        AntigravityProvider::parse_process_info(output).expect("a process should be detected");

    assert_eq!(process.pid, Some(4242));
    assert_eq!(process.source, ProcessSource::Ide);
    assert_eq!(process.csrf_token, "deadbeef-aaaa-bbbb-cccc-dddddddddddd");
}

#[test]
fn agy_cli_matches_when_only_cli_running() {
    // No --csrf_token anywhere: only the agy CLI line should match.
    let output = "7777\tC:\\Users\\test\\AppData\\Local\\agy\\bin\\agy.exe";

    let process = AntigravityProvider::parse_process_info(output)
        .expect("agy CLI should be detected when it is the only match");

    assert_eq!(process.source, ProcessSource::Cli);
    assert!(process.csrf_token.is_empty());
}

#[test]
fn non_antigravity_process_without_csrf_is_not_matched() {
    // An unrelated tokenless process must not be mistaken for the agy CLI.
    let output = "1234\tC:\\Windows\\System32\\notepad.exe";

    let process = AntigravityProvider::parse_process_info(output);

    assert!(process.is_none(), "unrelated process must not match");
}

#[test]
fn is_agy_cli_command_matches_known_names() {
    assert!(is_agy_cli_command("agy serve"));
    assert!(is_agy_cli_command(
        "C:\\Users\\test\\AppData\\Local\\agy\\bin\\agy.exe session"
    ));
    assert!(is_agy_cli_command(
        "C:\\Users\\user\\AppData\\Local\\agy\\bin\\AGY.EXE --model gemini-3.7-flash-high"
    ));
    assert!(is_agy_cli_command(
        "\"C:\\Users\\user\\AppData\\Local\\agy\\bin\\agy.exe\" --model gemini-3.7-flash-high"
    ));
    assert!(is_agy_cli_command("/usr/local/bin/antigravity-cli status"));
    assert!(is_agy_cli_command("/opt/antigravity_cli run"));
    assert!(is_agy_cli_command("\"C:\\Tools\\antigravity-cli\" status"));
    assert!(is_agy_cli_command("\"C:\\Tools\\antigravity_cli\" run"));
}

#[test]
fn is_agy_cli_command_rejects_unrelated_names() {
    // A leading path separator prevents `notantigravity-cli` from matching.
    assert!(!is_agy_cli_command(
        "notagy.exe --model gemini-3.7-flash-high"
    ));
    assert!(!is_agy_cli_command(
        "C:\\Tools\\someagy.exe --model gemini-3.7-flash-high"
    ));
    assert!(!is_agy_cli_command("notantigravity-cli status"));
    assert!(!is_agy_cli_command("C:\\Tools\\notantigravity-cli status"));
    assert!(!is_agy_cli_command("C:\\Windows\\System32\\notepad.exe"));
    assert!(!is_agy_cli_command("language_server.exe --csrf_token abc"));
    assert!(!is_agy_cli_command(""));
}

// ── Upstream 0.50.1 #2963: one lane per quota bucket ──────────────────────

#[test]
fn multiple_models_in_same_quota_bucket_collapse_to_one_lane() {
    // Two Claude variants sharing the same remaining fraction (same 5h
    // session bucket) should produce one extra rate window, not two.
    let resp = make_response(vec![
        ("Claude 3.5 Sonnet", 0.8),
        ("Claude 4 Sonnet", 0.8),
        ("Gemini 2.5 Pro Low", 0.5),
    ]);
    let provider = AntigravityProvider::new();
    let snap = provider.parse_user_status(resp).unwrap();
    assert_eq!(
        snap.extra_rate_windows.len(),
        2,
        "models sharing a quota bucket collapse to one lane"
    );
}

#[test]
fn models_in_distinct_quota_buckets_keep_separate_lanes() {
    let resp = make_response(vec![
        ("Claude 3.5 Sonnet", 0.8),
        ("Claude 4 Sonnet", 0.7),
        ("Gemini 2.5 Pro Low", 0.5),
    ]);
    let provider = AntigravityProvider::new();
    let snap = provider.parse_user_status(resp).unwrap();
    assert_eq!(snap.extra_rate_windows.len(), 3);
}

#[test]
fn not_installed_maps_to_local_runtime_offline() {
    // Antigravity's `NotInstalled` reports the local language-server probe
    // finding nothing to talk to: a runtime that is not running, not a
    // credential problem.
    assert_eq!(
        AntigravityProvider::new()
            .error_state_kind(&ProviderError::NotInstalled(NOT_RUNNING_MESSAGE.into())),
        crate::core::ProviderStateKind::LocalRuntimeOffline
    );
}

#[test]
fn probe_failure_maps_to_unknown() {
    // A failed probe (PowerShell unavailable etc.) says nothing about the
    // runtime itself - inconclusive, not offline.
    assert_eq!(
        AntigravityProvider::new().error_state_kind(&ProviderError::NotInstalled(
            "Failed to detect Antigravity process".into()
        )),
        crate::core::ProviderStateKind::Unknown
    );
}
