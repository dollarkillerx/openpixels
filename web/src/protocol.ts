import type { LensProfile } from "./lens";

export type Adjustments = Record<string, number>;

/** A healed spot. See `HealSpot` in op-core for the units. */
export interface HealSpot {
  x: number;
  y: number;
  radius: number;
  sourceDx: number;
  sourceDy: number;
}

/** A liquify brush step. See `WarpStroke` in op-core for the units. */
export interface WarpStroke {
  x: number;
  y: number;
  dx: number;
  dy: number;
  radius: number;
  strength: number;
}

export interface LoadResult {
  width: number;
  height: number;
  isRaw: boolean;
  camera: string | null;
  baseExposure: number | null;
  lens: LensProfile | null;
  cameraMatch: boolean;
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

export interface DevelopTileRequest {
  index: number;
  input: ArrayBuffer;
  tileJson: string;
  recipeJson: string;
}

export type DevelopTileReply = { index: number; rgb: ArrayBuffer } | { index: number; error: string };

/** Messages the engine worker answers, each paired by id with one reply. */
export type EngineRequest =
  | { type: "init"; canvas: OffscreenCanvas }
  | { type: "open"; buffer: ArrayBuffer; adjustments: Adjustments; heals: HealSpot[]; warps: WarpStroke[] }
  | { type: "export"; quality: number }
  | { type: "thumbnail"; maxLongEdge: number; framed: boolean }
  | { type: "healAt"; x: number; y: number; stageWidth: number; stageHeight: number; radius: number }
  | { type: "setHeals"; heals: HealSpot[] }
  | { type: "setWarps"; strokes: WarpStroke[] }
  | {
      type: "liquify";
      fromX: number;
      fromY: number;
      toX: number;
      toY: number;
      stageWidth: number;
      stageHeight: number;
      radius: number;
      strength: number;
      mode: number;
    }
  | { type: "modelInput"; width: number; height: number; mean: number[]; std: number[] }
  | { type: "setMask"; mask: ArrayBuffer; width: number; height: number }
  | { type: "setSkinMask"; mask: ArrayBuffer; faceWidth: number }
  | { type: "setLocalMask"; mask: ArrayBuffer; width: number; height: number }
  | { type: "setLook"; size: number; data: ArrayBuffer };

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
