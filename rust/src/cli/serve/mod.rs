//! Local HTTP server for scriptable usage/cost JSON + the built-in web dashboard.
//!
//! Upstream 0.44 #2227: bind host + optional dashboard bearer token gate.
//! Non-loopback binds require a token and `--allow-plain-http`.
//! Upstream 0.48.0 #2684: the request head is bounded as a whole — 16,384-byte
//! cap and a single 10 s monotonic deadline across ALL reads, enforced before
//! any Host allowlist or bearer handling; over-cap connections close instantly.
//! Upstream 0.48.0 A1-A5: `GET /` serves the embedded web dashboard,
//! `GET /icons/<name>.svg` serves embedded brand icons, and
//! `GET /dashboard/v1/snapshot` serves the stable dashboard-v1 JSON contract
//! behind the same bearer gate + `Cache-Control: no-store` (+ `WWW-Authenticate`
//! on its 401s, per pinned upstream).

pub mod dashboard;
mod data;

use std::sync::Arc;
use std::time::Duration;

use clap::Args;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

use dashboard::snapshot::DashboardIdentity;

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

    /// Dashboard snapshot identity detail: redacted (default) or full. `full`
    /// exposes real account emails to every authorized dashboard client.
    #[arg(long, value_parser = ["redacted", "full"], default_value = "redacted")]
    pub identity: String,
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
    /// Dashboard snapshot identity mode (`redacted` default, `full` opt-in).
    identity: DashboardIdentity,
    /// Dashboard state (coordinator + producer). Always `Some` from `run`;
    /// `None` only in pure-transport tests, where dashboard routes answer 503.
    dashboard: Option<dashboard::DashboardState>,
}

pub async fn run(args: ServeArgs) -> anyhow::Result<()> {
    let mut config = validate_serve_args(&args)?;
    config.dashboard = Some(dashboard::DashboardState::live(
        args.refresh_interval.max(1) as u32,
        config.identity,
    ));
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

    // clap's value_parser already rejects anything but redacted|full.
    let Some(identity) = DashboardIdentity::parse(&args.identity) else {
        anyhow::bail!("--identity must be redacted or full.");
    };

    let token = resolve_dashboard_token(args.dashboard_token.as_deref())?;
    if let Some(err) = validate_serve_startup(&host, token.is_some(), args.allow_plain_http) {
        anyhow::bail!("{err}");
    }

    Ok(ServeConfig {
        host,
        port: args.port,
        token_digest: token.as_ref().map(|t| sha256_digest(t.as_bytes())),
        head_read_budget: HEAD_READ_TIMEOUT,
        identity,
        dashboard: None,
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

/// Strongly typed route table for the serve surface.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ServeRoute {
    /// `GET /` — embedded web dashboard shell.
    DashboardHome,
    /// `GET /icons/<name>.svg` — embedded brand icon.
    ProviderIcon {
        name: String,
    },
    Health,
    Usage {
        provider: Option<String>,
    },
    Cost {
        provider: Option<String>,
    },
    /// `GET /dashboard/v1/snapshot` — stable dashboard-v1 JSON contract.
    DashboardSnapshot,
}

fn resolve_route(request: &ServeRequest) -> Option<ServeRoute> {
    let provider = request.query.get("provider").cloned();
    match request.path.as_str() {
        "/" => Some(ServeRoute::DashboardHome),
        "/health" => Some(ServeRoute::Health),
        "/usage" => Some(ServeRoute::Usage { provider }),
        "/cost" => Some(ServeRoute::Cost { provider }),
        "/dashboard/v1/snapshot" => Some(ServeRoute::DashboardSnapshot),
        path if path.starts_with("/icons/") && path.ends_with(".svg") => {
            let name = &path["/icons/".len()..path.len() - ".svg".len()];
            // Resource names are flat; separators or an empty stem never resolve.
            if name.is_empty() || name.contains('/') || name.contains('\\') {
                return None;
            }
            Some(ServeRoute::ProviderIcon {
                name: name.to_string(),
            })
        }
        _ => None,
    }
}

fn unauthorized_response() -> String {
    json_response(401, serde_json::json!({ "error": "unauthorized" }))
}

/// Upstream: dashboard-route 401s advertise the bearer scheme.
fn unauthorized_dashboard_response() -> String {
    json_response_with_headers(
        401,
        serde_json::json!({ "error": "unauthorized" }),
        &[("WWW-Authenticate", "Bearer")],
    )
}

async fn route_request(request: &ServeRequest, config: &ServeConfig) -> String {
    if request.method != "GET" {
        return json_response(405, serde_json::json!({ "error": "method not allowed" }));
    }
    if !allowed_host(&request.host, &config.host) {
        return json_response(403, serde_json::json!({ "error": "forbidden host" }));
    }

    let Some(route) = resolve_route(request) else {
        return json_response(404, serde_json::json!({ "error": "not found" }));
    };

    match route {
        ServeRoute::DashboardHome => match &config.dashboard {
            Some(state) => dashboard::home_response(state),
            None => json_response(503, serde_json::json!({ "error": "dashboard unavailable" })),
        },
        ServeRoute::ProviderIcon { name } => dashboard::icon_response(&name),
        ServeRoute::Health => json_response(
            200,
            serde_json::json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }),
        ),
        ServeRoute::Usage { provider } => {
            if !authorize_request(
                request.authorization.as_deref(),
                config.token_digest.as_ref(),
            ) {
                return unauthorized_response();
            }
            data::usage_response(provider.as_deref()).await
        }
        ServeRoute::Cost { provider } => {
            if !authorize_request(
                request.authorization.as_deref(),
                config.token_digest.as_ref(),
            ) {
                return unauthorized_response();
            }
            data::cost_response(provider.as_deref()).await
        }
        ServeRoute::DashboardSnapshot => {
            if !authorize_request(
                request.authorization.as_deref(),
                config.token_digest.as_ref(),
            ) {
                return unauthorized_dashboard_response();
            }
            match &config.dashboard {
                Some(state) => dashboard::snapshot_response(state).await,
                None => json_response(
                    500,
                    serde_json::json!({ "error": "dashboard not configured" }),
                ),
            }
        }
    }
}

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
    http_response(
        status,
        "application/json; charset=utf-8",
        body,
        extra_headers,
    )
}

/// Single writer for every serve response: status line, content type, exact
/// content length, optional extra headers, `Connection: close`.
fn http_response(
    status: u16,
    content_type: &str,
    body: String,
    extra_headers: &[(&str, &str)],
) -> String {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    };
    let extra = extra_headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect::<String>();
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}",
        body.len()
    )
}

#[cfg(test)]
mod tests;
