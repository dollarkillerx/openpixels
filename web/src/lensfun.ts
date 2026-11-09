/** Looks up lens corrections in the Lensfun database (CC BY-SA 3.0; see NOTICE.txt beside it). */
import type { LensQuery } from "./protocol";

type Sample = [number, string, ...number[]];

interface Lens {
  names: string[];
  crop: number;
  aspect: number;
  distortion: Sample[];
  tca: Sample[];
  /** focal, aperture, distance, k1, k2, k3 */
  vignetting: number[][];
}

interface Database {
  cameras: Record<string, number>;
  lenses: Lens[];
}

const DISTORTION_PARAMETERS: Record<string, string[]> = { ptlens: ["a", "b", "c"], poly3: ["k1"], poly5: ["k1", "k2"] };
const TCA_PARAMETERS: Record<string, string[]> = { linear: ["kr", "kb"], poly3: ["vr", "vb", "cr", "cb", "br", "bb"] };
/** Tolerates a rounded crop factor when checking that a calibration covers the frame. */
const CROP_TOLERANCE = 1.03;

let database: Promise<Database | null> | null = null;

function loadDatabase() {
  database ??= fetch("/lensfun/lensfun.json")
    .then((response) => (response.ok ? (response.json() as Promise<Database>) : null))
    .catch(() => null);
  return database;
}

/** Must match `normalize` in scripts/build-lensfun.mjs. */
const normalize = (text: string) => text.toLowerCase().replace(/[^a-z0-9.]/g, "");

/** The two values bracketing `at` in sorted `values`, and how far `at` is between them. */
function bracket(values: number[], at: number): [number, number, number] {
  if (at <= values[0]) return [values[0], values[0], 0];
  const last = values[values.length - 1];
  if (at >= last) return [last, last, 0];
  const upper = values.findIndex((value) => value >= at);
  const [low, high] = [values[upper - 1], values[upper]];
  return [low, high, (at - low) / (high - low)];
}

const unique = (values: number[]) => [...new Set(values)].sort((a, b) => a - b);

/** Interpolates by focal length, between samples of the model nearest to it. */
function atFocal(samples: Sample[], focal: number, parameters: Record<string, string[]>) {
  if (samples.length === 0) return null;
  const nearest = samples.reduce((best, sample) => (Math.abs(sample[0] - focal) < Math.abs(best[0] - focal) ? sample : best));
  const model = nearest[1];
  const same = samples.filter((sample) => sample[1] === model);
  const [low, high, t] = bracket(unique(same.map((sample) => sample[0])), focal);
  const lower = same.find((sample) => sample[0] === low)!;
  const upper = same.find((sample) => sample[0] === high)!;
  const result: Record<string, string | number> = { model };
  parameters[model].forEach((name, index) => {
    const a = lower[index + 2] as number;
    result[name] = a + ((upper[index + 2] as number) - a) * t;
  });
  return result;
}

/** At the farthest calibrated distance, as a landscape-style default. */
function vignettingAt(samples: number[][], focal: number, aperture: number) {
  if (samples.length === 0) return null;
  const farthest = new Map<string, number[]>();
  for (const sample of samples) {
    const key = `${sample[0]}/${sample[1]}`;
    const known = farthest.get(key);
    if (!known || sample[2] > known[2]) farthest.set(key, sample);
  }
  const usable = [...farthest.values()];
  const atFocalLength = (value: number) => {
    const here = usable.filter((sample) => sample[0] === value);
    // Falloff changes about evenly per stop, so interpolate in stops.
    const stops = unique(here.map((sample) => Math.log2(sample[1])));
    const [low, high, t] = bracket(stops, Math.log2(aperture));
    const find = (stop: number) => here.find((sample) => Math.log2(sample[1]) === stop)!;
    const [a, b] = [find(low), find(high)];
    return [3, 4, 5].map((index) => a[index] + (b[index] - a[index]) * t);
  };
  const [low, high, t] = bracket(unique(usable.map((sample) => sample[0])), focal);
  const [a, b] = [atFocalLength(low), atFocalLength(high)];
  const [k1, k2, k3] = a.map((value, index) => value + (b[index] - value) * t);
  return { k1, k2, k3 };
}

/** The corrections for a shot as `LensfunCalibration` in op-core, or null if unknown. */
export async function lensfunCalibration(query: LensQuery) {
  const db = await loadDatabase();
  if (!db) return null;
  // Exact names only: a near match would silently apply another lens's corrections.
  const name = normalize(query.lens);
  const entries = db.lenses.filter((entry) => entry.names.includes(name));
  if (entries.length === 0) return null;
  const camera = db.cameras[normalize(query.camera)];
  // A lens can be calibrated on several sensors. One smaller than this camera's would
  // leave the corners unmeasured, so use the largest that is not.
  const crop = camera ?? Math.min(...entries.map((entry) => entry.crop));
  const lens = entries
    .filter((entry) => entry.crop <= crop * CROP_TOLERANCE)
    .sort((a, b) => b.crop - a.crop)[0];
  if (!lens) return null;

  const distortion = atFocal(lens.distortion, query.focal, DISTORTION_PARAMETERS);
  const tca = atFocal(lens.tca, query.focal, TCA_PARAMETERS);
  const vignetting = query.aperture ? vignettingAt(lens.vignetting, query.focal, query.aperture) : null;
  if (!distortion && !tca && !vignetting) return null;
  const relative = lens.crop / crop;
  return {
    distortion,
    tca,
    vignetting,
    huginScale: relative * Math.hypot(lens.aspect, 1),
    cornerScale: relative,
  };
}
