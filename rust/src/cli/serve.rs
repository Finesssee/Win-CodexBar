//! Local HTTP server for scriptable usage/cost JSON.
//!
//! Upstream 0.44 #2227: bind host + optional dashboard bearer token gate.
//! Non-loopback binds require a token and `--allow-plain-http`.
//! Upstream 0.48.0 #2684: the request head is bounded as a whole — 16,384-byte
//! cap and a single 10 s monotonic deadline across ALL reads, enforced before
//! any Host allowlist or bearer handling; over-cap connections close instantly.

use std::sync::Arc;
use std::time::Duration;

use clap::Args;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

use super::usage::ProviderSelection;
use crate::core::{CostScanOptions, FetchContext, ProviderId, SourceMode, instantiate_provider};
use crate::cost_scanner::CostScanner;

const DASHBOARD_TOKEN_ENV: &str = "CODEXBAR_DASHBOARD_TOKEN";

/// Maximum bytes accepted for one complete HTTP request head, `\r\n\r\n`
/// terminator included. A terminator whose final byte is exactly byte 16,384 is
/// valid; anything more is rejected without being consumed or parsed.
/// Upstream 0.48.0 #2684: `readRequest` loops `while data.count < 16384`.
const HEAD_CAP: usize = 16 * 1024;

/// Bytes read per socket poll while assembling the head (upstream uses 4096).
const HEAD_READ_CHUNK: usize = 4096;

/// Overall budget for delivering one complete request head. Upstream 0.48.0
/// #2684: `requestTotalReadTimeoutMilliseconds = 10000` — one monotonic budget
/// across all reads; a per-read timeout alone can be reset indefinitely by a
/// client trickling one byte per window.
const HEAD_READ_TIMEOUT: Duration = Duration::from_millis(10_000);

/// Maximum concurrent client connections; over-cap connections are closed
/// immediately without a response. Upstream 0.48.0 `maximumConnections = 16`.
const MAX_CONNECTIONS: usize = 16;

/// Why assembling a request head failed. Every variant maps to a single
/// 400 Bad Request + close (upstream `.invalidRequest`); nothing is parsed,
/// authenticated, or routed on a failed head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeadReadError {
    /// The overall head-read budget elapsed before the head was complete.
    Deadline,
    /// The head reached [`HEAD_CAP`] bytes without a complete `\r\n\r\n`
    /// terminator.
    Oversize,
    /// The client half-closed or errored before the head was complete.
    UnexpectedEof,
}

#[derive(Args, Debug, Clone)]
pub struct ServeArgs {
    /// Local HTTP port
    #[arg(long, default_value = "8080")]
    pub port: u16,

    /// IPv4 bind address or localhost (default: 127.0.0.1)
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

    /// Response cache TTL in seconds
    #[arg(long = "refresh-interval", default_value = "60")]
    pub refresh_interval: u64,

    /// Bearer token for /usage and /cost (prefer CODEXBAR_DASHBOARD_TOKEN)
    #[arg(long = "dashboard-token", env = "CODEXBAR_DASHBOARD_TOKEN")]
    pub dashboard_token: Option<String>,

    /// Accept sending the dashboard token over cleartext HTTP on a non-loopback host
    #[arg(long = "allow-plain-http", default_value_t = false)]
    pub allow_plain_http: bool,
}

/// Normalized serve bind configuration after startup validation.
#[derive(Debug, Clone)]
struct ServeConfig {
    host: String,
    port: u16,
    token_digest: Option<[u8; 32]>,
    /// Overall budget for reading one request head. Production uses
    /// [`HEAD_READ_TIMEOUT`]; tests inject a short budget (upstream 0.48.0
    /// #2684 makes the deadline injectable for exactly this reason).
    head_read_budget: Duration,
}

pub async fn run(args: ServeArgs) -> anyhow::Result<()> {
    let config = validate_serve_args(&args)?;
    let listener = TcpListener::bind((config.host.as_str(), config.port)).await?;
    eprintln!(
        "CodexBar server listening on http://{}:{}",
        config.host, config.port
    );
    if !is_loopback_host(&config.host) {
        eprintln!(
            "Warning: plain HTTP on a non-loopback host; the bearer token gating \
             /usage and /cost crosses the network in cleartext on every request."
        );
    }

    serve_listener(listener, Arc::new(config), MAX_CONNECTIONS).await
}

/// Accept loop with the upstream-parity concurrency gate: at most
/// `max_connections` clients are served at once; a connection arriving when
/// every slot is held is closed immediately without a response. Combined with
/// the whole-head deadline in [`read_request_head`], slow-trickle clients can
/// no longer exhaust every slot pre-auth (upstream 0.48.0 #2684).
async fn serve_listener(
    listener: TcpListener,
    config: Arc<ServeConfig>,
    max_connections: usize,
) -> anyhow::Result<()> {
    let gate = Arc::new(Semaphore::new(max_connections));
    loop {
        let (stream, _) = listener.accept().await?;
        let Ok(permit) = gate.clone().try_acquire_owned() else {
            // Over-cap: close immediately without a response (upstream parity).
            drop(stream);
            continue;
        };
        let config = config.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(error) = handle_client(stream, &config).await {
                tracing::debug!("serve client error: {error}");
            }
        });
    }
}

/// Startup validation for bind host + dashboard token flags.
///
/// | bind host    | token   | --allow-plain-http | result                          |
/// |--------------|---------|--------------------|---------------------------------|
/// | loopback     | absent  | any                | serve                           |
/// | loopback     | present | any                | serve; data routes gated        |
/// | non-loopback | absent  | any                | error: token required           |
/// | non-loopback | present | absent             | error: pass --allow-plain-http  |
/// | non-loopback | present | present            | serve; data routes gated        |
fn validate_serve_args(args: &ServeArgs) -> anyhow::Result<ServeConfig> {
    let host = bind_host(&args.host);
    if !is_supported_ipv4_bind_host(&host) {
        anyhow::bail!("--host must be 'localhost' or an IPv4 address.");
    }
    if args.port == 0 {
        anyhow::bail!("--port must be between 1 and 65535.");
    }

    let token = resolve_dashboard_token(args.dashboard_token.as_deref())?;
    if let Some(err) = validate_serve_startup(&host, token.is_some(), args.allow_plain_http) {
        anyhow::bail!("{err}");
    }

    Ok(ServeConfig {
        host,
        port: args.port,
        token_digest: token.as_ref().map(|t| sha256_digest(t.as_bytes())),
        head_read_budget: HEAD_READ_TIMEOUT,
    })
}

fn resolve_dashboard_token(cli_token: Option<&str>) -> anyhow::Result<Option<String>> {
    // Prefer env (already merged by clap env=) but still reject empty/whitespace.
    if let Some(raw) = cli_token {
        let bearer = raw.trim();
        if bearer.is_empty() {
            anyhow::bail!(
                "{DASHBOARD_TOKEN_ENV} / --dashboard-token must not be empty or whitespace."
            );
        }
        return Ok(Some(bearer.to_string()));
    }
    Ok(None)
}

fn validate_serve_startup(
    host: &str,
    has_configured_bearer: bool,
    allow_plain_http: bool,
) -> Option<String> {
    if is_loopback_host(host) {
        return None;
    }
    if !has_configured_bearer {
        return Some(format!(
            "--dashboard-token (or {DASHBOARD_TOKEN_ENV}) is required for non-loopback --host '{host}'."
        ));
    }
    if !allow_plain_http {
        return Some(format!(
            "Refusing to serve the dashboard token over cleartext HTTP on non-loopback --host '{host}'. \
             Pass --allow-plain-http to accept that the bearer token crosses the network \
             unencrypted on every request."
        ));
    }
    None
}

fn bind_host(host: &str) -> String {
    let trimmed = host.trim();
    if trimmed.eq_ignore_ascii_case("localhost") {
        "127.0.0.1".to_string()
    } else {
        trimmed.to_string()
    }
}

fn is_loopback_host(host: &str) -> bool {
    let normalized = host.trim().to_ascii_lowercase();
    normalized == "localhost"
        || normalized == "127.0.0.1"
        || normalized == "::1"
        || normalized == "[::1]"
        || normalized.starts_with("127.")
}

fn is_supported_ipv4_bind_host(host: &str) -> bool {
    let parts: Vec<_> = host.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    parts.iter().all(|part| {
        !part.is_empty()
            && part.bytes().all(|b| b.is_ascii_digit())
            && part.parse::<u8>().is_ok_and(|v| v.to_string() == *part)
    })
}

fn sha256_digest(bytes: &[u8]) -> [u8; 32] {
    let hash = Sha256::digest(bytes);
    let mut out = [0_u8; 32];
    out.copy_from_slice(&hash);
    out
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0_u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn authorize_request(auth_header: Option<&str>, expected: Option<&[u8; 32]>) -> bool {
    let Some(expected) = expected else {
        // No token configured: open on loopback (startup already blocks non-loopback without token).
        return true;
    };
    let Some(token) = bearer_token(auth_header) else {
        return false;
    };
    let digest = sha256_digest(token.as_bytes());
    constant_time_eq(&digest, expected)
}

fn bearer_token(authorization: Option<&str>) -> Option<String> {
    let authorization = authorization?;
    let trimmed = authorization.trim();
    let rest = trimmed
        .strip_prefix("Bearer ")
        .or_else(|| trimmed.strip_prefix("bearer "))?;
    let token = rest.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

async fn handle_client(mut stream: TcpStream, config: &ServeConfig) -> anyhow::Result<()> {
    // Upstream 0.48.0 #2684: the head is assembled inside one overall budget and
    // byte cap BEFORE any Host allowlist or bearer handling. Any head failure is
    // a single 400 + close; nothing is parsed, authenticated, or routed.
    let head = match read_request_head(&mut stream, config.head_read_budget).await {
        Ok(head) => head,
        Err(_) => {
            respond_and_close_gracefully(&mut stream, invalid_request_response().as_bytes()).await;
            return Ok(());
        }
    };
    let request = String::from_utf8_lossy(&head);
    let response = match parse_request(&request) {
        Ok(request) => route_request(&request, config).await,
        Err(status) => json_response(status, serde_json::json!({ "error": "bad request" })),
    };
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}

/// Read one complete request head under one overall deadline.
///
/// Upstream 0.48.0 #2684 (`CLILocalHTTPServer.readRequest`): the deadline is a
/// single monotonic budget for the WHOLE head (default 10 s) — never a per-read
/// timeout that a client sending one byte per window could reset forever.
/// `tokio::time::timeout` around the entire loop implements exactly that
/// semantic and cannot be extended by arriving bytes.
async fn read_request_head(
    stream: &mut TcpStream,
    budget: Duration,
) -> Result<Vec<u8>, HeadReadError> {
    tokio::time::timeout(budget, read_head_loop(stream))
        .await
        .map_err(|_| HeadReadError::Deadline)?
}

/// Assemble the head until the `\r\n\r\n` terminator, capped at [`HEAD_CAP`]
/// bytes. A terminator whose final byte is exactly byte 16,384 is valid; at the
/// cap without a complete terminator the request is rejected, and each read is
/// length-clamped so byte 16,385 is never consumed.
async fn read_head_loop(stream: &mut TcpStream) -> Result<Vec<u8>, HeadReadError> {
    let mut buf = Vec::with_capacity(HEAD_READ_CHUNK);
    let mut chunk = [0_u8; HEAD_READ_CHUNK];
    loop {
        if let Some(end) = find_header_end(&buf) {
            buf.truncate(end);
            return Ok(buf);
        }
        if buf.len() >= HEAD_CAP {
            return Err(HeadReadError::Oversize);
        }
        // Clamp the read so we can never pull past the cap.
        let want = (HEAD_CAP - buf.len()).min(HEAD_READ_CHUNK);
        let n = stream
            .read(&mut chunk[..want])
            .await
            .map_err(|_| HeadReadError::UnexpectedEof)?;
        if n == 0 {
            return Err(HeadReadError::UnexpectedEof);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// Offset just past `\r\n\r\n` when `buf` holds a complete head terminator.
fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

/// Upstream 0.48.0 pinned failure response for head-deadline / oversize /
/// incomplete-EOF: 400 Bad Request with `{"error":"invalid request"}`,
/// `Cache-Control: no-store`, `Connection: close`. Upstream has no 408/431.
fn invalid_request_response() -> String {
    json_response_with_headers(
        400,
        serde_json::json!({ "error": "invalid request" }),
        &[("Cache-Control", "no-store")],
    )
}

/// Deliver an error response on a rejected head reliably: write it, half-close
/// the write side so the client sees FIN right after the bytes, then briefly
/// drain whatever the client already sent. Closing a socket with unread data in
/// its receive queue tears the connection down with RST on Windows, discarding
/// the response before the client reads it — the drain keeps the close clean.
/// The drain is bounded independently of the head-read budget, so this cannot
/// re-open the slow-trickle hold that #2684 closes.
async fn respond_and_close_gracefully(stream: &mut TcpStream, response: &[u8]) {
    let _ = stream.write_all(response).await;
    let _ = stream.shutdown().await;
    let drain = async {
        let mut sink = [0_u8; 512];
        while let Ok(n) = stream.read(&mut sink).await {
            if n == 0 {
                break;
            }
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(1), drain).await;
}

async fn route_request(request: &ServeRequest, config: &ServeConfig) -> String {
    if request.method != "GET" {
        return json_response(405, serde_json::json!({ "error": "method not allowed" }));
    }
    if !allowed_host(&request.host, &config.host) {
        return json_response(403, serde_json::json!({ "error": "forbidden host" }));
    }

    match request.path.as_str() {
        "/health" => json_response(
            200,
            serde_json::json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }),
        ),
        "/usage" => {
            if !authorize_request(
                request.authorization.as_deref(),
                config.token_digest.as_ref(),
            ) {
                return json_response(401, serde_json::json!({ "error": "unauthorized" }));
            }
            usage_response(request.query.get("provider").map(String::as_str)).await
        }
        "/cost" => {
            if !authorize_request(
                request.authorization.as_deref(),
                config.token_digest.as_ref(),
            ) {
                return json_response(401, serde_json::json!({ "error": "unauthorized" }));
            }
            cost_response(request.query.get("provider").map(String::as_str)).await
        }
        _ => json_response(404, serde_json::json!({ "error": "not found" })),
    }
}

async fn usage_response(provider: Option<&str>) -> String {
    let selection = match ProviderSelection::from_arg(provider) {
        Ok(selection) => selection,
        Err(error) => {
            return json_response(400, serde_json::json!({ "error": error.to_string() }));
        }
    };
    let ctx = FetchContext {
        source_mode: SourceMode::Auto,
        include_credits: true,
        web_timeout: 60,
        verbose: false,
        manual_cookie_header: None,
        api_key: None,
        workspace_id: None,
        api_region: None,
        gateway_url: None,
        auto_prefer_web: false,
    };

    let mut results = Vec::new();
    for provider_id in selection.as_list() {
        let provider = instantiate_provider(provider_id);
        match provider.fetch_usage(&ctx).await {
            Ok(result) => results.push(serde_json::json!({
                "provider": provider_id.cli_name(),
                "source": result.source_label,
                "usage": result.usage,
                "cost": result.cost,
            })),
            Err(error) => results.push(serde_json::json!({
                "provider": provider_id.cli_name(),
                "error": error.to_string(),
            })),
        }
    }
    json_response(200, serde_json::Value::Array(results))
}

async fn cost_response(provider: Option<&str>) -> String {
    let selection = match ProviderSelection::from_arg(provider) {
        Ok(selection) => selection,
        Err(error) => {
            return json_response(400, serde_json::json!({ "error": error.to_string() }));
        }
    };
    let scanner = CostScanner::new(30).with_options(CostScanOptions::app_driven());
    let mut results = Vec::new();
    for provider_id in selection.as_list() {
        let (supported, summary) = match provider_id {
            ProviderId::Codex => (true, scanner.scan_codex()),
            ProviderId::Claude => (true, scanner.scan_claude()),
            _ => (false, Default::default()),
        };
        if supported {
            results.push(serde_json::json!({
                "provider": provider_id.cli_name(),
                "supported": true,
                "days_scanned": 30,
                "cost": {
                    "total_usd": summary.total_cost_usd,
                    "currency": "USD"
                },
                "tokens": {
                    "input": summary.input_tokens,
                    "output": summary.output_tokens,
                    "cached": summary.cached_tokens
                },
                "sessions_count": summary.sessions_count,
                "by_model": summary.by_model,
            }));
        } else {
            results.push(serde_json::json!({
                "provider": provider_id.cli_name(),
                "supported": false,
                "error": "Local cost scanning not available for this provider"
            }));
        }
    }
    json_response(200, serde_json::Value::Array(results))
}

#[derive(Debug)]
struct ServeRequest {
    method: String,
    path: String,
    host: String,
    authorization: Option<String>,
    query: std::collections::HashMap<String, String>,
}

fn parse_request(raw: &str) -> Result<ServeRequest, u16> {
    let mut lines = raw.split("\r\n");
    let first = lines.next().ok_or(400_u16)?;
    let mut parts = first.split_whitespace();
    let method = parts.next().ok_or(400_u16)?.to_uppercase();
    let target = parts.next().ok_or(400_u16)?;
    if parts.next().is_none() || !target.starts_with('/') {
        return Err(400);
    }

    let mut hosts = Vec::new();
    let mut authorization = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(400);
        };
        if name.trim().eq_ignore_ascii_case("host") {
            hosts.push(value.trim().to_string());
        } else if name.trim().eq_ignore_ascii_case("authorization") {
            authorization = Some(value.trim().to_string());
        }
    }
    if hosts.len() != 1 {
        return Err(400);
    }

    let (path, query) = parse_target(target);
    Ok(ServeRequest {
        method,
        path,
        host: hosts.remove(0),
        authorization,
        query,
    })
}

fn parse_target(target: &str) -> (String, std::collections::HashMap<String, String>) {
    let Some((path, query_string)) = target.split_once('?') else {
        return (target.to_string(), Default::default());
    };
    let query = query_string
        .split('&')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            Some((url_decode(key), url_decode(value)))
        })
        .collect();
    (path.to_string(), query)
}

fn allowed_host(host: &str, bind_host: &str) -> bool {
    let trimmed = host.trim();
    if trimmed.is_empty() || trimmed.contains(',') {
        return false;
    }
    let without_port = if let Some(rest) = trimmed.strip_prefix('[') {
        let Some((addr, port)) = rest.split_once(']') else {
            return false;
        };
        if !port.is_empty() && !valid_port_suffix(port) {
            return false;
        }
        format!("[{addr}]")
    } else {
        let segments: Vec<_> = trimmed.split(':').collect();
        match segments.as_slice() {
            [host] => host.to_string(),
            [host, port] if valid_port(port) => host.to_string(),
            _ => return false,
        }
    };
    let host_lc = without_port.to_ascii_lowercase();
    let bind_lc = bind_host.trim().to_ascii_lowercase();

    // Always accept loopback Host headers.
    if matches!(
        host_lc.as_str(),
        "127.0.0.1" | "localhost" | "localhost." | "[::1]"
    ) {
        return true;
    }
    // Also accept the configured non-loopback bind host.
    host_lc == bind_lc
}

fn valid_port_suffix(raw: &str) -> bool {
    raw.is_empty() || raw.strip_prefix(':').is_some_and(valid_port)
}

fn valid_port(raw: &str) -> bool {
    raw.parse::<u16>().is_ok_and(|port| port > 0)
}

fn url_decode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut bytes = raw.as_bytes().iter().copied().peekable();
    while let Some(byte) = bytes.next() {
        if byte == b'+' {
            out.push(' ');
        } else if byte == b'%' {
            let hi = bytes.next();
            let lo = bytes.next();
            if let (Some(hi), Some(lo)) = (hi, lo)
                && let Ok(value) =
                    u8::from_str_radix(std::str::from_utf8(&[hi, lo]).unwrap_or_default(), 16)
            {
                out.push(value as char);
            }
        } else {
            out.push(byte as char);
        }
    }
    out
}

fn json_response(status: u16, payload: serde_json::Value) -> String {
    json_response_with_headers(status, payload, &[])
}

fn json_response_with_headers(
    status: u16,
    payload: serde_json::Value,
    extra_headers: &[(&str, &str)],
) -> String {
    let body = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Internal Server Error",
    };
    let extra = extra_headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect::<String>();
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}",
        body.len()
    )
}

#[cfg(test)]
mod tests {
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
        assert!(
            elapsed < Duration::from_secs(2),
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
}
