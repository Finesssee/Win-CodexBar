// Ported from rust/src/native_ui/provider_icons.rs and
// rust/src/native_ui/theme.rs::{provider_color, provider_icon}.
// Keep in sync with the Rust registries when new providers are added.

import abacus from "./icons/ProviderIcon-abacus.svg?raw";
import alibaba from "./icons/ProviderIcon-alibaba.svg?raw";
import amp from "./icons/ProviderIcon-amp.svg?raw";
import antigravity from "./icons/ProviderIcon-antigravity.svg?raw";
import augment from "./icons/ProviderIcon-augment.svg?raw";
import bedrock from "./icons/ProviderIcon-bedrock.svg?raw";
import claude from "./icons/ProviderIcon-claude.svg?raw";
import codebuff from "./icons/ProviderIcon-codebuff.svg?raw";
import codex from "./icons/ProviderIcon-codex.svg?raw";
import commandcode from "./icons/ProviderIcon-commandcode.svg?raw";
import copilot from "./icons/ProviderIcon-copilot.svg?raw";
import crof from "./icons/ProviderIcon-crof.svg?raw";
import crossmodel from "./icons/ProviderIcon-crossmodel.svg?raw";
import cursor from "./icons/ProviderIcon-cursor.svg?raw";
import deepgram from "./icons/ProviderIcon-deepgram.svg?raw";
import deepinfra from "./icons/ProviderIcon-deepinfra.svg?raw";
import aiand from "./icons/ProviderIcon-aiand.svg?raw";
import clinepass from "./icons/ProviderIcon-clinepass.svg?raw";
import longcat from "./icons/ProviderIcon-longcat.svg?raw";
import neuralwatt from "./icons/ProviderIcon-neuralwatt.svg?raw";
import zoommate from "./icons/ProviderIcon-zoommate.svg?raw";
import zenmux from "./icons/ProviderIcon-zenmux.svg?raw";
import deepseek from "./icons/ProviderIcon-deepseek.svg?raw";
import doubao from "./icons/ProviderIcon-doubao.svg?raw";
import elevenlabs from "./icons/ProviderIcon-elevenlabs.svg?raw";
import factory from "./icons/ProviderIcon-factory.svg?raw";
import gemini from "./icons/ProviderIcon-gemini.svg?raw";
import grok from "./icons/ProviderIcon-grok.svg?raw";
import groq from "./icons/ProviderIcon-groq.svg?raw";
import jetbrains from "./icons/ProviderIcon-jetbrains.svg?raw";
import kilo from "./icons/ProviderIcon-kilo.svg?raw";
import kimi from "./icons/ProviderIcon-kimi.svg?raw";
import kiro from "./icons/ProviderIcon-kiro.svg?raw";
import llmproxy from "./icons/ProviderIcon-llmproxy.svg?raw";
import manus from "./icons/ProviderIcon-manus.svg?raw";
import mimo from "./icons/ProviderIcon-mimo.svg?raw";
import minimax from "./icons/ProviderIcon-minimax.svg?raw";
import mistral from "./icons/ProviderIcon-mistral.svg?raw";
import notion from "./icons/ProviderIcon-notion.svg?raw";
import xai from "./icons/ProviderIcon-xai.svg?raw";
import ollama from "./icons/ProviderIcon-ollama.svg?raw";
import opencode from "./icons/ProviderIcon-opencode.svg?raw";
import opencodego from "./icons/ProviderIcon-opencodego.svg?raw";
import openrouter from "./icons/ProviderIcon-openrouter.svg?raw";
import perplexity from "./icons/ProviderIcon-perplexity.svg?raw";
import qoder from "./icons/ProviderIcon-qoder.svg?raw";
import sakana from "./icons/ProviderIcon-sakana.svg?raw";
import stepfun from "./icons/ProviderIcon-stepfun.svg?raw";
import sub2api from "./icons/ProviderIcon-sub2api.svg?raw";
import t3chat from "./icons/ProviderIcon-t3chat.svg?raw";
import venice from "./icons/ProviderIcon-venice.svg?raw";
import vertexai from "./icons/ProviderIcon-vertexai.svg?raw";
import warp from "./icons/ProviderIcon-warp.svg?raw";
import windsurf from "./icons/ProviderIcon-windsurf.svg?raw";
import zai from "./icons/ProviderIcon-zai.svg?raw";

/**
 * Replace hard-coded fills/strokes in the bundled brand SVGs with
 * `currentColor` so the icon picks up the brand color via CSS, making each
 * provider visually distinct in compact tray rows.
 */
function tint(raw: string): string {
  return raw
    .replace(/fill="white"/gi, 'fill="currentColor"')
    .replace(/fill="#fff"/gi, 'fill="currentColor"')
    .replace(/fill="#ffffff"/gi, 'fill="currentColor"')
    .replace(/stroke="white"/gi, 'stroke="currentColor"');
}

export interface ProviderIcon {
  /** CLI-style provider id (lowercase, normalized). */
  id: string;
  /** Brand hex color. */
  brandColor: string;
  /** Single-character fallback used when no SVG is available. */
  fallbackLetter: string;
  /** Raw SVG markup when the provider ships a brand asset. */
  svgPath?: string;
}

const RAW: Record<string, string> = {
  abacus: tint(abacus),
  alibaba: tint(alibaba),
  amp: tint(amp),
  antigravity: tint(antigravity),
  augment: tint(augment),
  bedrock: tint(bedrock),
  claude: tint(claude),
  codebuff: tint(codebuff),
  codex: tint(codex),
  commandcode: tint(commandcode),
  copilot: tint(copilot),
  crof: tint(crof),
  crossmodel: tint(crossmodel),
  cursor: tint(cursor),
  deepgram: tint(deepgram),
  deepinfra: tint(deepinfra),
  aiand: tint(aiand),
  clinepass: tint(clinepass),
  longcat: tint(longcat),
  neuralwatt: tint(neuralwatt),
  zoommate: tint(zoommate),
  zenmux: tint(zenmux),
  deepseek: tint(deepseek),
  doubao: tint(doubao),
  elevenlabs: tint(elevenlabs),
  factory: tint(factory),
  gemini: tint(gemini),
  grok: tint(grok),
  groq: tint(groq),
  jetbrains: tint(jetbrains),
  kilo: tint(kilo),
  kimi: tint(kimi),
  kiro: tint(kiro),
  llmproxy: tint(llmproxy),
  manus: tint(manus),
  mimo: tint(mimo),
  minimax: tint(minimax),
  notion: tint(notion),
  xai: tint(xai),
  mistral: tint(mistral),
  ollama: tint(ollama),
  opencode: tint(opencode),
  opencodego: tint(opencodego),
  openrouter: tint(openrouter),
  perplexity: tint(perplexity),
  qoder: tint(qoder),
  sakana: tint(sakana),
  stepfun: tint(stepfun),
  sub2api: tint(sub2api),
  t3chat: tint(t3chat),
  venice: tint(venice),
  vertexai: tint(vertexai),
  warp: tint(warp),
  windsurf: tint(windsurf),
  zai: tint(zai),
};

/**
 * Registry of provider icons. Matches the entries in
 * `rust/src/native_ui/provider_icons.rs` and pulls brand colors / fallback
 * letters from `rust/src/native_ui/theme.rs::{provider_color, provider_icon}`.
 */
export const PROVIDER_ICON_REGISTRY: Record<string, ProviderIcon> = {
  alibaba:     { id: "alibaba",     brandColor: "#ff6a00", fallbackLetter: "阿", svgPath: RAW.alibaba },
  alibabatokenplan: { id: "alibabatokenplan", brandColor: "#ff6a00", fallbackLetter: "阿", svgPath: RAW.alibaba },
  amp:         { id: "amp",         brandColor: "#dc2626", fallbackLetter: "⚡", svgPath: RAW.amp },
  antigravity: { id: "antigravity", brandColor: "#60ba7e", fallbackLetter: "◉", svgPath: RAW.antigravity },
  augment:     { id: "augment",     brandColor: "#6366f1", fallbackLetter: "A", svgPath: RAW.augment },
  claude:      { id: "claude",      brandColor: "#cc7c5e", fallbackLetter: "◈", svgPath: RAW.claude },
  codebuff:    { id: "codebuff",    brandColor: "#44ff00", fallbackLetter: "B", svgPath: RAW.codebuff },
  codex:       { id: "codex",       brandColor: "#49a3b0", fallbackLetter: "◆", svgPath: RAW.codex },
  copilot:     { id: "copilot",     brandColor: "#a855f7", fallbackLetter: "⬡", svgPath: RAW.copilot },
  cursor:      { id: "cursor",      brandColor: "#00bfa5", fallbackLetter: "▸", svgPath: RAW.cursor },
  deepgram:    { id: "deepgram",    brandColor: "#13ef93", fallbackLetter: "D", svgPath: RAW.deepgram },
  deepinfra:   { id: "deepinfra",   brandColor: "#2a3275", fallbackLetter: "D", svgPath: RAW.deepinfra },
  aiand:       { id: "aiand",       brandColor: "#e25c2b", fallbackLetter: "&", svgPath: RAW.aiand },
  clinepass:   { id: "clinepass",   brandColor: "#61a3fa", fallbackLetter: "C", svgPath: RAW.clinepass },
  longcat:     { id: "longcat",     brandColor: "#ffd100", fallbackLetter: "L", svgPath: RAW.longcat },
  neuralwatt:  { id: "neuralwatt",  brandColor: "#38d98c", fallbackLetter: "N", svgPath: RAW.neuralwatt },
  zoommate:    { id: "zoommate",    brandColor: "#0B5CFF", fallbackLetter: "Z", svgPath: RAW.zoommate },
  zenmux:      { id: "zenmux",      brandColor: "#6c5ce7", fallbackLetter: "Z", svgPath: RAW.zenmux },
  deepseek:    { id: "deepseek",    brandColor: "#527df0", fallbackLetter: "D", svgPath: RAW.deepseek },
  elevenlabs:  { id: "elevenlabs",  brandColor: "#111827", fallbackLetter: "E", svgPath: RAW.elevenlabs },
  factory:     { id: "factory",     brandColor: "#ff6b35", fallbackLetter: "◎", svgPath: RAW.factory },
  gemini:      { id: "gemini",      brandColor: "#ab87ea", fallbackLetter: "✦", svgPath: RAW.gemini },
  grok:        { id: "grok",        brandColor: "#111827", fallbackLetter: "G", svgPath: RAW.grok },
  groq:        { id: "groq",        brandColor: "#f55036", fallbackLetter: "G", svgPath: RAW.groq },
  jetbrains:   { id: "jetbrains",   brandColor: "#ff3399", fallbackLetter: "J", svgPath: RAW.jetbrains },
  kilo:        { id: "kilo",        brandColor: "#5d87ff", fallbackLetter: "K", svgPath: RAW.kilo },
  bedrock:     { id: "bedrock",     brandColor: "#ff9900", fallbackLetter: "B", svgPath: RAW.bedrock },
  kimi:        { id: "kimi",        brandColor: "#fe603c", fallbackLetter: "☽", svgPath: RAW.kimi },
  kimik2:      { id: "kimik2",      brandColor: "#4c00ff", fallbackLetter: "☽", svgPath: RAW.kimi },
  kiro:        { id: "kiro",        brandColor: "#ff9900", fallbackLetter: "K", svgPath: RAW.kiro },
  llmproxy:    { id: "llmproxy",    brandColor: "#4f46e5", fallbackLetter: "L", svgPath: RAW.llmproxy },
  minimax:     { id: "minimax",     brandColor: "#fe603c", fallbackLetter: "M", svgPath: RAW.minimax },
  mistral:     { id: "mistral",     brandColor: "#ff500f", fallbackLetter: "M", svgPath: RAW.mistral },
  ollama:      { id: "ollama",      brandColor: "#8b95b0", fallbackLetter: "○", svgPath: RAW.ollama },
  azureopenai: { id: "azureopenai", brandColor: "#0078d4", fallbackLetter: "A" },
  t3chat:      { id: "t3chat",      brandColor: "#8b5cf6", fallbackLetter: "T", svgPath: RAW.t3chat },
  opencode:    { id: "opencode",    brandColor: "#3b82f6", fallbackLetter: "○", svgPath: RAW.opencode },
  opencodego:  { id: "opencodego",  brandColor: "#3b82f6", fallbackLetter: "○", svgPath: RAW.opencodego },
  openrouter:  { id: "openrouter",  brandColor: "#6b7280", fallbackLetter: "R", svgPath: RAW.openrouter },
  perplexity:  { id: "perplexity",  brandColor: "#1fb8cd", fallbackLetter: "P", svgPath: RAW.perplexity },
  vertexai:    { id: "vertexai",    brandColor: "#4285f4", fallbackLetter: "△", svgPath: RAW.vertexai },
  warp:        { id: "warp",        brandColor: "#6366f1", fallbackLetter: "W", svgPath: RAW.warp },
  windsurf:    { id: "windsurf",    brandColor: "#22c55e", fallbackLetter: "W", svgPath: RAW.windsurf },
  wayfinder:   { id: "wayfinder",   brandColor: "#14b8a6", fallbackLetter: "W" },
  zai:         { id: "zai",         brandColor: "#e85a6a", fallbackLetter: "Z", svgPath: RAW.zai },
  // Aliases / Rust-side normalizations without their own SVG.
  nanogpt:     { id: "nanogpt",     brandColor: "#687fa1", fallbackLetter: "N" },
  infini:      { id: "infini",      brandColor: "#687fa1", fallbackLetter: "I" },
  abacus:      { id: "abacus",      brandColor: "#7c3aed", fallbackLetter: "A", svgPath: RAW.abacus },
  manus:       { id: "manus",       brandColor: "#34322d", fallbackLetter: "M", svgPath: RAW.manus },
  mimo:        { id: "mimo",        brandColor: "#ff6900", fallbackLetter: "M", svgPath: RAW.mimo },
  doubao:      { id: "doubao",      brandColor: "#2563eb", fallbackLetter: "D", svgPath: RAW.doubao },
  commandcode: { id: "commandcode", brandColor: "#44ff00", fallbackLetter: "C", svgPath: RAW.commandcode },
  crof:        { id: "crof",        brandColor: "#7c3aed", fallbackLetter: "C", svgPath: RAW.crof },
  crossmodel:  { id: "crossmodel",  brandColor: "#c084fc", fallbackLetter: "X", svgPath: RAW.crossmodel },
  qoder:       { id: "qoder",       brandColor: "#2563eb", fallbackLetter: "Q", svgPath: RAW.qoder },
  codebuddy:   { id: "codebuddy",   brandColor: "#0052d9", fallbackLetter: "C" },
  sakana:      { id: "sakana",      brandColor: "#0ea5e9", fallbackLetter: "S", svgPath: RAW.sakana },
  stepfun:     { id: "stepfun",     brandColor: "#999999", fallbackLetter: "S", svgPath: RAW.stepfun },
  sub2api:     { id: "sub2api",     brandColor: "#2dc6d8", fallbackLetter: "S", svgPath: RAW.sub2api },
  venice:      { id: "venice",      brandColor: "#111827", fallbackLetter: "V", svgPath: RAW.venice },
  openaiapi:   { id: "openaiapi",   brandColor: "#10a37f", fallbackLetter: "O" },
  chutes:      { id: "chutes",      brandColor: "#ff5c35", fallbackLetter: "C" },
  litellm:     { id: "litellm",     brandColor: "#0ea5e9", fallbackLetter: "L" },
  poe:         { id: "poe",         brandColor: "#5d5fef", fallbackLetter: "P" },
  devin:       { id: "devin",       brandColor: "#111827", fallbackLetter: "D" },
  zed:         { id: "zed",         brandColor: "#084ccf", fallbackLetter: "Z" },
  qwencloud:   { id: "qwencloud",   brandColor: "#615CED", fallbackLetter: "Q" },
  notion:      { id: "notion",      brandColor: "#337EA9", fallbackLetter: "N", svgPath: RAW.notion },
  xai:         { id: "xai",         brandColor: "#8e8e93", fallbackLetter: "X", svgPath: RAW.xai },
};

const ALIASES: Record<string, string> = {
  droid: "factory",
  "z.ai": "zai",
  "vertex ai": "vertexai",
  "jetbrains ai": "jetbrains",
  "kimi k2": "kimik2",
  tongyi: "alibaba",
  qwen: "qwencloud",
  "qwen cloud": "qwencloud",
  "qwen-cloud": "qwencloud",
  "notion ai": "notion",
  "notion-ai": "notion",
  notionai: "notion",
  qianwen: "alibaba",
  "alibaba token plan": "alibabatokenplan",
  "alibaba-token-plan": "alibabatokenplan",
  "alibaba-token": "alibabatokenplan",
  "bailian-token-plan": "alibabatokenplan",
  "open router": "openrouter",
  "aws bedrock": "bedrock",
  "aws-bedrock": "bedrock",
  "mistral ai": "mistral",
  "warp terminal": "warp",
  "warp ai": "warp",
  manicode: "codebuff",
  "deep seek": "deepseek",
  "deep-seek": "deepseek",
  "deep infra": "deepinfra",
  "deep-infra": "deepinfra",
  di: "deepinfra",
  "ai&": "aiand",
  "ai-and": "aiand",
  "ai and": "aiand",
  "zen-mux": "zenmux",
  "cline-pass": "clinepass",
  "long-cat": "longcat",
  lc: "longcat",
  "neural-watt": "neuralwatt",
  nw: "neuralwatt",
  "zoom-mate": "zoommate",
  "zoom mate": "zoommate",
  codeium: "windsurf",
  "xiaomi mimo": "mimo",
  xiaomimimo: "mimo",
  "command code": "commandcode",
  "command-code": "commandcode",
  "cross model": "crossmodel",
  "cross-model": "crossmodel",
  "sakana ai": "sakana",
  "sakana-ai": "sakana",
  "step fun": "stepfun",
  "step-fun": "stepfun",
  "sub-2-api": "sub2api",
  "sub 2 api": "sub2api",
  "openai api": "openaiapi",
  "openai-api": "openaiapi",
  "azure openai": "azureopenai",
  "azure-openai": "azureopenai",
  "t3 chat": "t3chat",
  "t3-chat": "t3chat",
  // xai is its own Management API provider (not an alias of consumer Grok).
  "x.ai": "xai",
  "x-ai": "xai",
  supergrok: "grok",
  "super-grok": "grok",
  "eleven labs": "elevenlabs",
  "eleven-labs": "elevenlabs",
  "11labs": "elevenlabs",
  dg: "deepgram",
  groqcloud: "groq",
  "groq cloud": "groq",
  "groq-cloud": "groq",
  "llm proxy": "llmproxy",
  "llm-proxy": "llmproxy",
  "chutes ai": "chutes",
  "chutes-ai": "chutes",
  "lite llm": "litellm",
  "lite-llm": "litellm",
  "zed ai": "zed",
  "zed-ai": "zed",
};

function normalize(id: string): string {
  const lower = id.toLowerCase();
  const aliased = ALIASES[lower];
  if (aliased) return aliased;
  return lower.replace(/[ \-]/g, "");
}

/** Return the registry entry for a provider id, falling back to a generic one. */
export function getProviderIcon(id: string): ProviderIcon {
  const key = normalize(id);
  return (
    PROVIDER_ICON_REGISTRY[key] ?? {
      id: key,
      brandColor: "#5d87ff",
      fallbackLetter: id.charAt(0).toUpperCase() || "●",
    }
  );
}
