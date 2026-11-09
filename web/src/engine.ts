import { buildSkinMask } from "./face";
import { MODEL, modelInputSize } from "./model";
import type {
  Adjustments,
  EngineRequest,
  ExportResult,
  FromWorker,
  HealSpot,
  LoadResult,
  ModelInput,
  SegmentReply,
  SegmentRequest,
  Thumbnail,
  ToWorker,
  WarpStroke,
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

/** Promise-based front end for the engine and segmentation workers. */
export class EngineClient {
  private readonly worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
  private readonly aiWorker = new Worker(new URL("./ai-worker.ts", import.meta.url), { type: "module" });
  private readonly pending = new Map<number, Pending>();
  private readonly pendingSegments = new Map<number, (reply: SegmentReply) => void>();
  private nextId = 1;

  /** Called after each frame reaches the GPU. */
  onFrame: (ms: number) => void = () => {};
  /** Called for failures in a worker that no request is waiting on. */
  onCrash: (message: string) => void = () => {};

  constructor(canvas: OffscreenCanvas) {
    this.worker.onmessage = ({ data }: MessageEvent<FromWorker>) => {
      if (data.type === "frame") {
        this.onFrame(data.ms);
        return;
      }
      const request = this.pending.get(data.id);
      this.pending.delete(data.id);
      if (data.error !== undefined) request?.reject(new Error(data.error));
      else request?.resolve(data.result);
    };
    this.worker.onerror = (event) => this.onCrash(event.message);

    this.aiWorker.onmessage = ({ data }: MessageEvent<SegmentReply>) => {
      this.pendingSegments.get(data.id)?.(data);
      this.pendingSegments.delete(data.id);
    };
    this.aiWorker.onerror = (event) => this.onCrash(event.message);

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

  open(buffer: ArrayBuffer, adjustments: Adjustments, heals: HealSpot[], warps: WarpStroke[]) {
    return this.call<LoadResult>({ type: "open", buffer, adjustments, heals, warps }, [buffer]);
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

  adjust(adjustments: Adjustments) {
    this.worker.postMessage({ type: "adjust", adjustments } satisfies ToWorker);
  }

  resize(width: number, height: number) {
    this.worker.postMessage({ type: "resize", width, height } satisfies ToWorker);
  }

  exportJpeg(quality: number) {
    return this.call<ExportResult>({ type: "export", quality });
  }

  /** `framed` applies the crop and rotation; analysis needs the unframed image. */
  thumbnail(maxLongEdge: number, framed: boolean) {
    return this.call<Thumbnail>({ type: "thumbnail", maxLongEdge, framed });
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

  /** Runs the matting model on the open image and applies the result as its mask. */
  async detectSubject(proxyWidth: number, proxyHeight: number): Promise<MaskStats> {
    const size = modelInputSize(proxyWidth, proxyHeight);
    const input = await this.call<ModelInput>({
      type: "modelInput",
      ...size,
      mean: MODEL.mean,
      std: MODEL.std,
    });

    const id = this.nextId++;
    const reply = await new Promise<SegmentReply>((resolve) => {
      this.pendingSegments.set(id, resolve);
      const request: SegmentRequest = { id, tensor: input.tensor, ...size };
      this.aiWorker.postMessage(request, [input.tensor]);
    });
    if ("error" in reply) throw new Error(reply.error);

    const { mask, width, height, loadMs, inferMs } = reply;
    await this.call<void>({ type: "setMask", mask, width, height }, [mask]);
    return { inputMs: input.ms, loadMs, inferMs };
  }
}
