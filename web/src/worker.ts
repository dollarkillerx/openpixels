/// <reference lib="webworker" />
import { decodeHeic, isHeic } from "./heic";
import { initI18n } from "./i18n";
import { renderWatermark } from "./output";
import init, { Engine, describe_profile } from "./wasm-pkg/op_wasm";
import { engineJson } from "./library";
import { lensfunCalibration } from "./lensfun";
import type {
  Adjustments,
  EngineRequest,
  DevelopTileReply,
  DevelopTileRequest,
  ErasePatch,
  ExportResult,
  FromWorker,
  HistogramData,
  LoadResult,
  ModelInput,
  Thumbnail,
  ToWorker,
} from "./protocol";

const PROXY_LONG_EDGE = 2048;
const HISTOGRAM_LONG_EDGE = 320;
const HISTOGRAM_DELAY_MS = 150;
const DETAIL_DELAY_MS = 120;
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
/** Counts edits as they arrive, so an open can tell older edits from newer ones. */
let adjustSeq = 0;
let pendingSeq = 0;
/** `adjustSeq` when the request being handled arrived. */
let requestArrival = 0;
let renderScheduled = false;
let hasImage = false;

let histogramTimer: ReturnType<typeof setTimeout> | undefined;
let detailTimer: ReturnType<typeof setTimeout> | undefined;

/** Waits for edits to settle; a histogram is not worth computing for every frame of a drag. */
function scheduleHistogram() {
  clearTimeout(histogramTimer);
  histogramTimer = setTimeout(() => {
    enqueue(async () => {
      if (!hasImage) return;
      const rgb = await engine!.histogram_thumbnail(HISTOGRAM_LONG_EDGE);
      const channels: HistogramData["channels"] = [new Uint32Array(256), new Uint32Array(256), new Uint32Array(256)];
      for (let at = 0; at < rgb.length; at += 3) {
        channels[0][rgb[at]]++;
        channels[1][rgb[at + 1]]++;
        channels[2][rgb[at + 2]]++;
      }
      post({ type: "histogram", data: { channels, total: rgb.length / 3 } }, channels.map((channel) => channel.buffer));
    });
  }, HISTOGRAM_DELAY_MS);
}

/** Waits for the view to settle, then develops what is in it at full resolution. */
function scheduleDetail() {
  clearTimeout(detailTimer);
  detailTimer = setTimeout(() => {
    enqueue(async () => {
      if (!hasImage || !engine!.needs_detail()) return;
      if (!engine!.has_full_image()) await developInParallel();
      engine!.update_detail();
      scheduleRender(false);
    });
  }, DETAIL_DELAY_MS);
}

/** `edited` is false when only the view changed, which leaves the histogram as it is. */
function scheduleRender(edited = true) {
  if (edited && hasImage) scheduleHistogram();
  if (renderScheduled || !hasImage) return;
  renderScheduled = true;
  enqueue(async () => {
    renderScheduled = false;
    const start = performance.now();
    if (pendingAdjustments) {
      // Taken first: an edit the engine rejects must not block every later frame.
      const next = pendingAdjustments;
      pendingAdjustments = null;
      engine!.set_adjustments(engineJson(next));
    }
    await engine!.render();
    if (engine!.needs_detail()) scheduleDetail();
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
  let failure: Error | null = null;

  // Each worker settles only once its tile in flight is back, so none touches the
  // engine after this task ends, even when another worker has failed.
  const runOn = (worker: Worker) =>
    new Promise<void>((resolve) => {
      const stop = () => {
        worker.onmessage = null;
        worker.onerror = null;
        resolve();
      };
      const sendNext = () => {
        if (failure || next >= tiles.length) {
          stop();
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
          failure ??= new Error(data.error);
          stop();
          return;
        }
        if (failure) {
          stop();
          return;
        }
        engine!.store_raw_tile(data.index, new Uint16Array(data.rgb));
        sendNext();
      };
      worker.onerror = (event) => {
        failure ??= new Error(event.message);
        stop();
      };
      sendNext();
    });

  await Promise.all(developerPool().map(runOn));
  if (failure) throw failure;
  engine!.finish_raw_develop();
}

/** RAW files with no corrections of their own use the lens database, if it knows the lens. */
async function applyLensDatabase(info: Pick<LoadResult, "lens" | "lensQuery" | "lensSource">) {
  if (info.lens || !info.lensQuery) return;
  const calibration = await lensfunCalibration(info.lensQuery);
  if (!calibration) return;
  info.lens = JSON.parse(engine!.set_lens_calibration(JSON.stringify(calibration)));
  info.lensSource = "lensfun";
}

function pushErases(erases: ErasePatch[]) {
  engine!.clear_erases();
  for (const { x, y, width, height, size, rgb, alpha } of erases) {
    engine!.push_erase(x, y, width, height, size, rgb, alpha);
  }
}

/** Handles a request and returns its result plus any buffers to transfer. */
async function handle(request: EngineRequest): Promise<[unknown, Transferable[]]> {
  switch (request.type) {
    case "init": {
      // Its own copy of the interface language, for the errors it reports.
      await initI18n(request.language);
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
        decoded = engine!.decode_rgba(raster.rgba, raster.width, raster.height, bytes);
      } else {
        decoded = engine!.decode(bytes);
      }
      const info = JSON.parse(decoded);
      await applyLensDatabase(info);
      const t1 = performance.now();
      const proxy = JSON.parse(engine!.build_proxy(PROXY_LONG_EDGE));
      engine!.set_warp_strokes(JSON.stringify(request.warps));
      const t2 = performance.now();
      engine!.set_adjustments(engineJson(request.adjustments));
      pushErases(request.erases);
      engine!.set_heals(JSON.stringify(request.heals));
      // Edits sent after this photo was asked for are its own; only older ones are dropped.
      if (pendingSeq <= requestArrival) pendingAdjustments = null;
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
        const next = pendingAdjustments;
        pendingAdjustments = null;
        engine!.set_adjustments(engineJson(next));
      }
      const start = performance.now();
      if (!engine!.has_full_image()) await developInParallel();
      const { format, quality, bitDepth, longEdge, watermark, colorSpace } = request.settings;
      const size = JSON.parse(engine!.export_size(longEdge));
      const overlay = await renderWatermark(watermark, size.width, size.height);
      const bytes = detach(
        await engine!.export_image(
          JSON.stringify({ format, quality, longEdge, bitDepth, colorSpace }),
          overlay?.rgba ?? new Uint8Array(0),
          overlay?.x ?? 0,
          overlay?.y ?? 0,
          overlay?.width ?? 0,
          overlay?.height ?? 0,
        ),
      );
      const result: ExportResult = { bytes, ms: performance.now() - start, heapMB: heapMB() };
      return [result, [bytes]];
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

    case "addHeals":
      engine!.add_heals(JSON.stringify(request.heals));
      scheduleRender();
      return [undefined, []];

    case "setErases":
      pushErases(request.erases);
      engine!.refresh_erases();
      scheduleRender();
      return [undefined, []];

    case "inpaintInput": {
      const { x, y, width, height, size } = request;
      const start = performance.now();
      // A RAW is only developed at full size on demand. It stays developed for the export.
      if (!engine!.has_full_image()) await developInParallel();
      const tensor = detach(engine!.inpaint_input(x, y, width, height, size));
      const result: ModelInput = { tensor, ms: performance.now() - start };
      return [result, [tensor]];
    }

    case "whiteBalanceAt": {
      const { x, y, stageWidth, stageHeight } = request;
      return [JSON.parse(engine!.white_balance_at(x, y, stageWidth, stageHeight)), []];
    }

    case "setProof": {
      const { icc, perceptual, paper, gamut } = request;
      const info = icc ? JSON.parse(engine!.set_proof(new Uint8Array(icc), perceptual, paper, gamut)) : null;
      if (!icc) engine!.clear_proof();
      // The histogram follows the proof.
      scheduleRender(true);
      return [info, []];
    }

    case "describeProfile":
      return [JSON.parse(describe_profile(new Uint8Array(request.icc))), []];

    case "autoTone":
      return [JSON.parse(await engine!.auto_tone()), []];

    case "hueAt": {
      const { x, y, stageWidth, stageHeight } = request;
      return [engine!.hue_at(x, y, stageWidth, stageHeight), []];
    }

    case "modelInput": {
      const start = performance.now();
      const { width, height, mean, std } = request;
      const tensor = detach(
        engine!.model_input(width, height, new Float32Array(mean), new Float32Array(std)),
      );
      const result: ModelInput = { tensor, ms: performance.now() - start };
      return [result, [tensor]];
    }

    case "refineMask": {
      const refined = detach(engine!.refine_model_mask(request.width, request.height, new Uint8Array(request.mask)));
      return [refined, [refined]];
    }

    case "setSkyMask":
      engine!.set_sky_mask(request.width, request.height, new Uint8Array(request.mask));
      scheduleRender();
      return [undefined, []];

    case "setReplaceImage":
      engine!.set_replace_image(request.sky, request.width, request.height, new Uint8Array(request.rgba));
      scheduleRender();
      return [undefined, []];

    case "warpField": {
      const offsets = detach(engine!.warp_field());
      const size = JSON.parse(engine!.warp_field_size());
      return [offsets.byteLength > 0 ? { ...size, offsets } : null, offsets.byteLength > 0 ? [offsets] : []];
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
    pendingSeq = ++adjustSeq;
    scheduleRender();
    return;
  }
  if (data.type === "resize") {
    const { width, height } = data;
    enqueue(() => engine?.resize(width, height));
    scheduleRender(false);
    return;
  }

  if (data.type === "view") {
    const { zoom, x, y } = data;
    enqueue(() => engine?.set_view(zoom, x, y));
    scheduleRender(false);
    return;
  }

  if (data.type === "clipWarnings") {
    const { highlights, shadows } = data;
    enqueue(() => engine?.set_clip_warnings(highlights, shadows));
    scheduleRender(false);
    return;
  }

  if (data.type === "compare") {
    const { split } = data;
    enqueue(() => engine?.set_compare(split));
    scheduleRender(false);
    return;
  }

  const { id } = data;
  const arrived = adjustSeq;
  enqueue(async () => {
    requestArrival = arrived;
    try {
      const [result, transfer] = await handle(data);
      post({ type: "reply", id, result }, transfer);
    } catch (error) {
      post({ type: "reply", id, error: error instanceof Error ? error.message : String(error) });
    }
  });
};
