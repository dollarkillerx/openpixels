import type { LensProfile } from "./lens";
import type { OutputSettings } from "./output";

export type Adjustments = Record<string, number>;

/** A healed spot. See `HealSpot` in op-core for the units. */
export interface HealSpot {
  x: number;
  y: number;
  radius: number;
  sourceDx: number;
  sourceDy: number;
  /** Copies the source exactly, as the clone stamp does. */
  clone?: boolean;
  /** Takes the red out of an eye instead of copying anything. */
  redEye?: boolean;
  /** Spots of one stamp drag, which are undone together. */
  group?: number;
}

/** A region filled in by the inpainting model. See `ErasePatch` in op-core. */
export interface ErasePatch {
  x: number;
  y: number;
  width: number;
  height: number;
  size: number;
  rgb: Uint8Array;
  alpha: Uint8Array;
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

/** What identifies a shot's lens. See `LensQuery` in op-core. */
export interface LensQuery {
  camera: string;
  lens: string;
  focal: number;
  aperture: number | null;
}

/** Pixel counts per 8-bit level, for red, green and blue. */
/** What an ICC profile is, as the engine reads it. */
export interface ProfileInfo {
  name: string;
  space: "RGB" | "CMYK" | "GRAY";
  isOutput: boolean;
}

export interface HistogramData {
  channels: [Uint32Array, Uint32Array, Uint32Array];
  total: number;
}

export interface LoadResult {
  width: number;
  height: number;
  isRaw: boolean;
  camera: string | null;
  baseExposure: number | null;
  lens: LensProfile | null;
  lensQuery: LensQuery | null;
  /** Set when the corrections came from the lens database and not the file. */
  lensSource?: "lensfun";
  /** `YYYY:MM:DD HH:MM:SS`, when the file records it. */
  captureTime: string | null;
  cameraMatch: boolean;
  proxyWidth: number;
  proxyHeight: number;
  fileMB: number;
  decodeMs: number;
  proxyMs: number;
  heapMB: number;
}

export interface ExportResult {
  bytes: ArrayBuffer;
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
  | { type: "open"; buffer: ArrayBuffer; adjustments: Adjustments; heals: HealSpot[]; warps: WarpStroke[]; erases: ErasePatch[] }
  | { type: "export"; settings: OutputSettings }
  | { type: "thumbnail"; maxLongEdge: number; framed: boolean }
  | { type: "healAt"; x: number; y: number; stageWidth: number; stageHeight: number; radius: number }
  | { type: "setHeals"; heals: HealSpot[] }
  | { type: "addHeals"; heals: HealSpot[] }
  | { type: "setErases"; erases: ErasePatch[] }
  | { type: "inpaintInput"; x: number; y: number; width: number; height: number; size: number }
  | { type: "whiteBalanceAt"; x: number; y: number; stageWidth: number; stageHeight: number }
  | { type: "autoTone" }
  | { type: "setProof"; icc: ArrayBuffer | null; perceptual: boolean; paper: boolean; gamut: boolean }
  | { type: "describeProfile"; icc: ArrayBuffer }
  | { type: "hueAt"; x: number; y: number; stageWidth: number; stageHeight: number }
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
  | { type: "setSkyMask"; mask: ArrayBuffer; width: number; height: number }
  | { type: "refineMask"; mask: ArrayBuffer; width: number; height: number }
  | { type: "setReplaceImage"; sky: boolean; rgba: ArrayBuffer; width: number; height: number }
  | { type: "warpField" }
  | { type: "setSkinMask"; mask: ArrayBuffer; faceWidth: number }
  | { type: "setLocalMask"; mask: ArrayBuffer; width: number; height: number }
  | { type: "setLook"; size: number; data: ArrayBuffer };

export type EngineCommand =
  | { type: "view"; zoom: number; x: number; y: number }
  | { type: "clipWarnings"; highlights: boolean; shadows: boolean }
  | { type: "adjust"; adjustments: Adjustments }
  | { type: "resize"; width: number; height: number }
  | { type: "compare"; split: number };

export type ToWorker = (EngineRequest & { id: number }) | EngineCommand;

export type FromWorker =
  | { type: "reply"; id: number; result?: unknown; error?: string }
  | { type: "frame"; ms: number }
  | { type: "histogram"; data: HistogramData };

export interface SegmentRequest {
  id: number;
  model: "matte" | "sky";
  tensor: ArrayBuffer;
  width: number;
  height: number;
}

export interface InpaintRequest {
  id: number;
  image: ArrayBuffer;
  mask: ArrayBuffer;
}

export type InpaintReply =
  | { id: number; rgb: ArrayBuffer; loadMs: number; inferMs: number }
  | { id: number; error: string };

export type SegmentReply =
  | { id: number; mask: ArrayBuffer; width: number; height: number; loadMs: number; inferMs: number }
  | { id: number; error: string };
