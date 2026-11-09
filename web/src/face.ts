import { FaceLandmarker, FilesetResolver, type Category, type FaceLandmarkerResult, type NormalizedLandmark } from "@mediapipe/tasks-vision";
import type { FaceFinding } from "./analysis";
import type { Thumbnail } from "./protocol";

const MODEL_URL = "/models/face_landmarker.task";
const MAX_FACES = 6;

interface Point {
  x: number;
  y: number;
}

export interface SkinMask {
  mask: ArrayBuffer;
  faces: number;
  faceWidth: number;
}

/** Window used to look for faces too small for the whole-image pass. */
const TILE_SIZE = 512;
const TILE_STRIDE = 256;
const TILE_CONFIDENCE = 0.8;
const DEFAULT_CONFIDENCE = 0.5;
/** Canvas sizes for retrying an extreme close-up, relative to its long edge. */
const CLOSE_UP_PADDINGS = [2, 3.5];
/** Assumed face size when none is found, as a fraction of the short edge. */
const CLOSE_UP_FACE_FRACTION = 0.7;

/** Facial features to keep out of the skin mask, and how much to grow each outline. */
const FEATURES = [
  { connections: FaceLandmarker.FACE_LANDMARKS_LEFT_EYE, grow: 1.4 },
  { connections: FaceLandmarker.FACE_LANDMARKS_RIGHT_EYE, grow: 1.4 },
  { connections: FaceLandmarker.FACE_LANDMARKS_LEFT_EYEBROW, grow: 1.25 },
  { connections: FaceLandmarker.FACE_LANDMARKS_RIGHT_EYEBROW, grow: 1.25 },
  { connections: FaceLandmarker.FACE_LANDMARKS_LIPS, grow: 1.15 },
];

/** Mesh vertices at the nostrils, which are dark spots that are not blemishes. */
const NOSTRILS = [79, 309];
const NOSTRIL_RADIUS = 0.05;

let landmarker: Promise<FaceLandmarker> | null = null;

function getLandmarker(): Promise<FaceLandmarker> {
  landmarker ??= (async () => {
    // Absolute for the same reason as the ONNX Runtime path in ai-worker.ts.
    const vision = await FilesetResolver.forVisionTasks(`${location.origin}/mediapipe`);
    return FaceLandmarker.createFromOptions(vision, {
      baseOptions: { modelAssetPath: MODEL_URL, delegate: "CPU" },
      runningMode: "IMAGE",
      numFaces: MAX_FACES,
      outputFaceBlendshapes: true,
    });
  })();
  return landmarker;
}

function convexHull(points: Point[]): Point[] {
  const sorted = [...points].sort((a, b) => a.x - b.x || a.y - b.y);
  const cross = (o: Point, a: Point, b: Point) => (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
  const half = (input: Point[]) => {
    const chain: Point[] = [];
    for (const point of input) {
      while (chain.length >= 2 && cross(chain[chain.length - 2], chain[chain.length - 1], point) <= 0) chain.pop();
      chain.push(point);
    }
    chain.pop();
    return chain;
  };
  return [...half(sorted), ...half([...sorted].reverse())];
}

function grown(polygon: Point[], factor: number): Point[] {
  const center = {
    x: polygon.reduce((sum, p) => sum + p.x, 0) / polygon.length,
    y: polygon.reduce((sum, p) => sum + p.y, 0) / polygon.length,
  };
  return polygon.map((p) => ({
    x: center.x + (p.x - center.x) * factor,
    y: center.y + (p.y - center.y) * factor,
  }));
}

function fill(context: OffscreenCanvasRenderingContext2D, polygon: Point[]) {
  context.beginPath();
  polygon.forEach((p, index) => (index === 0 ? context.moveTo(p.x, p.y) : context.lineTo(p.x, p.y)));
  context.closePath();
  context.fill();
}

function drawFace(
  context: OffscreenCanvasRenderingContext2D,
  landmarks: NormalizedLandmark[],
  width: number,
  height: number,
): number {
  const point = (index: number): Point => ({ x: landmarks[index].x * width, y: landmarks[index].y * height });
  const hullOf = (connections: { start: number; end: number }[]) => {
    const indices = new Set(connections.flatMap(({ start, end }) => [start, end]));
    return convexHull([...indices].map(point));
  };

  const face = hullOf(FaceLandmarker.FACE_LANDMARKS_FACE_OVAL);
  context.fillStyle = "white";
  fill(context, face);

  context.fillStyle = "black";
  for (const { connections, grow } of FEATURES) fill(context, grown(hullOf(connections), grow));

  const xs = face.map((p) => p.x);
  const faceWidth = Math.max(...xs) - Math.min(...xs);
  for (const index of NOSTRILS) {
    const { x, y } = point(index);
    context.beginPath();
    context.arc(x, y, faceWidth * NOSTRIL_RADIUS, 0, Math.PI * 2);
    context.fill();
  }
  return faceWidth;
}

function toImageData({ rgb, width, height }: Thumbnail): ImageData {
  const pixels = new Uint8ClampedArray(width * height * 4);
  const source = new Uint8Array(rgb);
  for (let i = 0, j = 0; i < source.length; i += 3, j += 4) {
    pixels[j] = source[i];
    pixels[j + 1] = source[i + 1];
    pixels[j + 2] = source[i + 2];
    pixels[j + 3] = 255;
  }
  return new ImageData(pixels, width, height);
}

function centerOf(landmarks: NormalizedLandmark[]): Point {
  const total = landmarks.reduce((sum, p) => ({ x: sum.x + p.x, y: sum.y + p.y }), { x: 0, y: 0 });
  return { x: total.x / landmarks.length, y: total.y / landmarks.length };
}

/**
 * The detector shrinks its input to a small square, so a face in a full-length or
 * group shot is too few pixels to see. Searching overlapping windows keeps them large.
 */
interface Detected {
  landmarks: NormalizedLandmark[];
  shapes: Category[];
}

function collect(result: FaceLandmarkerResult): Detected[] {
  return result.faceLandmarks.map((landmarks, index) => ({ landmarks, shapes: result.faceBlendshapes[index]?.categories ?? [] }));
}

/** A face filling the frame is too big for the detector, so give it room around it. */
function detectPadded(detector: FaceLandmarker, image: ImageData, padding: number): Detected[] {
  const size = Math.round(Math.max(image.width, image.height) * padding);
  const [left, top] = [Math.round((size - image.width) / 2), Math.round((size - image.height) / 2)];
  const source = new OffscreenCanvas(image.width, image.height);
  source.getContext("2d")!.putImageData(image, 0, 0);
  const canvas = new OffscreenCanvas(size, size);
  const context = canvas.getContext("2d", { willReadFrequently: true })!;
  context.fillStyle = "#808080";
  context.fillRect(0, 0, size, size);
  context.drawImage(source, left, top);
  return collect(detector.detect(context.getImageData(0, 0, size, size))).map(({ landmarks, shapes }) => ({
    landmarks: landmarks.map((point) => ({ ...point, x: (point.x * size - left) / image.width, y: (point.y * size - top) / image.height })),
    shapes,
  }));
}

/** Faces of any size: as they are, then extreme close-ups, then ones too small to see whole. */
async function detectFaces(detector: FaceLandmarker, thumbnail: Thumbnail, detailed: () => Promise<Thumbnail>): Promise<Detected[]> {
  const image = toImageData(thumbnail);
  const whole = collect(detector.detect(image));
  if (whole.length > 0) return whole;
  for (const padding of CLOSE_UP_PADDINGS) {
    const padded = detectPadded(detector, image, padding);
    if (padded.length > 0) return padded;
  }
  return detectInTiles(detector, toImageData(await detailed()));
}

async function detectInTiles(detector: FaceLandmarker, image: ImageData): Promise<Detected[]> {
  // Many windows mean many chances of a false match, so demand more certainty.
  await detector.setOptions({ minFaceDetectionConfidence: TILE_CONFIDENCE });
  const source = new OffscreenCanvas(image.width, image.height);
  source.getContext("2d")!.putImageData(image, 0, 0);
  const tile = new OffscreenCanvas(TILE_SIZE, TILE_SIZE);
  const context = tile.getContext("2d", { willReadFrequently: true })!;

  const steps = (size: number) => {
    const last = Math.max(0, size - TILE_SIZE);
    const positions: number[] = [];
    for (let at = 0; at < last; at += TILE_STRIDE) positions.push(at);
    positions.push(last);
    return positions;
  };

  const found: Detected[] = [];
  for (const top of steps(image.height)) {
    for (const left of steps(image.width)) {
      context.clearRect(0, 0, TILE_SIZE, TILE_SIZE);
      context.drawImage(source, -left, -top);
      for (const { landmarks, shapes } of collect(detector.detect(context.getImageData(0, 0, TILE_SIZE, TILE_SIZE)))) {
        const mapped = landmarks.map((p) => ({
          ...p,
          x: (left + p.x * TILE_SIZE) / image.width,
          y: (top + p.y * TILE_SIZE) / image.height,
        }));
        const center = centerOf(mapped);
        // Overlapping windows see the same face more than once.
        const duplicate = found.some((other) => {
          const c = centerOf(other.landmarks);
          return Math.hypot((c.x - center.x) * image.width, (c.y - center.y) * image.height) < TILE_STRIDE / 2;
        });
        if (!duplicate) found.push({ landmarks: mapped, shapes });
      }
    }
  }
  await detector.setOptions({ minFaceDetectionConfidence: DEFAULT_CONFIDENCE });
  return found;
}

/**
 * Finds faces and rasterises their skin at the given proxy size. `detailed` supplies a
 * larger image for faces the first pass misses. With no face at all, as in a profile
 * or a tight close-up, the whole frame is passed on and the retouch's own colour
 * checks decide what is skin; `faces` is then 0.
 */
/** Where the faces are in a thumbnail, and how open their eyes are. */
export async function locateFaces(thumbnail: Thumbnail, detailed: () => Promise<Thumbnail>): Promise<FaceFinding[]> {
  const detector = await getLandmarker();
  return (await detectFaces(detector, thumbnail, detailed)).map(({ landmarks, shapes }) => {
    const xs = landmarks.map((point) => point.x * thumbnail.width);
    const ys = landmarks.map((point) => point.y * thumbnail.height);
    const shape = (name: string) => shapes.find((item) => item.categoryName === name)?.score ?? 0;
    return {
      box: { left: Math.max(0, Math.min(...xs)), top: Math.max(0, Math.min(...ys)), right: Math.min(thumbnail.width, Math.max(...xs)), bottom: Math.min(thumbnail.height, Math.max(...ys)) },
      blink: Math.min(shape("eyeBlinkLeft"), shape("eyeBlinkRight")),
    };
  });
}

export async function buildSkinMask(
  thumbnail: Thumbnail,
  detailed: () => Promise<Thumbnail>,
  proxyWidth: number,
  proxyHeight: number,
): Promise<SkinMask> {
  const detector = await getLandmarker();
  const faceLandmarks = (await detectFaces(detector, thumbnail, detailed)).map((face) => face.landmarks);
  if (faceLandmarks.length === 0) {
    const everything = new Uint8Array(proxyWidth * proxyHeight).fill(255);
    return {
      mask: everything.buffer,
      faces: 0,
      faceWidth: Math.min(proxyWidth, proxyHeight) * CLOSE_UP_FACE_FRACTION,
    };
  }

  const canvas = new OffscreenCanvas(proxyWidth, proxyHeight);
  const context = canvas.getContext("2d", { willReadFrequently: true })!;
  context.fillStyle = "black";
  context.fillRect(0, 0, proxyWidth, proxyHeight);
  const widths = faceLandmarks.map((landmarks) => drawFace(context, landmarks, proxyWidth, proxyHeight));

  const { data } = context.getImageData(0, 0, proxyWidth, proxyHeight);
  const mask = new Uint8Array(proxyWidth * proxyHeight);
  for (let i = 0; i < mask.length; i++) mask[i] = data[i * 4];

  const sorted = [...widths].sort((a, b) => a - b);
  return { mask: mask.buffer, faces: widths.length, faceWidth: sorted[Math.floor(sorted.length / 2)] };
}
