import { FaceLandmarker, FilesetResolver, type NormalizedLandmark } from "@mediapipe/tasks-vision";
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

/** Finds faces in `thumbnail` and rasterises their skin at the given proxy size. */
export async function buildSkinMask(
  { rgb, width, height }: Thumbnail,
  proxyWidth: number,
  proxyHeight: number,
): Promise<SkinMask | null> {
  const pixels = new Uint8ClampedArray(width * height * 4);
  const source = new Uint8Array(rgb);
  for (let i = 0, j = 0; i < source.length; i += 3, j += 4) {
    pixels[j] = source[i];
    pixels[j + 1] = source[i + 1];
    pixels[j + 2] = source[i + 2];
    pixels[j + 3] = 255;
  }

  const detector = await getLandmarker();
  const { faceLandmarks } = detector.detect(new ImageData(pixels, width, height));
  if (faceLandmarks.length === 0) return null;

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
