//! Command Runner
//!
//! Executes CLI commands with output capture.
//! On Windows, uses standard process spawning with output capture.
//! Designed for running interactive CLI tools like `codex` and `claude`.

#![allow(
    dead_code,
    reason = "command runner types reserved for future host management integration"
)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Command runner configuration
#[derive(Debug, Clone)]
pub struct CommandOptions {
    /// Number of rows for terminal (advisory)
    pub rows: u16,
    /// Number of columns for terminal (advisory)
    pub cols: u16,
    /// Command timeout
    pub timeout: Duration,
    /// Stop early after this idle time
    pub idle_timeout: Option<Duration>,
    /// Working directory
    pub working_directory: Option<PathBuf>,
    /// Extra arguments to pass
    pub extra_args: Vec<String>,
    /// Initial delay before sending input
    pub initial_delay: Duration,
    /// Send enter key every N seconds
    pub send_enter_every: Option<Duration>,
    /// Send specific input when substring is seen
    pub send_on_substrings: HashMap<String, String>,
    /// Stop when URL is detected
    pub stop_on_url: bool,
    /// Stop when any of these substrings are seen
    pub stop_on_substrings: Vec<String>,
    /// Time to wait after stop condition before returning
    pub settle_after_stop: Duration,
}

impl Default for CommandOptions {
    fn default() -> Self {
        Self {
            rows: 50,
            cols: 160,
            timeout: Duration::from_secs(20),
            idle_timeout: None,
            working_directory: None,
            extra_args: Vec::new(),
            initial_delay: Duration::from_millis(400),
            send_enter_every: None,
            send_on_substrings: HashMap::new(),
            stop_on_url: false,
            stop_on_substrings: Vec::new(),
            settle_after_stop: Duration::from_millis(250),
        }
    }
}

/// Result of running a command
#[derive(Debug, Clone)]
pub struct CommandResult {
    /// Captured output text (stdout)
    pub text: String,
    /// Captured stderr text
    pub stderr: String,
    /// Whether the command timed out
    pub timed_out: bool,
    /// Exit code if available
    pub exit_code: Option<i32>,
}

/// Command runner errors
#[derive(Debug, Clone)]
pub enum CommandError {
    /// Binary not found in PATH
    BinaryNotFound(String),
    /// Failed to launch process
    LaunchFailed(String),
    /// Command timed out
    TimedOut,
    /// IO error
    IoError(String),
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandError::BinaryNotFound(bin) => {
                write!(f, "Binary '{}' not found. Install it or add to PATH.", bin)
            }
            CommandError::LaunchFailed(msg) => write!(f, "Failed to launch process: {}", msg),
            CommandError::TimedOut => write!(f, "Command timed out"),
            CommandError::IoError(msg) => write!(f, "IO error: {}", msg),
        }
    }
}

impl std::error::Error for CommandError {}

/// Command runner for executing CLI tools
pub struct CommandRunner {
    /// Environment variables to add.
    env_additions: HashMap<String, String>,
    /// Whether the child inherits the ambient process environment.
    inherit_environment: bool,
}

impl CommandRunner {
    /// Upper bound for a single captured line fed into user-facing tails.
    pub const MAX_LINE_CHARS: usize = 200;
    const MAX_CAPTURE_BYTES: usize = 1024 * 1024;

    pub fn new() -> Self {
        Self {
            env_additions: HashMap::new(),
            inherit_environment: true,
        }
    }

    /// Add an environment variable.
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env_additions.insert(key.into(), value.into());
        self
    }

    /// Spawn the child from only explicitly supplied variables.
    ///
    /// This is for credential-sensitive passive probes that must not inherit
    /// unrelated API keys, cookies, cloud credentials, or agent sockets.
    pub fn without_inherited_env(mut self) -> Self {
        self.inherit_environment = false;
        self
    }

    /// Find a binary in PATH
    pub fn which(binary: &str) -> Option<PathBuf> {
        which::which(binary).ok()
    }

    fn is_explicit_binary_path(binary: &str) -> bool {
        let path = std::path::Path::new(binary);
        path.is_absolute() || path.components().count() > 1
    }

    /// Run a command and capture output
    pub fn run(
        &self,
        binary: &str,
        input: Option<&str>,
        options: &CommandOptions,
    ) -> Result<CommandResult, CommandError> {
        let binary_path = Self::resolve_binary(binary)?;
        let mut child = self.spawn_child(&binary_path, options)?;

        let start = Instant::now();
        let deadline = start + options.timeout;

        Self::send_initial_input(&mut child, input, options.initial_delay, deadline);

        // Capture output
        let (output, stderr, timed_out) = match self.capture_output(&mut child, options, deadline) {
            Ok(captured) => captured,
            Err(e) => {
                let _exit_code = Self::terminate_child(&mut child);
                return Err(e);
            }
        };

        let exit_code = if timed_out {
            Self::terminate_child(&mut child)
        } else {
            Self::finish_child_after_eof(&mut child)
        };

        Ok(CommandResult {
            text: output,
            stderr,
            timed_out,
            exit_code,
        })
    }

    fn resolve_binary(binary: &str) -> Result<PathBuf, CommandError> {
        if Self::is_explicit_binary_path(binary) {
            return Self::resolve_explicit_binary(binary);
        }

        Self::which(binary).ok_or_else(|| CommandError::BinaryNotFound(binary.to_string()))
    }

    fn resolve_explicit_binary(binary: &str) -> Result<PathBuf, CommandError> {
        let path = PathBuf::from(binary);
        if path.exists() {
            Ok(path)
        } else {
            Err(CommandError::BinaryNotFound(binary.to_string()))
        }
    }

    fn spawn_child(
        &self,
        binary_path: &PathBuf,
        options: &CommandOptions,
    ) -> Result<Child, CommandError> {
        let mut cmd = Command::new(binary_path);
        Self::configure_command_args(&mut cmd, options);
        self.configure_command_environment(&mut cmd);
        Self::configure_command_stdio(&mut cmd);
        Self::hide_windows_console(&mut cmd);

        cmd.spawn()
            .map_err(|e| CommandError::LaunchFailed(e.to_string()))
    }

    fn configure_command_args(cmd: &mut Command, options: &CommandOptions) {
        cmd.args(&options.extra_args);

        if let Some(dir) = &options.working_directory {
            cmd.current_dir(dir);
        }
    }

    fn configure_command_environment(&self, cmd: &mut Command) {
        let mut env = if self.inherit_environment {
            std::env::vars().collect::<HashMap<_, _>>()
        } else {
            cmd.env_clear();
            HashMap::new()
        };
        env.extend(self.env_additions.clone());
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());

        cmd.envs(env);
    }

    fn configure_command_stdio(cmd: &mut Command) {
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
    }

    #[cfg(windows)]
    fn hide_windows_console(cmd: &mut Command) {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    #[cfg(not(windows))]
    fn hide_windows_console(_cmd: &mut Command) {}

    fn send_initial_input(
        child: &mut Child,
        input: Option<&str>,
        initial_delay: Duration,
        deadline: Instant,
    ) {
        let Some(input_text) = input else {
            return;
        };
        let Some(mut stdin) = child.stdin.take() else {
            return;
        };

        use std::io::Write;
        std::thread::sleep(initial_delay.min(deadline.saturating_duration_since(Instant::now())));
        if Self::past_deadline(deadline) {
            return;
        }
        // Fire-and-forget seeding: the child may close stdin before we finish
        // writing, and a write error there must not abort output capture.
        let _written_input = stdin.write_all(input_text.as_bytes());
        let _written_newline = stdin.write_all(b"\n");
        let _flushed_stdin = stdin.flush();
    }

    /// Grace period between stdout/stderr EOF and process-object exit.
    ///
    /// Windows is the motivating case: PowerShell tears down its console
    /// handles slightly before the process object transitions to signaled, so
    /// a single `try_wait` right after capture sees `Ok(None)` even though the
    /// process exited successfully shortly afterward. Hosted Windows runners
    /// have occasionally exceeded 250 ms here; keep the wait bounded but long
    /// enough to preserve the real exit code instead of killing a clean exit.
    const EXIT_GRACE_PERIOD: Duration = Duration::from_secs(1);

    fn finish_child_after_eof(child: &mut Child) -> Option<i32> {
        let mut remaining = Self::EXIT_GRACE_PERIOD;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return status.code(),
                Ok(None) => {}
                Err(_) => break,
            }
            if remaining.is_zero() {
                break;
            }
            let step = remaining.min(Duration::from_millis(10));
            std::thread::sleep(step);
            remaining -= step;
        }
        Self::terminate_child(child)
    }

    /// Terminate a command whose capture ended because of timeout/error.
    /// Do not apply the clean-EOF grace here: the command has already exceeded
    /// its execution contract and should be torn down immediately.
    fn terminate_child(child: &mut Child) -> Option<i32> {
        let killed = child.kill().is_ok();
        let reaped = child.wait().ok();
        if killed {
            return None;
        }
        reaped.and_then(|status| status.code())
    }

    /// Capture output from a running process
    ///
    /// Returns `(stdout, stderr, timed_out)`. Stderr lines are kept in a
    /// separate buffer (bounded like stdout) so callers can surface real
    /// diagnostics; they are never folded into the stdout `text` field.
    fn capture_output(
        &self,
        child: &mut Child,
        options: &CommandOptions,
        deadline: Instant,
    ) -> Result<(String, String, bool), CommandError> {
        let mut output = String::new();
        let mut stderr_output = String::new();
        let mut last_output_time = Instant::now();
        let (sender, receiver) = mpsc::channel();
        Self::read_stream(Self::stdout_reader(child)?, sender.clone(), true);
        Self::read_stream(Self::stderr_reader(child)?, sender, false);
        let mut closed_streams = 0;

        loop {
            if Self::past_deadline(deadline) {
                return Ok((output, stderr_output, true));
            }
            if Self::idle_timed_out(options.idle_timeout, last_output_time) {
                break;
            }

            let wait = Self::next_wait(deadline, options.idle_timeout, last_output_time);
            match receiver.recv_timeout(wait) {
                Ok(StreamEvent::Line { text, capture }) => {
                    last_output_time = Instant::now();
                    if !capture {
                        Self::append_output_line(&mut stderr_output, &text);
                        continue;
                    }
                    Self::append_output_line(&mut output, &text);
                    if Self::should_stop_after_line(&text, options) {
                        std::thread::sleep(
                            options
                                .settle_after_stop
                                .min(deadline.saturating_duration_since(Instant::now())),
                        );
                        break;
                    }
                }
                Ok(StreamEvent::Closed) => {
                    closed_streams += 1;
                    if closed_streams == 2 {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }

        Ok((output, stderr_output, false))
    }

    fn stdout_reader(child: &mut Child) -> Result<BufReader<ChildStdout>, CommandError> {
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CommandError::IoError("Failed to capture stdout".to_string()))?;

        Ok(BufReader::new(stdout))
    }

    fn stderr_reader(child: &mut Child) -> Result<BufReader<ChildStderr>, CommandError> {
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| CommandError::IoError("Failed to capture stderr".to_string()))?;

        Ok(BufReader::new(stderr))
    }

    fn read_stream(
        reader: BufReader<impl std::io::Read + Send + 'static>,
        sender: mpsc::Sender<StreamEvent>,
        capture: bool,
    ) {
        std::thread::spawn(move || {
            for line in reader.lines().map_while(Result::ok) {
                if sender
                    .send(StreamEvent::Line {
                        text: line,
                        capture,
                    })
                    .is_err()
                {
                    return;
                }
            }
            let _sent_closed = sender.send(StreamEvent::Closed);
        });
    }

    fn next_wait(
        deadline: Instant,
        idle_timeout: Option<Duration>,
        last_output_time: Instant,
    ) -> Duration {
        let deadline_wait = deadline.saturating_duration_since(Instant::now());
        let idle_wait = idle_timeout
            .map(|timeout| timeout.saturating_sub(last_output_time.elapsed()))
            .unwrap_or(deadline_wait);
        deadline_wait.min(idle_wait).min(Duration::from_millis(25))
    }

    fn past_deadline(deadline: Instant) -> bool {
        Instant::now() >= deadline
    }

    fn append_output_line(output: &mut String, line: &str) {
        if output.len() >= Self::MAX_CAPTURE_BYTES {
            return;
        }
        let remaining = Self::MAX_CAPTURE_BYTES - output.len();
        let end = line
            .char_indices()
            .take_while(|(index, _)| *index < remaining)
            .map(|(index, character)| index + character.len_utf8())
            .last()
            .unwrap_or(0);
        output.push_str(&line[..end]);
        if output.len() >= Self::MAX_CAPTURE_BYTES {
            return;
        }
        output.push('\n');
    }

    fn should_stop_after_line(line: &str, options: &CommandOptions) -> bool {
        Self::line_has_stop_url(line, options.stop_on_url)
            || Self::line_has_stop_substring(line, &options.stop_on_substrings)
    }

    fn line_has_stop_url(line: &str, stop_on_url: bool) -> bool {
        stop_on_url && (line.contains("https://") || line.contains("http://"))
    }

    fn line_has_stop_substring(line: &str, stop_substrings: &[String]) -> bool {
        stop_substrings
            .iter()
            .any(|stop_substr| line.contains(stop_substr))
    }

    fn idle_timed_out(idle_timeout: Option<Duration>, last_output_time: Instant) -> bool {
        idle_timeout.is_some_and(|idle_timeout| last_output_time.elapsed() > idle_timeout)
    }

    /// Run a command asynchronously
    pub async fn run_async(
        &self,
        binary: &str,
        input: Option<&str>,
        options: &CommandOptions,
    ) -> Result<CommandResult, CommandError> {
        let binary = binary.to_string();
        let input = input.map(|s| s.to_string());
        let options = options.clone();
        let env = self.env_additions.clone();
        let inherit_environment = self.inherit_environment;

        tokio::task::spawn_blocking(move || {
            let runner = CommandRunner {
                env_additions: env,
                inherit_environment,
            };
            runner.run(&binary, input.as_deref(), &options)
        })
        .await
        .map_err(|e| CommandError::LaunchFailed(e.to_string()))?
    }
}

impl Default for CommandRunner {
    fn default() -> Self {
        Self::new()
    }
}

enum StreamEvent {
    Line { text: String, capture: bool },
    Closed,
}

/// Rolling buffer for substring matching across chunk boundaries
pub struct RollingBuffer {
    max_needle_len: usize,
    tail: Vec<u8>,
}

impl RollingBuffer {
    pub fn new(max_needle_len: usize) -> Self {
        Self {
            max_needle_len,
            tail: Vec::with_capacity(max_needle_len),
        }
    }

    /// Append data and return the combined buffer for searching
    pub fn append(&mut self, data: &[u8]) -> Vec<u8> {
        if data.is_empty() {
            return Vec::new();
        }

        let mut combined = Vec::with_capacity(self.tail.len() + data.len());
        combined.extend_from_slice(&self.tail);
        combined.extend_from_slice(data);

        // Keep only the tail for next search
        if self.max_needle_len > 1 && combined.len() >= self.max_needle_len - 1 {
            self.tail = combined[combined.len() - (self.max_needle_len - 1)..].to_vec();
        } else {
            self.tail = combined.clone();
        }

        combined
    }

    /// Reset the buffer
    pub fn reset(&mut self) {
        self.tail.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_options_default() {
        let opts = CommandOptions::default();
        assert_eq!(opts.rows, 50);
        assert_eq!(opts.cols, 160);
        assert_eq!(opts.timeout, Duration::from_secs(20));
    }

    #[test]
    fn test_rolling_buffer() {
        let mut buf = RollingBuffer::new(5);

        let result = buf.append(b"hello");
        assert_eq!(result, b"hello");

        let result = buf.append(b" world");
        // Should include tail from previous
        assert!(result.len() > 6);
    }

    #[test]
    fn test_command_runner_new() {
        let runner = CommandRunner::new();
        assert!(runner.env_additions.is_empty());
        assert!(runner.inherit_environment);
    }

    #[test]
    fn test_command_runner_with_env() {
        let runner = CommandRunner::new()
            .with_env("FOO", "bar")
            .with_env("BAZ", "qux");

        assert_eq!(runner.env_additions.get("FOO"), Some(&"bar".to_string()));
        assert_eq!(runner.env_additions.get("BAZ"), Some(&"qux".to_string()));
    }

    #[test]
    fn clean_environment_is_opt_in() {
        let runner = CommandRunner::new()
            .without_inherited_env()
            .with_env("PATH", "fixture");
        assert!(!runner.inherit_environment);
        assert_eq!(
            runner.env_additions.get("PATH"),
            Some(&"fixture".to_string())
        );
    }

    #[test]
    fn captured_output_is_bounded() {
        let mut output = String::new();
        let line = "x".repeat(CommandRunner::MAX_CAPTURE_BYTES + 10);
        CommandRunner::append_output_line(&mut output, &line);
        assert_eq!(output.len(), CommandRunner::MAX_CAPTURE_BYTES);
    }

    #[cfg(windows)]
    #[test]
    fn command_timeout_is_a_wall_clock_bound_even_without_output() {
        let runner = CommandRunner::new();
        let options = CommandOptions {
            timeout: Duration::from_millis(100),
            initial_delay: Duration::ZERO,
            extra_args: vec![
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-Command".to_string(),
                "Start-Sleep -Seconds 30".to_string(),
            ],
            ..CommandOptions::default()
        };
        let started = Instant::now();

        let result = runner.run("powershell.exe", None, &options).unwrap();

        assert!(result.timed_out);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timeout took {:?}",
            started.elapsed()
        );
    }

    #[cfg(windows)]
    #[test]
    fn large_capture_is_not_truncated() {
        let runner = CommandRunner::new();
        let options = CommandOptions {
            initial_delay: Duration::ZERO,
            extra_args: vec![
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-Command".to_string(),
                "$big = 'A' * 90000; Write-Output $big".to_string(),
            ],
            ..CommandOptions::default()
        };

        let result = runner.run("powershell.exe", None, &options).unwrap();

        assert!(!result.timed_out);
        assert_eq!(result.exit_code, Some(0));
        assert!(
            result.text.len() >= 90_000,
            "expected >= 90000 bytes, got {}",
            result.text.len()
        );
    }

    #[cfg(windows)]
    #[test]
    fn stderr_is_captured_separately_from_stdout() {
        let runner = CommandRunner::new();
        let options = CommandOptions {
            initial_delay: Duration::ZERO,
            extra_args: vec![
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-Command".to_string(),
                concat!(
                    "$ErrorActionPreference='Stop';",
                    "Write-Output 'stdout line';",
                    "Write-Error 'boom diagnostics'"
                )
                .to_string(),
            ],
            ..CommandOptions::default()
        };

        let result = runner.run("powershell.exe", None, &options).unwrap();

        assert_eq!(result.exit_code, Some(1));
        assert!(
            result.text.contains("stdout line"),
            "stdout: {}",
            result.text
        );
        assert!(
            result.stderr.contains("boom diagnostics"),
            "stderr: {}",
            result.stderr
        );
        // PowerShell's error banner echoes the whole script line into
        // stderr, so asserting stderr lacks "stdout line" would be wrong;
        // instead assert stderr diagnostics never leaked into stdout.
        assert!(
            !result.text.contains("boom diagnostics"),
            "stderr leaked into stdout: {}",
            result.text
        );
    }

    #[test]
    fn test_error_display() {
        let err = CommandError::BinaryNotFound("codex".to_string());
        assert!(err.to_string().contains("codex"));

        let err = CommandError::TimedOut;
        assert!(err.to_string().contains("timed out"));
    }
}
