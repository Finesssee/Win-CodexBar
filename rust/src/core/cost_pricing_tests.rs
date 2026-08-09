use super::*;

#[test]
fn test_normalize_codex_model() {
    assert_eq!(CostUsagePricing::normalize_codex_model("gpt-5"), "gpt-5");
    assert_eq!(
        CostUsagePricing::normalize_codex_model("openai/gpt-5"),
        "gpt-5"
    );
    assert_eq!(
        CostUsagePricing::normalize_codex_model("gpt-5-codex"),
        "gpt-5"
    );
    assert_eq!(
        CostUsagePricing::normalize_codex_model(""),
        CostUsagePricing::CODEX_UNATTRIBUTED_MODEL
    );
    assert_eq!(
        CostUsagePricing::normalize_codex_model("unknown"),
        CostUsagePricing::CODEX_UNATTRIBUTED_MODEL
    );
}

#[test]
fn unattributed_codex_usage_stays_unpriced() {
    assert!(
        CostUsagePricing::codex_cost_usd(CostUsagePricing::CODEX_UNATTRIBUTED_MODEL, 1_000, 0, 500)
            .is_none()
    );
    assert!(CostUsagePricing::is_codex_unattributed_model("unknown"));
    assert!(CostUsagePricing::is_codex_unattributed_model("  "));
}

#[test]
fn test_normalize_claude_model() {
    assert_eq!(
        CostUsagePricing::normalize_claude_model("claude-sonnet-4-5"),
        "claude-sonnet-4-5"
    );
    assert_eq!(
        CostUsagePricing::normalize_claude_model("anthropic.claude-sonnet-4-5"),
        "claude-sonnet-4-5"
    );
}

#[test]
fn test_codex_cost() {
    let cost = CostUsagePricing::codex_cost_usd("gpt-5", 1000, 0, 500).unwrap();
    assert!((cost - 0.00625).abs() < 1e-10);
}

#[test]
fn test_claude_cost() {
    assert!(
        CostUsagePricing::claude_cost_usd("claude-haiku-4-5-20251001", 1000, 0, 0, 500).is_some()
    );
}

#[test]
fn test_opus_4_8_cost() {
    let cost = CostUsagePricing::claude_cost_usd("claude-opus-4-8", 1_000, 0, 0, 500).unwrap();
    assert!((cost - 0.0175).abs() < 1e-10);
}

#[test]
fn test_fable_5_cost() {
    let cost = CostUsagePricing::claude_cost_usd("claude-fable-5", 1_000, 0, 0, 500).unwrap();
    assert!((cost - 0.035).abs() < 1e-10);
}

#[test]
fn test_claude_input_cost_per_token() {
    assert_eq!(
        CostUsagePricing::claude_input_cost_per_token("claude-opus-4-8"),
        Some(5e-6)
    );
    assert_eq!(
        CostUsagePricing::claude_input_cost_per_token("claude-fable-5"),
        Some(1e-5)
    );
    assert_eq!(
        CostUsagePricing::claude_input_cost_per_token("totally-unknown-model"),
        None
    );
}

#[test]
fn test_format_model_name() {
    assert_eq!(
        CostUsagePricing::format_model_name("claude-3.5-sonnet"),
        "Sonnet 3.5"
    );
    assert_eq!(
        CostUsagePricing::format_model_name("claude-opus-4"),
        "Opus 4"
    );
    assert_eq!(CostUsagePricing::format_model_name("gpt-5"), "GPT-5");
}

#[test]
fn test_gpt54_mini_cost() {
    let cost = CostUsagePricing::codex_cost_usd("gpt-5.4-mini", 1000, 0, 500).unwrap();
    assert!((cost - 0.003).abs() < 1e-10);
}

#[test]
fn test_gpt54_nano_cost() {
    let cost = CostUsagePricing::codex_cost_usd("gpt-5.4-nano", 1000, 0, 500).unwrap();
    assert!((cost - 0.000825).abs() < 1e-10);
}

#[test]
fn test_normalize_gpt54_codex() {
    assert_eq!(
        CostUsagePricing::normalize_codex_model("gpt-5.4-mini-codex"),
        "gpt-5.4-mini"
    );
}

#[test]
fn test_gpt55_pricing() {
    assert_eq!(
        CostUsagePricing::normalize_codex_model("openai/gpt-5.5-2026-04-23"),
        "gpt-5.5"
    );
    assert_eq!(
        CostUsagePricing::normalize_codex_model("gpt-5.5-pro-2026-04-23"),
        "gpt-5.5-pro"
    );
    let cost = CostUsagePricing::codex_cost_usd("gpt-5.5", 1000, 500, 500).unwrap();
    assert!((cost - 0.01775).abs() < 1e-10);
}

#[test]
fn test_format_gpt54_mini() {
    assert_eq!(
        CostUsagePricing::format_model_name("gpt-5.4-mini"),
        "GPT-5.4 Mini"
    );
}

#[test]
fn test_opus_4_7_cost() {
    assert!(CostUsagePricing::claude_cost_usd("claude-opus-4-7", 1000, 0, 0, 500).is_some());
}

#[test]
fn test_sonnet_4_6_cost() {
    assert!(CostUsagePricing::claude_cost_usd("claude-sonnet-4-6", 1000, 0, 0, 500).is_some());
}

#[test]
fn test_gpt5_pro_cost() {
    let cost = CostUsagePricing::codex_cost_usd("gpt-5-pro", 1000, 0, 500).unwrap();
    assert!((cost - 0.075).abs() < 1e-10);
}

#[test]
fn test_gpt56_standard_pricing() {
    for (model, expected) in [
        ("gpt-5.6-sol", 0.0332),
        ("gpt-5.6-terra", 0.01328),
        ("gpt-5.6-luna", 0.001328),
    ] {
        let cost = CostUsagePricing::codex_cost_usd(model, 1_000, 400, 1_000);
        assert!((cost.unwrap() - expected).abs() < 1e-10, "{model}");
    }
}

#[test]
fn test_gpt56_long_context_pricing() {
    for (model, expected) in [
        ("gpt-5.6-sol", 45.272001),
        ("gpt-5.6-terra", 18.1088004),
        ("gpt-5.6-luna", 1.81088004),
    ] {
        let cost = CostUsagePricing::codex_cost_usd(model, 272_001, 272_001, 1_000_000);
        assert!((cost.unwrap() - expected).abs() < 1e-10, "{model}");
    }
}

#[test]
fn test_gpt56_context_threshold_is_exclusive() {
    for (model, expected) in [
        ("gpt-5.6-sol", 0.136),
        ("gpt-5.6-terra", 0.0544),
        ("gpt-5.6-luna", 0.00544),
    ] {
        let cost = CostUsagePricing::codex_cost_usd(model, 272_000, 272_000, 0);
        assert!((cost.unwrap() - expected).abs() < 1e-10, "{model}");
    }
}

#[test]
fn test_normalize_gpt56_aliases() {
    for model in [
        "gpt-5.6",
        "openai/gpt-5.6",
        "gpt-5.6-codex",
        "gpt-5.6-2099-01-01",
        "openai/gpt-5.6-codex-2099-01-01",
    ] {
        assert_eq!(
            CostUsagePricing::normalize_codex_model(model),
            "gpt-5.6-sol",
            "{model}"
        );
    }
}

#[test]
fn test_codex_display_label() {
    assert_eq!(
        CostUsagePricing::codex_display_label("gpt-5.3-codex-spark"),
        Some("Research Preview")
    );
    assert_eq!(CostUsagePricing::codex_display_label("gpt-5.4"), None);
}

#[test]
fn test_codex_fast_multiplier() {
    assert_eq!(
        CostUsagePricing::codex_api_fast_multiplier("gpt-5.6-sol"),
        Some(2.0)
    );
    assert_eq!(
        CostUsagePricing::codex_api_fast_multiplier("gpt-5.6-terra"),
        Some(2.0)
    );
    assert_eq!(
        CostUsagePricing::codex_api_fast_multiplier("gpt-5.6-luna"),
        Some(2.0)
    );
    assert_eq!(
        CostUsagePricing::codex_api_fast_multiplier("gpt-5.4"),
        Some(2.0)
    );
    assert_eq!(
        CostUsagePricing::codex_api_fast_multiplier("gpt-5.5"),
        Some(2.5)
    );
    assert_eq!(
        CostUsagePricing::codex_api_fast_multiplier("gpt-5.6-sol-fast"),
        Some(2.0)
    );
    assert_eq!(CostUsagePricing::codex_api_fast_multiplier("unknown"), None);
}

#[test]
fn test_codex_fast_cost_is_double_standard() {
    let standard = CostUsagePricing::codex_cost_usd("gpt-5.6-sol", 1000, 0, 500).unwrap();
    let fast = CostUsagePricing::codex_fast_cost_usd("gpt-5.6-sol", 1000, 0, 500).unwrap();
    assert!((fast - standard * 2.0).abs() < 1e-10);
}

#[test]
fn test_codex_fast_cost_none_above_long_context_threshold() {
    // Input above 272_000 → None (Fast not offered)
    assert_eq!(
        CostUsagePricing::codex_fast_cost_usd("gpt-5.6-sol", 272_001, 0, 100),
        None
    );
}

#[test]
fn test_codex_fast_cost_usd_suffixed_models_resolve_to_base() {
    // gpt-5.5-fast → base gpt-5.5 → 2.5x multiplier
    let base = CostUsagePricing::codex_cost_usd("gpt-5.5", 1000, 0, 500).unwrap();
    let fast = CostUsagePricing::codex_fast_cost_usd("gpt-5.5-fast", 1000, 0, 500).unwrap();
    assert!(
        (fast - base * 2.5).abs() < 1e-10,
        "gpt-5.5-fast should be gpt-5.5 × 2.5"
    );

    // gpt-5.6-sol-priority → base gpt-5.6-sol → 2.0x multiplier
    let sol_base = CostUsagePricing::codex_cost_usd("gpt-5.6-sol", 1000, 400, 1000).unwrap();
    let sol_fast =
        CostUsagePricing::codex_fast_cost_usd("gpt-5.6-sol-priority", 1000, 400, 1000).unwrap();
    assert!(
        (sol_fast - sol_base * 2.0).abs() < 1e-10,
        "gpt-5.6-sol-priority should be gpt-5.6-sol × 2.0"
    );
}

#[test]
fn test_codex_fast_cost_usd_base_model_unsuffixed() {
    // Unsuffixed base models still resolve to themselves.
    assert_eq!(
        CostUsagePricing::codex_fast_base_model("gpt-5.6-terra"),
        "gpt-5.6-terra"
    );
    assert_eq!(
        CostUsagePricing::codex_fast_base_model("gpt-5.5"),
        "gpt-5.5"
    );
    // Unknown models return normalized original.
    assert_eq!(
        CostUsagePricing::codex_fast_base_model("my-custom-model"),
        "my-custom-model"
    );
}
