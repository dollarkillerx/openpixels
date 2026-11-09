import type { Adjustments } from "./protocol";

/** Lens data found in a file. See `LensProfile` in op-core. */
export interface LensProfile {
  distortion: number[];
  vignette: number[];
  chromaRed: number[];
  chromaBlue: number[];
}

export interface LensView {
  aspect: [number, number];
  distortion: number[];
  scale: number;
  perspective: [number, number];
}

/** A liquify field as the engine has it: interleaved (du, dv). */
export interface WarpView {
  width: number;
  height: number;
  offsets: Float32Array;
}

const MANUAL_DISTORTION = 0.2;
const MANUAL_PERSPECTIVE = 0.3;
const BOUNDARY_STEPS = 64;
const INVERSE_ITERATIONS = 8;

const radial = (distortion: number[], r2: number) =>
  1 + r2 * (distortion[0] + r2 * (distortion[1] + r2 * (distortion[2] + r2 * distortion[3])));

function keystone(aspect: [number, number], perspective: [number, number], px: number, py: number): [number, number] {
  const depth = 1 - (perspective[0] * py) / aspect[1] - (perspective[1] * px) / aspect[0];
  return [px / depth, py / depth];
}

/** The geometric correction in effect. Mirrors `LensMap` in op-core. */
export function lensView(profile: LensProfile | null, edit: Adjustments, width: number, height: number): LensView {
  const distortion = edit.lensAuto && profile ? [...profile.distortion] : [0, 0, 0, 0];
  distortion[0] -= (edit.lensDistortion ?? 0) * MANUAL_DISTORTION;
  const diagonal = Math.hypot(width, height);
  const aspect: [number, number] = [width / diagonal, height / diagonal];

  const perspective: [number, number] = [
    (edit.perspectiveV ?? 0) * MANUAL_PERSPECTIVE,
    (edit.perspectiveH ?? 0) * MANUAL_PERSPECTIVE,
  ];

  let scale = 1;
  if (distortion.some((value) => value !== 0) || perspective.some((value) => value !== 0)) {
    let [furthest, nearest] = [0, Infinity];
    for (let step = 0; step <= BOUNDARY_STEPS; step++) {
      const position = (step / BOUNDARY_STEPS) * 2 - 1;
      for (const [x, y] of [[position, 1], [position, -1], [1, position], [-1, position]]) {
        const [px, py] = keystone(aspect, perspective, x * aspect[0], y * aspect[1]);
        const stretch = radial(distortion, px * px + py * py);
        const reach = Math.max(Math.abs(px / aspect[0]) * stretch, Math.abs(py / aspect[1]) * stretch);
        furthest = Math.max(furthest, reach);
        nearest = Math.min(nearest, reach);
      }
    }
    scale = 1 / (edit.perspectiveKeep ? nearest : furthest);
  }
  return { aspect, distortion, scale, perspective };
}

/** Where a corrected position (u, v) reads from in the unwarped image. */
export function lensToSource(view: LensView, u: number, v: number) {
  const [px, py] = keystone(view.aspect, view.perspective, (u - 0.5) * 2 * view.aspect[0], (v - 0.5) * 2 * view.aspect[1]);
  const stretch = radial(view.distortion, px * px + py * py) * view.scale;
  return { x: 0.5 + (px * stretch) / (2 * view.aspect[0]), y: 0.5 + (py * stretch) / (2 * view.aspect[1]) };
}

/** The corrected position showing source UV (x, y): the inverse of `lensToSource`. */
export function sourceToLens(view: LensView, x: number, y: number) {
  let [u, v] = [x, y];
  for (let i = 0; i < INVERSE_ITERATIONS; i++) {
    const mapped = lensToSource(view, u, v);
    u += x - mapped.x;
    v += y - mapped.y;
  }
  return { x: u, y: v };
}

/** Bilinear lookup. Mirrors `WarpField::offset_at` in op-core. */
function warpOffset({ width, height, offsets }: WarpView, u: number, v: number): [number, number] {
  const px = Math.min(width - 1, Math.max(0, u * width - 0.5));
  const py = Math.min(height - 1, Math.max(0, v * height - 0.5));
  const [x0, y0] = [Math.floor(px), Math.floor(py)];
  const [x1, y1] = [Math.min(x0 + 1, width - 1), Math.min(y0 + 1, height - 1)];
  const [tx, ty] = [px - x0, py - y0];
  const at = (x: number, y: number, channel: number) => offsets[(y * width + x) * 2 + channel];
  const mix = (channel: number) => {
    const top = at(x0, y0, channel) * (1 - tx) + at(x1, y0, channel) * tx;
    const bottom = at(x0, y1, channel) * (1 - tx) + at(x1, y1, channel) * tx;
    return top * (1 - ty) + bottom * ty;
  };
  return [mix(0), mix(1)];
}

/** Where a position on the framed image reads from, through liquify and lens correction. */
export function shownToSource(view: LensView | null, warp: WarpView | null, u: number, v: number) {
  const [du, dv] = warp ? warpOffset(warp, u, v) : [0, 0];
  return view ? lensToSource(view, u + du, v + dv) : { x: u + du, y: v + dv };
}

/** The position on the framed image that shows source (x, y): the inverse of `shownToSource`. */
export function sourceToShown(view: LensView | null, warp: WarpView | null, x: number, y: number) {
  const target = view ? sourceToLens(view, x, y) : { x, y };
  if (!warp) return target;
  let { x: u, y: v } = target;
  for (let i = 0; i < INVERSE_ITERATIONS; i++) {
    const [du, dv] = warpOffset(warp, u, v);
    u = target.x - du;
    v = target.y - dv;
  }
  return { x: u, y: v };
}
