import { buildSkinMask } from "./face";
import { MODEL, modelInputSize } from "./model";
import type {
  Adjustments,
  EngineRequest,
  ExportResult,
  FromWorker,
  LoadResult,
  ModelInput,
  SegmentReply,
  SegmentRequest,
  Thumbnail,
  ToWorker,
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

  open(buffer: ArrayBuffer, adjustments: Adjustments) {
    return this.call<LoadResult>({ type: "open", buffer, adjustments }, [buffer]);
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

  thumbnail(maxLongEdge: number) {
    return this.call<Thumbnail>({ type: "thumbnail", maxLongEdge });
  }

  /** Finds faces in the open image and builds its skin corrections. */
  async prepareSkin(proxyWidth: number, proxyHeight: number): Promise<SkinStats> {
    const start = performance.now();
    const thumbnail = await this.thumbnail(FACE_DETECTION_LONG_EDGE);
    const skin = await buildSkinMask(thumbnail, proxyWidth, proxyHeight);
    if (skin) {
      await this.call<void>({ type: "setSkinMask", mask: skin.mask, faceWidth: skin.faceWidth }, [skin.mask]);
    }
    return { faces: skin?.faces ?? 0, ms: performance.now() - start };
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
