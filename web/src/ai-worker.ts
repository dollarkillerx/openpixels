/// <reference lib="webworker" />
// The JSEP build, on purpose. As of 1.31.1 the "/webgpu" entry point (the native
// WebGPU execution provider) runs MODNet without any error but returns a wrong matte.
import * as ort from "onnxruntime-web/all";
import { MODEL } from "./model";
import type { FromAi, ToAi } from "./protocol";

// Absolute on purpose: the Vite dev server rejects dynamic imports of root-relative
// public files, and leaves absolute URLs alone.
ort.env.wasm.wasmPaths = `${self.location.origin}/ort/`;
// All the model work happens on the GPU, so extra CPU threads would buy nothing and
// would require cross-origin isolation.
ort.env.wasm.numThreads = 1;

let session: ort.InferenceSession | null = null;

const post = (message: FromAi, transfer: Transferable[] = []) =>
  self.postMessage(message, { transfer });

async function segment({ tensor, width, height, generation }: ToAi) {
  let loadMs = 0;
  if (!session) {
    const start = performance.now();
    // WebGPU only: a silent fallback to CPU would hide a slowdown of orders of magnitude.
    session = await ort.InferenceSession.create(MODEL.url, { executionProviders: ["webgpu"] });
    loadMs = performance.now() - start;
  }

  const start = performance.now();
  const input = new ort.Tensor("float32", new Float32Array(tensor), [1, 3, height, width]);
  const results = await session.run({ [session.inputNames[0]]: input });
  const output = results[session.outputNames[0]];
  const data = (await output.getData()) as Float32Array;
  const inferMs = performance.now() - start;
  output.dispose();

  // Alpha matte in 0..1. The clamped array rounds and saturates for us.
  const { buffer: mask } = Uint8ClampedArray.from(data, (alpha) => alpha * 255);
  post({ type: "mask", mask, width, height, loadMs, inferMs, generation }, [mask]);
}

self.onmessage = ({ data }: MessageEvent<ToAi>) => {
  segment(data).catch((error: unknown) => {
    post({ type: "error", message: error instanceof Error ? error.message : String(error) });
  });
};
