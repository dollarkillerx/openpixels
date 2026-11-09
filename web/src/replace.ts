/** Images that can replace a photo's background or sky. */

export interface ReplaceImage {
  width: number;
  height: number;
  rgba: ArrayBuffer;
}

export const REPLACE_OFF = 0;
export const REPLACE_BACKGROUND = 1;
export const REPLACE_SKY = 2;

/** Imported images are stored at this size at most. */
const MAX_LONG_EDGE = 4096;
const WIDTH = 1600;
const HEIGHT = 1067;

type Painter = (context: OffscreenCanvasRenderingContext2D) => void;

const flat = (colour: string): Painter => (context) => {
  context.fillStyle = colour;
  context.fillRect(0, 0, WIDTH, HEIGHT);
};

const vertical = (stops: [number, string][]): Painter => (context) => {
  const gradient = context.createLinearGradient(0, 0, 0, HEIGHT);
  for (const [at, colour] of stops) gradient.addColorStop(at, colour);
  context.fillStyle = gradient;
  context.fillRect(0, 0, WIDTH, HEIGHT);
};

const studio: Painter = (context) => {
  const gradient = context.createRadialGradient(WIDTH / 2, HEIGHT * 0.45, 0, WIDTH / 2, HEIGHT * 0.45, WIDTH * 0.7);
  gradient.addColorStop(0, "#8f8f92");
  gradient.addColorStop(1, "#37373a");
  context.fillStyle = gradient;
  context.fillRect(0, 0, WIDTH, HEIGHT);
};

function valueNoise(seed: number) {
  const lattice = new Float32Array(256).map((_, index) => {
    const value = Math.sin((index + seed) * 127.1) * 43758.5453;
    return value - Math.floor(value);
  });
  const at = (x: number, y: number) => lattice[(x * 57 + y * 131) & 255];
  const smooth = (t: number) => t * t * (3 - 2 * t);
  return (x: number, y: number) => {
    const [ix, iy] = [Math.floor(x), Math.floor(y)];
    const [tx, ty] = [smooth(x - ix), smooth(y - iy)];
    const top = at(ix, iy) * (1 - tx) + at(ix + 1, iy) * tx;
    const bottom = at(ix, iy + 1) * (1 - tx) + at(ix + 1, iy + 1) * tx;
    return top * (1 - ty) + bottom * ty;
  };
}

/** Lays soft clouds of `colour` over what is already drawn. */
const clouds = (colour: [number, number, number], cover: number): Painter => (context) => {
  const noise = valueNoise(11);
  const image = context.getImageData(0, 0, WIDTH, HEIGHT);
  for (let y = 0; y < HEIGHT; y++) {
    for (let x = 0; x < WIDTH; x++) {
      let value = 0;
      let amplitude = 0.5;
      // Wider than tall, as clouds look when seen from below at a distance.
      for (let octave = 0, fx = x / 260, fy = y / 110; octave < 5; octave++, fx *= 2, fy *= 2, amplitude /= 2) {
        value += noise(fx, fy) * amplitude;
      }
      const t = Math.min(1, Math.max(0, (value - cover) / 0.22));
      const alpha = t * t * (3 - 2 * t) * 0.92;
      const at = (y * WIDTH + x) * 4;
      for (let channel = 0; channel < 3; channel++) {
        image.data[at + channel] += (colour[channel] - image.data[at + channel]) * alpha;
      }
    }
  }
  context.putImageData(image, 0, 0);
};

const both = (...painters: Painter[]): Painter => (context) => painters.forEach((paint) => paint(context));

interface BuiltIn {
  id: number;
  name: string;
  mode: number;
  paint: Painter;
}

/** Drawn here, so the project ships no third-party imagery. */
export const BUILT_IN_REPLACEMENTS: BuiltIn[] = [
  { id: 1, name: "纯白", mode: REPLACE_BACKGROUND, paint: flat("#f4f4f4") },
  { id: 2, name: "中灰", mode: REPLACE_BACKGROUND, paint: flat("#808082") },
  { id: 3, name: "纯黑", mode: REPLACE_BACKGROUND, paint: flat("#101012") },
  { id: 4, name: "影棚灰", mode: REPLACE_BACKGROUND, paint: studio },
  { id: 11, name: "晴空", mode: REPLACE_SKY, paint: both(vertical([[0, "#2f6fc0"], [0.75, "#8db8e4"], [1, "#d4e4f2"]]), clouds([250, 250, 252], 0.56)) },
  { id: 12, name: "晚霞", mode: REPLACE_SKY, paint: both(vertical([[0, "#2c3c74"], [0.5, "#c06a78"], [0.85, "#f4a65e"], [1, "#fbd59a"]]), clouds([255, 196, 150], 0.6)) },
  { id: 13, name: "阴天", mode: REPLACE_SKY, paint: both(vertical([[0, "#7f8791"], [1, "#c3c8ce"]]), clouds([214, 217, 222], 0.42)) },
];

const painted = new Map<number, ReplaceImage>();

function paintBuiltIn(item: BuiltIn): ReplaceImage {
  let image = painted.get(item.id);
  if (!image) {
    const canvas = new OffscreenCanvas(WIDTH, HEIGHT);
    const context = canvas.getContext("2d", { willReadFrequently: true })!;
    item.paint(context);
    image = { width: WIDTH, height: HEIGHT, rgba: context.getImageData(0, 0, WIDTH, HEIGHT).data.buffer as ArrayBuffer };
    painted.set(item.id, image);
  }
  return image;
}

export function builtInReplacement(id: number): ReplaceImage | null {
  const item = BUILT_IN_REPLACEMENTS.find((entry) => entry.id === id);
  // A copy, because sending the buffer to the engine detaches it.
  if (!item) return null;
  const image = paintBuiltIn(item);
  return { ...image, rgba: image.rgba.slice(0) };
}

export async function decodeReplacement(blob: Blob): Promise<ReplaceImage> {
  const bitmap = await createImageBitmap(blob);
  const scale = Math.min(1, MAX_LONG_EDGE / Math.max(bitmap.width, bitmap.height));
  const [width, height] = [Math.max(1, Math.round(bitmap.width * scale)), Math.max(1, Math.round(bitmap.height * scale))];
  const canvas = new OffscreenCanvas(width, height);
  const context = canvas.getContext("2d", { willReadFrequently: true })!;
  context.drawImage(bitmap, 0, 0, width, height);
  bitmap.close();
  return { width, height, rgba: context.getImageData(0, 0, width, height).data.buffer as ArrayBuffer };
}
