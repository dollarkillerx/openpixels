/** HEIC/HEIF decoding through libheif (LGPL-3.0), loaded only when a HEIC file is opened. */
import { t } from "./i18n";

const HEVC_BRANDS = new Set(["heic", "heix", "hevc", "hevx", "heim", "heis", "hevm", "hevs"]);
const CONTAINER_BRANDS = new Set(["mif1", "msf1"]);
const BOX_HEADER = 8;
const BRANDS_START = 16;
const BRAND_SIZE = 4;

function brandAt(bytes: Uint8Array, at: number) {
  return String.fromCharCode(bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]);
}

/** True for HEIF files holding HEVC images, which rules out AVIF sharing the container. */
export function isHeic(bytes: Uint8Array): boolean {
  if (bytes.length < BRANDS_START || brandAt(bytes, 4) !== "ftyp") return false;
  const major = brandAt(bytes, BOX_HEADER);
  if (HEVC_BRANDS.has(major)) return true;
  if (!CONTAINER_BRANDS.has(major)) return false;
  const boxSize = Math.min((bytes[0] << 24) | (bytes[1] << 16) | (bytes[2] << 8) | bytes[3], bytes.length);
  for (let at = BRANDS_START; at + BRAND_SIZE <= boxSize; at += BRAND_SIZE) {
    if (HEVC_BRANDS.has(brandAt(bytes, at))) return true;
  }
  return false;
}

export interface DecodedRaster {
  width: number;
  height: number;
  rgba: Uint8Array;
}

/** Decodes the primary image, with its rotation already applied. */
export async function decodeHeic(bytes: Uint8Array): Promise<DecodedRaster> {
  const { default: createLibheif } = await import("libheif-js/libheif-wasm/libheif-bundle.mjs");
  const libheif = await createLibheif();
  const decoder = new libheif.HeifDecoder();
  const images = decoder.decode(bytes);
  if (!images.length) throw new Error(t("HEIC 文件里没有可解码的图像"));
  const image = images[0];
  const [width, height] = [image.get_width(), image.get_height()];
  try {
    const shown = await new Promise<{ data: Uint8ClampedArray } | null>((resolve) =>
      image.display({ data: new Uint8ClampedArray(width * height * 4), width, height }, resolve),
    );
    if (!shown) throw new Error(t("HEIC 解码失败"));
    return { width, height, rgba: new Uint8Array(shown.data.buffer) };
  } finally {
    for (const item of images) item.free();
  }
}
