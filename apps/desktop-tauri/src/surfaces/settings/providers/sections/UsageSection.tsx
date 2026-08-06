import type {
  ProviderDetail,
  RateWindowSnapshot,
} from "../../../../types/bridge";
import type { LocaleKey } from "../../../../i18n/keys";
import { useFormattedResetTime } from "../../../../hooks/useFormattedResetTime";

interface Props {
  provider: ProviderDetail;
  resetTimeRelative: boolean;
  t: (key: LocaleKey) => string;
}

interface BarSpec {
  key: string;
  label: string;
  rate: RateWindowSnapshot;
}

/**
 * Stacked usage bars — session / weekly / model-specific / tertiary.
 * Mirrors the bars in
 * `rust/src/native_ui/preferences.rs::render_provider_detail_panel`.
 */
export function UsageSection({ provider, resetTimeRelative, t }: Props) {
  const bars: BarSpec[] = [];
  if (provider.session) {
    bars.push({
      key: "session",
      label: t("ProviderSessionLabel"),
      rate: provider.session,
    });
  }
  if (provider.weekly) {
    bars.push({
      key: "weekly",
      label: t("ProviderWeeklyLabel"),
      rate: provider.weekly,
    });
  }
  if (provider.modelSpecific) {
    bars.push({
      key: "modelSpecific",
      label: t("DetailWindowModelSpecific"),
      rate: provider.modelSpecific,
    });
  }
  if (provider.tertiary) {
    bars.push({
      key: "tertiary",
      label: t("DetailWindowTertiary"),
      rate: provider.tertiary,
    });
  }
  for (const extra of provider.extraRateWindows ?? []) {
    bars.push({
      key: extra.id,
      label: extra.title,
      rate: extra.window,
    });
  }

  if (bars.length === 0) {
    return null;
  }

  return (
    <section className="provider-detail-section">
      <h4>{t("ProviderUsage")}</h4>
      {bars.map((b) => (
        <UsageBar
          key={b.key}
          label={b.label}
          rate={b.rate}
          resetTimeRelative={resetTimeRelative}
          t={t}
        />
      ))}
    </section>
  );
}

function UsageBar({
  label,
  rate,
  resetTimeRelative,
  t,
}: {
  label: string;
  rate: RateWindowSnapshot;
  resetTimeRelative: boolean;
  t: (key: LocaleKey) => string;
}) {
  const usedPct = Number.isFinite(rate.usedPercent) ? Math.max(0, rate.usedPercent) : 0;
  const pct = Math.min(100, usedPct);
  const isInformational = rate.isInformational === true;
  const formattedReset = useFormattedResetTime(
    rate.resetsAt,
    rate.resetDescription,
    resetTimeRelative,
  );
  const resetHint = formattedReset
    ? resetTimeRelative
      ? formattedReset
      : `${t("MetricResetsIn")} ${formattedReset}`
    : null;

  return (
    <div className="provider-usage-bar">
      <div className="provider-usage-bar__header">
        <span className="provider-usage-bar__label">{label}</span>
        <span
          className="provider-usage-bar__pct"
          data-exhausted={rate.isExhausted || undefined}
        >
          {isInformational
            ? rate.resetDescription?.trim() || formattedReset || "—"
            : rate.isExhausted
            ? usedPct > 100
              ? `${usedPct.toFixed(0)}%`
              : t("DetailWindowExhausted")
            : `${usedPct.toFixed(0)}%`}
        </span>
      </div>
      {!isInformational && (
        <div className="provider-usage-bar__track">
          <div
            className="provider-usage-bar__fill"
            style={{ width: `${pct}%` }}
            data-exhausted={rate.isExhausted || undefined}
          />
        </div>
      )}
      {!isInformational && resetHint && (
        <span className="provider-usage-bar__reset">{resetHint}</span>
      )}
    </div>
  );
}
