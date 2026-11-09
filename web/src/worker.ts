/// <reference lib="webworker" />
import init, { Engine } from "./wasm-pkg/op_wasm";
import type { Adjustments, FromWorker, ToWorker } from "./protocol";

const PROXY_LONG_EDGE = 2048;

let engine: Engine | null = null;
let memory: WebAssembly.Memory | null = null;

// wasm-bindgen forbids re-entering `&mut self` methods while an async one is
// pending, so every engine call goes through this chain.
let queue: Promise<unknown> = Promise.resolve();

const post = (message: FromWorker, transfer: Transferable[] = []) =>
  self.postMessage(message, { transfer });

function enqueue(task: () => Promise<void> | void) {
  queue = queue.then(task).catch((error: unknown) => {
    post({ type: "error", message: error instanceof Error ? error.message : String(error) });
  });
}

const heapMB = () => (memory ? memory.buffer.byteLength / 1048576 : 0);

// Slider drags arrive far faster than frames. Only the newest values get rendered.
let pendingAdjustments: Adjustments | null = null;
let renderScheduled = false;
let hasImage = false;

function scheduleRender() {
  if (renderScheduled || !hasImage) return;
  renderScheduled = true;
  enqueue(async () => {
    renderScheduled = false;
    const start = performance.now();
    if (pendingAdjustments) {
      engine!.set_adjustments(JSON.stringify(pendingAdjustments));
      pendingAdjustments = null;
    }
    await engine!.render();
    post({ type: "frame", ms: performance.now() - start });
  });
}

self.onmessage = ({ data }: MessageEvent<ToWorker>) => {
  switch (data.type) {
    case "init":
      enqueue(async () => {
        const exports = await init();
        memory = exports.memory;
        engine = await Engine.create(data.canvas);
        post({ type: "ready" });
      });
      break;

    case "load":
      enqueue(async () => {
        const fileMB = data.buffer.byteLength / 1048576;
        const t0 = performance.now();
        const info = JSON.parse(engine!.decode(new Uint8Array(data.buffer)));
        const t1 = performance.now();
        const proxy = JSON.parse(engine!.build_proxy(PROXY_LONG_EDGE));
        const t2 = performance.now();
        hasImage = true;
        post({
          type: "loaded",
          heapMB: heapMB(),
          result: {
            ...info,
            name: data.name,
            proxyWidth: proxy.width,
            proxyHeight: proxy.height,
            fileMB,
            decodeMs: t1 - t0,
            proxyMs: t2 - t1,
          },
        });
        scheduleRender();
      });
      break;

    case "adjust":
      pendingAdjustments = data.adjustments;
      scheduleRender();
      break;

    case "resize":
      enqueue(() => engine!.resize(data.width, data.height));
      scheduleRender();
      break;

    case "modelInput":
      enqueue(() => {
        const start = performance.now();
        const values = engine!.model_input(
          data.width,
          data.height,
          new Float32Array(data.mean),
          new Float32Array(data.std),
        );
        const tensor = values.buffer.slice(
          values.byteOffset,
          values.byteOffset + values.byteLength,
        ) as ArrayBuffer;
        post(
          {
            type: "modelInput",
            tensor,
            width: data.width,
            height: data.height,
            generation: data.generation,
            ms: performance.now() - start,
          },
          [tensor],
        );
      });
      break;

    case "setMask":
      enqueue(() => engine!.set_mask(data.width, data.height, new Uint8Array(data.mask)));
      scheduleRender();
      break;

    case "export":
      enqueue(async () => {
        const start = performance.now();
        const bytes = await engine!.export_jpeg(data.quality);
        const jpeg = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
        post({ type: "exported", jpeg, ms: performance.now() - start, heapMB: heapMB() }, [jpeg]);
      });
      break;
  }
};
