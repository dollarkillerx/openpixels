import { buildSkinMask } from "./face";
import type { Lut } from "./lut";
import type { OutputSettings } from "./output";
import type { EraseCrop } from "./erase";
import type { WarpView } from "./lens";
import type { ModelMask } from "./local";
import { INPAINT_MODEL, MODEL, SKY_MODEL, modelInputSize } from "./model";
import type { ReplaceImage } from "./replace";
import type {
  Adjustments,
  EngineRequest,
  ErasePatch,
  ExportResult,
  FromWorker,
  HealSpot,
  HistogramData,
  InpaintReply,
  InpaintRequest,
  LoadResult,
  ModelInput,
  SegmentReply,
  SegmentRequest,
  Thumbnail,
  ToWorker,
  WarpStroke,
  ProfileInfo,
} from "./protocol";

export interface MaskStats {
  inputMs: number;
  loadMs: number;
  inferMs: number;
}

interface Pending {
  resolve: (value: unknown) => void;
  reject: (reason: Error) => void;
}

export interface SkinStats {
  faces: number;
  ms: number;
}

const FACE_DETECTION_LONG_EDGE = 1024;
const SMALL_FACE_DETECTION_LONG_EDGE = 2048;
/** The inpainting model is large, so its worker goes away when no longer in use. */
const INPAINT_IDLE_MS = 5 * 60 * 1000;

/** Promise-based front end for the engine and segmentation workers. */
export class EngineClient {
  private readonly worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
  private readonly aiWorker = new Worker(new URL("./ai-worker.ts", import.meta.url), { type: "module" });
  private readonly pending = new Map<number, Pending>();
  private readonly pendingSegments = new Map<number, (reply: SegmentReply) => void>();
  private inpaintWorker: Worker | null = null;
  private inpaintIdleTimer = 0;
  private nextId = 1;

  /** Called after each frame reaches the GPU. */
  onFrame: (ms: number) => void = () => {};
  /** Called when the histogram of the edited image has changed. */
  onHistogram: (data: HistogramData) => void = () => {};
  /** Called for failures in a worker that no request is waiting on. */
  onCrash: (message: string) => void = () => {};

  constructor(canvas: OffscreenCanvas) {
    this.worker.onmessage = ({ data }: MessageEvent<FromWorker>) => {
      if (data.type === "frame") {
        this.onFrame(data.ms);
        return;
      }
      if (data.type === "histogram") {
        this.onHistogram(data.data);
        return;
      }
      const request = this.pending.get(data.id);
      this.pending.delete(data.id);
      if (data.error !== undefined) request?.reject(new Error(data.error));
      else request?.resolve(data.result);
    };
    this.worker.onerror = (event) => {
      // Nothing more will come back for these.
      for (const request of this.pending.values()) request.reject(new Error(event.message));
      this.pending.clear();
      this.onCrash(event.message);
    };

    this.aiWorker.onmessage = ({ data }: MessageEvent<SegmentReply>) => {
      this.pendingSegments.get(data.id)?.(data);
      this.pendingSegments.delete(data.id);
    };
    this.aiWorker.onerror = (event) => {
      for (const [id, settle] of this.pendingSegments) settle({ id, error: event.message });
      this.pendingSegments.clear();
      this.onCrash(event.message);
    };

    this.ready = this.call<void>({ type: "init", canvas }, [canvas]);
  }

  readonly ready: Promise<void>;

  private call<T>(request: EngineRequest, transfer: Transferable[] = []): Promise<T> {
    const id = this.nextId++;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (value: unknown) => void, reject });
      this.worker.postMessage({ ...request, id } satisfies ToWorker, transfer);
    });
  }

  open(buffer: ArrayBuffer, adjustments: Adjustments, heals: HealSpot[], warps: WarpStroke[], erases: ErasePatch[]) {
    return this.call<LoadResult>({ type: "open", buffer, adjustments, heals, warps, erases }, [buffer]);
  }

  setErases(erases: ErasePatch[]) {
    return this.call<void>({ type: "setErases", erases });
  }

  /** Fills the marked part of a rectangle, given as fractions of the image. Resolves to interleaved sRGB bytes. */
  inpaint(crop: EraseCrop, mask: Float32Array<ArrayBuffer>) {
    // The inpaint worker answers one request at a time.
    const result = this.inpaintQueue.then(() => this.inpaintNow(crop, mask));
    this.inpaintQueue = result.catch(() => {});
    return result;
  }

  private inpaintQueue: Promise<unknown> = Promise.resolve();

  private async inpaintNow(crop: EraseCrop, mask: Float32Array<ArrayBuffer>) {
    const input = await this.call<ModelInput>({ type: "inpaintInput", ...crop, size: INPAINT_MODEL.size });
    window.clearTimeout(this.inpaintIdleTimer);
    this.inpaintWorker ??= new Worker(new URL("./inpaint-worker.ts", import.meta.url), { type: "module" });
    const worker = this.inpaintWorker;
    const id = this.nextId++;
    const reply = await new Promise<InpaintReply>((resolve, reject) => {
      worker.onmessage = ({ data }: MessageEvent<InpaintReply>) => {
        if (data.id === id) resolve(data);
      };
      worker.onerror = (event) => reject(new Error(event.message));
      const request: InpaintRequest = { id, image: input.tensor, mask: mask.buffer };
      worker.postMessage(request, [input.tensor, mask.buffer]);
    }).finally(() => {
      this.inpaintIdleTimer = window.setTimeout(() => this.releaseInpaint(), INPAINT_IDLE_MS);
    });
    if ("error" in reply) throw new Error(reply.error);
    return { rgb: new Uint8Array(reply.rgb), loadMs: reply.loadMs, inferMs: reply.inferMs };
  }

  private releaseInpaint() {
    this.inpaintWorker?.terminate();
    this.inpaintWorker = null;
  }

  /** Pushes the image between two positions in 0..=1 on the stage. */
  liquify(request: Omit<Extract<EngineRequest, { type: "liquify" }>, "type">) {
    return this.call<WarpStroke | null>({ type: "liquify", ...request });
  }

  setWarps(strokes: WarpStroke[]) {
    return this.call<void>({ type: "setWarps", strokes });
  }

  /** Heals at a position in 0..=1 on the stage. Resolves to null if it missed the image. */
  healAt(x: number, y: number, stageWidth: number, stageHeight: number, radius: number) {
    return this.call<HealSpot | null>({ type: "healAt", x, y, stageWidth, stageHeight, radius });
  }

  setHeals(heals: HealSpot[]) {
    return this.call<void>({ type: "setHeals", heals });
  }

  addHeals(heals: HealSpot[]) {
    return this.call<void>({ type: "addHeals", heals });
  }

  adjust(adjustments: Adjustments) {
    this.worker.postMessage({ type: "adjust", adjustments } satisfies ToWorker);
  }

  resize(width: number, height: number) {
    this.worker.postMessage({ type: "resize", width, height } satisfies ToWorker);
  }

  exportImage(settings: OutputSettings) {
    return this.call<ExportResult>({ type: "export", settings });
  }

  /** Shows the image `zoom` times its fitted size, around (x, y) in 0..=1. */
  setView(zoom: number, x: number, y: number) {
    this.worker.postMessage({ type: "view", zoom, x, y } satisfies ToWorker);
  }

  setClipWarnings(highlights: boolean, shadows: boolean) {
    this.worker.postMessage({ type: "clipWarnings", highlights, shadows } satisfies ToWorker);
  }

  /** Shows the unedited image on this fraction of the frame, from the left. 0 is off. */
  compare(split: number) {
    this.worker.postMessage({ type: "compare", split } satisfies ToWorker);
  }

  /** Shows the preview through an ICC profile, or stops when `icc` is null. Exports are unaffected. */
  setProof(icc: ArrayBuffer | null, options: { perceptual: boolean; paper: boolean; gamut: boolean }) {
    const copy = icc ? icc.slice(0) : null;
    return this.call<ProfileInfo | null>({ type: "setProof", icc: copy, ...options }, copy ? [copy] : []);
  }

  /** Checks an ICC file can be used for soft proofing. Rejects with the reason if not. */
  describeProfile(icc: ArrayBuffer) {
    const copy = icc.slice(0);
    return this.call<ProfileInfo>({ type: "describeProfile", icc: copy }, [copy]);
  }

  /** Tone sliders suggested for the photo as framed. */
  autoTone() {
    return this.call<Adjustments>({ type: "autoTone" });
  }

  /** The hue at a stage position in degrees, or a negative number if it has none. */
  hueAt(x: number, y: number, stageWidth: number, stageHeight: number) {
    return this.call<number>({ type: "hueAt", x, y, stageWidth, stageHeight });
  }

  /** Finds the white balance making a stage position neutral. Null if it cannot. */
  whiteBalanceAt(x: number, y: number, stageWidth: number, stageHeight: number) {
    return this.call<{ temperature: number; tint: number } | null>({ type: "whiteBalanceAt", x, y, stageWidth, stageHeight });
  }

  /** `framed` applies the crop and rotation; analysis needs the unframed image. */
  thumbnail(maxLongEdge: number, framed: boolean) {
    return this.call<Thumbnail>({ type: "thumbnail", maxLongEdge, framed });
  }

  /** Applies a look, or none. */
  setLook(lut: Lut | null) {
    if (!lut) return this.call<void>({ type: "setLook", size: 0, data: new ArrayBuffer(0) });
    const data = lut.data.slice().buffer;
    return this.call<void>({ type: "setLook", size: lut.size, data }, [data]);
  }

  setLocalMask(mask: ArrayBuffer, width: number, height: number) {
    return this.call<void>({ type: "setLocalMask", mask, width, height }, [mask]);
  }

  /** Finds faces in the open image and builds its skin corrections. */
  async prepareSkin(proxyWidth: number, proxyHeight: number): Promise<SkinStats> {
    const start = performance.now();
    const thumbnail = await this.thumbnail(FACE_DETECTION_LONG_EDGE, false);
    const skin = await buildSkinMask(
      thumbnail,
      () => this.thumbnail(SMALL_FACE_DETECTION_LONG_EDGE, false),
      proxyWidth,
      proxyHeight,
    );
    await this.call<void>({ type: "setSkinMask", mask: skin.mask, faceWidth: skin.faceWidth }, [skin.mask]);
    return { faces: skin.faces, ms: performance.now() - start };
  }

  private async segment(model: SegmentRequest["model"], size: { width: number; height: number }, mean: number[], std: number[]) {
    const input = await this.call<ModelInput>({ type: "modelInput", ...size, mean, std });
    const id = this.nextId++;
    const reply = await new Promise<SegmentReply>((resolve) => {
      this.pendingSegments.set(id, resolve);
      const request: SegmentRequest = { id, model, tensor: input.tensor, ...size };
      this.aiWorker.postMessage(request, [input.tensor]);
    });
    if ("error" in reply) throw new Error(reply.error);
    return { ...reply, inputMs: input.ms };
  }

  /** Runs the matting model on the open image and applies the result as its mask. */
  async detectSubject(proxyWidth: number, proxyHeight: number): Promise<MaskStats> {
    const { mask, width, height, inputMs, loadMs, inferMs } = await this.segment("matte", modelInputSize(proxyWidth, proxyHeight), MODEL.mean, MODEL.std);
    await this.call<void>({ type: "setMask", mask, width, height }, [mask]);
    return { inputMs, loadMs, inferMs };
  }

  /** Finds the sky in the open image, for replacing it. */
  async detectSky(): Promise<MaskStats> {
    const size = { width: SKY_MODEL.size, height: SKY_MODEL.size };
    const { mask, width, height, inputMs, loadMs, inferMs } = await this.segment("sky", size, SKY_MODEL.mean, SKY_MODEL.std);
    await this.call<void>({ type: "setSkyMask", mask, width, height }, [mask]);
    return { inputMs, loadMs, inferMs };
  }

  /** A model's mask for the open image, following its edges, at the proxy's size. */
  async modelMask(model: "subject" | "sky", proxyWidth: number, proxyHeight: number): Promise<ModelMask> {
    const reply =
      model === "sky"
        ? await this.segment("sky", { width: SKY_MODEL.size, height: SKY_MODEL.size }, SKY_MODEL.mean, SKY_MODEL.std)
        : await this.segment("matte", modelInputSize(proxyWidth, proxyHeight), MODEL.mean, MODEL.std);
    const refined = await this.call<ArrayBuffer>({ type: "refineMask", mask: reply.mask, width: reply.width, height: reply.height }, [reply.mask]);
    return { mask: new Uint8Array(refined), width: proxyWidth, height: proxyHeight };
  }

  setReplaceImage(sky: boolean, { rgba, width, height }: ReplaceImage) {
    return this.call<void>({ type: "setReplaceImage", sky, rgba, width, height }, [rgba]);
  }

  /** The liquify field, or null if the image is not warped. */
  async warpField(): Promise<WarpView | null> {
    const field = await this.call<{ width: number; height: number; offsets: ArrayBuffer } | null>({ type: "warpField" });
    return field && { ...field, offsets: new Float32Array(field.offsets) };
  }
}
