import { BarChart } from "../../../../../components/charts/BarChart";
import { providerCostColor } from "../../../../../components/charts/chartPalette";
import type { LocaleKey } from "../../../../../i18n/keys";
import type { DailyTokenPoint } from "../../../../../types/bridge";

interface Props {
  data: DailyTokenPoint[];
  title: string;
  ariaLabel: string;
  providerId: string;
  animations: boolean;
  emptyMessage: string;
  /** Local history backfill has not reached the requested depth yet. */
  incomplete: boolean;
  t: (key: LocaleKey) => string;
}

/**
 * Tokens chart mode (upstream 0.50.0 #2930): exact local token totals per
 * day, defaulting Codex to this view. An incomplete backfill shows a
 * "Refreshing" marker instead of silently missing days.
 */
export function TokensHistoryChart({
  data,
  title,
  ariaLabel,
  providerId,
  animations,
  emptyMessage,
  incomplete,
  t,
}: Props) {
  const recent = data.slice(-30);
  const points = recent.map((p) => ({ label: p.date, value: p.tokens }));
  return (
    <div className="provider-detail-chart">
      <div className="provider-detail-chart__title">
        {title}
        {incomplete && (
          <span className="provider-detail-chart__refreshing">
            {t("DetailChartRefreshing")}
          </span>
        )}
      </div>
      <BarChart
        data={points}
        color={providerCostColor(providerId)}
        ariaLabel={ariaLabel}
        valueFormatter={(v) => Intl.NumberFormat().format(v)}
        animations={animations}
        emptyMessage={emptyMessage}
      />
    </div>
  );
}
