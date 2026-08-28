#[cfg(test)]
mod tests {
    use super::{
        ModelsDevCache, ModelsDevCacheArtifact, ModelsDevCatalog, ModelsDevRefreshCoordinator,
    };
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn decodes_top_level_provider_map_and_converts_million_token_rates() {
        let catalog = ModelsDevCatalog::decode(
            r#"{
                "openai": {
                    "id": "openai",
                    "models": {
                        "openai/gpt-fresh": {
                            "id": "openai/gpt-fresh",
                            "cost": {
                                "input": 2.5,
                                "output": 10,
                                "cache_read": 0.25,
                                "cache_write": 3.75,
                                "context_over_200k": {
                                    "input": 5,
                                    "output": 15,
                                    "cache_read": 0.5,
                                    "cache_write": 7.5
                                }
                            }
                        }
                    }
                },
                "anthropic": {
                    "models": {
                        "claude-fresh": {
                            "id": "claude-fresh",
                            "cost": { "input": 3, "output": 15 }
                        }
                    }
                }
            }"#,
        )
        .expect("top-level catalog");

        let pricing = catalog.lookup("openai", "gpt-fresh").expect("pricing");
        assert_eq!(pricing.input_cost_per_token, 2.5e-6);
        assert_eq!(pricing.output_cost_per_token, 10e-6);
        assert_eq!(pricing.cache_read_input_cost_per_token, Some(0.25e-6));
        assert_eq!(pricing.cache_write_input_cost_per_token, Some(3.75e-6));
        assert_eq!(pricing.threshold_tokens, Some(200_000));
        assert_eq!(pricing.input_cost_per_token_above_threshold, Some(5e-6));
    }

    #[test]
    fn decodes_providers_envelope() {
        let catalog = ModelsDevCatalog::decode(
            r#"{
                "providers": {
                    "anthropic": {
                        "id": "anthropic",
                        "models": {
                            "claude-fresh": {
                                "id": "claude-fresh",
                                "cost": { "input": 3, "output": 15 }
                            }
                        }
                    },
                    "openai": {
                        "models": {
                            "gpt-fresh": {
                                "id": "gpt-fresh",
                                "cost": { "input": 2.5, "output": 10 }
                            }
                        }
                    }
                }
            }"#,
        )
        .expect("enveloped catalog");

        assert_eq!(
            catalog
                .lookup("anthropic", "claude-fresh")
                .expect("pricing")
                .output_cost_per_token,
            15e-6
        );
    }

    #[test]
    fn cache_artifact_is_versioned_and_expires_after_one_day() {
        let catalog = ModelsDevCatalog::decode(
            r#"{
                "openai": {
                    "models": {
                        "gpt-fresh": {
                            "id": "gpt-fresh",
                            "cost": { "input": 2.5, "output": 10 }
                        }
                    }
                },
                "anthropic": {
                    "models": {
                        "claude-fresh": {
                            "id": "claude-fresh",
                            "cost": { "input": 3, "output": 15 }
                        }
                    }
                }
            }"#,
        )
        .expect("catalog");
        let fetched_at = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let artifact = ModelsDevCacheArtifact::new(catalog, fetched_at);

        assert_eq!(artifact.version, ModelsDevCache::ARTIFACT_VERSION);
        assert!(!artifact.is_stale(fetched_at + Duration::from_secs(86_400)));
        assert!(artifact.is_stale(fetched_at + Duration::from_secs(86_401)));
        assert_eq!(
            ModelsDevCache::cache_path(Some(PathBuf::from("cache-root").as_path())),
            PathBuf::from("cache-root")
                .join("model-pricing")
                .join("models-dev-v1.json")
        );
    }

    #[tokio::test]
    async fn concurrent_refreshes_for_one_cache_path_share_one_operation() {
        let coordinator = ModelsDevRefreshCoordinator::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let first_calls = Arc::clone(&calls);
        let path = PathBuf::from("pricing.json");
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);

        let first = coordinator.refresh(path.clone(), now, async move {
            first_calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(10)).await;
            true
        });
        let second = coordinator.refresh(path, now, async {
            panic!("the second caller must await the first operation");
        });

        assert!(tokio::join!(first, second).0);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failed_refresh_is_not_retried_within_the_attempt_window() {
        let coordinator = ModelsDevRefreshCoordinator::default();
        let path = PathBuf::from("pricing.json");
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);

        assert!(
            !coordinator
                .refresh(path.clone(), now, async { false })
                .await
        );
        assert!(
            !coordinator
                .refresh(path, now + Duration::from_secs(60), async {
                    panic!("the 15-minute bound must suppress this attempt");
                })
                .await
        );
    }

    #[test]
    fn cache_path_uses_the_existing_per_user_cache_root() {
        let cache_root = ModelsDevCache::default_cache_root().expect("per-user cache root");
        assert_eq!(
            ModelsDevCache::cache_path(None),
            cache_root.join("model-pricing").join("models-dev-v1.json")
        );
    }
}

use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex as AsyncMutex, watch};

const MODELS_DEV_URL: &str = "https://models.dev/api.json";
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const REFRESH_ATTEMPT_WINDOW: Duration = Duration::from_secs(15 * 60);

/// Per-token pricing decoded from the models.dev catalog.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DynamicModelPricing {
    pub input_cost_per_token: f64,
    pub output_cost_per_token: f64,
    pub cache_read_input_cost_per_token: Option<f64>,
    pub cache_write_input_cost_per_token: Option<f64>,
    pub threshold_tokens: Option<u64>,
    pub input_cost_per_token_above_threshold: Option<f64>,
    pub output_cost_per_token_above_threshold: Option<f64>,
    pub cache_read_input_cost_per_token_above_threshold: Option<f64>,
    pub cache_write_input_cost_per_token_above_threshold: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
struct ModelsDevCatalog {
    providers: HashMap<String, ModelsDevProvider>,
}

/// Immutable models.dev view for callers that price many rows in one pass.
/// Loading this once avoids repeating cache metadata checks for every row.
#[derive(Debug, Clone)]
pub struct ModelsDevPricingSnapshot {
    artifact: Option<Arc<ModelsDevCacheArtifact>>,
}

impl ModelsDevPricingSnapshot {
    pub fn lookup(&self, provider_id: &str, model_id: &str) -> Option<DynamicModelPricing> {
        self.artifact
            .as_ref()
            .and_then(|artifact| artifact.catalog.lookup(provider_id, model_id))
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ModelsDevCatalogWire {
    Envelope {
        providers: HashMap<String, ModelsDevProvider>,
    },
    ProviderMap(HashMap<String, ModelsDevProvider>),
}

impl<'de> Deserialize<'de> for ModelsDevCatalog {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let providers = match ModelsDevCatalogWire::deserialize(deserializer)? {
            ModelsDevCatalogWire::Envelope { providers }
            | ModelsDevCatalogWire::ProviderMap(providers) => providers,
        };
        Ok(Self {
            providers: providers
                .into_iter()
                .map(|(key, provider)| {
                    (
                        normalize_provider_id(provider.id.as_deref().unwrap_or(&key)),
                        provider,
                    )
                })
                .collect(),
        })
    }
}

impl ModelsDevCatalog {
    #[cfg(test)]
    fn decode(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }

    fn lookup(&self, provider_id: &str, model_id: &str) -> Option<DynamicModelPricing> {
        let provider = self.providers.get(&normalize_provider_id(provider_id))?;
        let candidates = model_id_candidates(model_id);
        for candidate in &candidates {
            if let Some(model) = provider.models.get(candidate)
                && let Some(pricing) = DynamicModelPricing::from_model(model)
            {
                return Some(pricing);
            }
        }
        provider.models.values().find_map(|model| {
            let model_candidates = model_id_candidates(&model.id);
            candidates
                .iter()
                .any(|candidate| model_candidates.contains(candidate))
                .then(|| DynamicModelPricing::from_model(model))
                .flatten()
        })
    }

    fn is_plausible_refresh(&self) -> bool {
        ["openai", "anthropic"].into_iter().all(|provider_id| {
            self.providers
                .get(provider_id)
                .is_some_and(|provider| provider.models.values().any(ModelsDevModel::is_priceable))
        })
    }

    fn merge_priceable_entries_from(&mut self, cached: &Self) {
        for (provider_id, cached_provider) in &cached.providers {
            let provider = self
                .providers
                .entry(provider_id.clone())
                .or_insert_with(|| cached_provider.clone());
            let present_ids: HashSet<String> = provider
                .models
                .values()
                .filter(|model| model.is_priceable())
                .map(|model| stable_model_identity(&model.id))
                .collect();
            for (model_key, cached_model) in &cached_provider.models {
                if !cached_model.is_priceable()
                    || present_ids.contains(&stable_model_identity(&cached_model.id))
                {
                    continue;
                }
                let mut fallback_key = model_key.clone();
                if provider.models.contains_key(&fallback_key) {
                    fallback_key = format!(
                        "codexbar-fallback:{model_key}:{}",
                        normalize_model_id(&cached_model.id)
                    );
                }
                provider.models.insert(fallback_key, cached_model.clone());
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelsDevProvider {
    id: Option<String>,
    #[serde(default)]
    models: HashMap<String, ModelsDevModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelsDevModel {
    id: String,
    cost: Option<ModelsDevCost>,
}

impl ModelsDevModel {
    fn is_priceable(&self) -> bool {
        self.cost.as_ref().is_some_and(|cost| {
            cost.input.is_some_and(|rate| valid_number(&rate))
                && cost.output.is_some_and(|rate| valid_number(&rate))
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelsDevCost {
    input: Option<f64>,
    output: Option<f64>,
    #[serde(rename = "cache_read")]
    cache_read: Option<f64>,
    #[serde(rename = "cache_write")]
    cache_write: Option<f64>,
    #[serde(rename = "context_over_200k")]
    context_over_200k: Option<ModelsDevContextCost>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelsDevContextCost {
    input: Option<f64>,
    output: Option<f64>,
    #[serde(rename = "cache_read")]
    cache_read: Option<f64>,
    #[serde(rename = "cache_write")]
    cache_write: Option<f64>,
}

impl DynamicModelPricing {
    fn from_model(model: &ModelsDevModel) -> Option<Self> {
        let cost = model.cost.as_ref()?;
        let input = cost.input.filter(valid_number)?;
        let output = cost.output.filter(valid_number)?;
        let above = cost.context_over_200k.as_ref();
        Some(Self {
            input_cost_per_token: per_token(input),
            output_cost_per_token: per_token(output),
            cache_read_input_cost_per_token: cost.cache_read.filter(valid_number).map(per_token),
            cache_write_input_cost_per_token: cost.cache_write.filter(valid_number).map(per_token),
            threshold_tokens: above.is_some().then_some(200_000),
            input_cost_per_token_above_threshold: above
                .and_then(|cost| cost.input)
                .filter(valid_number)
                .map(per_token),
            output_cost_per_token_above_threshold: above
                .and_then(|cost| cost.output)
                .filter(valid_number)
                .map(per_token),
            cache_read_input_cost_per_token_above_threshold: above
                .and_then(|cost| cost.cache_read)
                .filter(valid_number)
                .map(per_token),
            cache_write_input_cost_per_token_above_threshold: above
                .and_then(|cost| cost.cache_write)
                .filter(valid_number)
                .map(per_token),
        })
    }
}

fn valid_number(rate: &f64) -> bool {
    rate.is_finite() && *rate >= 0.0
}

fn per_token(rate: f64) -> f64 {
    rate / 1_000_000.0
}

fn normalize_provider_id(provider_id: &str) -> String {
    provider_id.trim().to_ascii_lowercase()
}

fn normalize_model_id(model_id: &str) -> String {
    model_id.trim().to_string()
}

fn stable_model_identity(model_id: &str) -> String {
    let model_id = normalize_model_id(model_id);
    if let Some((base, suffix)) = model_id.split_once('@') {
        if suffix == "default" {
            return base.to_string();
        }
        if suffix.len() == 8 && suffix.bytes().all(|byte| byte.is_ascii_digit()) {
            return format!("{base}-{suffix}");
        }
    }
    model_id
}

fn model_id_candidates(model_id: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    append_model_candidate(&mut candidates, model_id.to_string());
    let mut index = 0;
    while index < candidates.len() {
        let candidate = candidates[index].clone();
        if let Some(rest) = candidate.strip_prefix("openai/") {
            append_model_candidate(&mut candidates, rest.to_string());
        }
        if let Some(rest) = candidate.strip_prefix("anthropic.") {
            append_model_candidate(&mut candidates, rest.to_string());
        }
        if candidate.contains("claude-")
            && let Some((_, tail)) = candidate.rsplit_once('.')
            && tail.starts_with("claude-")
        {
            append_model_candidate(&mut candidates, tail.to_string());
        }
        if let Some((base, suffix)) = candidate.split_once('@') {
            if suffix.len() == 8 && suffix.bytes().all(|byte| byte.is_ascii_digit()) {
                append_model_candidate(&mut candidates, format!("{base}-{suffix}"));
            }
            append_model_candidate(&mut candidates, base.to_string());
        } else if candidate.starts_with("claude-") {
            append_model_candidate(&mut candidates, format!("{candidate}@default"));
        }
        if let Some(base) = candidate.strip_suffix("-v1:0") {
            append_model_candidate(&mut candidates, base.to_string());
        }
        if let Some(base) = strip_date_suffix(&candidate) {
            append_model_candidate(&mut candidates, base.to_string());
        }
        index += 1;
    }
    candidates
}

fn append_model_candidate(candidates: &mut Vec<String>, candidate: String) {
    let candidate = normalize_model_id(&candidate);
    if !candidate.is_empty() && !candidates.contains(&candidate) {
        candidates.push(candidate);
    }
}

fn strip_date_suffix(model_id: &str) -> Option<&str> {
    let suffix = model_id.rsplit_once('-')?.1;
    if suffix.len() == 8 && suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some(&model_id[..model_id.len() - suffix.len() - 1]);
    }
    if suffix.len() != 2 || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let without_day = &model_id[..model_id.len() - 3];
    let month = without_day.rsplit_once('-')?.1;
    if month.len() != 2 || !month.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let without_month = &without_day[..without_day.len() - 3];
    let year = without_month.rsplit_once('-')?.1;
    if year.len() == 4 && year.bytes().all(|byte| byte.is_ascii_digit()) {
        Some(&without_month[..without_month.len() - 5])
    } else {
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelsDevCacheArtifact {
    version: u32,
    fetched_at_unix_ms: u64,
    catalog: ModelsDevCatalog,
}

impl ModelsDevCacheArtifact {
    fn new(catalog: ModelsDevCatalog, fetched_at: SystemTime) -> Self {
        Self {
            version: ModelsDevCache::ARTIFACT_VERSION,
            fetched_at_unix_ms: unix_ms(fetched_at),
            catalog,
        }
    }

    fn fetched_at(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(self.fetched_at_unix_ms)
    }

    fn is_stale(&self, now: SystemTime) -> bool {
        now.duration_since(self.fetched_at()).unwrap_or_default() > CACHE_TTL
    }
}

struct ModelsDevCacheLoad {
    artifact: Option<Arc<ModelsDevCacheArtifact>>,
    is_stale: bool,
}

struct ModelsDevCacheMemoEntry {
    modified_at: Option<SystemTime>,
    size: Option<u64>,
    artifact: Option<Arc<ModelsDevCacheArtifact>>,
}

static CACHE_MEMO: LazyLock<Mutex<HashMap<PathBuf, ModelsDevCacheMemoEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct ModelsDevCache;

impl ModelsDevCache {
    const ARTIFACT_VERSION: u32 = 1;

    fn default_cache_root() -> Option<PathBuf> {
        dirs::cache_dir().map(|path| path.join("CodexBar"))
    }

    fn cache_path(cache_root: Option<&Path>) -> PathBuf {
        cache_root
            .map(Path::to_path_buf)
            .or_else(Self::default_cache_root)
            .map(|root| {
                root.join("model-pricing")
                    .join(format!("models-dev-v{}.json", Self::ARTIFACT_VERSION))
            })
            .unwrap_or_default()
    }
}

fn unix_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

impl ModelsDevCache {
    fn load(now: SystemTime, cache_root: Option<&Path>) -> ModelsDevCacheLoad {
        let cache_path = Self::cache_path(cache_root);
        if cache_path.as_os_str().is_empty() {
            return ModelsDevCacheLoad {
                artifact: None,
                is_stale: true,
            };
        }
        let cache_path = standardized_cache_path(&cache_path);
        let (modified_at, size) = file_identity(&cache_path);
        let memoized = {
            let memo = CACHE_MEMO.lock().expect("models.dev cache memo lock");
            memo.get(&cache_path)
                .filter(|entry| entry.modified_at == modified_at && entry.size == size)
                .map(|entry| entry.artifact.clone())
        };
        let artifact = memoized.unwrap_or_else(|| {
            let artifact = fs::read(&cache_path)
                .ok()
                .and_then(|contents| {
                    serde_json::from_slice::<ModelsDevCacheArtifact>(&contents).ok()
                })
                .filter(|artifact| artifact.version == Self::ARTIFACT_VERSION)
                .map(Arc::new);
            CACHE_MEMO
                .lock()
                .expect("models.dev cache memo lock")
                .insert(
                    cache_path,
                    ModelsDevCacheMemoEntry {
                        modified_at,
                        size,
                        artifact: artifact.clone(),
                    },
                );
            artifact
        });
        let is_stale = artifact
            .as_ref()
            .is_none_or(|artifact| artifact.is_stale(now));
        ModelsDevCacheLoad { artifact, is_stale }
    }
}

fn file_identity(path: &Path) -> (Option<SystemTime>, Option<u64>) {
    let Ok(metadata) = fs::metadata(path) else {
        return (None, None);
    };
    (metadata.modified().ok(), Some(metadata.len()))
}

fn standardized_cache_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|current_dir| current_dir.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        }
    })
}

impl ModelsDevCache {
    fn save(catalog: ModelsDevCatalog, fetched_at: SystemTime, cache_root: Option<&Path>) -> bool {
        let cache_path = Self::cache_path(cache_root);
        if cache_path.as_os_str().is_empty() {
            return false;
        }
        let cache_path = standardized_cache_path(&cache_path);
        let Some(parent) = cache_path.parent() else {
            return false;
        };
        if fs::create_dir_all(parent).is_err() {
            return false;
        }
        let artifact = Arc::new(ModelsDevCacheArtifact::new(catalog, fetched_at));
        let Ok(contents) = serde_json::to_vec(&*artifact) else {
            return false;
        };
        let Ok(mut file) = std::fs::File::create(&cache_path) else {
            return false;
        };
        if std::io::Write::write_all(&mut file, &contents).is_err() {
            return false;
        }
        let (modified_at, size) = file_identity(&cache_path);
        CACHE_MEMO
            .lock()
            .expect("models.dev cache memo lock")
            .insert(
                cache_path,
                ModelsDevCacheMemoEntry {
                    modified_at,
                    size,
                    artifact: Some(artifact),
                },
            );
        true
    }
}

#[derive(Default)]
struct ModelsDevRefreshCoordinator {
    state: Arc<AsyncMutex<ModelsDevRefreshState>>,
}

#[derive(Default)]
struct ModelsDevRefreshState {
    in_flight: HashMap<PathBuf, watch::Receiver<Option<bool>>>,
    last_attempt: HashMap<PathBuf, SystemTime>,
}

impl ModelsDevRefreshCoordinator {
    async fn refresh<F>(&self, cache_path: PathBuf, now: SystemTime, operation: F) -> bool
    where
        F: Future<Output = bool> + Send + 'static,
    {
        let cache_path = standardized_cache_path(&cache_path);
        let mut state = self.state.lock().await;
        if let Some(in_flight) = state.in_flight.get(&cache_path) {
            let receiver = in_flight.clone();
            drop(state);
            return wait_for_refresh(receiver).await;
        }
        if state
            .last_attempt
            .get(&cache_path)
            .is_some_and(|last_attempt| {
                now.duration_since(*last_attempt).unwrap_or_default() < REFRESH_ATTEMPT_WINDOW
            })
        {
            return false;
        }

        state.last_attempt.insert(cache_path.clone(), now);
        let (sender, receiver) = watch::channel(None);
        state.in_flight.insert(cache_path.clone(), receiver.clone());
        drop(state);

        let state = Arc::clone(&self.state);
        tokio::spawn(async move {
            let result = operation.await;
            // Fire-and-forget result delivery; send fails only if all receivers
            // dropped, and the watch channel keeps the latest value regardless.
            let _delivered = sender.send(Some(result));
            state
                .lock()
                .await
                .in_flight
                .retain(|path, _| path != &cache_path);
        });
        wait_for_refresh(receiver).await
    }
}

async fn wait_for_refresh(mut receiver: watch::Receiver<Option<bool>>) -> bool {
    loop {
        if let Some(result) = *receiver.borrow() {
            return result;
        }
        if receiver.changed().await.is_err() {
            return false;
        }
    }
}

static REFRESH_COORDINATOR: LazyLock<ModelsDevRefreshCoordinator> =
    LazyLock::new(ModelsDevRefreshCoordinator::default);

/// Loads the cached models.dev catalog once for bulk-pricing callers.
pub fn pricing_snapshot() -> ModelsDevPricingSnapshot {
    let load = ModelsDevCache::load(SystemTime::now(), None);
    let artifact = (!load.is_stale).then_some(load.artifact).flatten();
    ModelsDevPricingSnapshot { artifact }
}

/// Looks up a cached models.dev price for a provider/model pair.
pub fn lookup(provider_id: &str, model_id: &str) -> Option<DynamicModelPricing> {
    let load = ModelsDevCache::load(SystemTime::now(), None);
    (!load.is_stale)
        .then_some(load.artifact)
        .flatten()
        .and_then(|artifact| artifact.catalog.lookup(provider_id, model_id))
}

/// Refreshes the models.dev cache once when supplied models lack cached pricing.
///
/// Returns true only if at least one supplied model has pricing after the coordinated refresh.
pub async fn refresh_unknown_models_if_needed(
    provider_id: &str,
    model_ids: &HashSet<String>,
) -> bool {
    if model_ids.is_empty() {
        return false;
    }
    refresh_unknown_models_at(provider_id, model_ids, SystemTime::now(), None).await
}

async fn refresh_unknown_models_at(
    provider_id: &str,
    model_ids: &HashSet<String>,
    now: SystemTime,
    cache_root: Option<&Path>,
) -> bool {
    let load = ModelsDevCache::load(now, cache_root);
    let unknown_models: Vec<String> = if load.is_stale {
        model_ids.iter().cloned().collect()
    } else {
        model_ids
            .iter()
            .filter(|model_id| {
                load.artifact
                    .as_ref()
                    .and_then(|artifact| artifact.catalog.lookup(provider_id, model_id))
                    .is_none()
            })
            .cloned()
            .collect()
    };
    if unknown_models.is_empty() {
        return true;
    }
    if load.artifact.as_ref().is_some_and(|artifact| {
        now.duration_since(artifact.fetched_at())
            .unwrap_or_default()
            < REFRESH_ATTEMPT_WINDOW
    }) {
        return false;
    }

    let cache_path = ModelsDevCache::cache_path(cache_root);
    if cache_path.as_os_str().is_empty() {
        return false;
    }
    let cache_root = cache_root.map(Path::to_path_buf);
    let refresh_cache_root = cache_root.clone();
    let _ = REFRESH_COORDINATOR
        .refresh(cache_path, now, async move {
            refresh_catalog(now, refresh_cache_root.as_deref()).await
        })
        .await;

    let refreshed = ModelsDevCache::load(now, cache_root.as_deref());
    !refreshed.is_stale
        && unknown_models.iter().any(|model_id| {
            refreshed
                .artifact
                .as_ref()
                .and_then(|artifact| artifact.catalog.lookup(provider_id, model_id))
                .is_some()
        })
}

async fn refresh_catalog(now: SystemTime, cache_root: Option<&Path>) -> bool {
    let Ok(client) = crate::core::apply_app_proxy(reqwest::Client::builder())
        .timeout(Duration::from_secs(20))
        .build()
    else {
        return false;
    };
    let Ok(response) = client.get(MODELS_DEV_URL).send().await else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    let Ok(mut catalog) = response.json::<ModelsDevCatalog>().await else {
        return false;
    };
    if !catalog.is_plausible_refresh() {
        return false;
    }
    if let Some(cached) = ModelsDevCache::load(now, cache_root).artifact {
        catalog.merge_priceable_entries_from(&cached.catalog);
    }
    ModelsDevCache::save(catalog, now, cache_root)
}
