import { useRef } from "react";

interface Props {
  label: string;
  hue: number;
  amount: number;
  disabled: boolean;
  onChange: (hue: number, amount: number) => void;
}

/** Picks a hue by angle and a strength by distance from the centre. */
export function ColorWheel({ label, hue, amount, disabled, onChange }: Props) {
  const wheelRef = useRef<HTMLDivElement>(null);
  const draggingRef = useRef(false);

  const update = (event: React.PointerEvent) => {
    const wheel = wheelRef.current;
    if (!wheel || !draggingRef.current) return;
    const bounds = wheel.getBoundingClientRect();
    const radius = bounds.width / 2;
    const dx = event.clientX - bounds.left - radius;
    const dy = event.clientY - bounds.top - radius;
    const angle = (Math.atan2(dx, -dy) * 180) / Math.PI;
    onChange((angle + 360) % 360, Math.min(Math.hypot(dx, dy) / radius, 1));
  };

  const radians = (hue * Math.PI) / 180;
  return (
    <div className="wheel-cell">
      <div
        ref={wheelRef}
        className={`wheel ${disabled ? "disabled" : ""}`}
        data-testid={`wheel-${label}`}
        onPointerDown={(event) => {
          if (disabled) return;
          event.currentTarget.setPointerCapture?.(event.pointerId);
          draggingRef.current = true;
          update(event);
        }}
        onPointerMove={update}
        onPointerUp={() => (draggingRef.current = false)}
        onPointerCancel={() => (draggingRef.current = false)}
        onDoubleClick={() => !disabled && onChange(0, 0)}
      >
        <div
          className="wheel-dot"
          style={{ left: `${50 + Math.sin(radians) * amount * 50}%`, top: `${50 - Math.cos(radians) * amount * 50}%` }}
        />
      </div>
      <span>{label}</span>
    </div>
  );
}
