import type { Adjustments } from "./protocol";

export interface Crop {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Box {
  left: number;
  top: number;
  width: number;
  height: number;
}

export const FULL_FRAME: Crop = { x: 0, y: 0, w: 1, h: 1 };
const MIN_CROP = 0.05;

export const cropOf = (edit: Adjustments): Crop => ({
  x: edit.cropX ?? 0,
  y: edit.cropY ?? 0,
  w: edit.cropW ?? 1,
  h: edit.cropH ?? 1,
});

export const cropFields = ({ x, y, w, h }: Crop): Adjustments => ({ cropX: x, cropY: y, cropW: w, cropH: h });

/** Size of the image after its quarter turns. */
export function frameSize(width: number, height: number, quarter: number) {
  const sideways = ((quarter % 4) + 4) % 4 === 1 || ((quarter % 4) + 4) % 4 === 3;
  return sideways ? { width: height, height: width } : { width, height };
}

/** The largest box of the given aspect ratio centred in a stage. */
export function fitBox(aspect: number, stageWidth: number, stageHeight: number): Box {
  const width = Math.min(stageWidth, stageHeight * aspect);
  const height = width / aspect;
  return { left: (stageWidth - width) / 2, top: (stageHeight - height) / 2, width, height };
}

/** The largest centred crop with `ratio` (width / height, in pixels). */
export function cropForRatio(ratio: number, frameWidth: number, frameHeight: number): Crop {
  const frameAspect = frameWidth / frameHeight;
  const [w, h] = ratio > frameAspect ? [1, frameAspect / ratio] : [ratio / frameAspect, 1];
  return { x: (1 - w) / 2, y: (1 - h) / 2, w, h };
}

function insideRotatedImage(crop: Crop, degrees: number, width: number, height: number): boolean {
  const radians = (degrees * Math.PI) / 180;
  const [sin, cos] = [Math.sin(radians), Math.cos(radians)];
  for (const [s, t] of [[0, 0], [1, 0], [0, 1], [1, 1]]) {
    const x = (crop.x + s * crop.w - 0.5) * width;
    const y = (crop.y + t * crop.h - 0.5) * height;
    if (Math.abs(x * cos + y * sin) > width / 2 + 1e-3 || Math.abs(-x * sin + y * cos) > height / 2 + 1e-3) {
      return false;
    }
  }
  return true;
}

/** Shrinks a crop about its centre until it holds no area from outside the image. */
export function shrinkToImage(crop: Crop, degrees: number, frameWidth: number, frameHeight: number): Crop {
  let current = crop;
  // Moving towards the centre is what frees the corners when the crop is off-centre.
  for (let step = 0; step < 200 && !insideRotatedImage(current, degrees, frameWidth, frameHeight); step++) {
    const w = current.w * 0.99;
    const h = current.h * 0.99;
    const centerX = current.x + current.w / 2;
    const centerY = current.y + current.h / 2;
    const towardsMiddle = 0.98;
    const x = 0.5 + (centerX - 0.5) * towardsMiddle - w / 2;
    const y = 0.5 + (centerY - 0.5) * towardsMiddle - h / 2;
    current = { x, y, w, h };
  }
  return current;
}

/** Source UV shown at output position (s, t). Mirrors `Geometry::uv_map` in op-core. */
export function outputToSource(s: number, t: number, edit: Adjustments, width: number, height: number) {
  const quarter = edit.quarter ?? 0;
  const frame = frameSize(width, height, quarter);
  const crop = cropOf(edit);
  const radians = ((edit.rotate ?? 0) * Math.PI) / 180;
  const [sin, cos] = [Math.sin(radians), Math.cos(radians)];
  const x = (crop.x + s * crop.w - 0.5) * frame.width;
  const y = (crop.y + t * crop.h - 0.5) * frame.height;
  const [ux, uy] = [x * cos + y * sin, -x * sin + y * cos];
  const turns = ((quarter % 4) + 4) % 4;
  const [sx, sy] = turns === 1 ? [uy, -ux] : turns === 2 ? [-ux, -uy] : turns === 3 ? [-uy, ux] : [ux, uy];
  return { x: sx / width + 0.5, y: sy / height + 0.5 };
}

/** Output position (s, t) showing source UV (u, v): the inverse of `outputToSource`. */
export function sourceToOutput(u: number, v: number, edit: Adjustments, width: number, height: number) {
  const origin = outputToSource(0, 0, edit, width, height);
  const endS = outputToSource(1, 0, edit, width, height);
  const endT = outputToSource(0, 1, edit, width, height);
  const [ax, ay, bx, by] = [endS.x - origin.x, endS.y - origin.y, endT.x - origin.x, endT.y - origin.y];
  const determinant = ax * by - ay * bx;
  const [du, dv] = [u - origin.x, v - origin.y];
  return { x: (du * by - dv * bx) / determinant, y: (dv * ax - du * ay) / determinant };
}

export type CropHandle = "move" | "nw" | "ne" | "sw" | "se";

/** A crop after dragging `handle` by a fraction of the frame. `ratio` locks the shape. */
export function dragCrop(
  start: Crop,
  handle: CropHandle,
  dx: number,
  dy: number,
  ratio: number | null,
  frameWidth: number,
  frameHeight: number,
): Crop {
  if (handle === "move") {
    return {
      ...start,
      x: Math.min(Math.max(start.x + dx, 0), 1 - start.w),
      y: Math.min(Math.max(start.y + dy, 0), 1 - start.h),
    };
  }

  const fromLeft = handle === "nw" || handle === "sw";
  const fromTop = handle === "nw" || handle === "ne";
  const right = start.x + start.w;
  const bottom = start.y + start.h;

  let w = Math.max(MIN_CROP, fromLeft ? start.w - dx : start.w + dx);
  let h = Math.max(MIN_CROP, fromTop ? start.h - dy : start.h + dy);
  w = Math.min(w, fromLeft ? right : 1 - start.x);
  h = Math.min(h, fromTop ? bottom : 1 - start.y);

  if (ratio !== null) {
    const heightForWidth = (w * frameWidth) / ratio / frameHeight;
    if (heightForWidth <= h) {
      h = heightForWidth;
    } else {
      w = (h * frameHeight * ratio) / frameWidth;
    }
    const maxH = fromTop ? bottom : 1 - start.y;
    const maxW = fromLeft ? right : 1 - start.x;
    const fit = Math.min(1, maxW / w, maxH / h);
    w *= fit;
    h *= fit;
  }
  return { x: fromLeft ? right - w : start.x, y: fromTop ? bottom - h : start.y, w, h };
}
