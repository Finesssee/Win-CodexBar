//! Codex routed-model pricing (upstream 0.50.1 #2946).
//!
//! Codex rollouts routed through a non-OpenAI backend (DeepSeek, Kimi,
//! OpenCode) carry the provider as a `provider/model` prefix. This module
//! detects the route and strips the prefix so the cost lookup prices
//! against the right models.dev catalog instead of falling back to OpenAI.

/// Detect a provider-qualified route prefix on a Codex model name and
/// return the matching models.dev provider id (upstream 0.50.1 #2946).
///
/// Known routes: `deepseek/` → "deepseek", `kimi/` → "kimi",
/// `opencode/` → "opencode". The `openai/` prefix is stripped by
/// [`normalize_codex_model`] and priced against the OpenAI catalog as
/// before. Unknown `provider/` prefixes return `None` here so the caller
/// leaves them unpriced rather than guessing.
pub fn codex_routed_provider(model: &str) -> Option<&'static str> {
    let trimmed = model.trim();
    let (prefix, _rest) = trimmed.split_once('/')?;
    match prefix.to_ascii_lowercase().as_str() {
        "deepseek" => Some("deepseek"),
        "kimi" => Some("kimi"),
        "opencode" => Some("opencode"),
        _ => None,
    }
}

/// Strip a known route prefix, returning the model id for a models.dev
/// lookup. Unknown prefixes are left intact (the caller leaves them
/// unpriced). `openai/` is also stripped here for the routed path.
pub fn strip_route_prefix(model: &str) -> &str {
    let trimmed = model.trim();
    if let Some(rest) = trimmed.strip_prefix("openai/") {
        return rest;
    }
    if codex_routed_provider(trimmed).is_some()
        && let Some((_prefix, rest)) = trimmed.split_once('/')
    {
        return rest;
    }
    trimmed
}
