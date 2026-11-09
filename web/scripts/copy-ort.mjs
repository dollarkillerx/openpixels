// ONNX Runtime loads its wasm by URL at runtime, so bundlers cannot see it.
// Copies the JSEP WebGPU build into public/ort, where the AI worker points it.
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const web = join(dirname(fileURLToPath(import.meta.url)), "..");
const source = join(web, "node_modules/onnxruntime-web/dist");
const target = join(web, "public/ort");

mkdirSync(target, { recursive: true });
for (const extension of ["mjs", "wasm"]) {
  const name = `ort-wasm-simd-threaded.jsep.${extension}`;
  copyFileSync(join(source, name), join(target, name));
}
