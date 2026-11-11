import { readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import react from "@vitejs/plugin-react";
import { defineConfig, type Plugin } from "vite";

/** Models in public/models that only the desktop edition uses (SAM 2.1 and SigLIP 2). */
const DESKTOP_ONLY_MODELS = ["sam", "siglip"];

/**
 * Keeps local test photos and the desktop-only models out of what gets deployed, along with
 * the hashed copy of the ONNX runtime that Vite emits: ai-worker loads the one in /ort
 * through `wasmPaths`.
 */
const trimDist = (): Plugin => ({
  name: "trim-dist",
  apply: "build",
  closeBundle() {
    const dist = join(__dirname, "dist");
    rmSync(join(dist, "fixtures"), { recursive: true, force: true });
    for (const name of DESKTOP_ONLY_MODELS) rmSync(join(dist, "models", name), { recursive: true, force: true });
    for (const name of readdirSync(join(dist, "assets"))) {
      if (name.startsWith("ort-wasm-") && name.endsWith(".wasm")) rmSync(join(dist, "assets", name));
    }
  },
});

/** Static hosts cap files at 25 MiB (Cloudflare Pages); parts stay a little under. */
const PART_SIZE = 20 * 1024 * 1024;
const SPLIT_ABOVE = 24 * 1024 * 1024;

/**
 * Splits big files in the build into parts, with a `<file>.parts.json` manifest that
 * src/assets.ts reads to put them back together. The originals are removed.
 */
const splitLargeAssets = (): Plugin => ({
  name: "split-large-assets",
  apply: "build",
  closeBundle() {
    const walk = (folder: string): string[] =>
      readdirSync(folder).flatMap((name) => {
        // trimDist removes the test photos at the same time; they are never deployed.
        if (name === "fixtures") return [];
        const path = join(folder, name);
        return statSync(path).isDirectory() ? walk(path) : [path];
      });
    for (const path of walk(join(__dirname, "dist"))) {
      const size = statSync(path).size;
      if (size <= SPLIT_ABOVE) continue;
      const data = readFileSync(path);
      const name = path.slice(path.lastIndexOf("/") + 1);
      const parts: string[] = [];
      for (let at = 0, index = 0; at < size; at += PART_SIZE, index++) {
        const part = `${name}.part${index}`;
        writeFileSync(`${path}.part${index}`, data.subarray(at, Math.min(size, at + PART_SIZE)));
        parts.push(part);
      }
      writeFileSync(`${path}.parts.json`, JSON.stringify({ parts, size }));
      rmSync(path);
    }
  },
});

export default defineConfig({
  plugins: [react(), trimDist(), splitLargeAssets()],
  worker: { format: "es" },
  optimizeDeps: { exclude: ["onnxruntime-web"] },
  build: { target: "es2022" },
  server: { port: 5173, strictPort: true },
});
