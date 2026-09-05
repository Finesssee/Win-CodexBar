import { useState } from "react";
import type {
  CostSummaryDisplayStyle,
  DailyCostPoint,
  PaceSnapshot,
  ProviderChartData,
  ProviderLocalUsageSummary,
  ProviderUsageSnapshot,
  RateWindowSnapshot,
  SessionEquivalentForecastSnapshot,
} from "../types/bridge";
import { useLocale } from "../hooks/useLocale";
import {
  useFormattedResetTime,
  type ResetTimeFormatMode,
} from "../hooks/useFormattedResetTime";
import { formatEta } from "../lib/formatEta";
import type { LocaleKey } from "../i18n/keys";
import { paceCategory } from "../surfaces/tray/paceCategory";
import { SimpleBarChart, StackedBarChart } from "./MiniBarChart";
import { getPaceBudget, type PaceBudget } from "../lib/paceBudget";
import PaceDetailsChart from "./PaceDetailsChart";

/** Format a reserve description from raw pace data at render time. */
function formatReserveDescription(
  snap: RateWindowSnapshot,
  t: (key: LocaleKey) => string,
): string | null {
  if (snap.reservePercent == null) return null;
  if (snap.reserveWillLastToReset) {
    return t("PanelReserveLastsUntilReset");
  }
  const eta = snap.reserveEtaSeconds;
  if (eta == null) return null;
  const h = Math.floor(eta / 3600);
  if (h >= 24) {
    return t("PanelReserveRunsOutInDaysHours")
      .replace("{}", String(Math.floor(h / 24)))
      .replace("{}", String(h % 24));
  }
  return t("PanelReserveRunsOutInHours").replace("{}", String(h));
}

/** Upstream session-quota estimate: "Estimated: {n} session quota(s) left". */
function formatSessionEquivalentEstimate(
  forecast: SessionEquivalentForecastSnapshot | null | undefined,
): string | null {
  if (!forecast) return null;
  const raw = forecast.estimatedWindowsToExhaustWeekly;
  if (!Number.isFinite(raw)) return null;
  const rounded = Math.round(Math.min(Math.max(raw, 0), 1_000_000) * 10) / 10;
  const display =
    Number.isInteger(rounded) || Math.abs(rounded - Math.round(rounded)) < 1e-9
      ? String(Math.round(rounded))
      : rounded.toFixed(1);
  const unit =
    rounded > 0 && rounded <= 1 ? "session quota" : "session quotas";
  return `Estimated: ${display} ${unit} left`;
}

const currencyFormatters = new Map<string, Intl.NumberFormat>();
const compactCountFormat0 = new Intl.NumberFormat("en-US", {
  notation: "compact",
  maximumFractionDigits: 0,
});
const compactCountFormat1 = new Intl.NumberFormat("en-US", {
  notation: "compact",
  maximumFractionDigits: 1,
});

function formatCurrency(amount: number, code: string): string {
  try {
    let formatter = currencyFormatters.get(code);
    if (!formatter) {
      formatter = new Intl.NumberFormat("en-US", {
        style: "currency",
        currency: code,
      });
      currencyFormatters.set(code, formatter);
    }
    return formatter.format(amount);
  } catch {
    return `${code} ${amount.toFixed(2)}`;
  }
}

function formatCompactCount(value: number | null): string {
  if (value == null || value <= 0) return "—";
  return (value >= 1_000_000 ? compactCountFormat1 : compactCountFormat0).format(
    value,
  );
}

function formatBudget(value: number): string {
  return value < 10
    ? value.toFixed(1).replace(/\.0$/, "")
    : Math.round(value).toString();
}

function LocalUsageBlock({
  providerId,
  summary,
  costHistory,
}: {
  providerId: string;
  summary: ProviderLocalUsageSummary;
  costHistory: DailyCostPoint[];
}) {
  const { t } = useLocale();
  const isCodex = providerId === "codex";
  const visibleHistory = costHistory.slice(-30);
  const maxCost = Math.max(
    ...visibleHistory.flatMap((point) => (point.value == null ? [] : [point.value])),
    0,
  );

  return (
    <section className="menu-card__group menu-card__local-usage">
      <div className="menu-card__local-grid">
        <div>
          <span className="menu-card__local-label">{t("PanelToday")}</span>
          <strong>
            {summary.todayCost != null
              ? formatCurrency(summary.todayCost, "USD")
              : "—"}
          </strong>
        </div>
        <div>
          <span className="menu-card__local-label">{t("PanelThirtyDayCost")}</span>
          <strong>
            {summary.thirtyDayCost != null
              ? formatCurrency(summary.thirtyDayCost, "USD")
              : "—"}
          </strong>
        </div>
        <div>
          <span className="menu-card__local-label">{t("PanelThirtyDayTokens")}</span>
          <strong>{formatCompactCount(summary.thirtyDayTokens)}</strong>
        </div>
        <div>
          <span className="menu-card__local-label">{t("PanelLatestTokens")}</span>
          <strong>{formatCompactCount(summary.latestTokens)}</strong>
        </div>
      </div>

      {isCodex && visibleHistory.length > 0 && (
        <div className="menu-card__local-chart" aria-label={t("PanelThirtyDayCostHistogram")}>
          {visibleHistory.map((point, index) => (
            <span
              key={`${point.date}-${index}`}
              style={{
                height: `${point.value == null || maxCost <= 0 ? 1 : Math.max(4, Math.round((point.value / maxCost) * 64))}px`,
                opacity: point.value == null ? 0 : undefined,
              }}
              title={`${point.date}: ${point.value == null ? "Unknown" : formatCurrency(point.value, "USD")}`}
            />
          ))}
        </div>
      )}

      <div className="menu-card__local-note">
        {summary.topModel && <strong>{t("PanelTopModelPrefix")}: {summary.topModel}</strong>}
        <span>
          {summary.estimateNote === "Estimated from local logs"
            ? t("PanelEstimatedFromLocalLogs")
            : summary.estimateNote}
        </span>
      </div>
    </section>
  );
}

function WayfinderUsageBlock({
  usage,
}: {
  usage: NonNullable<ProviderUsageSnapshot["wayfinderUsage"]>;
}) {
  const { t } = useLocale();
  const formatAmount = (value: number) =>
    usage.priced ? `${value.toFixed(4)} ${usage.unit.toUpperCase()}` : "—";

  return (
    <section className="menu-card__group">
      <div className="menu-card__local-grid">
        <div>
          <span className="menu-card__local-label">{t("WayfinderGatewayStatus")}</span>
          <strong>{usage.gatewayStatus}</strong>
        </div>
        <div>
          <span className="menu-card__local-label">{t("WayfinderModels")}</span>
          <strong>{usage.modelCount}</strong>
        </div>
        <div>
          <span className="menu-card__local-label">{t("WayfinderRequests")}</span>
          <strong>{formatCompactCount(usage.requests)}</strong>
        </div>
        <div>
          <span className="menu-card__local-label">{t("WayfinderTokens")}</span>
          <strong>{formatCompactCount(usage.tokens)}</strong>
        </div>
      </div>
      <div className="menu-card__cost-line">
        {t("WayfinderSaved")}: {formatAmount(usage.saved)} ({usage.savedPercent.toFixed(1)}%)
      </div>
      {(usage.offline || usage.dryRun || usage.missingKeys.length > 0) && (
        <div className="menu-card__local-note">
          {usage.offline && <span>{t("WayfinderOffline")}</span>}
          {usage.dryRun && <span>{t("WayfinderDryRun")}</span>}
          {usage.missingKeys.length > 0 && (
            <span>{t("WayfinderMissingKeys")}: {usage.missingKeys.join(", ")}</span>
          )}
        </div>
      )}
    </section>
  );
}

function paceStageKey(stage: PaceSnapshot["stage"]): LocaleKey {
  switch (stage) {
    case "on_track":
      return "DetailPaceOnTrack";
    case "slightly_ahead":
      return "DetailPaceSlightlyAhead";
    case "ahead":
      return "DetailPaceAhead";
    case "far_ahead":
      return "DetailPaceFarAhead";
    case "slightly_behind":
      return "DetailPaceSlightlyBehind";
    case "behind":
      return "DetailPaceBehind";
    case "far_behind":
      return "DetailPaceFarBehind";
    default:
      return "DetailPaceOnTrack";
  }
}

type UsageLevel = "normal" | "high" | "critical" | "exhausted";
const WEEKLY_WINDOW_MINUTES = 7 * 24 * 60;

function levelOf(remainPct: number, exhausted: boolean): UsageLevel {
  if (exhausted) return "exhausted";
  if (remainPct <= 5) return "critical";
  if (remainPct <= 25) return "high";
  return "normal";
}

export interface MetricEntry {
  id: string;
  label: string;
  snap: RateWindowSnapshot;
  resetFormatMode?: ResetTimeFormatMode;
  sessionEquivalentForecast?: SessionEquivalentForecastSnapshot | null;
}

type MetricPaceView =
  | { kind: "budget"; budget: PaceBudget }
  | { kind: "reserve"; percent: number }
  | { kind: "none" };

function getMetricPaceView(snap: RateWindowSnapshot): MetricPaceView {
  if (snap.isExhausted) return { kind: "none" };

  const isWeeklyWindow =
    snap.windowMinutes != null && snap.windowMinutes >= WEEKLY_WINDOW_MINUTES;
  const budget = isWeeklyWindow ? getPaceBudget(snap) : null;
  if (budget) return { kind: "budget", budget };

  if (snap.reservePercent != null) {
    return { kind: "reserve", percent: snap.reservePercent };
  }

  return { kind: "none" };
}
type MetricRowDisplay = {
  resetTimeRelative: boolean;
  showResetWhenExhausted?: boolean;
  showPace?: boolean;
  showAsUsed?: boolean;
  costSummaryDisplayStyle?: CostSummaryDisplayStyle;
};

/**
 * Single metric row inside the card — mirrors upstream `MetricRow`:
 *   • title (body / medium)
 *   • UsageProgressBar (capsule, 6pt)
 *   • HStack: "N% used"  ··  reset countdown (right-aligned, secondary)
 */
function MetricRow({
  title,
  snap,
  exhaustedLabel,
  display,
  expanded,
  onToggleExpanded,
  resetFormatMode,
  sessionEquivalentForecast,
}: {
  title: string;
  snap: RateWindowSnapshot;
  exhaustedLabel: string;
  display: MetricRowDisplay;
  expanded: boolean;
  onToggleExpanded: () => void;
  resetFormatMode?: ResetTimeFormatMode;
  sessionEquivalentForecast?: SessionEquivalentForecastSnapshot | null;
}) {
  const { t } = useLocale();
  const {
    resetTimeRelative,
    showResetWhenExhausted = false,
    showPace = true,
    showAsUsed = false,
  } = display;
  const isInformational = snap.isInformational === true;
  const usedPct = Number.isFinite(snap.usedPercent) ? Math.max(0, snap.usedPercent) : 0;
  const barPct = Math.min(100, usedPct);
  const remain = 100 - usedPct;
  const displayPct = showAsUsed ? usedPct : Math.max(0, remain);
  const barDisplayPct = showAsUsed ? barPct : Math.max(0, Math.min(100, remain));
  const displayLabel = showAsUsed ? t("PanelUsedSuffix") : t("PanelLeftSuffix");
  const level = levelOf(remain, snap.isExhausted);
  const resetText = useFormattedResetTime(
    snap.resetsAt,
    isInformational ? null : snap.resetDescription,
    resetTimeRelative,
    resetFormatMode ?? "reset",
  );
  const infoPrimary = snap.resetDescription?.trim() || resetText || "—";
  const resetTarget = snap.resetsAt ? Date.parse(snap.resetsAt) : Number.NaN;
  const replacesPercent =
    showResetWhenExhausted &&
    snap.isExhausted &&
    Number.isFinite(resetTarget) &&
    resetTarget > Date.now() &&
    resetText !== null;
  const paceView = showPace ? getMetricPaceView(snap) : { kind: "none" as const };
  const reserveDescription = formatReserveDescription(snap, t);
  const forecastText = formatSessionEquivalentEstimate(sessionEquivalentForecast);
  return (
    <div className="menu-metric">
      <span className="menu-metric__title">{title}</span>
      {!isInformational && (
        <div className="menu-metric__bar">
          <div className="menu-metric__bar-fill" data-level={level} style={{ width: `${barDisplayPct}%` }} />
        </div>
      )}
      <div className="menu-metric__row">
        <span className="menu-metric__pct">
          {isInformational
            ? infoPrimary
            : replacesPercent
              ? resetText
              : `${Math.round(displayPct)}% ${displayLabel}`}
        </span>
        {isInformational &&
          snap.resetDescription?.trim() &&
          resetText &&
          resetText !== infoPrimary && (
            <span className="menu-metric__reset">{resetText}</span>
          )}
        {!isInformational && resetText && !replacesPercent && (
          <span className="menu-metric__reset">{resetText}</span>
        )}
      </div>
      {!isInformational && snap.isExhausted && (
        <div className="menu-metric__exhausted">{exhaustedLabel}</div>
      )}
      {!isInformational && paceView.kind === "budget" && (
        <div className="menu-metric__budget">
          <button
            type="button"
            className="menu-metric__budget-header"
            onClick={onToggleExpanded}
            aria-expanded={expanded}
          >
            <span>{t("PanelOnPaceBudget")}</span>
            {reserveDescription && <span>{reserveDescription}</span>}
          </button>
          <div className="menu-metric__budget-pills">
            {[
              [t("PanelNow"), paceView.budget.now],
              [t("PanelOneHour"), paceView.budget.nextHour],
              [t("PanelFiveHours"), paceView.budget.nextFiveHours],
              [t("PanelTodayBudget"), paceView.budget.today],
            ].map(([label, value]) => (
              <span className="menu-metric__budget-pill" key={String(label)}>
                {label} {formatBudget(Number(value))}%
              </span>
            ))}
          </div>
          {expanded && <PaceDetailsChart snap={snap} t={t} />}
        </div>
      )}
      {!isInformational && paceView.kind === "reserve" && (
        <div className="menu-metric__row menu-metric__reserve">
          <span className="menu-metric__pct">{Math.round(paceView.percent)}% {t("PanelReserveSuffix")}</span>
          {reserveDescription && (
            <span className="menu-metric__reset">{reserveDescription}</span>
          )}
        </div>
      )}
      {showPace && !isInformational && forecastText && (
        <div className="menu-metric__row menu-metric__forecast">
          <span className="menu-metric__pct">{forecastText}</span>
        </div>
      )}
    </div>
  );
}

export interface MenuCardPresence {
  hasMetrics: boolean;
  hasCost: boolean;
  hasPace: boolean;
  hasCharts: boolean;
  hasCostHistory: boolean;
  hasCreditsHistory: boolean;
  hasUsageBreakdown: boolean;
  localUsage: ProviderChartData["localUsage"] | null;
  wayfinderUsage: ProviderUsageSnapshot["wayfinderUsage"] | null;
  hasDetails: boolean;
}

export interface MenuCardDetailsProps {
  provider: ProviderUsageSnapshot;
  display: MetricRowDisplay;
  metrics: MetricEntry[];
  chartData: ProviderChartData | null;
  presence: MenuCardPresence;
  onLayoutChange?: () => void;
}

/**
 * Single source of truth for "does this card have a body" and which sections
 * are present. Pure; computed once in `MenuCard` and threaded into
 * `MenuCardDetails` so the two files never diverge on the predicate suite.
 */
export function describeCard(
  provider: ProviderUsageSnapshot,
  chartData: ProviderChartData | null,
  visibleMetrics: MetricEntry[],
  costSummaryDisplayStyle: CostSummaryDisplayStyle = "detailed",
  showPace = true,
): MenuCardPresence {
  const hasCostHistory =
    chartData !== null && chartData.costHistory.some((point) => point.value != null);
  const hasCreditsHistory =
    chartData !== null && chartData.creditsHistory.length > 0;
  const hasUsageBreakdown =
    chartData !== null && chartData.usageBreakdown.length > 0;
  const hasCharts = hasCostHistory || hasCreditsHistory || hasUsageBreakdown;
  const isWayfinder = provider.providerId === "wayfinder";
  const localUsage = provider.error ? null : chartData?.localUsage ?? null;
  const wayfinderUsage = isWayfinder ? provider.wayfinderUsage : null;
  const hasMetrics = visibleMetrics.length > 0;
  const hasCost =
    !!provider.cost &&
    (costSummaryDisplayStyle !== "hidden" || provider.cost.alwaysVisible === true);
  const hasPace = showPace && !!provider.pace;
  const hasDetails =
    !provider.error &&
    (hasMetrics || hasCost || hasPace || hasCharts || !!localUsage || !!wayfinderUsage);
  return {
    hasMetrics,
    hasCost,
    hasPace,
    hasCharts,
    hasCostHistory,
    hasCreditsHistory,
    hasUsageBreakdown,
    localUsage,
    wayfinderUsage,
    hasDetails,
  };
}

/** Metrics / cost / pace / charts body of a provider MenuCard. */
export default function MenuCardDetails({
  provider,
  display,
  metrics,
  chartData,
  presence,
  onLayoutChange,
}: MenuCardDetailsProps) {
  const { t } = useLocale();
  const [expandedPaceWindow, setExpandedPaceWindow] = useState<string | null>(null);
  const formattedCostReset = useFormattedResetTime(
    provider.cost?.resetsAt ?? null,
    null,
    display.resetTimeRelative,
  );
  const localCostHistory = chartData?.costHistory ?? [];
  const costStyle = display.costSummaryDisplayStyle ?? "detailed";

  const {
    hasMetrics,
    hasCost,
    hasPace,
    hasCharts,
    hasCostHistory,
    hasCreditsHistory,
    hasUsageBreakdown,
    localUsage,
    wayfinderUsage,
  } = presence;

  return (
    <div className="menu-card__content">
      {!provider.error && hasMetrics && (
        <section className="menu-card__group menu-card__metrics">
          {metrics.map((m) => (
            <MetricRow
              key={m.id}
              title={m.label}
              snap={m.snap}
              exhaustedLabel={t("DetailWindowExhausted")}
              display={display}
              expanded={expandedPaceWindow === m.id}
              resetFormatMode={m.resetFormatMode}
              sessionEquivalentForecast={m.sessionEquivalentForecast}
              onToggleExpanded={() => {
                setExpandedPaceWindow((current) =>
                  current === m.id ? null : m.id,
                );
                requestAnimationFrame(() => onLayoutChange?.());
              }}
            />
          ))}
        </section>
      )}

      {wayfinderUsage && <WayfinderUsageBlock usage={wayfinderUsage} />}

      {localUsage && (
        <LocalUsageBlock
          providerId={provider.providerId}
          summary={localUsage}
          costHistory={localCostHistory}
        />
      )}

      {hasMetrics && hasCost && <div className="menu-card__divider" />}

      {hasCost && provider.cost && (
        <section className="menu-card__group menu-card__cost">
          <div className="menu-card__group-title">
            {provider.cost.alwaysVisible === true && (provider.cost.limit ?? 0) <= 0
              ? t("ApiSpendTitle")
              : provider.cost.balance != null && provider.cost.limit == null
                ? provider.cost.period || t("CreditsLabel")
              : `${t("DetailCostTitle")} — ${provider.cost.period}`}
          </div>
          {provider.cost.balance != null && provider.cost.limit == null ? (
            <div className="menu-card__cost-line">
              {provider.cost.formattedBalance ||
                formatCurrency(
                  provider.cost.balance,
                  provider.cost.currencyCode,
                )}
            </div>
          ) : (
            <>
              <div className="menu-card__cost-line">
                {t("DetailCostUsed")}:{" "}
                {provider.cost.formattedUsed ||
                  formatCurrency(
                    provider.cost.used,
                    provider.cost.currencyCode,
                  )}
                {provider.cost.limit != null && (
                  <>
                    {" / "}
                    {provider.cost.formattedLimit ||
                      formatCurrency(
                        provider.cost.limit,
                        provider.cost.currencyCode,
                      )}
                  </>
                )}
              </div>
              {costStyle === "detailed" && provider.cost.balance != null && (
                <div className="menu-card__cost-line menu-card__cost-line--muted">
                  {t("DetailCostBalance")}:{" "}
                  {provider.cost.formattedBalance ||
                    formatCurrency(
                      provider.cost.balance,
                      provider.cost.currencyCode,
                    )}
                </div>
              )}
              {costStyle === "detailed" && provider.cost.remaining != null && (
                <div className="menu-card__cost-line menu-card__cost-line--muted">
                  {t("DetailCostRemaining")}:{" "}
                  {formatCurrency(
                    provider.cost.remaining,
                    provider.cost.currencyCode,
                  )}
                </div>
              )}
              {costStyle === "detailed" && formattedCostReset && (
                <div className="menu-card__cost-line menu-card__cost-line--muted">
                  {t("DetailCostResets")}: {formattedCostReset}
                </div>
              )}
            </>
          )}
          {provider.providerId === "mistral" && provider.cost && (
            <div className="menu-card__cost-line menu-card__monthly-spend">
              {t("MistralMonthlySpend")}:{" "}
              {provider.cost.currencySymbol
                ? `${provider.cost.currencySymbol}${provider.cost.used.toFixed(2)}`
                : provider.cost.formattedUsed}
            </div>
          )}
        </section>
      )}

      {(hasMetrics || hasCost) && hasPace && <div className="menu-card__divider" />}

      {hasPace && provider.pace && (
        <section className="menu-card__group menu-card__pace">
          <div className="menu-card__pace-header">
            <span className="menu-card__group-title">{t("DetailPaceTitle")}</span>
            <span
              className="menu-card__pace-label"
              data-pace={paceCategory(provider.pace.stage)}
            >
              {t(paceStageKey(provider.pace.stage))} (
              {provider.pace.deltaPercent >= 0 ? "+" : ""}
              {provider.pace.deltaPercent.toFixed(1)}%)
            </span>
          </div>
          <div className="menu-card__pace-bars">
            <div className="menu-card__pace-track" title={t("PanelExpected")}>
              <div
                className="menu-card__pace-fill menu-card__pace-fill--expected"
                style={{ width: `${provider.pace.expectedUsedPercent.toFixed(1)}%` }}
              />
            </div>
            <div className="menu-card__pace-track" title={t("PanelActual")}>
              <div
                className="menu-card__pace-fill"
                data-pace={paceCategory(provider.pace.stage)}
                style={{ width: `${provider.pace.actualUsedPercent.toFixed(1)}%` }}
              />
            </div>
          </div>
          {provider.pace.etaSeconds != null && !provider.pace.willLastToReset && (
            <div className="menu-card__pace-eta">
              ⚠{" "}
              {t("DetailPaceRunsOutIn")} {formatEta(provider.pace.etaSeconds)}
            </div>
          )}
          {provider.pace.willLastToReset && (
            <div className="menu-card__pace-ok">
              ✓ {t("DetailPaceWillLastToReset")}
            </div>
          )}
        </section>
      )}

      {(hasMetrics || hasCost || hasPace) && hasCharts && (
        <div className="menu-card__divider" />
      )}

      {hasCharts && (
        <section className="menu-card__group menu-card__charts">
          {hasCostHistory && (
            <SimpleBarChart
              points={chartData!.costHistory}
              label={t("DetailChartCost")}
              color="var(--provider-accent, var(--accent))"
              formatValue={(v) => `$${v.toFixed(2)}`}
              t={t}
            />
          )}
          {hasCreditsHistory && (
            <SimpleBarChart
              points={chartData!.creditsHistory}
              label={t("DetailChartCredits")}
              color="var(--provider-status-ok)"
              formatValue={(v) => v.toFixed(1)}
              t={t}
            />
          )}
          {hasUsageBreakdown && (
            <StackedBarChart
              points={chartData!.usageBreakdown}
              label={t("DetailChartUsageBreakdown")}
              height={56}
              t={t}
            />
          )}
        </section>
      )}
    </div>
  );
}
