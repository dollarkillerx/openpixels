/** Scores photos for culling and groups the ones taken of the same moment. */

export interface FaceFinding {
  /** In pixels of the analysed image. */
  box: { left: number; top: number; right: number; bottom: number };
  /** 0 is wide open, 1 is shut. The more open eye counts. */
  blink: number;
}

export interface PhotoAnalysis {
  /** Milliseconds, from the capture time when `timeKnown`, otherwise the file's date. */
  time: number;
  timeKnown: boolean;
  /** Laplacian variance over the faces, or the middle of the frame if there are none. */
  sharpness: number;
  /** Fraction of pixels blown out or crushed. */
  clipped: number;
  faces: number;
  eyesClosed: boolean;
  /** A coarse picture of the photo, for telling whether two show the same scene. */
  signature: number[];
}

const SIGNATURE_GRID = 8;
const EYES_CLOSED_BLINK = 0.55;
/** Middle of the frame to judge sharpness on when no face is found. */
const CENTRE_MARGIN = 0.2;

/** Photos further apart than this are different moments, however alike they look. */
const MAX_GROUP_GAP_MS = 2 * 60 * 1000;
/**
 * Mean difference between signatures, in 0..1, below which two photos show one scene.
 * Measured: shifted or blurred copies differ by under 0.01, and different portraits by
 * 0.065 or more. Without capture times to back it up, the looks alone must be closer.
 */
const SAME_SCENE_TIMED = 0.05;
const SAME_SCENE_UNTIMED = 0.03;

/** A softness this many times worse than the group's best counts as out of focus. */
const SOFT_VARIANCE_RATIO = 3;
const BADLY_EXPOSED_EXTRA = 0.15;

const luma = (rgb: Uint8Array, at: number) => 0.2126 * rgb[at] + 0.7152 * rgb[at + 1] + 0.0722 * rgb[at + 2];

function laplacianVariance(rgb: Uint8Array, width: number, left: number, top: number, right: number, bottom: number) {
  let [sum, squares, count] = [0, 0, 0];
  for (let y = Math.max(1, top); y < bottom - 1; y++) {
    for (let x = Math.max(1, left); x < right - 1; x++) {
      const at = (y * width + x) * 3;
      const value = luma(rgb, at - 3) + luma(rgb, at + 3) + luma(rgb, at - width * 3) + luma(rgb, at + width * 3) - 4 * luma(rgb, at);
      sum += value;
      squares += value * value;
      count++;
    }
  }
  return count > 0 ? squares / count - (sum / count) ** 2 : 0;
}

export function analyzeImage(rgb: Uint8Array, width: number, height: number, faces: FaceFinding[], time: number, timeKnown: boolean): PhotoAnalysis {
  let clippedPixels = 0;
  const cells = new Float32Array(SIGNATURE_GRID * SIGNATURE_GRID * 3);
  const counts = new Uint32Array(SIGNATURE_GRID * SIGNATURE_GRID);
  for (let y = 0; y < height; y++) {
    const row = Math.min(SIGNATURE_GRID - 1, Math.floor((y / height) * SIGNATURE_GRID));
    for (let x = 0; x < width; x++) {
      const at = (y * width + x) * 3;
      const [r, g, b] = [rgb[at], rgb[at + 1], rgb[at + 2]];
      if (Math.max(r, g, b) >= 254 || Math.max(r, g, b) <= 2) clippedPixels++;
      const cell = row * SIGNATURE_GRID + Math.min(SIGNATURE_GRID - 1, Math.floor((x / width) * SIGNATURE_GRID));
      // Brightness and two colour differences: small shifts in framing change these little.
      cells[cell * 3] += (0.3 * r + 0.6 * g + 0.1 * b) / 255;
      cells[cell * 3 + 1] += (r - g) / 510 + 0.5;
      cells[cell * 3 + 2] += (b - g) / 510 + 0.5;
      counts[cell]++;
    }
  }
  const signature = Array.from(cells, (value, index) => Math.round((value / Math.max(1, counts[Math.floor(index / 3)])) * 1000) / 1000);

  let sharpness: number;
  if (faces.length > 0) {
    // The largest face is the subject: its focus is what matters.
    const area = (face: FaceFinding) => (face.box.right - face.box.left) * (face.box.bottom - face.box.top);
    const main = faces.reduce((best, face) => (area(face) > area(best) ? face : best));
    sharpness = laplacianVariance(rgb, width, Math.round(main.box.left), Math.round(main.box.top), Math.round(main.box.right), Math.round(main.box.bottom));
  } else {
    sharpness = laplacianVariance(rgb, width, Math.round(width * CENTRE_MARGIN), Math.round(height * CENTRE_MARGIN), Math.round(width * (1 - CENTRE_MARGIN)), Math.round(height * (1 - CENTRE_MARGIN)));
  }

  return {
    time,
    timeKnown,
    sharpness,
    clipped: clippedPixels / (width * height),
    faces: faces.length,
    eyesClosed: faces.some((face) => face.blink >= EYES_CLOSED_BLINK),
    signature,
  };
}

export function signatureDistance(a: number[], b: number[]) {
  let total = 0;
  for (let i = 0; i < a.length; i++) total += Math.abs(a[i] - b[i]);
  return total / a.length;
}

export interface Analyzed {
  id: string;
  analysis: PhotoAnalysis;
}

/** Splits photos, in shooting order, wherever the moment or the scene changes. */
export function groupPhotos(items: Analyzed[]): string[][] {
  const ordered = [...items].sort((a, b) => a.analysis.time - b.analysis.time || a.id.localeCompare(b.id));
  const groups: Analyzed[][] = [];
  for (const item of ordered) {
    const group = groups[groups.length - 1];
    const previous = group?.[group.length - 1];
    const timed = previous?.analysis.timeKnown && item.analysis.timeKnown;
    const tooLate = previous && timed && item.analysis.time - previous.analysis.time > MAX_GROUP_GAP_MS;
    if (previous && !tooLate && signatureDistance(previous.analysis.signature, item.analysis.signature) <= (timed ? SAME_SCENE_TIMED : SAME_SCENE_UNTIMED)) {
      group.push(item);
    } else {
      groups.push([item]);
    }
  }
  return groups.map((group) => group.map((item) => item.id));
}

/** A photo's score in 0..100, relative to the sharpest one it is compared with. */
export function scorePhoto(analysis: PhotoAnalysis, bestSharpness: number) {
  // On a log scale, so "half as sharp" costs the same at any level of detail.
  const relative = bestSharpness > 0 ? Math.max(0, 1 + Math.log(Math.max(analysis.sharpness, 1e-6) / bestSharpness) / Math.log(8)) : 1;
  const exposure = Math.max(0, 1 - analysis.clipped * 4);
  const eyes = analysis.eyesClosed ? 0 : 1;
  return Math.round(100 * (0.55 * relative + 0.2 * exposure + 0.25 * eyes));
}

export interface Verdict {
  score: number;
  best: boolean;
  reject: boolean;
}

/** Ranks one group. A photo on its own is its group's best, and never rejected. */
export function judgeGroup(group: Analyzed[]): Map<string, Verdict> {
  const bestSharpness = Math.max(...group.map((item) => item.analysis.sharpness));
  const scored = group.map((item) => ({ item, score: scorePhoto(item.analysis, bestSharpness) }));
  const winner = scored.reduce((best, entry) => (entry.score > best.score ? entry : best));
  const verdicts = new Map<string, Verdict>();
  for (const { item, score } of scored) {
    const best = item === winner.item;
    const { analysis } = item;
    const reject =
      !best &&
      ((analysis.eyesClosed && !winner.item.analysis.eyesClosed) ||
        analysis.sharpness * SOFT_VARIANCE_RATIO < bestSharpness ||
        analysis.clipped > winner.item.analysis.clipped + BADLY_EXPOSED_EXTRA);
    verdicts.set(item.id, { score, best, reject });
  }
  return verdicts;
}
