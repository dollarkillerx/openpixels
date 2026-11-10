/** How finished files are written: format, size, naming and watermark. */

export type OutputFormat = "jpeg" | "png" | "tiff";
export type WatermarkPosition = "topLeft" | "topRight" | "bottomLeft" | "bottomRight" | "center";

export interface Watermark {
  enabled: boolean;
  text: string;
  /** A PNG as a data URL, or null for text alone. */
  logo: string | null;
  /** Text height as a fraction of the long edge. */
  size: number;
  opacity: number;
  position: WatermarkPosition;
}

/** Colour spaces an export can be delivered in. */
export type OutputColorSpace = "srgb" | "p3" | "adobe" | "prophoto";

export interface OutputSettings {
  format: OutputFormat;
  colorSpace: OutputColorSpace;
  quality: number;
  /** Bits per channel for PNG and TIFF; JPEG is always 8. */
  bitDepth: 8 | 16;
  /** Longest side in pixels; 0 keeps the full size. */
  longEdge: number;
  template: string;
  watermark: Watermark;
}

export const DEFAULT_OUTPUT: OutputSettings = {
  format: "jpeg",
  colorSpace: "srgb",
  quality: 92,
  bitDepth: 8,
  longEdge: 0,
  template: "{name}",
  watermark: { enabled: false, text: "", logo: null, size: 0.03, opacity: 0.7, position: "bottomRight" },
};

export const FORMAT_DETAILS: Record<OutputFormat, { label: string; mime: string; extension: string }> = {
  jpeg: { label: "JPEG", mime: "image/jpeg", extension: "jpg" },
  png: { label: "PNG", mime: "image/png", extension: "png" },
  tiff: { label: "TIFF", mime: "image/tiff", extension: "tif" },
};

export interface WatermarkRaster {
  rgba: Uint8Array;
  x: number;
  y: number;
  width: number;
  height: number;
}

const MARGIN = 0.02;
const LOGO_HEIGHT = 2;
const GAP = 0.5;

/** Draws a watermark for an image of this size, or returns null if there is none to draw. */
export async function renderWatermark(mark: Watermark, imageWidth: number, imageHeight: number): Promise<WatermarkRaster | null> {
  const text = mark.text.trim();
  if (!mark.enabled || (!text && !mark.logo)) return null;

  const longEdge = Math.max(imageWidth, imageHeight);
  const fontSize = Math.max(1, Math.round(mark.size * longEdge));
  const margin = Math.round(MARGIN * longEdge);
  const font = `600 ${fontSize}px system-ui, -apple-system, "PingFang SC", "Noto Sans SC", sans-serif`;

  const measuring = new OffscreenCanvas(1, 1).getContext("2d")!;
  measuring.font = font;
  const textWidth = text ? Math.ceil(measuring.measureText(text).width) : 0;

  const logo = mark.logo ? await createImageBitmap(await (await fetch(mark.logo)).blob()) : null;
  const logoHeight = logo ? fontSize * LOGO_HEIGHT : 0;
  const logoWidth = logo ? Math.round((logo.width / logo.height) * logoHeight) : 0;
  const gap = logo && text ? Math.round(fontSize * GAP) : 0;

  const padding = Math.ceil(fontSize * 0.25);
  const width = Math.max(1, Math.min(logoWidth + gap + textWidth + padding * 2, imageWidth));
  const height = Math.max(1, Math.min(Math.max(logoHeight, Math.ceil(fontSize * 1.4)) + padding * 2, imageHeight));

  const canvas = new OffscreenCanvas(width, height);
  const context = canvas.getContext("2d", { willReadFrequently: true })!;
  context.globalAlpha = mark.opacity;
  if (logo) {
    context.drawImage(logo, padding, (height - logoHeight) / 2, logoWidth, logoHeight);
    logo.close();
  }
  if (text) {
    context.font = font;
    context.textBaseline = "middle";
    context.fillStyle = "white";
    context.shadowColor = "rgba(0, 0, 0, 0.55)";
    context.shadowBlur = fontSize * 0.15;
    context.fillText(text, padding + logoWidth + gap, height / 2);
  }

  const left = mark.position === "topLeft" || mark.position === "bottomLeft";
  const top = mark.position === "topLeft" || mark.position === "topRight";
  const centred = mark.position === "center";
  const x = centred ? Math.round((imageWidth - width) / 2) : left ? margin : imageWidth - width - margin;
  const y = centred ? Math.round((imageHeight - height) / 2) : top ? margin : imageHeight - height - margin;
  const { data } = context.getImageData(0, 0, width, height);
  return { rgba: new Uint8Array(data.buffer), x, y, width, height };
}
