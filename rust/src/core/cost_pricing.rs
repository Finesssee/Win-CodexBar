//! Cost Usage Pricing
//!
//! Model-specific token pricing for Codex (OpenAI) and Claude (Anthropic) models.
//! Supports tiered pricing for models with token thresholds.

#![allow(dead_code)]

use super::models_dev_pricing;
use std::collections::HashMap;
use std::sync::LazyLock;

/// Whole-request Codex rates for input above the model context threshold.
#[derive(Debug, Clone, Copy)]
pub struct CodexLongContextRates {
    pub input_cost_per_token: f64,
    pub output_cost_per_token: f64,
    pub cache_read_input_cost_per_token: f64,
}

/// Codex (OpenAI) model pricing
#[derive(Debug, Clone, Copy)]
pub struct CodexPricing {
    /// Cost per input token in USD
    pub input_cost_per_token: f64,
    /// Cost per output token in USD
    pub output_cost_per_token: f64,
    /// Cost per cached input token in USD
    pub cache_read_input_cost_per_token: f64,
    /// Optional display label override (e.g. "Research Preview")
    pub display_label: Option<&'static str>,
    /// Whole-request rates above the Codex long-context threshold.
    pub long_context: Option<CodexLongContextRates>,
}

/// Claude (Anthropic) model pricing with optional tiered pricing
#[derive(Debug, Clone, Copy)]
pub struct ClaudePricing {
    /// Cost per input token in USD
    pub input_cost_per_token: f64,
    /// Cost per output token in USD
    pub output_cost_per_token: f64,
    /// Cost per cache creation input token in USD
    pub cache_creation_input_cost_per_token: f64,
    /// Cost per cache read input token in USD
    pub cache_read_input_cost_per_token: f64,
    /// Token threshold for tiered pricing (None = no tiering)
    pub threshold_tokens: Option<i32>,
    /// Cost per input token above threshold
    pub input_cost_per_token_above_threshold: Option<f64>,
    /// Cost per output token above threshold
    pub output_cost_per_token_above_threshold: Option<f64>,
    /// Cost per cache creation input token above threshold
    pub cache_creation_input_cost_per_token_above_threshold: Option<f64>,
    /// Cost per cache read input token above threshold
    pub cache_read_input_cost_per_token_above_threshold: Option<f64>,
}

/// Codex model pricing table
static CODEX_PRICING: LazyLock<HashMap<&'static str, CodexPricing>> = LazyLock::new(|| {
    let mut m = HashMap::new();

    // GPT-5 pricing
    m.insert(
        "gpt-5",
        CodexPricing {
            input_cost_per_token: 1.25e-6,
            output_cost_per_token: 1e-5,
            cache_read_input_cost_per_token: 1.25e-7,
            display_label: None,
            long_context: None,
        },
    );
    m.insert(
        "gpt-5-codex",
        CodexPricing {
            input_cost_per_token: 1.25e-6,
            output_cost_per_token: 1e-5,
            cache_read_input_cost_per_token: 1.25e-7,
            display_label: None,
            long_context: None,
        },
    );
    m.insert(
        "gpt-5-mini",
        CodexPricing {
            input_cost_per_token: 2.5e-7,
            output_cost_per_token: 2e-6,
            cache_read_input_cost_per_token: 2.5e-8,
            display_label: None,
            long_context: None,
        },
    );
    m.insert(
        "gpt-5-nano",
        CodexPricing {
            input_cost_per_token: 5e-8,
            output_cost_per_token: 4e-7,
            cache_read_input_cost_per_token: 5e-9,
            display_label: None,
            long_context: None,
        },
    );
    m.insert(
        "gpt-5-pro",
        CodexPricing {
            input_cost_per_token: 1.5e-5,
            output_cost_per_token: 1.2e-4,
            cache_read_input_cost_per_token: 1.5e-5,
            display_label: None,
            long_context: None,
        },
    );
    m.insert(
        "gpt-5.1",
        CodexPricing {
            input_cost_per_token: 1.25e-6,
            output_cost_per_token: 1e-5,
            cache_read_input_cost_per_token: 1.25e-7,
            display_label: None,
            long_context: None,
        },
    );
    m.insert(
        "gpt-5.1-codex",
        CodexPricing {
            input_cost_per_token: 1.25e-6,
            output_cost_per_token: 1e-5,
            cache_read_input_cost_per_token: 1.25e-7,
            display_label: None,
            long_context: None,
        },
    );
    m.insert(
        "gpt-5.1-codex-max",
        CodexPricing {
            input_cost_per_token: 1.25e-6,
            output_cost_per_token: 1e-5,
            cache_read_input_cost_per_token: 1.25e-7,
            display_label: None,
            long_context: None,
        },
    );
    m.insert(
        "gpt-5.1-codex-mini",
        CodexPricing {
            input_cost_per_token: 2.5e-7,
            output_cost_per_token: 2e-6,
            cache_read_input_cost_per_token: 2.5e-8,
            display_label: None,
            long_context: None,
        },
    );
    m.insert(
        "gpt-5.2",
        CodexPricing {
            input_cost_per_token: 1.75e-6,
            output_cost_per_token: 1.4e-5,
            cache_read_input_cost_per_token: 1.75e-7,
            display_label: None,
            long_context: None,
        },
    );
    m.insert(
        "gpt-5.2-codex",
        CodexPricing {
            input_cost_per_token: 1.75e-6,
            output_cost_per_token: 1.4e-5,
            cache_read_input_cost_per_token: 1.75e-7,
            display_label: None,
            long_context: None,
        },
    );
    m.insert(
        "gpt-5.2-pro",
        CodexPricing {
            input_cost_per_token: 2.1e-5,
            output_cost_per_token: 1.68e-4,
            cache_read_input_cost_per_token: 2.1e-5,
            display_label: None,
            long_context: None,
        },
    );
    m.insert(
        "gpt-5.3-codex",
        CodexPricing {
            input_cost_per_token: 1.75e-6,
            output_cost_per_token: 1.4e-5,
            cache_read_input_cost_per_token: 1.75e-7,
            display_label: None,
            long_context: None,
        },
    );
    m.insert(
        "gpt-5.3-codex-spark",
        CodexPricing {
            input_cost_per_token: 0.0,
            output_cost_per_token: 0.0,
            cache_read_input_cost_per_token: 0.0,
            display_label: Some("Research Preview"),
            long_context: None,
        },
    );

    // GPT-5.4 pricing (updated to match upstream 0.22)
    m.insert(
        "gpt-5.4",
        CodexPricing {
            input_cost_per_token: 2.5e-6,
            output_cost_per_token: 1.5e-5,
            cache_read_input_cost_per_token: 2.5e-7,
            display_label: None,
            long_context: None,
        },
    );
    m.insert(
        "gpt-5.4-codex",
        CodexPricing {
            input_cost_per_token: 2.5e-6,
            output_cost_per_token: 1.5e-5,
            cache_read_input_cost_per_token: 2.5e-7,
            display_label: None,
            long_context: None,
        },
    );

    // GPT-5.4 Mini pricing (updated to match upstream 0.22)
    m.insert(
        "gpt-5.4-mini",
        CodexPricing {
            input_cost_per_token: 7.5e-7,
            output_cost_per_token: 4.5e-6,
            cache_read_input_cost_per_token: 7.5e-8,
            display_label: None,
            long_context: None,
        },
    );
    m.insert(
        "gpt-5.4-mini-codex",
        CodexPricing {
            input_cost_per_token: 7.5e-7,
            output_cost_per_token: 4.5e-6,
            cache_read_input_cost_per_token: 7.5e-8,
            display_label: None,
            long_context: None,
        },
    );

    // GPT-5.4 Nano pricing (updated to match upstream 0.22)
    m.insert(
        "gpt-5.4-nano",
        CodexPricing {
            input_cost_per_token: 2e-7,
            output_cost_per_token: 1.25e-6,
            cache_read_input_cost_per_token: 2e-8,
            display_label: None,
            long_context: None,
        },
    );
    m.insert(
        "gpt-5.4-nano-codex",
        CodexPricing {
            input_cost_per_token: 2e-7,
            output_cost_per_token: 1.25e-6,
            cache_read_input_cost_per_token: 2e-8,
            display_label: None,
            long_context: None,
        },
    );

    // GPT-5.4 Pro
    m.insert(
        "gpt-5.4-pro",
        CodexPricing {
            input_cost_per_token: 3e-5,
            output_cost_per_token: 1.8e-4,
            cache_read_input_cost_per_token: 3e-5,
            display_label: None,
            long_context: None,
        },
    );
    m.insert(
        "gpt-5.5",
        CodexPricing {
            input_cost_per_token: 5e-6,
            output_cost_per_token: 3e-5,
            cache_read_input_cost_per_token: 5e-7,
            display_label: None,
            long_context: None,
        },
    );
    m.insert(
        "gpt-5.5-pro",
        CodexPricing {
            input_cost_per_token: 3e-5,
            output_cost_per_token: 1.8e-4,
            cache_read_input_cost_per_token: 3e-5,
            display_label: None,
            long_context: None,
        },
    );
    m.insert(
        "gpt-5.6-sol",
        CodexPricing {
            input_cost_per_token: 5e-6,
            output_cost_per_token: 3e-5,
            cache_read_input_cost_per_token: 5e-7,
            display_label: None,
            long_context: Some(CodexLongContextRates {
                input_cost_per_token: 1e-5,
                output_cost_per_token: 4.5e-5,
                cache_read_input_cost_per_token: 1e-6,
            }),
        },
    );
    m.insert(
        "gpt-5.6-terra",
        CodexPricing {
            input_cost_per_token: 2e-6,
            output_cost_per_token: 1.2e-5,
            cache_read_input_cost_per_token: 2e-7,
            display_label: None,
            long_context: Some(CodexLongContextRates {
                input_cost_per_token: 4e-6,
                output_cost_per_token: 1.8e-5,
                cache_read_input_cost_per_token: 4e-7,
            }),
        },
    );
    m.insert(
        "gpt-5.6-luna",
        CodexPricing {
            input_cost_per_token: 2e-7,
            output_cost_per_token: 1.2e-6,
            cache_read_input_cost_per_token: 2e-8,
            display_label: None,
            long_context: Some(CodexLongContextRates {
                input_cost_per_token: 4e-7,
                output_cost_per_token: 1.8e-6,
                cache_read_input_cost_per_token: 4e-8,
            }),
        },
    );

    m
});

const CODEX_LONG_CONTEXT_THRESHOLD: u64 = 272_000;

/// Claude model pricing table
static CLAUDE_PRICING: LazyLock<HashMap<&'static str, ClaudePricing>> = LazyLock::new(|| {
    let mut m = HashMap::new();

    // Fable 5
    m.insert(
        "claude-fable-5",
        ClaudePricing {
            input_cost_per_token: 1e-5,
            output_cost_per_token: 5e-5,
            cache_creation_input_cost_per_token: 1.25e-5,
            cache_read_input_cost_per_token: 1e-6,
            threshold_tokens: None,
            input_cost_per_token_above_threshold: None,
            output_cost_per_token_above_threshold: None,
            cache_creation_input_cost_per_token_above_threshold: None,
            cache_read_input_cost_per_token_above_threshold: None,
        },
    );

    // Haiku 4.5
    m.insert(
        "claude-haiku-4-5",
        ClaudePricing {
            input_cost_per_token: 1e-6,
            output_cost_per_token: 5e-6,
            cache_creation_input_cost_per_token: 1.25e-6,
            cache_read_input_cost_per_token: 1e-7,
            threshold_tokens: None,
            input_cost_per_token_above_threshold: None,
            output_cost_per_token_above_threshold: None,
            cache_creation_input_cost_per_token_above_threshold: None,
            cache_read_input_cost_per_token_above_threshold: None,
        },
    );
    m.insert(
        "claude-haiku-4-5-20251001",
        ClaudePricing {
            input_cost_per_token: 1e-6,
            output_cost_per_token: 5e-6,
            cache_creation_input_cost_per_token: 1.25e-6,
            cache_read_input_cost_per_token: 1e-7,
            threshold_tokens: None,
            input_cost_per_token_above_threshold: None,
            output_cost_per_token_above_threshold: None,
            cache_creation_input_cost_per_token_above_threshold: None,
            cache_read_input_cost_per_token_above_threshold: None,
        },
    );

    // Opus 4.6
    m.insert(
        "claude-opus-4-6",
        ClaudePricing {
            input_cost_per_token: 5e-6,
            output_cost_per_token: 2.5e-5,
            cache_creation_input_cost_per_token: 6.25e-6,
            cache_read_input_cost_per_token: 5e-7,
            threshold_tokens: None,
            input_cost_per_token_above_threshold: None,
            output_cost_per_token_above_threshold: None,
            cache_creation_input_cost_per_token_above_threshold: None,
            cache_read_input_cost_per_token_above_threshold: None,
        },
    );
    m.insert(
        "claude-opus-4-6-20260205",
        ClaudePricing {
            input_cost_per_token: 5e-6,
            output_cost_per_token: 2.5e-5,
            cache_creation_input_cost_per_token: 6.25e-6,
            cache_read_input_cost_per_token: 5e-7,
            threshold_tokens: None,
            input_cost_per_token_above_threshold: None,
            output_cost_per_token_above_threshold: None,
            cache_creation_input_cost_per_token_above_threshold: None,
            cache_read_input_cost_per_token_above_threshold: None,
        },
    );

    // Opus 4.7 (same pricing as Opus 4.6)
    m.insert(
        "claude-opus-4-7",
        ClaudePricing {
            input_cost_per_token: 5e-6,
            output_cost_per_token: 2.5e-5,
            cache_creation_input_cost_per_token: 6.25e-6,
            cache_read_input_cost_per_token: 5e-7,
            threshold_tokens: None,
            input_cost_per_token_above_threshold: None,
            output_cost_per_token_above_threshold: None,
            cache_creation_input_cost_per_token_above_threshold: None,
            cache_read_input_cost_per_token_above_threshold: None,
        },
    );

    // Opus 4.8 (same pricing as Opus 4.5/4.6/4.7)
    m.insert(
        "claude-opus-4-8",
        ClaudePricing {
            input_cost_per_token: 5e-6,
            output_cost_per_token: 2.5e-5,
            cache_creation_input_cost_per_token: 6.25e-6,
            cache_read_input_cost_per_token: 5e-7,
            threshold_tokens: None,
            input_cost_per_token_above_threshold: None,
            output_cost_per_token_above_threshold: None,
            cache_creation_input_cost_per_token_above_threshold: None,
            cache_read_input_cost_per_token_above_threshold: None,
        },
    );

    // Opus 4.5
    m.insert(
        "claude-opus-4-5",
        ClaudePricing {
            input_cost_per_token: 5e-6,
            output_cost_per_token: 2.5e-5,
            cache_creation_input_cost_per_token: 6.25e-6,
            cache_read_input_cost_per_token: 5e-7,
            threshold_tokens: None,
            input_cost_per_token_above_threshold: None,
            output_cost_per_token_above_threshold: None,
            cache_creation_input_cost_per_token_above_threshold: None,
            cache_read_input_cost_per_token_above_threshold: None,
        },
    );
    m.insert(
        "claude-opus-4-5-20251101",
        ClaudePricing {
            input_cost_per_token: 5e-6,
            output_cost_per_token: 2.5e-5,
            cache_creation_input_cost_per_token: 6.25e-6,
            cache_read_input_cost_per_token: 5e-7,
            threshold_tokens: None,
            input_cost_per_token_above_threshold: None,
            output_cost_per_token_above_threshold: None,
            cache_creation_input_cost_per_token_above_threshold: None,
            cache_read_input_cost_per_token_above_threshold: None,
        },
    );

    // Sonnet 4.5 (with tiered pricing at 200k tokens)
    m.insert(
        "claude-sonnet-4-5",
        ClaudePricing {
            input_cost_per_token: 3e-6,
            output_cost_per_token: 1.5e-5,
            cache_creation_input_cost_per_token: 3.75e-6,
            cache_read_input_cost_per_token: 3e-7,
            threshold_tokens: Some(200_000),
            input_cost_per_token_above_threshold: Some(6e-6),
            output_cost_per_token_above_threshold: Some(2.25e-5),
            cache_creation_input_cost_per_token_above_threshold: Some(7.5e-6),
            cache_read_input_cost_per_token_above_threshold: Some(6e-7),
        },
    );
    m.insert(
        "claude-sonnet-4-5-20250929",
        ClaudePricing {
            input_cost_per_token: 3e-6,
            output_cost_per_token: 1.5e-5,
            cache_creation_input_cost_per_token: 3.75e-6,
            cache_read_input_cost_per_token: 3e-7,
            threshold_tokens: Some(200_000),
            input_cost_per_token_above_threshold: Some(6e-6),
            output_cost_per_token_above_threshold: Some(2.25e-5),
            cache_creation_input_cost_per_token_above_threshold: Some(7.5e-6),
            cache_read_input_cost_per_token_above_threshold: Some(6e-7),
        },
    );

    // Sonnet 4.6 (same pricing as Sonnet 4.5, with 200k tier)
    m.insert(
        "claude-sonnet-4-6",
        ClaudePricing {
            input_cost_per_token: 3e-6,
            output_cost_per_token: 1.5e-5,
            cache_creation_input_cost_per_token: 3.75e-6,
            cache_read_input_cost_per_token: 3e-7,
            threshold_tokens: Some(200_000),
            input_cost_per_token_above_threshold: Some(6e-6),
            output_cost_per_token_above_threshold: Some(2.25e-5),
            cache_creation_input_cost_per_token_above_threshold: Some(7.5e-6),
            cache_read_input_cost_per_token_above_threshold: Some(6e-7),
        },
    );

    // Opus 4
    m.insert(
        "claude-opus-4-20250514",
        ClaudePricing {
            input_cost_per_token: 1.5e-5,
            output_cost_per_token: 7.5e-5,
            cache_creation_input_cost_per_token: 1.875e-5,
            cache_read_input_cost_per_token: 1.5e-6,
            threshold_tokens: None,
            input_cost_per_token_above_threshold: None,
            output_cost_per_token_above_threshold: None,
            cache_creation_input_cost_per_token_above_threshold: None,
            cache_read_input_cost_per_token_above_threshold: None,
        },
    );
    m.insert(
        "claude-opus-4-1",
        ClaudePricing {
            input_cost_per_token: 1.5e-5,
            output_cost_per_token: 7.5e-5,
            cache_creation_input_cost_per_token: 1.875e-5,
            cache_read_input_cost_per_token: 1.5e-6,
            threshold_tokens: None,
            input_cost_per_token_above_threshold: None,
            output_cost_per_token_above_threshold: None,
            cache_creation_input_cost_per_token_above_threshold: None,
            cache_read_input_cost_per_token_above_threshold: None,
        },
    );

    // Sonnet 4
    m.insert(
        "claude-sonnet-4-20250514",
        ClaudePricing {
            input_cost_per_token: 3e-6,
            output_cost_per_token: 1.5e-5,
            cache_creation_input_cost_per_token: 3.75e-6,
            cache_read_input_cost_per_token: 3e-7,
            threshold_tokens: Some(200_000),
            input_cost_per_token_above_threshold: Some(6e-6),
            output_cost_per_token_above_threshold: Some(2.25e-5),
            cache_creation_input_cost_per_token_above_threshold: Some(7.5e-6),
            cache_read_input_cost_per_token_above_threshold: Some(6e-7),
        },
    );

    m
});

fn codex_cost_from_rates(
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    input_rate: f64,
    cache_read_rate: f64,
    output_rate: f64,
) -> f64 {
    let cached = cached_input_tokens.min(input_tokens);
    let non_cached = input_tokens.saturating_sub(cached);
    (non_cached as f64) * input_rate
        + (cached as f64) * cache_read_rate
        + (output_tokens as f64) * output_rate
}

/// Cost usage pricing utilities
pub struct CostUsagePricing;

impl CostUsagePricing {
    /// Sentinel model key for model-less Codex token events.
    ///
    /// Usage remains visible under this key but is never priced as a real model
    /// (including catalog collisions with a generic "unknown" entry).
    pub const CODEX_UNATTRIBUTED_MODEL: &'static str = "unknown";

    /// True when `model` is the unattributed / model-less sentinel.
    pub fn is_codex_unattributed_model(model: &str) -> bool {
        Self::normalize_codex_model(model) == Self::CODEX_UNATTRIBUTED_MODEL
    }

    /// Normalize a Codex model name for pricing lookup
    pub fn normalize_codex_model(raw: &str) -> String {
        let mut trimmed = raw.trim().to_string();
        if trimmed.is_empty()
            || trimmed.eq_ignore_ascii_case("unknown")
            || trimmed.eq_ignore_ascii_case("unpriced")
        {
            return Self::CODEX_UNATTRIBUTED_MODEL.to_string();
        }

        // Remove "openai/" prefix
        if let Some(rest) = trimmed.strip_prefix("openai/") {
            trimmed = rest.to_string();
        }

        // Check if base model (without -codex suffix) exists in pricing
        if let Some(idx) = trimmed.find("-codex") {
            let base = &trimmed[..idx];
            if CODEX_PRICING.contains_key(base) || base == "gpt-5.6" {
                trimmed = base.to_string();
            }
        }

        let date_pattern = regex_lite::Regex::new(r"-\d{4}-\d{2}-\d{2}$").unwrap();
        if let Some(mat) = date_pattern.find(&trimmed) {
            let base = &trimmed[..mat.start()];
            if CODEX_PRICING.contains_key(base) || base == "gpt-5.6" {
                trimmed = base.to_string();
            }
        }

        if trimmed == "gpt-5.6" {
            return "gpt-5.6-sol".to_string();
        }

        trimmed
    }

    /// Get the display label for a Codex model (e.g. "Research Preview")
    pub fn codex_display_label(model: &str) -> Option<&'static str> {
        let key = Self::normalize_codex_model(model);
        CODEX_PRICING
            .get(key.as_str())
            .and_then(|p| p.display_label)
    }

    /// Normalize a Claude model name for pricing lookup
    pub fn normalize_claude_model(raw: &str) -> String {
        let mut trimmed = raw.trim().to_string();

        // Remove "anthropic." prefix
        if let Some(rest) = trimmed.strip_prefix("anthropic.") {
            trimmed = rest.to_string();
        }

        // Handle nested model names like "anthropic.claude-sonnet-4.claude-sonnet-4-20250514"
        if trimmed.contains("claude-")
            && let Some(last_dot) = trimmed.rfind('.')
        {
            let tail = &trimmed[last_dot + 1..];
            if tail.starts_with("claude-") {
                trimmed = tail.to_string();
            }
        }

        // Remove version suffix like "-v1:0"
        let version_pattern = regex_lite::Regex::new(r"-v\d+:\d+$").unwrap();
        trimmed = version_pattern.replace(&trimmed, "").to_string();

        // Try without date suffix if base exists in pricing
        let date_pattern = regex_lite::Regex::new(r"-\d{8}$").unwrap();
        if let Some(mat) = date_pattern.find(&trimmed) {
            let base = &trimmed[..mat.start()];
            if CLAUDE_PRICING.contains_key(base) {
                return base.to_string();
            }
        }

        trimmed
    }

    /// Strip Fast/priority suffix to find the base model for pricing lookup.
    ///
    /// Fast-tier models ("gpt-5.5-fast", "gpt-5.6-sol-priority") price as the
    /// standard base × multiplier. Both `codex_api_fast_multiplier` and
    /// `codex_fast_cost_usd` must use this helper so the original suffix does
    /// not leak into the base lookup (audit C4).
    pub fn codex_fast_base_model(model: &str) -> String {
        let key = Self::normalize_codex_model(model);
        key.strip_suffix("-fast")
            .or_else(|| key.strip_suffix("-priority"))
            .map(Self::normalize_codex_model)
            .unwrap_or(key)
    }

    /// Fast-tier multiplier per model (upstream 0.48.0 C4). Fast USD = Standard
    /// cost × multiplier. Returns `None` for models without a Fast lane.
    ///
    /// Multipliers: gpt-5.4, gpt-5.4-mini, gpt-5.6-sol, gpt-5.6-terra,
    /// gpt-5.6-luna → 2.0; gpt-5.5 → 2.5; else nil.
    pub fn codex_api_fast_multiplier(model: &str) -> Option<f64> {
        let base = Self::codex_fast_base_model(model);
        match base.as_str() {
            "gpt-5.4" | "gpt-5.4-mini" | "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna" => {
                Some(2.0)
            }
            "gpt-5.5" => Some(2.5),
            _ => None,
        }
    }

    /// Fast-tier cost in USD for a model (upstream 0.48.0 C4).
    ///
    /// Computes the standard cost for the BASE model (stripping fast/priority
    /// suffixes), then applies the Fast multiplier. Returns `None` when the
    /// model has no Fast lane or when long-context input exceeds the 272 000
    /// threshold guard (Fast is not offered above that).
    pub fn codex_fast_cost_usd(model: &str, input: i32, cached: i32, output: i32) -> Option<f64> {
        let multiplier = Self::codex_api_fast_multiplier(model)?;
        // Long-context guard: Fast is not offered above the threshold.
        if (input as u64) > CODEX_LONG_CONTEXT_THRESHOLD {
            return None;
        }
        let base = Self::codex_fast_base_model(model);
        let base_cost = Self::codex_cost_usd(
            &base,
            input.max(0) as u64,
            cached.max(0) as u64,
            output.max(0) as u64,
        )?;
        Some(base_cost * multiplier)
    }

    /// Calculate cost for Codex usage in USD
    pub fn codex_cost_usd(
        model: &str,
        input_tokens: u64,
        cached_input_tokens: u64,
        output_tokens: u64,
    ) -> Option<f64> {
        let key = Self::normalize_codex_model(model);
        // Model-less / deliberately unattributed usage stays unpriced even if a
        // pricing catalog later contains a colliding generic entry.
        if key == Self::CODEX_UNATTRIBUTED_MODEL {
            return None;
        }
        if let Some(pricing) = CODEX_PRICING.get(key.as_str()) {
            let (input_rate, cache_read_rate, output_rate) =
                if input_tokens > CODEX_LONG_CONTEXT_THRESHOLD {
                    if let Some(long_context) = pricing.long_context {
                        (
                            long_context.input_cost_per_token,
                            long_context.cache_read_input_cost_per_token,
                            long_context.output_cost_per_token,
                        )
                    } else {
                        (
                            pricing.input_cost_per_token,
                            pricing.cache_read_input_cost_per_token,
                            pricing.output_cost_per_token,
                        )
                    }
                } else {
                    (
                        pricing.input_cost_per_token,
                        pricing.cache_read_input_cost_per_token,
                        pricing.output_cost_per_token,
                    )
                };
            return Some(codex_cost_from_rates(
                input_tokens,
                cached_input_tokens,
                output_tokens,
                input_rate,
                cache_read_rate,
                output_rate,
            ));
        }

        let pricing = models_dev_pricing::lookup("openai", model)?;
        let use_tier = pricing
            .threshold_tokens
            .is_some_and(|threshold| input_tokens > threshold);
        Some(codex_cost_from_rates(
            input_tokens,
            cached_input_tokens,
            output_tokens,
            if use_tier {
                pricing
                    .input_cost_per_token_above_threshold
                    .unwrap_or(pricing.input_cost_per_token)
            } else {
                pricing.input_cost_per_token
            },
            if use_tier {
                pricing
                    .cache_read_input_cost_per_token_above_threshold
                    .or(pricing.cache_read_input_cost_per_token)
                    .unwrap_or(pricing.input_cost_per_token)
            } else {
                pricing
                    .cache_read_input_cost_per_token
                    .unwrap_or(pricing.input_cost_per_token)
            },
            if use_tier {
                pricing
                    .output_cost_per_token_above_threshold
                    .unwrap_or(pricing.output_cost_per_token)
            } else {
                pricing.output_cost_per_token
            },
        ))
    }

    /// Calculate cost for Claude usage in USD
    pub fn claude_cost_usd(
        model: &str,
        input_tokens: i32,
        cache_read_input_tokens: i32,
        cache_creation_input_tokens: i32,
        output_tokens: i32,
    ) -> Option<f64> {
        let key = Self::normalize_claude_model(model);
        if let Some(pricing) = CLAUDE_PRICING.get(key.as_str()) {
            /// Calculate tiered cost
            fn tiered(tokens: i32, base: f64, above: Option<f64>, threshold: Option<i32>) -> f64 {
                let tokens = tokens.max(0);
                match (threshold, above) {
                    (Some(thresh), Some(above_rate)) => {
                        let below = tokens.min(thresh);
                        let over = (tokens - thresh).max(0);
                        (below as f64) * base + (over as f64) * above_rate
                    }
                    _ => (tokens as f64) * base,
                }
            }

            let cost = tiered(
                input_tokens,
                pricing.input_cost_per_token,
                pricing.input_cost_per_token_above_threshold,
                pricing.threshold_tokens,
            ) + tiered(
                cache_read_input_tokens,
                pricing.cache_read_input_cost_per_token,
                pricing.cache_read_input_cost_per_token_above_threshold,
                pricing.threshold_tokens,
            ) + tiered(
                cache_creation_input_tokens,
                pricing.cache_creation_input_cost_per_token,
                pricing.cache_creation_input_cost_per_token_above_threshold,
                pricing.threshold_tokens,
            ) + tiered(
                output_tokens,
                pricing.output_cost_per_token,
                pricing.output_cost_per_token_above_threshold,
                pricing.threshold_tokens,
            );

            return Some(cost);
        }

        let pricing = models_dev_pricing::lookup("anthropic", model)?;
        let input_tokens = input_tokens.max(0);
        let cache_read_input_tokens = cache_read_input_tokens.max(0);
        let cache_creation_input_tokens = cache_creation_input_tokens.max(0);
        let output_tokens = output_tokens.max(0);
        let use_tier = pricing.threshold_tokens.is_some_and(|threshold| {
            (input_tokens as u64)
                + (cache_read_input_tokens as u64)
                + (cache_creation_input_tokens as u64)
                > threshold
        });
        let input_rate = if use_tier {
            pricing
                .input_cost_per_token_above_threshold
                .unwrap_or(pricing.input_cost_per_token)
        } else {
            pricing.input_cost_per_token
        };
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
        let output_rate = if use_tier {
            pricing
                .output_cost_per_token_above_threshold
                .unwrap_or(pricing.output_cost_per_token)
        } else {
            pricing.output_cost_per_token
        };
        Some(
            (input_tokens as f64) * input_rate
                + (cache_read_input_tokens as f64) * cache_read_rate
                + (cache_creation_input_tokens as f64) * cache_write_rate
                + (output_tokens as f64) * output_rate,
        )
    }

    /// Base per-token input rate for a Claude model. Exposed for callers that
    /// need a rate the standard cost function doesn't model — e.g. the usage
    /// scanner's one-hour cache-write premium, billed at 2x the input rate.
    pub fn claude_input_cost_per_token(model: &str) -> Option<f64> {
        let key = Self::normalize_claude_model(model);
        if let Some(pricing) = CLAUDE_PRICING.get(key.as_str()) {
            return Some(pricing.input_cost_per_token);
        }
        models_dev_pricing::lookup("anthropic", model).map(|p| p.input_cost_per_token)
    }

    /// Format model name for display (e.g., "claude-3.5-sonnet" → "Sonnet 3.5")
    pub fn format_model_name(model: &str) -> String {
        let lower = model.to_lowercase();

        // GPT models: format as "GPT-{version}[ Mini| Nano]"
        if lower.contains("gpt-") {
            let version = regex_lite::Regex::new(r"gpt-(\d+(?:\.\d+)?)")
                .ok()
                .and_then(|re| re.captures(&lower))
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string());

            let suffix = if lower.contains("nano") {
                " Nano"
            } else if lower.contains("mini") {
                " Mini"
            } else {
                ""
            };

            return match version {
                Some(v) => format!("GPT-{}{}", v, suffix),
                None => model.to_string(),
            };
        }

        // Claude models: extract version and family
        let version = regex_lite::Regex::new(r"(\d+(?:\.\d+)?)")
            .ok()
            .and_then(|re| re.find(&lower))
            .map(|m| m.as_str().to_string());

        let family = if lower.contains("opus") {
            "Opus"
        } else if lower.contains("sonnet") {
            "Sonnet"
        } else if lower.contains("haiku") {
            "Haiku"
        } else {
            return model.to_string();
        };

        match version {
            Some(v) => format!("{} {}", family, v),
            None => family.to_string(),
        }
    }
}

#[cfg(test)]
#[path = "cost_pricing_tests.rs"]
mod tests;
