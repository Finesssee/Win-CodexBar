//! `codexbar workspaces` — list local Codex project usage.

use clap::{Args, Subcommand};
use serde::Serialize;

use crate::codex_workspaces::{CodexWorkspacesIndex, ProgressPhase};

#[derive(Args, Debug, Default)]
pub struct WorkspacesArgs {
    #[command(subcommand)]
    pub command: Option<WorkspacesCommand>,

    /// Shorthand for list --json
    #[arg(long, global = true)]
    pub json: bool,

    /// Pretty-print JSON
    #[arg(long, global = true)]
    pub pretty: bool,

    /// History window in days (default 30)
    #[arg(short, long, default_value = "30", global = true)]
    pub days: u32,

    /// Force a full re-index even when a cached snapshot exists
    #[arg(long, global = true)]
    pub force: bool,
}

#[derive(Subcommand, Debug)]
pub enum WorkspacesCommand {
    /// List projects (default)
    List(WorkspacesListArgs),
}

#[derive(Args, Debug, Default)]
pub struct WorkspacesListArgs {}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectRow {
    name: String,
    sessions: u32,
    cost_usd: f64,
    unknown_tokens: u64,
    latest_activity: Option<String>,
    top_model: Option<String>,
    path: Option<String>,
}

pub async fn run(args: WorkspacesArgs) -> anyhow::Result<()> {
    match args.command {
        None | Some(WorkspacesCommand::List(_)) => {}
    }

    let index = CodexWorkspacesIndex::new(args.days);
    let snapshot = index.load_snapshot(args.force, |p| {
        if matches!(
            p.phase,
            ProgressPhase::ScanningLogs | ProgressPhase::IndexingProjects
        ) {
            tracing::debug!(
                phase = ?p.phase,
                processed = ?p.processed_file_count,
                total = ?p.total_file_count,
                "workspaces index progress"
            );
        }
    })?;

    if args.json {
        if args.pretty {
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
        } else {
            println!("{}", serde_json::to_string(&snapshot)?);
        }
        return Ok(());
    }

    println!(
        "Codex workspaces ({} day{}, {} projects, status={:?})",
        snapshot.history_days,
        if snapshot.history_days == 1 { "" } else { "s" },
        snapshot.projects.len(),
        snapshot.source_status
    );
    println!(
        "Indexed {} file(s), skipped {}.",
        snapshot.indexed_file_count, snapshot.skipped_file_count
    );
    println!();
    println!(
        "{:<28} {:>8} {:>10} {:>20} MODEL",
        "PROJECT", "SESSIONS", "COST", "LATEST"
    );

    if snapshot.projects.is_empty() {
        println!("(no projects)");
        return Ok(());
    }

    for project in &snapshot.projects {
        let cost = format!("${:.4}", project.cost_estimate.known_usd);
        let latest = project
            .latest_activity
            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "-".into());
        let model = project.top_model.as_deref().unwrap_or("-");
        println!(
            "{:<28} {:>8} {:>10} {:>20} {}",
            truncate(&project.display_name, 28),
            project.session_count,
            cost,
            latest,
            model
        );
        let _ = ProjectRow {
            name: project.display_name.clone(),
            sessions: project.session_count,
            cost_usd: project.cost_estimate.known_usd,
            unknown_tokens: project.cost_estimate.unknown_tokens,
            latest_activity: project.latest_activity.map(|t| t.to_rfc3339()),
            top_model: project.top_model.clone(),
            path: project.path.clone(),
        };
    }

    println!();
    println!(
        "Total tokens: {} (in {} / out {} / cached {})",
        snapshot.total.total_tokens,
        snapshot.total.input_tokens,
        snapshot.total.output_tokens,
        snapshot.total.cached_input_tokens
    );
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
