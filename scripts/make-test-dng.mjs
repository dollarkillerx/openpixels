#!/usr/bin/env node
// Writes an uncompressed RGGB DNG with a synthetic scene, for measuring the decode
// and export path at real sensor sizes without needing a camera file.
//
//   node scripts/make-test-dng.mjs <width> <height> <output.dng>
//
// ColorMatrix1 is the XYZ -> linear sRGB matrix, so the "camera" colour space is
// sRGB and the decoded values can be compared directly to the scene below.
import { closeSync, mkdirSync, openSync, writeSync } from "node:fs";
import { dirname } from "node:path";

const [width, height, output] = [Number(process.argv[2]), Number(process.argv[3]), process.argv[4]];
if (!width || !height || !output) {
  console.error("usage: make-test-dng.mjs <width> <height> <output.dng>");
  process.exit(1);
}

const BLACK = 512;
const WHITE = 16383;
const COLOR_MATRIX = [3.2406, -1.5372, -0.4986, -0.9689, 1.8758, 0.0415, 0.0557, -0.204, 1.057];

const TYPE = { BYTE: 1, ASCII: 2, SHORT: 3, LONG: 4, RATIONAL: 5, SRATIONAL: 10 };
const TYPE_SIZE = { 1: 1, 2: 1, 3: 2, 4: 4, 5: 8, 10: 8 };

const ascii = (text) => Buffer.from(`${text}\0`, "latin1");
function srationals(values) {
  const out = Buffer.alloc(values.length * 8);
  values.forEach((value, i) => {
    out.writeInt32LE(Math.round(value * 10000), i * 8);
    out.writeInt32LE(10000, i * 8 + 4);
  });
  return out;
}
function rationals(values) {
  const out = Buffer.alloc(values.length * 8);
  values.forEach((value, i) => {
    out.writeUInt32LE(Math.round(value * 10000), i * 8);
    out.writeUInt32LE(10000, i * 8 + 4);
  });
  return out;
}

// [tag, type, count, inline value array | Buffer stored out of line]. Ascending by tag.
const stripBytes = width * height * 2;
const entries = [
  [254, TYPE.LONG, 1, [0]],
  [256, TYPE.LONG, 1, [width]],
  [257, TYPE.LONG, 1, [height]],
  [258, TYPE.SHORT, 1, [16]],
  [259, TYPE.SHORT, 1, [1]],
  [262, TYPE.SHORT, 1, [32803]],
  [271, TYPE.ASCII, 11, ascii("OpenPixels")],
  [272, TYPE.ASCII, 10, ascii("Synthetic")],
  [273, TYPE.LONG, 1, ["strip"]],
  [274, TYPE.SHORT, 1, [1]],
  [277, TYPE.SHORT, 1, [1]],
  [278, TYPE.LONG, 1, [height]],
  [279, TYPE.LONG, 1, [stripBytes]],
  [284, TYPE.SHORT, 1, [1]],
  [33421, TYPE.SHORT, 2, [2, 2]],
  [33422, TYPE.BYTE, 4, [0, 1, 1, 2]],
  [50706, TYPE.BYTE, 4, [1, 4, 0, 0]],
  [50708, TYPE.ASCII, 20, ascii("OpenPixels Synthetic")],
  [50714, TYPE.SHORT, 1, [BLACK]],
  [50717, TYPE.SHORT, 1, [WHITE]],
  [50721, TYPE.SRATIONAL, 9, srationals(COLOR_MATRIX)],
  [50728, TYPE.RATIONAL, 3, rationals([1, 1, 1])],
  [50778, TYPE.SHORT, 1, [21]],
];

const ifdOffset = 8;
const ifdSize = 2 + entries.length * 12 + 4;
let extraOffset = ifdOffset + ifdSize;
const extras = [];
for (const entry of entries) {
  if (Buffer.isBuffer(entry[3])) {
    extras.push({ entry, offset: extraOffset });
    extraOffset += entry[3].length + (entry[3].length % 2);
  }
}
const stripOffset = extraOffset;

const header = Buffer.alloc(stripOffset);
header.write("II", 0, "latin1");
header.writeUInt16LE(42, 2);
header.writeUInt32LE(ifdOffset, 4);
header.writeUInt16LE(entries.length, ifdOffset);

entries.forEach(([tag, type, count, value], i) => {
  const at = ifdOffset + 2 + i * 12;
  header.writeUInt16LE(tag, at);
  header.writeUInt16LE(type, at + 2);
  header.writeUInt32LE(Buffer.isBuffer(value) ? value.length / (TYPE_SIZE[type] === 8 ? 8 : 1) : count, at + 4);
  if (Buffer.isBuffer(value)) {
    header.writeUInt32LE(extras.find((extra) => extra.entry[3] === value).offset, at + 8);
    return;
  }
  value.forEach((item, n) => {
    const number = item === "strip" ? stripOffset : item;
    const position = at + 8 + n * TYPE_SIZE[type];
    if (type === TYPE.BYTE) header.writeUInt8(number, position);
    else if (type === TYPE.SHORT) header.writeUInt16LE(number, position);
    else header.writeUInt32LE(number, position);
  });
});
for (const { entry, offset } of extras) entry[3].copy(header, offset);

/** Scene-linear value of channel c (0 R, 1 G, 2 B) at a photosite. */
function scene(x, y, c) {
  if (c === 0) return x / width;
  if (c === 1) return y / height;
  const checker = ((x >> 8) ^ (y >> 8)) & 1;
  return checker ? 0.65 : 0.25;
}

mkdirSync(dirname(output), { recursive: true });
const fd = openSync(output, "w");
writeSync(fd, header);
const row = Buffer.allocUnsafe(width * 2);
for (let y = 0; y < height; y++) {
  for (let x = 0; x < width; x++) {
    const c = y & 1 ? (x & 1 ? 2 : 1) : x & 1 ? 1 : 0;
    row.writeUInt16LE(Math.round(BLACK + scene(x, y, c) * (WHITE - BLACK)), x * 2);
  }
  writeSync(fd, row);
}
closeSync(fd);
console.log(`${output}: ${width}x${height}, ${((stripOffset + stripBytes) / 1e6).toFixed(0)} MB`);
