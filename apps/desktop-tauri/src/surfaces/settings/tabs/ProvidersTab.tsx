import { useMemo, useState } from "react";
import type {
  ProviderCatalogEntry,
  ProviderUsageSnapshot,
  SettingsUpdate,
} from "../../../types/bridge";
import type { BootstrapState } from "../../../types/bridge";
import { useLocale } from "../../../hooks/useLocale";
import type { LocaleKey } from "../../../i18n/keys";
import {
  ProvidersSidebar,
  type ProviderSidebarRow,
  type ProviderSidebarStatus,
} from "../providers/ProvidersSidebar";
import { ProviderDetailPane } from "../providers/ProviderDetailPane";
import { reorderProviders } from "../../../lib/tauri";
import { selectSingleMetricUsageWindow } from "../../../lib/usageWindows";
import { useProviders } from "../../../hooks/useProviders";

interface ProvidersTabProps {
  settings: BootstrapState["settings"];
  providers: ProviderCatalogEntry[];
  set: (patch: SettingsUpdate) => void;
  saving: boolean;
}

export default function ProvidersTab({
  settings,
  providers,
  set,
  saving,
}: ProvidersTabProps) {
  const { t } = useLocale();
  const { providers: snapshots } = useProviders();
  const [selectedId, setSelectedId] = useState<string | null>(
    providers[0]?.id ?? null,
  );
  // Locally-owned catalog order so drag-reorder feels instant before the
  // backend `reorder_providers` round-trip settles.
  const [orderedProviders, setOrderedProviders] =
    useState<ProviderCatalogEntry[]>(providers);
  const [prevProviders, setPrevProviders] = useState(providers);
  const [searchText, setSearchText] = useState("");
  if (providers !== prevProviders) {
    setPrevProviders(providers);
    setOrderedProviders(providers);
  }

  const enabled = useMemo(
    () => new Set(settings.enabledProviders),
    [settings.enabledProviders],
  );

  const toggle = (id: string, on: boolean) => {
    const next = new Set(enabled);
    if (on) next.add(id);
    else next.delete(id);
    set({
      enabledProviders: orderedProviders.reduce<string[]>((ids, provider) => {
        if (next.has(provider.id)) ids.push(provider.id);
        return ids;
      }, []),
    });
  };

  const rows: ProviderSidebarRow[] = useMemo(() => {
    const snapshotMap = new Map(snapshots.map((s) => [s.providerId, s]));
    return orderedProviders.map((p) => {
      const isOn = enabled.has(p.id);
      const snap = snapshotMap.get(p.id) ?? null;
      return {
        id: p.id,
        displayName: p.displayName,
        enabled: isOn,
        status: deriveProviderStatus(isOn, snap),
        subtitlePrimary: providerSidebarSubtitle(p.id, isOn, snap, t),
        subtitleSecondary: providerSidebarMetric(snap),
      };
    });
  }, [enabled, orderedProviders, snapshots, t]);

  const normalizedSearch = searchText.trim().toLowerCase();
  const visibleRows = useMemo(
    () =>
      normalizedSearch
        ? rows.filter((row) => {
            const name = row.displayName.toLowerCase();
            const id = row.id.toLowerCase();
            return name.includes(normalizedSearch) || id.includes(normalizedSearch);
          })
        : rows,
    [normalizedSearch, rows],
  );

  // Derive selection from visible rows — no effect to mirror/adjust state.
  const resolvedSelectedId =
    visibleRows.length === 0
      ? null
      : selectedId && visibleRows.some((row) => row.id === selectedId)
        ? selectedId
        : visibleRows[0].id;

  const handleReorder = (ids: string[]) => {
    const byId = new Map(orderedProviders.map((p) => [p.id, p]));
    const nextIds = normalizedSearch
      ? mergeFilteredOrder(
          orderedProviders.map((p) => p.id),
          new Set(visibleRows.map((row) => row.id)),
          ids,
        )
      : ids;
    const next = nextIds
      .map((id) => byId.get(id))
      .filter((p): p is ProviderCatalogEntry => Boolean(p));
    setOrderedProviders(next);
    void reorderProviders(nextIds).catch(() => {
      setOrderedProviders(providers);
    });
  };

  const selectedEntry =
    orderedProviders.find((p) => p.id === resolvedSelectedId) ?? null;

  return (
    <div className="provider-split">
      <ProvidersSidebar
        providers={visibleRows}
        selectedId={resolvedSelectedId}
        searchText={searchText}
        onSearchTextChange={setSearchText}
        onSelect={setSelectedId}
        onReorder={handleReorder}
        onToggleEnabled={toggle}
        disabled={saving}
      />
      <ProviderDetailPane
        providerId={resolvedSelectedId}
        cookieDomain={selectedEntry?.cookieDomain ?? null}
        resetTimeRelative={settings.resetTimeRelative}
        providerMetrics={settings.providerMetrics}
        providerAccentColors={settings.providerAccentColors}
        wayfinderGatewayUrl={settings.wayfinderGatewayUrl ?? "http://127.0.0.1:8088"}
        settingsDisabled={saving}
        onSettingsChange={set}
      />
    </div>
  );
}

function mergeFilteredOrder(
  fullOrder: string[],
  visibleIds: Set<string>,
  reorderedVisibleIds: string[],
): string[] {
  const nextVisible = [...reorderedVisibleIds];
  return fullOrder.map((id) =>
    visibleIds.has(id) ? (nextVisible.shift() ?? id) : id,
  );
}

// ── Provider sidebar subtitle helpers (port of
//    rust/src/native_ui/preferences.rs::provider_sidebar_subtitle). ─────

function deriveProviderStatus(
  isEnabled: boolean,
  snap: ProviderUsageSnapshot | null,
): ProviderSidebarStatus {
  if (!isEnabled) return "disabled";
  if (!snap) return "loading";
  if (snap.error) return "error";
  const updatedMs = new Date(snap.updatedAt).getTime();
  if (Number.isFinite(updatedMs)) {
    const ageMins = (Date.now() - updatedMs) / 60_000;
    if (ageMins > 10) return "stale";
  }
  return "ok";
}

/**
 * Minimal port of `provider_sidebar_source_hint`
 * (rust/src/native_ui/preferences.rs:3753). When we have a live snapshot the
 * backend-supplied `sourceLabel` wins; otherwise we fall back to the neutral
 * "Not detected" / "Disabled" copy.
 */
function providerSidebarSubtitle(
  providerId: string,
  isEnabled: boolean,
  snap: ProviderUsageSnapshot | null,
  t: (key: LocaleKey) => string,
): string {
  if (!isEnabled) {
    return `${t("ProviderDisabled")} — ${providerSourceHintShort(providerId, t)}`;
  }
  if (!snap) {
    return t("WaitingForUsage");
  }
  const source = snap.sourceLabel || providerSourceHintShort(providerId, t);
  return source;
}

function providerSourceHintShort(
  providerId: string,
  t: (key: LocaleKey) => string,
): string {
  const id = providerId.toLowerCase();
  switch (id) {
    case "cursor":
    case "factory":
    case "droid":
    case "kimi":
    case "kimik2":
    case "augment":
    case "opencode":
    case "amp":
    case "ollama":
    case "alibaba":
    case "infini":
    case "manus":
    case "mimo":
    case "zoommate":
    case "notion":
    case "t3chat":
    case "commandcode":
      return t("ProviderSourceWebShort");
    case "gemini":
    case "antigravity":
    case "jetbrains":
      return t("ProviderSourceCliShort");
    case "copilot":
      return t("ProviderSourceOauthShort");
    case "zai":
    case "vertexai":
    case "openrouter":
    case "bedrock":
    case "nanogpt":
    case "warp":
    case "deepinfra":
    case "aiand":
    case "zenmux":
    case "clinepass":
    case "neuralwatt":
    case "doubao":
    case "crof":
    case "stepfun":
    case "venice":
    case "openaiapi":
    case "elevenlabs":
    case "deepgram":
    case "groq":
    case "llmproxy":
    case "xai":
    case "fireworks":
      return t("ProviderSourceApiShort");
    case "kiro":
      return t("ProviderSourceKiroEnvShort");
    case "claude":
    case "codex":
    case "minimax":
    default:
      return t("ProviderSourceAutoShort");
  }
}

function providerSidebarMetric(
  snap: ProviderUsageSnapshot | null,
): string | undefined {
  if (!snap) return undefined;
  const rate = selectSingleMetricUsageWindow(snap);
  if (Number.isFinite(rate.usedPercent)) {
    return `${Math.round(Math.max(0, rate.usedPercent))}%`;
  }
  return undefined;
}
