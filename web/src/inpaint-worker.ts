/// <reference lib="webworker" />
import * as ort from "onnxruntime-web/all";
import { fetchAsset } from "./assets";
import { INPAINT_MODEL } from "./model";
import type { InpaintReply, InpaintRequest } from "./protocol";

// Absolute for the same reason as in ai-worker.ts.
ort.env.wasm.wasmPaths = `${self.location.origin}/ort/`;
// One thread, on purpose: as of 1.31.1, several hang in this worker on this model.
ort.env.wasm.numThreads = 1;

// The runtime's wasm is passed in as bytes, so it can come from a split build like the models.
let runtime: Promise<void> | null = null;
const loadRuntime = () =>
  (runtime ??= fetchAsset(`${self.location.origin}/ort/ort-wasm-simd-threaded.jsep.wasm`).then((binary) => {
    ort.env.wasm.wasmBinary = binary;
  }));

let session: ort.InferenceSession | null = null;

const post = (message: InpaintReply, transfer: Transferable[] = []) =>
  self.postMessage(message, { transfer });

async function inpaint({ id, image, mask }: InpaintRequest) {
  const size = INPAINT_MODEL.size;
  let loadMs = 0;
  if (!session) {
    const start = performance.now();
    // CPU on purpose. As of onnxruntime-web 1.31.1, on this model the JSEP WebGPU
    // provider fails in the Fourier blocks and the native one returns a wrong fill.
    await loadRuntime();
    session = await ort.InferenceSession.create(await fetchAsset(INPAINT_MODEL.url), { executionProviders: ["wasm"] });
    loadMs = performance.now() - start;
  }

  const start = performance.now();
  const results = await session.run({
    image: new ort.Tensor("float32", new Float32Array(image), [1, 3, size, size]),
    mask: new ort.Tensor("float32", new Float32Array(mask), [1, 1, size, size]),
  });
  const output = results[session.outputNames[0]];
  const planes = (await output.getData()) as Float32Array;
  const inferMs = performance.now() - start;

  // Planar 0..255 to interleaved bytes. The clamped array rounds and saturates for us.
  const pixels = size * size;
  const interleaved = new Uint8ClampedArray(pixels * 3);
  for (let i = 0; i < pixels; i++) {
    for (let channel = 0; channel < 3; channel++) {
      interleaved[i * 3 + channel] = planes[channel * pixels + i];
    }
  }
  output.dispose();

  const rgb = interleaved.buffer as ArrayBuffer;
  post({ id, rgb, loadMs, inferMs }, [rgb]);
}

self.onmessage = ({ data }: MessageEvent<InpaintRequest>) => {
  inpaint(data).catch((error: unknown) => {
    post({ id: data.id, error: error instanceof Error ? error.message : String(error) });
  });
};
