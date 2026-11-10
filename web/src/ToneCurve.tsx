import { useRef, useState } from "react";
import { parametricOffset } from "./library";
import type { Adjustments } from "./protocol";

const SIZE = 240;
const POINTS = [0, 1, 2, 3, 4];
const CHANNELS = [
  { key: "M", label: "RGB", color: "#e7e7ea" },
  { key: "R", label: "红", color: "#e0605a" },
  { key: "G", label: "绿", color: "#62b868" },
  { key: "B", label: "蓝", color: "#5a8fe0" },
];

interface Props {
  adjustments: Adjustments;
  disabled: boolean;
  onChange: (key: string, value: number) => void;
}

/** What the parametric sliders add at a point; they only act on the master curve. */
function parametric(adjustments: Adjustments, channel: string, point: number) {
  return channel === "M" ? parametricOffset(adjustments, point) : 0;
}

/** Control point positions as output values, for drawing. */
function outputs(adjustments: Adjustments, channel: string) {
  return POINTS.map((i) => i / 4 + (adjustments[`curve${channel}${i}`] ?? 0) + parametric(adjustments, channel, i));
}

/** Smooth path through the points. The exact curve is computed by the engine. */
function pathOf(values: number[]) {
  const y = (value: number) => SIZE - value * SIZE;
  let path = `M 0 ${y(values[0])}`;
  for (let i = 1; i < values.length; i++) {
    const x0 = ((i - 1) / 4) * SIZE;
    const x1 = (i / 4) * SIZE;
    const middle = (x0 + x1) / 2;
    path += ` C ${middle} ${y(values[i - 1])}, ${middle} ${y(values[i])}, ${x1} ${y(values[i])}`;
  }
  return path;
}

export function ToneCurve({ adjustments, disabled, onChange }: Props) {
  const [channel, setChannel] = useState("M");
  const svgRef = useRef<SVGSVGElement>(null);
  const dragRef = useRef<number | null>(null);
  const current = CHANNELS.find((item) => item.key === channel)!;
  const values = outputs(adjustments, channel);

  const moveTo = (event: React.PointerEvent) => {
    const index = dragRef.current;
    const svg = svgRef.current;
    if (index === null || !svg) return;
    const bounds = svg.getBoundingClientRect();
    const output = Math.min(Math.max(1 - (event.clientY - bounds.top) / bounds.height, 0), 1);
    onChange(`curve${channel}${index}`, output - index / 4 - parametric(adjustments, channel, index));
  };

  return (
    <div>
      <div className="row" role="group" aria-label="曲线通道">
        {CHANNELS.map((item) => (
          <button
            key={item.key}
            className={`button ${item.key === channel ? "" : "secondary"}`}
            onClick={() => setChannel(item.key)}
          >
            {item.label}
          </button>
        ))}
      </div>
      <svg
        ref={svgRef}
        className="curve"
        viewBox={`-8 -8 ${SIZE + 16} ${SIZE + 16}`}
        data-testid="tone-curve"
        onPointerMove={moveTo}
        onPointerUp={() => (dragRef.current = null)}
        onPointerCancel={() => (dragRef.current = null)}
      >
        <rect x={0} y={0} width={SIZE} height={SIZE} className="curve-frame" />
        {POINTS.slice(1, 4).map((i) => (
          <g key={i} className="curve-grid">
            <line x1={(i / 4) * SIZE} y1={0} x2={(i / 4) * SIZE} y2={SIZE} />
            <line x1={0} y1={(i / 4) * SIZE} x2={SIZE} y2={(i / 4) * SIZE} />
          </g>
        ))}
        <path d={pathOf(values)} fill="none" stroke={current.color} strokeWidth={2} />
        {POINTS.map((i) => (
          <circle
            key={i}
            data-point={i}
            cx={(i / 4) * SIZE}
            cy={SIZE - values[i] * SIZE}
            r={7}
            fill={current.color}
            style={{ cursor: disabled ? "default" : "ns-resize", touchAction: "none" }}
            onPointerDown={(event) => {
              if (disabled) return;
              event.currentTarget.setPointerCapture?.(event.pointerId);
              dragRef.current = i;
            }}
            onPointerMove={moveTo}
            onDoubleClick={() => !disabled && onChange(`curve${channel}${i}`, 0)}
          />
        ))}
      </svg>
    </div>
  );
}
