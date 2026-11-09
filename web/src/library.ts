import type { LocalLayer } from "./local";
import type { Adjustments, HealSpot, WarpStroke } from "./protocol";

const EDITS_KEY = "openpixels.edits.v1";
const PRESETS_KEY = "openpixels.presets.v1";
const HEALS_KEY = "openpixels.heals.v1";
const WARPS_KEY = "openpixels.warps.v1";
const LOCALS_KEY = "openpixels.locals.v1";

/** Editing aids that belong to the session, never to a saved edit. */
const SESSION_ONLY = new Set(["maskOverlay", "cropEditing"]);

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

/** Serialises edits for the engine, which takes the HSL bands as two groups of four. */
export function engineJson(adjustments: Adjustments): string {
  const bands = (prefix: string) =>
    [0, 4].map((start) => [0, 1, 2, 3].map((i) => adjustments[`${prefix}${start + i}`] ?? 0));
  return JSON.stringify({
    ...adjustments,
    hslHue: bands("hslHue"),
    hslSaturation: bands("hslSat"),
    hslLuminance: bands("hslLum"),
    local: [0, 1, 2, 3].map((layer) =>
      ["Exposure", "Contrast", "Saturation", "Temperature"].map((name) => adjustments[`local${layer}${name}`] ?? 0),
    ),
  });
}

/** Identifies a photo across sessions without reading its contents. */
export const fileId = (file: File) => `${file.name}|${file.size}|${file.lastModified}`;

/** Drops session-only fields and sliders left at their neutral value. */
export function persistable(adjustments: Adjustments): Adjustments {
  return Object.fromEntries(
    Object.entries(adjustments).filter(([key, value]) => !SESSION_ONLY.has(key) && value !== 0),
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

export const loadPresets = (): Presets => read<Presets>(PRESETS_KEY, {});
export const savePresets = (presets: Presets) => write(PRESETS_KEY, presets);

/** Output name for `sourceName`, made unique among names already in `used`. */
export function exportName(sourceName: string, used: Set<string>): string {
  const base = sourceName.replace(/\.[^.]*$/, "") || "photo";
  let name = `${base}.jpg`;
  for (let copy = 2; used.has(name.toLowerCase()); copy++) name = `${base}-${copy}.jpg`;
  used.add(name.toLowerCase());
  return name;
}
