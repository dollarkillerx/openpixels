import type { Adjustments } from "./protocol";

export const MAX_LOCAL_LAYERS = 4;
export const LOCAL_SLIDERS = [
  { suffix: "Exposure", label: "曝光", min: -3, max: 3 },
  { suffix: "Contrast", label: "对比度", min: -1, max: 1 },
  { suffix: "Saturation", label: "饱和度", min: -1, max: 1 },
  { suffix: "Temperature", label: "色温", min: -1, max: 1 },
];

/** Per-layer tone and colour limits, stored beside the sliders in the edit. */
export const LOCAL_RANGE_SUFFIXES = ["RangeMode", "RangeLow", "RangeHigh", "RangeHue", "RangeWidth"];

export interface Point {
  x: number;
  y: number;
}

export type AiLocalKind = "subject" | "background" | "sky";
export type LocalKind = "linear" | "radial" | "brush" | "full" | AiLocalKind;

export const isAiLocal = (kind: LocalKind): kind is AiLocalKind => kind === "subject" || kind === "background" || kind === "sky";
/** Shapes with handles to drag on the image. */
export const hasLocalHandles = (kind: LocalKind) => kind === "linear" || kind === "radial";

/** A mask found by a model, at the size of the proxy. */
export interface ModelMask {
  mask: Uint8Array;
  width: number;
  height: number;
}
export type ModelMasks = Partial<Record<"subject" | "sky", ModelMask>>;
/** The model behind an AI layer; a background is the inverse of the subject. */
export const modelOf = (kind: AiLocalKind) => (kind === "sky" ? "sky" : "subject");

/** A local adjustment's shape, in source-image UV. Its values live in the edit. */
export interface LocalLayer {
  kind: LocalKind;
  /** Gradient start, or the centre of a radial. */
  a: Point;
  /** Where a gradient ends, or a point on the radial's edge. */
  b: Point;
  /** Brush paths; on an AI layer, what is added to the model's mask. */
  strokes: Point[][];
  /** Brush paths taken away from the mask. */
  erase?: Point[][];
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
  } else if (layer.kind === "full") {
    context.fillStyle = "white";
    context.fillRect(0, 0, width, height);
  } else {
    drawBrush(context, layer, width, height);
  }
}

/** A model's mask at another size, bilinearly. */
function resampled({ mask, width, height }: ModelMask, outWidth: number, outHeight: number) {
  if (width === outWidth && height === outHeight) return mask.slice();
  const out = new Uint8Array(outWidth * outHeight);
  for (let row = 0; row < outHeight; row++) {
    const fy = Math.min(height - 1, Math.max(0, ((row + 0.5) * height) / outHeight - 0.5));
    const [y0, ty] = [Math.floor(fy), fy - Math.floor(fy)];
    const y1 = Math.min(y0 + 1, height - 1);
    for (let col = 0; col < outWidth; col++) {
      const fx = Math.min(width - 1, Math.max(0, ((col + 0.5) * width) / outWidth - 0.5));
      const [x0, tx] = [Math.floor(fx), fx - Math.floor(fx)];
      const x1 = Math.min(x0 + 1, width - 1);
      const top = mask[y0 * width + x0] * (1 - tx) + mask[y0 * width + x1] * tx;
      const bottom = mask[y1 * width + x0] * (1 - tx) + mask[y1 * width + x1] * tx;
      out[row * outWidth + col] = Math.round(top * (1 - ty) + bottom * ty);
    }
  }
  return out;
}

/** A half-covered sky edge would let the old brightness through, so it is tightened. */
const tightenSky = (value: number) => {
  const t = Math.min(1, Math.max(0, (value / 255 - 0.3) / 0.4));
  return Math.round(255 * t * t * (3 - 2 * t));
};

/** Rasterises up to four layers into the channels of one RGBA buffer. */
export function renderLocalMask(layers: LocalLayer[], width: number, height: number, models: ModelMasks = {}): ArrayBuffer {
  const canvas = new OffscreenCanvas(width, height);
  const context = canvas.getContext("2d", { willReadFrequently: true })!;
  const out = new Uint8Array(width * height * 4);
  const painted = (layer: LocalLayer, strokes: Point[][]) => {
    context.fillStyle = "black";
    context.fillRect(0, 0, width, height);
    drawBrush(context, { ...layer, strokes }, width, height);
    return context.getImageData(0, 0, width, height).data;
  };
  layers.slice(0, MAX_LOCAL_LAYERS).forEach((layer, channel) => {
    context.globalCompositeOperation = "source-over";
    const values = new Uint8Array(width * height);
    if (isAiLocal(layer.kind)) {
      const model = models[modelOf(layer.kind)];
      if (model) values.set(resampled(model, width, height));
      if (layer.kind === "background") for (let i = 0; i < values.length; i++) values[i] = 255 - values[i];
      if (layer.kind === "sky") for (let i = 0; i < values.length; i++) values[i] = tightenSky(values[i]);
      if (layer.strokes.length > 0) {
        const added = painted(layer, layer.strokes);
        for (let i = 0; i < values.length; i++) values[i] = Math.max(values[i], added[i * 4]);
      }
    } else {
      drawLayer(context, layer, width, height);
      const { data } = context.getImageData(0, 0, width, height);
      for (let i = 0; i < values.length; i++) values[i] = data[i * 4];
    }
    if (layer.erase?.length) {
      const removed = painted(layer, layer.erase);
      for (let i = 0; i < values.length; i++) values[i] = Math.round((values[i] * (255 - removed[i * 4])) / 255);
    }
    for (let i = 0; i < values.length; i++) out[i * 4 + channel] = layer.invert ? 255 - values[i] : values[i];
  });
  return out.buffer;
}

/** The edit with layer `index` removed, and later layers' values moved down a slot. */
export function removeLocalValues(edit: Adjustments, index: number, count: number): Adjustments {
  const next = { ...edit };
  for (let layer = index; layer < count; layer++) {
    for (const suffix of [...LOCAL_SLIDERS.map((slider) => slider.suffix), ...LOCAL_RANGE_SUFFIXES]) {
      const following = edit[`local${layer + 1}${suffix}`];
      if (layer + 1 < count && following !== undefined) next[`local${layer}${suffix}`] = following;
      else delete next[`local${layer}${suffix}`];
    }
  }
  return next;
}
