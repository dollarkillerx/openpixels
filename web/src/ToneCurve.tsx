import { useRef, useState } from "react";
import { IDENTITY_POINTS, curvePoints, parametricOffset, splineThrough, withCurvePoints, type CurvePoint } from "./curve";
import type { Adjustments, HistogramData } from "./protocol";

const SIZE = 240;
const MARGIN = 8;
const GRID = [0.25, 0.5, 0.75];
/** How close, in drawing units, a press must be to grab a point. */
const GRAB_RADIUS = 14;
/** A new point this close to the curve lands on it and leaves the shape alone. */
const SNAP_DISTANCE = 12;
const MIN_SPACING = 0.01;
const MAX_POINTS = 16;
/** Dragging a point this far outside the graph removes it. */
const REMOVE_DISTANCE = 32;
const PATH_STEPS = 120;
/** An end point dropped this near its corner goes back to it exactly. */
const CORNER_SNAP = 0.02;
const CHANNELS = [
  { key: "M", label: "RGB", color: "#e7e7ea" },
  { key: "R", label: "红", color: "#e0605a" },
  { key: "G", label: "绿", color: "#62b868" },
  { key: "B", label: "蓝", color: "#5a8fe0" },
];

interface Props {
  adjustments: Adjustments;
  histogram: HistogramData | null;
  disabled: boolean;
  onChange: (next: Adjustments) => void;
}

const clamp = (value: number, low: number, high: number) => Math.min(Math.max(value, low), high);

/** The histogram of a curve channel as a filled area, drawn under the curve. */
function histogramPath(histogram: HistogramData, channel: string) {
  const index = CHANNELS.findIndex((item) => item.key === channel) - 1;
  const counts = Array.from({ length: 256 }, (_, bin) =>
    index >= 0 ? histogram.channels[index][bin] : (histogram.channels[0][bin] + histogram.channels[1][bin] + histogram.channels[2][bin]) / 3,
  );
  // Square root, and the clipped ends left out of the scale, as in the main histogram.
  const tallest = Math.sqrt(Math.max(1, ...counts.slice(1, 255)));
  let path = `M 0 ${SIZE}`;
  counts.forEach((count, bin) => {
    path += ` L ${((bin / 255) * SIZE).toFixed(1)} ${(SIZE - Math.min(1, Math.sqrt(count) / tallest) * SIZE).toFixed(1)}`;
  });
  return `${path} L ${SIZE} ${SIZE} Z`;
}

export function ToneCurve({ adjustments, histogram, disabled, onChange }: Props) {
  const [channel, setChannel] = useState("M");
  const svgRef = useRef<SVGSVGElement>(null);
  const dragRef = useRef<number | null>(null);
  // Pointer events can arrive faster than React renders.
  const latestRef = useRef(adjustments);
  latestRef.current = adjustments;

  const current = CHANNELS.find((item) => item.key === channel)!;
  const added = (edit: Adjustments, x: number) => (channel === "M" ? parametricOffset(edit, x) : 0);
  const points = curvePoints(adjustments, channel);
  const spline = splineThrough(points);
  const untouched = points.length === 2 && points === IDENTITY_POINTS;

  const shownY = (edit: Adjustments, x: number, y: number) => clamp(y + added(edit, x), 0, 1);
  const pathData = Array.from({ length: PATH_STEPS + 1 }, (_, i) => {
    const x = i / PATH_STEPS;
    return `${i === 0 ? "M" : "L"} ${(x * SIZE).toFixed(1)} ${((1 - shownY(adjustments, x, spline(x))) * SIZE).toFixed(1)}`;
  }).join(" ");

  const write = (next: CurvePoint[]) => {
    const updated = withCurvePoints(latestRef.current, channel, next);
    latestRef.current = updated;
    onChange(updated);
  };

  const pointerAt = (event: React.MouseEvent) => {
    const bounds = svgRef.current!.getBoundingClientRect();
    const span = SIZE + 2 * MARGIN;
    return { px: ((event.clientX - bounds.left) / bounds.width) * span - MARGIN, py: ((event.clientY - bounds.top) / bounds.height) * span - MARGIN };
  };

  const nearestPoint = (edit: Adjustments, list: CurvePoint[], px: number, py: number) => {
    let best = -1;
    let bestDistance = GRAB_RADIUS;
    list.forEach((point, index) => {
      const distance = Math.hypot(point.x * SIZE - px, (1 - shownY(edit, point.x, point.y)) * SIZE - py);
      if (distance <= bestDistance) {
        best = index;
        bestDistance = distance;
      }
    });
    return best;
  };

  const press = (event: React.PointerEvent<SVGSVGElement>) => {
    if (disabled || event.button !== 0) return;
    const edit = latestRef.current;
    const list = curvePoints(edit, channel);
    const { px, py } = pointerAt(event);
    const grabbed = nearestPoint(edit, list, px, py);
    event.currentTarget.setPointerCapture?.(event.pointerId);
    if (grabbed >= 0) {
      dragRef.current = grabbed;
      return;
    }
    const x = clamp(px / SIZE, 0, 1);
    const after = list.findIndex((point) => point.x > x);
    if (list.length >= MAX_POINTS || after <= 0 || x - list[after - 1].x < MIN_SPACING || list[after].x - x < MIN_SPACING) return;
    const onCurve = splineThrough(list)(x);
    const pressedY = 1 - py / SIZE - added(edit, x);
    const nearCurve = Math.abs(pressedY - onCurve) * SIZE <= SNAP_DISTANCE;
    write([...list.slice(0, after), { x, y: nearCurve ? onCurve : clamp(pressedY, 0, 1) }, ...list.slice(after)]);
    dragRef.current = after;
  };

  const move = (event: React.PointerEvent<SVGSVGElement>) => {
    const index = dragRef.current;
    if (index === null || disabled) return;
    const edit = latestRef.current;
    const list = curvePoints(edit, channel);
    const { px, py } = pointerAt(event);
    const last = list.length - 1;
    const outside = px < -REMOVE_DISTANCE || px > SIZE + REMOVE_DISTANCE || py < -REMOVE_DISTANCE || py > SIZE + REMOVE_DISTANCE;
    if (outside && index > 0 && index < last) {
      dragRef.current = null;
      write(list.filter((_, position) => position !== index));
      return;
    }
    let x = clamp(px / SIZE, index > 0 ? list[index - 1].x + MIN_SPACING : 0, index < last ? list[index + 1].x - MIN_SPACING : 1);
    let y = clamp(1 - py / SIZE - added(edit, x), 0, 1);
    if (index === 0 || index === last) {
      const corner = index === 0 ? 0 : 1;
      if (Math.hypot(x - corner, y - corner) < CORNER_SNAP) [x, y] = [corner, corner];
    }
    write(list.map((point, position) => (position === index ? { x, y } : point)));
  };

  const release = () => {
    dragRef.current = null;
  };

  const removeAt = (event: React.MouseEvent) => {
    if (disabled) return;
    const edit = latestRef.current;
    const list = curvePoints(edit, channel);
    const { px, py } = pointerAt(event);
    const index = nearestPoint(edit, list, px, py);
    if (index < 0) return;
    if (index > 0 && index < list.length - 1) write(list.filter((_, position) => position !== index));
    else write(list.map((point, position) => (position === index ? (index === 0 ? { x: 0, y: 0 } : { x: 1, y: 1 }) : point)));
  };

  return (
    <div>
      <div className="row" role="group" aria-label="曲线通道">
        {CHANNELS.map((item) => (
          <button key={item.key} className={`button ${item.key === channel ? "" : "secondary"}`} onClick={() => setChannel(item.key)}>
            {item.label}
          </button>
        ))}
        <button className="button ghost" disabled={disabled || untouched} onClick={() => write(IDENTITY_POINTS)}>重置曲线</button>
      </div>
      <svg
        ref={svgRef}
        className="curve"
        viewBox={`${-MARGIN} ${-MARGIN} ${SIZE + 2 * MARGIN} ${SIZE + 2 * MARGIN}`}
        data-testid="tone-curve"
        style={{ cursor: disabled ? "default" : "crosshair", touchAction: "none" }}
        onPointerDown={press}
        onPointerMove={move}
        onPointerUp={release}
        onPointerCancel={release}
        onDoubleClick={removeAt}
      >
        <rect x={0} y={0} width={SIZE} height={SIZE} className="curve-frame" />
        {histogram && <path d={histogramPath(histogram, channel)} fill={current.color} className="curve-histogram" data-testid="curve-histogram" />}
        {GRID.map((at) => (
          <g key={at} className="curve-grid">
            <line x1={at * SIZE} y1={0} x2={at * SIZE} y2={SIZE} />
            <line x1={0} y1={at * SIZE} x2={SIZE} y2={at * SIZE} />
          </g>
        ))}
        <line x1={0} y1={SIZE} x2={SIZE} y2={0} className="curve-diagonal" />
        <path d={pathData} fill="none" stroke={current.color} strokeWidth={2} strokeLinejoin="round" />
        {points.map((point, index) => (
          <circle key={index} data-point={index} cx={point.x * SIZE} cy={(1 - shownY(adjustments, point.x, point.y)) * SIZE} r={5} fill={current.color} style={{ cursor: disabled ? "default" : "grab" }} />
        ))}
      </svg>
      <p className="progress curve-hint">点击曲线加点，拖动调整。双击或拖出框外可删除。</p>
    </div>
  );
}
