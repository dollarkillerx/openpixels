// ONNX Runtime and MediaPipe load their wasm by URL at runtime, so bundlers cannot
// see it. Copies the builds in use into public/, where the app points them.
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const web = join(dirname(fileURLToPath(import.meta.url)), "..");

function copy(source, target, names) {
  mkdirSync(join(web, target), { recursive: true });
  for (const name of names) copyFileSync(join(web, source, name), join(web, target, name));
}

copy("node_modules/onnxruntime-web/dist", "public/ort", [
  "ort-wasm-simd-threaded.jsep.mjs",
  "ort-wasm-simd-threaded.jsep.wasm",
]);
copy("node_modules/@mediapipe/tasks-vision/wasm", "public/mediapipe", [
  "vision_wasm_internal.js",
  "vision_wasm_internal.wasm",
]);
