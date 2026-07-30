//! Cost command implementation
//!
//! Scans local JSONL logs to calculate token costs for Codex and Claude.

use clap::Args;

use super::usage::{OutputFormat, ProviderSelection};
use crate::core::{CostScanOptions, ProviderId};
use crate::cost_scanner::{CostScanner, CostSummary};

/// Arguments for the cost command
#[derive(Args, Debug, Default)]
pub struct CostArgs {
    /// Provider to query (codex, claude, cursor, gemini, copilot, all, both)
    #[arg(short, long)]
    pub provider: Option<String>,

    /// Output format: text or json
    #[arg(short, long, default_value = "text")]
    pub format: OutputFormat,

    /// Shorthand for --format json
    #[arg(long)]
    pub json: bool,

    /// Disable ANSI colors in text output
    #[arg(long = "no-color")]
    pub no_color: bool,

    /// Pretty-print JSON output
    #[arg(long)]
    pub pretty: bool,

    /// Number of days to scan (default: 30)
    #[arg(short, long, default_value = "30")]
    pub days: u32,
}

/// Run the cost command
pub async fn run(args: CostArgs) -> anyhow::Result<()> {
    let format = if args.json {
        OutputFormat::Json
    } else {
        args.format
    };

    let providers = ProviderSelection::from_arg(args.provider.as_deref())?;
    let use_color = !args.no_color && is_terminal();
    let scanner = CostScanner::new(args.days).with_options(CostScanOptions::app_driven());

    tracing::debug!(
        "Running cost command: providers={:?}, format={:?}, days={}",
        providers.as_list(),
        format,
        args.days
    );

    // Collect cost data for requested providers
    let mut results: Vec<CostResult> = Vec::new();

    for provider in providers.as_list() {
        match provider {
            ProviderId::Codex => {
                let summary = scanner.scan_codex();
                results.push(CostResult {
                    provider: provider.cli_name().to_string(),
                    display_name: provider.display_name().to_string(),
                    summary,
                    supported: true,
                });
            }
            ProviderId::Claude => {
                let summary = scanner.scan_claude();
                results.push(CostResult {
                    provider: provider.cli_name().to_string(),
                    display_name: provider.display_name().to_string(),
                    summary,
                    supported: true,
                });
            }
            _ => {
                // Other providers don't have local logs to scan
                results.push(CostResult {
                    provider: provider.cli_name().to_string(),
                    display_name: provider.display_name().to_string(),
                    summary: CostSummary::default(),
                    supported: false,
                });
            }
        }
    }

    match format {
        OutputFormat::Text => {
            print_text_output(&results, use_color, args.days);
        }
        OutputFormat::Json => {
            print_json_output(&results, args.pretty, args.days)?;
        }
    }

    Ok(())
}

/// Cost result for a provider
struct CostResult {
    provider: String,
    display_name: String,
    summary: CostSummary,
    supported: bool,
}

/// Print text output
fn print_text_output(results: &[CostResult], use_color: bool, days: u32) {
    for (i, result) in results.iter().enumerate() {
        if use_color {
            println!(
                "\x1b[1m{} Cost (last {} days)\x1b[0m",
                result.display_name, days
            );
        } else {
            println!("{} Cost (last {} days)", result.display_name, days);
        }

        if !result.supported {
            println!("  Local cost scanning not available for this provider");
            println!("  (Only Codex and Claude have local logs)");
        } else if result.summary.sessions_count == 0 {
            println!("  No usage data found");
            println!("  Check that you have used {} locally", result.display_name);
        } else {
            // Total cost
            if use_color {
                println!(
                    "  Total:    \x1b[32m{}\x1b[0m",
                    result.summary.format_total()
                );
            } else {
                println!("  Total:    {}", result.summary.format_total());
            }

            // Token breakdown
            println!(
                "  Tokens:   {} input, {} output, {} cached",
                format_number(result.summary.input_tokens),
                format_number(result.summary.output_tokens),
                format_number(result.summary.cached_tokens)
            );

            // Sessions
            println!("  Sessions: {}", result.summary.sessions_count);

            // Cost by model
            if !result.summary.by_model.is_empty() {
                println!("  By model:");
                let mut models: Vec<_> = result.summary.by_model.iter().collect();
                models.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));
                for (model, cost) in models {
                    println!("    {}: ${:.2}", model, cost);
                }
            }

            if !result.summary.by_speed.is_empty() {
                println!("  Codex speed:");
                for bucket in ["standard", "fast"] {
                    if let Some(cost) = result.summary.by_speed.get(bucket) {
                        let tokens = result
                            .summary
                            .by_speed_tokens
                            .get(bucket)
                            .map(|counts| format_number(counts.total()))
                            .unwrap_or_else(|| "0".to_string());
                        println!("    {}: ${:.2} ({} tokens)", bucket, cost, tokens);
                    }
                }
            }
        }

        if i < results.len() - 1 {
            println!();
        }
    }
}

/// Print JSON output
fn print_json_output(results: &[CostResult], pretty: bool, days: u32) -> anyhow::Result<()> {
    let payloads: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            if !r.supported {
                serde_json::json!({
                    "provider": r.provider,
                    "supported": false,
                    "error": "Local cost scanning not available for this provider"
                })
            } else {
                serde_json::json!({
                    "provider": r.provider,
                    "supported": true,
                    "days_scanned": days,
                    "cost": {
                        "total_usd": r.summary.total_cost_usd,
                        "currency": "USD"
                    },
                    "tokens": {
                        "input": r.summary.input_tokens,
                        "output": r.summary.output_tokens,
                        "cached": r.summary.cached_tokens
                    },
                    "sessions_count": r.summary.sessions_count,
                    "by_model": r.summary.by_model,
                    "by_speed": r.summary.by_speed,
                    "by_speed_tokens": r.summary.by_speed_tokens.iter().map(|(bucket, counts)| {
                        (bucket.clone(), serde_json::json!({
                            "input": counts.input_tokens,
                            "output": counts.output_tokens,
                            "cached": counts.cached_tokens,
                            "total": counts.total()
                        }))
                    }).collect::<serde_json::Map<_, _>>(),
                    "period": {
                        "start": r.summary.period_start.map(|d| d.to_string()),
                        "end": r.summary.period_end.map(|d| d.to_string())
                    }
                })
            }
        })
        .collect();

    let output = if pretty {
        serde_json::to_string_pretty(&payloads)?
    } else {
        serde_json::to_string(&payloads)?
    };
    println!("{}", output);

    Ok(())
}

/// Format a number with commas
fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if i > 0 && (chars.len() - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(*c);
    }
    result
}

/// Check if stdout is a terminal
fn is_terminal() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}
