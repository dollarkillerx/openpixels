import { useEffect, useRef } from "react";
import type { Box } from "./frame";
import { renderWatermark, type Watermark } from "./output";

/** Shows where the watermark will land. It is drawn by the code the export uses. */
export function WatermarkPreview({ mark, box }: { mark: Watermark; box: Box }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    let cancelled = false;
    const width = Math.round(box.width);
    const height = Math.round(box.height);
    void renderWatermark(mark, width, height).then((raster) => {
      const canvas = canvasRef.current;
      if (cancelled || !canvas) return;
      canvas.width = width;
      canvas.height = height;
      if (!raster) return;
      const pixels = new ImageData(new Uint8ClampedArray(raster.rgba), raster.width, raster.height);
      canvas.getContext("2d")!.putImageData(pixels, raster.x, raster.y);
    });
    return () => {
      cancelled = true;
    };
  }, [mark, box.width, box.height]);

  return <canvas ref={canvasRef} className="watermark-preview" style={{ left: box.left, top: box.top, width: box.width, height: box.height }} />;
}
