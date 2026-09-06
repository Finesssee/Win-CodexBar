import { useRef, useState } from "react";
import { useChartAnimation } from "./useChartAnimation";

/**
 * LineChart — dependency-free SVG line chart with optional area fill,
 * entrance animation that sweeps the polyline up from the baseline,
 * and per-point hover tooltip.
 *
 * Port target: the credits-history line in
 * `rust/src/native_ui/charts.rs`.
 */

export interface LineChartPoint {
  label: string;
  value: number | null;
}

export interface LineChartProps {
  data: LineChartPoint[];
  color?: string;
  height?: number;
  valueFormatter?: (n: number) => string;
  ariaLabel: string;
  /** When true, render a faint filled area under the line. Defaults true. */
  area?: boolean;
  animations?: boolean;
  emptyMessage?: string;
}

const DEFAULT_COLOR = "var(--chart-credits)";
const SVG_WIDTH = 280;

export function LineChart({
  data,
  color = DEFAULT_COLOR,
  height = 56,
  valueFormatter,
  ariaLabel,
  area = true,
  animations = true,
  emptyMessage,
}: LineChartProps) {
  const fmt = valueFormatter ?? ((v: number) => v.toFixed(2));
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [hover, setHover] = useState<{ i: number; x: number; y: number } | null>(null);

  const anim = useChartAnimation(data.length, animations, [
    data.length,
    data[0]?.label,
    data[data.length - 1]?.label,
  ]);

  if (data.length === 0) {
    return (
      <div className="chart chart--line">
        <div className="chart__empty">{emptyMessage ?? ""}</div>
      </div>
    );
  }

  const values = data.flatMap((p) => (p.value == null ? [] : [p.value]));
  const hasKnownValues = values.length > 0;
  const max = hasKnownValues ? Math.max(...values, 0.0001) : 0.0001;
  const min = hasKnownValues ? Math.min(...values, 0) : 0;
  const range = Math.max(max - min, 0.0001);

  const plotHeight = Math.max(1, height - 4);
  const pad = 2;
  const usableWidth = SVG_WIDTH - pad * 2;

  // Baseline target Y (plot bottom) — the line animates from the
  // baseline up to its final Y, mirroring the bar entrance.
  const baselineY = pad + plotHeight;

  const step = data.length > 1 ? usableWidth / (data.length - 1) : 0;
  const coords = data.map((p, i) => {
    const x = pad + i * step;
    if (p.value == null) return null;
    const finalY = pad + plotHeight - ((p.value - min) / range) * plotHeight;
    const t = anim.barProgress(i);
    const y = baselineY + (finalY - baselineY) * t;
    return { x, y };
  });

  const segments: Array<Array<{ x: number; y: number }>> = [];
  let segment: Array<{ x: number; y: number }> = [];
  for (const coord of coords) {
    if (coord) {
      segment.push(coord);
    } else if (segment.length > 0) {
      segments.push(segment);
      segment = [];
    }
  }
  if (segment.length > 0) segments.push(segment);

  if (data.length === 1 && segments[0]?.length === 1) {
    const point = segments[0][0];
    segments[0].push({ x: pad + usableWidth, y: point.y });
  }

  const onPointMove = (e: React.MouseEvent<SVGCircleElement>, i: number) => {
    const host = containerRef.current;
    if (!host) return;
    const rect = host.getBoundingClientRect();
    setHover({ i, x: e.clientX - rect.left, y: e.clientY - rect.top });
  };
  const onLeave = () => setHover(null);
  const hoveredPoint = hover ? data[hover.i] : null;

  return (
    <div className="chart chart--line" ref={containerRef}>
      <svg
        width={SVG_WIDTH}
        height={height}
        viewBox={`0 0 ${SVG_WIDTH} ${height}`}
        className="chart__svg"
        role="img"
        aria-label={ariaLabel}
      >
        {area &&
          segments.map((points, i) => {
            if (points.length < 2) return null;
            const path = [
              `M ${points[0].x.toFixed(1)} ${baselineY.toFixed(1)}`,
              ...points.map((point) => `L ${point.x.toFixed(1)} ${point.y.toFixed(1)}`),
              `L ${points[points.length - 1].x.toFixed(1)} ${baselineY.toFixed(1)}`,
              "Z",
            ].join(" ");
            return (
              <path
                key={`area-${i}`}
                d={path}
                fill={color}
                opacity={0.18}
                className="chart__area"
              />
            );
          })}
        {segments.map((points, i) =>
          points.length < 2 ? null : (
            <polyline
              key={`line-${i}`}
              points={points.map((point) => `${point.x.toFixed(1)},${point.y.toFixed(1)}`).join(" ")}
              fill="none"
              stroke={color}
              strokeWidth={1.5}
              strokeLinejoin="round"
              strokeLinecap="round"
              opacity={0.95}
              className="chart__line"
            />
          ),
        )}
        {data.map((p, i) => {
          const coord = coords[i];
          if (p.value == null || !coord) return null;
          return (
            <circle
              key={`${p.label}-${i}`}
              cx={coord.x}
              cy={coord.y}
              r={hover?.i === i ? 3 : 1.8}
              fill={color}
              className="chart__point"
              onMouseMove={(e) => onPointMove(e, i)}
              onMouseLeave={onLeave}
            >
              <title>
                {p.label}: {fmt(p.value)}
              </title>
            </circle>
          );
        })}
      </svg>
      <div className="chart__axis">
        <span style={{ left: `${pad}px` }}>{data[0].label.slice(-5)}</span>
        <span className="chart__axis-max" style={{ left: `${SVG_WIDTH / 2}px` }}>
          {hasKnownValues ? fmt(max) : ""}
        </span>
        <span style={{ left: `${SVG_WIDTH - pad}px` }}>{data[data.length - 1].label.slice(-5)}</span>
      </div>
      {hover && hoveredPoint?.value != null && !anim.running && (
        <div
          className="chart__tooltip"
          style={{ left: hover.x, top: hover.y }}
          role="tooltip"
        >
          <span className="chart__tooltip-label">{hoveredPoint.label}</span>
          <strong>{fmt(hoveredPoint.value)}</strong>
        </div>
      )}
    </div>
  );
}
