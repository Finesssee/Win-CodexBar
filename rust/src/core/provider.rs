//! Provider trait - defines the interface all providers must implement

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

use super::ProviderFetchResult;

/// Unique identifier for a provider
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Codex,
    Claude,
    Cursor,
    Factory,
    Gemini,
    Antigravity,
    Copilot,
    Zai,
    MiniMax,
    Kiro,
    VertexAI,
    Augment,
    OpenCode,
    Kimi,
    KimiK2,
    Amp,
    Warp,
    Ollama,
    AzureOpenAI,
    T3Chat,
    OpenRouter,
    JetBrains,
    Alibaba,
    AlibabaTokenPlan,
    NanoGPT,
    Infini,
    Perplexity,
    Abacus,
    Mistral,
    OpenCodeGo,
    Kilo,
    Bedrock,
    Codebuff,
    DeepSeek,
    DeepInfra,
    AiAnd,
    Windsurf,
    Manus,
    MiMo,
    Doubao,
    CommandCode,
    Crof,
    StepFun,
    Venice,
    OpenAIApi,
    Grok,
    ElevenLabs,
    Deepgram,
    Groq,
    LLMProxy,
    Chutes,
    LiteLLM,
    Poe,
    Devin,
    Zed,
    CrossModel,
    Qoder,
    CodeBuddy,
    Sakana,
    Sub2Api,
    Wayfinder,
    ZenMux,
    ClinePass,
    LongCat,
    Neuralwatt,
    ZoomMate,
    QwenCloud,
    Notion,
    Xai,
}

impl ProviderId {
    /// Get all provider IDs
    pub fn all() -> &'static [ProviderId] {
        &[
            ProviderId::Codex,
            ProviderId::Claude,
            ProviderId::Cursor,
            ProviderId::Factory,
            ProviderId::Gemini,
            ProviderId::Antigravity,
            ProviderId::Copilot,
            ProviderId::Zai,
            ProviderId::MiniMax,
            ProviderId::Kiro,
            ProviderId::VertexAI,
            ProviderId::Augment,
            ProviderId::OpenCode,
            ProviderId::Kimi,
            ProviderId::KimiK2,
            ProviderId::Amp,
            ProviderId::Warp,
            ProviderId::Ollama,
            ProviderId::AzureOpenAI,
            ProviderId::T3Chat,
            ProviderId::OpenRouter,
            ProviderId::JetBrains,
            ProviderId::Alibaba,
            ProviderId::AlibabaTokenPlan,
            ProviderId::NanoGPT,
            ProviderId::Infini,
            ProviderId::Perplexity,
            ProviderId::Abacus,
            ProviderId::Mistral,
            ProviderId::OpenCodeGo,
            ProviderId::Kilo,
            ProviderId::Bedrock,
            ProviderId::Codebuff,
            ProviderId::DeepSeek,
            ProviderId::DeepInfra,
            ProviderId::AiAnd,
            ProviderId::Windsurf,
            ProviderId::Manus,
            ProviderId::MiMo,
            ProviderId::Doubao,
            ProviderId::CommandCode,
            ProviderId::Crof,
            ProviderId::StepFun,
            ProviderId::Venice,
            ProviderId::OpenAIApi,
            ProviderId::Grok,
            ProviderId::ElevenLabs,
            ProviderId::Deepgram,
            ProviderId::Groq,
            ProviderId::LLMProxy,
            ProviderId::Chutes,
            ProviderId::LiteLLM,
            ProviderId::Poe,
            ProviderId::Devin,
            ProviderId::Zed,
            ProviderId::CrossModel,
            ProviderId::Qoder,
            ProviderId::CodeBuddy,
            ProviderId::Sakana,
            ProviderId::Sub2Api,
            ProviderId::Wayfinder,
            ProviderId::ZenMux,
            ProviderId::ClinePass,
            ProviderId::LongCat,
            ProviderId::Neuralwatt,
            ProviderId::ZoomMate,
            ProviderId::QwenCloud,
            ProviderId::Notion,
            ProviderId::Xai,
        ]
    }

    /// Get the CLI name for this provider
    pub fn cli_name(&self) -> &'static str {
        match self {
            ProviderId::Codex => "codex",
            ProviderId::Claude => "claude",
            ProviderId::Cursor => "cursor",
            ProviderId::Factory => "factory",
            ProviderId::Gemini => "gemini",
            ProviderId::Antigravity => "antigravity",
            ProviderId::Copilot => "copilot",
            ProviderId::Zai => "zai",
            ProviderId::MiniMax => "minimax",
            ProviderId::Kiro => "kiro",
            ProviderId::VertexAI => "vertexai",
            ProviderId::Augment => "augment",
            ProviderId::OpenCode => "opencode",
            ProviderId::Kimi => "kimi",
            ProviderId::KimiK2 => "kimik2",
            ProviderId::Amp => "amp",
            ProviderId::Warp => "warp",
            ProviderId::Ollama => "ollama",
            ProviderId::AzureOpenAI => "azureopenai",
            ProviderId::T3Chat => "t3chat",
            ProviderId::OpenRouter => "openrouter",
            ProviderId::JetBrains => "jetbrains",
            ProviderId::Alibaba => "alibaba",
            ProviderId::AlibabaTokenPlan => "alibabatokenplan",
            ProviderId::NanoGPT => "nanogpt",
            ProviderId::Infini => "infini",
            ProviderId::Perplexity => "perplexity",
            ProviderId::Abacus => "abacus",
            ProviderId::Mistral => "mistral",
            ProviderId::OpenCodeGo => "opencodego",
            ProviderId::Kilo => "kilo",
            ProviderId::Bedrock => "bedrock",
            ProviderId::Codebuff => "codebuff",
            ProviderId::DeepSeek => "deepseek",
            ProviderId::DeepInfra => "deepinfra",
            ProviderId::AiAnd => "aiand",
            ProviderId::Windsurf => "windsurf",
            ProviderId::Manus => "manus",
            ProviderId::MiMo => "mimo",
            ProviderId::Doubao => "doubao",
            ProviderId::CommandCode => "commandcode",
            ProviderId::Crof => "crof",
            ProviderId::StepFun => "stepfun",
            ProviderId::Venice => "venice",
            ProviderId::OpenAIApi => "openaiapi",
            ProviderId::Grok => "grok",
            ProviderId::ElevenLabs => "elevenlabs",
            ProviderId::Deepgram => "deepgram",
            ProviderId::Groq => "groq",
            ProviderId::LLMProxy => "llmproxy",
            ProviderId::Chutes => "chutes",
            ProviderId::LiteLLM => "litellm",
            ProviderId::Poe => "poe",
            ProviderId::Devin => "devin",
            ProviderId::Zed => "zed",
            ProviderId::CrossModel => "crossmodel",
            ProviderId::Qoder => "qoder",
            ProviderId::CodeBuddy => "codebuddy",
            ProviderId::Sakana => "sakana",
            ProviderId::Sub2Api => "sub2api",
            ProviderId::Wayfinder => "wayfinder",
            ProviderId::ZenMux => "zenmux",
            ProviderId::ClinePass => "clinepass",
            ProviderId::LongCat => "longcat",
            ProviderId::Neuralwatt => "neuralwatt",
            ProviderId::ZoomMate => "zoommate",
            ProviderId::QwenCloud => "qwen-cloud",
            ProviderId::Notion => "notion",
            ProviderId::Xai => "xai",
        }
    }

    /// Get the display name for this provider
    pub fn display_name(&self) -> &'static str {
        match self {
            ProviderId::Codex => "Codex",
            ProviderId::Claude => "Claude",
            ProviderId::Cursor => "Cursor",
            ProviderId::Factory => "Factory",
            ProviderId::Gemini => "Gemini",
            ProviderId::Antigravity => "Antigravity",
            ProviderId::Copilot => "Copilot",
            ProviderId::Zai => "z.ai",
            ProviderId::MiniMax => "MiniMax",
            ProviderId::Kiro => "Kiro",
            ProviderId::VertexAI => "Vertex AI",
            ProviderId::Augment => "Augment",
            ProviderId::OpenCode => "OpenCode",
            ProviderId::Kimi => "Kimi",
            // Soft-removed (upstream #2254); still resolvable via CLI for legacy configs.
            ProviderId::KimiK2 => "Kimi K2 (removed)",
            ProviderId::Amp => "Amp",
            ProviderId::Warp => "Warp",
            ProviderId::Ollama => "Ollama",
            ProviderId::AzureOpenAI => "Azure OpenAI",
            ProviderId::T3Chat => "T3 Chat",
            ProviderId::OpenRouter => "OpenRouter",
            ProviderId::JetBrains => "JetBrains AI",
            ProviderId::Alibaba => "Alibaba",
            ProviderId::AlibabaTokenPlan => "Alibaba Token Plan",
            ProviderId::NanoGPT => "NanoGPT",
            ProviderId::Infini => "Infini",
            ProviderId::Perplexity => "Perplexity",
            ProviderId::Abacus => "Abacus AI",
            ProviderId::Mistral => "Mistral",
            ProviderId::OpenCodeGo => "OpenCode Go",
            ProviderId::Kilo => "Kilo",
            ProviderId::Bedrock => "AWS Bedrock",
            ProviderId::Codebuff => "Codebuff",
            ProviderId::DeepSeek => "DeepSeek",
            ProviderId::DeepInfra => "DeepInfra",
            ProviderId::AiAnd => "ai&",
            ProviderId::Windsurf => "Windsurf",
            ProviderId::Manus => "Manus",
            ProviderId::MiMo => "Xiaomi MiMo",
            ProviderId::Doubao => "Doubao",
            ProviderId::CommandCode => "Command Code",
            ProviderId::Crof => "Crof",
            ProviderId::StepFun => "StepFun",
            ProviderId::Venice => "Venice",
            ProviderId::OpenAIApi => "OpenAI API",
            ProviderId::Grok => "Grok",
            ProviderId::ElevenLabs => "ElevenLabs",
            ProviderId::Deepgram => "Deepgram",
            ProviderId::Groq => "Groq",
            ProviderId::LLMProxy => "LLM Proxy",
            ProviderId::Chutes => "Chutes",
            ProviderId::LiteLLM => "LiteLLM",
            ProviderId::Poe => "Poe",
            ProviderId::Devin => "Devin",
            ProviderId::Zed => "Zed",
            // Soft-removed (upstream #2254); still resolvable via CLI for legacy configs.
            ProviderId::CrossModel => "CrossModel (removed)",
            ProviderId::Qoder => "Qoder",
            ProviderId::CodeBuddy => "CodeBuddy",
            ProviderId::Sakana => "Sakana AI",
            ProviderId::Sub2Api => "sub2api",
            ProviderId::Wayfinder => "Wayfinder",
            ProviderId::ZenMux => "ZenMux",
            ProviderId::ClinePass => "ClinePass",
            ProviderId::LongCat => "LongCat",
            ProviderId::Neuralwatt => "Neuralwatt",
            ProviderId::ZoomMate => "ZoomMate",
            ProviderId::QwenCloud => "Qwen Cloud",
            ProviderId::Notion => "Notion AI",
            ProviderId::Xai => "xAI",
        }
    }

    /// Get the cookie domain for this provider.
    /// Returns the domain used for cookie extraction, or None if the provider
    /// doesn't use cookies for authentication.
    pub fn cookie_domain(&self) -> Option<&'static str> {
        match self {
            ProviderId::Claude => Some("claude.ai"),
            ProviderId::Cursor => Some("cursor.com"),
            ProviderId::Factory => Some("app.factory.ai"),
            ProviderId::Codex => Some("chatgpt.com"),
            ProviderId::Gemini => Some("aistudio.google.com"),
            ProviderId::Kiro => Some("kiro.dev"),
            ProviderId::Kimi => Some("kimi.moonshot.cn"),
            ProviderId::KimiK2 => Some("platform.moonshot.cn"),
            ProviderId::MiniMax => Some("platform.minimax.io"),
            ProviderId::OpenCode => Some("opencode.ai"),
            ProviderId::Augment => Some("app.augmentcode.com"),
            ProviderId::Amp => Some("sourcegraph.com"),
            ProviderId::Antigravity => Some("antigravity.ai"),
            ProviderId::Alibaba => {
                Some(crate::providers::AlibabaRegion::Singapore.primary_cookie_domain())
            }
            ProviderId::AlibabaTokenPlan => Some("bailian.console.aliyun.com"),
            ProviderId::Ollama => Some("ollama.com"),
            ProviderId::T3Chat => Some("t3.chat"),
            ProviderId::Perplexity => Some("perplexity.ai"),
            ProviderId::Abacus => Some("apps.abacus.ai"),
            ProviderId::Mistral => Some("admin.mistral.ai"),
            ProviderId::OpenCodeGo => Some("opencode.ai"),
            ProviderId::Manus => Some("manus.im"),
            ProviderId::MiMo => Some("platform.xiaomimimo.com"),
            ProviderId::CommandCode => Some("commandcode.ai"),
            ProviderId::Grok => Some("grok.com"),
            ProviderId::Qoder => Some("qoder.com"),
            ProviderId::CodeBuddy => Some("codebuddy.cn"),
            ProviderId::Sakana => Some("console.sakana.ai"),
            ProviderId::LongCat => Some("longcat.chat"),
            // Token-based providers (don't use cookies)
            ProviderId::Copilot => None,
            ProviderId::Zai => None,
            ProviderId::VertexAI => None,
            ProviderId::JetBrains => None,
            ProviderId::Warp => None,
            ProviderId::AzureOpenAI => None,
            ProviderId::OpenRouter => None,
            ProviderId::NanoGPT => None,
            ProviderId::Infini => None,
            ProviderId::Kilo => None,
            ProviderId::Bedrock => None,
            ProviderId::Codebuff => None,
            ProviderId::DeepSeek => None,
            ProviderId::DeepInfra => None,
            ProviderId::AiAnd => None,
            ProviderId::Windsurf => None,
            ProviderId::Doubao => None,
            ProviderId::Crof => None,
            ProviderId::StepFun => None,
            ProviderId::Venice => None,
            ProviderId::OpenAIApi => None,
            ProviderId::ElevenLabs => None,
            ProviderId::Deepgram => None,
            ProviderId::Groq => None,
            ProviderId::LLMProxy => None,
            ProviderId::Chutes => None,
            ProviderId::LiteLLM => None,
            ProviderId::Poe => None,
            ProviderId::Devin => None,
            ProviderId::Zed => None,
            ProviderId::CrossModel => None,
            ProviderId::Sub2Api => None,
            ProviderId::Wayfinder => None,
            ProviderId::ZenMux => None,
            ProviderId::ClinePass => None,
            ProviderId::Neuralwatt => None,
            ProviderId::ZoomMate => Some("zoommate.zoom.us"),
            ProviderId::QwenCloud => Some("qwencloud.com"),
            ProviderId::Notion => Some("app.notion.com"),
            ProviderId::Xai => None,
        }
    }

    /// Parse from CLI name string
    pub fn from_cli_name(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "codex" | "openai" => Some(ProviderId::Codex),
            "claude" | "anthropic" => Some(ProviderId::Claude),
            "cursor" => Some(ProviderId::Cursor),
            "factory" | "droid" => Some(ProviderId::Factory),
            "gemini" | "google" => Some(ProviderId::Gemini),
            "antigravity" | "agy" => Some(ProviderId::Antigravity),
            "copilot" | "github" => Some(ProviderId::Copilot),
            "zai" | "z.ai" => Some(ProviderId::Zai),
            "minimax" => Some(ProviderId::MiniMax),
            "kiro" | "aws" => Some(ProviderId::Kiro),
            "vertexai" | "vertex" | "vertex ai" => Some(ProviderId::VertexAI),
            "augment" => Some(ProviderId::Augment),
            "opencode" => Some(ProviderId::OpenCode),
            "kimi" | "moonshot" => Some(ProviderId::Kimi),
            "kimik2" | "kimi-k2" | "kimi k2" | "k2" | "kimi k2 (removed)" => {
                Some(ProviderId::KimiK2)
            }
            "amp" | "sourcegraph" => Some(ProviderId::Amp),
            "warp" | "warp-ai" | "warp-terminal" => Some(ProviderId::Warp),
            "ollama" => Some(ProviderId::Ollama),
            "azureopenai" | "azure-openai" | "azure openai" => Some(ProviderId::AzureOpenAI),
            "t3chat" | "t3-chat" | "t3 chat" => Some(ProviderId::T3Chat),
            "openrouter" | "or" => Some(ProviderId::OpenRouter),
            "jetbrains" | "jetbrains-ai" | "jetbrains ai" | "intellij" => {
                Some(ProviderId::JetBrains)
            }
            "alibaba" | "tongyi" | "qianwen" => Some(ProviderId::Alibaba),
            "alibabatokenplan" | "alibaba-token-plan" | "alibaba token plan" | "alibaba-token"
            | "bailian-token-plan" => Some(ProviderId::AlibabaTokenPlan),
            "nanogpt" | "nano-gpt" => Some(ProviderId::NanoGPT),
            "infini" | "infini-ai" => Some(ProviderId::Infini),
            "perplexity" | "pplx" => Some(ProviderId::Perplexity),
            "abacus" | "abacus ai" | "abacus-ai" => Some(ProviderId::Abacus),
            "mistral" | "mistral-ai" | "mistral ai" => Some(ProviderId::Mistral),
            "opencodego" | "opencode-go" | "opencode go" => Some(ProviderId::OpenCodeGo),
            "kilo" => Some(ProviderId::Kilo),
            "bedrock" | "aws-bedrock" | "aws bedrock" => Some(ProviderId::Bedrock),
            "codebuff" | "manicode" => Some(ProviderId::Codebuff),
            "deepseek" | "deep-seek" | "ds" => Some(ProviderId::DeepSeek),
            "deepinfra" | "deep-infra" | "di" => Some(ProviderId::DeepInfra),
            "aiand" | "ai&" | "ai-and" | "ai and" => Some(ProviderId::AiAnd),
            "windsurf" | "codeium" => Some(ProviderId::Windsurf),
            "manus" => Some(ProviderId::Manus),
            "mimo" | "xiaomi" | "xiaomimimo" | "xiaomi-mimo" | "xiaomi mimo" => {
                Some(ProviderId::MiMo)
            }
            "doubao" | "ark" | "volcengine" => Some(ProviderId::Doubao),
            "commandcode" | "command-code" | "command code" => Some(ProviderId::CommandCode),
            "crof" => Some(ProviderId::Crof),
            "stepfun" | "step-fun" | "step fun" => Some(ProviderId::StepFun),
            "venice" => Some(ProviderId::Venice),
            "openaiapi" | "openai-api" | "openai api" | "openai-balance" => {
                Some(ProviderId::OpenAIApi)
            }
            "grok" | "supergrok" | "super-grok" => Some(ProviderId::Grok),
            "xai" | "x.ai" | "x-ai" => Some(ProviderId::Xai),
            "elevenlabs" | "eleven-labs" | "11labs" => Some(ProviderId::ElevenLabs),
            "deepgram" | "dg" => Some(ProviderId::Deepgram),
            "groq" | "groqcloud" | "groq-cloud" | "groq cloud" => Some(ProviderId::Groq),
            "llmproxy" | "llm-proxy" | "llm proxy" => Some(ProviderId::LLMProxy),
            "chutes" | "chutes-ai" | "chutes ai" => Some(ProviderId::Chutes),
            "litellm" | "lite-llm" | "lite llm" => Some(ProviderId::LiteLLM),
            "poe" => Some(ProviderId::Poe),
            "devin" => Some(ProviderId::Devin),
            "zed" | "zed-ai" => Some(ProviderId::Zed),
            "crossmodel" | "cross-model" | "cross model" | "crossmodel (removed)" => {
                Some(ProviderId::CrossModel)
            }
            "qoder" => Some(ProviderId::Qoder),
            "codebuddy" | "code-buddy" | "codebuddy-cn" | "codebuddycn" | "腾讯codebuddy" => {
                Some(ProviderId::CodeBuddy)
            }
            "sakana" | "sakana-ai" | "sakana ai" => Some(ProviderId::Sakana),
            "sub2api" | "sub-2-api" | "sub 2 api" => Some(ProviderId::Sub2Api),
            "wayfinder" => Some(ProviderId::Wayfinder),
            "zenmux" | "zen-mux" => Some(ProviderId::ZenMux),
            "clinepass" | "cline-pass" | "cline" => Some(ProviderId::ClinePass),
            "longcat" | "long-cat" | "lc" => Some(ProviderId::LongCat),
            "neuralwatt" | "neural-watt" | "nw" | "neural" => Some(ProviderId::Neuralwatt),
            "qwen-cloud" | "qwencloud" | "qwen" | "qwen-token-plan" | "qwen cloud" => {
                Some(ProviderId::QwenCloud)
            }
            "zoommate" | "zoom-mate" | "zoom mate" => Some(ProviderId::ZoomMate),
            "notion" | "notion-ai" | "notionai" | "notion ai" => Some(ProviderId::Notion),
            _ => None,
        }
    }

    /// Soft-removed providers (upstream #2254: Kimi K2 + CrossModel).
    ///
    /// Modules and CLI resolution stay so existing configs and `--provider kimik2`
    /// still work. Settings UI hides them unless already enabled in settings.
    pub fn is_deprecated(&self) -> bool {
        matches!(self, ProviderId::KimiK2 | ProviderId::CrossModel)
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.cli_name())
    }
}

/// Data source mode for fetching usage
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceMode {
    /// Automatically choose the best available source
    #[default]
    Auto,
    /// Use OAuth API
    OAuth,
    /// Use web API with browser cookies
    Web,
    /// Use CLI probe
    Cli,
}

impl SourceMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "auto" => Some(SourceMode::Auto),
            "oauth" => Some(SourceMode::OAuth),
            "web" => Some(SourceMode::Web),
            "cli" => Some(SourceMode::Cli),
            _ => None,
        }
    }
}

/// Metadata about a provider
#[derive(Debug, Clone)]
pub struct ProviderMetadata {
    pub id: ProviderId,
    pub display_name: &'static str,
    pub session_label: &'static str,
    pub weekly_label: &'static str,
    pub supports_opus: bool,
    pub supports_credits: bool,
    pub default_enabled: bool,
    pub is_primary: bool,
    pub dashboard_url: Option<&'static str>,
    pub status_page_url: Option<&'static str>,
}

/// Errors that can occur when fetching provider data
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("Provider not installed: {0}")]
    NotInstalled(String),

    #[error("Authentication required")]
    AuthRequired,

    #[error("OAuth error: {0}")]
    OAuth(String),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("Timeout")]
    Timeout,

    #[error("Source mode '{0:?}' not supported for this provider")]
    UnsupportedSource(SourceMode),

    #[error("No cookies available for web API")]
    NoCookies,

    #[error("{0}")]
    Other(String),
}

/// Context passed to provider fetch operations
#[derive(Debug, Clone)]
pub struct FetchContext {
    /// Source mode to use
    pub source_mode: SourceMode,

    /// Whether to include credits/cost data
    pub include_credits: bool,

    /// Timeout for web operations in seconds
    pub web_timeout: u64,

    /// Whether to enable verbose logging
    pub verbose: bool,

    /// Manual cookie header (for testing)
    pub manual_cookie_header: Option<String>,

    /// API key for providers that require authentication
    pub api_key: Option<String>,

    /// Optional provider workspace/project scope from persisted settings.
    pub workspace_id: Option<String>,

    /// Optional provider API/web region from persisted settings.
    pub api_region: Option<String>,

    /// Optional provider gateway URL, used by local gateway-backed providers.
    pub gateway_url: Option<String>,

    /// When true, Auto mode prefers web before local (token-account scope,
    /// manual cookie source, etc.). Workspace overrides are checked separately.
    pub auto_prefer_web: bool,
}

impl Default for FetchContext {
    fn default() -> Self {
        Self {
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
        }
    }
}

/// Trait that all providers must implement
#[async_trait]
pub trait Provider: Send + Sync {
    /// Get the provider's unique identifier
    fn id(&self) -> ProviderId;

    /// Get provider metadata
    fn metadata(&self) -> &ProviderMetadata;

    /// Fetch usage data from this provider
    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError>;

    /// Get the available source modes for this provider
    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto]
    }

    /// Check if OAuth is supported
    fn supports_oauth(&self) -> bool {
        false
    }

    /// Check if web API (cookies) is supported
    fn supports_web(&self) -> bool {
        false
    }

    /// Check if CLI probe is supported
    fn supports_cli(&self) -> bool {
        false
    }

    /// Detect the version of the CLI tool (if applicable)
    fn detect_version(&self) -> Option<String> {
        None
    }
}

/// Get the CLI name map for argument parsing
pub fn cli_name_map() -> HashMap<&'static str, ProviderId> {
    let mut map = HashMap::new();
    for id in ProviderId::all() {
        map.insert(id.cli_name(), *id);
    }
    // Add aliases
    map.insert("openai", ProviderId::Codex);
    map.insert("anthropic", ProviderId::Claude);
    map.insert("codebuff", ProviderId::Codebuff);
    map.insert("manicode", ProviderId::Codebuff);
    map.insert("deep-seek", ProviderId::DeepSeek);
    map.insert("ds", ProviderId::DeepSeek);
    map.insert("deep-infra", ProviderId::DeepInfra);
    map.insert("di", ProviderId::DeepInfra);
    map.insert("ai&", ProviderId::AiAnd);
    map.insert("ai-and", ProviderId::AiAnd);
    map.insert("codeium", ProviderId::Windsurf);
    map.insert("google", ProviderId::Gemini);
    map.insert("agy", ProviderId::Antigravity);
    map.insert("github", ProviderId::Copilot);
    map.insert("aws", ProviderId::Kiro);
    map.insert("vertex", ProviderId::VertexAI);
    map.insert("sourcegraph", ProviderId::Amp);
    map.insert("warp-ai", ProviderId::Warp);
    map.insert("warp-terminal", ProviderId::Warp);
    map.insert("or", ProviderId::OpenRouter);
    map.insert("aws-bedrock", ProviderId::Bedrock);
    map.insert("aws bedrock", ProviderId::Bedrock);
    map.insert("tongyi", ProviderId::Alibaba);
    map.insert("qianwen", ProviderId::Alibaba);
    map.insert("qwen", ProviderId::QwenCloud);
    map.insert("qwencloud", ProviderId::QwenCloud);
    map.insert("qwen-token-plan", ProviderId::QwenCloud);
    map.insert("infini-ai", ProviderId::Infini);
    map.insert("pplx", ProviderId::Perplexity);
    map.insert("abacus-ai", ProviderId::Abacus);
    map.insert("mistral-ai", ProviderId::Mistral);
    map.insert("opencode-go", ProviderId::OpenCodeGo);
    map.insert("xiaomimimo", ProviderId::MiMo);
    map.insert("xiaomi-mimo", ProviderId::MiMo);
    map.insert("ark", ProviderId::Doubao);
    map.insert("volcengine", ProviderId::Doubao);
    map.insert("command-code", ProviderId::CommandCode);
    map.insert("step-fun", ProviderId::StepFun);
    map.insert("openai-api", ProviderId::OpenAIApi);
    map.insert("openai-balance", ProviderId::OpenAIApi);
    map.insert("xai", ProviderId::Xai);
    map.insert("x.ai", ProviderId::Xai);
    map.insert("supergrok", ProviderId::Grok);
    map.insert("eleven-labs", ProviderId::ElevenLabs);
    map.insert("11labs", ProviderId::ElevenLabs);
    map.insert("dg", ProviderId::Deepgram);
    map.insert("groqcloud", ProviderId::Groq);
    map.insert("groq-cloud", ProviderId::Groq);
    map.insert("chutes-ai", ProviderId::Chutes);
    map.insert("lite-llm", ProviderId::LiteLLM);
    map.insert("zed-ai", ProviderId::Zed);
    map.insert("llm-proxy", ProviderId::LLMProxy);
    map.insert("cross-model", ProviderId::CrossModel);
    map.insert("sakana-ai", ProviderId::Sakana);
    map.insert("sub-2-api", ProviderId::Sub2Api);
    map.insert("notion-ai", ProviderId::Notion);
    map.insert("notionai", ProviderId::Notion);
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_id_all() {
        let all = ProviderId::all();
        assert_eq!(all.len(), 69);
        assert!(all.contains(&ProviderId::Claude));
        assert!(all.contains(&ProviderId::Codex));
        assert!(all.contains(&ProviderId::Kimi));
        assert!(all.contains(&ProviderId::KimiK2));
        assert!(all.contains(&ProviderId::Amp));
        assert!(all.contains(&ProviderId::AzureOpenAI));
        assert!(all.contains(&ProviderId::T3Chat));
        assert!(all.contains(&ProviderId::JetBrains));
        assert!(all.contains(&ProviderId::AlibabaTokenPlan));
        assert!(all.contains(&ProviderId::NanoGPT));
        assert!(all.contains(&ProviderId::Infini));
        assert!(all.contains(&ProviderId::Bedrock));
        assert!(all.contains(&ProviderId::Codebuff));
        assert!(all.contains(&ProviderId::DeepSeek));
        assert!(all.contains(&ProviderId::DeepInfra));
        assert!(all.contains(&ProviderId::AiAnd));
        assert!(all.contains(&ProviderId::Windsurf));
        assert!(all.contains(&ProviderId::Manus));
        assert!(all.contains(&ProviderId::MiMo));
        assert!(all.contains(&ProviderId::Doubao));
        assert!(all.contains(&ProviderId::CommandCode));
        assert!(all.contains(&ProviderId::Crof));
        assert!(all.contains(&ProviderId::StepFun));
        assert!(all.contains(&ProviderId::Venice));
        assert!(all.contains(&ProviderId::OpenAIApi));
        assert!(all.contains(&ProviderId::Grok));
        assert!(all.contains(&ProviderId::ElevenLabs));
        assert!(all.contains(&ProviderId::Deepgram));
        assert!(all.contains(&ProviderId::Groq));
        assert!(all.contains(&ProviderId::LLMProxy));
        assert!(all.contains(&ProviderId::Chutes));
        assert!(all.contains(&ProviderId::LiteLLM));
        assert!(all.contains(&ProviderId::Poe));
        assert!(all.contains(&ProviderId::Devin));
        assert!(all.contains(&ProviderId::Zed));
        assert!(all.contains(&ProviderId::CrossModel));
        assert!(all.contains(&ProviderId::Qoder));
        assert!(all.contains(&ProviderId::CodeBuddy));
        assert!(all.contains(&ProviderId::Sakana));
        assert!(all.contains(&ProviderId::Sub2Api));
        assert!(all.contains(&ProviderId::Wayfinder));
        assert!(all.contains(&ProviderId::ZenMux));
        assert!(all.contains(&ProviderId::ClinePass));
        assert!(all.contains(&ProviderId::LongCat));
        assert!(all.contains(&ProviderId::Neuralwatt));
        assert!(all.contains(&ProviderId::ZoomMate));
        assert!(all.contains(&ProviderId::QwenCloud));
        assert!(all.contains(&ProviderId::Notion));
        assert!(all.contains(&ProviderId::Xai));
    }

    #[test]
    fn deprecated_providers_are_kimik2_and_crossmodel() {
        assert!(ProviderId::KimiK2.is_deprecated());
        assert!(ProviderId::CrossModel.is_deprecated());
        assert!(!ProviderId::Kimi.is_deprecated());
        assert!(!ProviderId::AiAnd.is_deprecated());
        assert!(ProviderId::KimiK2.display_name().contains("(removed)"));
        assert!(ProviderId::CrossModel.display_name().contains("(removed)"));
        assert_eq!(
            ProviderId::from_cli_name("kimik2"),
            Some(ProviderId::KimiK2)
        );
        assert_eq!(
            ProviderId::from_cli_name("crossmodel"),
            Some(ProviderId::CrossModel)
        );
    }

    #[test]
    fn aiand_cli_aliases_resolve() {
        assert_eq!(ProviderId::from_cli_name("aiand"), Some(ProviderId::AiAnd));
        assert_eq!(ProviderId::from_cli_name("ai&"), Some(ProviderId::AiAnd));
        assert_eq!(ProviderId::from_cli_name("ai-and"), Some(ProviderId::AiAnd));
        assert_eq!(ProviderId::AiAnd.cli_name(), "aiand");
        assert_eq!(ProviderId::AiAnd.display_name(), "ai&");
    }

    #[test]
    fn test_provider_id_cli_name() {
        assert_eq!(ProviderId::Claude.cli_name(), "claude");
        assert_eq!(ProviderId::Codex.cli_name(), "codex");
        assert_eq!(ProviderId::Factory.cli_name(), "factory");
        assert_eq!(ProviderId::Zai.cli_name(), "zai");
    }

    #[test]
    fn test_provider_id_display_name() {
        assert_eq!(ProviderId::Claude.display_name(), "Claude");
        assert_eq!(ProviderId::Factory.display_name(), "Factory");
        assert_eq!(ProviderId::Zai.display_name(), "z.ai");
    }

    #[test]
    fn test_provider_id_from_cli_name() {
        assert_eq!(
            ProviderId::from_cli_name("claude"),
            Some(ProviderId::Claude)
        );
        assert_eq!(
            ProviderId::from_cli_name("anthropic"),
            Some(ProviderId::Claude)
        );
        assert_eq!(
            ProviderId::from_cli_name("CLAUDE"),
            Some(ProviderId::Claude)
        );
        assert_eq!(ProviderId::from_cli_name("codex"), Some(ProviderId::Codex));
        assert_eq!(ProviderId::from_cli_name("openai"), Some(ProviderId::Codex));
        assert_eq!(
            ProviderId::from_cli_name("factory"),
            Some(ProviderId::Factory)
        );
        assert_eq!(
            ProviderId::from_cli_name("agy"),
            Some(ProviderId::Antigravity)
        );
        assert_eq!(ProviderId::from_cli_name("zed"), Some(ProviderId::Zed));
        assert_eq!(ProviderId::from_cli_name("unknown"), None);
    }

    #[test]
    fn test_provider_id_from_display_name_aliases() {
        for provider_id in ProviderId::all() {
            assert_eq!(
                ProviderId::from_cli_name(provider_id.display_name()),
                Some(*provider_id),
                "display name should round-trip for {}",
                provider_id.display_name()
            );
        }
    }

    #[test]
    fn test_provider_id_display() {
        assert_eq!(format!("{}", ProviderId::Claude), "claude");
        assert_eq!(format!("{}", ProviderId::Codex), "codex");
    }

    #[test]
    fn test_source_mode_from_str() {
        assert_eq!(SourceMode::parse("auto"), Some(SourceMode::Auto));
        assert_eq!(SourceMode::parse("oauth"), Some(SourceMode::OAuth));
        assert_eq!(SourceMode::parse("web"), Some(SourceMode::Web));
        assert_eq!(SourceMode::parse("cli"), Some(SourceMode::Cli));
        assert_eq!(SourceMode::parse("AUTO"), Some(SourceMode::Auto));
        assert_eq!(SourceMode::parse("invalid"), None);
    }

    #[test]
    fn test_fetch_context_default() {
        let ctx = FetchContext::default();
        assert_eq!(ctx.source_mode, SourceMode::Auto);
        assert!(ctx.include_credits);
        assert_eq!(ctx.web_timeout, 60);
        assert!(!ctx.verbose);
        assert!(ctx.manual_cookie_header.is_none());
        assert!(ctx.api_key.is_none());
    }

    #[test]
    fn test_cli_name_map() {
        let map = cli_name_map();
        assert_eq!(map.get("claude"), Some(&ProviderId::Claude));
        assert_eq!(map.get("anthropic"), Some(&ProviderId::Claude));
        assert_eq!(map.get("codex"), Some(&ProviderId::Codex));
        assert_eq!(map.get("openai"), Some(&ProviderId::Codex));
        assert_eq!(map.get("agy"), Some(&ProviderId::Antigravity));
    }

    #[test]
    fn test_provider_id_cookie_domain() {
        // Cookie-based providers
        assert_eq!(ProviderId::Claude.cookie_domain(), Some("claude.ai"));
        assert_eq!(ProviderId::Cursor.cookie_domain(), Some("cursor.com"));
        assert_eq!(ProviderId::Factory.cookie_domain(), Some("app.factory.ai"));
        assert_eq!(ProviderId::Codex.cookie_domain(), Some("chatgpt.com"));
        assert_eq!(
            ProviderId::Gemini.cookie_domain(),
            Some("aistudio.google.com")
        );
        assert_eq!(ProviderId::Kiro.cookie_domain(), Some("kiro.dev"));
        assert_eq!(ProviderId::Kimi.cookie_domain(), Some("kimi.moonshot.cn"));
        assert_eq!(ProviderId::OpenCode.cookie_domain(), Some("opencode.ai"));

        // Token-based providers (no cookies)
        assert_eq!(ProviderId::Copilot.cookie_domain(), None);
        assert_eq!(ProviderId::Zai.cookie_domain(), None);
        assert_eq!(ProviderId::VertexAI.cookie_domain(), None);
        assert_eq!(ProviderId::JetBrains.cookie_domain(), None);
    }

    #[test]
    fn test_provider_id_alibaba() {
        assert_eq!(ProviderId::Alibaba.cli_name(), "alibaba");
        assert_eq!(ProviderId::Alibaba.display_name(), "Alibaba");
        assert_eq!(
            ProviderId::Alibaba.cookie_domain(),
            Some("modelstudio.console.alibabacloud.com")
        );
        assert_eq!(
            ProviderId::from_cli_name("alibaba"),
            Some(ProviderId::Alibaba)
        );
        assert_eq!(
            ProviderId::from_cli_name("tongyi"),
            Some(ProviderId::Alibaba)
        );
        assert_eq!(
            ProviderId::from_cli_name("qianwen"),
            Some(ProviderId::Alibaba)
        );
    }

    #[test]
    fn test_provider_id_qwen_cloud() {
        assert_eq!(ProviderId::QwenCloud.cli_name(), "qwen-cloud");
        assert_eq!(ProviderId::QwenCloud.display_name(), "Qwen Cloud");
        assert_eq!(ProviderId::QwenCloud.cookie_domain(), Some("qwencloud.com"));
        assert_eq!(
            ProviderId::from_cli_name("qwen-cloud"),
            Some(ProviderId::QwenCloud)
        );
        assert_eq!(
            ProviderId::from_cli_name("qwencloud"),
            Some(ProviderId::QwenCloud)
        );
        assert_eq!(
            ProviderId::from_cli_name("qwen"),
            Some(ProviderId::QwenCloud)
        );
        assert_eq!(
            ProviderId::from_cli_name("qwen-token-plan"),
            Some(ProviderId::QwenCloud)
        );
        assert_eq!(
            ProviderId::from_cli_name("qwen cloud"),
            Some(ProviderId::QwenCloud)
        );
        // Bare "qwen" must not resolve to Alibaba Coding Plan.
        assert_ne!(ProviderId::from_cli_name("qwen"), Some(ProviderId::Alibaba));
    }

    #[test]
    fn test_provider_id_notion() {
        assert_eq!(ProviderId::Notion.cli_name(), "notion");
        assert_eq!(ProviderId::Notion.display_name(), "Notion AI");
        assert_eq!(ProviderId::Notion.cookie_domain(), Some("app.notion.com"));
        assert_eq!(
            ProviderId::from_cli_name("notion"),
            Some(ProviderId::Notion)
        );
        assert_eq!(
            ProviderId::from_cli_name("notion-ai"),
            Some(ProviderId::Notion)
        );
        assert_eq!(
            ProviderId::from_cli_name("notionai"),
            Some(ProviderId::Notion)
        );
        assert_eq!(
            ProviderId::from_cli_name("notion ai"),
            Some(ProviderId::Notion)
        );
    }

    #[test]
    fn test_provider_id_xai() {
        assert_eq!(ProviderId::Xai.cli_name(), "xai");
        assert_eq!(ProviderId::Xai.display_name(), "xAI");
        assert_eq!(ProviderId::Xai.cookie_domain(), None);
        assert_eq!(ProviderId::from_cli_name("xai"), Some(ProviderId::Xai));
        assert_eq!(ProviderId::from_cli_name("x.ai"), Some(ProviderId::Xai));
        assert_eq!(ProviderId::from_cli_name("x-ai"), Some(ProviderId::Xai));
        // Grok keeps consumer aliases; xai is the developer-platform provider.
        assert_eq!(ProviderId::from_cli_name("grok"), Some(ProviderId::Grok));
        assert_eq!(
            ProviderId::from_cli_name("supergrok"),
            Some(ProviderId::Grok)
        );
    }
}
