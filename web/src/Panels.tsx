import { useState, type ReactNode } from "react";
import { IS_LITE } from "./edition";

export type PanelTab = "color" | "compose" | "retouch" | "smart" | "output";

export const PANEL_TABS: { id: PanelTab; label: string }[] = [
  { id: "color", label: "调色" },
  { id: "compose", label: "构图" },
  { id: "retouch", label: "修饰" },
  ...(IS_LITE ? [] : [{ id: "smart" as const, label: "智能" }]),
  { id: "output", label: "输出" },
];

/** Which tab each section belongs to, and where it sits in that tab. */
const SECTIONS = {
  whiteBalance: { tab: "color", order: 1 },
  tone: { tab: "color", order: 2 },
  colorEffects: { tab: "color", order: 3 },
  curve: { tab: "color", order: 4 },
  hsl: { tab: "color", order: 5 },
  grade: { tab: "color", order: 6 },
  calibration: { tab: "color", order: 7 },
  look: { tab: "color", order: 8 },
  presets: { tab: "color", order: 9 },
  history: { tab: "color", order: 10 },
  crop: { tab: "compose", order: 1 },
  perspective: { tab: "compose", order: 2 },
  camera: { tab: "compose", order: 3 },
  lensManual: { tab: "compose", order: 4 },
  heal: { tab: "retouch", order: 1 },
  skin: { tab: "retouch", order: 2 },
  liquify: { tab: "retouch", order: 3 },
  local: { tab: "retouch", order: 4 },
  // Lite has no 智能 tab, since this is all that would be left on it.
  subject: { tab: IS_LITE ? "retouch" : "smart", order: IS_LITE ? 5 : 1 },
  replace: { tab: "smart", order: 2 },
  export: { tab: "output", order: 1 },
  batch: { tab: "output", order: 2 },
  stats: { tab: "output", order: 3 },
} as const;

export type SectionId = keyof typeof SECTIONS;

/** Beyond what Lightroom Classic offers, so only in the full edition. */
const FULL_EDITION_ONLY: SectionId[] = ["skin", "liquify", "replace"];

/** Rarely needed at first glance, so these start folded. */
const FOLDED_BY_DEFAULT: SectionId[] = ["grade", "calibration", "presets", "history", "lensManual", "stats"];

const STORAGE_KEY = "openpixels.panel.v1";

function readFolded(): Record<string, boolean> {
  try {
    return JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "{}");
  } catch {
    return {};
  }
}

function writeFolded(id: string, folded: boolean) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ ...readFolded(), [id]: folded }));
  } catch {
    // Only a convenience: the panel still works if this cannot be remembered.
  }
}

interface SectionProps {
  id: SectionId;
  title: ReactNode;
  active: PanelTab;
  testId?: string;
  children: ReactNode;
}

/** A foldable block of the side panel. Stays mounted on other tabs, just hidden. */
export function Section({ id, title, active, testId, children }: SectionProps) {
  const { tab, order } = SECTIONS[id];
  const [folded, setFolded] = useState(() => readFolded()[id] ?? FOLDED_BY_DEFAULT.includes(id));
  if (IS_LITE && FULL_EDITION_ONLY.includes(id)) return null;
  return (
    <section className={`section ${folded ? "folded" : ""}`} style={{ order }} hidden={tab !== active} data-testid={testId}>
      <h2>
        <button
          type="button"
          aria-expanded={!folded}
          onClick={() => {
            writeFolded(id, !folded);
            setFolded(!folded);
          }}
        >
          {title}
        </button>
      </h2>
      <div className="section-body" hidden={folded}>
        {children}
      </div>
    </section>
  );
}
