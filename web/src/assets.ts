/**
 * Loads large runtime files (models, the ONNX Runtime wasm). Static hosts such as
 * Cloudflare Pages cap single files at 25 MiB, so the build splits bigger files into parts
 * and writes `<file>.parts.json` beside them; see `splitLargeAssets` in vite.config.ts.
 * Unsplit files (the dev server, other hosts) are fetched as they are.
 */

interface PartsManifest {
  parts: string[];
  size: number;
}

async function manifestFor(url: string): Promise<PartsManifest | null> {
  try {
    const response = await fetch(`${url}.parts.json`);
    // A dev server answers unknown paths with the app's HTML, so check what came back.
    if (!response.ok || !(response.headers.get("content-type") ?? "").includes("json")) return null;
    return (await response.json()) as PartsManifest;
  } catch {
    return null;
  }
}

async function bytes(url: string): Promise<Uint8Array> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
  return new Uint8Array(await response.arrayBuffer());
}

/** The whole file at `url`, reassembled from its parts if the build split it. */
export async function fetchAsset(url: string): Promise<Uint8Array<ArrayBuffer>> {
  const manifest = await manifestFor(url);
  if (!manifest) return (await bytes(url)) as Uint8Array<ArrayBuffer>;
  const base = url.slice(0, url.lastIndexOf("/") + 1);
  const parts = await Promise.all(manifest.parts.map((name) => bytes(base + name)));
  const whole = new Uint8Array(manifest.size);
  let at = 0;
  for (const part of parts) {
    whole.set(part, at);
    at += part.length;
  }
  if (at !== manifest.size) throw new Error(`${url}: expected ${manifest.size} bytes, got ${at}`);
  return whole;
}
