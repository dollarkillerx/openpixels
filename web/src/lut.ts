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

/** Looks made for this project, addressed by `lutId`. */
export const BUILT_IN_LOOKS: { id: number; name: string; grade: Grade }[] = [
  {
    id: 1,
    name: "暖调胶片",
    grade: (r, g, b) => [scurve(r, 0.12) * 1.04 + 0.015, scurve(g, 0.1) + 0.005, scurve(b, 0.08) * 0.92 + 0.03],
  },
  {
    id: 2,
    name: "清冷",
    grade: (r, g, b) => [scurve(r, 0.08) * 0.95, scurve(g, 0.08) * 0.99, scurve(b, 0.06) * 1.05 + 0.01],
  },
  {
    id: 3,
    name: "青橙",
    grade: (r, g, b) => {
      const y = luma(r, g, b);
      const warm = y;
      const cool = 1 - y;
      return [r + 0.07 * warm - 0.06 * cool, g + 0.015 * warm + 0.015 * cool, b - 0.08 * warm + 0.07 * cool];
    },
  },
  {
    id: 4,
    name: "褪色",
    grade: (r, g, b) => {
      const lift = (v: number) => 0.08 + v * 0.86;
      const y = luma(r, g, b);
      const soften = (v: number) => y + (v - y) * 0.82;
      return [lift(soften(r)), lift(soften(g)), lift(soften(b))];
    },
  },
  {
    id: 5,
    name: "黑白",
    grade: (r, g, b) => {
      const y = scurve(0.3 * r + 0.6 * g + 0.1 * b, 0.15);
      return [y, y, y];
    },
  },
];

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
    if (line.startsWith("LUT_1D_SIZE")) throw new Error("只支持 3D LUT，这是 1D LUT");
    if (line.startsWith("LUT_3D_SIZE")) {
      size = Number(line.split(/\s+/)[1]);
      continue;
    }
    if (/^[A-Z]/.test(line)) continue;
    const parts = line.split(/\s+/).map(Number);
    if (parts.length === 3 && parts.every(Number.isFinite)) values.push(...parts);
  }
  if (!Number.isInteger(size) || size < 2 || size > 129) throw new Error("没有找到有效的 LUT_3D_SIZE");
  if (values.length !== size * size * size * 3) {
    throw new Error(`LUT 数据不完整：应有 ${size ** 3} 行，实际 ${values.length / 3} 行`);
  }
  return { size, data: Float32Array.from(values, (value) => Math.min(Math.max(value, 0), 1)) };
}
