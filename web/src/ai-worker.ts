/// <reference lib="webworker" />
// The JSEP build, on purpose. As of 1.31.1 the "/webgpu" entry point (the native
// WebGPU execution provider) runs MODNet without any error but returns a wrong matte.
import * as ort from "onnxruntime-web/all";
import { MODEL, SKY_MODEL } from "./model";
import type { SegmentReply, SegmentRequest } from "./protocol";

// Absolute on purpose: the Vite dev server rejects dynamic imports of root-relative
// public files, and leaves absolute URLs alone.
ort.env.wasm.wasmPaths = `${self.location.origin}/ort/`;
// All the model work happens on the GPU, so extra CPU threads would buy nothing and
// would require cross-origin isolation.
ort.env.wasm.numThreads = 1;

const URLS = { matte: MODEL.url, sky: SKY_MODEL.url };
const sessions = new Map<SegmentRequest["model"], ort.InferenceSession>();

const post = (message: SegmentReply, transfer: Transferable[] = []) =>
  self.postMessage(message, { transfer });

/** Probability of one class at each pixel, from per-class logits. */
function classProbability(logits: Float32Array, classes: number, wanted: number) {
  const pixels = logits.length / classes;
  const out = new Uint8ClampedArray(pixels);
  for (let i = 0; i < pixels; i++) {
    let total = 0;
    for (let c = 0; c < classes; c++) total += Math.exp(logits[c * pixels + i]);
    out[i] = (Math.exp(logits[wanted * pixels + i]) / total) * 255;
  }
  return out;
}

/**
 * Clears patches of "sky" entirely below the sky that reaches the top of the frame,
 * which are really snow, water or bright walls. Gaps between branches are level with
 * the sky, so they stay.
 */
function removeStraySky(mask: Uint8ClampedArray, width: number, height: number) {
  const labels = new Int32Array(mask.length);
  const regions: { top: number; bottom: number; touchesTop: boolean; size: number }[] = [];
  const stack: number[] = [];
  for (let start = 0; start < mask.length; start++) {
    if (labels[start] || mask[start] < 128) continue;
    regions.push({ top: height, bottom: 0, touchesTop: false, size: 0 });
    const region = regions[regions.length - 1];
    labels[start] = regions.length;
    stack.push(start);
    while (stack.length > 0) {
      const at = stack.pop()!;
      const [x, y] = [at % width, Math.floor(at / width)];
      region.top = Math.min(region.top, y);
      region.bottom = Math.max(region.bottom, y);
      region.touchesTop ||= y === 0;
      region.size++;
      for (const next of [x > 0 ? at - 1 : -1, x < width - 1 ? at + 1 : -1, y > 0 ? at - width : -1, y < height - 1 ? at + width : -1]) {
        if (next >= 0 && !labels[next] && mask[next] >= 128) {
          labels[next] = regions.length;
          stack.push(next);
        }
      }
    }
  }
  const main = regions.filter((region) => region.touchesTop).sort((a, b) => b.size - a.size)[0];
  if (!main) return;
  for (let i = 0; i < mask.length; i++) {
    if (labels[i] && regions[labels[i] - 1].top > main.bottom) mask[i] = 0;
  }
}

async function segment({ id, model, tensor, width, height }: SegmentRequest) {
  let loadMs = 0;
  let session = sessions.get(model);
  if (!session) {
    const start = performance.now();
    // WebGPU only: a silent fallback to CPU would hide a slowdown of orders of magnitude.
    session = await ort.InferenceSession.create(URLS[model], { executionProviders: ["webgpu"] });
    sessions.set(model, session);
    loadMs = performance.now() - start;
  }

  const start = performance.now();
  const input = new ort.Tensor("float32", new Float32Array(tensor), [1, 3, height, width]);
  const results = await session.run({ [session.inputNames[0]]: input });
  const output = results[session.outputNames[0]];
  const data = (await output.getData()) as Float32Array;
  const inferMs = performance.now() - start;
  output.dispose();

  // The clamped arrays round and saturate for us; a matte is alpha in 0..1.
  let result: Uint8ClampedArray;
  if (model === "sky") {
    result = classProbability(data, SKY_MODEL.classes, SKY_MODEL.skyClass);
    removeStraySky(result, width, height);
  } else {
    result = Uint8ClampedArray.from(data, (alpha) => alpha * 255);
  }
  const mask = result.buffer as ArrayBuffer;
  post({ id, mask, width, height, loadMs, inferMs }, [mask]);
}

self.onmessage = ({ data }: MessageEvent<SegmentRequest>) => {
  segment(data).catch((error: unknown) => {
    post({ id: data.id, error: error instanceof Error ? error.message : String(error) });
  });
};
