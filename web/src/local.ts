import type { Adjustments } from "./protocol";

export const MAX_LOCAL_LAYERS = 4;
export const LOCAL_SLIDERS = [
  { suffix: "Exposure", label: "曝光", min: -3, max: 3 },
  { suffix: "Contrast", label: "对比度", min: -1, max: 1 },
  { suffix: "Saturation", label: "饱和度", min: -1, max: 1 },
  { suffix: "Temperature", label: "色温", min: -1, max: 1 },
];

export interface Point {
  x: number;
  y: number;
}

export type LocalKind = "linear" | "radial" | "brush";

/** A local adjustment's shape, in source-image UV. Its values live in the edit. */
export interface LocalLayer {
  kind: LocalKind;
  /** Gradient start, or the centre of a radial. */
  a: Point;
  /** Where a gradient ends, or a point on the radial's edge. */
  b: Point;
  /** Brush paths. */
  strokes: Point[][];
  /** Brush radius as a fraction of the long edge. */
  radius: number;
  invert: boolean;
}

const RADIAL_SOFT_START = 0.55;
const BRUSH_SPACING = 0.25;

function drawBrush(context: OffscreenCanvasRenderingContext2D, layer: LocalLayer, width: number, height: number) {
  const radius = layer.radius * Math.max(width, height);
  const dab = (x: number, y: number) => {
    const gradient = context.createRadialGradient(x, y, 0, x, y, radius);
    gradient.addColorStop(0, "rgba(255,255,255,0.6)");
    gradient.addColorStop(0.6, "rgba(255,255,255,0.4)");
    gradient.addColorStop(1, "rgba(255,255,255,0)");
    context.fillStyle = gradient;
    context.fillRect(x - radius, y - radius, radius * 2, radius * 2);
  };
  for (const stroke of layer.strokes) {
    stroke.forEach((point, index) => {
      const x = point.x * width;
      const y = point.y * height;
      if (index === 0) {
        dab(x, y);
        return;
      }
      const previous = stroke[index - 1];
      const [px, py] = [previous.x * width, previous.y * height];
      const steps = Math.max(1, Math.ceil(Math.hypot(x - px, y - py) / (radius * BRUSH_SPACING)));
      for (let step = 1; step <= steps; step++) dab(px + ((x - px) * step) / steps, py + ((y - py) * step) / steps);
    });
  }
}

function drawLayer(context: OffscreenCanvasRenderingContext2D, layer: LocalLayer, width: number, height: number) {
  context.fillStyle = "black";
  context.fillRect(0, 0, width, height);
  const [ax, ay, bx, by] = [layer.a.x * width, layer.a.y * height, layer.b.x * width, layer.b.y * height];
  if (layer.kind === "linear") {
    const gradient = context.createLinearGradient(ax, ay, bx, by);
    gradient.addColorStop(0, "white");
    gradient.addColorStop(1, "black");
    context.fillStyle = gradient;
    context.fillRect(0, 0, width, height);
  } else if (layer.kind === "radial") {
    const radius = Math.max(Math.hypot(bx - ax, by - ay), 1);
    const gradient = context.createRadialGradient(ax, ay, radius * RADIAL_SOFT_START, ax, ay, radius);
    gradient.addColorStop(0, "white");
    gradient.addColorStop(1, "black");
    context.fillStyle = gradient;
    context.fillRect(0, 0, width, height);
  } else {
    drawBrush(context, layer, width, height);
  }
}

/** Rasterises up to four layers into the channels of one RGBA buffer. */
export function renderLocalMask(layers: LocalLayer[], width: number, height: number): ArrayBuffer {
  const canvas = new OffscreenCanvas(width, height);
  const context = canvas.getContext("2d", { willReadFrequently: true })!;
  const out = new Uint8Array(width * height * 4);
  layers.slice(0, MAX_LOCAL_LAYERS).forEach((layer, channel) => {
    context.globalCompositeOperation = "source-over";
    drawLayer(context, layer, width, height);
    const { data } = context.getImageData(0, 0, width, height);
    for (let i = 0; i < width * height; i++) {
      out[i * 4 + channel] = layer.invert ? 255 - data[i * 4] : data[i * 4];
    }
  });
  return out.buffer;
}

/** The edit with layer `index` removed, and later layers' values moved down a slot. */
export function removeLocalValues(edit: Adjustments, index: number, count: number): Adjustments {
  const next = { ...edit };
  for (let layer = index; layer < count; layer++) {
    for (const { suffix } of LOCAL_SLIDERS) {
      const following = edit[`local${layer + 1}${suffix}`];
      if (layer + 1 < count && following !== undefined) next[`local${layer}${suffix}`] = following;
      else delete next[`local${layer}${suffix}`];
    }
  }
  return next;
}
