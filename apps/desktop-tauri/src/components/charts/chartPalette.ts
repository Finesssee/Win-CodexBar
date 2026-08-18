/**
 * Chart palette helpers — map provider ids and service names to the
 * CSS custom properties declared in `styles.css`. Keeping the mapping
 * centralised means Phase 12 (theming) can flip the tokens in one
 * place rather than touching every chart component.
 *
 * Provider token mapping mirrors `native_ui/theme.rs::provider_color`
 * and service mapping mirrors `native_ui/charts.rs::color_for_service`.
 */

const PROVIDER_TOKEN: Record<string, string> = {
  claude: "--chart-claude",
  codex: "--chart-codex",
  gemini: "--chart-gemini",
  cursor: "--chart-cursor",
  copilot: "--chart-copilot",
  jetbrains: "--chart-jetbrains",
  "jetbrains ai": "--chart-jetbrains",
  antigravity: "--chart-antigravity",
  augment: "--chart-augment",
  amp: "--chart-amp",
  factory: "--chart-factory",
  droid: "--chart-droid",
  kimi: "--chart-kimi",
  kimik2: "--chart-kimik2",
  "kimi k2": "--chart-kimik2",
  kiro: "--chart-kiro",
  opencode: "--chart-opencode",
  minimax: "--chart-minimax",
  vertexai: "--chart-vertexai",
  "vertex ai": "--chart-vertexai",
  zai: "--chart-zai",
  "z.ai": "--chart-zai",
  alibaba: "--chart-alibaba",
  tongyi: "--chart-alibaba",
  nanogpt: "--chart-nanogpt",
  mistral: "--chart-mistral",
  codebuff: "--chart-codebuff",
  manicode: "--chart-codebuff",
  deepseek: "--chart-deepseek",
  "deep seek": "--chart-deepseek",
  windsurf: "--chart-windsurf",
  codeium: "--chart-windsurf",
  elevenlabs: "--chart-elevenlabs",
  "eleven labs": "--chart-elevenlabs",
  deepgram: "--chart-deepgram",
  grok: "--chart-grok",
  xai: "--chart-grok",
  supergrok: "--chart-grok",
  groq: "--chart-groq",
  groqcloud: "--chart-groq",
  llmproxy: "--chart-llmproxy",
  "llm proxy": "--chart-llmproxy",
};

/** CSS color expression for a provider's cost-series bars. */
export function providerCostColor(providerId: string): string {
  const token = PROVIDER_TOKEN[providerId.toLowerCase()];
  if (token) return `var(${token}, var(--provider-accent, var(--chart-cost)))`;
  return "var(--provider-accent, var(--chart-cost))";
}

/** CSS color expression for a provider's credits-series line. */
export function providerCreditsColor(providerId: string): string {
  const token = PROVIDER_TOKEN[providerId.toLowerCase()];
  if (token) return `var(${token}, var(--provider-accent, var(--chart-credits)))`;
  return "var(--provider-accent, var(--chart-credits))";
}

/**
 * Resolve a UsageBreakdownChart service name to a palette token. `ordered`
 * is the sorted list of distinct services in the visible data so that
 * unrelated services receive different colors.
 */
export function serviceColorVar(service: string, ordered: string[]): string {
  const lower = service.toLowerCase();
  if (lower === "cli") return "var(--chart-service-cli)";
  if (lower.includes("github") && lower.includes("review")) {
    return "var(--chart-service-review)";
  }
  if (lower.includes("api")) return "var(--chart-service-api)";

  // Deterministic palette spread — prefer the position within the
  // sorted unique-services list so the colors are stable across
  // renders, but wrap into the 5-slot extras palette.
  const idx = Math.max(0, ordered.indexOf(service));
  const slot = (idx % 5) + 1;
  return `var(--chart-service-${slot})`;
}
