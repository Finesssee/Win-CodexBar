//! Remote (SSH / Tailscale) agent-session retrieval and wire decoding.

use super::*;
use futures::future::join_all;

impl RemoteSessionFetcher {
    const BUNDLED_CLI_FALLBACK: &'static str =
        "/Applications/CodexBar.app/Contents/Helpers/CodexBarCLI";

    pub fn new(per_host_timeout: Duration) -> Self {
        Self { per_host_timeout }
    }

    pub async fn fetch(&self, hosts: &[String]) -> Vec<AgentSessionHostResult> {
        let valid = Self::sanitized_hosts(hosts);
        let valid_keys = valid
            .iter()
            .map(|host| host.to_ascii_lowercase())
            .collect::<HashSet<_>>();
        let mut invalid = hosts
            .iter()
            .filter(|host| {
                Self::validate_host(host).is_err()
                    && !valid_keys.contains(&host.trim().to_ascii_lowercase())
            })
            .map(|_| {
                AgentSessionHostResult::failed(
                    "<invalid SSH host>",
                    "Invalid SSH host entry; use a host name or user@host without spaces or options.",
                )
            })
            .collect::<Vec<_>>();
        let timeout = self.per_host_timeout;
        let mut results = Self::fetch_hosts_with(&valid, |host| async move {
            Self::fetch_host(host, timeout).await
        })
        .await;
        results.append(&mut invalid);
        results.sort_by(|lhs, rhs| {
            lhs.host
                .to_ascii_lowercase()
                .cmp(&rhs.host.to_ascii_lowercase())
        });
        results
    }

    pub(crate) async fn tailscale_hosts() -> Result<Vec<String>, String> {
        let options = CommandOptions {
            timeout: Duration::from_secs(5),
            initial_delay: Duration::ZERO,
            extra_args: vec!["status".to_string(), "--json".to_string()],
            ..CommandOptions::default()
        };
        match CommandRunner::new().run_async("tailscale", None, &options).await {
            Err(CommandError::BinaryNotFound(_)) => Ok(Vec::new()),
            Err(_) => Err(
                "Unable to query Tailscale peers; manual SSH hosts are still available.".to_string(),
            ),
            Ok(result) if result.exit_code == Some(0) && !result.timed_out => {
                TailscaleStatusParser::hosts(&result.text).map_err(|_| {
                    "Tailscale returned an invalid status response; manual SSH hosts are still available."
                        .to_string()
                })
            }
            Ok(_) => Err(
                "Tailscale status failed; manual SSH hosts are still available.".to_string(),
            ),
        }
    }

    async fn fetch_host(host: String, timeout: Duration) -> AgentSessionHostResult {
        let options = match Self::ssh_options(&host, timeout) {
            Ok(options) => options,
            Err(error) => return AgentSessionHostResult::failed("<invalid SSH host>", error),
        };
        let result = CommandRunner::new().run_async("ssh", None, &options).await;
        match result {
            Ok(result) if result.timed_out => AgentSessionHostResult::failed(
                host,
                "SSH session discovery timed out; verify the host is reachable and key authentication is configured.",
            ),
            Ok(result) if result.exit_code == Some(0) => {
                Self::decode_remote_sessions(&host, &result.text).unwrap_or_else(|error| {
                    AgentSessionHostResult::failed(
                        host,
                        actionable_message(
                            "Remote session response was not valid JSON; update CodexBar on the remote host",
                            error,
                        ),
                    )
                })
            }
            Ok(result) => AgentSessionHostResult::failed(
                host,
                format!(
                    "SSH session discovery failed{}; verify BatchMode key access and the remote codexbar installation.",
                    result
                        .exit_code
                        .map(|code| format!(" with exit code {code}"))
                        .unwrap_or_default()
                ),
            ),
            Err(error) => AgentSessionHostResult::failed(
                host,
                actionable_message(
                    "Unable to start SSH; install the Windows OpenSSH client and verify PATH",
                    error,
                ),
            ),
        }
    }

    pub(crate) fn ssh_options(host: &str, timeout: Duration) -> Result<CommandOptions, String> {
        let host = Self::validate_host(host)?;
        let connect_timeout = timeout.as_secs().clamp(1, 3);
        // Upstream 0.48.0 #2626: negotiate the v2 session JSON (Pi-family
        // included) first, fall back to the legacy v1 array for older hosts.
        let remote_command = remote_sessions_command(Self::BUNDLED_CLI_FALLBACK);
        Ok(CommandOptions {
            timeout,
            initial_delay: Duration::ZERO,
            extra_args: vec![
                "-o".to_string(),
                "BatchMode=yes".to_string(),
                "-o".to_string(),
                format!("ConnectTimeout={connect_timeout}"),
                "--".to_string(),
                host,
                "sh".to_string(),
                "-lc".to_string(),
                remote_command,
            ],
            ..CommandOptions::default()
        })
    }

    pub(crate) async fn fetch_hosts_with<F, Fut>(
        hosts: &[String],
        fetch: F,
    ) -> Vec<AgentSessionHostResult>
    where
        F: Fn(String) -> Fut + Clone,
        Fut: Future<Output = AgentSessionHostResult>,
    {
        let mut results = join_all(hosts.iter().cloned().map(|host| fetch.clone()(host))).await;
        results.sort_by(|lhs, rhs| {
            lhs.host
                .to_ascii_lowercase()
                .cmp(&rhs.host.to_ascii_lowercase())
        });
        results
    }

    fn decode_remote_sessions(host: &str, body: &str) -> Result<AgentSessionHostResult, String> {
        if let Ok(mut sessions) = serde_json::from_str::<Vec<AgentSession>>(body) {
            for session in &mut sessions {
                session.host = host.to_string();
            }
            return Ok(AgentSessionHostResult::success(host, sessions));
        }
        let mut result = Self::decode_host_result(body)?;
        result.host = host.to_string();
        for session in &mut result.sessions {
            session.host = host.to_string();
        }
        Ok(result)
    }

    pub fn sanitized_hosts(hosts: &[String]) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut sanitized = Vec::new();

        for host in hosts {
            let Ok(host) = Self::validate_host(host) else {
                continue;
            };

            let key = host.to_ascii_lowercase();
            if seen.insert(key) {
                sanitized.push(host);
            }
        }

        sanitized
    }

    pub fn merge_hosts(manual: &[String], automatic: &[String]) -> Vec<String> {
        Self::sanitized_hosts(&manual.iter().chain(automatic).cloned().collect::<Vec<_>>())
    }

    pub fn validate_host(host: &str) -> Result<String, String> {
        let host = host.trim();
        if host.is_empty() {
            return Err("host must not be empty".to_string());
        }
        if host.starts_with('-') {
            return Err("host must not start with '-'".to_string());
        }
        if host
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || !is_safe_host_char(c))
        {
            return Err(
                "host must not contain whitespace, control characters, or unsafe shell characters"
                    .to_string(),
            );
        }

        Ok(host.to_string())
    }

    pub fn decode_host_result(body: &str) -> Result<AgentSessionHostResult, String> {
        let result: AgentSessionHostResult = serde_json::from_str(body)
            .map_err(|err| actionable_message("Unable to decode remote session response", err))?;
        Self::validate_host(&result.host).map_err(|err| {
            actionable_message("Remote session response has an invalid host", err)
        })?;
        Ok(result)
    }

    pub fn failed_result(host: &str, err: impl std::fmt::Display) -> AgentSessionHostResult {
        AgentSessionHostResult::failed(host.to_string(), err)
    }
}

impl Default for RemoteSessionFetcher {
    fn default() -> Self {
        Self {
            per_host_timeout: Duration::from_secs(5),
        }
    }
}

/// `--json-v2` (Pi-family aware) first, legacy `--json` for older installs —
/// upstream 0.48.0 #2626 negotiation, including the bundled macOS CLI
/// fallback path.
fn remote_sessions_command(bundled_cli_fallback: &str) -> String {
    [
        "codexbar sessions --json-v2",
        "codexbar sessions --json",
        &format!("'{bundled_cli_fallback}' sessions --json-v2"),
        &format!("'{bundled_cli_fallback}' sessions --json"),
    ]
    .join(" || ")
}
