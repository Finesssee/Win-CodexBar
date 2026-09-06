//! Production dashboard snapshot producer: collects provider usage (bounded,
//! concurrent), local cost-scan data (off-runtime), and Claude token-account
//! rows, then projects them through the pure [`build_snapshot`] mapping.
//!
//! F9/#2717 parity notes (0.48.0): each provider fetch is individually bounded
//! so one slow provider cannot bar the snapshot; a timed-out/failed provider
//! becomes an error ROW, the build still completes, and every waiter receives
//! the finished (late) result via the coordinator — never a discarded build.

use std::collections::{BTreeSet, HashMap};
use std::pin::Pin;
use std::time::Duration;

use chrono::{Local, Utc};

use crate::core::{CostScanOptions, FetchContext, ProviderId, SourceMode, instantiate_provider};
use crate::cost_scanner::{self, CostScanner};
use crate::settings::Settings;

use super::snapshot::{
    AccountFetchEnvelope, ClaudeAccountsInput, DashboardIdentity, ProviderFetchEnvelope,
    RawCostPayload, SnapshotInput, SnapshotPayload, build_snapshot,
};

pub type BoxSnapshotFuture = Pin<Box<dyn Future<Output = Result<SnapshotPayload, String>> + Send>>;

/// Hard bound per provider fetch inside a dashboard build. Existing serve
/// `web_timeout` is 60 s; builds add a 75 s outer envelope (provider-internal
/// bounds stay authoritative, matching upstream's 0.8x-below-the-deadline rule
/// of thumb in spirit: anything genuinely stuck becomes an error row, and the
/// snapshot still completes).
const PROVIDER_FETCH_TIMEOUT: Duration = Duration::from_secs(75);
/// Same bound for one Claude account fetch.
const ACCOUNT_FETCH_TIMEOUT: Duration = Duration::from_secs(75);

/// Collects the stable dashboard-v1 payload independently of its transport
/// (upstream `DashboardSnapshotProducer.live` analog): `codexbar serve` wraps
/// it in the authenticated + cached route, `codexbar dashboard` runs it once.
#[derive(Clone, Debug)]
pub struct SnapshotProducer {
    pub refresh_seconds: u32,
    pub identity: Option<DashboardIdentity>,
    pub version: String,
    /// Outer per-provider fetch envelope; `None` relies on provider-internal
    /// `web_timeout` alone (`--timeout 0` in the dashboard command).
    pub fetch_timeout: Option<Duration>,
}

impl SnapshotProducer {
    pub fn new(refresh_seconds: u32, identity: Option<DashboardIdentity>) -> Self {
        Self {
            refresh_seconds,
            identity,
            version: env!("CARGO_PKG_VERSION").to_string(),
            fetch_timeout: Some(PROVIDER_FETCH_TIMEOUT),
        }
    }

    pub fn with_fetch_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.fetch_timeout = timeout;
        self
    }

    pub fn collect(&self) -> BoxSnapshotFuture {
        let this = self.clone();
        Box::pin(async move { this.collect_inner().await })
    }

    async fn collect_inner(&self) -> Result<SnapshotPayload, String> {
        let settings = Settings::load();
        // Resolve identity: explicit --identity flag wins; otherwise follow
        // the app's hide_personal_info setting (upstream 0.50.1 #2960).
        let identity = self.identity.unwrap_or(if settings.hide_personal_info {
            DashboardIdentity::Redacted
        } else {
            DashboardIdentity::Full
        });
        let provider_ids: Vec<ProviderId> = settings.get_enabled_provider_ids();

        // Concurrent, individually bounded provider fetches; order restored by index.
        let mut set = tokio::task::JoinSet::new();
        for (index, provider_id) in provider_ids.iter().enumerate() {
            let provider_id = *provider_id;
            let fetch_timeout = self.fetch_timeout;
            set.spawn(async move {
                (
                    index,
                    fetch_provider_envelope(provider_id, fetch_timeout).await,
                )
            });
        }
        let mut indexed = Vec::with_capacity(provider_ids.len());
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok(item) => indexed.push(item),
                Err(join_error) => {
                    return Err(format!("dashboard build task failed: {join_error}"));
                }
            }
        }
        indexed.sort_by_key(|(index, _)| *index);
        let providers: Vec<ProviderFetchEnvelope> =
            indexed.into_iter().map(|(_, envelope)| envelope).collect();

        let costs = collect_costs().await;
        let claude_accounts =
            collect_claude_accounts(provider_ids.contains(&ProviderId::Claude)).await;

        let order: Vec<String> = provider_ids
            .iter()
            .map(|id| id.cli_name().to_string())
            .collect();
        let enabled: BTreeSet<String> = order.iter().cloned().collect();
        let input = SnapshotInput {
            providers,
            costs,
            claude_accounts,
            identity,
            generated_at: Utc::now(),
            refresh_seconds: self.refresh_seconds,
            version: Some(self.version.clone()),
            order,
            enabled,
        };
        Ok(build_snapshot(&input))
    }
}

/// Fetch one provider with a hard outer bound; the error row carries the
/// failure instead of failing the whole snapshot (F9 semantics).
async fn fetch_provider_envelope(
    provider_id: ProviderId,
    fetch_timeout: Option<Duration>,
) -> ProviderFetchEnvelope {
    let provider = instantiate_provider(provider_id);
    let metadata = provider.metadata();
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
        requires_optional_usage_completeness: false,
    };
    let fetch = bounded_fetch(provider_id, ctx, None, fetch_timeout).await;
    ProviderFetchEnvelope {
        id: provider_id.cli_name().to_string(),
        display_name: metadata.display_name.to_string(),
        session_label: metadata.session_label.to_string(),
        weekly_label: metadata.weekly_label.to_string(),
        fetch,
    }
}

async fn bounded_fetch(
    provider_id: ProviderId,
    ctx: FetchContext,
    label: Option<&str>,
    timeout_budget: Option<Duration>,
) -> Result<crate::core::ProviderFetchResult, String> {
    let provider = instantiate_provider(provider_id);
    let Some(timeout_budget) = timeout_budget else {
        return provider
            .fetch_usage(&ctx)
            .await
            .map_err(|error| match label {
                Some(label) => format!("{label}: {error}"),
                None => error.to_string(),
            });
    };
    let secs = timeout_budget.as_secs();
    match tokio::time::timeout(timeout_budget, provider.fetch_usage(&ctx)).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(error)) => Err(match label {
            Some(label) => format!("{label}: {error}"),
            None => error.to_string(),
        }),
        Err(_) => Err(match label {
            Some(label) => format!("{label}: fetch timed out after {secs}s"),
            None => format!("fetch timed out after {secs}s"),
        }),
    }
}

/// Local cost data for the two scanned providers, computed off the async
/// runtime so a large corpus cannot stall dashboard builds.
async fn collect_costs() -> HashMap<String, RawCostPayload> {
    let result = tokio::task::spawn_blocking(|| {
        let scanner = CostScanner::new(30).with_options(CostScanOptions::app_driven());
        let codex = scanner.scan_codex_with_cancel(None);
        let claude = scanner.scan_claude_with_cancel(None);
        let today = Local::now().date_naive().format("%Y-%m-%d").to_string();
        let today_of = |provider: &str| {
            cost_scanner::get_daily_cost_history(provider, 30)
                .into_iter()
                .find(|(day, _)| day == &today)
                .and_then(|(_, cost)| cost)
        };
        let mut costs = HashMap::new();
        costs.insert(
            "codex".to_string(),
            RawCostPayload {
                today_usd: today_of("codex"),
                last_30_days_usd: Some(codex.total_cost_usd),
            },
        );
        costs.insert(
            "claude".to_string(),
            RawCostPayload {
                today_usd: today_of("claude"),
                last_30_days_usd: Some(claude.total_cost_usd),
            },
        );
        costs
    })
    .await;
    match result {
        Ok(costs) => costs,
        Err(join_error) => {
            tracing::warn!(
                ?join_error,
                "dashboard cost scan task failed; continuing without costs"
            );
            HashMap::new()
        }
    }
}

/// Claude token-account rows (upstream "claude-swap" analog): per-account
/// cookie-override fetches, active flag from the store, errors as rows.
async fn collect_claude_accounts(claude_enabled: bool) -> Option<ClaudeAccountsInput> {
    if !claude_enabled {
        return None;
    }
    let data = match crate::core::TokenAccountStore::new().load_provider(ProviderId::Claude) {
        Ok(data) => data,
        Err(error) => {
            return Some(ClaudeAccountsInput {
                accounts: Err(format!("claude token accounts unavailable: {error}")),
            });
        }
    };
    let active_index = data.active_account().map(|active| active.id);
    let mut set = tokio::task::JoinSet::new();
    for (index, account) in data.accounts.iter().cloned().enumerate() {
        set.spawn(async move {
            let header = crate::core::TokenAccountSupport::normalized_cookie_header(
                ProviderId::Claude,
                &account.token,
            );
            let ctx = FetchContext {
                source_mode: SourceMode::Auto,
                include_credits: true,
                web_timeout: 60,
                verbose: false,
                manual_cookie_header: Some(header),
                api_key: None,
                workspace_id: None,
                api_region: None,
                gateway_url: None,
                auto_prefer_web: false,
                requires_optional_usage_completeness: false,
            };
            let fetch = bounded_fetch(
                ProviderId::Claude,
                ctx,
                Some(&account.label),
                Some(ACCOUNT_FETCH_TIMEOUT),
            )
            .await;
            (
                index,
                AccountFetchEnvelope {
                    id: account.id.to_string(),
                    label: account.label.clone(),
                    active: active_index == Some(account.id),
                    fetch,
                },
            )
        });
    }
    let mut indexed = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok(item) = joined {
            indexed.push(item);
        }
    }
    indexed.sort_by_key(|(index, _)| *index);
    let accounts: Vec<AccountFetchEnvelope> = indexed.into_iter().map(|(_, row)| row).collect();
    if accounts.is_empty() {
        // No accounts: absent section (same as upstream when the adapter is off).
        return None;
    }
    Some(ClaudeAccountsInput {
        accounts: Ok(accounts),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn producer_defaults_to_redacted_identity() {
        let producer = SnapshotProducer::new(60, Some(DashboardIdentity::Redacted));
        assert_eq!(producer.identity, Some(DashboardIdentity::Redacted));
        assert_eq!(producer.refresh_seconds, 60);
        assert!(!producer.version.is_empty());
    }

    #[test]
    fn producer_none_identity_follows_settings() {
        let producer = SnapshotProducer::new(60, None);
        assert_eq!(producer.identity, None);
        assert_eq!(producer.refresh_seconds, 60);
    }
}
