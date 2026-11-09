/// <reference lib="webworker" />
import { decodeHeic, isHeic } from "./heic";
import init, { Engine } from "./wasm-pkg/op_wasm";
import { engineJson } from "./library";
import type {
  Adjustments,
  EngineRequest,
  DevelopTileReply,
  DevelopTileRequest,
  ExportResult,
  FromWorker,
  LoadResult,
  ModelInput,
  Thumbnail,
  ToWorker,
} from "./protocol";

const PROXY_LONG_EDGE = 2048;
const MAX_DEVELOP_WORKERS = 8;

let engine: Engine | null = null;
let memory: WebAssembly.Memory | null = null;

// wasm-bindgen forbids re-entering `&mut self` methods while an async one is
// pending, so every engine call goes through this chain.
let queue: Promise<unknown> = Promise.resolve();

const post = (message: FromWorker, transfer: Transferable[] = []) =>
  self.postMessage(message, { transfer });

function enqueue(task: () => Promise<void> | void) {
  queue = queue.then(task).catch((error: unknown) => console.error(error));
}

const heapMB = () => (memory ? memory.buffer.byteLength / 1048576 : 0);

/** Copies a wasm-owned array out so its buffer can be transferred. */
function detach(view: Uint8Array | Float32Array | Uint16Array): ArrayBuffer {
  return view.buffer.slice(view.byteOffset, view.byteOffset + view.byteLength) as ArrayBuffer;
}

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
      engine!.set_adjustments(engineJson(pendingAdjustments));
      pendingAdjustments = null;
    }
    await engine!.render();
    post({ type: "frame", ms: performance.now() - start });
  });
}

let developers: Worker[] | null = null;

function developerPool(): Worker[] {
  if (!developers) {
    const count = Math.max(1, Math.min((navigator.hardwareConcurrency || 2) - 1, MAX_DEVELOP_WORKERS));
    developers = Array.from(
      { length: count },
      () => new Worker(new URL("./develop-worker.ts", import.meta.url), { type: "module" }),
    );
  }
  return developers;
}

/** Develops the open RAW's tiles across the worker pool, if it has any to split up. */
async function developInParallel() {
  const plan = JSON.parse(engine!.begin_raw_develop());
  if (!plan) return;
  const recipeJson = JSON.stringify(plan.recipe);
  const tiles: unknown[] = plan.tiles;
  let next = 0;

  const runOn = (worker: Worker) =>
    new Promise<void>((resolve, reject) => {
      const sendNext = () => {
        if (next >= tiles.length) {
          worker.onmessage = null;
          resolve();
          return;
        }
        const index = next++;
        const values = engine!.raw_tile_input(index);
        const input = detach(values);
        const request: DevelopTileRequest = { index, input, tileJson: JSON.stringify(tiles[index]), recipeJson };
        worker.postMessage(request, [input]);
      };
      worker.onmessage = ({ data }: MessageEvent<DevelopTileReply>) => {
        if ("error" in data) {
          worker.onmessage = null;
          reject(new Error(data.error));
          return;
        }
        engine!.store_raw_tile(data.index, new Uint16Array(data.rgb));
        sendNext();
      };
      worker.onerror = (event) => reject(new Error(event.message));
      sendNext();
    });

  await Promise.all(developerPool().map(runOn));
  engine!.finish_raw_develop();
}

/** Handles a request and returns its result plus any buffers to transfer. */
async function handle(request: EngineRequest): Promise<[unknown, Transferable[]]> {
  switch (request.type) {
    case "init": {
      const exports = await init();
      memory = exports.memory;
      engine = await Engine.create(request.canvas);
      return [undefined, []];
    }

    case "open": {
      const fileMB = request.buffer.byteLength / 1048576;
      hasImage = false;
      const t0 = performance.now();
      const bytes = new Uint8Array(request.buffer);
      let decoded: string;
      if (isHeic(bytes)) {
        const raster = await decodeHeic(bytes);
        decoded = engine!.decode_rgba(raster.rgba, raster.width, raster.height);
      } else {
        decoded = engine!.decode(bytes);
      }
      const info = JSON.parse(decoded);
      const t1 = performance.now();
      const proxy = JSON.parse(engine!.build_proxy(PROXY_LONG_EDGE));
      engine!.set_warp_strokes(JSON.stringify(request.warps));
      const t2 = performance.now();
      engine!.set_adjustments(engineJson(request.adjustments));
      engine!.set_heals(JSON.stringify(request.heals));
      pendingAdjustments = null;
      hasImage = true;
      scheduleRender();
      const result: LoadResult = {
        ...info,
        proxyWidth: proxy.width,
        proxyHeight: proxy.height,
        fileMB,
        decodeMs: t1 - t0,
        proxyMs: t2 - t1,
        heapMB: heapMB(),
      };
      return [result, []];
    }

    case "export": {
      if (pendingAdjustments) {
        engine!.set_adjustments(engineJson(pendingAdjustments));
        pendingAdjustments = null;
      }
      const start = performance.now();
      await developInParallel();
      const jpeg = detach(await engine!.export_jpeg(request.quality));
      const result: ExportResult = { jpeg, ms: performance.now() - start, heapMB: heapMB() };
      return [result, [jpeg]];
    }

    case "thumbnail": {
      const { maxLongEdge, framed } = request;
      const size = JSON.parse(framed ? engine!.framed_thumbnail_size(maxLongEdge) : engine!.thumbnail_size(maxLongEdge));
      const rgb = detach(await (framed ? engine!.framed_thumbnail(maxLongEdge) : engine!.thumbnail(maxLongEdge)));
      const result: Thumbnail = { rgb, width: size.width, height: size.height };
      return [result, [rgb]];
    }

    case "healAt": {
      const { x, y, stageWidth, stageHeight, radius } = request;
      const spot = JSON.parse(engine!.heal_at(x, y, stageWidth, stageHeight, radius));
      scheduleRender();
      return [spot, []];
    }

    case "liquify": {
      const { fromX, fromY, toX, toY, stageWidth, stageHeight, radius, strength, mode } = request;
      const stroke = JSON.parse(engine!.liquify(fromX, fromY, toX, toY, stageWidth, stageHeight, radius, strength, mode));
      scheduleRender();
      return [stroke, []];
    }

    case "setWarps":
      engine!.set_warp_strokes(JSON.stringify(request.strokes));
      scheduleRender();
      return [undefined, []];

    case "setHeals":
      engine!.set_heals(JSON.stringify(request.heals));
      scheduleRender();
      return [undefined, []];

    case "modelInput": {
      const start = performance.now();
      const { width, height, mean, std } = request;
      const tensor = detach(
        engine!.model_input(width, height, new Float32Array(mean), new Float32Array(std)),
      );
      const result: ModelInput = { tensor, ms: performance.now() - start };
      return [result, [tensor]];
    }

    case "setMask":
      engine!.set_mask(request.width, request.height, new Uint8Array(request.mask));
      scheduleRender();
      return [undefined, []];

    case "setLook":
      engine!.set_look(request.size, new Float32Array(request.data));
      scheduleRender();
      return [undefined, []];

    case "setLocalMask":
      engine!.set_local_mask(request.width, request.height, new Uint8Array(request.mask));
      scheduleRender();
      return [undefined, []];

    case "setSkinMask":
      engine!.set_skin_mask(new Uint8Array(request.mask), request.faceWidth);
      scheduleRender();
      return [undefined, []];
  }
}

self.onmessage = ({ data }: MessageEvent<ToWorker>) => {
  if (data.type === "adjust") {
    pendingAdjustments = data.adjustments;
    scheduleRender();
    return;
  }
  if (data.type === "resize") {
    const { width, height } = data;
    enqueue(() => engine?.resize(width, height));
    scheduleRender();
    return;
  }

  const { id } = data;
  enqueue(async () => {
    try {
      const [result, transfer] = await handle(data);
      post({ type: "reply", id, result }, transfer);
    } catch (error) {
      post({ type: "reply", id, error: error instanceof Error ? error.message : String(error) });
    }
  });
};
