use clap::Parser;
use codexbar::{
    cli::{self, Cli, Commands, exit_codes},
    logging, wsl,
};
use std::future::Future;
use std::path::{Path, PathBuf};
use tokio::runtime::Runtime;

fn launch_log_path() -> PathBuf {
    std::env::temp_dir().join(format!("codexbar_launch_{}.log", std::process::id()))
}

fn append_launch_log(log_path: &Path, message: &str) {
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all(message.as_bytes())
        });
}

fn launch_arg_summary() -> String {
    let arg_count = std::env::args().count().saturating_sub(1);
    format!("{} CLI argument value(s) omitted", arg_count)
}

fn main() {
    let log_path = launch_log_path();
    append_launch_log(
        &log_path,
        &format!(
            "main() started at {:?}\nArgs: {:?}\n",
            std::time::SystemTime::now(),
            launch_arg_summary()
        ),
    );

    let exit_code = run(&log_path);

    append_launch_log(&log_path, &format!("Exiting with code: {}\n", exit_code));

    std::process::exit(exit_code);
}

fn run(log_path: &Path) -> i32 {
    append_launch_log(log_path, &startup_log());

    let cli = Cli::parse();

    if let Err(e) = logging::init(cli.verbose, cli.json_output) {
        eprintln!("Failed to initialize logging: {}", e);
        return exit_codes::UNEXPECTED_FAILURE;
    }

    let rt = match Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("Failed to create runtime: {}", e);
            return exit_codes::UNEXPECTED_FAILURE;
        }
    };

    dispatch_command(&rt, cli.command)
}

fn startup_log() -> String {
    let mut log = format!("Starting at {:?}\n", std::time::SystemTime::now());

    if let Some(wsl_log) = wsl_log() {
        log.push_str(&wsl_log);
    }

    log.push_str(&format!("Args: {:?}\n", launch_arg_summary()));
    log
}

fn wsl_log() -> Option<String> {
    if !wsl::is_wsl() {
        return None;
    }

    let mut log = "Running inside WSL\n".to_string();
    if let Some(info) = wsl::get_wsl_info() {
        log.push_str(&format!("  Distro: {}\n", info.distro_name));
        log.push_str(&format!("  Drive mount: {:?}\n", info.drive_mount));
    }

    Some(log)
}

fn dispatch_command(rt: &Runtime, command: Option<Commands>) -> i32 {
    match command {
        Some(Commands::Usage(args)) => run_categorized(rt, cli::usage::run(args)),
        Some(Commands::Cost(args)) => run_categorized(rt, cli::cost::run(args)),
        Some(Commands::Guard(args)) => rt.block_on(cli::guard::run(args)),
        Some(Commands::Diagnose(args)) => run_categorized(rt, cli::diagnose::run(args)),
        Some(Commands::Sessions(args)) => run_categorized(rt, cli::sessions::run(args)),
        Some(Commands::Serve(args)) => run_unexpected(rt, cli::serve::run(args)),
        Some(Commands::Dashboard(args)) => run_unexpected(rt, cli::dashboard::run(args)),
        Some(Commands::Autostart(args)) => run_unexpected(rt, cli::autostart::run(args)),
        Some(Commands::Account(args)) => run_unexpected(rt, cli::account::run(args)),
        Some(Commands::Config(args)) => run_unexpected(rt, cli::config::run(args)),
        Some(Commands::Hooks(args)) => run_unexpected(rt, cli::hooks::run(args)),
        Some(Commands::Workspaces(args)) => run_categorized(rt, cli::workspaces::run(args)),
        None => missing_subcommand(),
    }
}

fn run_categorized<F>(rt: &Runtime, future: F) -> i32
where
    F: Future<Output = anyhow::Result<()>>,
{
    run_command(rt, future, categorize_error)
}

fn run_unexpected<F>(rt: &Runtime, future: F) -> i32
where
    F: Future<Output = anyhow::Result<()>>,
{
    run_command(rt, future, |_| exit_codes::UNEXPECTED_FAILURE)
}

fn run_command<F>(rt: &Runtime, future: F, error_code: fn(&anyhow::Error) -> i32) -> i32
where
    F: Future<Output = anyhow::Result<()>>,
{
    match rt.block_on(future) {
        Ok(()) => exit_codes::SUCCESS,
        Err(e) => {
            eprintln!("Error: {}", e);
            error_code(&e)
        }
    }
}

fn missing_subcommand() -> i32 {
    // The egui menubar shell has been retired; the desktop UI lives in
    // apps/desktop-tauri. The CLI binary now requires an explicit subcommand.
    eprintln!(
        "codexbar is now CLI-only. Run a subcommand (e.g. `codexbar usage -p claude`) \
         or launch the Tauri desktop shell via `apps/desktop-tauri`.\n\
         Use `codexbar --help` for the full list of subcommands."
    );
    exit_codes::USAGE_ERROR
}

/// Categorize an error into the appropriate exit code
fn categorize_error(e: &anyhow::Error) -> i32 {
    let msg = e.to_string().to_lowercase();

    if msg.contains("not installed") || msg.contains("not found") || msg.contains("binary") {
        exit_codes::PROVIDER_MISSING
    } else if msg.contains("parse") || msg.contains("format") || msg.contains("invalid") {
        exit_codes::PARSE_ERROR
    } else if msg.contains("timeout") || msg.contains("timed out") {
        exit_codes::CLI_TIMEOUT
    } else {
        exit_codes::UNEXPECTED_FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_log_path_is_process_scoped() {
        let path = launch_log_path();
        let file_name = path.file_name().and_then(|name| name.to_str()).unwrap();

        assert!(file_name.starts_with("codexbar_launch_"));
        assert!(file_name.ends_with(".log"));
        assert!(file_name.contains(&std::process::id().to_string()));
    }
}
