import type { Adjustments } from "./protocol";

export interface CurvePoint {
  x: number;
  y: number;
}

export const CURVE_CHANNELS = ["M", "R", "G", "B"];
export const IDENTITY_POINTS: CurvePoint[] = [{ x: 0, y: 0 }, { x: 1, y: 1 }];

/** Edits saved before free points stored offsets at these five fixed inputs. */
const LEGACY_POINT_COUNT = 5;
const CHANNEL_KEY = /^curve[MRGB](\d|N|[XY]\d+)$/;
/** Keep in step with `op-core/src/curve.rs`. */
const PARAMETRIC_REACH = 0.1;
const PARAMETRIC_REGIONS = [
  { key: "curveHighlights", centre: 0.8, width: 0.2 },
  { key: "curveLights", centre: 0.6, width: 0.25 },
  { key: "curveDarks", centre: 0.4, width: 0.25 },
  { key: "curveShadows", centre: 0.2, width: 0.2 },
];

/** A channel's control points, in order of input. */
export function curvePoints(adjustments: Adjustments, channel: string): CurvePoint[] {
  const count = adjustments[`curve${channel}N`];
  if (count) {
    return Array.from({ length: count }, (_, i) => ({ x: adjustments[`curve${channel}X${i}`] ?? 0, y: adjustments[`curve${channel}Y${i}`] ?? 0 }));
  }
  const legacy = Array.from({ length: LEGACY_POINT_COUNT }, (_, i) => adjustments[`curve${channel}${i}`] ?? 0);
  if (legacy.every((offset) => offset === 0)) return IDENTITY_POINTS;
  return legacy.map((offset, i) => ({ x: i / (LEGACY_POINT_COUNT - 1), y: i / (LEGACY_POINT_COUNT - 1) + offset }));
}

function isIdentity(points: CurvePoint[]) {
  return points.length === 2 && points[0].x === 0 && points[0].y === 0 && points[1].x === 1 && points[1].y === 1;
}

/** The edit with a channel's points replaced. An untouched curve stores nothing. */
export function withCurvePoints(adjustments: Adjustments, channel: string, points: CurvePoint[]): Adjustments {
  const next: Adjustments = {};
  for (const [key, value] of Object.entries(adjustments)) {
    if (!CHANNEL_KEY.test(key) || key[5] !== channel) next[key] = value;
  }
  if (isIdentity(points)) return next;
  next[`curve${channel}N`] = points.length;
  points.forEach((point, i) => {
    next[`curve${channel}X${i}`] = point.x;
    next[`curve${channel}Y${i}`] = point.y;
  });
  return next;
}

/** What the engine takes for its parametric regions. */
export const parametricAmounts = (adjustments: Adjustments) => PARAMETRIC_REGIONS.map(({ key }) => adjustments[key] ?? 0);

/** How far the parametric sliders move the master curve at an input. */
export function parametricOffset(adjustments: Adjustments, x: number): number {
  let offset = 0;
  for (const { key, centre, width } of PARAMETRIC_REGIONS) {
    const distance = Math.abs(x - centre) / width;
    if (distance < 1) offset += (adjustments[key] ?? 0) * PARAMETRIC_REACH * (0.5 + 0.5 * Math.cos(Math.PI * distance));
  }
  return offset;
}

function tangentsOf(points: CurvePoint[]): number[] {
  const spans = points.length - 1;
  const widths = points.slice(1).map((point, i) => point.x - points[i].x);
  const slopes = points.slice(1).map((point, i) => (point.y - points[i].y) / widths[i]);
  const tangents = points.map(() => slopes[0]);
  tangents[spans] = slopes[spans - 1];
  for (let i = 1; i < spans; i++) {
    tangents[i] = slopes[i - 1] * slopes[i] <= 0 ? 0 : (widths[i] * slopes[i - 1] + widths[i - 1] * slopes[i]) / (widths[i - 1] + widths[i]);
  }
  if (spans > 1) {
    const end = (slope: number, inner: number) => (slope * (3 * slope - inner) <= 0 ? 0 : (3 * slope - inner) / 2);
    tangents[0] = end(slopes[0], tangents[1]);
    tangents[spans] = end(slopes[spans - 1], tangents[spans - 1]);
  }
  for (let i = 0; i < spans; i++) {
    if (slopes[i] === 0) {
      tangents[i] = tangents[i + 1] = 0;
      continue;
    }
    const [a, b] = [tangents[i] / slopes[i], tangents[i + 1] / slopes[i]];
    const size = a * a + b * b;
    if (size > 9) {
      const scale = 3 / Math.sqrt(size);
      tangents[i] = scale * a * slopes[i];
      tangents[i + 1] = scale * b * slopes[i];
    }
  }
  return tangents;
}

/** The same monotone spline the engine applies, so what is drawn is what happens. */
export function splineThrough(points: CurvePoint[]): (x: number) => number {
  const tangents = tangentsOf(points);
  const last = points.length - 1;
  return (x) => {
    if (x <= points[0].x) return points[0].y;
    if (x >= points[last].x) return points[last].y;
    let segment = 0;
    while (points[segment + 1].x <= x) segment++;
    const [start, end] = [points[segment], points[segment + 1]];
    const width = end.x - start.x;
    const t = (x - start.x) / width;
    const [t2, t3] = [t * t, t * t * t];
    return (2 * t3 - 3 * t2 + 1) * start.y + (t3 - 2 * t2 + t) * width * tangents[segment] + (-2 * t3 + 3 * t2) * end.y + (t3 - t2) * width * tangents[segment + 1];
  };
}
