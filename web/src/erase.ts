import { INPAINT_MODEL } from "./model";

/** One brush mark. `x` and `y` are fractions of the image, `radius` of its long edge. */
export interface EraseDab {
  x: number;
  y: number;
  radius: number;
}

/** A rectangle of the proxy, in pixels. */
export interface EraseCrop {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** How much wider than the painted area the model sees, so it has context to draw on. */
const CONTEXT_FACTOR = 2;
/** Grows the painted area, in model pixels, to cover soft object edges. */
const HOLE_GROWTH = 4;
/** Part of that growth over which the patch fades out. */
const FEATHER = 3;

/** The region to send to the model: at least its input size, so small marks stay sharp. */
export function eraseCrop(dabs: EraseDab[], proxyWidth: number, proxyHeight: number): EraseCrop {
  const longEdge = Math.max(proxyWidth, proxyHeight);
  let [left, top, right, bottom] = [Infinity, Infinity, -Infinity, -Infinity];
  for (const dab of dabs) {
    const radius = dab.radius * longEdge;
    left = Math.min(left, dab.x * proxyWidth - radius);
    right = Math.max(right, dab.x * proxyWidth + radius);
    top = Math.min(top, dab.y * proxyHeight - radius);
    bottom = Math.max(bottom, dab.y * proxyHeight + radius);
  }
  const side = Math.max(INPAINT_MODEL.size, Math.ceil(Math.max(right - left, bottom - top) * CONTEXT_FACTOR));
  const width = Math.min(side, proxyWidth);
  const height = Math.min(side, proxyHeight);
  const place = (centre: number, span: number, limit: number) =>
    Math.min(limit - span, Math.max(0, Math.round(centre - span / 2)));
  return {
    x: place((left + right) / 2, width, proxyWidth),
    y: place((top + bottom) / 2, height, proxyHeight),
    width,
    height,
  };
}

/** Rasterises the marks over the crop: the model's binary hole, and the patch's alpha. */
export function rasterizeErase(dabs: EraseDab[], crop: EraseCrop, proxyWidth: number, proxyHeight: number) {
  const size = INPAINT_MODEL.size;
  const mask = new Float32Array(size * size);
  const alpha = new Uint8Array(size * size);
  const longEdge = Math.max(proxyWidth, proxyHeight);
  const [scaleX, scaleY] = [size / crop.width, size / crop.height];
  // Margins are set in model pixels and measured in proxy pixels.
  const growth = HOLE_GROWTH / Math.min(scaleX, scaleY);
  const feather = FEATHER / Math.min(scaleX, scaleY);

  for (const dab of dabs) {
    const reach = dab.radius * longEdge + growth;
    const [centreX, centreY] = [dab.x * proxyWidth - crop.x, dab.y * proxyHeight - crop.y];
    const x0 = Math.max(0, Math.floor((centreX - reach) * scaleX));
    const x1 = Math.min(size, Math.ceil((centreX + reach) * scaleX));
    const y0 = Math.max(0, Math.floor((centreY - reach) * scaleY));
    const y1 = Math.min(size, Math.ceil((centreY + reach) * scaleY));
    for (let y = y0; y < y1; y++) {
      for (let x = x0; x < x1; x++) {
        const distance = Math.hypot((x + 0.5) / scaleX - centreX, (y + 0.5) / scaleY - centreY);
        if (distance > reach) continue;
        const at = y * size + x;
        mask[at] = 1;
        const t = Math.min(1, (reach - distance) / feather);
        alpha[at] = Math.max(alpha[at], Math.round(255 * t * t * (3 - 2 * t)));
      }
    }
  }
  return { mask, alpha };
}
