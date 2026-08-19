//! Claude routed-model pricing via models.dev (upstream 0.53).

use super::models_dev_pricing;

pub fn models_dev_target(model: &str, normalized: String) -> Option<(&'static str, String)> {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some((route, raw_model)) = trimmed.split_once('/') {
        if raw_model.trim().is_empty() {
            return None;
        }
        let provider = match route.to_ascii_lowercase().as_str() {
            "anthropic" => "anthropic",
            "openai" => "openai",
            "google" => "google",
            "moonshot" => "moonshot",
            "kimi-for-coding" => "kimi-for-coding",
            "minimax" => "minimax",
            "deepseek" => "deepseek",
            _ => return None,
        };
        return Some((provider, raw_model.trim().to_string()));
    }

    let lower = normalized.to_ascii_lowercase();
    let provider = if lower.starts_with("claude-") {
        "anthropic"
    } else if lower.starts_with("gpt-")
        || lower.starts_with("chatgpt-")
        || lower.starts_with("text-embedding-")
        || ["o1", "o3", "o4"]
            .iter()
            .any(|prefix| lower == *prefix || lower.starts_with(&format!("{prefix}-")))
    {
        "openai"
    } else if ["gemini-", "gemma-", "deep-research-", "veo-", "lyria-"]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
    {
        "google"
    } else if lower == "kimi-for-coding" || lower == "k3" || lower.starts_with("k3-") {
        "kimi-for-coding"
    } else if lower.starts_with("kimi-") || lower.starts_with("moonshot-") {
        "moonshot"
    } else if lower.starts_with("minimax-") {
        "minimax"
    } else if lower.starts_with("deepseek-") {
        "deepseek"
    } else {
        "anthropic"
    };

    Some((provider, normalized))
}

pub fn cost_usd(
    model: &str,
    normalized: String,
    input: i32,
    cache_read: i32,
    cache_write: i32,
    output: i32,
) -> Option<f64> {
    let (provider, lookup_model) = models_dev_target(model, normalized)?;
    let pricing = models_dev_pricing::lookup(provider, &lookup_model)?;
    let input = input.max(0);
    let cache_read = cache_read.max(0);
    let cache_write = cache_write.max(0);
    let output = output.max(0);
    let use_tier = pricing.threshold_tokens.is_some_and(|threshold| {
        (input as u64) + (cache_read as u64) + (cache_write as u64) > threshold
    });
    let pick = |base: f64, above: Option<f64>| {
        if use_tier {
            above.unwrap_or(base)
        } else {
            base
        }
    };
    let input_rate = pick(
        pricing.input_cost_per_token,
        pricing.input_cost_per_token_above_threshold,
    );
    let cache_read_rate = if use_tier {
        pricing
            .cache_read_input_cost_per_token_above_threshold
            .or(pricing.cache_read_input_cost_per_token)
            .unwrap_or(input_rate)
    } else {
        pricing
            .cache_read_input_cost_per_token
            .unwrap_or(input_rate)
    };
    let cache_write_rate = if use_tier {
        pricing
            .cache_write_input_cost_per_token_above_threshold
            .or(pricing.cache_write_input_cost_per_token)
            .unwrap_or(input_rate)
    } else {
        pricing
            .cache_write_input_cost_per_token
            .unwrap_or(input_rate)
    };
    let output_rate = pick(
        pricing.output_cost_per_token,
        pricing.output_cost_per_token_above_threshold,
    );

    Some(
        (input as f64) * input_rate
            + (cache_read as f64) * cache_read_rate
            + (cache_write as f64) * cache_write_rate
            + (output as f64) * output_rate,
    )
}

pub fn input_cost_per_token(model: &str, normalized: String) -> Option<f64> {
    let (provider, lookup_model) = models_dev_target(model, normalized)?;
    models_dev_pricing::lookup(provider, &lookup_model).map(|pricing| pricing.input_cost_per_token)
}
