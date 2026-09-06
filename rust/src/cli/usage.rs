//! Usage command implementation

use clap::Args;
use serde::Serialize;

use crate::core::{
    CostSnapshot, FetchContext, ProviderFetchResult, ProviderId, RateWindow, SourceMode,
    TokenAccountStore, TokenAccountSupport, UsagePace, UsageSnapshot, instantiate_provider,
};
use crate::settings::ApiKeys;
use crate::status::{ProviderStatus as StatusInfo, StatusLevel, fetch_provider_status};

pub const PROVIDER_ARG_HELP: &str = "Provider to query (for example: codex, claude, gemini, antigravity/agy, nanogpt, deepseek, codebuff, windsurf, all, both)";

/// Arguments for the usage command
#[derive(Args, Debug, Default)]
pub struct UsageArgs {
    #[arg(short, long, help = PROVIDER_ARG_HELP)]
    pub provider: Option<String>,

    /// Output format: text, json, or toon
    #[arg(short, long, default_value = "text")]
    pub format: UsageOutputFormat,

    /// Shorthand for --format json
    #[arg(long)]
    pub json: bool,

    /// Skip credits line in output
    #[arg(long = "no-credits")]
    pub no_credits: bool,

    /// Disable ANSI colors in text output
    #[arg(long = "no-color")]
    pub no_color: bool,

    /// Pretty-print JSON output
    #[arg(long)]
    pub pretty: bool,

    /// Fetch and include provider status pages
    #[arg(long)]
    pub status: bool,

    /// Fetch all token accounts where supported
    #[arg(long = "all-accounts")]
    pub all_accounts: bool,

    /// Token-account label or 1-based index (requires a single provider)
    #[arg(long = "account")]
    pub account: Option<String>,

    /// Data source: auto, oauth, web, cli
    #[arg(long, default_value = "auto", value_parser = ["auto", "web", "cli", "oauth"])]
    pub source: String,

    /// Web fetch timeout in seconds
    #[arg(long = "web-timeout", default_value = "60")]
    pub web_timeout: u64,

    /// Print one compact line per provider
    #[arg(long)]
    pub brief: bool,
}

/// Output format enum
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UsageOutputFormat {
    #[default]
    Text,
    Json,
    Toon,
}

impl std::str::FromStr for OutputFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "text" => Ok(OutputFormat::Text),
            "json" => Ok(OutputFormat::Json),
            _ => Err(format!("Invalid format: {}. Use 'text' or 'json'", s)),
        }
    }
}

impl std::str::FromStr for UsageOutputFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "text" => Ok(UsageOutputFormat::Text),
            "json" => Ok(UsageOutputFormat::Json),
            "toon" => Ok(UsageOutputFormat::Toon),
            _ => Err(format!(
                "Invalid format: {}. Use 'text', 'json', or 'toon'",
                s
            )),
        }
    }
}

/// Provider selection from CLI args
#[derive(Debug, Clone)]
pub enum ProviderSelection {
    Single(ProviderId),
    Both,
    All,
}

impl ProviderSelection {
    pub fn from_arg(arg: Option<&str>) -> anyhow::Result<Self> {
        match arg.map(|s| s.to_lowercase()).as_deref() {
            Some("all") => Ok(ProviderSelection::All),
            Some("both") => Ok(ProviderSelection::Both),
            Some(name) => {
                if let Some(id) = ProviderId::from_cli_name(name) {
                    Ok(ProviderSelection::Single(id))
                } else {
                    anyhow::bail!(
                        "Unknown provider: '{}'. Use --help to see available providers.",
                        name
                    )
                }
            }
            None => Ok(ProviderSelection::Single(ProviderId::Claude)), // Default to Claude
        }
    }

    pub fn as_list(&self) -> Vec<ProviderId> {
        match self {
            ProviderSelection::Single(id) => vec![*id],
            ProviderSelection::Both => vec![ProviderId::Codex, ProviderId::Claude],
            ProviderSelection::All => ProviderId::all().to_vec(),
        }
    }
}

/// JSON output payload
#[derive(Debug, Serialize)]
pub struct ProviderPayload {
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub source: String,
    #[serde(flatten)]
    pub result: ProviderFetchResult,
}

/// Error payload for JSON output
#[derive(Debug, Serialize)]
struct ErrorPayload {
    provider: String,
    error: String,
}

/// Run the usage command
pub async fn run(args: UsageArgs) -> anyhow::Result<()> {
    let command = UsageCommand::from_args(args)?;
    command.log();
    let output = collect_usage_output(&command).await;
    print_usage_output(output)
}

struct UsageCommand {
    format: UsageOutputFormat,
    providers: Vec<ProviderId>,
    use_color: bool,
    brief: bool,
    fetch_status: bool,
    pretty: bool,
    /// Optional token-account label/index for a single-provider fetch.
    account: Option<String>,
    ctx: FetchContext,
}

impl UsageCommand {
    fn from_args(args: UsageArgs) -> anyhow::Result<Self> {
        let format = effective_format(&args);
        let source_mode = SourceMode::parse(&args.source).unwrap_or(SourceMode::Auto);
        let providers = ProviderSelection::from_arg(args.provider.as_deref())?.as_list();
        if args.account.is_some() && providers.len() != 1 {
            anyhow::bail!("--account requires a single --provider (not all/both)");
        }

        Ok(Self {
            format,
            providers,
            use_color: !args.no_color && is_terminal(),
            brief: args.brief,
            fetch_status: args.status,
            pretty: args.pretty,
            account: args.account.clone(),
            ctx: build_usage_fetch_context(&args, source_mode),
        })
    }

    fn log(&self) {
        tracing::debug!(
            "Running usage command: providers={:?}, format={:?}, source={:?}, status={}",
            self.providers,
            self.format,
            self.ctx.source_mode,
            self.fetch_status
        );
    }
}

fn effective_format(args: &UsageArgs) -> UsageOutputFormat {
    if args.json {
        UsageOutputFormat::Json
    } else {
        args.format
    }
}

fn build_usage_fetch_context(args: &UsageArgs, source_mode: SourceMode) -> FetchContext {
    FetchContext {
        source_mode,
        include_credits: !args.no_credits,
        web_timeout: args.web_timeout,
        verbose: false,
        manual_cookie_header: None,
        api_key: None,
        workspace_id: None,
        api_region: None,
        gateway_url: None,
        auto_prefer_web: false,
        // `codexbar usage` is a foreground read: optional enrichment (e.g. the
        // OpenCode Go Zen balance) is worth its full bounded wait (#2583).
        requires_optional_usage_completeness: true,
    }
}

enum UsageOutput {
    Text(Vec<String>),
    Json {
        results: Vec<serde_json::Value>,
        pretty: bool,
    },
    Toon(Vec<serde_json::Value>),
}

async fn collect_usage_output(command: &UsageCommand) -> UsageOutput {
    match command.format {
        UsageOutputFormat::Text => {
            let mut sections = Vec::new();
            for provider_id in &command.providers {
                sections.push(fetch_provider_text_output(*provider_id, command).await);
            }
            UsageOutput::Text(sections)
        }
        UsageOutputFormat::Json => {
            let mut results = Vec::new();
            for provider_id in &command.providers {
                results.push(fetch_provider_json_output(*provider_id, command).await);
            }
            UsageOutput::Json {
                results,
                pretty: command.pretty,
            }
        }
        UsageOutputFormat::Toon => {
            let mut results = Vec::new();
            for provider_id in &command.providers {
                results.push(fetch_provider_json_output(*provider_id, command).await);
            }
            UsageOutput::Toon(results)
        }
    }
}

async fn fetch_provider_text_output(provider_id: ProviderId, command: &UsageCommand) -> String {
    match fetch_provider_result(provider_id, command).await {
        Ok((result, status)) => {
            if command.brief {
                render_brief_text(provider_id, &result)
            } else {
                render_text_with_status(provider_id, &result, status.as_ref(), command.use_color)
            }
        }
        Err(e) => render_text_error(provider_id, &e.to_string(), command.use_color),
    }
}

async fn fetch_provider_json_output(
    provider_id: ProviderId,
    command: &UsageCommand,
) -> serde_json::Value {
    match fetch_provider_result(provider_id, command).await {
        Ok((result, status)) => render_json_result(provider_id, result, status.as_ref()),
        Err(e) => serde_json::json!({
            "provider": provider_id.cli_name(),
            "error": e.to_string(),
        }),
    }
}

async fn fetch_provider_result(
    provider_id: ProviderId,
    command: &UsageCommand,
) -> anyhow::Result<(ProviderFetchResult, Option<StatusInfo>)> {
    let provider = instantiate_provider(provider_id);
    let status_future = command
        .fetch_status
        .then(|| fetch_provider_status(provider_id.cli_name()));
    let mut ctx = command.ctx.clone();
    if ctx.api_key.is_none() {
        ctx.api_key = resolve_cli_api_key(provider_id, command.account.as_deref())?;
    }
    let result = provider.fetch_usage(&ctx).await?;
    let status = if let Some(fut) = status_future {
        fut.await
    } else {
        None
    };
    Ok((result, status))
}

/// Resolve an API key from token accounts (active or `--account`) then stored keys.
///
/// Token-account env injection takes precedence over `api_keys.json` so multi-key
/// providers (OpenRouter, z.ai, ...) honor the selected labeled account.
fn resolve_cli_api_key(
    provider_id: ProviderId,
    account_ref: Option<&str>,
) -> anyhow::Result<Option<String>> {
    if TokenAccountSupport::is_supported(provider_id)
        && let Ok(data) = TokenAccountStore::new().load_provider(provider_id)
        && !data.accounts.is_empty()
    {
        let account = if let Some(account_ref) = account_ref {
            find_token_account(&data, account_ref)?
        } else {
            data.active_account().ok_or_else(|| {
                anyhow::anyhow!("No active token account for {}", provider_id.display_name())
            })?
        };
        if let Some(env) = TokenAccountSupport::env_override(provider_id, &account.token)
            && let Some(key) = env.into_values().next()
        {
            return Ok(Some(key));
        }
    }

    Ok(ApiKeys::load()
        .get(provider_id.cli_name())
        .map(|s| s.to_string()))
}

fn find_token_account<'a>(
    data: &'a crate::core::ProviderAccountData,
    account_ref: &str,
) -> anyhow::Result<&'a crate::core::TokenAccount> {
    if let Ok(idx) = account_ref.parse::<usize>()
        && idx > 0
        && idx <= data.accounts.len()
    {
        return Ok(&data.accounts[idx - 1]);
    }
    if let Some(account) = data
        .accounts
        .iter()
        .find(|a| a.label.eq_ignore_ascii_case(account_ref))
    {
        return Ok(account);
    }
    anyhow::bail!(
        "Account '{}' not found. Use 'codexbar account list <provider>' to see accounts.",
        account_ref
    )
}

fn render_text_error(provider_id: ProviderId, error_msg: &str, use_color: bool) -> String {
    let header = if use_color {
        format!("\x1b[1m{}\x1b[0m", provider_id.display_name())
    } else {
        provider_id.display_name().to_string()
    };
    format!("{}  Error: {}", header, error_msg)
}

fn render_json_result(
    provider_id: ProviderId,
    result: ProviderFetchResult,
    status: Option<&StatusInfo>,
) -> serde_json::Value {
    let usage = &result.usage;
    let primary_pace = usage
        .primary
        .window_minutes
        .is_some_and(|m| m == crate::core::SESSION_WINDOW_MINUTES)
        .then(|| UsagePace::weekly(&usage.primary, None, crate::core::SESSION_WINDOW_MINUTES))
        .flatten()
        .map(pace_json);
    let secondary_pace = usage
        .secondary
        .as_ref()
        .and_then(|w| UsagePace::weekly(w, None, w.window_minutes.unwrap_or(10080)))
        .map(pace_json);

    let mut json_result = serde_json::json!({
        "provider": provider_id.cli_name(),
        "source": result.source_label,
        "usage": result.usage,
        "cost": result.cost,
    });
    if primary_pace.is_some() || secondary_pace.is_some() {
        json_result["pace"] = serde_json::json!({
            "primary": primary_pace,
            "secondary": secondary_pace,
        });
    }

    if let Some(s) = status {
        json_result["status"] = serde_json::json!({
            "level": format!("{:?}", s.level).to_lowercase(),
            "description": s.description,
        });
    }

    json_result
}

/// Serialize a [`UsagePace`] into a compact JSON object for the `--json` output.
fn pace_json(pace: UsagePace) -> serde_json::Value {
    serde_json::json!({
        "stage": format!("{:?}", pace.stage).to_lowercase(),
        "deltaPercent": pace.delta_percent,
        "expectedUsedPercent": pace.expected_used_percent,
        "willLastToReset": pace.will_last_to_reset,
    })
}

fn print_usage_output(output: UsageOutput) -> anyhow::Result<()> {
    match output {
        UsageOutput::Text(sections) => {
            println!("{}", sections.join("\n\n"));
        }
        UsageOutput::Json { results, pretty } => {
            let output = if pretty {
                serde_json::to_string_pretty(&results)?
            } else {
                serde_json::to_string(&results)?
            };
            println!("{}", output);
        }
        UsageOutput::Toon(results) => {
            println!(
                "{}",
                super::toon::encode(&serde_json::Value::Array(results))
            );
        }
    }

    Ok(())
}

/// Check if stdout is a terminal
fn is_terminal() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

/// Render usage as text with optional status
pub fn render_text_with_status(
    provider: ProviderId,
    result: &ProviderFetchResult,
    status: Option<&StatusInfo>,
    use_color: bool,
) -> String {
    let mut lines = Vec::new();
    let metadata = instantiate_provider(provider).metadata().clone();

    lines.push(render_usage_header(provider, result, status, use_color));
    append_status_line(&mut lines, status);
    append_account_lines(&mut lines, &result.usage);
    append_usage_window_lines(&mut lines, &result.usage, &metadata, use_color);
    append_cost_line(&mut lines, result.cost.as_ref());

    lines.join("\n")
}

fn render_usage_header(
    provider: ProviderId,
    result: &ProviderFetchResult,
    status: Option<&StatusInfo>,
    use_color: bool,
) -> String {
    let status_indicator = render_status_indicator(status, use_color);
    if use_color {
        format!(
            "\x1b[1m{}\x1b[0m ({}){}",
            provider.display_name(),
            result.source_label,
            status_indicator
        )
    } else {
        format!(
            "{} ({}){}",
            provider.display_name(),
            result.source_label,
            status_indicator
        )
    }
}

fn render_status_indicator(status: Option<&StatusInfo>, use_color: bool) -> String {
    let Some(status) = status else {
        return String::new();
    };

    let (symbol, color) = match status.level {
        StatusLevel::Operational => ("●", "\x1b[32m"), // Green
        StatusLevel::Degraded => ("◐", "\x1b[33m"),    // Yellow
        StatusLevel::Partial => ("◑", "\x1b[33m"),     // Yellow
        StatusLevel::Major => ("○", "\x1b[31m"),       // Red
        StatusLevel::Unknown => ("?", "\x1b[90m"),     // Gray
    };

    if use_color {
        format!(" {}{}\x1b[0m", color, symbol)
    } else {
        format!(" {}", symbol)
    }
}

fn append_status_line(lines: &mut Vec<String>, status: Option<&StatusInfo>) {
    if let Some(s) = status
        && s.level != StatusLevel::Operational
        && s.level != StatusLevel::Unknown
    {
        lines.push(format!("  Status: {}", s.description));
    }
}

fn append_account_lines(lines: &mut Vec<String>, usage: &UsageSnapshot) {
    if let Some(ref email) = usage.account_email {
        lines.push(format!("  Account: {}", email));
    }
    if let Some(ref method) = usage.login_method {
        lines.push(format!("  Plan:    {}", method));
    }
}

fn append_usage_window_lines(
    lines: &mut Vec<String>,
    usage: &UsageSnapshot,
    metadata: &crate::core::ProviderMetadata,
    use_color: bool,
) {
    let primary_label = usage
        .primary_label
        .as_deref()
        .unwrap_or(metadata.session_label);
    append_window_line(lines, primary_label, &usage.primary, use_color);
    // Pace for primary windows whose provider published a common cadence.
    let pace_minutes = usage.primary.window_minutes.filter(|minutes| {
        *minutes == crate::core::SESSION_WINDOW_MINUTES
            || *minutes >= crate::core::WEEKLY_WINDOW_MINUTES
    });
    if let Some(minutes) = pace_minutes
        && let Some(pace) = UsagePace::weekly(&usage.primary, None, minutes)
    {
        lines.push(format!(
            "  Pace:    {} {}",
            pace.stage.emoji(),
            pace.format_status()
        ));
    }
    append_secondary_window_line(
        lines,
        usage.secondary.as_ref(),
        usage
            .secondary_label
            .as_deref()
            .unwrap_or(metadata.weekly_label),
        use_color,
    );
    append_model_specific_line(lines, usage.model_specific.as_ref(), use_color);
    // F5 (upstream 0.48.0): monthly (30-day) lane. Label by duration cadence.
    if let Some(tertiary) = usage.tertiary.as_ref() {
        let cadence =
            crate::core::RateWindowCadence::from_minutes(tertiary.window_minutes.unwrap_or(0));
        let label = match cadence {
            crate::core::RateWindowCadence::Monthly => "Monthly",
            _ => "Tertiary",
        };
        append_window_line(lines, label, tertiary, use_color);
    }
}

fn append_window_line(lines: &mut Vec<String>, label: &str, window: &RateWindow, use_color: bool) {
    if window.is_informational {
        let description = window.reset_description.as_deref().unwrap_or("unavailable");
        lines.push(format!("  {:<8} {}", format!("{}:", label), description));
        return;
    }

    let bar = render_progress_bar(window.used_percent, 20, use_color);
    let reset = window
        .format_countdown()
        .map(|c| format!(" (resets in {})", c))
        .unwrap_or_default();
    lines.push(format!(
        "  {:<8} {} {} used{}",
        format!("{}:", label),
        bar,
        format_percent(window.used_percent),
        reset
    ));
}

fn append_secondary_window_line(
    lines: &mut Vec<String>,
    secondary: Option<&RateWindow>,
    label: &str,
    use_color: bool,
) {
    if let Some(secondary) = secondary {
        append_window_line(lines, label, secondary, use_color);
        let window_minutes = secondary.window_minutes.unwrap_or(10080);
        if let Some(pace) = UsagePace::weekly(secondary, None, window_minutes) {
            lines.push(format!(
                "  Pace:    {} {}",
                pace.stage.emoji(),
                pace.format_status()
            ));
        }
    }
}

fn append_model_specific_line(
    lines: &mut Vec<String>,
    model_specific: Option<&RateWindow>,
    use_color: bool,
) {
    if let Some(opus) = model_specific {
        let opus_bar = render_progress_bar(opus.used_percent, 20, use_color);
        lines.push(format!(
            "  Opus:    {} {} used",
            opus_bar,
            format_percent(opus.used_percent)
        ));
    }
}

pub fn render_brief_text(provider: ProviderId, result: &ProviderFetchResult) -> String {
    let metadata = instantiate_provider(provider).metadata().clone();
    let usage = &result.usage;
    let primary_label = usage
        .primary_label
        .as_deref()
        .unwrap_or(metadata.session_label);
    let mut parts = Vec::new();
    let reset = if usage.primary.is_informational {
        parts.push(format!("{primary_label} unavailable"));
        usage.secondary.as_ref().unwrap_or(&usage.primary)
    } else {
        parts.push(format!(
            "{} {}",
            primary_label,
            format_percent(usage.primary.used_percent)
        ));
        &usage.primary
    }
    .format_countdown()
    .unwrap_or_else(|| "n/a".to_string());
    if let Some(secondary) = &usage.secondary {
        parts.push(format!(
            "{} {}",
            usage
                .secondary_label
                .as_deref()
                .unwrap_or(metadata.weekly_label),
            format_percent(secondary.used_percent)
        ));
    }
    parts.push(format!("resets {reset}"));
    if let Some(plan) = &usage.login_method {
        parts.push(plan.clone());
    }
    format!("{}: {}", provider.display_name(), parts.join(", "))
}

fn format_percent(percent: f64) -> String {
    if !percent.is_finite() {
        "0%".to_string()
    } else if percent > 0.0 && percent < 1.0 {
        "<1%".to_string()
    } else {
        format!("{:.0}%", percent.clamp(0.0, 100.0))
    }
}

fn append_cost_line(lines: &mut Vec<String>, cost: Option<&CostSnapshot>) {
    let Some(cost) = cost else {
        return;
    };

    if let Some(limit) = cost.format_limit() {
        lines.push(format!(
            "  Cost:    {} / {} ({})",
            cost.format_used(),
            limit,
            cost.period
        ));
    } else {
        lines.push(format!(
            "  Cost:    {} ({})",
            cost.format_used(),
            cost.period
        ));
    }
}

/// Render usage as text (backwards compatible version)
pub fn render_text(provider: ProviderId, result: &ProviderFetchResult, use_color: bool) -> String {
    render_text_with_status(provider, result, None, use_color)
}

/// Render a text-based progress bar
fn render_progress_bar(percent: f64, width: usize, use_color: bool) -> String {
    let percent = if percent.is_finite() {
        percent.clamp(0.0, 100.0)
    } else {
        0.0
    };
    // percent is clamped to 0..=100, so the rounded product cannot exceed width.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "percent clamped to 0..=100, so the product cannot exceed width"
    )]
    let filled = ((percent / 100.0) * width as f64).round() as usize;
    let empty = width.saturating_sub(filled);

    let bar = format!("[{}{}]", "█".repeat(filled), "░".repeat(empty));

    if use_color {
        let color = if percent >= 90.0 {
            "\x1b[31m" // Red
        } else if percent >= 70.0 {
            "\x1b[33m" // Yellow
        } else {
            "\x1b[32m" // Green
        };
        format!("{}{}\x1b[0m", color, bar)
    } else {
        bar
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{ProviderAccountData, TokenAccount, TokenAccountSupport};

    fn fetch_result(usage: UsageSnapshot) -> ProviderFetchResult {
        ProviderFetchResult::new(usage, "test")
    }

    #[test]
    fn usage_output_format_accepts_toon() {
        assert_eq!(
            "toon".parse::<UsageOutputFormat>(),
            Ok(UsageOutputFormat::Toon)
        );
        assert!("toon".parse::<OutputFormat>().is_err());
    }

    #[test]
    fn openrouter_account_ref_resolves_labeled_key() {
        let mut data = ProviderAccountData::new();
        data.add_account(TokenAccount::new("Personal", "sk-or-v1-personal"));
        data.add_account(TokenAccount::new("Work", "sk-or-v1-work"));
        data.set_active(0);

        let work = find_token_account(&data, "Work").unwrap();
        let env = TokenAccountSupport::env_override(ProviderId::OpenRouter, &work.token).unwrap();
        assert_eq!(
            env.get("OPENROUTER_API_KEY").map(String::as_str),
            Some("sk-or-v1-work")
        );

        let by_index = find_token_account(&data, "2").unwrap();
        assert_eq!(by_index.token, "sk-or-v1-work");
    }

    #[test]
    fn text_rendering_shows_sub_one_percent_usage() {
        let result = fetch_result(UsageSnapshot::new(RateWindow::new(0.4)));

        let output = render_text_with_status(ProviderId::Codex, &result, None, false);

        assert!(output.contains("<1% used"));
    }

    #[test]
    fn brief_rendering_keeps_one_line_per_provider() {
        let result = fetch_result(
            UsageSnapshot::new(RateWindow::new(0.4))
                .with_secondary(RateWindow::new(100.0))
                .with_login_method("Pro"),
        );

        let output = render_brief_text(ProviderId::Claude, &result);

        assert_eq!(
            output,
            "Claude: Session (5h) <1%, Weekly 100%, resets n/a, Pro"
        );
    }

    #[test]
    fn secondary_label_override_is_shared_by_full_and_brief_renderers() {
        let result = fetch_result(
            UsageSnapshot::new(RateWindow::new(10.0))
                .with_secondary(RateWindow::new(20.0))
                .with_secondary_label("Weekly"),
        );
        let full = render_text_with_status(ProviderId::Antigravity, &result, None, false);
        let brief = render_brief_text(ProviderId::Antigravity, &result);
        assert!(full.contains("Weekly:"));
        assert!(brief.contains("Weekly 20%"));
    }
    #[test]
    fn primary_label_override_is_shared_by_full_and_brief_renderers() {
        let result =
            fetch_result(UsageSnapshot::new(RateWindow::new(42.0)).with_primary_label("Monthly"));

        let full = render_text_with_status(ProviderId::Grok, &result, None, false);
        let brief = render_brief_text(ProviderId::Grok, &result);

        assert!(full.contains("Monthly:"));
        assert!(brief.contains("Grok: Monthly 42%"));
        assert!(!brief.contains("Credits 42%"));
    }

    #[test]
    fn gemini_plan_preserves_acronym_casing() {
        let result = fetch_result(
            UsageSnapshot::new(RateWindow::new(0.0))
                .with_login_method("Gemini Code Assist in Google One AI Pro"),
        );

        let output = render_text(ProviderId::Gemini, &result, false);

        assert!(output.contains("Plan:    Gemini Code Assist in Google One AI Pro"));
        assert!(!output.contains("Google One Ai Pro"));
    }
}
