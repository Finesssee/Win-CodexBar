//! Provider implementations

#![allow(dead_code)]

pub mod abacus;
pub mod aiand;
pub mod alibaba;
pub mod alibabatokenplan;
pub mod amp;
pub mod antigravity;
pub mod augment;
pub mod azureopenai;
pub mod bedrock;
pub mod chutes;
pub mod claude;
pub mod clinepass;
pub mod codebuddy;
pub mod codebuff;
pub mod codex;
pub mod commandcode;
pub mod copilot;
pub mod crof;
pub mod crossmodel;
pub mod cursor;
pub mod deepgram;
pub mod deepinfra;
pub mod deepseek;
pub mod devin;
pub mod doubao;
pub mod elevenlabs;
pub mod factory;
pub mod gemini;
pub mod grok;
pub mod groq;
pub mod infini;
pub mod jetbrains;
pub mod kilo;
pub mod kimi;
pub mod kimik2;
pub mod kiro;
pub mod litellm;
pub mod llmproxy;
pub mod longcat;
pub mod manus;
pub mod mimo;
pub mod minimax;
pub mod mistral;
pub mod nanogpt;
pub mod neuralwatt;
pub mod notion;
pub mod ollama;
pub mod openai;
pub mod openaiapi;
pub mod opencode;
pub mod opencodego;
pub mod openrouter;
pub mod perplexity;
pub mod poe;
pub mod qoder;
pub mod qwencloud;
pub mod sakana;
pub mod stepfun;
pub mod sub2api;
pub mod t3chat;
pub mod venice;
pub mod vertexai;
pub mod warp;
pub mod wayfinder;
pub mod windsurf;
pub mod xai;
pub mod zai;
pub mod zed;
pub mod zenmux;
pub mod zoommate;

// Re-export provider implementations
pub use abacus::AbacusProvider;
pub use aiand::AiAndProvider;
pub use alibaba::{AlibabaProvider, AlibabaRegion};
pub use alibabatokenplan::{AlibabaTokenPlanProvider, AlibabaTokenPlanRegion};
pub use amp::AmpProvider;
pub use antigravity::AntigravityProvider;
pub use augment::AugmentProvider;
pub use azureopenai::AzureOpenAIProvider;
pub use bedrock::BedrockProvider;
pub use chutes::ChutesProvider;
pub use claude::ClaudeProvider;
pub use clinepass::ClinePassProvider;
pub use codebuddy::CodeBuddyProvider;
pub use codebuff::CodebuffProvider;
pub use codex::CodexProvider;
pub use commandcode::CommandCodeProvider;
pub use copilot::CopilotProvider;
pub use crof::CrofProvider;
pub use crossmodel::CrossModelProvider;
pub use cursor::CursorProvider;
pub use deepgram::DeepgramProvider;
pub use deepinfra::DeepInfraProvider;
pub use deepseek::DeepSeekProvider;
pub use devin::DevinProvider;
pub use doubao::DoubaoProvider;
pub use elevenlabs::ElevenLabsProvider;
pub use factory::FactoryProvider;
pub use gemini::GeminiProvider;
pub use grok::GrokProvider;
pub use groq::GroqProvider;
pub use infini::InfiniProvider;
pub use jetbrains::JetBrainsProvider;
pub use kilo::KiloProvider;
pub use kimi::KimiProvider;
pub use kimik2::KimiK2Provider;
pub use kiro::KiroProvider;
pub use litellm::LiteLLMProvider;
pub use llmproxy::LLMProxyProvider;
pub use longcat::LongCatProvider;
pub use manus::ManusProvider;
pub use mimo::MiMoProvider;
pub use minimax::{MiniMaxProvider, MiniMaxRegion};
pub use mistral::MistralProvider;
pub use nanogpt::NanoGPTProvider;
pub use neuralwatt::NeuralwattProvider;
pub use notion::NotionProvider;
pub use ollama::OllamaProvider;
pub use openaiapi::OpenAIApiProvider;
pub use opencode::OpenCodeProvider;
pub use opencodego::OpenCodeGoProvider;
pub use openrouter::OpenRouterProvider;
pub use perplexity::PerplexityProvider;
pub use poe::PoeProvider;
pub use qoder::QoderProvider;
pub use qwencloud::QwenCloudProvider;
pub use sakana::SakanaProvider;
pub use stepfun::StepFunProvider;
pub use sub2api::Sub2ApiProvider;
pub use t3chat::T3ChatProvider;
pub use venice::VeniceProvider;
pub use vertexai::VertexAIProvider;
pub use warp::WarpProvider;
pub use wayfinder::WayfinderProvider;
pub use windsurf::WindsurfProvider;
pub use xai::XaiProvider;
pub use zai::ZaiProvider;
pub use zed::ZedProvider;
pub use zenmux::ZenMuxProvider;
pub use zoommate::ZoomMateProvider;

pub(crate) fn browser_cookie_header(
    domains: &[&str],
) -> Result<String, crate::core::ProviderError> {
    crate::browser::cookies::get_cookie_header_for_domains(domains)
        .map_err(map_browser_cookie_error)
}

pub(crate) fn browser_cookies_for_domain(
    domain: &str,
) -> Result<Vec<crate::browser::cookies::Cookie>, crate::core::ProviderError> {
    crate::browser::cookies::get_cookies_for_domain(domain).map_err(map_browser_cookie_error)
}

fn map_browser_cookie_error(
    error: crate::browser::cookies::CookieError,
) -> crate::core::ProviderError {
    match error {
        crate::browser::cookies::CookieError::BrowserNotInstalled
        | crate::browser::cookies::CookieError::NotFound(_) => {
            crate::core::ProviderError::NoCookies
        }
        _ => crate::core::ProviderError::Other(format!("Failed to read browser cookies: {error}")),
    }
}

pub(crate) fn resolve_api_key(
    explicit: Option<&str>,
    credential_target: &str,
    env_names: &[&str],
) -> Result<String, crate::core::ProviderError> {
    if let Some(key) = explicit
        && !key.trim().is_empty()
    {
        return Ok(key.trim().to_string());
    }
    if let Ok(entry) = keyring::Entry::new(credential_target, "api_key")
        && let Ok(key) = entry.get_password()
        && !key.trim().is_empty()
    {
        return Ok(key);
    }
    for env in env_names {
        if let Ok(key) = std::env::var(env)
            && !key.trim().is_empty()
        {
            return Ok(key);
        }
    }
    Err(crate::core::ProviderError::NotInstalled(format!(
        "API key not found. Set {} in Preferences or environment.",
        env_names.join(" / ")
    )))
}

pub(crate) fn validated_https_url(
    raw: &str,
    label: &str,
) -> Result<reqwest::Url, crate::core::ProviderError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(crate::core::ProviderError::Other(format!(
            "{label} URL is empty"
        )));
    }
    let lower = trimmed.to_ascii_lowercase();
    if ["%2f", "%5c", "%3f", "%23", "%40", "%3a"]
        .iter()
        .any(|encoded| lower.contains(encoded))
    {
        return Err(crate::core::ProviderError::Other(format!(
            "{label} URL must not contain encoded host delimiters"
        )));
    }
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let url = reqwest::Url::parse(&candidate)
        .map_err(|e| crate::core::ProviderError::Other(format!("Invalid {label} URL: {e}")))?;
    let host = url.host_str().ok_or_else(|| {
        crate::core::ProviderError::Other(format!("{label} URL must include a host"))
    })?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || host.contains('%')
        || host.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(crate::core::ProviderError::Other(format!(
            "{label} URL must use HTTPS without user info or encoded host tricks"
        )));
    }
    Ok(url)
}

/// Extract the first semver-like substring (`\d+(?:\.\d+)+`) from `s`.
pub(crate) fn extract_semver(s: &str) -> Option<String> {
    let re = regex_lite::Regex::new(r"(\d+(?:\.\d+)+)").ok()?;
    re.find(s).map(|m| m.as_str().to_string())
}

/// Extract the first capture group of `pattern` from `text` as an `f64`.
pub(crate) fn extract_number(pattern: &str, text: &str) -> Option<f64> {
    let re = regex_lite::Regex::new(pattern).ok()?;
    re.captures(text)?.get(1)?.as_str().parse().ok()
}

/// Extract a `renewAt`/`renew_at` timestamp from a JS/JSON-ish payload.
///
/// Accepts either a numeric epoch (seconds or milliseconds) or an RFC 3339
/// date string. Returns the parsed instant in UTC.
pub(crate) fn extract_renewal(text: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let re = regex_lite::Regex::new(
        r#"(?:"renewAt"|"renew_at"|renewAt|renew_at)\s*[:=]\s*"?([^",}\s]+)"?"#,
    )
    .ok()?;
    let raw = re.captures(text)?.get(1)?.as_str().trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(number) = raw.parse::<f64>()
        && number.is_finite()
        && number > 0.0
    {
        let seconds = if number > 10_000_000_000.0 {
            number / 1000.0
        } else {
            number
        };
        return chrono::DateTime::<chrono::Utc>::from_timestamp(seconds as i64, 0);
    }
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}
