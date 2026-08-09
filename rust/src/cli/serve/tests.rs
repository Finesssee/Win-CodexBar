use super::*;

#[test]
fn rejects_non_loopback_hosts_by_default() {
    assert!(allowed_host("127.0.0.1:8080", "127.0.0.1"));
    assert!(allowed_host("localhost", "127.0.0.1"));
    assert!(allowed_host("[::1]:8080", "127.0.0.1"));
    assert!(!allowed_host("example.com", "127.0.0.1"));
    assert!(!allowed_host("127.0.0.1, example.com", "127.0.0.1"));
}

#[test]
fn allows_configured_non_loopback_host() {
    assert!(allowed_host("192.168.1.10:8080", "192.168.1.10"));
    assert!(allowed_host("192.168.1.10", "192.168.1.10"));
    // Loopback Host headers still work when bound to LAN.
    assert!(allowed_host("127.0.0.1:8080", "192.168.1.10"));
    assert!(!allowed_host("10.0.0.1", "192.168.1.10"));
}

#[test]
fn parses_usage_route_provider_query() {
    let request =
        parse_request("GET /usage?provider=deepseek HTTP/1.1\r\nHost: localhost:8080\r\n\r\n")
            .unwrap();
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/usage");
    assert_eq!(request.query.get("provider"), Some(&"deepseek".to_string()));
}

#[test]
fn parses_authorization_header() {
    let request = parse_request(
        "GET /usage HTTP/1.1\r\nHost: localhost:8080\r\nAuthorization: Bearer secret-token\r\n\r\n",
    )
    .unwrap();
    assert_eq!(
        request.authorization.as_deref(),
        Some("Bearer secret-token")
    );
}

#[test]
fn validate_startup_requires_token_and_plain_http_for_lan() {
    assert!(validate_serve_startup("127.0.0.1", false, false).is_none());
    assert!(validate_serve_startup("127.0.0.1", true, false).is_none());

    let missing = validate_serve_startup("0.0.0.0", false, false).unwrap();
    assert!(missing.contains("dashboard-token"));

    let plain = validate_serve_startup("192.168.1.5", true, false).unwrap();
    assert!(plain.contains("allow-plain-http"));

    assert!(validate_serve_startup("192.168.1.5", true, true).is_none());
}

#[test]
fn validate_serve_args_accepts_loopback_without_token() {
    let config = validate_serve_args(&ServeArgs {
        port: 8080,
        host: "localhost".into(),
        refresh_interval: 60,
        dashboard_token: None,
        allow_plain_http: false,
        identity: "redacted".into(),
    })
    .unwrap();
    assert_eq!(config.host, "127.0.0.1");
    assert!(config.token_digest.is_none());
}

#[test]
fn validate_serve_args_rejects_lan_without_token() {
    let err = validate_serve_args(&ServeArgs {
        port: 8080,
        host: "0.0.0.0".into(),
        refresh_interval: 60,
        dashboard_token: None,
        allow_plain_http: true,
        identity: "redacted".into(),
    })
    .unwrap_err()
    .to_string();
    assert!(err.contains("dashboard-token"));
}

#[test]
fn validate_serve_args_rejects_lan_without_allow_plain_http() {
    let err = validate_serve_args(&ServeArgs {
        port: 8080,
        host: "192.168.0.2".into(),
        refresh_interval: 60,
        dashboard_token: Some("tok".into()),
        allow_plain_http: false,
        identity: "redacted".into(),
    })
    .unwrap_err()
    .to_string();
    assert!(err.contains("allow-plain-http"));
}

#[test]
fn auth_gate_constant_time_compare() {
    let digest = sha256_digest(b"correct-token");
    assert!(authorize_request(
        Some("Bearer correct-token"),
        Some(&digest)
    ));
    assert!(!authorize_request(
        Some("Bearer wrong-token"),
        Some(&digest)
    ));
    assert!(!authorize_request(None, Some(&digest)));
    assert!(!authorize_request(
        Some("Basic correct-token"),
        Some(&digest)
    ));
    // No configured token → open.
    assert!(authorize_request(None, None));
}

#[test]
fn bearer_token_extraction() {
    assert_eq!(bearer_token(Some("Bearer abc")), Some("abc".to_string()));
    assert_eq!(bearer_token(Some("bearer  xyz  ")), Some("xyz".to_string()));
    assert_eq!(bearer_token(Some("Bearer")), None);
    assert_eq!(bearer_token(Some("Token abc")), None);
}

#[test]
fn rejects_empty_dashboard_token() {
    let err = resolve_dashboard_token(Some("   "))
        .unwrap_err()
        .to_string();
    assert!(err.contains("empty"));
}

// ── Upstream 0.48.0 #2684: whole-head bound (16 KiB cap + 10 s TOTAL deadline) ──

use std::time::Instant;

/// Connected (server, client) TCP pair on loopback.
async fn connected_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let client = TcpStream::connect(addr).await.unwrap();
    let (server, _) = listener.accept().await.unwrap();
    (server, client)
}

fn head_test_config(budget: Duration, token: Option<&str>) -> ServeConfig {
    ServeConfig {
        host: "127.0.0.1".to_string(),
        port: 8080,
        token_digest: token.map(|t| sha256_digest(t.as_bytes())),
        head_read_budget: budget,
        identity: DashboardIdentity::Redacted,
        dashboard: None,
    }
}

/// Generous budget for tests that must not trip the deadline.
fn fast_budget() -> Duration {
    Duration::from_millis(2_000)
}

/// Complete request head whose `\r\n\r\n` terminator's final byte is
/// exactly byte 16,384 — the upstream-valid boundary.
fn head_at_exact_cap() -> Vec<u8> {
    let mut head = String::from("GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Pad: ");
    let pad = HEAD_CAP - head.len() - 4;
    head.push_str(&"a".repeat(pad));
    head.push_str("\r\n\r\n");
    assert_eq!(head.len(), HEAD_CAP);
    head.into_bytes()
}

/// Send `request`, read until the server closes, return the raw response.
/// Strict outer timeouts turn a hang into a test failure, not a stalled CI.
async fn request_roundtrip(request: &[u8], budget: Duration, token: Option<&str>) -> String {
    let (server, mut client) = connected_pair().await;
    let config = head_test_config(budget, token);
    let server_task = tokio::spawn(async move { handle_client(server, &config).await });
    client.write_all(request).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), client.read_to_end(&mut response))
        .await
        .expect("client read timed out")
        .unwrap();
    // Dropping the client lets the server-side drain finish immediately.
    drop(client);
    server_task.await.unwrap().unwrap();
    String::from_utf8_lossy(&response).into_owned()
}

#[test]
fn invalid_request_response_is_pinned() {
    let response = invalid_request_response();
    assert!(response.starts_with("HTTP/1.1 400 Bad Request\r\n"));
    assert!(response.contains("Cache-Control: no-store\r\n"));
    assert!(response.contains("Connection: close\r\n"));
    assert!(response.ends_with(r#"{"error":"invalid request"}"#));
}

#[test]
fn find_header_end_offsets() {
    assert_eq!(find_header_end(b"\r\n\r\n"), Some(4));
    assert_eq!(find_header_end(b"a\r\n\r\n"), Some(5));
    assert_eq!(find_header_end(b"aa\r\n\r\n"), Some(6));
    assert_eq!(find_header_end(b"a\r\n\r"), None);
    assert_eq!(find_header_end(b"a\r\n\rXX"), None);
    // Terminator straddling a chunk boundary.
    assert_eq!(find_header_end(b"abc\r\n\r"), None);
    assert_eq!(find_header_end(b"abc\r\n\r\ndef"), Some(7));
}

#[tokio::test]
async fn head_reader_accepts_terminator_ending_exactly_at_cap() {
    // Upstream boundary: a terminator whose final byte is byte 16,384 is valid.
    let (mut server, mut client) = connected_pair().await;
    client.write_all(&head_at_exact_cap()).await.unwrap();
    let head = read_request_head(&mut server, fast_budget()).await.unwrap();
    assert_eq!(head.len(), HEAD_CAP);
}

#[tokio::test]
async fn head_ending_exactly_at_cap_parses_and_routes_normally() {
    let response = request_roundtrip(&head_at_exact_cap(), fast_budget(), None).await;
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "exact-cap head must route to /health, got: {response}"
    );
}

#[tokio::test]
async fn head_reader_rejects_at_cap_without_terminator() {
    let (mut server, mut client) = connected_pair().await;
    client.write_all(&[b'x'; HEAD_CAP]).await.unwrap();
    let result = read_request_head(&mut server, fast_budget()).await;
    assert_eq!(result, Err(HeadReadError::Oversize));
}

#[tokio::test]
async fn head_reader_maps_incomplete_eof() {
    let (mut server, mut client) = connected_pair().await;
    client
        .write_all(b"GET /health HTTP/1.1\r\nHost: 127.")
        .await
        .unwrap();
    client.shutdown().await.unwrap();
    let result = read_request_head(&mut server, fast_budget()).await;
    assert_eq!(result, Err(HeadReadError::UnexpectedEof));
}

#[tokio::test]
async fn head_reader_maps_total_deadline_on_silent_client() {
    let (mut server, _client) = connected_pair().await;
    let result = read_request_head(&mut server, Duration::from_millis(150)).await;
    assert_eq!(result, Err(HeadReadError::Deadline));
}

#[tokio::test]
async fn oversized_head_rejected_before_auth_or_routing() {
    // A complete-looking authenticated request line drowned past the cap with
    // no terminator: must be rejected before any bearer evaluation.
    let mut junk = String::from(
        "GET /usage HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer s3cret\r\nX-Pad: ",
    );
    junk.push_str(&"a".repeat(HEAD_CAP));
    assert!(junk.len() > HEAD_CAP);
    let response = request_roundtrip(junk.as_bytes(), fast_budget(), Some("s3cret")).await;
    assert!(response.starts_with("HTTP/1.1 400"), "got: {response}");
    // Proof the bearer gate / routing never ran: not 401, not the usage payload.
    assert!(!response.starts_with("HTTP/1.1 401"));
    assert!(response.contains("Cache-Control: no-store\r\n"));
    assert!(response.contains("Connection: close\r\n"));
    assert!(response.contains(r#""error":"invalid request""#));
}

#[tokio::test]
async fn incomplete_head_eof_gets_pinned_400() {
    let (server, mut client) = connected_pair().await;
    let config = head_test_config(fast_budget(), None);
    let server_task = tokio::spawn(async move { handle_client(server, &config).await });
    client
        .write_all(b"GET /health HTTP/1.1\r\nHost: 127.")
        .await
        .unwrap();
    client.shutdown().await.unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    drop(client);
    server_task.await.unwrap().unwrap();
    let response = String::from_utf8_lossy(&response);
    assert!(response.starts_with("HTTP/1.1 400"), "got: {response}");
    assert!(response.contains("Cache-Control: no-store\r\n"));
    assert!(response.contains(r#""error":"invalid request""#));
}

#[tokio::test]
async fn silent_client_is_closed_at_total_deadline() {
    let budget = Duration::from_millis(250);
    let (server, mut client) = connected_pair().await;
    let config = head_test_config(budget, None);
    let server_task = tokio::spawn(async move { handle_client(server, &config).await });
    let started = Instant::now();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    let elapsed = started.elapsed();
    drop(client);
    server_task.await.unwrap().unwrap();
    assert!(
        elapsed >= budget,
        "deadline fired early: {elapsed:?} < {budget:?}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "silent client outlived the total deadline: {elapsed:?}"
    );
    let response = String::from_utf8_lossy(&response);
    assert!(response.starts_with("HTTP/1.1 400"), "got: {response}");
}

#[tokio::test]
async fn trickling_bytes_do_not_reset_total_head_deadline() {
    // One byte every 60 ms: under a per-read timeout this client would hold its
    // connection for the full 3 s loop; the 400 ms TOTAL budget must kill it.
    // (Red→green mirrored from upstream CLIServeRequestDeadlineLinuxTests.)
    let budget = Duration::from_millis(400);
    let (server, mut client) = connected_pair().await;
    let config = head_test_config(budget, None);
    let server_task = tokio::spawn(async move { handle_client(server, &config).await });

    let started = Instant::now();
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(60)).await;
        if client.write_all(b"a").await.is_err() {
            break;
        }
        // Stop trickling the moment the server answers or closes.
        // peek() does NOT consume bytes — the full response stays readable.
        let mut peek = [0_u8; 1];
        if tokio::time::timeout(Duration::from_millis(10), client.peek(&mut peek))
            .await
            .is_ok()
        {
            break;
        }
    }
    let mut response = Vec::new();
    let _ = client.read_to_end(&mut response).await;
    let elapsed = started.elapsed();
    drop(client);
    server_task.await.unwrap().unwrap();

    assert!(
        elapsed >= budget,
        "deadline fired early: {elapsed:?} < {budget:?}"
    );
    // Upper ceiling 2.5 s: under a per-read-reset design this client would
    // hold the connection for the whole 50-byte loop (~3.5 s incl. peeks),
    // so this still fails red — while tolerating full-suite scheduling lag.
    assert!(
        elapsed < Duration::from_millis(2_500),
        "trickling bytes extended the overall deadline: {elapsed:?}"
    );
    let response = String::from_utf8_lossy(&response);
    assert!(
        response.starts_with("HTTP/1.1 400"),
        "trickling client must get the pinned 400, got: {response}"
    );
    assert!(response.contains("Cache-Control: no-store\r\n"));
    assert!(response.contains(r#""error":"invalid request""#));
}

#[tokio::test]
async fn authenticated_request_succeeds_and_bad_tokens_stay_401() {
    // Deterministic 200: /cost with a provider the local scanner reports as
    // unsupported — full auth pass, zero network/disk access.
    let ok = request_roundtrip(
            b"GET /cost?provider=gemini HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer s3cret\r\n\r\n",
            fast_budget(),
            Some("s3cret"),
        )
        .await;
    assert!(ok.starts_with("HTTP/1.1 200"), "got: {ok}");
    assert!(ok.contains("\"supported\":false"));

    let wrong = request_roundtrip(
            b"GET /cost?provider=gemini HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer nope\r\n\r\n",
            fast_budget(),
            Some("s3cret"),
        )
        .await;
    assert!(wrong.starts_with("HTTP/1.1 401"), "got: {wrong}");

    let missing = request_roundtrip(
        b"GET /cost?provider=gemini HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        fast_budget(),
        Some("s3cret"),
    )
    .await;
    assert!(missing.starts_with("HTTP/1.1 401"), "got: {missing}");
}

#[tokio::test]
async fn host_gate_unchanged_on_hardened_path() {
    let forbidden = request_roundtrip(
        b"GET /health HTTP/1.1\r\nHost: example.com\r\n\r\n",
        fast_budget(),
        None,
    )
    .await;
    assert!(forbidden.starts_with("HTTP/1.1 403"), "got: {forbidden}");
    assert!(forbidden.contains(r#""error":"forbidden host""#));

    let ok = request_roundtrip(
        b"GET /health HTTP/1.1\r\nHost: localhost:9999\r\n\r\n",
        fast_budget(),
        None,
    )
    .await;
    assert!(ok.starts_with("HTTP/1.1 200"), "got: {ok}");
}

#[tokio::test]
async fn over_cap_connection_closes_immediately_without_response() {
    // Upstream 0.48.0 parity: maximumConnections = 16; slot 17 is closed at
    // once, no response bytes, and a freed slot is usable again.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let config = Arc::new(head_test_config(Duration::from_secs(60), None));
    let server_task = tokio::spawn(serve_listener(listener, config, MAX_CONNECTIONS));

    // Fill every permit with trickling clients that never complete a head.
    let mut tricklers = Vec::new();
    for _ in 0..MAX_CONNECTIONS {
        let mut client = TcpStream::connect(addr).await.unwrap();
        tricklers.push(tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if client.write_all(b"a").await.is_err() {
                    break;
                }
            }
        }));
    }

    // Probe until the gate is provably full: an over-cap connection gets an
    // immediate EOF with zero response bytes.
    let mut rejected_seen = false;
    for _ in 0..40 {
        let mut probe = TcpStream::connect(addr).await.unwrap();
        let mut buf = [0_u8; 16];
        match tokio::time::timeout(Duration::from_millis(300), probe.read(&mut buf)).await {
            Ok(Ok(0)) => {
                rejected_seen = true;
                break;
            }
            // Probe landed in a still-filling slot; free it and retry.
            _ => drop(probe),
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        rejected_seen,
        "over-cap connection never got the immediate close"
    );

    // Ending the tricklers releases their permits via EOF; a normal client
    // must then be served (strict outer timeout).
    for task in &tricklers {
        task.abort();
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
    let mut good = TcpStream::connect(addr).await.unwrap();
    good.write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), good.read_to_end(&mut response))
        .await
        .expect("no connection slot freed after trickling clients ended")
        .unwrap();
    assert!(
        String::from_utf8_lossy(&response).starts_with("HTTP/1.1 200"),
        "freed slot must serve a normal request, got: {}",
        String::from_utf8_lossy(&response)
    );
    server_task.abort();
}

#[tokio::test]
async fn deadline_driven_release_frees_gated_slot() {
    // Regression (review follow-up): a semaphore permit MUST be owned for the
    // whole handle_client future and released when the SERVER's total head
    // deadline completes its 400/close path — not by client EOF/manual drop.
    // The holder client stays connected the entire test.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let holder_budget = Duration::from_millis(500);
    let config = Arc::new(head_test_config(holder_budget, None));
    let server_task = tokio::spawn(serve_listener(listener, config, 1));

    // Fill the single permit with a holder that never sends a single byte.
    let mut holder = TcpStream::connect(addr).await.unwrap();

    // Synchronize until that permit is provably held: an over-cap probe gets
    // an immediate close with zero response bytes.
    let mut rejected = false;
    for _ in 0..40 {
        let mut probe = TcpStream::connect(addr).await.unwrap();
        let mut buf = [0_u8; 16];
        match tokio::time::timeout(Duration::from_millis(300), probe.read(&mut buf)).await {
            Ok(Ok(0)) => {
                rejected = true;
                break;
            }
            // Probe landed while the holder was still being accepted; retry.
            _ => drop(probe),
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(rejected, "over-cap probe was never closed immediately");

    // Causality phase: the holder is NOT dropped/aborted/shut down. The
    // server's injected total head deadline expires on its own, completing
    // the pinned 400/close path. read_to_end returns at the server's FIN;
    // the holder socket itself stays OPEN.
    let mut holder_response = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(3),
        holder.read_to_end(&mut holder_response),
    )
    .await
    .expect("server never drove its deadline/close on the held slot")
    .unwrap();
    let holder_response = String::from_utf8_lossy(&holder_response);
    assert!(
        holder_response.starts_with("HTTP/1.1 400"),
        "deadline path must answer the holder with the pinned 400, got: {holder_response}"
    );
    assert!(holder_response.contains("Cache-Control: no-store\r\n"));
    assert!(holder_response.contains(r#""error":"invalid request""#));

    // The permit frees only when the server task finishes — after the
    // deadline AND the bounded (~1 s) graceful-drain that runs while the
    // still-connected holder stays silent. Retry a normal client until the
    // freed slot serves it; early retries may still be over-cap closed.
    let started = Instant::now();
    let health = loop {
        let attempt = async {
            let mut good = TcpStream::connect(addr).await.ok()?;
            good.write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
                .await
                .ok()?;
            let mut response = Vec::new();
            tokio::time::timeout(Duration::from_millis(800), good.read_to_end(&mut response))
                .await
                .ok()?
                .ok()?;
            Some(String::from_utf8_lossy(&response).into_owned())
        };
        if let Some(text) = attempt.await
            && text.starts_with("HTTP/1.1 200")
        {
            break text;
        }
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "permit was never released after the server deadline + graceful drain"
        );
        tokio::time::sleep(Duration::from_millis(120)).await;
    };
    assert!(health.contains("\"status\":\"ok\""), "got: {health}");
    // Holder is still connected throughout everything above; cleanup only
    // after all success assertions.
    server_task.abort();
    drop(holder);
}

// ── Upstream 0.48.0 A1–A5: dashboard routes ───────────────────────────

use dashboard::coordinator::SnapshotBuildFn;
use dashboard::snapshot::{
    AccountFetchEnvelope, ClaudeAccountsInput, DashboardIdentity as DashboardIdMode,
    ProviderFetchEnvelope, SnapshotInput, build_snapshot,
};

fn stub_build(identity: DashboardIdMode, with_accounts: bool, delay: Duration) -> SnapshotBuildFn {
    std::sync::Arc::new(move || {
        Box::pin(async move {
            if delay > Duration::ZERO {
                tokio::time::sleep(delay).await;
            }
            let mut usage = crate::core::UsageSnapshot::new(crate::core::RateWindow::new(11.0));
            usage.account_email = Some("dev@example.com".to_string());
            usage.login_method = Some("Claude Max".to_string());
            let claude_accounts = with_accounts.then(|| ClaudeAccountsInput {
                accounts: Ok(vec![AccountFetchEnvelope {
                    id: "u-1".to_string(),
                    label: "Work".to_string(),
                    active: true,
                    fetch: Ok(crate::core::ProviderFetchResult::new(usage.clone(), "test")),
                }]),
            });
            Ok(build_snapshot(&SnapshotInput {
                providers: vec![ProviderFetchEnvelope {
                    id: "claude".to_string(),
                    display_name: "Claude".to_string(),
                    session_label: "Session".to_string(),
                    weekly_label: "Weekly".to_string(),
                    fetch: Ok(crate::core::ProviderFetchResult::new(usage, "test")),
                }],
                costs: std::collections::HashMap::new(),
                claude_accounts,
                identity,
                generated_at: chrono::Utc::now(),
                refresh_seconds: 60,
                version: Some("test".to_string()),
                order: vec![],
                enabled: std::collections::BTreeSet::new(),
            }))
        })
    })
}

fn stub_state_ok() -> dashboard::DashboardState {
    dashboard::DashboardState::stub(
        stub_build(DashboardIdMode::Redacted, false, Duration::ZERO),
        3600,
        DashboardIdMode::Redacted,
    )
}

fn dashboard_test_config(
    token: Option<&str>,
    state: Option<dashboard::DashboardState>,
) -> ServeConfig {
    let mut config = head_test_config(fast_budget(), token);
    config.dashboard = state;
    config
}

#[test]
fn resolve_route_maps_paths() {
    let req = |path: &str, query: &[(&str, &str)]| {
        let mut request =
            parse_request(&format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")).unwrap();
        for (k, v) in query {
            request.query.insert(k.to_string(), v.to_string());
        }
        request
    };
    assert_eq!(
        resolve_route(&req("/", &[])),
        Some(ServeRoute::DashboardHome)
    );
    assert_eq!(
        resolve_route(&req("/health", &[])),
        Some(ServeRoute::Health)
    );
    assert_eq!(
        resolve_route(&req("/usage?provider=codex", &[])),
        Some(ServeRoute::Usage {
            provider: Some("codex".to_string())
        })
    );
    assert_eq!(
        resolve_route(&req("/dashboard/v1/snapshot", &[])),
        Some(ServeRoute::DashboardSnapshot)
    );
    assert_eq!(
        resolve_route(&req("/icons/ProviderIcon-codex.svg", &[])),
        Some(ServeRoute::ProviderIcon {
            name: "ProviderIcon-codex".to_string()
        })
    );
    assert_eq!(resolve_route(&req("/icons/../x.svg", &[])), None);
    assert_eq!(resolve_route(&req("/icons/.svg", &[])), None);
    assert_eq!(resolve_route(&req("/dashboard/v1/other", &[])), None);
    assert_eq!(resolve_route(&req("/usage.json", &[])), None);
}

#[tokio::test]
async fn dashboard_home_serves_html_no_store() {
    let config = dashboard_test_config(None, Some(stub_state_ok()));
    let response =
        request_roundtrip_dashboard(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n", config).await;
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "got: {}",
        &response[..80.min(response.len())]
    );
    assert!(response.contains("Content-Type: text/html; charset=utf-8\r\n"));
    assert!(response.contains("Cache-Control: no-store\r\n"));
    assert!(response.contains("/dashboard/v1/snapshot"));
    assert!(response.contains("ProviderIcon-codex.svg"));
}

#[tokio::test]
async fn icon_route_serves_svg_immutable_and_404s_unknown() {
    let config = dashboard_test_config(None, None);
    let response = request_roundtrip_dashboard(
        b"GET /icons/ProviderIcon-codex.svg HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        config,
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "got: {}",
        &response[..80.min(response.len())]
    );
    assert!(response.contains("Content-Type: image/svg+xml\r\n"));
    assert!(response.contains("Cache-Control: public, max-age=86400, immutable\r\n"));
    assert!(response.contains("<svg"));

    let config = dashboard_test_config(None, None);
    let missing = request_roundtrip_dashboard(
        b"GET /icons/ProviderIcon-nope.svg HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        config,
    )
    .await;
    assert!(missing.starts_with("HTTP/1.1 404"));
}

#[tokio::test]
async fn snapshot_route_gates_on_token_and_advertises_bearer_on_401() {
    // No token -> 401 + WWW-Authenticate (upstream dashboard-rule parity).
    let config = dashboard_test_config(Some("s3cret"), Some(stub_state_ok()));
    let unauthorized = request_roundtrip_dashboard(
        b"GET /dashboard/v1/snapshot HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        config,
    )
    .await;
    assert!(
        unauthorized.starts_with("HTTP/1.1 401"),
        "got: {unauthorized}"
    );
    assert!(unauthorized.contains("WWW-Authenticate: Bearer\r\n"));

    // Valid token -> 200 + schema v1 payload + no-store.
    let config = dashboard_test_config(Some("s3cret"), Some(stub_state_ok()));
    let response = request_roundtrip_dashboard(
            b"GET /dashboard/v1/snapshot HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer s3cret\r\n\r\n",
            config,
        )
        .await;
    assert!(response.starts_with("HTTP/1.1 200"), "got: {response}");
    assert!(response.contains("Cache-Control: no-store\r\n"));
    assert!(response.contains("\"schemaVersion\": 1"));
    assert!(response.contains("\"providers\""));
}

#[tokio::test]
async fn dashboard_home_and_icons_are_public_when_token_configured() {
    let config = dashboard_test_config(Some("s3cret"), Some(stub_state_ok()));
    let response =
        request_roundtrip_dashboard(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n", config).await;
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "shell stays public: {response}"
    );
    let config = dashboard_test_config(Some("s3cret"), Some(stub_state_ok()));
    let icon = request_roundtrip_dashboard(
        b"GET /icons/ProviderIcon-claude.svg HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        config,
    )
    .await;
    assert!(
        icon.starts_with("HTTP/1.1 200"),
        "icons stay public: {icon}"
    );
    assert!(!icon.contains("WWW-Authenticate"));
}

#[tokio::test]
async fn snapshot_identity_modes_redact_or_expose() {
    // Redacted (default): `redacted@domain`, raw address never leaks.
    let state = dashboard::DashboardState::stub(
        stub_build(DashboardIdMode::Redacted, false, Duration::ZERO),
        3600,
        DashboardIdMode::Redacted,
    );
    let config = dashboard_test_config(None, Some(state));
    let redacted = request_roundtrip_dashboard(
        b"GET /dashboard/v1/snapshot HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        config,
    )
    .await;
    assert!(redacted.contains("redacted@example.com"), "got: {redacted}");
    assert!(
        !redacted.contains("dev@example.com"),
        "raw email leaked: {redacted}"
    );

    // Full opt-in: real account email exposed.
    let state = dashboard::DashboardState::stub(
        stub_build(DashboardIdMode::Full, false, Duration::ZERO),
        3600,
        DashboardIdMode::Full,
    );
    let config = dashboard_test_config(None, Some(state));
    let full = request_roundtrip_dashboard(
        b"GET /dashboard/v1/snapshot HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        config,
    )
    .await;
    assert!(full.contains("dev@example.com"), "got: {full}");
}

#[tokio::test]
async fn snapshot_claude_accounts_nest_under_claude_row() {
    let state = dashboard::DashboardState::stub(
        stub_build(DashboardIdMode::Redacted, true, Duration::ZERO),
        3600,
        DashboardIdMode::Redacted,
    );
    let config = dashboard_test_config(None, Some(state));
    let response = request_roundtrip_dashboard(
        b"GET /dashboard/v1/snapshot HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        config,
    )
    .await;
    assert!(response.contains("\"accounts\""), "got: {response}");
    assert!(response.contains("\"label\": \"Work\""), "got: {response}");
    assert!(response.contains("\"active\": true"), "got: {response}");
    assert!(response.contains("redacted@example.com"));
}

#[tokio::test]
async fn snapshot_late_build_is_delivered_not_discarded() {
    // F9/2717 parity: a slow snapshot build completes and the response carries
    // the finished result — never a discarded-build error.
    let state = dashboard::DashboardState::stub(
        stub_build(DashboardIdMode::Redacted, false, Duration::from_millis(250)),
        3600,
        DashboardIdMode::Redacted,
    );
    let config = dashboard_test_config(None, Some(state));
    let started = std::time::Instant::now();
    let response = request_roundtrip_dashboard(
        b"GET /dashboard/v1/snapshot HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        config,
    )
    .await;
    let elapsed = started.elapsed();
    assert!(response.starts_with("HTTP/1.1 200"), "got: {response}");
    assert!(response.contains("\"schemaVersion\": 1"));
    assert!(
        elapsed >= Duration::from_millis(250),
        "response arrived before the build finished: {elapsed:?}"
    );
}

/// Roundtrip helper for route-level tests (separate from head-level helper).
async fn request_roundtrip_dashboard(request: &[u8], config: ServeConfig) -> String {
    let (server, mut client) = connected_pair().await;
    let server_task = tokio::spawn(async move { handle_client(server, &config).await });
    client.write_all(request).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), client.read_to_end(&mut response))
        .await
        .expect("client read timed out")
        .unwrap();
    drop(client);
    server_task.await.unwrap().unwrap();
    String::from_utf8_lossy(&response).into_owned()
}
