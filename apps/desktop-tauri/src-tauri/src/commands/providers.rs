use super::*;
use std::sync::Arc;

const MAX_CONCURRENT_PROVIDER_FETCHES: usize = 8;

// ── Provider refresh commands ────────────────────────────────────────

/// Build a `FetchContext` for a provider using persisted cookies/keys.
pub(crate) fn build_fetch_context(
    id: ProviderId,
    settings: &Settings,
    cookies: &ManualCookies,
    api_keys: &ApiKeys,
    token_accounts: &HashMap<ProviderId, ProviderAccountData>,
) -> FetchContext {
    let cookie_source = settings.cookie_source(id);
    let stored_cookie = cookies.get(id.cli_name()).map(|s| s.to_string());
    let stored_api_key = api_keys.get(id.cli_name()).map(|s| s.to_string());
    let token_override = token_accounts
        .get(&id)
        .and_then(|data| data.active_account())
        .cloned()
        .map(|account| TokenAccountOverride::from_account(id, account));
    let active_token_cookie = token_override
        .as_ref()
        .and_then(|override_data| override_data.cookie_header.clone());
    let active_token_env = token_override
        .as_ref()
        .and_then(|override_data| override_data.env_override.as_ref());
    let active_token_api_key = active_token_env.and_then(|env| env.values().next().cloned());
    let usage_source = SourceMode::parse(settings.usage_source(id)).unwrap_or_default();
    // Selected token-account key overrides a stored provider apiKey (upstream #2271 / #1183).
    let api_key = active_token_api_key.or(stored_api_key);
    let has_kimi_code_api_key =
        id == ProviderId::Kimi && api_key.as_deref().is_some_and(|key| !key.trim().is_empty());

    let (mut source_mode, mut cookie_header) = if id.cookie_domain().is_none() {
        let source_mode = if active_token_env.is_some() {
            SourceMode::OAuth
        } else {
            usage_source
        };
        (source_mode, None)
    } else {
        match cookie_source {
            _ if active_token_env.is_some() => (SourceMode::OAuth, None),
            "off" if id == ProviderId::Claude && usage_source != SourceMode::Cli => {
                (SourceMode::OAuth, None)
            }
            "off" if has_kimi_code_api_key && usage_source == SourceMode::Auto => {
                (SourceMode::Auto, None)
            }
            // Droid/Factory: cookie-off must never scrape browser cookies. Map to
            // Cli (API-only in the provider) so Auto does not fall through to web.
            "off" if id == ProviderId::Factory => (SourceMode::Cli, None),
            "off" => (SourceMode::Cli, None),
            "manual" => {
                let cookie_header = active_token_cookie.or(stored_cookie);
                let source_mode = if has_kimi_code_api_key && usage_source == SourceMode::Auto {
                    SourceMode::Auto
                } else if cookie_header.is_some() {
                    SourceMode::Web
                } else if id == ProviderId::Claude && usage_source != SourceMode::Cli {
                    SourceMode::OAuth
                } else {
                    SourceMode::Cli
                };
                (source_mode, cookie_header)
            }
            // `browser` is accepted as a legacy alias from older settings.
            "auto" | "browser" | "web" => {
                // Try browser cookie extraction as fallback when no manual cookie is set.
                // On non-Windows this is a harmless no-op that returns an error.
                let cookie_header = active_token_cookie.or(stored_cookie).or_else(|| {
                    provider_cookie_domain(id, settings).and_then(|domain| {
                        codexbar::browser::cookies::get_cookie_header(domain)
                            .ok()
                            .filter(|h| !h.is_empty())
                    })
                });
                (usage_source, cookie_header)
            }
            _ => (usage_source, stored_cookie),
        }
    };

    // Cookie-web providers (Cursor, OpenCode, …) reject SourceMode::Cli. The shell
    // historically mapped "manual + no cookie" to Cli, which surfaces as
    // "Source mode 'Cli' not supported". Remap to Web and try browser cookies
    // unless the user explicitly disabled cookies ("off").
    if source_mode == SourceMode::Cli
        && cookie_source != "off"
        && !instantiate_provider(id).supports_cli()
    {
        if cookie_header
            .as_deref()
            .map(str::trim)
            .is_none_or(|s| s.is_empty())
        {
            cookie_header = provider_cookie_domain(id, settings).and_then(|domain| {
                codexbar::browser::cookies::get_cookie_header(domain)
                    .ok()
                    .filter(|h| !h.is_empty())
            });
        }
        source_mode = SourceMode::Web;
    }

    let workspace_id = settings.workspace_id(id).trim().to_string();
    let api_region = settings.api_region(id).trim().to_string();
    let gateway_url = (id == ProviderId::Wayfinder && !settings.gateway_url(id).is_empty())
        .then(|| settings.gateway_url(id).to_string());
    // Local-first Auto providers (OpenCode Go) flip to web-first when a
    // token account or manual cookie source scopes the session to web creds.
    let auto_prefer_web = token_override.is_some() || cookie_source == "manual";

    FetchContext {
        source_mode,
        manual_cookie_header: cookie_header,
        api_key,
        workspace_id: (!workspace_id.is_empty()).then_some(workspace_id),
        api_region: (!api_region.is_empty()).then_some(api_region),
        gateway_url,
        auto_prefer_web,
        ..FetchContext::default()
    }
}

pub(crate) fn provider_cookie_domain(id: ProviderId, settings: &Settings) -> Option<&'static str> {
    if id == ProviderId::MiniMax {
        return Some(
            codexbar::providers::MiniMaxProvider::cookie_domain_for_region(Some(
                settings.api_region(id),
            )),
        );
    }
    if id == ProviderId::Alibaba {
        return Some(
            codexbar::providers::AlibabaProvider::cookie_domain_for_region(Some(
                settings.api_region(id),
            )),
        );
    }
    id.cookie_domain()
}

const DEFAULT_PROVIDER_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(35);
const SLOW_PROVIDER_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(75);
const MAX_CONTEXT_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(65);

pub(crate) fn provider_fetch_timeout(id: ProviderId, ctx: &FetchContext) -> std::time::Duration {
    let provider_timeout = match id {
        ProviderId::Claude | ProviderId::Codex | ProviderId::Copilot => SLOW_PROVIDER_FETCH_TIMEOUT,
        _ => DEFAULT_PROVIDER_FETCH_TIMEOUT,
    };
    let context_timeout = std::time::Duration::from_secs(ctx.web_timeout.saturating_add(5));
    provider_timeout.max(context_timeout.min(MAX_CONTEXT_FETCH_TIMEOUT))
}

pub(crate) fn is_provider_cache_fresh(
    updated_at: Option<std::time::Instant>,
    stale_after: std::time::Duration,
) -> bool {
    updated_at
        .map(|updated| updated.elapsed() <= stale_after)
        .unwrap_or(false)
}

pub(crate) fn upsert_provider_cache(
    cache: &mut Vec<ProviderUsageSnapshot>,
    snapshot: ProviderUsageSnapshot,
) {
    if let Some(existing) = cache
        .iter_mut()
        .find(|existing| existing.provider_id == snapshot.provider_id)
    {
        *existing = snapshot;
    } else {
        cache.push(snapshot);
    }
}

/// Drop cached snapshots for providers that are no longer enabled.
pub(crate) fn prune_provider_cache_to_enabled(
    cache: &mut Vec<ProviderUsageSnapshot>,
    enabled_ids: &[ProviderId],
) {
    cache.retain(|snapshot| {
        enabled_ids
            .iter()
            .any(|id| id.cli_name() == snapshot.provider_id)
    });
}

/// Invalidate in-flight publish work and remove disabled providers from cache.
///
/// Also clears the refresh lock so a follow-up force refresh can start immediately
/// (otherwise `begin_provider_refresh` no-ops while a superseded batch still holds
/// `is_refreshing`, and newly enabled providers never get a replacement run).
pub(crate) fn invalidate_provider_refresh_and_prune_disabled(
    state: &tauri::State<'_, Mutex<AppState>>,
    enabled_ids: &[ProviderId],
) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    guard.provider_refresh_generation = guard.provider_refresh_generation.wrapping_add(1);
    guard.is_refreshing = false;
    guard.provider_refresh_started_at = None;
    prune_provider_cache_to_enabled(&mut guard.provider_cache, enabled_ids);
    // Drop transient-failure counters for providers that left the enabled set.
    guard
        .transient_provider_failure_counts
        .retain(|id, _| enabled_ids.contains(id));
    Ok(())
}

pub(crate) fn is_current_provider_refresh_generation(guard: &AppState, generation: u64) -> bool {
    guard.provider_refresh_generation == generation
}

/// Core refresh logic, usable from both the Tauri command and tray menu actions.
pub(crate) async fn do_refresh_providers(app: &tauri::AppHandle) -> Result<(), String> {
    do_refresh_providers_with_policy(app, true).await
}

pub(crate) async fn do_refresh_providers_if_stale(app: &tauri::AppHandle) -> Result<(), String> {
    do_refresh_providers_with_policy(app, false).await
}

async fn do_refresh_providers_with_policy(
    app: &tauri::AppHandle,
    force: bool,
) -> Result<(), String> {
    let state = app.state::<Mutex<AppState>>();

    let Some(generation) = begin_provider_refresh(&state, force)? else {
        return Ok(());
    };

    let inputs = ProviderRefreshInputs::load();
    // Ensure cache only contains currently enabled providers for this generation.
    if let Ok(mut guard) = state.lock()
        && is_current_provider_refresh_generation(&guard, generation)
    {
        prune_provider_cache_to_enabled(&mut guard.provider_cache, &inputs.enabled_ids);
    }

    events::emit_refresh_started(
        app,
        inputs
            .enabled_ids
            .iter()
            .map(|id| id.cli_name().to_string())
            .collect(),
    );
    let enabled_count = inputs.enabled_ids.len();

    let handles = spawn_provider_refreshes(app, &inputs, generation);
    await_provider_refreshes(handles).await;

    let Some(error_count) = finish_provider_refresh(&state, generation)? else {
        // Superseded by a newer generation (or invalidate). Do not clear UI
        // "refreshing" for a dead batch or stamp tray from incomplete work.
        return Ok(());
    };
    update_tray_and_notifications(app, &state, &inputs.settings, &inputs.token_accounts)?;

    events::emit_refresh_complete(app, enabled_count, error_count);
    crate::auto_refresh::schedule_refresh_enrichment(&inputs.settings);

    Ok(())
}

fn begin_provider_refresh(
    state: &tauri::State<'_, Mutex<AppState>>,
    force: bool,
) -> Result<Option<u64>, String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    if guard.is_refreshing {
        return Ok(None);
    }
    if provider_cache_can_skip_refresh(&guard, force) {
        return Ok(None);
    }

    guard.provider_refresh_generation = guard.provider_refresh_generation.wrapping_add(1);
    let generation = guard.provider_refresh_generation;
    guard.is_refreshing = true;
    guard.provider_refresh_started_at = Some(std::time::Instant::now());
    Ok(Some(generation))
}

fn provider_cache_can_skip_refresh(guard: &AppState, force: bool) -> bool {
    !force
        && !guard.provider_cache.is_empty()
        && is_provider_cache_fresh(guard.provider_cache_updated_at, PROVIDER_CACHE_STALE_AFTER)
}

struct ProviderRefreshInputs {
    settings: Settings,
    enabled_ids: Vec<ProviderId>,
    manual_cookies: ManualCookies,
    api_keys: ApiKeys,
    token_accounts: HashMap<ProviderId, ProviderAccountData>,
}

impl ProviderRefreshInputs {
    fn load() -> Self {
        let settings = Settings::load();
        let enabled_ids = settings.get_enabled_provider_ids();
        let manual_cookies = ManualCookies::load();
        let api_keys = ApiKeys::load();
        let token_accounts = TokenAccountStore::new().load().unwrap_or_else(|e| {
            tracing::warn!("failed to load token accounts for provider refresh: {e}");
            HashMap::new()
        });

        Self {
            settings,
            enabled_ids,
            manual_cookies,
            api_keys,
            token_accounts,
        }
    }
}

fn spawn_provider_refreshes(
    app: &tauri::AppHandle,
    inputs: &ProviderRefreshInputs,
    generation: u64,
) -> Vec<tokio::task::JoinHandle<()>> {
    let mut handles = Vec::with_capacity(inputs.enabled_ids.len());
    let fetch_permits = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_PROVIDER_FETCHES));

    for id in &inputs.enabled_ids {
        let id = *id;
        let app_handle = app.clone();
        let fetch_permits = Arc::clone(&fetch_permits);
        let ctx = build_fetch_context(
            id,
            &inputs.settings,
            &inputs.manual_cookies,
            &inputs.api_keys,
            &inputs.token_accounts,
        );

        handles.push(tokio::spawn(async move {
            let Ok(_permit) = fetch_permits.acquire_owned().await else {
                return;
            };
            refresh_provider(app_handle, id, ctx, generation).await;
        }));
    }

    // ADR 0003 multi-account lanes: when Codex is enabled, refresh every
    // account snapshot (ambient + managed) on the same cycle, bounded by the
    // shared fetch semaphore. The ambient account still publishes the single
    // "codex" provider snapshot used by tray/menu; the lanes fill the account
    // snapshot store consumed by the Settings accounts panel.
    if inputs.enabled_ids.contains(&ProviderId::Codex) {
        let app_handle = app.clone();
        let fetch_permits = Arc::clone(&fetch_permits);
        handles.push(tokio::spawn(async move {
            super::codex_accounts::refresh_codex_account_lanes(app_handle, fetch_permits).await;
        }));
    }

    handles
}

async fn refresh_provider(
    app: tauri::AppHandle,
    id: ProviderId,
    ctx: FetchContext,
    generation: u64,
) {
    let snapshot = fetch_provider_snapshot(id, ctx).await;

    let state = app.state::<Mutex<AppState>>();
    let published = if let Ok(mut guard) = state.lock() {
        if !is_current_provider_refresh_generation(&guard, generation) {
            tracing::debug!(
                provider = id.cli_name(),
                generation,
                current = guard.provider_refresh_generation,
                "dropping superseded provider refresh result"
            );
            None
        } else {
            let snapshot = preserve_last_good_transient_failure(&mut guard, id, snapshot);
            // F6 (upstream 0.48.0): backfill missing reset timestamps from the
            // cached snapshot before persisting and publishing.
            let cached = guard
                .provider_cache
                .iter()
                .find(|c| c.provider_id == snapshot.provider_id && c.error.is_none())
                .cloned();
            let mut snapshot = snapshot;
            codex_reset_backfill(&mut snapshot, cached.as_ref());
            upsert_provider_cache(&mut guard.provider_cache, snapshot.clone());
            Some(snapshot)
        }
    } else {
        None
    };

    if let Some(snapshot) = published {
        events::emit_provider_updated(&app, &snapshot);
    }
}

/// F6 (upstream 0.48.0 UsageStore+CodexResetBackfill): backfill missing
/// `resets_at` / `reset_description` on fresh Codex windows from the cached
/// lane data when the cached reset is still future. Fresh `used_percent` is
/// untouched; only the reset timestamp/description are backfilled.
///
/// This is Codex-scoped by design (upstream: "Provider-specific by design"):
/// other providers do not carry bounded resume state.
///
/// Applies to the bridge snapshot before publishing so every surface (tray,
/// CLI, frontend) sees the backfilled reset instead of a missing one.
pub(super) fn codex_reset_backfill(
    snapshot: &mut ProviderUsageSnapshot,
    cached: Option<&ProviderUsageSnapshot>,
) {
    let Some(cached) = cached else { return };
    if snapshot.provider_id != "codex" {
        return;
    }

    // Backfill each slot from the corresponding cached slot.
    backfill_slot_window(&mut snapshot.primary, &cached.primary);
    if let (Some(fresh), Some(cached_sec)) = (&mut snapshot.secondary, &cached.secondary) {
        backfill_slot_window(fresh, cached_sec);
    }
    // Tertiary (monthly/other): the Codex bridge doesn't normally populate this,
    // but the slot exists for forward-compat. Backfill when available.
    if let (Some(fresh), Some(cached_ter)) = (&mut snapshot.tertiary, &cached.tertiary) {
        backfill_slot_window(fresh, cached_ter);
    }
}

/// Backfill `resets_at` and `reset_description` on a fresh window from the
/// cached window whose reset is still in the future. `used_percent` is never
/// overwritten (upstream: "fresh used_percent untouched").
fn backfill_slot_window(
    fresh: &mut bridge::RateWindowSnapshot,
    cached: &bridge::RateWindowSnapshot,
) {
    if fresh.resets_at.is_some() {
        return;
    }
    let Some(cached_reset) = &cached.resets_at else {
        return;
    };
    // Only backfill when the cached reset is still future — a stale reset is
    // worse than a missing one.
    if let Ok(cached_dt) = chrono::DateTime::parse_from_rfc3339(cached_reset) {
        if cached_dt <= chrono::Utc::now() {
            return;
        }
    } else {
        return;
    }
    fresh.resets_at = Some(cached_reset.clone());
    fresh.reset_description = fresh
        .reset_description
        .clone()
        .or_else(|| cached.reset_description.clone());
}

pub(super) fn preserve_last_good_transient_failure(
    guard: &mut AppState,
    id: ProviderId,
    snapshot: ProviderUsageSnapshot,
) -> ProviderUsageSnapshot {
    if snapshot.error.is_none() {
        guard.transient_provider_failure_counts.remove(&id);
        return snapshot;
    }

    if id != ProviderId::Claude {
        guard.transient_provider_failure_counts.remove(&id);
        return snapshot;
    }

    let error = snapshot.error.as_deref();
    // Hard auth loss / subscription-unavailable answers should not keep stale bars.
    if is_hard_claude_auth_loss(error) {
        guard.transient_provider_failure_counts.remove(&id);
        return snapshot;
    }

    let preservable = is_transient_claude_auth_error(error)
        || is_claude_cli_usage_parse_failure(error)
        || is_claude_cli_rate_limit_failure(error)
        || is_claude_timeout_failure(error);
    if !preservable {
        guard.transient_provider_failure_counts.remove(&id);
        return snapshot;
    }

    let Some(previous) = guard
        .provider_cache
        .iter()
        .find(|cached| cached.provider_id == id.cli_name() && cached.error.is_none())
        .cloned()
    else {
        return snapshot;
    };

    // Parse / rate-limit / timeout: keep last-good every time (upstream #2247).
    // Transient auth (unauthorized-ish) still only preserves once so real logout surfaces.
    let parse_or_rate = is_claude_cli_usage_parse_failure(error)
        || is_claude_cli_rate_limit_failure(error)
        || is_claude_timeout_failure(error);

    let count = guard
        .transient_provider_failure_counts
        .entry(id)
        .or_insert(0);
    if parse_or_rate || *count == 0 {
        if !parse_or_rate {
            *count = 1;
        }
        tracing::warn!(
            provider = id.cli_name(),
            error = error.unwrap_or(""),
            "preserving last good Claude snapshot after transient failure"
        );
        previous
    } else {
        *count = count.saturating_add(1);
        snapshot
    }
}

fn is_transient_claude_auth_error(error: Option<&str>) -> bool {
    let Some(error) = error else {
        return false;
    };
    let lower = error.to_ascii_lowercase();
    lower.contains("unauthorized")
        || lower.contains("authentication required")
        || lower.contains("auth required")
}

fn is_hard_claude_auth_loss(error: Option<&str>) -> bool {
    let Some(error) = error else {
        return false;
    };
    let lower = error.to_ascii_lowercase();
    // Credentials truly missing / login required — clear stale usage.
    lower.contains("credentials not found")
        || lower.contains("run `claude` to authenticate")
        || (lower.contains("not installed") && lower.contains("claude"))
        || (lower.contains("subscription") && lower.contains("unavailable"))
}

fn is_claude_cli_usage_parse_failure(error: Option<&str>) -> bool {
    let Some(error) = error else {
        return false;
    };
    let lower = error.to_ascii_lowercase();
    lower.contains("parse error")
        || lower.contains("empty output")
        || lower.contains("missing current session")
        || lower.contains("treated /usage as a normal prompt")
        || lower.contains("local activity stats")
        || lower.contains("could not parse")
}

fn is_claude_cli_rate_limit_failure(error: Option<&str>) -> bool {
    let Some(error) = error else {
        return false;
    };
    let lower = error.to_ascii_lowercase();
    lower.contains("rate limit") || lower.contains("rate_limit") || lower.contains("ratelimited")
}

fn is_claude_timeout_failure(error: Option<&str>) -> bool {
    let Some(error) = error else {
        return false;
    };
    error.eq_ignore_ascii_case("timeout") || error.to_ascii_lowercase().contains("timed out")
}

async fn fetch_provider_snapshot(id: ProviderId, ctx: FetchContext) -> ProviderUsageSnapshot {
    let provider = instantiate_provider(id);
    let metadata = provider.metadata().clone();
    let started = std::time::Instant::now();

    let mut snapshot =
        match tokio::time::timeout(provider_fetch_timeout(id, &ctx), provider.fetch_usage(&ctx))
            .await
        {
            Ok(Ok(result)) => ProviderUsageSnapshot::from_fetch_result(id, &metadata, &result),
            Ok(Err(e)) => ProviderUsageSnapshot::from_error(
                id,
                &metadata,
                codexbar::logging::safe_error_message(e),
            ),
            Err(_) => ProviderUsageSnapshot::from_error(id, &metadata, "Timeout".to_string()),
        };

    record_provider_fetch_duration(id, &mut snapshot, started);
    snapshot
}

fn record_provider_fetch_duration(
    id: ProviderId,
    snapshot: &mut ProviderUsageSnapshot,
    started: std::time::Instant,
) {
    let fetch_duration_ms = started.elapsed().as_millis();
    snapshot.fetch_duration_ms = Some(fetch_duration_ms);
    if fetch_duration_ms > 5_000 {
        tracing::warn!(
            provider = id.cli_name(),
            fetch_duration_ms,
            "slow provider refresh"
        );
    }
}

async fn await_provider_refreshes(handles: Vec<tokio::task::JoinHandle<()>>) {
    for handle in handles {
        let _ = handle.await;
    }
}

/// Finish a refresh batch. Returns `None` when `generation` was superseded
/// (do not emit complete / tray updates for dead work). Returns `Some(error_count)`
/// when this batch still owns the generation and the lock was released.
fn finish_provider_refresh(
    state: &tauri::State<'_, Mutex<AppState>>,
    generation: u64,
) -> Result<Option<usize>, String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    if !is_current_provider_refresh_generation(&guard, generation) {
        // A newer begin or invalidate owns the lock/generation. Do not clear
        // is_refreshing — that would race a live successor batch.
        return Ok(None);
    }
    guard.is_refreshing = false;
    guard.provider_refresh_started_at = None;
    guard.provider_cache_updated_at = Some(std::time::Instant::now());
    Ok(Some(
        guard
            .provider_cache
            .iter()
            .filter(|s| s.error.is_some())
            .count(),
    ))
}

fn update_tray_and_notifications(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, Mutex<AppState>>,
    settings: &Settings,
    token_accounts: &HashMap<ProviderId, ProviderAccountData>,
) -> Result<(), String> {
    let cached = {
        let guard = state.lock().map_err(|e| e.to_string())?;
        guard.provider_cache.clone()
    };
    crate::tray_bridge::update_tray_status_items(app, &cached);
    crate::tray_bridge::update_tray_icon_and_tooltip(app, &cached);
    notify_usage_thresholds(state, settings, token_accounts, &cached);
    Ok(())
}

fn notify_usage_thresholds(
    state: &tauri::State<'_, Mutex<AppState>>,
    settings: &Settings,
    token_accounts: &HashMap<ProviderId, ProviderAccountData>,
    cached: &[ProviderUsageSnapshot],
) {
    let cli_map = codexbar::core::cli_name_map();
    if let Ok(mut guard) = state.lock() {
        for snapshot in cached {
            if snapshot.error.is_none()
                && let Some(&provider) = cli_map.get(snapshot.provider_id.as_str())
            {
                let token_account_id = token_accounts
                    .get(&provider)
                    .and_then(ProviderAccountData::active_account)
                    .map(|account| account.id);
                let account = quota_notification_account_identity(snapshot, token_account_id);
                // Skip session notifications for synthetic/no-session placeholders
                // (e.g. Claude web five_hour: null → informational 5h 0%).
                if !snapshot.primary.is_informational {
                    guard.notification_manager.check_and_notify(
                        provider,
                        &account,
                        "session",
                        snapshot.primary.used_percent,
                        settings,
                    );
                    guard.notification_manager.check_session_transition(
                        provider,
                        &account,
                        snapshot.primary.used_percent,
                        settings,
                    );
                    dispatch_quota_hooks(
                        settings,
                        provider,
                        &account,
                        "session",
                        snapshot.primary.used_percent,
                    );
                }
                if let Some(weekly) = &snapshot.secondary
                    && !weekly.is_informational
                {
                    guard.notification_manager.check_and_notify(
                        provider,
                        &account,
                        "weekly",
                        weekly.used_percent,
                        settings,
                    );
                    dispatch_quota_hooks(
                        settings,
                        provider,
                        &account,
                        "weekly",
                        weekly.used_percent,
                    );
                }
                notify_predictive_pace(
                    &mut guard.notification_manager,
                    provider,
                    snapshot,
                    token_accounts,
                    settings,
                );
            }
        }
    }
}

fn dispatch_quota_hooks(
    settings: &Settings,
    provider: ProviderId,
    account: &str,
    window: &str,
    used_percent: f64,
) {
    if !settings.hooks_enabled {
        return;
    }
    let thresholds = settings.usage_thresholds(provider, window);
    let account = if settings.hide_personal_info || account.is_empty() {
        None
    } else {
        Some(account)
    };
    codexbar::core::emit_quota_threshold_hooks(
        true,
        provider.cli_name(),
        window,
        used_percent,
        thresholds.high,
        thresholds.critical,
        account,
    );
}

/// Stable account discriminator for threshold/session toast dedupe.
/// Prefer token-account id, then email, org, plan; empty for single-account lanes.
fn quota_notification_account_identity(
    snapshot: &ProviderUsageSnapshot,
    token_account_id: Option<uuid::Uuid>,
) -> String {
    if let Some(id) = token_account_id {
        return format!("token-account:{}", id.as_hyphenated());
    }
    if let Some(email) = snapshot
        .account_email
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return email.to_ascii_lowercase();
    }
    if let Some(org) = snapshot
        .account_organization
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return format!("org:{}", org.to_ascii_lowercase());
    }
    // Do not fall back to plan_name/login_method — those are display tiers and
    // flicker across refreshes, re-arming still-hot windows for a new identity.
    String::new()
}

fn notify_predictive_pace(
    manager: &mut codexbar::notifications::NotificationManager,
    provider: ProviderId,
    snapshot: &ProviderUsageSnapshot,
    token_accounts: &HashMap<ProviderId, ProviderAccountData>,
    settings: &Settings,
) {
    let enabled = settings.show_notifications && settings.predictive_pace_warning_enabled;
    manager.set_predictive_warnings_enabled(provider, enabled);
    if !enabled || !matches!(provider, ProviderId::Claude | ProviderId::Codex) {
        return;
    }

    let token_account_id = token_accounts
        .get(&provider)
        .and_then(ProviderAccountData::active_account)
        .map(|account| account.id);
    let Some(identity) = predictive_warning_identity(
        provider,
        &snapshot.source_label,
        snapshot.account_email.as_deref(),
        token_account_id,
    ) else {
        return;
    };
    let observed_at = chrono::DateTime::parse_from_rfc3339(&snapshot.updated_at)
        .ok()
        .map(|date| date.with_timezone(&chrono::Utc));

    for (warning_window, window, default_window_minutes) in [
        (
            codexbar::notifications::PredictiveWarningWindow::Session,
            Some(&snapshot.primary),
            300,
        ),
        (
            codexbar::notifications::PredictiveWarningWindow::Weekly,
            snapshot.secondary.as_ref(),
            10080,
        ),
    ] {
        let Some(window) = window else {
            continue;
        };
        if window.is_informational {
            continue;
        }
        let rate_window = RateWindow::with_details(
            window.used_percent,
            window.window_minutes,
            window
                .resets_at
                .as_deref()
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                .map(|date| date.with_timezone(&chrono::Utc)),
            window.reset_description.clone(),
        );
        let Some(pace) =
            codexbar::core::UsagePace::weekly(&rate_window, observed_at, default_window_minutes)
        else {
            continue;
        };
        manager.check_predictive_pace(
            provider,
            &identity,
            warning_window,
            &rate_window,
            &pace,
            settings,
        );
    }
}

fn predictive_warning_identity(
    provider: ProviderId,
    source_label: &str,
    account_email: Option<&str>,
    token_account_id: Option<uuid::Uuid>,
) -> Option<String> {
    if !matches!(provider, ProviderId::Claude | ProviderId::Codex) {
        return None;
    }
    if let Some(id) = token_account_id {
        return Some(format!("token-account:{}", id.as_hyphenated()));
    }
    let source = source_label.trim().to_ascii_lowercase();
    let account = account_email?.trim().to_ascii_lowercase();
    if source.is_empty() || account.is_empty() {
        return None;
    }
    Some(format!("{source}:{account}"))
}

#[tauri::command]
pub async fn refresh_providers(app: tauri::AppHandle) -> Result<(), String> {
    do_refresh_providers(&app).await
}

#[tauri::command]
pub async fn refresh_providers_if_stale(app: tauri::AppHandle) -> Result<(), String> {
    do_refresh_providers_if_stale(&app).await
}

#[tauri::command]
pub fn get_cached_providers(
    state: tauri::State<'_, Mutex<AppState>>,
) -> Vec<ProviderUsageSnapshot> {
    let mut snapshots = state
        .lock()
        .map(|guard| guard.provider_cache.clone())
        .unwrap_or_default();
    let spark_usage_visible = Settings::load().codex_spark_usage_visible();
    for snapshot in &mut snapshots {
        super::filter_hidden_codex_spark_rows(snapshot, spark_usage_visible);
    }

    snapshots
}

#[cfg(test)]
mod predictive_warning_tests {
    use super::*;

    #[test]
    fn predictive_warning_identity_keeps_claude_sources_and_token_accounts_separate() {
        let account_id = uuid::Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();

        assert_eq!(
            predictive_warning_identity(
                ProviderId::Claude,
                "cli",
                Some("Person@Example.com"),
                None,
            )
            .as_deref(),
            Some("cli:person@example.com")
        );
        assert_eq!(
            predictive_warning_identity(
                ProviderId::Claude,
                "oauth",
                Some("Person@Example.com"),
                None,
            )
            .as_deref(),
            Some("oauth:person@example.com")
        );
        assert_eq!(
            predictive_warning_identity(
                ProviderId::Claude,
                "oauth",
                Some("Person@Example.com"),
                Some(account_id),
            )
            .as_deref(),
            Some("token-account:aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")
        );
    }

    #[test]
    fn predictive_warning_identity_skips_unidentified_accounts() {
        assert_eq!(
            predictive_warning_identity(ProviderId::Claude, "oauth", None, None),
            None
        );
        assert_eq!(
            predictive_warning_identity(ProviderId::Codex, "cli", Some("  "), None),
            None
        );
    }

    fn empty_snapshot() -> ProviderUsageSnapshot {
        let metadata = codexbar::core::instantiate_provider(ProviderId::Claude)
            .metadata()
            .clone();
        ProviderUsageSnapshot::from_error(ProviderId::Claude, &metadata, "unused".to_string())
    }

    #[test]
    fn quota_notification_account_identity_prefers_token_then_email() {
        let account_id = uuid::Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();
        let mut snapshot = empty_snapshot();
        snapshot.account_email = Some("Person@Example.com".to_string());
        snapshot.account_organization = Some("Acme Org".to_string());
        snapshot.plan_name = Some("Pro".to_string());

        assert_eq!(
            quota_notification_account_identity(&snapshot, Some(account_id)),
            "token-account:aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
        );
        assert_eq!(
            quota_notification_account_identity(&snapshot, None),
            "person@example.com"
        );

        snapshot.account_email = None;
        assert_eq!(
            quota_notification_account_identity(&snapshot, None),
            "org:acme org"
        );

        snapshot.account_organization = None;
        // plan_name/login_method is not a stable ownership key — fall through to "".
        assert_eq!(quota_notification_account_identity(&snapshot, None), "");

        snapshot.plan_name = None;
        assert_eq!(quota_notification_account_identity(&snapshot, None), "");
    }
}

#[cfg(test)]
mod reset_backfill_tests {
    use super::*;
    use crate::commands::bridge::{ProviderUsageSnapshot, RateWindowSnapshot};

    fn win(used: f64, resets_at: Option<&str>) -> RateWindowSnapshot {
        RateWindowSnapshot {
            used_percent: used,
            remaining_percent: 100.0 - used,
            window_minutes: Some(300),
            resets_at: resets_at.map(String::from),
            reset_description: None,
            is_exhausted: false,
            is_informational: false,
            reserve_percent: None,
            reserve_description: None,
            reserve_will_last_to_reset: false,
            reserve_eta_seconds: None,
        }
    }

    fn codex_snapshot(primary: RateWindowSnapshot) -> ProviderUsageSnapshot {
        ProviderUsageSnapshot {
            provider_id: "codex".into(),
            display_name: "Codex".into(),
            primary,
            primary_label: None,
            secondary: None,
            secondary_label: None,
            model_specific: None,
            tertiary: None,
            tertiary_label: None,
            extra_rate_windows: Vec::new(),
            cost: None,
            plan_name: None,
            account_email: None,
            source_label: String::new(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            error: None,
            pace: None,
            account_organization: None,
            tray_status_label: None,
            fetch_duration_ms: None,
            wayfinder_usage: None,
            session_equivalent_forecast: None,
        }
    }

    #[test]
    fn f6_backfills_future_cached_reset() {
        // Cached has a future resets_at; fresh has none → backfilled.
        let future = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
        let cached = codex_snapshot(win(50.0, Some(&future)));
        let mut fresh = codex_snapshot(win(30.0, None));
        codex_reset_backfill(&mut fresh, Some(&cached));
        assert_eq!(fresh.primary.resets_at.as_deref(), Some(future.as_str()));
        // used_percent is NOT overwritten.
        assert!((fresh.primary.used_percent - 30.0).abs() < f64::EPSILON);
    }

    #[test]
    fn f6_does_not_backfill_stale_cached_reset() {
        // Cached reset is in the past → not backfilled.
        let past = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        let cached = codex_snapshot(win(50.0, Some(&past)));
        let mut fresh = codex_snapshot(win(30.0, None));
        codex_reset_backfill(&mut fresh, Some(&cached));
        assert!(
            fresh.primary.resets_at.is_none(),
            "stale reset not backfilled"
        );
    }

    #[test]
    fn f6_does_not_overwrite_existing_resets_at() {
        // Fresh already has resets_at → cached not applied.
        let future1 = (chrono::Utc::now() + chrono::Duration::hours(3)).to_rfc3339();
        let future2 = (chrono::Utc::now() + chrono::Duration::hours(5)).to_rfc3339();
        let cached = codex_snapshot(win(50.0, Some(&future2)));
        let mut fresh = codex_snapshot(win(30.0, Some(&future1)));
        codex_reset_backfill(&mut fresh, Some(&cached));
        assert_eq!(fresh.primary.resets_at.as_deref(), Some(future1.as_str()));
    }

    #[test]
    fn f6_skips_non_codex_provider() {
        let future = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
        let mut cached = codex_snapshot(win(50.0, Some(&future)));
        cached.provider_id = "claude".into();
        let mut fresh = codex_snapshot(win(30.0, None));
        fresh.provider_id = "claude".into();
        codex_reset_backfill(&mut fresh, Some(&cached));
        assert!(fresh.primary.resets_at.is_none(), "non-codex skip");
    }

    #[test]
    fn f6_skips_when_no_cached_snapshot() {
        let mut fresh = codex_snapshot(win(30.0, None));
        codex_reset_backfill(&mut fresh, None);
        assert!(fresh.primary.resets_at.is_none());
    }
}
