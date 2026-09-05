import { useMemo, useRef, useState } from "react";
import { useChartAnimation } from "./useChartAnimation";

/**
 * BarChart — dependency-free SVG bar chart with entrance animation,
 * hover tooltip, and a yellow peak cap. Mirrors the visuals from
 * `rust/src/native_ui/charts.rs` (ChartBar::draw).
 *
 * Phase 10 additions:
 *   - entrance animation with staggered ease-out (respects
 *     `animations` prop and `prefers-reduced-motion`)
 *   - absolute-positioned hover tooltip
 *   - peak cap rendered as a separate rect filled with `--chart-peak`
 */

export interface BarChartPoint {
  label: string;
  value: number | null;
}

export interface BarChartProps {
  data: BarChartPoint[];
  color?: string;
  height?: number;
  valueFormatter?: (n: number) => string;
  ariaLabel: string;
  /** When false, bars render at their final size immediately. */
  animations?: boolean;
  /** Optional empty-state message rendered when `data.length === 0`. */
  emptyMessage?: string;
}

const DEFAULT_COLOR = "var(--chart-cost)";
const BAR_GAP = 2;
const SVG_WIDTH = 280;
const CAP_HEIGHT = 5;

export function BarChart({
  data,
  color = DEFAULT_COLOR,
  height = 56,
  valueFormatter,
  ariaLabel,
  animations = true,
  emptyMessage,
}: BarChartProps) {
  const fmt = valueFormatter ?? ((v: number) => v.toFixed(2));
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [hover, setHover] = useState<{ i: number; x: number; y: number } | null>(null);

  const anim = useChartAnimation(data.length, animations, [
    data.length,
    data[0]?.label,
    data[data.length - 1]?.label,
  ]);

  const { max, peakIndex } = useMemo(() => {
    let m = 0.0001;
    let p = -1;
    for (let i = 0; i < data.length; i++) {
      const v = data[i].value;
      if (v != null && v > m) {
        m = v;
        p = i;
      }
    }
    return { max: m, peakIndex: p };
  }, [data]);

  if (data.length === 0) {
    return (
      <div className="chart chart--bar">
        <div className="chart__empty">{emptyMessage ?? ""}</div>
      </div>
    );
  }

  const barWidth = Math.max(
    1,
    Math.floor((SVG_WIDTH - (data.length - 1) * BAR_GAP) / data.length),
  );
  const actualWidth = data.length * barWidth + (data.length - 1) * BAR_GAP;
  const plotHeight = Math.max(1, height - 4);

  const onMove = (e: React.MouseEvent<SVGRectElement>, i: number) => {
    const host = containerRef.current;
    if (!host) return;
    const hostRect = host.getBoundingClientRect();
    setHover({ i, x: e.clientX - hostRect.left, y: e.clientY - hostRect.top });
  };
  const onLeave = () => setHover(null);

  return (
    <div className="chart chart--bar" ref={containerRef}>
      <svg
        width={actualWidth}
        height={height}
        viewBox={`0 0 ${actualWidth} ${height}`}
        className="chart__svg"
        role="img"
        aria-label={ariaLabel}
      >
        {data.map((p, i) => {
          const base = p.value == null ? 1 : p.value === 0 ? 1 : Math.max(3, (p.value / max) * plotHeight);
          const eased = anim.barProgress(i);
          const barH = base * eased;
          const x = i * (barWidth + BAR_GAP);
          const y = height - barH;
          const isPeak = i === peakIndex && barH > CAP_HEIGHT;
          const bodyH = isPeak ? Math.max(0, barH - CAP_HEIGHT) : barH;
          const bodyY = isPeak ? y + CAP_HEIGHT : y;
          const isHovered = hover?.i === i;

          return (
            <g key={`${p.label}-${i}`}>
              <rect
                x={x}
                y={bodyY}
                width={barWidth}
                height={bodyH}
                fill={color}
                opacity={p.value == null ? 0 : p.value === 0 ? 0.25 : isHovered ? 1 : 0.9}
                rx={1}
                className="chart__bar"
                onMouseMove={p.value == null ? undefined : (e) => onMove(e, i)}
                onMouseLeave={onLeave}
              >
                <title>
                  {p.value == null ? p.label : `${p.label}: ${fmt(p.value)}`}
                </title>
              </rect>
              {isPeak && (
                <rect
                  x={x}
                  y={y}
                  width={barWidth}
                  height={CAP_HEIGHT}
                  fill="var(--chart-peak)"
                  rx={1}
                  className="chart__peak-cap"
                  pointerEvents="none"
                />
              )}
            </g>
          );
        })}
      </svg>
      <div className="chart__axis">
        <span style={{ left: `${barWidth / 2}px` }}>{data[0].label.slice(-5)}</span>
        <span className="chart__axis-max" style={{ left: `${actualWidth / 2}px` }}>{fmt(max)}</span>
        <span style={{ left: `${actualWidth - barWidth / 2}px` }}>{data[data.length - 1].label.slice(-5)}</span>
      </div>
      {hover && !anim.running && (
        <div
          className="chart__tooltip"
          style={{ left: hover.x, top: hover.y }}
          role="tooltip"
        >
          <span className="chart__tooltip-label">{data[hover.i].label}</span>
          <strong>{fmt(data[hover.i].value ?? 0)}</strong>
        </div>
      )}
    </div>
  );
}
