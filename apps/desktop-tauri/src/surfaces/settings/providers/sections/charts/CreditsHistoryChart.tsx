import { LineChart } from "../../../../../components/charts/LineChart";
import { providerCreditsColor } from "../../../../../components/charts/chartPalette";
import type { DailyCostPoint } from "../../../../../types/bridge";

interface Props {
  data: DailyCostPoint[];
  title: string;
  ariaLabel: string;
  providerId: string;
  animations: boolean;
  emptyMessage: string;
}

/**
 * Port target: the credits_history line/area in
 * `rust/src/native_ui/preferences.rs::render_provider_detail_panel`.
 */
export function CreditsHistoryChart({
  data,
  title,
  ariaLabel,
  providerId,
  animations,
  emptyMessage,
}: Props) {
  const recent = data.slice(-30);
  const points = recent.map((p) => ({ label: p.date, value: p.value }));
  return (
    <div className="provider-detail-chart">
      <div className="provider-detail-chart__title">{title}</div>
      <LineChart
        data={points}
        color={providerCreditsColor(providerId)}
        ariaLabel={ariaLabel}
        valueFormatter={(v) => v.toFixed(1)}
        animations={animations}
        emptyMessage={emptyMessage}
      />
    </div>
  );
}
