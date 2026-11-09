import { readdirSync, rmSync } from "node:fs";
import { join } from "node:path";
import react from "@vitejs/plugin-react";
import { defineConfig, type Plugin } from "vite";

/**
 * Keeps local test photos out of what gets deployed, along with the hashed copy of the ONNX
 * runtime that Vite emits: ai-worker loads the one in /ort through `wasmPaths`.
 */
const trimDist = (): Plugin => ({
  name: "trim-dist",
  apply: "build",
  closeBundle() {
    const dist = join(__dirname, "dist");
    rmSync(join(dist, "fixtures"), { recursive: true, force: true });
    for (const name of readdirSync(join(dist, "assets"))) {
      if (name.startsWith("ort-wasm-") && name.endsWith(".wasm")) rmSync(join(dist, "assets", name));
    }
  },
});

export default defineConfig({
  plugins: [react(), trimDist()],
  worker: { format: "es" },
  optimizeDeps: { exclude: ["onnxruntime-web"] },
  build: { target: "es2022" },
  server: { port: 5173, strictPort: true },
});
