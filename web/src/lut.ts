import { t } from "./i18n";

/** A 3D colour lookup table: `size`^3 RGB triples in 0..1, with red varying fastest. */
export interface Lut {
  size: number;
  data: Float32Array<ArrayBuffer>;
}

const BUILT_IN_SIZE = 25;

type Grade = (r: number, g: number, b: number) => [number, number, number];

const luma = (r: number, g: number, b: number) => 0.2126 * r + 0.7152 * g + 0.0722 * b;
const scurve = (v: number, amount: number) => v + amount * (v - 0.5) * (1 - Math.abs(2 * v - 1)) * 2;

function generate(grade: Grade): Lut {
  const size = BUILT_IN_SIZE;
  const data = new Float32Array(size * size * size * 3);
  let at = 0;
  for (let b = 0; b < size; b++) {
    for (let g = 0; g < size; g++) {
      for (let r = 0; r < size; r++) {
        const out = grade(r / (size - 1), g / (size - 1), b / (size - 1));
        for (const value of out) data[at++] = Math.min(Math.max(value, 0), 1);
      }
    }
  }
  return { size, data };
}

const BASIC_GROUP = t("基础");

type Rgb = [number, number, number];

/** A change to one range of hues, weighted down towards grey. */
interface HueBand {
  hue: number;
  width: number;
  /** Degrees to rotate towards. */
  shift?: number;
  saturation?: number;
  value?: number;
}

/** A look described by its colour and tone character, rather than a stored table. */
interface Recipe {
  gains?: Rgb;
  bands?: HueBand[];
  saturation?: number;
  contrast?: number;
  /** Lifts black. */
  fade?: number;
  /** Lowers white. */
  roll?: number;
  /** Added to shadows and to highlights respectively. */
  shadows?: Rgb;
  highlights?: Rgb;
  /** Channel mix for black and white. */
  mono?: Rgb;
}

function toHsv(r: number, g: number, b: number): Rgb {
  const peak = Math.max(r, g, b);
  const span = peak - Math.min(r, g, b);
  if (span === 0) return [0, 0, peak];
  const sector = peak === r ? ((g - b) / span + 6) % 6 : peak === g ? (b - r) / span + 2 : (r - g) / span + 4;
  return [sector * 60, span / peak, peak];
}

function fromHsv(h: number, s: number, v: number): Rgb {
  const channel = (n: number) => {
    const k = (n + h / 60) % 6;
    return v - v * s * Math.max(0, Math.min(k, 4 - k, 1));
  };
  return [channel(5), channel(3), channel(1)];
}

function applyBands([r, g, b]: Rgb, bands: HueBand[]): Rgb {
  let [h, s, v] = toHsv(r, g, b);
  const [originalHue, neutral] = [h, Math.min(Math.max((s - 0.05) / 0.2, 0), 1)];
  for (const band of bands) {
    const away = Math.abs(((originalHue - band.hue + 540) % 360) - 180) / band.width;
    if (away >= 1) continue;
    const weight = (0.5 + 0.5 * Math.cos(Math.PI * away)) * neutral;
    h += (band.shift ?? 0) * weight;
    s *= 1 + (band.saturation ?? 0) * weight;
    v *= 1 + (band.value ?? 0) * weight;
  }
  return fromHsv((h + 360) % 360, Math.min(s, 1), v);
}

/** Where a channel brightened by a gain starts to bend so its top still reaches 1. */
const GAIN_KNEE = 0.75;

/**
 * Multiplies by `gain`, rolling off above the knee so values that would pass 1 keep
 * their order instead of clipping. The roll-off starts at slope 1 and ends at 1, and
 * stays rising for gains up to 1 + 1 - GAIN_KNEE (1.25).
 */
export function gained(value: number, gain: number): number {
  const scaled = value * gain;
  if (gain <= 1 || scaled <= GAIN_KNEE) return scaled;
  const t = Math.min((scaled - GAIN_KNEE) / (gain - GAIN_KNEE), 1);
  const startSlope = (gain - GAIN_KNEE) / (1 - GAIN_KNEE);
  return GAIN_KNEE + (1 - GAIN_KNEE) * (startSlope * t + (1 - startSlope) * t * t);
}

/** Brings a channel that can reach `top` back under 1 with the same roll-off as `gained`. */
const ceiling = (value: number, top: number) => gained(value / top, top);

function fromRecipe(recipe: Recipe): Grade {
  const { gains = [1, 1, 1], bands = [], saturation = 1, contrast = 0, fade = 0, roll = 0, shadows = [0, 0, 0], highlights = [0, 0, 0], mono } = recipe;
  return (red, green, blue) => {
    let rgb: Rgb = [gained(red, gains[0]), gained(green, gains[1]), gained(blue, gains[2])];
    if (mono) {
      const y = rgb[0] * mono[0] + rgb[1] * mono[1] + rgb[2] * mono[2];
      rgb = [y, y, y];
    } else {
      if (bands.length) rgb = applyBands(rgb, bands);
      const y = luma(...rgb);
      rgb = rgb.map((v) => y + (v - y) * saturation) as Rgb;
    }
    const y = Math.min(Math.max(luma(...rgb), 0), 1);
    return rgb.map((v, i) => {
      const toned = fade + scurve(Math.min(Math.max(v, 0), 1), contrast) * (1 - fade - roll);
      return toned + shadows[i] * (1 - y) * (1 - y) + highlights[i] * y * y;
    }) as Rgb;
  };
}

const RECIPES: { id: number; group: string; name: string; recipe: Recipe }[] = [
  { id: 6, group: t("胶片"), name: t("标准正片"), recipe: { contrast: 0.1, saturation: 1.08, bands: [{ hue: 120, width: 60, shift: -6 }] } },
  {
    id: 7,
    group: t("胶片"),
    name: t("鲜艳正片"),
    recipe: {
      contrast: 0.24,
      saturation: 1.32,
      bands: [
        { hue: 120, width: 70, shift: 10, value: -0.08 },
        { hue: 220, width: 60, saturation: 0.15, value: -0.08 },
        { hue: 0, width: 35, saturation: 0.1 },
      ],
    },
  },
  {
    id: 8,
    group: t("胶片"),
    name: t("经典反转"),
    recipe: {
      contrast: 0.16,
      saturation: 0.78,
      shadows: [-0.015, 0.01, 0.02],
      highlights: [0.01, 0.005, -0.015],
      bands: [
        { hue: 10, width: 40, shift: 8, saturation: -0.1 },
        { hue: 210, width: 60, saturation: -0.2 },
        { hue: 100, width: 60, shift: -12, saturation: -0.15 },
      ],
    },
  },
  { id: 9, group: t("胶片"), name: t("柔和负片"), recipe: { contrast: -0.06, fade: 0.015, saturation: 0.92, gains: [1.01, 1, 0.99], bands: [{ hue: 25, width: 35, value: 0.03 }] } },
  {
    id: 10,
    group: t("胶片"),
    name: t("人像暖调负片"),
    recipe: {
      contrast: 0.03,
      fade: 0.025,
      roll: 0.015,
      saturation: 0.95,
      gains: [1.03, 1, 0.95],
      bands: [
        { hue: 25, width: 35, shift: -3, saturation: -0.05, value: 0.05 },
        { hue: 110, width: 60, shift: -15, saturation: -0.25 },
      ],
      shadows: [0.012, 0, -0.008],
    },
  },
  { id: 11, group: t("胶片"), name: t("金色负片"), recipe: { contrast: 0.12, saturation: 1.14, gains: [1.05, 1, 0.9], fade: 0.01, bands: [{ hue: 45, width: 40, saturation: 0.15 }] } },
  {
    id: 12,
    group: t("胶片"),
    name: t("拍立得"),
    recipe: { contrast: -0.08, fade: 0.07, roll: 0.05, saturation: 0.86, gains: [1.03, 1, 0.94], shadows: [0, 0.015, 0.03], highlights: [0.015, 0.01, -0.02] },
  },
  {
    id: 13,
    group: t("电影"),
    name: t("低饱和电影"),
    recipe: { contrast: -0.1, fade: 0.025, roll: 0.02, saturation: 0.72, shadows: [-0.01, 0.012, 0.02], bands: [{ hue: 110, width: 60, saturation: -0.2 }] },
  },
  {
    id: 14,
    group: t("电影"),
    name: t("钨丝灯夜景"),
    recipe: {
      contrast: 0.14,
      saturation: 1.05,
      gains: [0.92, 0.98, 1.1],
      shadows: [-0.02, 0.005, 0.045],
      highlights: [0.05, 0.005, -0.02],
      bands: [{ hue: 0, width: 40, saturation: 0.2 }],
    },
  },
  { id: 15, group: t("电影"), name: t("褪银"), recipe: { contrast: 0.32, saturation: 0.5, roll: 0.01 } },
  {
    id: 16,
    group: t("电影"),
    name: t("末日橙"),
    recipe: {
      contrast: 0.26,
      saturation: 1.05,
      gains: [1.05, 0.99, 0.88],
      shadows: [-0.01, 0.015, 0.03],
      bands: [
        { hue: 200, width: 60, saturation: -0.35 },
        { hue: 100, width: 60, shift: -35, saturation: -0.4 },
      ],
    },
  },
  {
    id: 17,
    group: t("创意"),
    name: t("赛博朋克"),
    recipe: {
      contrast: 0.2,
      saturation: 1.18,
      gains: [1, 0.98, 1.02],
      shadows: [0.015, -0.02, 0.07],
      bands: [
        { hue: 30, width: 55, shift: -50, saturation: 0.1 },
        { hue: 120, width: 65, shift: 55 },
        { hue: 215, width: 55, shift: -22, saturation: 0.15 },
        { hue: 340, width: 35, shift: -10, saturation: 0.2 },
      ],
    },
  },
  {
    id: 18,
    group: t("创意"),
    name: t("霓虹夜色"),
    recipe: {
      contrast: 0.18,
      saturation: 1.2,
      gains: [0.98, 0.97, 1.05],
      shadows: [0.01, -0.02, 0.08],
      bands: [
        { hue: 30, width: 55, shift: -22 },
        { hue: 210, width: 65, shift: 25, saturation: 0.2 },
      ],
    },
  },
  {
    id: 19,
    group: t("创意"),
    name: t("港风复古"),
    recipe: {
      contrast: 0.12,
      fade: 0.03,
      saturation: 0.92,
      shadows: [-0.015, 0.04, 0.012],
      highlights: [0.04, 0.005, -0.03],
      bands: [
        { hue: 0, width: 35, saturation: 0.15 },
        { hue: 120, width: 70, shift: 18 },
      ],
    },
  },
  {
    id: 20,
    group: t("创意"),
    name: t("日系清新"),
    recipe: {
      contrast: -0.12,
      fade: 0.06,
      saturation: 0.86,
      gains: [0.99, 1.02, 1.03],
      shadows: [0, 0.015, 0.025],
      bands: [
        { hue: 110, width: 60, shift: 15, saturation: -0.1, value: 0.06 },
        { hue: 25, width: 35, value: 0.05 },
      ],
    },
  },
  {
    id: 21,
    group: t("创意"),
    name: t("交叉冲洗"),
    recipe: { contrast: 0.26, saturation: 1.15, shadows: [-0.02, 0.01, 0.1], highlights: [0.03, 0.04, -0.07], bands: [{ hue: 120, width: 70, shift: -15 }] },
  },
  { id: 22, group: t("黑白"), name: t("细腻黑白"), recipe: { mono: [0.25, 0.65, 0.1], contrast: 0.1 } },
  { id: 23, group: t("黑白"), name: t("高反差黑白"), recipe: { mono: [0.4, 0.5, 0.1], contrast: 0.34 } },
  { id: 24, group: t("黑白"), name: t("复古棕调"), recipe: { mono: [0.3, 0.6, 0.1], contrast: 0.08, fade: 0.04, shadows: [0.03, 0.01, -0.03], highlights: [0.03, 0.005, -0.05] } },
];

/** Looks made for this project, addressed by `lutId`. */
export const BUILT_IN_LOOKS: { id: number; group: string; name: string; grade: Grade }[] = [
  {
    id: 1,
    group: BASIC_GROUP,
    name: t("暖调胶片"),
    grade: (r, g, b) => [ceiling(scurve(r, 0.12) * 1.04 + 0.015, 1.055), ceiling(scurve(g, 0.1) + 0.005, 1.005), scurve(b, 0.08) * 0.92 + 0.03],
  },
  {
    id: 2,
    group: BASIC_GROUP,
    name: t("清冷"),
    grade: (r, g, b) => [scurve(r, 0.08) * 0.95, scurve(g, 0.08) * 0.99, ceiling(scurve(b, 0.06) * 1.05 + 0.01, 1.06)],
  },
  {
    id: 3,
    group: BASIC_GROUP,
    name: t("青橙"),
    grade: (r, g, b) => {
      const y = luma(r, g, b);
      const warm = y;
      const cool = 1 - y;
      return [ceiling(r + 0.07 * warm - 0.06 * cool, 1.07), ceiling(g + 0.015 * warm + 0.015 * cool, 1.015), ceiling(b - 0.08 * warm + 0.07 * cool, 1.07)];
    },
  },
  {
    id: 4,
    group: BASIC_GROUP,
    name: t("褪色"),
    grade: (r, g, b) => {
      const lift = (v: number) => 0.08 + v * 0.86;
      const y = luma(r, g, b);
      const soften = (v: number) => y + (v - y) * 0.82;
      return [lift(soften(r)), lift(soften(g)), lift(soften(b))];
    },
  },
  {
    id: 5,
    group: BASIC_GROUP,
    name: t("黑白"),
    grade: (r, g, b) => {
      const y = scurve(0.3 * r + 0.6 * g + 0.1 * b, 0.15);
      return [y, y, y];
    },
  },
  ...RECIPES.map(({ recipe, ...look }) => ({ ...look, grade: fromRecipe(recipe) })),
];

export const LOOK_GROUPS = [BASIC_GROUP, ...new Set(RECIPES.map((look) => look.group))];

const builtInCache = new Map<number, Lut>();

export function builtInLut(id: number): Lut | null {
  const look = BUILT_IN_LOOKS.find((item) => item.id === id);
  if (!look) return null;
  let lut = builtInCache.get(id);
  if (!lut) {
    lut = generate(look.grade);
    builtInCache.set(id, lut);
  }
  return lut;
}

/** Parses an Adobe/IRIDAS `.cube` 3D LUT. Throws with a readable reason if it cannot. */
export function parseCube(text: string): Lut {
  let size = 0;
  const values: number[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith("#") || line.startsWith("TITLE")) continue;
    if (line.startsWith("LUT_1D_SIZE")) throw new Error(t("只支持 3D LUT，这是 1D LUT"));
    if (line.startsWith("DOMAIN_MIN") || line.startsWith("DOMAIN_MAX")) {
      const wanted = line.startsWith("DOMAIN_MIN") ? 0 : 1;
      if (line.split(/\s+/).slice(1).some((value) => Number(value) !== wanted)) throw new Error(t("只支持定义域为 0–1 的 LUT"));
      continue;
    }
    if (line.startsWith("LUT_3D_SIZE")) {
      size = Number(line.split(/\s+/)[1]);
      continue;
    }
    if (/^[A-Z]/.test(line)) continue;
    const parts = line.split(/\s+/).map(Number);
    if (parts.length === 3 && parts.every(Number.isFinite)) values.push(...parts);
  }
  if (!Number.isInteger(size) || size < 2 || size > 129) throw new Error(t("没有找到有效的 LUT_3D_SIZE"));
  if (values.length !== size * size * size * 3) {
    throw new Error(t("LUT 数据不完整：应有 {expected} 行，实际 {actual} 行", { expected: size ** 3, actual: values.length / 3 }));
  }
  return { size, data: Float32Array.from(values, (value) => Math.min(Math.max(value, 0), 1)) };
}
