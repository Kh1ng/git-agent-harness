import { useState, useRef } from 'react';

export interface TrendPoint {
  date: string;
  value: number;
}

const WIDTH = 640;
const HEIGHT = 200;
const PAD_LEFT = 40;
const PAD_BOTTOM = 24;
const PAD_TOP = 12;
const PAD_RIGHT = 12;

/**
 * One reusable SVG line chart -- single hue, thin 2px line, rounded data
 * end, hairline gridlines, hover crosshair + tooltip. No charting
 * dependency: the repo has none, and this is the "one high-quality
 * reusable visualization" the spec asks for rather than several weak ones.
 */
export function TrendChart({
  data,
  valueLabel,
  formatValue = (v) => String(v)
}: {
  data: TrendPoint[];
  valueLabel: string;
  formatValue?: (v: number) => string;
}) {
  const [hoverIndex, setHoverIndex] = useState<number | null>(null);
  const svgRef = useRef<SVGSVGElement>(null);

  if (data.length === 0) {
    return <div className="h-[200px] flex items-center justify-center text-sm text-muted">No data</div>;
  }

  const maxValue = Math.max(...data.map((d) => d.value), 1);
  const plotWidth = WIDTH - PAD_LEFT - PAD_RIGHT;
  const plotHeight = HEIGHT - PAD_TOP - PAD_BOTTOM;

  const xFor = (i: number) => PAD_LEFT + (data.length === 1 ? plotWidth / 2 : (i / (data.length - 1)) * plotWidth);
  const yFor = (v: number) => PAD_TOP + plotHeight - (v / maxValue) * plotHeight;

  const linePath = data.map((d, i) => `${i === 0 ? 'M' : 'L'}${xFor(i)},${yFor(d.value)}`).join(' ');
  const areaPath = `${linePath} L${xFor(data.length - 1)},${PAD_TOP + plotHeight} L${xFor(0)},${PAD_TOP + plotHeight} Z`;

  const gridLines = [0, 0.5, 1].map((f) => PAD_TOP + plotHeight * (1 - f));

  const handleMove = (e: React.MouseEvent<SVGSVGElement>) => {
    const svg = svgRef.current;
    if (!svg) return;
    const rect = svg.getBoundingClientRect();
    const relativeX = ((e.clientX - rect.left) / rect.width) * WIDTH;
    let nearest = 0;
    let nearestDist = Infinity;
    data.forEach((_, i) => {
      const dist = Math.abs(xFor(i) - relativeX);
      if (dist < nearestDist) {
        nearestDist = dist;
        nearest = i;
      }
    });
    setHoverIndex(nearest);
  };

  const hovered = hoverIndex !== null ? data[hoverIndex] : null;

  return (
    <div className="relative">
      <svg
        ref={svgRef}
        viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
        className="w-full h-auto"
        onMouseMove={handleMove}
        onMouseLeave={() => setHoverIndex(null)}
        role="img"
        aria-label={`${valueLabel} trend over ${data.length} points`}
      >
        {gridLines.map((y, i) => (
          <line key={i} x1={PAD_LEFT} x2={WIDTH - PAD_RIGHT} y1={y} y2={y} stroke="rgb(var(--border-subtle))" strokeWidth={1} />
        ))}

        <text x={PAD_LEFT - 8} y={PAD_TOP + 4} textAnchor="end" fontSize={10} fill="rgb(var(--ink-muted))">
          {formatValue(maxValue)}
        </text>
        <text x={PAD_LEFT - 8} y={PAD_TOP + plotHeight + 4} textAnchor="end" fontSize={10} fill="rgb(var(--ink-muted))">
          0
        </text>

        <path d={areaPath} fill="rgb(var(--accent) / 0.12)" stroke="none" />
        <path d={linePath} fill="none" stroke="rgb(var(--accent))" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" />

        {/* rounded data-end on the last point */}
        <circle cx={xFor(data.length - 1)} cy={yFor(data[data.length - 1].value)} r={3} fill="rgb(var(--accent))" />

        {hoverIndex !== null && (
          <>
            <line
              x1={xFor(hoverIndex)}
              x2={xFor(hoverIndex)}
              y1={PAD_TOP}
              y2={PAD_TOP + plotHeight}
              stroke="rgb(var(--ink-muted))"
              strokeWidth={1}
              strokeDasharray="2,2"
            />
            <circle cx={xFor(hoverIndex)} cy={yFor(data[hoverIndex].value)} r={4} fill="rgb(var(--surface-card))" stroke="rgb(var(--accent))" strokeWidth={2} />
          </>
        )}

        {data.map((d, i) => (
          <text
            key={d.date}
            x={xFor(i)}
            y={HEIGHT - 6}
            textAnchor="middle"
            fontSize={9}
            fill="rgb(var(--ink-muted))"
            style={{ display: data.length > 10 && i % Math.ceil(data.length / 8) !== 0 ? 'none' : undefined }}
          >
            {d.date.slice(5)}
          </text>
        ))}
      </svg>

      {hovered && (
        <div
          className="absolute pointer-events-none card px-2 py-1 text-xs shadow-lg"
          style={{
            left: `${(xFor(hoverIndex!) / WIDTH) * 100}%`,
            top: 0,
            transform: 'translate(-50%, -110%)'
          }}
        >
          <div className="text-muted">{hovered.date}</div>
          <div className="text-primary font-medium">
            {valueLabel}: {formatValue(hovered.value)}
          </div>
        </div>
      )}
    </div>
  );
}
