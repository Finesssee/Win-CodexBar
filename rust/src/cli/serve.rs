//! Local HTTP server for scriptable usage/cost JSON.
//!
//! Upstream 0.44 #2227: bind host + optional dashboard bearer token gate.
//! Non-loopback binds require a token and `--allow-plain-http`.

use clap::Args;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::usage::ProviderSelection;
use crate::core::{CostScanOptions, FetchContext, ProviderId, SourceMode, instantiate_provider};
use crate::cost_scanner::CostScanner;

const DASHBOARD_TOKEN_ENV: &str = "CODEXBAR_DASHBOARD_TOKEN";

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

    loop {
        let (stream, _) = listener.accept().await?;
        let config = config.clone();
        tokio::spawn(async move {
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
    let mut buffer = vec![0_u8; 8192];
    let n = stream.read(&mut buffer).await?;
    let request = String::from_utf8_lossy(&buffer[..n]);
    let response = match parse_request(&request) {
        Ok(request) => route_request(&request, config).await,
        Err(status) => json_response(status, serde_json::json!({ "error": "bad request" })),
    };
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
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
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
}
