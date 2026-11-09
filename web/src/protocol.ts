export type Adjustments = Record<string, number>;

export interface LoadResult {
  width: number;
  height: number;
  isRaw: boolean;
  camera: string | null;
  baseExposure: number | null;
  proxyWidth: number;
  proxyHeight: number;
  fileMB: number;
  decodeMs: number;
  proxyMs: number;
  heapMB: number;
}

export interface ExportResult {
  jpeg: ArrayBuffer;
  ms: number;
  heapMB: number;
}

export interface ModelInput {
  tensor: ArrayBuffer;
  ms: number;
}

export interface Thumbnail {
  rgb: ArrayBuffer;
  width: number;
  height: number;
}

/** Messages the engine worker answers, each paired by id with one reply. */
export type EngineRequest =
  | { type: "init"; canvas: OffscreenCanvas }
  | { type: "open"; buffer: ArrayBuffer; adjustments: Adjustments }
  | { type: "export"; quality: number }
  | { type: "thumbnail"; maxLongEdge: number }
  | { type: "modelInput"; width: number; height: number; mean: number[]; std: number[] }
  | { type: "setMask"; mask: ArrayBuffer; width: number; height: number };

export type EngineCommand =
  | { type: "adjust"; adjustments: Adjustments }
  | { type: "resize"; width: number; height: number };

export type ToWorker = (EngineRequest & { id: number }) | EngineCommand;

export type FromWorker =
  | { type: "reply"; id: number; result?: unknown; error?: string }
  | { type: "frame"; ms: number };

export interface SegmentRequest {
  id: number;
  tensor: ArrayBuffer;
  width: number;
  height: number;
}

export type SegmentReply =
  | { id: number; mask: ArrayBuffer; width: number; height: number; loadMs: number; inferMs: number }
  | { id: number; error: string };
