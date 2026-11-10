/**
 * Colour matching: builds a look that gives one image the tonal distribution and colour
 * balance of another. Statistical, with no model involved.
 */
import type { Lut } from "./lut";
import { t } from "./i18n";

const LUT_SIZE = 33;
const LUMINANCE_POINTS = 33;
/** Tonal bands in which the colour balance is measured separately. */
const CHROMA_BANDS = 5;
/** Keeps a very flat or very vivid reference from pushing colours to extremes. */
const CHROMA_GAIN_LIMITS = [0.6, 1.5];
const MAX_CHROMA_SHIFT = 40;
/** How far tones follow the reference: 1 would copy its contrast exactly. */
const TONE_STRENGTH = 0.85;
/** Clipped pixels say little about the look. */
const USABLE_L = [1, 99];

type Lab = [number, number, number];

const toLinear = (v: number) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
const toSrgb = (v: number) => (v <= 0.0031308 ? v * 12.92 : 1.055 * v ** (1 / 2.4) - 0.055);
const LINEAR_BYTE = Float32Array.from({ length: 256 }, (_, i) => toLinear(i / 255));

const WHITE = [0.95047, 1, 1.08883];
const f = (t: number) => (t > 216 / 24389 ? Math.cbrt(t) : (841 / 108) * t + 4 / 29);
const fInverse = (t: number) => (t > 6 / 29 ? t * t * t : (108 / 841) * (t - 4 / 29));

function linearToLab(r: number, g: number, b: number): Lab {
  const x = f((0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / WHITE[0]);
  const y = f(0.2126729 * r + 0.7151522 * g + 0.072175 * b);
  const z = f((0.0193339 * r + 0.119192 * g + 0.9503041 * b) / WHITE[2]);
  return [116 * y - 16, 500 * (x - y), 200 * (y - z)];
}

function labToLinear([l, a, b]: Lab): [number, number, number] {
  const y = (l + 16) / 116;
  const X = WHITE[0] * fInverse(y + a / 500);
  const Y = fInverse(y);
  const Z = WHITE[2] * fInverse(y - b / 200);
  return [
    3.2404542 * X - 1.5371385 * Y - 0.4985314 * Z,
    -0.969266 * X + 1.8760108 * Y + 0.041556 * Z,
    0.0556434 * X - 0.2040259 * Y + 1.0572252 * Z,
  ];
}

interface Statistics {
  /** Luminance at evenly spaced percentiles. */
  quantiles: Float32Array;
  /** Mean a and b for each tonal band. */
  mean: [number, number][];
  deviation: [number, number];
}

/** How much a pixel of lightness `l` belongs to tonal band `band`. */
function bandWeight(l: number, band: number) {
  const centre = (band / (CHROMA_BANDS - 1)) * 100;
  return Math.max(0, 1 - Math.abs(l - centre) / (100 / (CHROMA_BANDS - 1)));
}

function measure(rgb: Uint8Array | Uint8ClampedArray, step: number): Statistics {
  const lightness: number[] = [];
  const total = Array.from({ length: CHROMA_BANDS }, () => [0, 0, 0]);
  let [sumA, sumB, sumAA, sumBB, count] = [0, 0, 0, 0, 0];
  for (let at = 0; at + 2 < rgb.length; at += step) {
    const [l, a, b] = linearToLab(LINEAR_BYTE[rgb[at]], LINEAR_BYTE[rgb[at + 1]], LINEAR_BYTE[rgb[at + 2]]);
    if (l < USABLE_L[0] || l > USABLE_L[1]) continue;
    lightness.push(l);
    sumA += a;
    sumB += b;
    sumAA += a * a;
    sumBB += b * b;
    count++;
    for (let band = 0; band < CHROMA_BANDS; band++) {
      const weight = bandWeight(l, band);
      total[band][0] += a * weight;
      total[band][1] += b * weight;
      total[band][2] += weight;
    }
  }
  if (count === 0) throw new Error(t("这张图几乎全是纯黑或纯白，无法分析色彩"));
  lightness.sort((x, y) => x - y);
  const quantiles = Float32Array.from({ length: LUMINANCE_POINTS }, (_, i) =>
    lightness[Math.min(count - 1, Math.round((i / (LUMINANCE_POINTS - 1)) * (count - 1)))],
  );
  const [meanA, meanB] = [sumA / count, sumB / count];
  const spread = (sum: number, mean: number) => Math.sqrt(Math.max(1e-6, sum / count - mean * mean));
  return {
    quantiles,
    // A band with almost no pixels falls back to the overall balance.
    mean: total.map(([a, b, weight]) => (weight > count * 0.01 ? [a / weight, b / weight] : [meanA, meanB])),
    deviation: [spread(sumAA, meanA), spread(sumBB, meanB)],
  };
}

/** Piecewise-linear mapping from one image's tones to another's, kept increasing. */
function toneCurve(from: Float32Array, to: Float32Array) {
  const xs = [0, ...from, 100];
  const ys = [0, ...to, 100];
  for (let i = 1; i < xs.length; i++) {
    xs[i] = Math.max(xs[i], xs[i - 1] + 1e-3);
    ys[i] = Math.max(ys[i], ys[i - 1]);
  }
  return (l: number) => {
    let i = 1;
    while (i < xs.length - 1 && xs[i] < l) i++;
    const t = Math.min(1, Math.max(0, (l - xs[i - 1]) / (xs[i] - xs[i - 1])));
    return l + (ys[i - 1] + (ys[i] - ys[i - 1]) * t - l) * TONE_STRENGTH;
  };
}

const bandMean = (mean: [number, number][], l: number, channel: 0 | 1) => {
  let [sum, weights] = [0, 0];
  for (let band = 0; band < CHROMA_BANDS; band++) {
    const weight = bandWeight(l, band);
    sum += mean[band][channel] * weight;
    weights += weight;
  }
  return weights > 0 ? sum / weights : 0;
};

const clamp = (value: number, low: number, high: number) => Math.min(high, Math.max(low, value));

/**
 * Builds a look that makes `source` resemble `reference`. Both are tightly packed
 * pixels with `sourceStep` and `referenceStep` bytes each, developed but without a look.
 */
export function matchColours(source: Uint8Array, sourceStep: number, reference: Uint8Array | Uint8ClampedArray, referenceStep: number): Lut {
  const from = measure(source, sourceStep);
  const to = measure(reference, referenceStep);
  const curve = toneCurve(from.quantiles, to.quantiles);
  const gain = [0, 1].map((c) => clamp(to.deviation[c] / from.deviation[c], CHROMA_GAIN_LIMITS[0], CHROMA_GAIN_LIMITS[1]));

  const data = new Float32Array(LUT_SIZE * LUT_SIZE * LUT_SIZE * 3);
  let at = 0;
  for (let b = 0; b < LUT_SIZE; b++) {
    for (let g = 0; g < LUT_SIZE; g++) {
      for (let r = 0; r < LUT_SIZE; r++) {
        const [l, labA, labB] = linearToLab(toLinear(r / (LUT_SIZE - 1)), toLinear(g / (LUT_SIZE - 1)), toLinear(b / (LUT_SIZE - 1)));
        const outL = clamp(curve(l), 0, 100);
        const move = (value: number, channel: 0 | 1) => {
          const sourceMean = bandMean(from.mean, l, channel);
          const shift = clamp(bandMean(to.mean, outL, channel) - sourceMean, -MAX_CHROMA_SHIFT, MAX_CHROMA_SHIFT);
          return sourceMean + shift + (value - sourceMean) * gain[channel];
        };
        for (const value of labToLinear([outL, move(labA, 0), move(labB, 1)])) {
          data[at++] = clamp(toSrgb(clamp(value, 0, 1)), 0, 1);
        }
      }
    }
  }
  return { size: LUT_SIZE, data };
}
