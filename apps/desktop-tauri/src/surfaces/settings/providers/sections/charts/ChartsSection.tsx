import { type CSSProperties, useEffect, useState } from "react";
import { getProviderChartData, getSettingsSnapshot } from "../../../../../lib/tauri";
import { providerSupportsChartData } from "../../../../../lib/providerCharts";
import type { ProviderChartData, SettingsSnapshot } from "../../../../../types/bridge";
import type { useLocale } from "../../../../../hooks/useLocale";
import { CostHistoryChart } from "./CostHistoryChart";
import { CreditsHistoryChart } from "./CreditsHistoryChart";
import { TokensHistoryChart } from "./TokensHistoryChart";
import { UsageBreakdownChart } from "./UsageBreakdownChart";

type T = ReturnType<typeof useLocale>["t"];

interface Props {
  providerId: string;
  accountEmail: string | null;
  /** Per-provider accent color override (hex); applied as CSS --provider-accent. */
  accentColor?: string;
  t: T;
}

type TabKey = "tokens" | "cost" | "credits" | "usage";

/**
 * Charts tabs block for the Settings → Providers detail pane.
 *
 * Port target: cost_history / credits_history / usage_breakdown blocks
 * in `rust/src/native_ui/preferences.rs::render_provider_detail_panel`.
 *
 * Phase 10: fetches the latest settings snapshot so the animation flag feeds
 * through to each chart component.
 */
export function ChartsSection({ providerId, accountEmail, accentColor, t }: Props) {
  const [data, setData] = useState<ProviderChartData | null>(null);
  const [active, setActive] = useState<TabKey | null>(null);
  const [animations, setAnimations] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setData(null);
    setActive(null);
    if (!providerSupportsChartData(providerId)) {
      return () => {
        cancelled = true;
      };
    }
    getProviderChartData(providerId, accountEmail ?? undefined)
      .then((d) => {
        if (!cancelled) setData(d);
      })
      .catch(() => {
        if (!cancelled) setData(null);
      });
    return () => {
      cancelled = true;
    };
  }, [providerId, accountEmail]);

  useEffect(() => {
    let cancelled = false;
    getSettingsSnapshot()
      .then((s: SettingsSnapshot) => {
        if (!cancelled) {
          setAnimations(s.enableAnimations);
        }
      })
      .catch(() => {
        // Keep defaults on failure.
      });
    return () => {
      cancelled = true;
    };
  }, [providerId]);

  if (!data) return null;

  const hasCost = data.costHistory.length > 0;
  const hasCredits = data.creditsHistory.length > 0;
  const hasUsage = data.usageBreakdown.length > 0;
  // Tokens mode needs at least one day with exact local token data.
  const hasTokens = data.tokensHistory.some((p) => p.tokens > 0);

  if (!hasCost && !hasCredits && !hasUsage && !hasTokens) return null;

  const available: TabKey[] = [];
  if (hasCost) available.push("cost");
  if (hasCredits) available.push("credits");
  if (hasUsage) available.push("usage");
  if (hasTokens) available.push("tokens");

  // Upstream 0.50.0 #2930: Codex defaults to exact local token totals.
  const defaultTab: TabKey =
    providerId === "codex" && hasTokens ? "tokens" : available[0];
  const current: TabKey =
    active && available.includes(active) ? active : defaultTab;
  const emptyMsg = t("DetailChartEmpty");

  const tabLabel = (k: TabKey): string => {
    if (k === "tokens") return t("DetailChartTokens");
    if (k === "cost") return t("DetailChartCost");
    if (k === "credits") return t("DetailChartCredits");
    return t("DetailChartUsageBreakdown");
  };
  return (

    <section
      className="provider-detail-section provider-detail-charts"
      style={accentColor ? ({ "--provider-accent": accentColor } as CSSProperties) : undefined}
    >
      <div className="provider-detail-charts__tabs" role="tablist">
        {available.map((k) => (
          <button
            key={k}
            type="button"
            role="tab"
            aria-selected={k === current}
            className="provider-detail-charts__tab"
            data-active={k === current ? "true" : "false"}
            onClick={() => setActive(k)}
          >
            {tabLabel(k)}
          </button>
        ))}
      </div>
      <div className="provider-detail-charts__body" role="tabpanel">
        {current === "tokens" && (
          <TokensHistoryChart
            data={data.tokensHistory}
            title={t("DetailChartTokens")}
            ariaLabel={t("DetailChartTokens")}
            providerId={providerId}
            animations={animations}
            emptyMessage={emptyMsg}
            incomplete={data.tokensIncomplete}
            t={t}
          />
        )}
        {current === "cost" && (
          <CostHistoryChart
            data={data.costHistory}
            title={t("DetailChartCost")}
            ariaLabel={t("DetailChartCost")}
            providerId={providerId}
            animations={animations}
            emptyMessage={emptyMsg}
          />
        )}
        {current === "credits" && (
          <CreditsHistoryChart
            data={data.creditsHistory}
            title={t("DetailChartCredits")}
            ariaLabel={t("DetailChartCredits")}
            providerId={providerId}
            animations={animations}
            emptyMessage={emptyMsg}
          />
        )}
        {current === "usage" && (
          <UsageBreakdownChart
            data={data.usageBreakdown}
            title={t("DetailChartUsageBreakdown")}
            ariaLabel={t("DetailChartUsageBreakdown")}
            animations={animations}
            emptyMessage={emptyMsg}
          />
        )}
      </div>
    </section>
  );
}
