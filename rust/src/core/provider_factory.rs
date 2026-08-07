//! Canonical provider factory.
//!
//! All call sites (CLI, Tauri desktop shell, legacy native UI) must construct
//! provider instances through [`instantiate`] so that adding a new provider
//! only requires editing `ProviderId`, the matching `providers/` module, and
//! this one match arm.

use super::{Provider, ProviderId};
use crate::providers::{
    AbacusProvider, AiAndProvider, AlibabaProvider, AlibabaTokenPlanProvider, AmpProvider,
    AntigravityProvider, AugmentProvider, AzureOpenAIProvider, BedrockProvider, ChutesProvider,
    ClaudeProvider, ClinePassProvider, CodebuffProvider, CodexProvider, CommandCodeProvider,
    CopilotProvider, CrofProvider, CrossModelProvider, CursorProvider, DeepInfraProvider,
    DeepSeekProvider, DeepgramProvider, DevinProvider, DoubaoProvider, ElevenLabsProvider,
    FactoryProvider, GeminiProvider, GrokProvider, GroqProvider, InfiniProvider, JetBrainsProvider,
    KiloProvider, KimiK2Provider, KimiProvider, KiroProvider, LLMProxyProvider, LiteLLMProvider,
    LongCatProvider, ManusProvider, MiMoProvider, MiniMaxProvider, MistralProvider,
    NanoGPTProvider, NeuralwattProvider, NotionProvider, OllamaProvider, OpenAIApiProvider,
    OpenCodeGoProvider, OpenCodeProvider, OpenRouterProvider, PerplexityProvider, PoeProvider,
    CodeBuddyProvider, QoderProvider, QwenCloudProvider, SakanaProvider, StepFunProvider,
    Sub2ApiProvider,
    T3ChatProvider, VeniceProvider, VertexAIProvider, WarpProvider, WayfinderProvider,
    WindsurfProvider, XaiProvider, ZaiProvider, ZedProvider, ZenMuxProvider, ZoomMateProvider,
};

/// Instantiate the concrete [`Provider`] implementation for a given [`ProviderId`].
///
/// Exhaustive over [`ProviderId`]: adding a new variant is a compile error until
/// the corresponding provider type is wired in below.
pub fn instantiate(id: ProviderId) -> Box<dyn Provider> {
    match id {
        ProviderId::Claude => Box::new(ClaudeProvider::new()),
        ProviderId::Codex => Box::new(CodexProvider::new()),
        ProviderId::Cursor => Box::new(CursorProvider::new()),
        ProviderId::Gemini => Box::new(GeminiProvider::new()),
        ProviderId::Copilot => Box::new(CopilotProvider::new()),
        ProviderId::Antigravity => Box::new(AntigravityProvider::new()),
        ProviderId::Factory => Box::new(FactoryProvider::new()),
        ProviderId::Zai => Box::new(ZaiProvider::new()),
        ProviderId::Kiro => Box::new(KiroProvider::new()),
        ProviderId::VertexAI => Box::new(VertexAIProvider::new()),
        ProviderId::Augment => Box::new(AugmentProvider::new()),
        ProviderId::MiniMax => Box::new(MiniMaxProvider::new()),
        ProviderId::OpenCode => Box::new(OpenCodeProvider::new()),
        ProviderId::Kimi => Box::new(KimiProvider::new()),
        ProviderId::KimiK2 => Box::new(KimiK2Provider::new()),
        ProviderId::Amp => Box::new(AmpProvider::new()),
        ProviderId::Warp => Box::new(WarpProvider::new()),
        ProviderId::Ollama => Box::new(OllamaProvider::new()),
        ProviderId::AzureOpenAI => Box::new(AzureOpenAIProvider::new()),
        ProviderId::T3Chat => Box::new(T3ChatProvider::new()),
        ProviderId::OpenRouter => Box::new(OpenRouterProvider::new()),
        ProviderId::JetBrains => Box::new(JetBrainsProvider::new()),
        ProviderId::Alibaba => Box::new(AlibabaProvider::new()),
        ProviderId::AlibabaTokenPlan => Box::new(AlibabaTokenPlanProvider::new()),
        ProviderId::NanoGPT => Box::new(NanoGPTProvider::new()),
        ProviderId::Infini => Box::new(InfiniProvider::default()),
        ProviderId::Perplexity => Box::new(PerplexityProvider::new()),
        ProviderId::Abacus => Box::new(AbacusProvider::new()),
        ProviderId::Mistral => Box::new(MistralProvider::new()),
        ProviderId::OpenCodeGo => Box::new(OpenCodeGoProvider::new()),
        ProviderId::Kilo => Box::new(KiloProvider::new()),
        ProviderId::Bedrock => Box::new(BedrockProvider::new()),
        ProviderId::Codebuff => Box::new(CodebuffProvider::new()),
        ProviderId::DeepSeek => Box::new(DeepSeekProvider::new()),
        ProviderId::DeepInfra => Box::new(DeepInfraProvider::new()),
        ProviderId::AiAnd => Box::new(AiAndProvider::new()),
        ProviderId::Windsurf => Box::new(WindsurfProvider::new()),
        ProviderId::Manus => Box::new(ManusProvider::new()),
        ProviderId::MiMo => Box::new(MiMoProvider::new()),
        ProviderId::Doubao => Box::new(DoubaoProvider::new()),
        ProviderId::CommandCode => Box::new(CommandCodeProvider::new()),
        ProviderId::Crof => Box::new(CrofProvider::new()),
        ProviderId::StepFun => Box::new(StepFunProvider::new()),
        ProviderId::Venice => Box::new(VeniceProvider::new()),
        ProviderId::OpenAIApi => Box::new(OpenAIApiProvider::new()),
        ProviderId::Grok => Box::new(GrokProvider::new()),
        ProviderId::ElevenLabs => Box::new(ElevenLabsProvider::new()),
        ProviderId::Deepgram => Box::new(DeepgramProvider::new()),
        ProviderId::Groq => Box::new(GroqProvider::new()),
        ProviderId::LLMProxy => Box::new(LLMProxyProvider::new()),
        ProviderId::Chutes => Box::new(ChutesProvider::new()),
        ProviderId::LiteLLM => Box::new(LiteLLMProvider::new()),
        ProviderId::Poe => Box::new(PoeProvider::new()),
        ProviderId::Devin => Box::new(DevinProvider::new()),
        ProviderId::Zed => Box::new(ZedProvider::new()),
        ProviderId::CrossModel => Box::new(CrossModelProvider::new()),
        ProviderId::Qoder => Box::new(QoderProvider::new()),
        ProviderId::CodeBuddy => Box::new(CodeBuddyProvider::new()),
        ProviderId::Sakana => Box::new(SakanaProvider::new()),
        ProviderId::Sub2Api => Box::new(Sub2ApiProvider::new()),
        ProviderId::Wayfinder => Box::new(WayfinderProvider::new()),
        ProviderId::ZenMux => Box::new(ZenMuxProvider::new()),
        ProviderId::ClinePass => Box::new(ClinePassProvider::new()),
        ProviderId::LongCat => Box::new(LongCatProvider::new()),
        ProviderId::Neuralwatt => Box::new(NeuralwattProvider::new()),
        ProviderId::ZoomMate => Box::new(ZoomMateProvider::new()),
        ProviderId::QwenCloud => Box::new(QwenCloudProvider::new()),
        ProviderId::Notion => Box::new(NotionProvider::new()),
        ProviderId::Xai => Box::new(XaiProvider::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_provider_id_is_instantiable() {
        for &id in ProviderId::all() {
            let provider = instantiate(id);
            assert_eq!(
                provider.id(),
                id,
                "factory returned wrong provider for {id}"
            );
        }
    }
}
