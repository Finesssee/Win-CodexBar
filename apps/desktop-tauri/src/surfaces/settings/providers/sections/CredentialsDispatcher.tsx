import type { LocaleKey } from "../../../../i18n/keys";
import { GeminiCliCreds } from "./credentials/GeminiCliCreds";
import { VertexAiCreds } from "./credentials/VertexAiCreds";
import { JetBrainsCreds } from "./credentials/JetBrainsCreds";
import { KiroCreds } from "./credentials/KiroCreds";
import { ClaudeCreds } from "./credentials/ClaudeCreds";
import { OpenAiExtras } from "./credentials/OpenAiExtras";
import { OpenRouterManagementCreds } from "./credentials/OpenRouterManagementCreds";

interface Props {
  providerId: string;
  t: (key: LocaleKey) => string;
}

/**
 * Dispatch the appropriate Phase-6d credential component based on the
 * current provider. Providers without a bespoke credentials UI render
 * nothing. Mirrors the `provider_id == ProviderId::*` chain in
 * `rust/src/native_ui/preferences.rs::render_provider_detail_panel`.
 */
export function CredentialsDispatcher({ providerId, t }: Props) {
  switch (providerId) {
    case "gemini":
      return <GeminiCliCreds providerId={providerId} t={t} />;
    case "vertexai":
      return <VertexAiCreds providerId={providerId} t={t} />;
    case "jetbrains":
      return <JetBrainsCreds t={t} />;
    case "kiro":
      return <KiroCreds t={t} />;
    case "claude":
      return <ClaudeCreds t={t} />;
    case "codex":
      return <OpenAiExtras providerId={providerId} t={t} />;
    case "openaiapi":
      return <OpenAiExtras providerId={providerId} t={t} />;
    case "litellm":
    case "devin":
    case "opencodego":
    case "zed":
    case "sub2api":
    case "xai":
      return <OpenAiExtras providerId={providerId} t={t} />;
    case "openrouter":
      return <OpenRouterManagementCreds t={t} />;
    default:
      return null;
  }
}
