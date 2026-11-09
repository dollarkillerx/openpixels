export type Adjustments = Record<string, number>;

export type ToWorker =
  | { type: "init"; canvas: OffscreenCanvas }
  | { type: "load"; buffer: ArrayBuffer; name: string }
  | { type: "adjust"; adjustments: Adjustments }
  | { type: "resize"; width: number; height: number }
  | { type: "export"; quality: number }
  | {
      type: "modelInput";
      width: number;
      height: number;
      mean: number[];
      std: number[];
      generation: number;
    }
  | { type: "setMask"; mask: ArrayBuffer; width: number; height: number };

export interface LoadResult {
  name: string;
  width: number;
  height: number;
  isRaw: boolean;
  camera: string | null;
  proxyWidth: number;
  proxyHeight: number;
  fileMB: number;
  decodeMs: number;
  proxyMs: number;
}

export type FromWorker =
  | { type: "ready" }
  | { type: "loaded"; result: LoadResult; heapMB: number }
  | { type: "frame"; ms: number }
  | { type: "exported"; jpeg: ArrayBuffer; ms: number; heapMB: number }
  | {
      type: "modelInput";
      tensor: ArrayBuffer;
      width: number;
      height: number;
      generation: number;
      ms: number;
    }
  | { type: "error"; message: string };

/** `generation` identifies the loaded image, so a late result for an old one is dropped. */
export type ToAi = {
  type: "segment";
  tensor: ArrayBuffer;
  width: number;
  height: number;
  generation: number;
};

export type FromAi =
  | {
      type: "mask";
      mask: ArrayBuffer;
      width: number;
      height: number;
      loadMs: number;
      inferMs: number;
      generation: number;
    }
  | { type: "error"; message: string };
