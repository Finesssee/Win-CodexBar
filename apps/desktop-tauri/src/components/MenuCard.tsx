import { useCallback, useEffect, useState } from "react";
import type {
  DailyCostPoint,
  PaceSnapshot,
  ProviderChartData,
  ProviderLocalUsageSummary,
  ProviderUsageSnapshot,
  RateWindowSnapshot,
  SessionEquivalentForecastSnapshot,
} from "../types/bridge";
import { getProviderChartData } from "../lib/tauri";
import { useLocale } from "../hooks/useLocale";
import {
  useFormattedResetTime,
  type ResetTimeFormatMode,
} from "../hooks/useFormattedResetTime";
import { formatRelativeUpdated } from "../lib/relativeTime";
import { formatEta } from "../lib/formatEta";
import type { LocaleKey } from "../i18n/keys";
import { paceCategory } from "../surfaces/tray/paceCategory";
import { SimpleBarChart, StackedBarChart } from "./MiniBarChart";
import { providerSupportsChartData } from "../lib/providerCharts";
import { getPaceBudget } from "../lib/paceBudget";
import PaceDetailsChart from "./PaceDetailsChart";

/** Small copy-to-clipboard button matching macOS CopyIconButton (doc.on.doc → checkmark). */
function CopyIconButton({ text }: { text: string }) {
  const { t } = useLocale();
  const [copied, setCopied] = useState(false);
  const handleCopy = useCallback(() => {
    navigator.clipboard.writeText(text).catch(() => {});
    setCopied(true);
    setTimeout(() => setCopied(false), 900);
  }, [text]);
  return (
    <button
      type="button"
      className="menu-card__copy-btn"
      onClick={handleCopy}
      aria-label={copied ? t("PanelCopied") : t("ActionCopyError")}
      title={copied ? t("PanelCopied") : t("ActionCopyError")}
    >
      {copied ? "✓" : (
        <svg width="12" height="12" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">
          <rect x="5" y="5" width="9" height="9" rx="1.5" stroke="currentColor" strokeWidth="1.5"/>
          <path d="M11 3V2.5A1.5 1.5 0 009.5 1H2.5A1.5 1.5 0 001 2.5v7A1.5 1.5 0 002.5 11H3" stroke="currentColor" strokeWidth="1.5"/>
        </svg>
      )}
    </button>
  );
}

interface MenuCardProps {
  provider: ProviderUsageSnapshot;
  hideEmail: boolean;
  resetTimeRelative: boolean;
  showResetWhenExhausted?: boolean;
  showAsUsed?: boolean;
  compactMetrics?: boolean;
  isRefreshing?: boolean;
  onLayoutChange?: () => void;
}

function maskEmail(email: string): string {
  const at = email.indexOf("@");
  if (at <= 1) return "••••@••••";
  return email[0] + "•".repeat(at - 1) + email.slice(at);
}

/** Localize raw provider window labels using the active locale. */
function localizeWindowLabel(
  raw: string | undefined,
  t: (key: LocaleKey) => string,
): string {
  if (raw?.trim().toLowerCase() === "weekly") {
    return t("ProviderWeeklyLabel");
  }
  return raw ?? "";
}

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

function formatCurrency(amount: number, code: string): string {
  try {
    return new Intl.NumberFormat("en-US", {
      style: "currency",
      currency: code,
    }).format(amount);
  } catch {
    return `${code} ${amount.toFixed(2)}`;
  }
}

function formatCompactCount(value: number | null): string {
  if (value == null || value <= 0) return "—";
  return new Intl.NumberFormat("en-US", {
    notation: "compact",
    maximumFractionDigits: value >= 1_000_000 ? 1 : 0,
  }).format(value);
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
  const visibleHistory = costHistory
    .slice(-30)
    .filter((point) => point.value > 0);
  const maxCost = Math.max(...visibleHistory.map((point) => point.value), 0);

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
                height: `${Math.max(4, Math.round((point.value / maxCost) * 64))}px`,
              }}
              title={`${point.date}: ${formatCurrency(point.value, "USD")}`}
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

function displayPlanName(
  planName: string | null,
  t: (key: LocaleKey) => string,
): string | null {
  if (!planName) return null;
  const normalized = planName.trim().toLowerCase();
  if (normalized === "default_claude_ai") return t("ProviderPlanClaudeAi");
  return planName;
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

interface MetricEntry {
  id: string;
  label: string;
  snap: RateWindowSnapshot;
  resetFormatMode?: ResetTimeFormatMode;
  sessionEquivalentForecast?: SessionEquivalentForecastSnapshot | null;
}

type MetricPaceView =
  | { kind: "budget"; budget: NonNullable<ReturnType<typeof getPaceBudget>> }
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
  resetTimeRelative,
  showResetWhenExhausted,
  showAsUsed,
  expanded,
  onToggleExpanded,
  resetFormatMode,
  sessionEquivalentForecast,
}: {
  title: string;
  snap: RateWindowSnapshot;
  exhaustedLabel: string;
  resetTimeRelative: boolean;
  showResetWhenExhausted: boolean;
  showAsUsed: boolean;
  expanded: boolean;
  onToggleExpanded: () => void;
  resetFormatMode?: ResetTimeFormatMode;
  sessionEquivalentForecast?: SessionEquivalentForecastSnapshot | null;
}) {
  const { t } = useLocale();
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
  const paceView = getMetricPaceView(snap);
  const reserveDescription = formatReserveDescription(snap, t);
  const forecastText = formatSessionEquivalentEstimate(sessionEquivalentForecast);
  const formatBudget = (value: number) =>
    value < 10 ? value.toFixed(1).replace(/\.0$/, "") : Math.round(value).toString();
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
      {!isInformational && forecastText && (
        <div className="menu-metric__row menu-metric__forecast">
          <span className="menu-metric__pct">{forecastText}</span>
        </div>
      )}
    </div>
  );
}

/**
 * Provider card — direct mirror of SwiftUI `UsageMenuCardView`.
 *
 * Layout (top to bottom):
 *   1. Header VStack(spacing: 3)
 *        – HStack: providerName (headline/semibold)  ··  email (subheadline/secondary, right)
 *        – HStack: subtitle "source · updated"        ··  plan (footnote/secondary, right)
 *   2. Divider (1pt)
 *   3. VStack(spacing: 12)
 *        – Metrics group VStack(spacing: 12) of MetricRow
 *        – (Divider) Cost group: title (body/medium) + session line + month line (footnote)
 *        – (Divider) Pace group (Tauri-only addition; placed last)
 *        – (Divider) Charts group (Tauri-only addition; placed last)
 *
 * Padding: upstream v0.32.2 uses wider horizontal card padding and slightly
 * taller header/content vertical padding so account/plan rows can breathe.
 */
export default function MenuCard({
  provider,
  hideEmail,
  resetTimeRelative,
  showResetWhenExhausted = false,
  showAsUsed = false,
  compactMetrics = false,
  isRefreshing = false,
  onLayoutChange,
}: MenuCardProps) {
  const { t } = useLocale();
  const [chartData, setChartData] = useState<ProviderChartData | null>(null);
  const [expandedPaceWindow, setExpandedPaceWindow] = useState<string | null>(null);
  const formattedCostReset = useFormattedResetTime(
    provider.cost?.resetsAt ?? null,
    null,
    resetTimeRelative,
  );

  useEffect(() => {
    if (!providerSupportsChartData(provider.providerId)) {
      setChartData(null);
      return;
    }
    let cancelled = false;
    setChartData(null);
    getProviderChartData(
      provider.providerId,
      provider.accountEmail ?? undefined,
    )
      .then((data) => {
        if (!cancelled) {
          setChartData(data);
          requestAnimationFrame(() => onLayoutChange?.());
        }
      })
      .catch(() => {
        /* chart data is best-effort */
      });
    return () => {
      cancelled = true;
    };
  }, [provider.providerId, provider.accountEmail, onLayoutChange]);

  const isWayfinder = provider.providerId === "wayfinder";
  const email = !isWayfinder && provider.accountEmail
    ? hideEmail
      ? maskEmail(provider.accountEmail)
      : provider.accountEmail
    : null;
  const planName = !isWayfinder ? displayPlanName(provider.planName, t) : null;

  const metrics: MetricEntry[] = [
    ...(isWayfinder
      ? []
      : [
          {
            id: "primary",
            label: provider.primaryLabel ?? t("DetailWindowPrimary"),
            snap: provider.primary,
          },
        ]),
  ];
  if (provider.secondary)
    metrics.push({
      id: "secondary",
      label: localizeWindowLabel(provider.secondaryLabel, t) || t("DetailWindowSecondary"),
      snap: provider.secondary,
      sessionEquivalentForecast: provider.sessionEquivalentForecast,
    });
  if (provider.modelSpecific)
    metrics.push({
      id: "model-specific",
      label: t("DetailWindowModelSpecific"),
      snap: provider.modelSpecific,
    });
  if (provider.tertiary)
    metrics.push({
      id: "tertiary",
      label: t("DetailWindowTertiary"),
      snap: provider.tertiary,
    });
  for (const extra of provider.extraRateWindows ?? []) {
    metrics.push({
      id: `extra-${extra.id}`,
      label: extra.title,
      snap: extra.window,
      resetFormatMode: extra.id === "reset-credits" ? "expires" : "reset",
    });
  }
  const visibleMetrics = compactMetrics ? metrics.slice(0, 2) : metrics;

  const hasCostHistory =
    chartData !== null && chartData.costHistory.some((point) => point.value > 0);
  const hasCreditsHistory =
    chartData !== null && chartData.creditsHistory.length > 0;
  const hasUsageBreakdown =
    chartData !== null && chartData.usageBreakdown.length > 0;
  const hasCharts = hasCostHistory || hasCreditsHistory || hasUsageBreakdown;
  const localUsage = provider.error ? null : chartData?.localUsage ?? null;
  const localCostHistory = chartData?.costHistory ?? [];
  const hasMetrics = visibleMetrics.length > 0;
  const hasCost = !!provider.cost;
  const hasPace = !!provider.pace;
  const wayfinderUsage = isWayfinder ? provider.wayfinderUsage : null;
  const hasDetails =
    !provider.error &&
    (hasMetrics || hasCost || hasPace || hasCharts || !!localUsage || !!wayfinderUsage);
  const cardClassName = [
    "menu-card",
    provider.error ? "menu-card--error" : null,
    isRefreshing ? "menu-card--refreshing" : null,
    hasDetails ? "menu-card--with-details" : "menu-card--header-only",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <article className={cardClassName} aria-busy={isRefreshing}>
      <header className="menu-card__header">
        <div className="menu-card__title-row">
          <div className="menu-card__name-group">
            <span className="menu-card__name">{provider.displayName}</span>
            {!provider.error && email && <span className="menu-card__email">{email}</span>}
          </div>
        </div>
        {provider.error ? (
          <div className="menu-card__error-block">
            <div className="menu-card__error-text">{provider.error}</div>
            <CopyIconButton text={provider.error} />
          </div>
        ) : (
          <div className="menu-card__subtitle-row">
            <span className="menu-card__subtitle">
              {Number.isNaN(Date.parse(provider.updatedAt))
                ? provider.updatedAt
                : formatRelativeUpdated(Date.parse(provider.updatedAt), t)}
            </span>
            {planName && (
              <span className="menu-card__plan-badge">{planName}</span>
            )}
          </div>
        )}
      </header>

      {hasDetails && <div className="menu-card__divider" />}

      {hasDetails && (
        <div className="menu-card__content">
          {!provider.error && hasMetrics && (
            <section className="menu-card__group menu-card__metrics">
              {visibleMetrics.map((m) => (
                <MetricRow
                  key={m.id}
                  title={m.label}
                  snap={m.snap}
                  exhaustedLabel={t("DetailWindowExhausted")}
                  resetTimeRelative={resetTimeRelative}
                  showResetWhenExhausted={showResetWhenExhausted}
                  showAsUsed={showAsUsed}
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

          {provider.cost && (
            <section className="menu-card__group menu-card__cost">
              <div className="menu-card__group-title">
                {t("DetailCostTitle")} — {provider.cost.period}
              </div>
              <div className="menu-card__cost-line">
                {t("DetailCostUsed")}:{" "}
                {provider.cost.formattedUsed ||
                  formatCurrency(provider.cost.used, provider.cost.currencyCode)}
                {provider.cost.limit != null && (
                  <>
                    {" / "}
                    {provider.cost.formattedLimit ||
                      formatCurrency(provider.cost.limit, provider.cost.currencyCode)}
                  </>
                )}
              </div>
              {provider.cost.remaining != null && (
                <div className="menu-card__cost-line menu-card__cost-line--muted">
                  {t("DetailCostRemaining")}:{" "}
                  {formatCurrency(provider.cost.remaining, provider.cost.currencyCode)}
                </div>
              )}
              {formattedCostReset && (
                <div className="menu-card__cost-line menu-card__cost-line--muted">
                  {t("DetailCostResets")}: {formattedCostReset}
                </div>
              )}
            </section>
          )}

          {(hasMetrics || hasCost) && hasPace && <div className="menu-card__divider" />}

          {provider.pace && (
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
                  color="var(--accent)"
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
      )}
    </article>
  );
}
