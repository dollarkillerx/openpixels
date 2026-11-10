import { useEffect, useRef } from "react";
import type { HistogramData } from "./protocol";
import { t } from "./i18n";

/** Only this share of pixels at an extreme counts as clipping, so noise does not. */
const CLIPPED_SHARE = 0.002;
const CHANNELS = [
  { index: 0, colour: "#e0483e" },
  { index: 1, colour: "#55b04a" },
  { index: 2, colour: "#3f73d8" },
];

interface Props {
  data: HistogramData | null;
  shadows: boolean;
  highlights: boolean;
  onToggleShadows: () => void;
  onToggleHighlights: () => void;
}

export function Histogram({ data, shadows, highlights, onToggleShadows, onToggleHighlights }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ratio = window.devicePixelRatio;
    const { clientWidth: width, clientHeight: height } = canvas;
    canvas.width = Math.round(width * ratio);
    canvas.height = Math.round(height * ratio);
    const context = canvas.getContext("2d")!;
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, width, height);
    if (!data) return;

    // Square root, so a large flat area does not flatten everything else.
    const scale = (value: number) => Math.sqrt(value);
    let tallest = 1;
    for (const { index } of CHANNELS) {
      // The extremes are left out, or clipping would squash the rest of the graph.
      for (let bin = 1; bin < 255; bin++) tallest = Math.max(tallest, data.channels[index][bin]);
    }
    context.globalCompositeOperation = "lighter";
    for (const { index, colour } of CHANNELS) {
      context.fillStyle = colour;
      context.beginPath();
      context.moveTo(0, height);
      for (let bin = 0; bin < 256; bin++) {
        context.lineTo((bin / 255) * width, height - Math.min(1, scale(data.channels[index][bin]) / scale(tallest)) * height);
      }
      context.lineTo(width, height);
      context.closePath();
      context.fill();
    }
    context.globalCompositeOperation = "source-over";
  }, [data]);

  const clipped = (bin: number) =>
    !!data && data.channels.some((channel) => channel[bin] / Math.max(1, data.total) > CLIPPED_SHARE);

  return (
    <div className="histogram" data-testid="histogram">
      <canvas ref={canvasRef} aria-label={t("直方图")} />
      <button
        type="button"
        className={`clip-toggle shadow ${shadows ? "on" : ""} ${clipped(0) ? "clipped" : ""}`}
        aria-label={t("显示阴影溢出")}
        aria-pressed={shadows}
        title={t("显示阴影溢出（J 同时切换两侧）")}
        onClick={onToggleShadows}
      />
      <button
        type="button"
        className={`clip-toggle highlight ${highlights ? "on" : ""} ${clipped(255) ? "clipped" : ""}`}
        aria-label={t("显示高光溢出")}
        aria-pressed={highlights}
        title={t("显示高光溢出（J 同时切换两侧）")}
        onClick={onToggleHighlights}
      />
    </div>
  );
}
