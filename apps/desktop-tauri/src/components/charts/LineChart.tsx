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
  value: number;
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

  const values = data.map((p) => p.value);
  const max = Math.max(...values, 0.0001);
  const min = Math.min(...values, 0);
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
    const finalY = pad + plotHeight - ((p.value - min) / range) * plotHeight;
    const t = anim.barProgress(i);
    const y = baselineY + (finalY - baselineY) * t;
    return { x, y, finalY };
  });

  if (coords.length === 1) {
    coords.push({ x: pad + usableWidth, y: coords[0].y, finalY: coords[0].finalY });
  }

  const polyline = coords.map((c) => `${c.x.toFixed(1)},${c.y.toFixed(1)}`).join(" ");

  const areaPath = area
    ? [
        `M ${coords[0].x.toFixed(1)} ${baselineY.toFixed(1)}`,
        ...coords.map((c) => `L ${c.x.toFixed(1)} ${c.y.toFixed(1)}`),
        `L ${coords[coords.length - 1].x.toFixed(1)} ${baselineY.toFixed(1)}`,
        "Z",
      ].join(" ")
    : null;

  const onPointMove = (e: React.MouseEvent<SVGCircleElement>, i: number) => {
    const host = containerRef.current;
    if (!host) return;
    const rect = host.getBoundingClientRect();
    setHover({ i, x: e.clientX - rect.left, y: e.clientY - rect.top });
  };
  const onLeave = () => setHover(null);

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
        {areaPath && (
          <path d={areaPath} fill={color} opacity={0.18} className="chart__area" />
        )}
        <polyline
          points={polyline}
          fill="none"
          stroke={color}
          strokeWidth={1.5}
          strokeLinejoin="round"
          strokeLinecap="round"
          opacity={0.95}
          className="chart__line"
        />
        {data.map((p, i) => (
          <circle
            key={`${p.label}-${i}`}
            cx={coords[i].x}
            cy={coords[i].y}
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
        ))}
      </svg>
      <div className="chart__axis">
        <span style={{ left: `${pad}px` }}>{data[0].label.slice(-5)}</span>
        <span className="chart__axis-max" style={{ left: `${SVG_WIDTH / 2}px` }}>{fmt(max)}</span>
        <span style={{ left: `${SVG_WIDTH - pad}px` }}>{data[data.length - 1].label.slice(-5)}</span>
      </div>
      {hover && !anim.running && (
        <div
          className="chart__tooltip"
          style={{ left: hover.x, top: hover.y }}
          role="tooltip"
        >
          <span className="chart__tooltip-label">{data[hover.i].label}</span>
          <strong>{fmt(data[hover.i].value)}</strong>
        </div>
      )}
    </div>
  );
}
