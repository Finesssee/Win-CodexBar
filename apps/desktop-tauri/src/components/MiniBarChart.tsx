/** MiniBarChart：不依赖外部库的轻量 SVG 柱状图。 */

import type { DailyCostPoint, DailyUsageBreakdown } from "../types/bridge";
import type { LocaleKey } from "../i18n/keys";

interface BarChartProps {
  points: DailyCostPoint[];
  color?: string;
  height?: number;
  label?: string;
  formatValue?: (v: number) => string;
  t: (key: LocaleKey) => string;
}

/** 展示每日成本或积分历史的简单柱状图。 */
export function SimpleBarChart({
  points,
  color = "#5d87ff",
  height = 48,
  label,
  formatValue,
  t,
}: BarChartProps) {
  const emptyMsg = t("DetailChartEmpty");
  if (points.length === 0) {
    return (
      <div className="mini-chart mini-chart--empty">
        {label && <span className="mini-chart__label">{label}</span>}
        <span className="mini-chart__empty-msg">{emptyMsg}</span>
      </div>
    );
  }

  const knownValues = points.flatMap((p) => (p.value == null ? [] : [p.value]));
  const max = Math.max(...knownValues, 0.0001);
  const BAR_GAP = 2;
  const fmt = formatValue ?? ((v: number) => v.toFixed(2));

  // 最多显示 30 根柱，并将日期标签缩短为末尾两位
  const visible = points.slice(-30);
  const svgWidth = 280;
  const barWidth = Math.max(
    1,
    Math.floor((svgWidth - (visible.length - 1) * BAR_GAP) / visible.length),
  );
  const actualWidth = visible.length * barWidth + (visible.length - 1) * BAR_GAP;

  return (
    <div className="mini-chart">
      {label && <span className="mini-chart__label">{label}</span>}
      <svg
        width={actualWidth}
        height={height}
        viewBox={`0 0 ${actualWidth} ${height}`}
        className="mini-chart__svg"
        aria-label={label ?? t("BarChartAriaLabel")}
      >
        {visible.map((p, i) => {
          const barH = p.value == null ? 1 : Math.max(1, (p.value / max) * (height - 4));
          const x = i * (barWidth + BAR_GAP);
          const y = height - barH;
          return (
            <rect
              key={p.date + i}
              x={x}
              y={y}
              width={barWidth}
              height={barH}
              fill={color}
              opacity={p.value == null ? 0 : p.value === 0 ? 0.25 : 0.9}
              rx={1}
            >
              <title>
                {p.date}: {p.value == null ? "Unknown" : fmt(p.value)}
              </title>
            </rect>
          );
        })}
      </svg>
      <div className="mini-chart__axis">
        {visible.length > 0 && (
          <>
            <span style={{ left: `${barWidth / 2}px` }}>{visible[0].date.slice(-5)}</span>
            <span style={{ left: `${actualWidth / 2}px` }}>{fmt(max)}</span>
            <span style={{ left: `${actualWidth - barWidth / 2}px` }}>{visible[visible.length - 1].date.slice(-5)}</span>
          </>
        )}
      </div>
    </div>
  );
}

// 按服务名称稳定分配颜色
const SERVICE_COLORS = [
  "#5d87ff",
  "#06d6a0",
  "#ffd166",
  "#ef476f",
  "#a78bfa",
  "#38bdf8",
  "#fb923c",
  "#4ade80",
];

function serviceColor(name: string, allServices: string[]): string {
  const idx = allServices.indexOf(name);
  return SERVICE_COLORS[idx % SERVICE_COLORS.length];
}

interface StackedBarChartProps {
  points: DailyUsageBreakdown[];
  height?: number;
  label?: string;
  t: (key: LocaleKey) => string;
}

/** 按服务展示每日用量明细的堆叠柱状图。 */
export function StackedBarChart({
  points,
  height = 64,
  label,
  t,
}: StackedBarChartProps) {
  const emptyMsg = t("DetailChartEmpty");
  if (points.length === 0) {
    return (
      <div className="mini-chart mini-chart--empty">
        {label && <span className="mini-chart__label">{label}</span>}
        <span className="mini-chart__empty-msg">{emptyMsg}</span>
      </div>
    );
  }

  const visible = points.slice(-30);
  const max = Math.max(...visible.map((p) => p.totalCreditsUsed), 0.0001);

  // Collect all unique service names for consistent coloring
  const allServices = Array.from(
    new Set(visible.flatMap((p) => p.services.map((s) => s.service))),
  ).sort();

  const BAR_GAP = 2;
  const svgWidth = 280;
  const barWidth = Math.max(
    1,
    Math.floor((svgWidth - (visible.length - 1) * BAR_GAP) / visible.length),
  );
  const actualWidth = visible.length * barWidth + (visible.length - 1) * BAR_GAP;

  return (
    <div className="mini-chart">
      {label && <span className="mini-chart__label">{label}</span>}
      <svg
        width={actualWidth}
        height={height}
        viewBox={`0 0 ${actualWidth} ${height}`}
        className="mini-chart__svg"
        aria-label={label ?? t("StackedBarChartAriaLabel")}
      >
        {visible.map((p, i) => {
          const x = i * (barWidth + BAR_GAP);
          const totalH = Math.max(1, (p.totalCreditsUsed / max) * (height - 4));
          // 固定服务排序，确保堆叠顺序可预测
          const sorted = [...p.services].sort((a, b) =>
            a.service.localeCompare(b.service),
          );
          let yOffset = height - totalH;
          return sorted.map((s) => {
            const segH = (s.creditsUsed / max) * (height - 4);
            const segY = yOffset;
            yOffset += segH;
            return (
              <rect
                key={`${p.day}-${s.service}`}
                x={x}
                y={segY}
                width={barWidth}
                height={Math.max(0.5, segH)}
                fill={serviceColor(s.service, allServices)}
                opacity={0.9}
                rx={1}
              >
                <title>
                  {p.day} {s.service}: {s.creditsUsed.toFixed(2)}{" "}
                  {t?.("CreditsLabel") ?? "credits"}
                </title>
              </rect>
            );
          });
        })}
      </svg>

      {/* Legend */}
      {allServices.length > 0 && (
        <div className="mini-chart__legend">
          {allServices.slice(0, 6).map((svc) => (
            <span key={svc} className="mini-chart__legend-item">
              <span
                className="mini-chart__legend-dot"
                style={{ background: serviceColor(svc, allServices) }}
              />
              {svc}
            </span>
          ))}
        </div>
      )}

      <div className="mini-chart__axis">
        {visible.length > 0 && (
          <>
            <span style={{ left: `${barWidth / 2}px` }}>{visible[0].day.slice(-5)}</span>
            <span style={{ left: `${actualWidth / 2}px` }}>{max.toFixed(1)}</span>
            <span style={{ left: `${actualWidth - barWidth / 2}px` }}>{visible[visible.length - 1].day.slice(-5)}</span>
          </>
        )}
      </div>
    </div>
  );
}
