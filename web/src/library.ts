import { CURVE_CHANNELS, curvePoints, parametricAmounts } from "./curve";
import type { LocalLayer } from "./local";
import { DEFAULT_OUTPUT, type OutputSettings } from "./output";
import type { Adjustments, HealSpot, WarpStroke } from "./protocol";

const EDITS_KEY = "openpixels.edits.v1";
const PRESETS_KEY = "openpixels.presets.v1";
const HEALS_KEY = "openpixels.heals.v1";
const WARPS_KEY = "openpixels.warps.v1";
const LOCALS_KEY = "openpixels.locals.v1";
const RATINGS_KEY = "openpixels.ratings.v1";
const FLAGS_KEY = "openpixels.flags.v1";
const OUTPUT_KEY = "openpixels.output.v1";
const LAST_PROJECT_KEY = "openpixels.lastProject.v1";

/** Editing aids that belong to the session, never to a saved edit. */
const SESSION_ONLY = new Set(["maskOverlay", "cropEditing", "localOverlay"]);
/** Settings whose untouched value is not 0, so a 0 must be stored. */
const ZERO_IS_A_CHOICE = new Set(["backgroundAmount", "skyAmount"]);

export type Presets = Record<string, Adjustments>;

function read<T>(key: string, fallback: T): T {
  try {
    const stored = localStorage.getItem(key);
    return stored ? (JSON.parse(stored) as T) : fallback;
  } catch {
    return fallback;
  }
}

function write(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Storage can be full or blocked; editing still works for this session.
  }
}

/** How the preview simulates an ICC profile. A viewing option, not part of any edit. */
export interface ProofSettings {
  on: boolean;
  profileId: number | null;
  perceptual: boolean;
  /** Shows the paper's white and the ink's black instead of the screen's. */
  paper: boolean;
  /** Marks colours the profile cannot reproduce. */
  gamut: boolean;
}

const PROOF_KEY = "openpixels.proof.v1";
const DEFAULT_PROOF: ProofSettings = { on: false, profileId: null, perceptual: true, paper: false, gamut: false };

export const loadProof = (): ProofSettings => ({ ...DEFAULT_PROOF, ...read<Partial<ProofSettings>>(PROOF_KEY, {}) });
export const saveProof = (settings: ProofSettings) => write(PROOF_KEY, settings);

/** Serialises edits for the engine, which takes the HSL bands as two groups of four. */
export function engineJson(adjustments: Adjustments): string {
  const bands = (prefix: string) =>
    [0, 4].map((start) => [0, 1, 2, 3].map((i) => adjustments[`${prefix}${start + i}`] ?? 0));
  // The engine rejects null, which is what NaN and Infinity become in JSON.
  const finite = (_key: string, value: unknown) => (typeof value === "number" && !Number.isFinite(value) ? 0 : value);
  return JSON.stringify({
    ...adjustments,
    hslHue: bands("hslHue"),
    hslSaturation: bands("hslSat"),
    hslLuminance: bands("hslLum"),
    curve: CURVE_CHANNELS.map((channel) => curvePoints(adjustments, channel).map((point) => [point.x, point.y])),
    parametric: parametricAmounts(adjustments),
    lensManual: [adjustments.lensDistortion ?? 0, adjustments.lensVignette ?? 0, adjustments.lensCaRed ?? 0, adjustments.lensCaBlue ?? 0],
    perspective: [adjustments.perspectiveV ?? 0, adjustments.perspectiveH ?? 0],
    replaceBackground: adjustments.backgroundAsset ? (adjustments.backgroundAmount ?? 1) : 0,
    replaceBackgroundExposure: adjustments.backgroundFillExposure ?? 0,
    replaceBackgroundShift: adjustments.backgroundShift ?? 0,
    replaceSky: adjustments.skyAsset ? (adjustments.skyAmount ?? 1) : 0,
    replaceSkyExposure: adjustments.skyFillExposure ?? 0,
    replaceSkyShift: adjustments.skyShift ?? 0,
    replaceSkyRelight: adjustments.skyRelight ?? 0,
    localRange: [0, 1, 2, 3].map((layer) => {
      const value = (name: string, fallback: number) => adjustments[`local${layer}Range${name}`] ?? fallback;
      const mode = value("Mode", 0);
      if (mode === 1) return [value("Low", 0), value("High", 1), 0, 0];
      if (mode === 2) return [0, 1, value("Hue", 0), value("Width", 30)];
      return [0, 1, 0, 0];
    }),
    grade: ["Shadow", "Mid", "High"].map((range) => [
      adjustments[`grade${range}Hue`] ?? 0,
      adjustments[`grade${range}Amount`] ?? 0,
      0,
      0,
    ]),
    local: [0, 1, 2, 3].map((layer) =>
      ["Exposure", "Contrast", "Saturation", "Temperature"].map((name) => adjustments[`local${layer}${name}`] ?? 0),
    ),
  }, finite);
}

/** The project to offer on the next visit. */
export const loadLastProject = () => read<number | null>(LAST_PROJECT_KEY, null);
export const saveLastProject = (id: number | null) => write(LAST_PROJECT_KEY, id);

/** Identifies a photo across sessions without reading its contents. */
export const fileId = (file: File) => `${file.name}|${file.size}|${file.lastModified}`;

/** Drops session-only fields and sliders left at their neutral value. */
export function persistable(adjustments: Adjustments): Adjustments {
  return Object.fromEntries(
    Object.entries(adjustments).filter(
      ([key, value]) => !SESSION_ONLY.has(key) && Number.isFinite(value) && (value !== 0 || ZERO_IS_A_CHOICE.has(key)),
    ),
  );
}

export const loadEdit = (id: string): Adjustments => read<Record<string, Adjustments>>(EDITS_KEY, {})[id] ?? {};

export function saveEdits(edits: Record<string, Adjustments>) {
  const all = read<Record<string, Adjustments>>(EDITS_KEY, {});
  for (const [id, adjustments] of Object.entries(edits)) {
    const kept = persistable(adjustments);
    if (Object.keys(kept).length > 0) all[id] = kept;
    else delete all[id];
  }
  write(EDITS_KEY, all);
}

export const loadHeals = (id: string): HealSpot[] => read<Record<string, HealSpot[]>>(HEALS_KEY, {})[id] ?? [];

export function saveHeals(id: string, heals: HealSpot[]) {
  const all = read<Record<string, HealSpot[]>>(HEALS_KEY, {});
  if (heals.length > 0) all[id] = heals;
  else delete all[id];
  write(HEALS_KEY, all);
}

/** Liquify strokes, grouped by the drag that made them so undo removes a whole drag. */
export const loadWarps = (id: string): WarpStroke[][] => read<Record<string, WarpStroke[][]>>(WARPS_KEY, {})[id] ?? [];

export function saveWarps(id: string, groups: WarpStroke[][]) {
  const all = read<Record<string, WarpStroke[][]>>(WARPS_KEY, {});
  if (groups.length > 0) all[id] = groups;
  else delete all[id];
  write(WARPS_KEY, all);
}

export const loadLocals = (id: string): LocalLayer[] => read<Record<string, LocalLayer[]>>(LOCALS_KEY, {})[id] ?? [];

export function saveLocals(id: string, layers: LocalLayer[]) {
  const all = read<Record<string, LocalLayer[]>>(LOCALS_KEY, {});
  if (layers.length > 0) all[id] = layers;
  else delete all[id];
  write(LOCALS_KEY, all);
}

export const loadRatings = (): Record<string, number> => read<Record<string, number>>(RATINGS_KEY, {});

export function saveRating(id: string, rating: number) {
  const all = loadRatings();
  if (rating > 0) all[id] = rating;
  else delete all[id];
  write(RATINGS_KEY, all);
}

/** 1 for a picked photo, -1 for a rejected one. */
const LABELS_KEY = "openpixels.labels.v1";
const SNAPSHOTS_KEY = "openpixels.snapshots.v1";
const OUTPUT_PRESETS_KEY = "openpixels.outputPresets.v1";

/** A colour label's index into `COLOR_LABELS`, plus one. */
export const loadLabels = (): Record<string, number> => read<Record<string, number>>(LABELS_KEY, {});

export function saveLabel(id: string, label: number) {
  const all = loadLabels();
  if (label !== 0) all[id] = label;
  else delete all[id];
  write(LABELS_KEY, all);
}

export interface Snapshot {
  name: string;
  edit: Adjustments;
}

export const loadSnapshots = (id: string): Snapshot[] => read<Record<string, Snapshot[]>>(SNAPSHOTS_KEY, {})[id] ?? [];

export function saveSnapshots(id: string, snapshots: Snapshot[]) {
  const all = read<Record<string, Snapshot[]>>(SNAPSHOTS_KEY, {});
  if (snapshots.length > 0) all[id] = snapshots;
  else delete all[id];
  write(SNAPSHOTS_KEY, all);
}

export const loadOutputPresets = (): Record<string, OutputSettings> => read<Record<string, OutputSettings>>(OUTPUT_PRESETS_KEY, {});
export const saveOutputPresets = (presets: Record<string, OutputSettings>) => write(OUTPUT_PRESETS_KEY, presets);

export const loadFlags = (): Record<string, number> => read<Record<string, number>>(FLAGS_KEY, {});

export function saveFlag(id: string, flag: number) {
  const all = loadFlags();
  if (flag !== 0) all[id] = flag;
  else delete all[id];
  write(FLAGS_KEY, all);
}

export const loadOutput = (): OutputSettings => {
  const stored = read<Partial<OutputSettings>>(OUTPUT_KEY, {});
  return { ...DEFAULT_OUTPUT, ...stored, watermark: { ...DEFAULT_OUTPUT.watermark, ...stored.watermark } };
};
export const saveOutput = (settings: OutputSettings) => write(OUTPUT_KEY, settings);

export const loadPresets = (): Presets => read<Presets>(PRESETS_KEY, {});
export const savePresets = (presets: Presets) => write(PRESETS_KEY, presets);

const FORBIDDEN_IN_NAMES = /[\\/:*?"<>|]/g;

/** Output name from a template, made unique among names already in `used`. */
export function exportName(sourceName: string, used: Set<string>, template = "{name}", index = 1, extension = "jpg"): string {
  const stem = sourceName.replace(/\.[^.]*$/, "") || "photo";
  const today = new Date();
  const date = `${today.getFullYear()}${String(today.getMonth() + 1).padStart(2, "0")}${String(today.getDate()).padStart(2, "0")}`;
  const filled = template
    .replaceAll("{name}", stem)
    .replaceAll("{n}", String(index).padStart(3, "0"))
    .replaceAll("{date}", date)
    .replace(FORBIDDEN_IN_NAMES, "_")
    .trim();
  const base = filled || stem;
  let name = `${base}.${extension}`;
  for (let copy = 2; used.has(name.toLowerCase()); copy++) name = `${base}-${copy}.${extension}`;
  used.add(name.toLowerCase());
  return name;
}
