import { useRef } from "react";
import { dragCrop, type Box, type Crop, type CropHandle } from "./frame";

interface Props {
  box: Box;
  crop: Crop;
  ratio: number | null;
  frameWidth: number;
  frameHeight: number;
  onChange: (crop: Crop) => void;
}

const CORNERS: CropHandle[] = ["nw", "ne", "sw", "se"];

export function CropOverlay({ box, crop, ratio, frameWidth, frameHeight, onChange }: Props) {
  const dragRef = useRef<{ handle: CropHandle; x: number; y: number; start: Crop } | null>(null);

  const begin = (handle: CropHandle) => (event: React.PointerEvent) => {
    event.stopPropagation();
    event.currentTarget.setPointerCapture(event.pointerId);
    dragRef.current = { handle, x: event.clientX, y: event.clientY, start: crop };
  };

  const move = (event: React.PointerEvent) => {
    const drag = dragRef.current;
    if (!drag) return;
    onChange(
      dragCrop(
        drag.start,
        drag.handle,
        (event.clientX - drag.x) / box.width,
        (event.clientY - drag.y) / box.height,
        ratio,
        frameWidth,
        frameHeight,
      ),
    );
  };

  const end = () => {
    dragRef.current = null;
  };

  const handlers = (handle: CropHandle) => ({
    onPointerDown: begin(handle),
    onPointerMove: move,
    onPointerUp: end,
    onPointerCancel: end,
  });

  return (
    <div className="crop-layer" style={{ ...box }}>
      <div
        className="crop-rect"
        data-testid="crop-rect"
        style={{
          left: `${crop.x * 100}%`,
          top: `${crop.y * 100}%`,
          width: `${crop.w * 100}%`,
          height: `${crop.h * 100}%`,
        }}
        {...handlers("move")}
      >
        {CORNERS.map((corner) => (
          <div key={corner} className={`crop-handle ${corner}`} data-handle={corner} {...handlers(corner)} />
        ))}
      </div>
    </div>
  );
}
