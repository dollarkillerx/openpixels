import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { IDENTITY_POINTS, curvePoints, parametricOffset, splineThrough, withCurvePoints } from "../src/curve.ts";
import { BUILT_IN_LOOKS, gained } from "../src/lut.ts";

const PARAMETRIC_KEYS = ["curveHighlights", "curveLights", "curveDarks", "curveShadows"];

interface SharedCase {
  points: [number, number][];
  parametric: number[];
  at: number[];
  expected: number[];
}

test("draws the same curve the engine applies", () => {
  const { cases } = JSON.parse(readFileSync(new URL("../../crates/op-core/src/curve_cases.json", import.meta.url), "utf8")) as { cases: SharedCase[] };
  for (const [number, { points, parametric, at, expected }] of cases.entries()) {
    const spline = splineThrough(points.map(([x, y]) => ({ x, y })));
    const adjustments = Object.fromEntries(PARAMETRIC_KEYS.map((key, i) => [key, parametric[i]]));
    at.forEach((x, i) => {
      const value = Math.min(Math.max(spline(x) + parametricOffset(adjustments, x), 0), 1);
      assert.ok(Math.abs(value - expected[i]) < 1e-4, `case ${number} at ${x}: ${value} against ${expected[i]}`);
    });
  }
});

test("reads curves saved before free points", () => {
  const points = curvePoints({ curveM2: 0.2 }, "M");
  assert.deepEqual(points.map((point) => point.x), [0, 0.25, 0.5, 0.75, 1]);
  assert.ok(Math.abs(points[2].y - 0.7) < 1e-9);
});

test("an untouched curve stores nothing, and writing replaces the old form", () => {
  const written = withCurvePoints({ curveM2: 0.2, exposure: 1 }, "M", [{ x: 0, y: 0 }, { x: 0.5, y: 0.6 }, { x: 1, y: 1 }]);
  assert.equal(written.curveM2, undefined);
  assert.equal(written.curveMN, 3);
  assert.deepEqual(withCurvePoints(written, "M", IDENTITY_POINTS), { exposure: 1 });
  assert.equal(withCurvePoints({ curveRN: 2 }, "M", IDENTITY_POINTS).curveRN, 2, "other channels are kept");
});

test("a brightening gain rolls off instead of clipping", () => {
  for (const gain of [1.05, 1.1, 1.25]) {
    const values = [0.7, 0.8, 0.9, 0.95, 0.98, 1].map((value) => gained(value, gain));
    assert.ok(values.every((value, i) => i === 0 || value > values[i - 1]), `gain ${gain}: ${values}`);
    assert.ok(Math.abs(values[values.length - 1] - 1) < 1e-9);
  }
  assert.equal(gained(0.5, 1.1), 0.55, "below the knee it is a plain gain");
  assert.equal(gained(0.9, 0.9), 0.81, "darkening gains are unchanged");
});

test("looks with a warming gain keep their brightest tones apart", () => {
  const luma = ([r, g, b]: number[]) => 0.2126 * r + 0.7152 * g + 0.0722 * b;
  for (const look of BUILT_IN_LOOKS) {
    for (const tint of [[1, 0.8, 0.6], [1, 1, 1]]) {
      // Clamped as the generated table is, so a channel that clips shows up here.
      const levels = [0.9, 0.95, 0.98, 1].map((value) => luma(look.grade(value * tint[0], value * tint[1], value * tint[2]).map((c) => Math.min(Math.max(c, 0), 1))));
      assert.ok(levels.every((value, i) => i === 0 || value > levels[i - 1]), `${look.name} ${tint}: ${levels}`);
    }
  }
});

test("batch names never reuse a file already in the folder", async () => {
  const { freeName } = await import("../src/names.ts");
  const taken = new Set(["dsc_0001.jpg", "dsc_0001 (2).jpg"]);
  assert.equal(freeName("DSC_0001.JPG", taken), "DSC_0001 (3).JPG");
  assert.equal(freeName("other.jpg", taken), "other.jpg");
});
