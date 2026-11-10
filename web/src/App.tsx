import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ColorWheel } from "./ColorWheel";
import { CropOverlay } from "./CropOverlay";
import { ToneCurve } from "./ToneCurve";
import { chooseBatchTarget } from "./batch";
import { EngineClient, type MaskStats, type SkinStats } from "./engine";
import { FIT_VIEW, FULL_FRAME, cropFields, cropForRatio, cropOf, fitBox, frameSize, outputToSource, shrinkToImage, sourceToOutput, viewBox, type Crop, type View } from "./frame";
import { Histogram } from "./Histogram";
import { BUILT_IN_LOOKS, LOOK_GROUPS, builtInLut, parseCube } from "./lut";
import { deleteProfile, deleteProject, forgetHandle, getAsset, getLut, listAssets, listLuts, listProfiles, listProjects, loadAnalyses, loadErases, recallHandle, recallHandleEntries, rememberHandle, saveAnalysis, saveAsset, saveErases, saveLut, saveProfile, saveProject, type Project, type StoredAsset, type StoredLut, type StoredProfile } from "./store";
import { analyzeImage, groupPhotos, judgeGroup, type PhotoAnalysis, type Verdict } from "./analysis";
import { matchColours } from "./colormatch";
import { locateFaces } from "./face";
import { BUILT_IN_REPLACEMENTS, REPLACE_BACKGROUND, REPLACE_SKY, builtInReplacement, decodeReplacement } from "./replace";
import { eraseCrop, rasterizeErase, type EraseDab } from "./erase";
import { lensView, shownToSource, sourceToShown, type WarpView } from "./lens";
import { FORMAT_DETAILS, type OutputColorSpace, type OutputFormat, type OutputSettings, type Watermark, type WatermarkPosition } from "./output";
import { ProjectLibrary } from "./ProjectLibrary";
import { IS_LITE } from "./edition";
import { PANEL_TABS, Section, type PanelTab, type SectionId } from "./Panels";
import { WatermarkPreview } from "./WatermarkPreview";
import { LOCAL_RANGE_SUFFIXES, LOCAL_SLIDERS, MAX_LOCAL_LAYERS, hasLocalHandles, isAiLocal, modelOf, removeLocalValues, renderLocalMask, type AiLocalKind, type LocalKind, type LocalLayer, type ModelMasks } from "./local";
import {
  exportName,
  fileId,
  loadEdit,
  loadFlags,
  loadHeals,
  loadLocals,
  loadOutput,
  loadLabels,
  loadOutputPresets,
  loadPresets,
  loadSnapshots,
  saveLabel,
  saveOutputPresets,
  saveSnapshots,
  type Snapshot,
  loadRatings,
  loadWarps,
  persistable,
  loadProof,
  saveProof,
  type ProofSettings,
  saveHeals,
  saveFlag,
  saveLocals,
  saveOutput,
  saveRating,
  saveWarps,
  savePresets,
  saveEdits,
  type Presets, loadLastProject, saveLastProject } from "./library";
import type { Adjustments, ErasePatch, HealSpot, HistogramData, LoadResult, Thumbnail, WarpStroke } from "./protocol";
import { INPAINT_MODEL } from "./model";
import { LANGUAGES, currentLanguage, setLanguage, t, type Language } from "./i18n";

interface SliderDef {
  key: string;
  label: string;
  min: number;
  max: number;
  /** The value when untouched, if that is not 0. */
  fallback?: number;
}

const GROUPS: { section: SectionId; title: string; sliders: SliderDef[] }[] = [
  {
    section: "whiteBalance",
    title: t("白平衡"),
    sliders: [
      { key: "temperature", label: t("色温"), min: -2, max: 2 },
      { key: "tint", label: t("色调"), min: -2, max: 2 },
    ],
  },
  {
    section: "tone",
    title: t("影调"),
    sliders: [
      { key: "exposure", label: t("曝光"), min: -4, max: 4 },
      { key: "contrast", label: t("对比度"), min: -1, max: 1 },
      { key: "highlights", label: t("高光"), min: -1, max: 1 },
      { key: "shadows", label: t("阴影"), min: -1, max: 1 },
      { key: "whites", label: t("白色"), min: -1, max: 1 },
      { key: "blacks", label: t("黑色"), min: -1, max: 1 },
    ],
  },
  {
    section: "colorEffects",
    title: t("色彩与效果"),
    sliders: [
      { key: "vibrance", label: t("自然饱和度"), min: -1, max: 1 },
      { key: "saturation", label: t("饱和度"), min: -1, max: 1 },
      { key: "dehaze", label: t("去雾"), min: -1, max: 1 },
      { key: "clarity", label: t("清晰度"), min: -1, max: 1 },
      { key: "texture", label: t("纹理"), min: -1, max: 1 },
      { key: "sharpen", label: t("锐化"), min: 0, max: 1 },
      { key: "sharpenRadius", label: t("锐化半径"), min: 1, max: 3, fallback: 1 },
      { key: "sharpenDetail", label: t("锐化细节"), min: 0, max: 1 },
      { key: "sharpenMasking", label: t("锐化蒙版"), min: 0, max: 1 },
      { key: "denoiseLuma", label: t("明度降噪"), min: 0, max: 1 },
      { key: "denoiseChroma", label: t("色彩降噪"), min: 0, max: 1 },
      { key: "vignette", label: t("暗角"), min: -1, max: 1 },
      { key: "postCropVignette", label: t("裁剪后暗角"), min: -1, max: 1 },
      { key: "grain", label: t("颗粒"), min: 0, max: 1 },
      { key: "grainSize", label: t("颗粒大小"), min: 0, max: 1 },
    ],
  },
  {
    section: "calibration",
    title: t("校准"),
    sliders: [
      { key: "calibrationRedHue", label: t("红原色色相"), min: -1, max: 1 },
      { key: "calibrationRedSat", label: t("红原色饱和度"), min: -1, max: 1 },
      { key: "calibrationGreenHue", label: t("绿原色色相"), min: -1, max: 1 },
      { key: "calibrationGreenSat", label: t("绿原色饱和度"), min: -1, max: 1 },
      { key: "calibrationBlueHue", label: t("蓝原色色相"), min: -1, max: 1 },
      { key: "calibrationBlueSat", label: t("蓝原色饱和度"), min: -1, max: 1 },
    ],
  },
  {
    section: "lensManual",
    title: t("镜头校正"),
    sliders: [
      { key: "lensDistortion", label: t("畸变"), min: -1, max: 1 },
      { key: "lensVignette", label: t("暗角补偿"), min: -1, max: 1 },
      { key: "lensCaRed", label: t("红/青边"), min: -1, max: 1 },
      { key: "lensCaBlue", label: t("蓝/黄边"), min: -1, max: 1 },
    ],
  },
  {
    section: "perspective",
    title: t("透视矫正"),
    sliders: [
      { key: "perspectiveV", label: t("垂直透视"), min: -1, max: 1 },
      { key: "perspectiveH", label: t("水平透视"), min: -1, max: 1 },
    ],
  },
];

const PARAMETRIC_SLIDERS: SliderDef[] = [
  { key: "curveHighlights", label: t("曲线高光"), min: -1, max: 1 },
  { key: "curveLights", label: t("曲线亮调"), min: -1, max: 1 },
  { key: "curveDarks", label: t("曲线暗调"), min: -1, max: 1 },
  { key: "curveShadows", label: t("曲线阴影"), min: -1, max: 1 },
];

const POINT_SLIDERS: SliderDef[] = [
  { key: "pointWidth", label: t("混合点范围"), min: 10, max: 90, fallback: 30 },
  { key: "pointHueShift", label: t("混合点色相"), min: -1, max: 1 },
  { key: "pointSatShift", label: t("混合点饱和度"), min: -1, max: 1 },
  { key: "pointLumShift", label: t("混合点明度"), min: -1, max: 1 },
];

const SETTING_LABELS: Record<string, string> = Object.fromEntries(
  [...GROUPS.flatMap((group) => group.sliders), ...PARAMETRIC_SLIDERS, ...POINT_SLIDERS].map((slider) => [slider.key, slider.label]),
);

/** The photos that have an analysis, in the shape grouping expects. */
const analyzedItems = (photos: { id: string }[], analyses: Record<string, PhotoAnalysis>) =>
  photos.filter((photo) => analyses[photo.id]).map((photo) => ({ id: photo.id, analysis: analyses[photo.id] }));

interface SettingName {
  label: string;
  /** Names the change when several keys of this kind move together. */
  group?: string;
  /** Whether the change in value means anything to a reader. */
  numeric?: boolean;
}

const CURVE_CHANNELS: Record<string, string> = { M: t("RGB 曲线"), R: t("红色曲线"), G: t("绿色曲线"), B: t("蓝色曲线") };
/** History names for edits made by one action rather than by moving a setting. */
const ACTION_LABELS = new WeakMap<Adjustments, string>();
const CROP_GROUP = t("裁剪与旋转");
const POINT_GROUP = t("颜色混合点");

/** What an edit key is called in the history list, or null if it has no name. */
function nameOfSetting(key: string): SettingName | null {
  const slider = SETTING_LABELS[key];
  if (key === "temperature" || key === "tint") return { label: slider, group: t("白平衡"), numeric: true };
  if (key === "pointHue") return { label: t("混合点取色"), group: POINT_GROUP };
  if (key.startsWith("point") && slider) return { label: slider, group: POINT_GROUP, numeric: true };
  if (slider) return { label: slider, numeric: true };
  if (key === "rotate") return { label: t("旋转"), group: CROP_GROUP, numeric: true };
  if (key === "quarter" || /^crop[XYWH]$/.test(key)) return { label: CROP_GROUP, group: CROP_GROUP };
  if (key === "lutId") return { label: t("滤镜"), group: t("滤镜") };
  if (key === "lutAmount") return { label: t("滤镜强度"), group: t("滤镜"), numeric: true };
  if (key === "cameraProfile") return { label: t("色彩配置") };
  if (key === "lensAuto") return { label: t("自动镜头校正") };
  if (key === "perspectiveKeep") return { label: t("保留全图") };
  if (key === "backgroundExposure") return { label: t("背景曝光"), numeric: true };
  if (key === "maskOverlay" || key === "localOverlay") return { label: t("显示蒙版") };

  const hsl = key.match(/^hsl(Hue|Sat|Lum)(\d)$/);
  if (hsl) {
    const channel = HSL_CHANNELS.find((item) => item.prefix === `hsl${hsl[1]}`)!;
    return { label: t("{band}{channel}", { band: HSL_BANDS[Number(hsl[2])].name, channel: channel.label }), numeric: true };
  }
  const curve = key.match(/^curve([MRGB])(?:\d|N|[XY]\d+)$/);
  if (curve) return { label: CURVE_CHANNELS[curve[1]], group: CURVE_CHANNELS[curve[1]] };
  const grade = key.match(/^grade(Shadow|Mid|High)(Hue|Amount)$/);
  if (grade) {
    const label = t("{range}色轮", { range: GRADE_RANGES.find((item) => item.key === grade[1])!.label });
    return { label, group: label };
  }
  const local = key.match(/^local\d(.+)$/);
  if (local) {
    const slider = LOCAL_SLIDERS.find((item) => item.suffix === local[1]);
    if (slider) return { label: t("局部{setting}", { setting: slider.label }), numeric: true };
    if (LOCAL_RANGE_SUFFIXES.includes(local[1])) return { label: t("局部范围"), group: t("局部范围") };
  }
  return null;
}

/** Names the change between two states of an edit, for the history list. */
function describeChange(before: Adjustments, after: Adjustments) {
  const changed = [...new Set([...Object.keys(before), ...Object.keys(after)])].filter((key) => (before[key] ?? 0) !== (after[key] ?? 0));
  if (changed.length === 0) return t("无变化");
  const action = ACTION_LABELS.get(after);
  if (action) return action;
  if (changed.every((key) => /^local\d/.test(key))) {
    // A layer always has an exposure, so that key coming or going means the layer did.
    const exposures = changed.filter((key) => /^local\dExposure$/.test(key));
    if (exposures.some((key) => !(key in before))) return t("添加局部图层");
    if (exposures.some((key) => !(key in after))) return t("删除局部图层");
  }
  const names = changed.map(nameOfSetting);
  const groups = new Set(names.map((name) => name?.group));
  if (changed.length > 1 && groups.size === 1 && names[0]?.group) return names[0].group;
  if (changed.length > 3) return t("多项调整（{count}）", { count: changed.length });
  const position = names.findIndex((name) => name);
  if (position < 0) return t("调整");
  const { label, numeric } = names[position]!;
  if (!numeric) return label;
  const delta = (after[changed[position]] ?? 0) - (before[changed[position]] ?? 0);
  return `${label} ${delta > 0 ? "+" : ""}${delta.toFixed(2)}`;
}

const HSL_BANDS = [
  { name: t("红"), color: "#e0483e" },
  { name: t("橙"), color: "#e88a2e" },
  { name: t("黄"), color: "#e5cb3a" },
  { name: t("绿"), color: "#55b04a" },
  { name: t("青"), color: "#3fbcc0" },
  { name: t("蓝"), color: "#3f73d8" },
  { name: t("紫"), color: "#7e52c8" },
  { name: t("品红"), color: "#cc4aa8" },
];

const HSL_CHANNELS = [
  { prefix: "hslHue", label: t("色相") },
  { prefix: "hslSat", label: t("饱和度") },
  { prefix: "hslLum", label: t("明度") },
];

const GRADE_RANGES = [
  { key: "Shadow", label: t("阴影") },
  { key: "Mid", label: t("中间调") },
  { key: "High", label: t("高光") },
];

const LIQUIFY_MODES = [
  { value: 0, label: t("推动") },
  { value: 1, label: t("膨胀") },
  { value: 2, label: t("收缩") },
];

const CROP_RATIOS: { label: string; value: number | null }[] = [
  { label: t("自由"), value: null },
  { label: "1:1", value: 1 },
  { label: "4:5", value: 4 / 5 },
  { label: "3:4", value: 3 / 4 },
  { label: "2:3", value: 2 / 3 },
  { label: "5:7", value: 5 / 7 },
  { label: "3:2", value: 3 / 2 },
  { label: "16:9", value: 16 / 9 },
];

const LOCAL_MASK_INTERVAL_MS = 33;
const MIN_LIQUIFY_STEP = 2;
const LIQUIFY_SETTLE_MS = 120;
const LONG_EDGE_CHOICES = [0, 6000, 4096, 3000, 2048, 1600, 1080];
const WATERMARK_POSITIONS: { value: WatermarkPosition; label: string }[] = [
  { value: "bottomRight", label: t("右下") },
  { value: "bottomLeft", label: t("左下") },
  { value: "topRight", label: t("右上") },
  { value: "topLeft", label: t("左上") },
  { value: "center", label: t("居中") },
];
const WATERMARK_LOGO_LONG_EDGE = 512;
/** Past the right edge, so the whole frame shows the unedited image. */
const COMPARE_ALL = 1.01;
const STAMP_SPACING = 0.35;
const STAMP_FLUSH_MS = 40;
const COLOR_LABELS = [
  { name: t("红"), color: "#e0483e" },
  { name: t("黄"), color: "#e5cb3a" },
  { name: t("绿"), color: "#55b04a" },
  { name: t("蓝"), color: "#3f73d8" },
  { name: t("紫"), color: "#7e52c8" },
];
/** Shown instead of the photo's own, until it has been opened. */
const QUICK_THUMBNAIL_LONG_EDGE = 320;
/** Framing and masks belong to one photo, so they are not carried over to others. */
const NOT_SYNCED = /^(crop|quarter$|rotate$|local\d)/;
const ERASE_SPACING = 0.3;
/** Largest zoom, as a share of actual pixels. */
const MAX_ZOOM_RATIO = 4;
const WHEEL_ZOOM_SPEED = 0.002;
const ANALYSIS_LONG_EDGE = 1024;
const ANALYSIS_DETAIL_LONG_EDGE = 2048;
const MATCH_LONG_EDGE = 256;
/** `groupFilter` value showing only the best photo of each group. */
const GROUP_BEST = -1;

/** Milliseconds for a `YYYY:MM:DD HH:MM:SS` capture time. Only differences between them matter. */
function captureMillis(text: string | null) {
  const match = text?.match(/^(\d{4}):(\d{2}):(\d{2}) (\d{2}):(\d{2}):(\d{2})/);
  if (!match) return null;
  const [year, month, day, hour, minute, second] = match.slice(1).map(Number);
  return Date.UTC(year, month - 1, day, hour, minute, second);
}
const FLAG_PICK = 1;
const FLAG_REJECT = -1;
const FLAG_KEYS: Record<string, number> = { p: FLAG_PICK, x: FLAG_REJECT, u: 0 };
const FRAME_WINDOW = 30;
const THUMBNAIL_LONG_EDGE = 200;
const SETTLE_MS = 400;
const HISTORY_LIMIT = 100;
const RAW_ACCEPT = ".arw,.cr2,.cr3,.nef,.nrw,.raf,.rw2,.orf,.dng,.pef,.srw,.iiq,.3fr,.jpg,.jpeg,.png,.tif,.tiff,.heic,.heif";

interface Photo {
  id: string;
  file: File;
  thumbnail?: string;
}

interface History {
  past: Adjustments[];
  future: Adjustments[];
}

const sameEdit = (a: Adjustments, b: Adjustments) =>
  JSON.stringify(Object.entries(persistable(a)).sort()) === JSON.stringify(Object.entries(persistable(b)).sort());

function thumbnailUrl({ rgb, width, height }: Thumbnail): string {
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const context = canvas.getContext("2d")!;
  const image = context.createImageData(width, height);
  const source = new Uint8Array(rgb);
  for (let i = 0, j = 0; i < source.length; i += 3, j += 4) {
    image.data[j] = source[i];
    image.data[j + 1] = source[i + 1];
    image.data[j + 2] = source[i + 2];
    image.data[j + 3] = 255;
  }
  context.putImageData(image, 0, 0);
  return canvas.toDataURL("image/jpeg", 0.8);
}

/** The analysis a saved edit needs before it can be rendered. */
const needsSubject = (edit: Adjustments) => (edit.backgroundExposure ?? 0) !== 0 || (edit.backgroundAsset ?? 0) > 0;
const needsSky = (edit: Adjustments) => (edit.skyAsset ?? 0) > 0;

/** What can be replaced, with each one's stored settings and sliders. */
const REPLACE_LAYERS = [
  {
    mode: REPLACE_BACKGROUND,
    title: t("换背景"),
    asset: "backgroundAsset",
    amount: "backgroundAmount",
    sliders: [
      { key: "backgroundAmount", label: t("融合强度"), min: 0, max: 1 },
      { key: "backgroundFillExposure", label: t("亮度"), min: -3, max: 3 },
      { key: "backgroundShift", label: t("上下位置"), min: -0.5, max: 0.5 },
    ],
  },
  {
    mode: REPLACE_SKY,
    title: t("换天"),
    asset: "skyAsset",
    amount: "skyAmount",
    sliders: [
      { key: "skyAmount", label: t("融合强度"), min: 0, max: 1 },
      { key: "skyFillExposure", label: t("亮度"), min: -3, max: 3 },
      { key: "skyShift", label: t("上下位置"), min: -0.5, max: 0.5 },
      { key: "skyRelight", label: t("前景融合"), min: 0, max: 1 },
    ],
  },
];
type ReplaceLayer = (typeof REPLACE_LAYERS)[number];
const DEFAULT_RELIGHT = 0.3;
const needsSkin = (edit: Adjustments) => (edit.skinSmooth ?? 0) !== 0 || (edit.skinBlemish ?? 0) !== 0;

const isDifferentImage = (error: unknown) => error instanceof Error && error.message === STALE;
const STALE = "stale";

export function App() {
  const stageRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const engineRef = useRef<EngineClient | null>(null);
  const framesRef = useRef<number[]>([]);
  const editsRef = useRef<Record<string, Adjustments>>({});
  const historyRef = useRef<Record<string, History>>({});
  const settledRef = useRef<Adjustments>({});
  const settleTimerRef = useRef<number | undefined>(undefined);
  const pendingSettleRef = useRef<(() => void) | null>(null);
  const openTicketRef = useRef(0);
  /** The photo the engine currently holds. */
  const openedIdRef = useRef<string | null>(null);
  /** The local layer shapes that went with each settled edit, so undo can restore them. */
  const localsAtRef = useRef(new WeakMap<Adjustments, LocalLayer[]>());

  const [ready, setReady] = useState(false);
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [photos, setPhotos] = useState<Photo[]>([]);
  const [currentId, setCurrentId] = useState<string | null>(null);
  const [adjustments, setAdjustments] = useState<Adjustments>({});
  const [loaded, setLoaded] = useState<LoadResult | null>(null);
  const [heapMB, setHeapMB] = useState(0);
  const [frame, setFrame] = useState<{ last: number; median: number } | null>(null);
  const [exported, setExported] = useState<{ url: string; name: string; ms: number; sizeMB: number } | null>(null);
  const [detecting, setDetecting] = useState(false);
  const [maskStats, setMaskStats] = useState<MaskStats | null>(null);
  const [skinStats, setSkinStats] = useState<SkinStats | null>(null);
  const [findingFaces, setFindingFaces] = useState(false);
  const [presets, setPresets] = useState<Presets>(loadPresets);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const selectionAnchorRef = useRef<string | null>(null);
  const [copiedEdit, setCopiedEdit] = useState<Adjustments | null>(null);
  const [labels, setLabels] = useState<Record<string, number>>(loadLabels);
  const [labelFilter, setLabelFilter] = useState(0);
  const [gridView, setGridView] = useState(false);
  const [snapshots, setSnapshots] = useState<Snapshot[]>([]);
  const [outputPresets, setOutputPresets] = useState<Record<string, OutputSettings>>(loadOutputPresets);
  const [outputPresetName, setOutputPresetName] = useState("");
  const [, setHistoryTick] = useState(0);
  const labelKeysRef = useRef<(id: string, value: number) => void>(() => {});
  const [selectedPreset, setSelectedPreset] = useState("");
  const [presetName, setPresetName] = useState("");
  const [batch, setBatch] = useState<{ done: number; total: number; ms: number } | null>(null);
  const [batchRunning, setBatchRunning] = useState(false);
  const [batchUrl, setBatchUrl] = useState<string | null>(null);
  const [hslBand, setHslBand] = useState(0);
  const [stageSize, setStageSize] = useState({ width: 1, height: 1 });
  const [cropRatio, setCropRatio] = useState<number | null>(null);
  const [heals, setHeals] = useState<HealSpot[]>([]);
  const [healing, setHealing] = useState(false);
  const [healRadius, setHealRadius] = useState(0.012);
  const [pointer, setPointer] = useState<{ x: number; y: number } | null>(null);
  const [warpGroups, setWarpGroups] = useState<WarpStroke[][]>([]);
  const [liquifying, setLiquifying] = useState(false);
  const [liquifyRadius, setLiquifyRadius] = useState(0.06);
  const [liquifyStrength, setLiquifyStrength] = useState(0.7);
  const [liquifyMode, setLiquifyMode] = useState(0);
  const [ratings, setRatings] = useState<Record<string, number>>(loadRatings);
  const [flags, setFlags] = useState<Record<string, number>>(loadFlags);
  const [flagFilter, setFlagFilter] = useState(0);
  const [analyses, setAnalyses] = useState<Record<string, PhotoAnalysis>>({});
  const [analyzing, setAnalyzing] = useState<{ done: number; total: number } | null>(null);
  const [cullStatus, setCullStatus] = useState("");
  const [groupFilter, setGroupFilter] = useState(0);
  const [replaceAssets, setReplaceAssets] = useState<StoredAsset[]>([]);
  const [matchReference, setMatchReference] = useState("");
  const [matching, setMatching] = useState(false);
  const [compare, setCompare] = useState(0);
  const compareHeldRef = useRef<number | null>(null);
  const compareDragRef = useRef(false);
  const [autoToning, setAutoToning] = useState(false);
  const [proof, setProof] = useState<ProofSettings>(loadProof);
  const [profiles, setProfiles] = useState<StoredProfile[]>([]);
  const [picker, setPicker] = useState<"white" | "point" | null>(null);
  const pickingWhite = picker === "white";
  const [toolHint, setToolHint] = useState("");
  const [panelTab, setPanelTab] = useState<PanelTab>("color");
  const [view, setViewState] = useState<View>(FIT_VIEW);
  const viewRef = useRef<View>(FIT_VIEW);
  const panRef = useRef<{ startX: number; startY: number; view: View; moved: boolean } | null>(null);
  const panClickRef = useRef(false);
  const spaceRef = useRef(false);
  const [histogram, setHistogram] = useState<HistogramData | null>(null);
  const [clipWarnings, setClipWarnings] = useState({ highlights: false, shadows: false });
  const zoomKeysRef = useRef<{ toggle: () => void }>({ toggle: () => {} });
  const quickThumbnailTried = useRef(new Set<string>());
  const [output, setOutput] = useState<OutputSettings>(loadOutput);
  const [healMode, setHealMode] = useState<"spot" | "clone" | "erase" | "redeye">("spot");
  const [eraseDabs, setEraseDabs] = useState<EraseDab[]>([]);
  const [eraseCount, setEraseCount] = useState(0);
  const [erasing, setErasing] = useState(false);
  const erasesRef = useRef<ErasePatch[]>([]);
  const warpRef = useRef<WarpView | null>(null);
  const eraseStrokeRef = useRef<{ drawing: boolean; last: { x: number; y: number } | null }>({ drawing: false, last: null });
  const [stampMarker, setStampMarker] = useState<{ x: number; y: number } | null>(null);
  const stampRef = useRef<{ source: { x: number; y: number } | null; offset: { x: number; y: number } | null; last: { x: number; y: number } | null; group: number; drawing: boolean }>({ source: null, offset: null, last: null, group: 0, drawing: false });
  const pendingDabsRef = useRef<HealSpot[]>([]);
  const dabTimerRef = useRef<number | null>(null);
  const [importedLuts, setImportedLuts] = useState<StoredLut[]>([]);
  const [restorable, setRestorable] = useState<{ id: string; handle: FileSystemFileHandle }[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const projectsRef = useRef<Project[]>([]);
  const [activeProject, setActiveProject] = useState<number | null>(null);
  const [lastProject, setLastProject] = useState<number | null>(loadLastProject);
  const [libraryOpen, setLibraryOpen] = useState(false);
  const [projectNotice, setProjectNotice] = useState("");
  const coverSourceRef = useRef<Record<number, string>>({});
  const fileInputRef = useRef<HTMLInputElement>(null);
  const handleDragRef = useRef<"a" | "b" | null>(null);
  const [minRating, setMinRating] = useState(0);
  const dragRef = useRef<{ x: number; y: number; strokes: WarpStroke[] } | null>(null);
  const [locals, setLocals] = useState<LocalLayer[]>([]);
  const [activeLocal, setActiveLocal] = useState(0);
  const [localTool, setLocalTool] = useState<LocalKind | null>(null);
  /** Painting onto the active layer's mask, instead of creating a layer. */
  const [localRefine, setLocalRefine] = useState<"add" | "subtract" | null>(null);
  const modelMasksRef = useRef<ModelMasks>({});
  const [localBrush, setLocalBrush] = useState(0.04);
  const localsRef = useRef<LocalLayer[]>([]);
  const localFrameRef = useRef<number | null>(null);
  const localDragRef = useRef(false);

  const current = photos.find((photo) => photo.id === currentId) ?? null;
  const passesFlag = (id: string) => {
    const flag = flags[id] ?? 0;
    return flagFilter === 0 || (flagFilter === 2 ? flag !== FLAG_REJECT : flag === flagFilter);
  };
  const { groupOf, verdicts, groupCount } = useMemo(() => {
    const items = analyzedItems(photos, analyses);
    const byId = new Map(items.map((item) => [item.id, item]));
    const groups = groupPhotos(items);
    const groupOf = new Map<string, number>();
    const verdicts = new Map<string, Verdict>();
    groups.forEach((ids, index) => {
      ids.forEach((id) => groupOf.set(id, index + 1));
      judgeGroup(ids.map((id) => byId.get(id)!)).forEach((verdict, id) => verdicts.set(id, verdict));
    });
    return { groupOf, verdicts, groupCount: groups.length };
  }, [photos, analyses]);
  const passesGroup = (id: string) =>
    groupFilter === 0 || (groupFilter === GROUP_BEST ? verdicts.get(id)?.best !== false : groupOf.get(id) === groupFilter);
  const shownPhotos = photos.filter(
    (photo) => (ratings[photo.id] ?? 0) >= minRating && passesFlag(photo.id) && passesGroup(photo.id) && (labelFilter === 0 || labels[photo.id] === labelFilter),
  );
  // An erase takes seconds and lands on the photo it started on.
  const locked = batchRunning || erasing;
  const lockedRef = useRef(locked);
  lockedRef.current = locked;
  const projectOfActive = projects.find((item) => item.id === activeProject) ?? null;
  const resumable = activeProject === null ? (projects.find((item) => item.id === lastProject) ?? null) : null;
  const lens = loaded ? lensView(loaded.lens, adjustments, loaded.width, loaded.height) : null;

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || engineRef.current) return;
    const engine = new EngineClient(canvas.transferControlToOffscreen());
    engineRef.current = engine;
    engine.onHistogram = setHistogram;
    engine.onFrame = (ms) => {
      const frames = framesRef.current;
      frames.push(ms);
      if (frames.length > FRAME_WINDOW) frames.shift();
      const sorted = [...frames].sort((a, b) => a - b);
      setFrame({ last: ms, median: sorted[Math.floor(sorted.length / 2)] });
    };
    engine.onCrash = setError;
    engine.ready.then(() => setReady(true)).catch((reason: Error) => setError(reason.message));
  }, []);

  useEffect(() => {
    const stage = stageRef.current;
    if (!stage || !ready) return;
    const observer = new ResizeObserver(([entry]) => {
      const ratio = window.devicePixelRatio;
      setStageSize({ width: entry.contentRect.width, height: entry.contentRect.height });
      engineRef.current?.resize(
        Math.round(entry.contentRect.width * ratio),
        Math.round(entry.contentRect.height * ratio),
      );
    });
    observer.observe(stage);
    return () => observer.disconnect();
  }, [ready]);

  useEffect(() => {
    listLuts().then(setImportedLuts).catch(() => {});
    listProfiles().then(setProfiles).catch(() => {});
    Promise.all([listProjects(), recallHandleEntries()])
      .then(([stored, entries]) => {
        projectsRef.current = stored;
        setProjects(stored);
        const inProjects = new Set(stored.flatMap((project) => project.photoIds));
        setRestorable(entries.filter((entry) => !inProjects.has(entry.id)));
      })
      .catch(() => {});
  }, []);

  const resolveLook = useCallback(async (id: number) => {
    if (!id) return null;
    return builtInLut(id) ?? (await getLut(id).catch(() => undefined)) ?? null;
  }, []);

  const resolveReplace = useCallback(async (id: number) => {
    const builtIn = builtInReplacement(id);
    if (builtIn) return builtIn;
    const asset = await getAsset(id).catch(() => undefined);
    return asset ? decodeReplacement(asset.blob) : null;
  }, []);

  const refreshThumbnail = useCallback(async (id: string) => {
    const engine = engineRef.current;
    // The engine renders whichever photo it has open, which may no longer be `id`.
    if (!engine || openedIdRef.current !== id) return;
    const ticket = openTicketRef.current;
    const thumbnail = await engine.thumbnail(THUMBNAIL_LONG_EDGE, true);
    if (ticket !== openTicketRef.current || openedIdRef.current !== id) return;
    const url = thumbnailUrl(thumbnail);
    setPhotos((list) => list.map((photo) => (photo.id === id ? { ...photo, thumbnail: url } : photo)));
  }, []);

  /** Opens a photo in the engine. Rejects with STALE if another open overtook it. */
  const openPhoto = useCallback(
    async (photo: Photo, edit: Adjustments, spots: HealSpot[], warps: WarpStroke[][]) => {
      const engine = engineRef.current!;
      const ticket = ++openTicketRef.current;
      const erases = await loadErases(photo.id);
      modelMasksRef.current = {};
      const result = await engine.open(await photo.file.arrayBuffer(), edit, spots, warps.flat(), erases);
      if (ticket !== openTicketRef.current) throw new Error(STALE);
      openedIdRef.current = photo.id;
      erasesRef.current = erases;
      setEraseCount(erases.length);
      warpRef.current = await engine.warpField();
      return result;
    },
    [],
  );

  /** Records an edit still waiting to settle, before anything changes the context. */
  const ensureModelMasks = useCallback(async (layers: LocalLayer[], proxyWidth: number, proxyHeight: number) => {
    const engine = engineRef.current!;
    const ticket = openTicketRef.current;
    for (const model of ["subject", "sky"] as const) {
      const needed = layers.some((layer) => isAiLocal(layer.kind) && modelOf(layer.kind) === model);
      if (!needed || modelMasksRef.current[model]) continue;
      const mask = await engine.modelMask(model, proxyWidth, proxyHeight);
      // Another photo opened meanwhile; this mask is not its.
      if (ticket !== openTicketRef.current) throw new Error(STALE);
      modelMasksRef.current[model] = mask;
    }
  }, []);

  const flushPendingEdit = useCallback(() => {
    window.clearTimeout(settleTimerRef.current);
    pendingSettleRef.current?.();
    pendingSettleRef.current = null;
  }, []);

  const proxySizeRef = useRef({ width: 1, height: 1 });

  /** Rasterises the layers shortly; drags change them faster than that is useful. */
  const scheduleLocalMask = useCallback(() => {
    if (localFrameRef.current !== null) return;
    localFrameRef.current = window.setTimeout(() => {
      localFrameRef.current = null;
      const { width, height } = proxySizeRef.current;
      void engineRef.current?.setLocalMask(renderLocalMask(localsRef.current, width, height, modelMasksRef.current), width, height);
    }, LOCAL_MASK_INTERVAL_MS);
  }, []);

  const selectPhoto = useCallback(
    async (photo: Photo) => {
      flushPendingEdit();
      const edit = persistable(editsRef.current[photo.id] ?? loadEdit(photo.id));
      const spots = loadHeals(photo.id);
      const warps = loadWarps(photo.id);
      editsRef.current[photo.id] = edit;
      setHeals(spots);
      setHealing(false);
      setPicker(null);
      setToolHint("");
      stampRef.current = { source: null, offset: null, last: null, group: 0, drawing: false };
      setStampMarker(null);
      setEraseDabs([]);
      setSnapshots(loadSnapshots(photo.id));
      viewRef.current = FIT_VIEW;
      setViewState(FIT_VIEW);
      setWarpGroups(warps);
      setLiquifying(false);
      const layers = loadLocals(photo.id);
      localsRef.current = layers;
      setLocals(layers);
      setActiveLocal(0);
      setLocalTool(null);
    setLocalRefine(null);
      settledRef.current = edit;
      localsAtRef.current.set(edit, layers);
      setCurrentId(photo.id);
      setAdjustments(edit);
      setBusy(t("解码中…"));
      setError("");
      setExported(null);
      setMaskStats(null);
      setSkinStats(null);
      setDetecting(false);
      setFindingFaces(false);
      try {
        const result = await openPhoto(photo, edit, spots, warps);
        const ticket = openTicketRef.current;
        setLoaded(result);
        setHeapMB(result.heapMB);
        proxySizeRef.current = { width: result.proxyWidth, height: result.proxyHeight };
        if (layers.length > 0) {
          await ensureModelMasks(layers, result.proxyWidth, result.proxyHeight);
          await engineRef.current!.setLocalMask(
            renderLocalMask(layers, result.proxyWidth, result.proxyHeight, modelMasksRef.current),
            result.proxyWidth,
            result.proxyHeight,
          );
        }
        if (ticket !== openTicketRef.current) return;
        setBusy("");
        // A saved edit needs its analysis back before it can be shown.
        if (needsSubject(edit)) {
          setDetecting(true);
          const stats = await engineRef.current!.detectSubject(result.proxyWidth, result.proxyHeight);
          if (ticket === openTicketRef.current) setMaskStats(stats);
          setDetecting(false);
        }
        if (needsSky(edit)) {
          setDetecting(true);
          await engineRef.current!.detectSky();
          setDetecting(false);
        }
        if (needsSkin(edit)) {
          setFindingFaces(true);
          const stats = await engineRef.current!.prepareSkin(result.proxyWidth, result.proxyHeight);
          if (ticket === openTicketRef.current) setSkinStats(stats);
          setFindingFaces(false);
        }
        await refreshThumbnail(photo.id);
      } catch (reason) {
        if (isDifferentImage(reason)) return;
        setError((reason as Error).message);
        setBusy("");
        setDetecting(false);
        setFindingFaces(false);
      }
    },
    [flushPendingEdit, openPhoto, refreshThumbnail],
  );

  const addFiles = useCallback(
    (files: File[]) => {
      if (files.length === 0) return;
      const incoming = files.map((file) => ({ id: fileId(file), file }));
      setPhotos((list) => {
        const known = new Set(list.map((photo) => photo.id));
        return [...list, ...incoming.filter((photo) => !known.has(photo.id))];
      });
      void selectPhoto(incoming[0]);
    },
    [selectPhoto],
  );

  // Photos added while a project is open belong to it, and its first photo is the cover.
  useEffect(() => {
    if (activeProject === null || photos.length === 0) return;
    const project = projectsRef.current.find((item) => item.id === activeProject);
    if (!project) return;
    const known = new Set(project.photoIds);
    const added = photos.filter((photo) => !known.has(photo.id)).map((photo) => photo.id);
    const photoIds = [...project.photoIds, ...added];
    const coverUrl = photos.find((photo) => photo.id === photoIds[0])?.thumbnail;
    const coverChanged = !!coverUrl && coverSourceRef.current[project.id] !== coverUrl;
    if (added.length === 0 && !coverChanged) return;
    if (coverUrl) coverSourceRef.current[project.id] = coverUrl;
    void (async () => {
      let cover = project.cover;
      if (coverChanged) {
        const blob = await (await fetch(coverUrl)).blob();
        cover = await new Promise<string>((resolve) => {
          const reader = new FileReader();
          reader.onload = () => resolve(reader.result as string);
          reader.readAsDataURL(blob);
        });
      }
      const latest = projectsRef.current.find((item) => item.id === project.id) ?? project;
      const merged = [...new Set([...latest.photoIds, ...photoIds])];
      const saved = await saveProject({ ...latest, photoIds: merged, cover });
      projectsRef.current = [...projectsRef.current.filter((item) => item.id !== saved.id), saved];
      setProjects(projectsRef.current);
    })().catch(() => {});
  }, [activeProject, photos]);

  // The look follows the edit, whatever changed it: a slider, undo, a preset or a new photo.
  const lookId = adjustments.lutId ?? 0;
  const backgroundAsset = adjustments.backgroundAsset ?? 0;
  const skyAsset = adjustments.skyAsset ?? 0;

  useEffect(() => {
    if (!ready || !loaded || !backgroundAsset) return;
    let cancelled = false;
    void resolveReplace(backgroundAsset).then((image) => {
      if (!cancelled && image) void engineRef.current?.setReplaceImage(false, image);
    });
    return () => {
      cancelled = true;
    };
  }, [ready, loaded, backgroundAsset, resolveReplace]);

  useEffect(() => {
    if (!ready || !loaded || !skyAsset) return;
    let cancelled = false;
    void resolveReplace(skyAsset).then((image) => {
      if (!cancelled && image) void engineRef.current?.setReplaceImage(true, image);
    });
    return () => {
      cancelled = true;
    };
  }, [ready, loaded, skyAsset, resolveReplace]);

  useEffect(() => {
    void loadAnalyses().then(setAnalyses).catch(() => {});
    void listAssets().then(setReplaceAssets).catch(() => {});
  }, []);
  useEffect(() => {
    if (!ready || !loaded) return;
    let cancelled = false;
    void resolveLook(lookId).then((lut) => {
      if (!cancelled) void engineRef.current?.setLook(lut);
    });
    return () => {
      cancelled = true;
    };
  }, [ready, loaded, lookId, resolveLook]);

  // Development shortcut: /?fixture=/a.dng,/b.dng loads files without a picker.
  useEffect(() => {
    if (!ready || !import.meta.env.DEV) return;
    const fixtures = new URLSearchParams(location.search).get("fixture");
    if (!fixtures) return;
    Promise.all(
      fixtures.split(",").map(async (path) => {
        const response = await fetch(path);
        if (!response.ok) throw new Error(`${path}: HTTP ${response.status}`);
        return new File([await response.blob()], path.split("/").pop()!, { lastModified: 1 });
      }),
    )
      .then(addFiles)
      .catch((reason: Error) => setError(reason.message));
  }, [ready, addFiles]);

  const applyEdit = useCallback(
    (next: Adjustments, recordNow = false) => {
      if (!currentId) return;
      setAdjustments(next);
      editsRef.current[currentId] = next;
      engineRef.current?.adjust(next);

      const settle = () => {
        pendingSettleRef.current = null;
        const previous = settledRef.current;
        if (sameEdit(previous, next)) return;
        const history = (historyRef.current[currentId] ??= { past: [], future: [] });
        history.past = [...history.past, previous].slice(-HISTORY_LIMIT);
        history.future = [];
        settledRef.current = next;
        localsAtRef.current.set(next, localsRef.current);
        saveEdits({ [currentId]: next });
        setHistoryTick((tick) => tick + 1);
      };
      pendingSettleRef.current = settle;
      window.clearTimeout(settleTimerRef.current);
      const settleAndRefresh = () => {
        settle();
        void refreshThumbnail(currentId).catch(() => {});
      };
      if (recordNow) settleAndRefresh();
      else settleTimerRef.current = window.setTimeout(settleAndRefresh, SETTLE_MS);
    },
    [currentId, refreshThumbnail],
  );

  const setValue = (key: string, value: number) => applyEdit({ ...adjustments, [key]: value });
  const reset = () => applyEdit({ maskOverlay: adjustments.maskOverlay ?? 0 }, true);

  const rate = useCallback((id: string, rating: number) => {
    saveRating(id, rating);
    setRatings((all) => ({ ...all, [id]: rating }));
  }, []);

  const flag = useCallback((id: string, value: number) => {
    saveFlag(id, value);
    setFlags((all) => ({ ...all, [id]: value }));
  }, []);

  const stepHistory = useCallback(
    (direction: "undo" | "redo") => {
      if (!currentId) return;
      flushPendingEdit();
      const history = historyRef.current[currentId];
      if (!history) return;
      const [from, to] = direction === "undo" ? [history.past, history.future] : [history.future, history.past];
      const target = from.pop();
      if (!target) return;
      // Layer shapes live apart from the edit; the state being left keeps its current ones.
      localsAtRef.current.set(settledRef.current, localsRef.current);
      to.push(settledRef.current);
      settledRef.current = target;
      const layers = localsAtRef.current.get(target);
      if (layers && layers !== localsRef.current) {
        localsRef.current = layers;
        setLocals(layers);
        setActiveLocal((index) => Math.min(index, Math.max(0, layers.length - 1)));
        setLocalRefine(null);
        scheduleLocalMask();
        saveLocals(currentId, layers);
      }
      setAdjustments(target);
      editsRef.current[currentId] = target;
      engineRef.current?.adjust(target);
      saveEdits({ [currentId]: target });
      void refreshThumbnail(currentId).catch(() => {});
    },
    [currentId, flushPendingEdit, refreshThumbnail, scheduleLocalMask],
  );

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const tag = (event.target as HTMLElement).tagName;
      // Text fields keep their own undo, and nothing may change while a batch runs.
      if (tag === "INPUT" || tag === "TEXTAREA" || lockedRef.current) return;
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "z") {
        event.preventDefault();
        stepHistory(event.shiftKey ? "redo" : "undo");
      }
    };
    window.addEventListener("keydown", onKey);
    const onRate = (event: KeyboardEvent) => {
      const tag = (event.target as HTMLElement).tagName;
      if (tag === "INPUT" || tag === "SELECT" || event.metaKey || event.ctrlKey) return;
      if (event.key.toLowerCase() === "s") toggleProofRef.current();
      if (/^[0-5]$/.test(event.key) && currentId) rate(currentId, Number(event.key));
      if (/^[6-9]$/.test(event.key) && currentId) {
        const value = Number(event.key) - 5;
        labelKeysRef.current(currentId, value);
      }
      if (event.key.toLowerCase() === "g") setGridView(true);
      if (event.key.toLowerCase() === "e" || event.key === "Enter") setGridView(false);
      const flagValue = FLAG_KEYS[event.key.toLowerCase()];
      if (flagValue !== undefined && currentId) flag(currentId, flagValue);
    };
    window.addEventListener("keydown", onRate);
    const isTyping = (event: KeyboardEvent) => ["INPUT", "SELECT", "TEXTAREA"].includes((event.target as HTMLElement).tagName);
    const holdCompare = (event: KeyboardEvent) => {
      if (event.key !== "\\" || event.repeat || isTyping(event)) return;
      setCompare((split) => {
        compareHeldRef.current = split;
        return COMPARE_ALL;
      });
    };
    const releaseCompare = (event: KeyboardEvent) => {
      if (event.key !== "\\" || compareHeldRef.current === null) return;
      setCompare(compareHeldRef.current);
      compareHeldRef.current = null;
    };
    window.addEventListener("keydown", holdCompare);
    window.addEventListener("keyup", releaseCompare);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("keydown", onRate);
      window.removeEventListener("keydown", holdCompare);
      window.removeEventListener("keyup", releaseCompare);
    };
  }, [stepHistory, currentId, rate, flag]);

  useEffect(() => {
    engineRef.current?.compare(compare);
  }, [compare]);

  useEffect(() => {
    engineRef.current?.setClipWarnings(clipWarnings.highlights, clipWarnings.shadows);
  }, [clipWarnings]);

  useEffect(() => {
    // Crop handles are laid out for the whole frame, so the crop is edited at fit.
    if ((adjustments.cropEditing ?? 0) === 0) return;
    viewRef.current = FIT_VIEW;
    setViewState(FIT_VIEW);
    engineRef.current?.setView(1, 0.5, 0.5);
  }, [adjustments.cropEditing]);

  useEffect(() => {
    const typing = (event: KeyboardEvent) => ["INPUT", "SELECT", "TEXTAREA"].includes((event.target as HTMLElement).tagName);
    const onDown = (event: KeyboardEvent) => {
      if (typing(event) || event.metaKey || event.ctrlKey) return;
      if (event.code === "Space" && (event.target as HTMLElement).tagName !== "BUTTON") {
        spaceRef.current = true;
        event.preventDefault();
      }
      if (event.key.toLowerCase() === "z") zoomKeysRef.current.toggle();
      if (event.key.toLowerCase() === "j") {
        setClipWarnings((all) => {
          const on = !(all.highlights && all.shadows);
          return { highlights: on, shadows: on };
        });
      }
    };
    const onUp = (event: KeyboardEvent) => {
      if (event.code === "Space") spaceRef.current = false;
    };
    const onBlur = () => {
      spaceRef.current = false;
    };
    window.addEventListener("keydown", onDown);
    window.addEventListener("keyup", onUp);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("keydown", onDown);
      window.removeEventListener("keyup", onUp);
      window.removeEventListener("blur", onBlur);
    };
  }, []);

  // The browser can read most finished files directly, so these need not be opened first.
  useEffect(() => {
    const waiting = photos.filter((photo) => !photo.thumbnail && !quickThumbnailTried.current.has(photo.id));
    if (waiting.length === 0) return;
    for (const photo of waiting) quickThumbnailTried.current.add(photo.id);
    void (async () => {
      for (const photo of waiting) {
        try {
          const bitmap = await createImageBitmap(photo.file);
          const scale = Math.min(1, QUICK_THUMBNAIL_LONG_EDGE / Math.max(bitmap.width, bitmap.height));
          const canvas = document.createElement("canvas");
          canvas.width = Math.max(1, Math.round(bitmap.width * scale));
          canvas.height = Math.max(1, Math.round(bitmap.height * scale));
          canvas.getContext("2d")!.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
          bitmap.close();
          const url = canvas.toDataURL("image/jpeg", 0.8);
          setPhotos((list) => list.map((item) => (item.id === photo.id && !item.thumbnail ? { ...item, thumbnail: url } : item)));
        } catch {
          // RAW and other formats the browser cannot read show their name until opened.
        }
      }
    })();
  }, [photos]);

  const wheelRef = useRef<(event: WheelEvent) => void>(() => {});
  useEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    // Not through React, whose wheel listeners are passive and could not stop the page scrolling.
    const onWheel = (event: WheelEvent) => wheelRef.current(event);
    stage.addEventListener("wheel", onWheel, { passive: false });
    return () => stage.removeEventListener("wheel", onWheel);
  }, []);

  const quarter = adjustments.quarter ?? 0;
  const rotate = adjustments.rotate ?? 0;
  const imageFrame = frameSize(loaded?.width ?? 1, loaded?.height ?? 1, quarter);
  const crop = cropOf(adjustments);
  const cropEditing = (adjustments.cropEditing ?? 0) !== 0;
  const shownCrop = cropEditing ? FULL_FRAME : crop;
  const outputWidth = imageFrame.width * shownCrop.w;
  const fitted = (stageWidth: number, stageHeight: number) =>
    fitBox(outputWidth / (imageFrame.height * shownCrop.h), stageWidth, stageHeight);
  /** The zoomed image on a stage of this size, from the latest view. */
  const frameBox = (stageWidth: number, stageHeight: number) =>
    viewBox(fitted(stageWidth, stageHeight), stageWidth, stageHeight, cropEditing ? FIT_VIEW : viewRef.current);
  const displayBox = frameBox(stageSize.width, stageSize.height);
  /** The zoom at which one image pixel fills one device pixel, never below fitted. */
  const actualPixelsZoom = () => {
    // Measured now: layout state can lag behind, and does not update in a hidden page.
    const stage = stageRef.current?.getBoundingClientRect() ?? stageSize;
    return Math.max(1, outputWidth / Math.max(1, fitted(stage.width, stage.height).width * window.devicePixelRatio));
  };
  const zoomPercent = Math.round((view.zoom / actualPixelsZoom()) * 100);

  const applyView = (next: View) => {
    const clamped = { zoom: next.zoom, x: Math.min(1, Math.max(0, next.x)), y: Math.min(1, Math.max(0, next.y)) };
    viewRef.current = clamped;
    setViewState(clamped);
    engineRef.current?.setView(clamped.zoom, clamped.x, clamped.y);
  };

  /** Zooms while keeping the point at stage position (px, py) where it is. */
  const zoomAround = (zoom: number, px: number, py: number) => {
    const limited = Math.min(Math.max(1, actualPixelsZoom() * MAX_ZOOM_RATIO), Math.max(1, zoom));
    const stage = stageRef.current!.getBoundingClientRect();
    const box = frameBox(stage.width, stage.height);
    const [s, t] = [(px - box.left) / box.width, (py - box.top) / box.height];
    const fit = fitted(stage.width, stage.height);
    const [width, height] = [fit.width * limited, fit.height * limited];
    applyView({ zoom: limited, x: s + (stage.width / 2 - px) / width, y: t + (stage.height / 2 - py) / height });
  };

  const zoomToRatio = (ratio: number) => {
    const stage = stageRef.current?.getBoundingClientRect();
    if (stage) zoomAround(actualPixelsZoom() * ratio, stage.width / 2, stage.height / 2);
  };

  zoomKeysRef.current.toggle = () => {
    if (!loaded || cropEditing) return;
    if (viewRef.current.zoom > 1) applyView(FIT_VIEW);
    else zoomToRatio(1);
  };

  const toolActive = healing || liquifying || !!localTool || !!localRefine || !!picker || cropEditing;

  /** Starts panning if this press is for that and not for the tool in use. */
  const startPan = (event: React.PointerEvent<HTMLElement>) => {
    if (viewRef.current.zoom <= 1 || (toolActive && !spaceRef.current)) return false;
    event.currentTarget.setPointerCapture?.(event.pointerId);
    panRef.current = { startX: event.clientX, startY: event.clientY, view: viewRef.current, moved: false };
    return true;
  };

  const continuePan = (event: React.PointerEvent<HTMLElement>) => {
    const pan = panRef.current;
    if (!pan) return false;
    const bounds = event.currentTarget.getBoundingClientRect();
    const box = fitted(bounds.width, bounds.height);
    const [dx, dy] = [event.clientX - pan.startX, event.clientY - pan.startY];
    pan.moved ||= Math.hypot(dx, dy) > 2;
    applyView({ zoom: pan.view.zoom, x: pan.view.x - dx / (box.width * pan.view.zoom), y: pan.view.y - dy / (box.height * pan.view.zoom) });
    return true;
  };

  const finishPan = () => {
    if (panRef.current?.moved) panClickRef.current = true;
    panRef.current = null;
  };

  wheelRef.current = (event) => {
    if (!loaded || cropEditing) return;
    event.preventDefault();
    const bounds = stageRef.current!.getBoundingClientRect();
    zoomAround(viewRef.current.zoom * Math.exp(-event.deltaY * WHEEL_ZOOM_SPEED), event.clientX - bounds.left, event.clientY - bounds.top);
  };

  const updateCrop = (next: Crop) =>
    applyEdit({ ...adjustments, ...cropFields(shrinkToImage(next, rotate, imageFrame.width, imageFrame.height)) });

  const toggleCropEditing = () => {
    setPicker(null);
    setHealing(false);
    setLiquifying(false);
    setLocalTool(null);
    setLocalRefine(null);
    setValue("cropEditing", cropEditing ? 0 : 1);
  };

  const chooseRatio = (value: number | null) => {
    setCropRatio(value);
    if (value !== null) updateCrop(cropForRatio(value, imageFrame.width, imageFrame.height));
  };

  const setRotation = (degrees: number) =>
    applyEdit({
      ...adjustments,
      rotate: degrees,
      ...cropFields(shrinkToImage(crop, degrees, imageFrame.width, imageFrame.height)),
    });

  const turn = (direction: 1 | -1) => {
    setCropRatio(null);
    applyEdit({ ...adjustments, quarter: quarter + direction, rotate: 0, ...cropFields(FULL_FRAME) }, true);
  };

  const resetCrop = () => {
    setCropRatio(null);
    applyEdit({ ...adjustments, quarter: 0, rotate: 0, ...cropFields(FULL_FRAME) }, true);
  };

  const toggleHealing = () => {
    if (cropEditing) setValue("cropEditing", 0);
    setPicker(null);
    setLiquifying(false);
    setLocalTool(null);
    setLocalRefine(null);
    setHealing((on) => !on);
  };

  const toggleLiquify = () => {
    if (cropEditing) setValue("cropEditing", 0);
    setPicker(null);
    setHealing(false);
    setLocalTool(null);
    setLocalRefine(null);
    setLiquifying((on) => !on);
  };

  const chooseLocalTool = (kind: LocalKind) => {
    if (cropEditing) setValue("cropEditing", 0);
    setPicker(null);
    setHealing(false);
    setLiquifying(false);
    setLocalTool((current) => (current === kind ? null : kind));
  };

  const togglePicker = (kind: "white" | "point") => {
    if (cropEditing) setValue("cropEditing", 0);
    setHealing(false);
    setLiquifying(false);
    setLocalTool(null);
    setLocalRefine(null);
    setToolHint("");
    setPicker((on) => (on === kind ? null : kind));
  };

  const removePointColour = () =>
    applyEdit({ ...adjustments, pointHue: 0, pointWidth: 0, pointHueShift: 0, pointSatShift: 0, pointLumShift: 0 }, true);

  const pickColour = async (event: React.MouseEvent<HTMLElement>) => {
    if (!picker || !currentId) return;
    const [photoId, ticket] = [currentId, openTicketRef.current];
    const latest = () => editsRef.current[photoId] ?? adjustments;
    const bounds = event.currentTarget.getBoundingClientRect();
    const u = (event.clientX - bounds.left) / bounds.width;
    const v = (event.clientY - bounds.top) / bounds.height;
    try {
      if (picker === "point") {
        const hue = await engineRef.current!.hueAt(u, v, Math.round(bounds.width), Math.round(bounds.height));
        if (ticket !== openTicketRef.current) return;
        if (hue < 0) {
          setToolHint(t("这里接近灰色，请点一处有颜色的地方。"));
          return;
        }
        setToolHint("");
        setPicker(null);
        const edit = latest();
        applyEdit({ ...edit, pointHue: hue, pointWidth: edit.pointWidth || 30 }, true);
        return;
      }
      const balance = await engineRef.current!.whiteBalanceAt(u, v, Math.round(bounds.width), Math.round(bounds.height));
      if (ticket !== openTicketRef.current) return;
      if (!balance) {
        setToolHint(t("这里太暗或已过曝，请点一处中性灰。"));
        return;
      }
      setToolHint("");
      setPicker(null);
      applyEdit({ ...latest(), ...balance }, true);
    } catch (reason) {
      setError((reason as Error).message);
    }
  };

  const renderSlider = (slider: SliderDef) => {
    const resting = slider.fallback ?? 0;
    const value = adjustments[slider.key] ?? resting;
    return (
      <label className={`slider ${value !== resting ? "changed" : ""}`} key={slider.key}>
        <span>{slider.label}</span>
        <output>{value.toFixed(2)}</output>
        <input
          aria-label={slider.label}
          type="range"
          min={slider.min}
          max={slider.max}
          step={0.01}
          value={value}
          disabled={!loaded || locked}
          onChange={(event) => setValue(slider.key, Number(event.target.value))}
          onDoubleClick={() => setValue(slider.key, resting)}
        />
      </label>
    );
  };

  const updateProof = (patch: Partial<ProofSettings>) =>
    setProof((current) => {
      const next = { ...current, ...patch };
      saveProof(next);
      return next;
    });
  const proofProfile = profiles.find((profile) => profile.id === proof.profileId) ?? null;
  const proofActive = proof.on && proofProfile !== null;
  const toggleProofRef = useRef(() => {});
  toggleProofRef.current = () => proofProfile && updateProof({ on: !proof.on });

  useEffect(() => {
    if (!ready) return;
    const engine = engineRef.current!;
    const options = { perceptual: proof.perceptual, paper: proof.paper, gamut: proof.gamut };
    engine.setProof(proofActive ? proofProfile.bytes : null, options).catch((reason: Error) => setError(t("软打样失败：{message}", { message: reason.message })));
  }, [ready, proofActive, proofProfile, proof.perceptual, proof.paper, proof.gamut]);

  const importProfile = async (file: File) => {
    try {
      const bytes = await file.arrayBuffer();
      const info = await engineRef.current!.describeProfile(bytes);
      const stored = await saveProfile({ ...info, name: info.name || file.name.replace(/\.(icc|icm)$/i, ""), bytes });
      setProfiles((list) => [...list, stored]);
      updateProof({ profileId: stored.id, on: true });
    } catch (reason) {
      setError(t("导入 ICC 失败：{message}", { message: (reason as Error).message }));
    }
  };

  const removeProfile = async (id: number) => {
    await deleteProfile(id).catch(() => {});
    setProfiles((list) => list.filter((profile) => profile.id !== id));
    if (proof.profileId === id) updateProof({ profileId: null, on: false });
  };

  const runAutoTone = async () => {
    const photoId = currentId;
    const ticket = openTicketRef.current;
    if (!photoId) return;
    setAutoToning(true);
    setError("");
    try {
      const suggestion = await engineRef.current!.autoTone();
      // Another photo was opened while this one was measured.
      if (ticket !== openTicketRef.current) return;
      // Keeps edits made while waiting; only the suggested settings change.
      const next = { ...(editsRef.current[photoId] ?? adjustments), ...suggestion };
      ACTION_LABELS.set(next, t("智能调色"));
      applyEdit(next, true);
    } catch (reason) {
      setError(t("智能调色失败：{message}", { message: (reason as Error).message }));
    } finally {
      setAutoToning(false);
    }
  };

  const updateOutput = (patch: Partial<OutputSettings>) => {
    const next = { ...output, ...patch };
    setOutput(next);
    saveOutput(next);
  };

  const updateWatermark = (patch: Partial<Watermark>) => updateOutput({ watermark: { ...output.watermark, ...patch } });

  const chooseLogo = async (file: File | undefined) => {
    if (!file) return;
    try {
      const bitmap = await createImageBitmap(file);
      const scale = Math.min(1, WATERMARK_LOGO_LONG_EDGE / Math.max(bitmap.width, bitmap.height));
      const canvas = document.createElement("canvas");
      canvas.width = Math.max(1, Math.round(bitmap.width * scale));
      canvas.height = Math.max(1, Math.round(bitmap.height * scale));
      canvas.getContext("2d")!.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
      bitmap.close();
      updateWatermark({ logo: canvas.toDataURL("image/png") });
    } catch {
      setError(t("无法读取这张水印图片"));
    }
  };

  const moveCompare = (event: React.PointerEvent<HTMLElement>) => {
    const stage = stageRef.current;
    if (!compareDragRef.current || !stage) return;
    const bounds = stage.getBoundingClientRect();
    const box = frameBox(bounds.width, bounds.height);
    setCompare(Math.min(0.99, Math.max(0.01, (event.clientX - bounds.left - box.left) / box.width)));
  };

  const flushDabs = () => {
    if (dabTimerRef.current !== null) window.clearTimeout(dabTimerRef.current);
    dabTimerRef.current = null;
    const dabs = pendingDabsRef.current;
    pendingDabsRef.current = [];
    if (dabs.length > 0) void engineRef.current?.addHeals(dabs).catch((reason: Error) => setError(reason.message));
  };

  const stampAlong = (point: { x: number; y: number }) => {
    const stamp = stampRef.current;
    if (!stamp.offset || !stamp.source || !loaded) return;
    const longEdge = Math.max(loaded.width, loaded.height);
    const toLong = (dx: number, dy: number) => [(dx * loaded.width) / longEdge, (dy * loaded.height) / longEdge];
    const previous = stamp.last ?? point;
    const [travelX, travelY] = toLong(point.x - previous.x, point.y - previous.y);
    const travel = Math.hypot(travelX, travelY);
    const spacing = healRadius * STAMP_SPACING;
    if (stamp.last && travel < spacing) return;
    const steps = Math.max(1, Math.floor(travel / spacing));
    const [sourceDx, sourceDy] = toLong(stamp.offset.x, stamp.offset.y);
    const dabs: HealSpot[] = [];
    for (let step = 1; step <= steps; step++) {
      const at = stamp.last ? step / steps : 1;
      dabs.push({ x: previous.x + (point.x - previous.x) * at, y: previous.y + (point.y - previous.y) * at, radius: healRadius, sourceDx, sourceDy, clone: true, group: stamp.group });
    }
    stamp.last = point;
    stamp.source = { x: point.x + stamp.offset.x, y: point.y + stamp.offset.y };
    setStampMarker(stamp.source);
    setHeals((list) => [...list, ...dabs]);
    pendingDabsRef.current.push(...dabs);
    if (dabTimerRef.current === null) dabTimerRef.current = window.setTimeout(flushDabs, STAMP_FLUSH_MS);
  };

  const startStamp = (event: React.PointerEvent<HTMLElement>) => {
    if (!healing || healMode !== "clone") return;
    const point = pointerToSource(event);
    if (!point) return;
    const stamp = stampRef.current;
    if (event.altKey) {
      stamp.source = point;
      stamp.offset = null;
      setStampMarker(point);
      setToolHint("");
      return;
    }
    if (!stamp.source) {
      setToolHint(t("先按住 Alt（Option）点击要取样的位置。"));
      return;
    }
    event.currentTarget.setPointerCapture?.(event.pointerId);
    // The offset stays until a new sample point is set, so strokes line up.
    stamp.offset ??= { x: stamp.source.x - point.x, y: stamp.source.y - point.y };
    stamp.group = Date.now();
    stamp.last = null;
    stamp.drawing = true;
    stampAlong(point);
  };

  const continueStamp = (event: React.PointerEvent<HTMLElement>) => {
    if (!stampRef.current.drawing) return;
    const point = pointerToSource(event);
    if (point) stampAlong(point);
  };

  const finishStamp = () => {
    const stamp = stampRef.current;
    if (!stamp.drawing || !currentId) return;
    stamp.drawing = false;
    flushDabs();
    setHeals((list) => {
      saveHeals(currentId, list);
      return list;
    });
    void refreshThumbnail(currentId).catch(() => {});
  };

  const paintErase = (point: { x: number; y: number }) => {
    if (!loaded) return;
    const stroke = eraseStrokeRef.current;
    const longEdge = Math.max(loaded.width, loaded.height);
    const previous = stroke.last ?? point;
    const travel = Math.hypot(((point.x - previous.x) * loaded.width) / longEdge, ((point.y - previous.y) * loaded.height) / longEdge);
    const spacing = healRadius * ERASE_SPACING;
    if (stroke.last && travel < spacing) return;
    const steps = Math.max(1, Math.floor(travel / spacing));
    const dabs = Array.from({ length: steps }, (_, step) => {
      const at = (step + 1) / steps;
      return { x: previous.x + (point.x - previous.x) * at, y: previous.y + (point.y - previous.y) * at, radius: healRadius };
    });
    stroke.last = point;
    setEraseDabs((list) => [...list, ...dabs]);
  };

  const startErase = (event: React.PointerEvent<HTMLElement>) => {
    if (!healing || healMode !== "erase" || locked) return;
    const point = pointerToSource(event);
    if (!point) return;
    event.currentTarget.setPointerCapture?.(event.pointerId);
    eraseStrokeRef.current = { drawing: true, last: null };
    paintErase(point);
  };

  const continueErase = (event: React.PointerEvent<HTMLElement>) => {
    if (!eraseStrokeRef.current.drawing) return;
    const point = pointerToSource(event);
    if (point) paintErase(point);
  };

  const finishErase = () => {
    eraseStrokeRef.current.drawing = false;
  };

  const commitErases = async (next: ErasePatch[]) => {
    if (!currentId) return;
    erasesRef.current = next;
    setEraseCount(next.length);
    await saveErases(currentId, next);
    await engineRef.current!.setErases(next);
    void refreshThumbnail(currentId).catch(() => {});
  };

  const runErase = async () => {
    if (!loaded || eraseDabs.length === 0 || locked) return;
    const { width: fullWidth, height: fullHeight } = loaded;
    setErasing(true);
    setError("");
    try {
      const crop = eraseCrop(eraseDabs, fullWidth, fullHeight);
      const { mask, alpha } = rasterizeErase(eraseDabs, crop, fullWidth, fullHeight);
      const region = { x: crop.x / fullWidth, y: crop.y / fullHeight, width: crop.width / fullWidth, height: crop.height / fullHeight };
      const { rgb } = await engineRef.current!.inpaint(region, mask);
      await commitErases([
        ...erasesRef.current,
        {
          ...region,
          size: INPAINT_MODEL.size,
          rgb,
          alpha,
        },
      ]);
      setEraseDabs([]);
    } catch (reason) {
      setError(t("消除失败：{message}", { message: (reason as Error).message }));
    } finally {
      setErasing(false);
    }
  };

  const addAiLocal = async (kind: AiLocalKind) => {
    if (!loaded || localsRef.current.length >= MAX_LOCAL_LAYERS) return;
    const ticket = openTicketRef.current;
    const centre = { x: 0.5, y: 0.5 };
    const layer: LocalLayer = { kind, a: centre, b: centre, strokes: [], radius: localBrush, invert: false };
    setDetecting(true);
    setError("");
    try {
      await ensureModelMasks([layer], loaded.proxyWidth, loaded.proxyHeight);
      if (ticket !== openTicketRef.current) return;
      const layers = localsRef.current;
      commitLocals([...layers, layer]);
      setActiveLocal(layers.length);
      setLocalTool(null);
      setLocalRefine(null);
      // Starts visible, so what was found can be judged straight away.
      const latest = (currentId && editsRef.current[currentId]) || adjustments;
      applyEdit({ ...latest, [`local${layers.length}Exposure`]: 0.5 }, true);
      if (currentId) void refreshThumbnail(currentId).catch(() => {});
    } catch (reason) {
      setError((reason as Error).message);
    } finally {
      setDetecting(false);
    }
  };

  const chooseLocalRefine = (mode: "add" | "subtract") => {
    if (cropEditing) setValue("cropEditing", 0);
    setPicker(null);
    setHealing(false);
    setLiquifying(false);
    setLocalTool(null);
    setLocalRefine((current) => (current === mode ? null : mode));
  };

  const addFullLocal = () => {
    const layers = localsRef.current;
    if (layers.length >= MAX_LOCAL_LAYERS) return;
    const centre = { x: 0.5, y: 0.5 };
    const index = layers.length;
    commitLocals([...layers, { kind: "full", a: centre, b: centre, strokes: [], radius: localBrush, invert: false }]);
    setActiveLocal(index);
    setLocalTool(null);
    setLocalRefine(null);
    applyEdit({ ...adjustments, [`local${index}Exposure`]: 0.5, [`local${index}RangeMode`]: 1 }, true);
  };

  const commitLocals = (next: LocalLayer[]) => {
    localsRef.current = next;
    setLocals(next);
    scheduleLocalMask();
    if (currentId) saveLocals(currentId, next);
  };

  /** A stage position in source UV, or null outside the displayed image. */
  const pointerToSource = (event: React.MouseEvent<HTMLElement>) => {
    const bounds = event.currentTarget.getBoundingClientRect();
    // Measured now: layout state can lag behind, and does not update in a hidden page.
    const box = frameBox(bounds.width, bounds.height);
    const s = (event.clientX - bounds.left - box.left) / box.width;
    const t = (event.clientY - bounds.top - box.top) / box.height;
    if (!loaded || s < 0 || s > 1 || t < 0 || t > 1) return null;
    const shown = outputToSource(s, t, adjustments, loaded.width, loaded.height);
    return shownToSource(lens, warpRef.current, shown.x, shown.y);
  };

  const findReplaceMask = async (mode: number) => {
    if (!loaded) return;
    const ticket = openTicketRef.current;
    setDetecting(true);
    setError("");
    try {
      if (mode === REPLACE_BACKGROUND) {
        const stats = await engineRef.current!.detectSubject(loaded.proxyWidth, loaded.proxyHeight);
        if (ticket === openTicketRef.current) setMaskStats(stats);
      } else if (mode === REPLACE_SKY) {
        await engineRef.current!.detectSky();
      }
    } catch (reason) {
      setError((reason as Error).message);
    } finally {
      setDetecting(false);
      if (currentId) void refreshThumbnail(currentId).catch(() => {});
    }
  };

  const chooseReplacement = (layer: ReplaceLayer, asset: number) => {
    const turningOn = asset > 0 && !adjustments[layer.asset];
    const patch: Adjustments = { [layer.asset]: asset };
    if (turningOn) {
      patch[layer.amount] = adjustments[layer.amount] || 1;
      if (layer.mode === REPLACE_SKY) patch.skyRelight = adjustments.skyRelight ?? DEFAULT_RELIGHT;
    }
    applyEdit({ ...adjustments, ...patch }, true);
    if (turningOn) void findReplaceMask(layer.mode);
  };

  const importReplacement = async (layer: ReplaceLayer, file: File | undefined) => {
    if (!file) return;
    try {
      await decodeReplacement(file);
      const stored = await saveAsset(file.name, layer.mode, file);
      setReplaceAssets((list) => [...list, stored]);
      chooseReplacement(layer, stored.id);
    } catch {
      setError(t("无法读取这张图片"));
    }
  };

  /** Builds a look from a reference, measured on this photo as it is without one. */
  const runColourMatch = async (reference: Blob, name: string) => {
    if (!loaded || !currentId) return;
    const engine = engineRef.current!;
    const [photoId, ticket] = [currentId, openTicketRef.current];
    setMatching(true);
    setError("");
    try {
      const bitmap = await createImageBitmap(reference);
      const scale = Math.min(1, MATCH_LONG_EDGE / Math.max(bitmap.width, bitmap.height));
      const [width, height] = [Math.max(1, Math.round(bitmap.width * scale)), Math.max(1, Math.round(bitmap.height * scale))];
      const context = new OffscreenCanvas(width, height).getContext("2d", { willReadFrequently: true })!;
      context.drawImage(bitmap, 0, 0, width, height);
      bitmap.close();
      await engine.setLook(null);
      const thumbnail = await engine.thumbnail(MATCH_LONG_EDGE, false);
      const lut = matchColours(new Uint8Array(thumbnail.rgb), 3, context.getImageData(0, 0, width, height).data, 4);
      const stored = await saveLut(t("追色 {name}", { name }), lut);
      setImportedLuts((list) => [...list, stored]);
      // The look is kept for later; it is only applied to the photo it was made from.
      if (ticket !== openTicketRef.current) return;
      applyEdit({ ...(editsRef.current[photoId] ?? adjustments), lutId: stored.id, lutAmount: 1 }, true);
    } catch (reason) {
      setError(reason instanceof DOMException ? t("浏览器读不了这个文件。RAW 请先打开一次，再用它作参考。") : t("追色失败：{message}", { message: (reason as Error).message }));
      if (ticket === openTicketRef.current) await engine.setLook(await resolveLook(lookId));
    } finally {
      setMatching(false);
    }
  };

  const matchFromStrip = async () => {
    const photo = photos.find((item) => item.id === matchReference);
    if (!photo) return;
    // A thumbnail shows its edits; otherwise read the file, which the browser cannot do for RAW.
    const reference = photo.thumbnail ? await (await fetch(photo.thumbnail)).blob() : photo.file;
    await runColourMatch(reference, photo.file.name.replace(/\.[^.]+$/, ""));
  };

  const runAnalysis = async () => {
    if (!current) return;
    const engine = engineRef.current!;
    const returnTo = current;
    const pending = photos.filter((photo) => !analyses[photo.id]);
    setCullStatus("");
    setError("");
    if (pending.length === 0) {
      setCullStatus(t("{count} 张都已分析过，共 {groups} 组。", { count: photos.length, groups: groupCount }));
      return;
    }
    flushPendingEdit();
    setBatchRunning(true);
    setAnalyzing({ done: 0, total: pending.length });
    const known = { ...analyses };
    let finished = false;
    try {
      for (const [index, photo] of pending.entries()) {
        const edit = persistable(editsRef.current[photo.id] ?? loadEdit(photo.id));
        const result = await openPhoto(photo, edit, loadHeals(photo.id), loadWarps(photo.id));
        const thumbnail = await engine.thumbnail(ANALYSIS_LONG_EDGE, false);
        const faces = await locateFaces(thumbnail, () => engine.thumbnail(ANALYSIS_DETAIL_LONG_EDGE, false));
        const captured = captureMillis(result.captureTime);
        const analysis = analyzeImage(new Uint8Array(thumbnail.rgb), thumbnail.width, thumbnail.height, faces, captured ?? photo.file.lastModified, captured !== null);
        await saveAnalysis(photo.id, analysis);
        known[photo.id] = analysis;
        setAnalyses((all) => ({ ...all, [photo.id]: analysis }));
        setAnalyzing({ done: index + 1, total: pending.length });
      }
      finished = true;
    } catch (reason) {
      setError(t("分析失败：{message}", { message: (reason as Error).message }));
    } finally {
      setBatchRunning(false);
      setAnalyzing(null);
      await selectPhoto(returnTo);
    }
    if (finished) setCullStatus(t("已分析 {count} 张，共 {groups} 组。", { count: pending.length, groups: groupPhotos(analyzedItems(photos, known)).length }));
  };

  /** Flags the best and the clearly failed shots of each group, keeping manual flags. */
  const autoPick = () => {
    let [picked, rejected] = [0, 0];
    for (const photo of photos) {
      const verdict = verdicts.get(photo.id);
      if (!verdict || flags[photo.id]) continue;
      if (verdict.best) {
        flag(photo.id, FLAG_PICK);
        picked++;
      } else if (verdict.reject) {
        flag(photo.id, FLAG_REJECT);
        rejected++;
      }
    }
    setCullStatus(t("选中 {picked} 张，淘汰 {rejected} 张。已手动标记的照片没有改动。", { picked, rejected }));
  };

  const importLut = async (file: File | undefined) => {
    if (!file) return;
    try {
      const stored = await saveLut(file.name.replace(/\.cube$/i, ""), parseCube(await file.text()));
      setImportedLuts((list) => [...list, stored]);
      applyEdit({ ...adjustments, lutId: stored.id, lutAmount: 1 }, true);
    } catch (reason) {
      setError(t("导入 LUT 失败：{message}", { message: (reason as Error).message }));
    }
  };

  const chooseLook = (id: number) =>
    applyEdit({ ...adjustments, lutId: id, lutAmount: id ? (adjustments.lutAmount || 1) : 0 }, true);

  /** Adds files chosen through a picker that hands back handles, so they can be reopened. */
  const addHandles = async (handles: FileSystemFileHandle[]) => {
    const files = await Promise.all(handles.map((handle) => handle.getFile()));
    await Promise.all(handles.map((handle, index) => rememberHandle(fileId(files[index]), handle)));
    addFiles(files);
  };

  const openPicker = async () => {
    const picker = (window as unknown as { showOpenFilePicker?: (o: object) => Promise<FileSystemFileHandle[]> })
      .showOpenFilePicker;
    if (!picker) {
      fileInputRef.current?.click();
      return;
    }
    try {
      await addHandles(await picker.call(window, { multiple: true }));
    } catch (reason) {
      if (!(reason instanceof DOMException && reason.name === "AbortError")) setError((reason as Error).message);
    }
  };

  /** Asks to read files again; browsers forget the permission between visits. */
  const grantRead = async (handles: FileSystemFileHandle[]) => {
    const granted: FileSystemFileHandle[] = [];
    for (const handle of handles) {
      const access = handle as FileSystemFileHandle & {
        requestPermission?: (d: { mode: string }) => Promise<PermissionState>;
      };
      if (!access.requestPermission || (await access.requestPermission({ mode: "read" })) === "granted") {
        granted.push(handle);
      }
    }
    return granted;
  };

  const restoreLibrary = async () => {
    const granted = await grantRead(restorable.map((entry) => entry.handle));
    setRestorable([]);
    try {
      await addHandles(granted);
    } catch (reason) {
      setError(t("恢复图库失败：{message}", { message: (reason as Error).message }));
    }
  };

  const clearLibrary = () => {
    for (const entry of restorable) void forgetHandle(entry.id);
    setRestorable([]);
  };

  const storeProject = async (project: Omit<Project, "id"> & { id?: number }) => {
    const saved = await saveProject(project);
    const others = projectsRef.current.filter((item) => item.id !== saved.id);
    projectsRef.current = [...others, saved];
    setProjects(projectsRef.current);
    return saved;
  };

  const clearWorkspace = () => {
    flushPendingEdit();
    openTicketRef.current++;
    setPhotos([]);
    setCurrentId(null);
    setLoaded(null);
    setAdjustments({});
    localsRef.current = [];
    setLocals([]);
    setProjectNotice("");
  };

  const rememberProject = (id: number | null) => {
    setActiveProject(id);
    setLastProject(id);
    saveLastProject(id);
  };

  const openProject = async (project: Project) => {
    setLibraryOpen(false);
    clearWorkspace();
    rememberProject(project.id);
    try {
      const handles = await Promise.all(project.photoIds.map((id) => recallHandle(id).catch(() => undefined)));
      const granted = await grantRead(handles.filter((handle): handle is FileSystemFileHandle => !!handle));
      const files = await Promise.all(granted.map((handle) => handle.getFile().catch(() => null)));
      const opened = files.filter((file): file is File => !!file);
      addFiles(opened);
      const missing = project.photoIds.length - opened.length;
      if (missing > 0) setProjectNotice(t("有 {count} 张没有读取权限或已移动，重新添加同一文件即可恢复修图。", { count: missing }));
    } catch (reason) {
      setError(t("打开项目失败：{message}", { message: (reason as Error).message }));
    }
  };

  const createProject = async (name: string, withCurrentPhotos: boolean) => {
    const created = await storeProject({ name, photoIds: withCurrentPhotos ? photos.map((photo) => photo.id) : [], updatedAt: Date.now() });
    if (withCurrentPhotos) {
      setLibraryOpen(false);
      rememberProject(created.id);
    } else {
      await openProject(created);
    }
  };

  const leaveProject = () => {
    clearWorkspace();
    rememberProject(null);
    setLibraryOpen(false);
  };

  const removeProject = async (project: Project) => {
    if (project.id === activeProject) leaveProject();
    await deleteProject(project.id);
    projectsRef.current = projectsRef.current.filter((item) => item.id !== project.id);
    setProjects(projectsRef.current);
    if (lastProject === project.id) rememberProject(activeProject === project.id ? null : activeProject);
    const stillUsed = new Set(projectsRef.current.flatMap((item) => item.photoIds));
    for (const id of project.photoIds) if (!stillUsed.has(id)) void forgetHandle(id);
  };

  const removeFromProject = () => {
    const project = projectsRef.current.find((item) => item.id === activeProject);
    if (!project || !current) return;
    const updated = { ...project, photoIds: project.photoIds.filter((id) => id !== current.id), cover: undefined };
    // Updated before saving, so nothing re-adds the photo while the write is under way.
    projectsRef.current = projectsRef.current.map((item) => (item.id === project.id ? updated : item));
    void storeProject(updated);
    delete coverSourceRef.current[project.id];
    if (!projectsRef.current.some((item) => item.photoIds.includes(current.id))) void forgetHandle(current.id);
    const index = photos.findIndex((photo) => photo.id === current.id);
    const rest = photos.filter((photo) => photo.id !== current.id);
    if (rest.length === 0) {
      clearWorkspace();
    } else {
      setPhotos(rest);
      void selectPhoto(rest[Math.min(index, rest.length - 1)]);
    }
  };

  const startHandle = (which: "a" | "b") => (event: React.PointerEvent<HTMLElement>) => {
    event.stopPropagation();
    event.currentTarget.setPointerCapture?.(event.pointerId);
    handleDragRef.current = which;
  };

  const moveHandle = (event: React.PointerEvent<HTMLElement>) => {
    const which = handleDragRef.current;
    const stage = stageRef.current;
    if (!which || !stage || !loaded) return;
    const bounds = stage.getBoundingClientRect();
    const box = frameBox(bounds.width, bounds.height);
    const s = (event.clientX - bounds.left - box.left) / box.width;
    const t = (event.clientY - bounds.top - box.top) / box.height;
    const shown = outputToSource(s, t, adjustments, loaded.width, loaded.height);
    const point = shownToSource(lens, warpRef.current, shown.x, shown.y);
    localsRef.current = localsRef.current.map((layer, index) => (index === activeLocal ? { ...layer, [which]: point } : layer));
    setLocals(localsRef.current);
    scheduleLocalMask();
  };

  const finishHandle = () => {
    if (!handleDragRef.current) return;
    handleDragRef.current = null;
    commitLocals(localsRef.current);
  };

  const startLocal = (event: React.PointerEvent<HTMLElement>) => {
    if (!localTool && !localRefine) return;
    const point = pointerToSource(event);
    if (!point) return;
    event.currentTarget.setPointerCapture?.(event.pointerId);
    localDragRef.current = true;
    const layers = localsRef.current;
    const active = layers[activeLocal];
    if (localRefine) {
      if (!active) {
        localDragRef.current = false;
        return;
      }
      // A layer's strokes share one size, set by the first of them.
      const untouched = active.strokes.length + (active.erase?.length ?? 0) === 0;
      const updated: LocalLayer =
        localRefine === "add"
          ? { ...active, strokes: [...active.strokes, [point]], radius: untouched ? localBrush : active.radius }
          : { ...active, erase: [...(active.erase ?? []), [point]], radius: untouched ? localBrush : active.radius };
      commitLocals(layers.map((layer, index) => (index === activeLocal ? updated : layer)));
      return;
    }
    if (!localTool) return;
    if (localTool === "brush" && active?.kind === "brush") {
      const updated = { ...active, strokes: [...active.strokes, [point]] };
      commitLocals(layers.map((layer, index) => (index === activeLocal ? updated : layer)));
      return;
    }
    if (layers.length >= MAX_LOCAL_LAYERS) {
      localDragRef.current = false;
      return;
    }
    const created: LocalLayer = {
      kind: localTool,
      a: point,
      b: point,
      strokes: localTool === "brush" ? [[point]] : [],
      radius: localBrush,
      invert: false,
    };
    const index = layers.length;
    commitLocals([...layers, created]);
    setActiveLocal(index);
    // Starts visible, so the shape can be judged while it is being drawn.
    applyEdit({ ...adjustments, [`local${index}Exposure`]: 0.5 });
  };

  const continueLocal = (event: React.PointerEvent<HTMLElement>) => {
    if (!localDragRef.current) return;
    const point = pointerToSource(event);
    if (!point) return;
    const layers = localsRef.current;
    const index = layers.length > 0 && layers[activeLocal] ? activeLocal : layers.length - 1;
    const layer = layers[index];
    if (!layer) return;
    const extended = (paths: LocalLayer["strokes"]) => [...paths.slice(0, -1), [...paths[paths.length - 1], point]];
    const updated: LocalLayer =
      localRefine === "subtract"
        ? { ...layer, erase: extended(layer.erase ?? [[point]]) }
        : localRefine === "add" || layer.kind === "brush"
          ? { ...layer, strokes: extended(layer.strokes) }
          : { ...layer, b: point };
    localsRef.current = layers.map((item, position) => (position === index ? updated : item));
    setLocals(localsRef.current);
    scheduleLocalMask();
  };

  const finishLocal = () => {
    if (!localDragRef.current) return;
    localDragRef.current = false;
    commitLocals(localsRef.current);
    if (currentId) void refreshThumbnail(currentId).catch(() => {});
  };

  const deleteLocal = () => {
    const layers = localsRef.current;
    if (!layers[activeLocal]) return;
    const next = removeLocalValues(adjustments, activeLocal, layers.length);
    if ((next.localOverlay ?? 0) > 0) next.localOverlay = 0;
    commitLocals(layers.filter((_, index) => index !== activeLocal));
    setActiveLocal(0);
    setLocalRefine(null);
    applyEdit(next, true);
  };

  const invertLocal = () =>
    commitLocals(
      localsRef.current.map((layer, index) => (index === activeLocal ? { ...layer, invert: !layer.invert } : layer)),
    );

  const startLiquify = (event: React.PointerEvent<HTMLElement>) => {
    if (!liquifying) return;
    event.currentTarget.setPointerCapture?.(event.pointerId);
    dragRef.current = { x: event.clientX, y: event.clientY, strokes: [] };
  };

  const continueLiquify = (event: React.PointerEvent<HTMLElement>) => {
    const drag = dragRef.current;
    if (!drag) return;
    if (Math.hypot(event.clientX - drag.x, event.clientY - drag.y) < MIN_LIQUIFY_STEP) return;
    const bounds = event.currentTarget.getBoundingClientRect();
    const position = (x: number, y: number) => [(x - bounds.left) / bounds.width, (y - bounds.top) / bounds.height];
    const [fromX, fromY] = position(drag.x, drag.y);
    const [toX, toY] = position(event.clientX, event.clientY);
    drag.x = event.clientX;
    drag.y = event.clientY;
    const strokes = drag.strokes;
    engineRef.current!
      .liquify({
        fromX,
        fromY,
        toX,
        toY,
        stageWidth: Math.round(bounds.width),
        stageHeight: Math.round(bounds.height),
        radius: liquifyRadius,
        strength: liquifyStrength,
        mode: liquifyMode,
      })
      .then((stroke) => {
        if (stroke) strokes.push(stroke);
      })
      .catch((reason: Error) => setError(reason.message));
  };

  const finishLiquify = async () => {
    const drag = dragRef.current;
    dragRef.current = null;
    if (!drag || !currentId) return;
    const ticket = openTicketRef.current;
    // Let replies for the last pointer moves arrive before recording the drag.
    await new Promise((resolve) => window.setTimeout(resolve, LIQUIFY_SETTLE_MS));
    if (drag.strokes.length === 0 || ticket !== openTicketRef.current) return;
    setWarpGroups((groups) => {
      const next = [...groups, drag.strokes];
      saveWarps(currentId, next);
      return next;
    });
    warpRef.current = await engineRef.current!.warpField();
    void refreshThumbnail(currentId).catch(() => {});
  };

  const replaceWarps = async (next: WarpStroke[][]) => {
    if (!currentId) return;
    setWarpGroups(next);
    saveWarps(currentId, next);
    await engineRef.current!.setWarps(next.flat());
    warpRef.current = await engineRef.current!.warpField();
    void refreshThumbnail(currentId).catch(() => {});
  };

  const healAtPointer = async (event: React.MouseEvent<HTMLElement>) => {
    if (!healing || !currentId) return;
    if (healMode === "redeye") {
      const point = pointerToSource(event);
      if (!point) return;
      const spot: HealSpot = { x: point.x, y: point.y, radius: healRadius, sourceDx: 0, sourceDy: 0, redEye: true };
      setHeals((list) => {
        const next = [...list, spot];
        saveHeals(currentId, next);
        return next;
      });
      try {
        await engineRef.current!.addHeals([spot]);
        void refreshThumbnail(currentId).catch(() => {});
      } catch (reason) {
        setError((reason as Error).message);
      }
      return;
    }
    if (healMode !== "spot") return;
    const bounds = event.currentTarget.getBoundingClientRect();
    const u = (event.clientX - bounds.left) / bounds.width;
    const v = (event.clientY - bounds.top) / bounds.height;
    const ticket = openTicketRef.current;
    try {
      const spot = await engineRef.current!.healAt(u, v, Math.round(bounds.width), Math.round(bounds.height), healRadius);
      if (!spot || ticket !== openTicketRef.current) return;
      setHeals((list) => {
        const next = [...list, spot];
        saveHeals(currentId, next);
        return next;
      });
      void refreshThumbnail(currentId).catch(() => {});
    } catch (reason) {
      setError((reason as Error).message);
    }
  };

  /** Removes the last spot, or the whole of the last stamp drag. */
  const undoHeal = () => {
    const group = heals[heals.length - 1]?.group;
    void replaceHeals(group === undefined ? heals.slice(0, -1) : heals.filter((spot) => spot.group !== group));
  };

  const replaceHeals = async (next: HealSpot[]) => {
    if (!currentId) return;
    setHeals(next);
    saveHeals(currentId, next);
    await engineRef.current!.setHeals(next);
    void refreshThumbnail(currentId).catch(() => {});
  };

  const brushDiameter =
    2 *
    (liquifying ? liquifyRadius : healRadius) *
    Math.max(loaded?.width ?? 1, loaded?.height ?? 1) *
    (displayBox.width / (imageFrame.width * shownCrop.w));

  const detectSubject = async () => {
    if (!loaded) return;
    setDetecting(true);
    setError("");
    const ticket = openTicketRef.current;
    try {
      const stats = await engineRef.current!.detectSubject(loaded.proxyWidth, loaded.proxyHeight);
      if (ticket === openTicketRef.current) setMaskStats(stats);
    } catch (reason) {
      setError(t("主体识别失败：{message}", { message: (reason as Error).message }));
    } finally {
      setDetecting(false);
    }
  };

  const findFaces = async () => {
    if (!loaded) return;
    setFindingFaces(true);
    setError("");
    const ticket = openTicketRef.current;
    try {
      const stats = await engineRef.current!.prepareSkin(loaded.proxyWidth, loaded.proxyHeight);
      if (ticket === openTicketRef.current) setSkinStats(stats);
    } catch (reason) {
      setError(t("人脸识别失败：{message}", { message: (reason as Error).message }));
    } finally {
      setFindingFaces(false);
    }
  };

  // Finished files are only held for their download link.
  useEffect(() => () => {
    if (exported) URL.revokeObjectURL(exported.url);
  }, [exported]);
  useEffect(() => () => {
    if (batchUrl) URL.revokeObjectURL(batchUrl);
  }, [batchUrl]);

  const startExport = async () => {
    if (!current) return;
    setBusy(t("导出中…"));
    setExported(null);
    try {
      const result = await engineRef.current!.exportImage(output);
      const details = FORMAT_DETAILS[output.format];
      const blob = new Blob([result.bytes], { type: details.mime });
      setExported({
        url: URL.createObjectURL(blob),
        name: exportName(current.file.name, new Set(), output.template, 1, details.extension),
        ms: result.ms,
        sizeMB: blob.size / 1048576,
      });
      setHeapMB(result.heapMB);
    } catch (reason) {
      setError((reason as Error).message);
    } finally {
      setBusy("");
    }
  };

  const savePreset = () => {
    const name = presetName.trim();
    if (!name) return;
    const kept = Object.fromEntries(Object.entries(persistable(adjustments)).filter(([key]) => !NOT_SYNCED.test(key)));
    const next = { ...presets, [name]: kept };
    setPresets(next);
    savePresets(next);
    setSelectedPreset(name);
    setPresetName("");
  };

  const applyPreset = () => {
    const preset = presets[selectedPreset];
    if (preset) applyEdit({ ...mergeSettings(preset, persistable(adjustments)), maskOverlay: adjustments.maskOverlay ?? 0 }, true);
  };

  const deletePreset = () => {
    const { [selectedPreset]: _removed, ...rest } = presets;
    setPresets(rest);
    savePresets(rest);
    setSelectedPreset("");
  };

  /** A photo's edit with `source`'s settings, keeping what belongs only to that photo. */
  const mergeSettings = (source: Adjustments, target: Adjustments) => {
    const merged: Adjustments = {};
    for (const [key, value] of Object.entries(target)) if (NOT_SYNCED.test(key)) merged[key] = value;
    for (const [key, value] of Object.entries(source)) if (!NOT_SYNCED.test(key)) merged[key] = value;
    return merged;
  };

  /** Gives photos other than the open one `source`'s settings, each undoable. */
  const applySettingsTo = (targets: Photo[], source: Adjustments) => {
    const changes: Record<string, Adjustments> = {};
    for (const photo of targets) {
      if (photo.id === currentId) continue;
      const previous = editsRef.current[photo.id] ?? loadEdit(photo.id);
      const history = (historyRef.current[photo.id] ??= { past: [], future: [] });
      history.past = [...history.past, previous].slice(-HISTORY_LIMIT);
      history.future = [];
      const merged = mergeSettings(source, previous);
      editsRef.current[photo.id] = merged;
      changes[photo.id] = merged;
    }
    saveEdits(changes);
  };

  const syncToAll = () => {
    flushPendingEdit();
    applySettingsTo(photos, persistable(adjustments));
  };

  const syncToSelected = () => {
    flushPendingEdit();
    applySettingsTo(photos.filter((photo) => selectedIds.has(photo.id)), persistable(adjustments));
  };

  const copySettings = () => {
    flushPendingEdit();
    setCopiedEdit(persistable(adjustments));
  };

  const pasteSettings = () => {
    if (!copiedEdit) return;
    applyEdit({ ...mergeSettings(copiedEdit, persistable(adjustments)), maskOverlay: adjustments.maskOverlay ?? 0 }, true);
    applySettingsTo(photos.filter((photo) => selectedIds.has(photo.id)), copiedEdit);
  };

  /** Plain click opens a photo; Cmd or Ctrl toggles it; Shift selects a run. */
  const pickPhoto = (photo: Photo, event: React.MouseEvent) => {
    if (event.metaKey || event.ctrlKey) {
      setSelectedIds((all) => {
        const next = new Set(all);
        if (!next.delete(photo.id)) next.add(photo.id);
        return next;
      });
      selectionAnchorRef.current = photo.id;
      return;
    }
    if (event.shiftKey) {
      const ids = shownPhotos.map((item) => item.id);
      const from = ids.indexOf(selectionAnchorRef.current ?? currentId ?? photo.id);
      const to = ids.indexOf(photo.id);
      setSelectedIds(new Set(ids.slice(Math.min(from, to), Math.max(from, to) + 1)));
      return;
    }
    setSelectedIds(new Set());
    selectionAnchorRef.current = photo.id;
    void selectPhoto(photo);
  };

  const label = (id: string, value: number) => {
    saveLabel(id, value);
    setLabels((all) => ({ ...all, [id]: value }));
  };
  // The key handler is set up once, so it reaches the current labels through this.
  labelKeysRef.current = (id, value) => label(id, labels[id] === value ? 0 : value);

  const storeSnapshots = (next: Snapshot[]) => {
    if (!currentId) return;
    setSnapshots(next);
    saveSnapshots(currentId, next);
  };

  const addSnapshot = () => {
    flushPendingEdit();
    storeSnapshots([...snapshots, { name: t("快照 {number}", { number: snapshots.length + 1 }), edit: persistable(adjustments) }]);
  };

  const saveOutputPreset = () => {
    const name = outputPresetName.trim();
    if (!name) return;
    const next = { ...outputPresets, [name]: output };
    setOutputPresets(next);
    saveOutputPresets(next);
    setOutputPresetName("");
  };

  const deleteOutputPreset = (name: string) => {
    const { [name]: _removed, ...rest } = outputPresets;
    setOutputPresets(rest);
    saveOutputPresets(rest);
  };


  const runBatch = async () => {
    if (shownPhotos.length === 0 || !current) return;
    let target;
    try {
      target = await chooseBatchTarget();
    } catch (reason) {
      setError((reason as Error).message);
      return;
    }
    if (!target) return;

    const engine = engineRef.current!;
    const returnTo = current;
    const started = performance.now();
    const used = new Set<string>();
    setBatchRunning(true);
    setBatchUrl(null);
    setError("");
    setBatch({ done: 0, total: shownPhotos.length, ms: 0 });
    flushPendingEdit();

    try {
      for (const [index, photo] of shownPhotos.entries()) {
        const edit = persistable(editsRef.current[photo.id] ?? loadEdit(photo.id));
        setCurrentId(photo.id);
        setAdjustments(edit);
        const result = await openPhoto(photo, edit, loadHeals(photo.id), loadWarps(photo.id));
        setLoaded(result);
        await engine.setLook(await resolveLook(edit.lutId ?? 0));
        const layers = loadLocals(photo.id);
        if (layers.length > 0) {
          await ensureModelMasks(layers, result.proxyWidth, result.proxyHeight);
          await engine.setLocalMask(
            renderLocalMask(layers, result.proxyWidth, result.proxyHeight, modelMasksRef.current),
            result.proxyWidth,
            result.proxyHeight,
          );
        }
        for (const layer of REPLACE_LAYERS) {
          const image = edit[layer.asset] ? await resolveReplace(edit[layer.asset]) : null;
          if (image) await engine.setReplaceImage(layer.mode === REPLACE_SKY, image);
        }
        if (needsSky(edit)) await engine.detectSky();
        if (needsSubject(edit)) await engine.detectSubject(result.proxyWidth, result.proxyHeight);
        if (needsSkin(edit)) await engine.prepareSkin(result.proxyWidth, result.proxyHeight);
        const { bytes, heapMB: heap } = await engine.exportImage(output);
        await target.write(exportName(photo.file.name, used, output.template, index + 1, FORMAT_DETAILS[output.format].extension), new Uint8Array(bytes));
        setHeapMB(heap);
        await refreshThumbnail(photo.id);
        setBatch({ done: index + 1, total: shownPhotos.length, ms: performance.now() - started });
      }
      setBatchUrl(await target.finish());
    } catch (reason) {
      setError(t("批量导出失败：{message}", { message: (reason as Error).message }));
    } finally {
      setBatchRunning(false);
      await selectPhoto(returnTo);
    }
  };

  const backgroundExposure = adjustments.backgroundExposure ?? 0;
  const skinReady = skinStats !== null;
  const history = currentId ? historyRef.current[currentId] : undefined;

  return (
    <div className="app">
      <header className="topbar">
        <h1>OpenPixels{IS_LITE && " Lite"}</h1>
        <button className="button ghost" disabled={!ready || locked} onClick={() => setLibraryOpen(true)}>{t("项目库")}</button>
        <button className="button ghost" disabled={!ready || locked} onClick={() => void openPicker()}>{t("添加图片")}</button>
        <input
          ref={fileInputRef}
          type="file"
          multiple
          hidden
          accept={RAW_ACCEPT}
          disabled={!ready || locked}
          onChange={(event) => {
            addFiles([...(event.target.files ?? [])]);
            event.target.value = "";
          }}
        />
        {projectOfActive && (
          <div className="project-bar" data-testid="active-project">
            <span>{projectOfActive.name}</span>
            <button className="button ghost" disabled={!current || locked} onClick={removeFromProject}>{t("移出当前图片")}</button>
          </div>
        )}
        <div className="topbar-center">
          <button className="button ghost" disabled={!history?.past.length || locked} onClick={() => stepHistory("undo")}>{t("撤销")}</button>
          <button className="button ghost" disabled={!history?.future.length || locked} onClick={() => stepHistory("redo")}>{t("重做")}</button>
          <button className="button ghost" disabled={!loaded || locked} onClick={reset}>{t("重置")}</button>
          <button
            className={`button ${proofActive ? "" : "ghost"}`}
            aria-pressed={proofActive}
            disabled={!proofProfile || locked}
            title={proofProfile ? t("按打样配置预览（S）") : t("先在“输出”页导入 ICC 配置")}
            onClick={() => updateProof({ on: !proof.on })}
          >
            {t("软打样")}
          </button>
          <button className={`button ${compare > 0 ? "" : "ghost"}`} title={t("按住 \\ 键可查看整张修图前")} disabled={!loaded || locked} onClick={() => setCompare((split) => (split > 0 ? 0 : 0.5))}>{t("对比")}</button>
        </div>
        {exported && <a className="download" href={exported.url} download={exported.name} title={exported.name}>{t("下载 {name}", { name: exported.name })}</a>}
        <select className="language" aria-label={t("界面语言")} value={currentLanguage()} onChange={(event) => setLanguage(event.target.value as Language)}>
          {LANGUAGES.map((language) => (
            <option key={language.code} value={language.code}>{language.name}</option>
          ))}
        </select>
        <button className="button" disabled={!loaded || !!busy || locked} onClick={() => void startExport()}>{t("导出当前")}</button>
      </header>
      {error && (
        <p className="error" role="alert">
          <span>{error}</span>
          <button type="button" aria-label={t("关闭错误提示")} title={t("关闭")} onClick={() => setError("")}>×</button>
        </p>
      )}
      {projectNotice && <p className="notice">{projectNotice}</p>}
      <div className="body">
      <div className="workspace">
        <div className="stage-area">
        <main
          className="stage"
          ref={stageRef}
          style={healing || liquifying ? { cursor: "none" } : localTool || localRefine || picker ? { cursor: "crosshair" } : view.zoom > 1 ? { cursor: "grab" } : undefined}
          onPointerDown={(event) => {
            panClickRef.current = false;
            if (startPan(event)) return;
            startLiquify(event);
            startLocal(event);
            startStamp(event);
            startErase(event);
          }}
          onPointerUp={() => {
            finishPan();
            void finishLiquify();
            finishLocal();
            finishStamp();
            finishErase();
          }}
          onPointerCancel={() => {
            finishPan();
            void finishLiquify();
            finishLocal();
            finishStamp();
            finishErase();
          }}
          onClick={(event) => {
            if (panClickRef.current) {
              panClickRef.current = false;
              return;
            }
            if (spaceRef.current) return;
            void healAtPointer(event);
            void pickColour(event);
          }}
          onPointerMove={(event) => {
            if (continuePan(event)) return;
            continueLocal(event);
            continueStamp(event);
            continueErase(event);
            if (!healing && !liquifying) return;
            const bounds = event.currentTarget.getBoundingClientRect();
            setPointer({ x: event.clientX - bounds.left, y: event.clientY - bounds.top });
            continueLiquify(event);
          }}
          onPointerLeave={() => setPointer(null)}
          onDragOver={(event) => event.preventDefault()}
          onDrop={(event) => {
            event.preventDefault();
            const files = [...event.dataTransfer.files];
            // Handles must be requested during the event, and let the photos be reopened.
            const pending = [...event.dataTransfer.items]
              .filter((item) => item.kind === "file")
              .map((item) =>
                (item as DataTransferItem & { getAsFileSystemHandle?: () => Promise<FileSystemHandle | null> }).getAsFileSystemHandle?.(),
              );
            void Promise.all(pending)
              .then(async (handles) => {
                const fileHandles = handles.filter((h): h is FileSystemFileHandle => h?.kind === "file");
                // Drags from other pages have no handle: such photos still open, for this session.
                if (files.length > 0 && fileHandles.length === files.length) await addHandles(fileHandles);
                else addFiles(files);
              })
              .catch(() => addFiles(files));
          }}
        >
          {toolHint && <p className="stage-hint" data-testid="tool-hint">{toolHint}</p>}
          {proofActive && loaded && (
            <p className="proof-badge" data-testid="proof-badge">
              {t("软打样：{profile}", { profile: proofProfile.name })}
              {proof.gamut && t("（粉色为超出色域）")}
            </p>
          )}
          {loaded && !cropEditing && (
            <div className="zoom-bar" role="group" aria-label={t("缩放")} onPointerDown={(event) => event.stopPropagation()} onClick={(event) => event.stopPropagation()}>
              <button type="button" className={view.zoom === 1 ? "current" : ""} title={t("适合窗口（Z 在适合和 100% 之间切换）")} onClick={() => applyView(FIT_VIEW)}>{t("适合")}</button>
              <button type="button" className={zoomPercent === 100 ? "current" : ""} onClick={() => zoomToRatio(1)}>100%</button>
              <button type="button" className={zoomPercent === 200 ? "current" : ""} onClick={() => zoomToRatio(2)}>200%</button>
              <output data-testid="zoom-percent">{zoomPercent}%</output>
            </div>
          )}
          <canvas ref={canvasRef} style={{ visibility: loaded ? undefined : "hidden" }} />
          {!loaded && !busy && (
            <div className="empty" onPointerDown={(event) => event.stopPropagation()} onClick={(event) => event.stopPropagation()}>
              <p>{ready ? t("拖入一组 RAW、JPEG、PNG 或 HEIC") : t("正在初始化 WebGPU…")}</p>
              {ready && <button className="button" onClick={() => void openPicker()}>{t("选择图片")}</button>}
              {resumable && photos.length === 0 && (
                <div className="restore" data-testid="resume-project">
                  <span>{t("上次的项目：{name}（{count} 张）", { name: resumable.name, count: resumable.photoIds.length })}</span>
                  <button className="button secondary" disabled={!ready} onClick={() => void openProject(resumable)}>{t("打开")}</button>
                </div>
              )}
              {restorable.length > 0 && photos.length === 0 && (
                <div className="restore" data-testid="restore">
                  <span>{t("上次有 {count} 张图片", { count: restorable.length })}</span>
                  <button className="button secondary" disabled={!ready} onClick={() => void restoreLibrary()}>{t("恢复")}</button>
                  <button className="button ghost" onClick={clearLibrary}>{t("忽略")}</button>
                </div>
              )}
            </div>
          )}
          {busy && <p className="hint">{busy}</p>}
          {(adjustments.localOverlay ?? 0) > 0 && loaded && locals[activeLocal] && hasLocalHandles(locals[activeLocal].kind) &&
            (["a", "b"] as const).map((which) => {
              const handle = locals[activeLocal][which];
              const corrected = sourceToShown(lens, warpRef.current, handle.x, handle.y);
              const point = sourceToOutput(corrected.x, corrected.y, adjustments, loaded.width, loaded.height);
              return (
                <div
                  key={which}
                  className={`local-handle ${which === "b" ? "end" : ""}`}
                  data-local-handle={which}
                  style={{ left: displayBox.left + point.x * displayBox.width, top: displayBox.top + point.y * displayBox.height }}
                  onPointerDown={startHandle(which)}
                  onPointerMove={moveHandle}
                  onPointerUp={finishHandle}
                  onPointerCancel={finishHandle}
                />
              );
            })}
          {cropEditing && loaded && (
            <CropOverlay
              box={displayBox}
              crop={crop}
              ratio={cropRatio}
              frameWidth={imageFrame.width}
              frameHeight={imageFrame.height}
              onChange={updateCrop}
            />
          )}
          {loaded && output.watermark.enabled && <WatermarkPreview mark={output.watermark} box={displayBox} />}
          {loaded && compare > 0 && compare <= 1 && (
            <div
              className="compare-line"
              data-testid="compare-line"
              style={{ left: displayBox.left + compare * displayBox.width, top: displayBox.top, height: displayBox.height }}
              onPointerDown={(event) => {
                event.stopPropagation();
                event.currentTarget.setPointerCapture?.(event.pointerId);
                compareDragRef.current = true;
              }}
              onPointerMove={moveCompare}
              onPointerUp={() => { compareDragRef.current = false; }}
              onPointerCancel={() => { compareDragRef.current = false; }}
              onClick={(event) => event.stopPropagation()}
            >
              <span className="compare-label before">{t("修图前")}</span>
              <span className="compare-label after">{t("修图后")}</span>
            </div>
          )}
          {healing && healMode === "erase" && eraseDabs.length > 0 && loaded && (
            <svg
              className="erase-overlay"
              data-testid="erase-overlay"
              style={{ left: displayBox.left, top: displayBox.top, width: displayBox.width, height: displayBox.height }}
            >
              <g opacity={0.55} fill="#ff3b30">
                {eraseDabs.map((dab, index) => {
                  const corrected = sourceToShown(lens, warpRef.current, dab.x, dab.y);
                  const point = sourceToOutput(corrected.x, corrected.y, adjustments, loaded.width, loaded.height);
                  return <circle key={index} cx={point.x * displayBox.width} cy={point.y * displayBox.height} r={(brushDiameter / 2) * (dab.radius / healRadius)} />;
                })}
              </g>
            </svg>
          )}
          {healing && healMode === "clone" && stampMarker && loaded && (() => {
            const corrected = sourceToShown(lens, warpRef.current, stampMarker.x, stampMarker.y);
            const point = sourceToOutput(corrected.x, corrected.y, adjustments, loaded.width, loaded.height);
            return (
              <div
                className="heal-cursor stamp-source"
                data-testid="stamp-source"
                style={{ left: displayBox.left + point.x * displayBox.width, top: displayBox.top + point.y * displayBox.height, width: brushDiameter, height: brushDiameter }}
              />
            );
          })()}
          {(healing || liquifying) && pointer && loaded && (
            <div
              className="heal-cursor"
              style={{
                left: pointer.x,
                top: pointer.y,
                width: brushDiameter,
                height: brushDiameter,
              }}
            />
          )}
        </main>
        {gridView && (
          <div className="grid-view" role="listbox" aria-label={t("图库网格")} aria-multiselectable="true">
            {shownPhotos.map((photo) => (
              <button
                key={photo.id}
                className={`grid-cell ${photo.id === currentId ? "current" : ""} ${selectedIds.has(photo.id) ? "selected" : ""} ${flags[photo.id] === FLAG_REJECT ? "rejected" : ""}`}
                disabled={locked}
                title={photo.file.name}
                onClick={(event) => pickPhoto(photo, event)}
                onDoubleClick={() => setGridView(false)}
              >
                <span className="grid-image">
                  {photo.thumbnail ? <img src={photo.thumbnail} alt={photo.file.name} /> : <span>{photo.file.name}</span>}
                </span>
                <span className="grid-meta">
                  <span className="grid-name">{photo.file.name}</span>
                  {(ratings[photo.id] ?? 0) > 0 && <span className="grid-stars">{"★".repeat(ratings[photo.id])}</span>}
                </span>
                {labels[photo.id] > 0 && <i className="thumb-label" style={{ background: COLOR_LABELS[labels[photo.id] - 1].color }} />}
                {flags[photo.id] === FLAG_PICK && <i className="thumb-flag pick" aria-label={t("已选中")} />}
                {flags[photo.id] === FLAG_REJECT && <i className="thumb-flag reject" aria-label={t("已淘汰")} />}
              </button>
            ))}
          </div>
        )}
        </div>

        {photos.length > 0 && (
          <div className="filmbar" role="toolbar" aria-label={t("选片")}>
            <button className={`button ${gridView ? "" : "ghost"}`} title={t("G 网格，E 单张")} onClick={() => setGridView((on) => !on)}>{gridView ? t("单张") : t("网格")}</button>
            <span className="filmbar-gap" />
            <div className="stars" role="group" aria-label={t("星标")}>
              {[1, 2, 3, 4, 5].map((star) => (
                <button
                  key={star}
                  className={`star ${(current ? (ratings[current.id] ?? 0) : 0) >= star ? "on" : ""}`}
                  aria-label={t("{count} 星", { count: star })}
                  disabled={!current || locked}
                  onClick={() => current && rate(current.id, ratings[current.id] === star ? 0 : star)}
                >
                  ★
                </button>
              ))}
            </div>
            <button className={`button ${current && flags[current.id] === FLAG_PICK ? "" : "ghost"}`} aria-label={t("选中")} title={t("快捷键 P")} disabled={!current || locked} onClick={() => current && flag(current.id, flags[current.id] === FLAG_PICK ? 0 : FLAG_PICK)}>{t("选中")}</button>
            <button className={`button ${current && flags[current.id] === FLAG_REJECT ? "" : "ghost"}`} aria-label={t("淘汰")} title={t("快捷键 X")} disabled={!current || locked} onClick={() => current && flag(current.id, flags[current.id] === FLAG_REJECT ? 0 : FLAG_REJECT)}>{t("淘汰")}</button>
            <div className="labels" role="group" aria-label={t("色标")}>
              {COLOR_LABELS.map((item, index) => (
                <button
                  key={item.name}
                  className={current && labels[current.id] === index + 1 ? "on" : ""}
                  style={{ background: item.color }}
                  aria-label={t("{color}色标", { color: item.name })}
                  title={index < 4 ? t("快捷键 {key}", { key: index + 6 }) : undefined}
                  disabled={!current || locked}
                  onClick={() => current && label(current.id, labels[current.id] === index + 1 ? 0 : index + 1)}
                />
              ))}
            </div>
            <span className="filmbar-gap" />
            <select aria-label={t("星标筛选")} value={minRating} disabled={locked} onChange={(event) => setMinRating(Number(event.target.value))}>
              <option value={0}>{t("全部星标")}</option>
              {[1, 2, 3, 4, 5].map((star) => (
                <option key={star} value={star}>{t("{count} 星及以上", { count: star })}</option>
              ))}
            </select>
            <select aria-label={t("色标筛选")} value={labelFilter} disabled={locked} onChange={(event) => setLabelFilter(Number(event.target.value))}>
              <option value={0}>{t("全部色标")}</option>
              {COLOR_LABELS.map((item, index) => (
                <option key={item.name} value={index + 1}>{t("{color}色", { color: item.name })}</option>
              ))}
            </select>
            <select aria-label={t("旗标筛选")} value={flagFilter} disabled={locked} onChange={(event) => setFlagFilter(Number(event.target.value))}>
              <option value={0}>{t("全部旗标")}</option>
              <option value={FLAG_PICK}>{t("只看选中")}</option>
              <option value={2}>{t("隐藏淘汰")}</option>
              <option value={FLAG_REJECT}>{t("只看淘汰")}</option>
            </select>
            <select aria-label={t("分组筛选")} value={groupFilter} disabled={locked || verdicts.size === 0} onChange={(event) => setGroupFilter(Number(event.target.value))}>
              <option value={0}>{t("全部分组")}</option>
              <option value={GROUP_BEST}>{t("每组最佳")}</option>
              {Array.from({ length: groupCount }, (_, index) => (
                <option key={index} value={index + 1}>{t("第 {number} 组", { number: index + 1 })}</option>
              ))}
            </select>
            <span className="filmbar-gap" />
            <button className="button ghost" disabled={!current || locked} onClick={() => void runAnalysis()}>{t("分析并分组")}</button>
            <button className="button ghost" disabled={verdicts.size === 0 || locked} onClick={autoPick}>{t("自动选片")}</button>
            {(analyzing || cullStatus) && (
              <p className="progress" data-testid="cull-status">{analyzing ? t("正在分析 {done}/{total}…", { done: analyzing.done, total: analyzing.total }) : cullStatus}</p>
            )}
            <span className="filmbar-count">{selectedIds.size > 0 ? t("已选 {selected}，{shown} / {total} 张", { selected: selectedIds.size, shown: shownPhotos.length, total: photos.length }) : t("{shown} / {total} 张", { shown: shownPhotos.length, total: photos.length })}</span>
          </div>
        )}
        {photos.length > 0 && !gridView && (
          <nav className="strip" aria-label={t("图库")}>
            {shownPhotos.map((photo) => (
              <button
                key={photo.id}
                className={`thumb ${photo.id === currentId ? "current" : ""} ${selectedIds.has(photo.id) ? "selected" : ""} ${flags[photo.id] === FLAG_REJECT ? "rejected" : ""}`}
                disabled={locked}
                title={photo.file.name}
                onClick={(event) => pickPhoto(photo, event)}
              >
                {labels[photo.id] > 0 && <i className="thumb-label" style={{ background: COLOR_LABELS[labels[photo.id] - 1].color }} />}
                {photo.thumbnail ? <img src={photo.thumbnail} alt={photo.file.name} /> : <span>{photo.file.name}</span>}
                {groupOf.has(photo.id) && <i className={`thumb-score ${verdicts.get(photo.id)?.best ? "best" : ""}`} title={t("分组 · 评分")}>{groupOf.get(photo.id)} · {verdicts.get(photo.id)?.score}</i>}
                {flags[photo.id] === FLAG_PICK && <i className="thumb-flag pick" aria-label={t("已选中")} />}
                {flags[photo.id] === FLAG_REJECT && <i className="thumb-flag reject" aria-label={t("已淘汰")} />}
              </button>
            ))}
          </nav>
        )}
      </div>

      {libraryOpen && (
        <ProjectLibrary
          projects={projects}
          activeId={activeProject}
          photoCount={photos.length}
          onOpen={(project) => void openProject(project)}
          onCreate={(name, withCurrent) => void createProject(name, withCurrent)}
          onRename={(project, name) => void storeProject({ ...project, name })}
          onDelete={(project) => void removeProject(project)}
          onLeave={leaveProject}
          onClose={() => setLibraryOpen(false)}
        />
      )}
      <aside className="panel">
        <Histogram
          data={loaded ? histogram : null}
          shadows={clipWarnings.shadows}
          highlights={clipWarnings.highlights}
          onToggleShadows={() => setClipWarnings((all) => ({ ...all, shadows: !all.shadows }))}
          onToggleHighlights={() => setClipWarnings((all) => ({ ...all, highlights: !all.highlights }))}
        />
        <nav className="panel-tabs" role="tablist" aria-label={t("功能")}>
          {PANEL_TABS.map((tab) => (
            <button key={tab.id} type="button" role="tab" aria-selected={panelTab === tab.id} className={panelTab === tab.id ? "current" : ""} onClick={() => setPanelTab(tab.id)}>
              {tab.label}
            </button>
          ))}
        </nav>
        <div className="panel-body">
        {GROUPS.map((group) => (
          <Section key={group.section} id={group.section} title={group.title} active={panelTab}>
            {group.section === "whiteBalance" && (
              <div className="row">
                <button className={`button ${pickingWhite ? "" : "secondary"}`} disabled={!loaded || locked} onClick={() => togglePicker("white")}>
                  {pickingWhite ? t("点击画面中性灰处…") : t("吸管")}
                </button>
              </div>
            )}
            {group.section === "tone" && (
              <div className="row">
                <button className="button secondary" disabled={!loaded || locked || autoToning} onClick={() => void runAutoTone()}>
                  {autoToning ? t("计算中…") : t("智能调色")}
                </button>
              </div>
            )}
            {group.section === "perspective" && (
              <label className="check">
                <input
                  type="checkbox"
                  aria-label={t("保留全图")}
                  checked={!!adjustments.perspectiveKeep}
                  disabled={!loaded || locked}
                  onChange={(event) => setValue("perspectiveKeep", event.target.checked ? 1 : 0)}
                />
                {t("保留全图（边缘留黑，不放大裁切）")}
              </label>
            )}
            {group.sliders.map(renderSlider)}
          </Section>
        ))}

        <Section id="camera" title={t("相机与镜头")} active={panelTab}>
          <label className="field">
            <span>{t("色彩配置")}</span>
            <select
              aria-label={t("色彩配置")}
              value={loaded?.cameraMatch && adjustments.cameraProfile ? 1 : 0}
              disabled={!loaded?.cameraMatch || locked}
              onChange={(event) => setValue("cameraProfile", Number(event.target.value))}
            >
              <option value={0}>{t("标准")}</option>
              <option value={1}>{t("相机匹配")}</option>
            </select>
          </label>
          <label className="check">
            <input
              type="checkbox"
              aria-label={t("使用自动镜头校正")}
              checked={!!loaded?.lens && !!adjustments.lensAuto}
              disabled={!loaded?.lens || locked}
              onChange={(event) => setValue("lensAuto", event.target.checked ? 1 : 0)}
            />
            {t("使用自动镜头校正")}
          </label>
          {loaded?.isRaw && (
            <p className="progress" data-testid="camera-status">
              {loaded.cameraMatch ? t("已按机内 JPEG 拟合相机色彩。") : t("默认色彩已接近机内 JPEG，无需相机匹配。")}
              {loaded.lensSource === "lensfun"
                ? t("Lensfun 镜头库有 {lens} 的校正数据（CC BY-SA 3.0），勾选上方选项后生效。", { lens: loaded.lensQuery?.lens ?? "" })
                : loaded.lens
                  ? t("文件内含镜头数据。")
                  : loaded.lensQuery
                    ? t("镜头库里没有 {lens} 的数据，可用下方滑块手动校正。", { lens: loaded.lensQuery.lens })
                    : t("文件没有可用的镜头数据，可用下方滑块手动校正。")}
            </p>
          )}
        </Section>

        <Section id="curve" title={t("曲线")} active={panelTab}>
          <ToneCurve adjustments={adjustments} histogram={loaded ? histogram : null} disabled={!loaded || locked} onChange={applyEdit} />
          {PARAMETRIC_SLIDERS.map(renderSlider)}
        </Section>

        <Section id="look" title={IS_LITE ? t("滤镜") : t("滤镜与追色")} active={panelTab}>
          <div className="row">
            <select aria-label={t("滤镜")} value={lookId} disabled={!loaded || locked} onChange={(event) => chooseLook(Number(event.target.value))}>
              <option value={0}>{t("无")}</option>
              {LOOK_GROUPS.map((group) => (
                <optgroup key={group} label={group}>
                  {BUILT_IN_LOOKS.filter((look) => look.group === group).map((look) => (
                    <option key={look.id} value={look.id}>{look.name}</option>
                  ))}
                </optgroup>
              ))}
              {importedLuts.map((lut) => (
                <option key={lut.id} value={lut.id}>{lut.name}</option>
              ))}
            </select>
            <label className={`button secondary ${!loaded || locked ? "disabled" : ""}`}>
              {t("导入 .cube")}
              <input
                type="file"
                hidden
                accept=".cube"
                aria-label={t("导入 LUT")}
                disabled={!loaded || locked}
                onChange={(event) => {
                  void importLut(event.target.files?.[0]);
                  event.target.value = "";
                }}
              />
            </label>
          </div>
          <label className="slider">
            <span>{t("滤镜强度")}</span>
            <output>{(adjustments.lutAmount ?? 0).toFixed(2)}</output>
            <input aria-label={t("滤镜强度")} type="range" min={0} max={1} step={0.01} value={adjustments.lutAmount ?? 0} disabled={!lookId || locked} onChange={(event) => setValue("lutAmount", Number(event.target.value))} />
          </label>
          {!IS_LITE && (
          <div className="row">
            <select aria-label={t("追色参考")} value={matchReference} disabled={!loaded || locked || matching} onChange={(event) => setMatchReference(event.target.value)}>
              <option value="">{t("选择参考照片")}</option>
              {photos.filter((photo) => photo.id !== currentId).map((photo) => (
                <option key={photo.id} value={photo.id}>{photo.file.name}</option>
              ))}
            </select>
            <button className="button secondary" disabled={!matchReference || !loaded || locked || matching} onClick={() => void matchFromStrip()}>{matching ? t("追色中…") : t("追色")}</button>
            <label className={`button secondary ${!loaded || locked || matching ? "disabled" : ""}`}>
              {t("导入参考图")}
              <input
                type="file"
                hidden
                accept="image/*"
                aria-label={t("导入追色参考图")}
                disabled={!loaded || locked || matching}
                onChange={(event) => {
                  const file = event.target.files?.[0];
                  if (file) void runColourMatch(file, file.name.replace(/\.[^.]+$/, ""));
                  event.target.value = "";
                }}
              />
            </label>
          </div>
          )}
        </Section>

        <Section id="grade" title={t("色调分离")} active={panelTab}>
          <div className="wheels">
            {GRADE_RANGES.map(({ key, label }) => (
              <ColorWheel
                key={key}
                label={label}
                hue={adjustments[`grade${key}Hue`] ?? 0}
                amount={adjustments[`grade${key}Amount`] ?? 0}
                disabled={!loaded || locked}
                onChange={(hue, amount) => applyEdit({ ...adjustments, [`grade${key}Hue`]: hue, [`grade${key}Amount`]: amount })}
              />
            ))}
          </div>
        </Section>

        <Section id="hsl" title="HSL" active={panelTab}>
          <div className="bands" role="group" aria-label={t("颜色通道")}>
            {HSL_BANDS.map((band, index) => (
              <button
                key={band.name}
                className={`band ${index === hslBand ? "current" : ""}`}
                style={{ background: band.color }}
                aria-label={band.name}
                title={band.name}
                onClick={() => setHslBand(index)}
              />
            ))}
          </div>
          {HSL_CHANNELS.map(({ prefix, label }) => {
            const key = `${prefix}${hslBand}`;
            const value = adjustments[key] ?? 0;
            const name = t("{band}{channel}", { band: HSL_BANDS[hslBand].name, channel: label });
            return (
              <label className="slider" key={prefix}>
                <span>{label}</span>
                <output>{value.toFixed(2)}</output>
                <input
                  aria-label={name}
                  type="range"
                  min={-1}
                  max={1}
                  step={0.01}
                  value={value}
                  disabled={!loaded || locked}
                  onChange={(event) => setValue(key, Number(event.target.value))}
                  onDoubleClick={() => setValue(key, 0)}
                />
              </label>
            );
          })}
          <h3 className="subhead">{t("颜色混合点")}</h3>
          <div className="row">
            <button className={`button ${picker === "point" ? "" : "secondary"}`} disabled={!loaded || locked} onClick={() => togglePicker("point")}>
              {picker === "point" ? t("点击画面要调的颜色…") : (adjustments.pointWidth ?? 0) > 0 ? t("重新取色") : t("吸管取色")}
            </button>
            {(adjustments.pointWidth ?? 0) > 0 && (
              <>
                <span className="point-swatch" data-testid="point-swatch" style={{ background: `hsl(${adjustments.pointHue ?? 0} 75% 50%)` }} />
                <button className="button secondary" disabled={locked} onClick={removePointColour}>{t("移除")}</button>
              </>
            )}
          </div>
          {(adjustments.pointWidth ?? 0) > 0 && POINT_SLIDERS.map(renderSlider)}
        </Section>

        <Section id="subject" title={t("AI 主体")} active={panelTab}>
          <button className="button" disabled={!loaded || detecting || locked} onClick={() => void detectSubject()}>
            {detecting ? t("识别中…") : maskStats ? t("重新识别") : t("识别主体")}
          </button>
          <label className="check">
            <input
              type="checkbox"
              aria-label={t("显示蒙版")}
              disabled={!maskStats || locked}
              checked={(adjustments.maskOverlay ?? 0) > 0}
              onChange={(event) => setValue("maskOverlay", event.target.checked ? 1 : 0)}
            />
            {t("显示蒙版（背景标红）")}
          </label>
          <label className="slider">
            <span>{t("背景曝光")}</span>
            <output>{backgroundExposure.toFixed(2)}</output>
            <input
              aria-label={t("背景曝光")}
              type="range"
              min={-4}
              max={4}
              step={0.01}
              value={backgroundExposure}
              disabled={!maskStats || locked}
              onChange={(event) => setValue("backgroundExposure", Number(event.target.value))}
              onDoubleClick={() => setValue("backgroundExposure", 0)}
            />
          </label>
        </Section>

        <Section id="replace" title={t("换背景 / 换天")} active={panelTab}>
          {REPLACE_LAYERS.map((layer) => {
            const asset = adjustments[layer.asset] ?? 0;
            return (
              <div key={layer.mode}>
                <div className="row">
                  <span className="layer-title">{layer.title}</span>
                  <select aria-label={t("{layer}素材", { layer: layer.title })} value={asset} disabled={!loaded || locked} onChange={(event) => chooseReplacement(layer, Number(event.target.value))}>
                    <option value={0}>{t("不替换")}</option>
                    {BUILT_IN_REPLACEMENTS.filter((item) => item.mode === layer.mode).map((item) => (
                      <option key={item.id} value={item.id}>{item.name}</option>
                    ))}
                    {replaceAssets.filter((item) => item.mode === layer.mode).map((item) => (
                      <option key={item.id} value={item.id}>{item.name}</option>
                    ))}
                  </select>
                  <label className={`button secondary ${!loaded || locked ? "disabled" : ""}`}>
                    {t("导入图片")}
                    <input
                      type="file"
                      hidden
                      accept="image/*"
                      aria-label={t("导入{layer}图片", { layer: layer.title })}
                      disabled={!loaded || locked}
                      onChange={(event) => {
                        void importReplacement(layer, event.target.files?.[0]);
                        event.target.value = "";
                      }}
                    />
                  </label>
                </div>
                {asset > 0 && layer.sliders.map(({ key, label, min, max }) => (
                  <label className="slider" key={key}>
                    <span>{label}</span>
                    <output>{(adjustments[key] ?? 0).toFixed(2)}</output>
                    <input aria-label={t("{layer}{setting}", { layer: layer.title, setting: label })} type="range" min={min} max={max} step={0.01} value={adjustments[key] ?? 0} disabled={locked} onChange={(event) => setValue(key, Number(event.target.value))} />
                  </label>
                ))}
              </div>
            );
          })}
          {detecting && (backgroundAsset > 0 || skyAsset > 0) && <p className="progress">{t("正在识别…")}</p>}
        </Section>

        <Section id="crop" title={t("裁剪与旋转")} active={panelTab}>
          <div className="row">
            <button className={`button ${cropEditing ? "" : "secondary"}`} disabled={!loaded || locked} onClick={toggleCropEditing}>
              {cropEditing ? t("完成裁剪") : t("裁剪")}
            </button>
            <select
              aria-label={t("裁剪比例")}
              value={CROP_RATIOS.findIndex((item) => item.value === cropRatio)}
              disabled={!loaded || locked}
              onChange={(event) => chooseRatio(CROP_RATIOS[Number(event.target.value)].value)}
            >
              {CROP_RATIOS.map((item, index) => (
                <option key={item.label} value={index}>{item.label}</option>
              ))}
            </select>
          </div>
          <label className="slider">
            <span>{t("旋转")}</span>
            <output>{rotate.toFixed(1)}°</output>
            <input
              aria-label={t("旋转")}
              type="range"
              min={-45}
              max={45}
              step={0.1}
              value={rotate}
              disabled={!loaded || locked}
              onChange={(event) => setRotation(Number(event.target.value))}
              onDoubleClick={() => setRotation(0)}
            />
          </label>
          <div className="row">
            <button className="button secondary" disabled={!loaded || locked} onClick={() => turn(-1)}>{t("左转 90°")}</button>
            <button className="button secondary" disabled={!loaded || locked} onClick={() => turn(1)}>{t("右转 90°")}</button>
            <button className="button secondary" disabled={!loaded || locked} onClick={resetCrop}>{t("还原")}</button>
          </div>
        </Section>

        <Section id="heal" title={t("修复与消除")} active={panelTab}>
          <div className="row">
            <button className={`button ${healing ? "" : "secondary"}`} disabled={!loaded || locked} onClick={toggleHealing}>
              {healing ? t("退出修复") : t("开始修复")}
            </button>
            <select aria-label={t("修复方式")} value={healMode} disabled={!loaded || locked} onChange={(event) => setHealMode(event.target.value as "spot" | "clone" | "erase" | "redeye")}>
              <option value="spot">{t("污点修复")}</option>
              <option value="clone">{t("仿制图章")}</option>
              <option value="redeye">{t("红眼去除")}</option>
              <option value="erase">{t("AI 消除")}</option>
            </select>
          </div>
          {healMode === "erase" ? (
            <>
              <div className="row">
                <button className="button" disabled={eraseDabs.length === 0 || locked} onClick={() => void runErase()}>{erasing ? t("消除中…") : t("消除涂抹区域")}</button>
                <button className="button secondary" disabled={eraseDabs.length === 0 || locked} onClick={() => setEraseDabs([])}>{t("清空涂抹")}</button>
              </div>
              <div className="row">
                <button className="button secondary" disabled={eraseCount === 0 || locked} onClick={() => void commitErases(erasesRef.current.slice(0, -1))}>{t("撤销一步")}</button>
                <button className="button secondary" disabled={eraseCount === 0 || locked} onClick={() => void commitErases([])}>{t("清除")}</button>
              </div>
            </>
          ) : (
            <div className="row">
              <button className="button secondary" disabled={heals.length === 0 || locked} onClick={undoHeal}>{t("撤销一步")}</button>
              <button className="button secondary" disabled={heals.length === 0 || locked} onClick={() => void replaceHeals([])}>{t("清除")}</button>
            </div>
          )}
          <label className="slider">
            <span>{t("画笔大小")}</span>
            <output>{(healRadius * 100).toFixed(1)}</output>
            <input
              aria-label={t("画笔大小")}
              type="range"
              min={0.002}
              max={0.06}
              step={0.001}
              value={healRadius}
              disabled={!loaded || locked}
              onChange={(event) => setHealRadius(Number(event.target.value))}
            />
          </label>
          {healMode === "redeye" && <p className="progress">{t("把画笔调到盖住瞳孔，点击瞳孔。只会去掉明显偏红的像素。")}</p>}
          {healMode === "clone" && <p className="progress">{t("按住 Alt（Option）点击取样，再拖动涂抹。")}</p>}
          {healMode === "erase" && <p className="progress">{erasing ? t("正在计算，通常需要 10–20 秒…") : t("涂抹要去掉的内容，可以多涂几笔，再点“消除涂抹区域”。")}</p>}
          <p className="progress" data-stat="heals">{healMode === "erase" ? t("已消除 {count} 处", { count: eraseCount }) : t("已修复 {count} 处", { count: heals.length })}</p>
        </Section>

        <Section id="local" title={t("局部调整（{count}/{max}）", { count: locals.length, max: MAX_LOCAL_LAYERS })} active={panelTab}>
          <div className="row">
            {([["linear", t("线性")], ["radial", t("径向")], ["brush", t("画笔")]] as [LocalKind, string][]).map(([kind, label]) => (
              <button key={kind} className={`button ${localTool === kind ? "" : "secondary"}`} disabled={!loaded || locked} onClick={() => chooseLocalTool(kind)}>{label}</button>
            ))}
            <button className="button secondary" disabled={!loaded || locked || locals.length >= MAX_LOCAL_LAYERS} onClick={addFullLocal}>{t("全图")}</button>
          </div>
          <div className="row" role="group" aria-label={t("AI 蒙版")}>
            {([["subject", t("主体")], ["background", t("背景")], ["sky", t("天空")]] as [AiLocalKind, string][]).map(([kind, label]) => (
              <button key={kind} className="button secondary" disabled={!loaded || locked || detecting || locals.length >= MAX_LOCAL_LAYERS} onClick={() => void addAiLocal(kind)}>{label}</button>
            ))}
            {detecting && <span className="progress">{t("识别中…")}</span>}
          </div>
          {locals[activeLocal] && (isAiLocal(locals[activeLocal].kind) || locals[activeLocal].kind === "brush") && (
            <div className="row">
              <button className={`button ${localRefine === "add" ? "" : "secondary"}`} disabled={locked} onClick={() => chooseLocalRefine("add")}>{t("添加到蒙版")}</button>
              <button className={`button ${localRefine === "subtract" ? "" : "secondary"}`} disabled={locked} onClick={() => chooseLocalRefine("subtract")}>{t("从蒙版减去")}</button>
            </div>
          )}
          {(localTool === "brush" || localRefine) && (
            <label className="slider">
              <span>{t("局部画笔")}</span>
              <output>{(localBrush * 100).toFixed(1)}</output>
              <input aria-label={t("局部画笔")} type="range" min={0.005} max={0.2} step={0.005} value={localBrush} onChange={(event) => setLocalBrush(Number(event.target.value))} />
            </label>
          )}
          {locals.length > 0 && (
            <>
              <div className="row" role="group" aria-label={t("局部图层")}>
                {locals.map((layer, index) => (
                  <button key={index} className={`button ${index === activeLocal ? "" : "secondary"}`} disabled={locked} onClick={() => {
                      setActiveLocal(index);
                      setLocalRefine(null);
                    }}>
                    {{ linear: t("线"), radial: t("圆"), brush: t("笔"), full: t("全"), subject: t("主"), background: t("背"), sky: t("天") }[layer.kind]}{index + 1}
                  </button>
                ))}
              </div>
              {LOCAL_SLIDERS.map(({ suffix, label, min, max }) => {
                const key = `local${activeLocal}${suffix}`;
                const value = adjustments[key] ?? 0;
                return (
                  <label className="slider" key={suffix}>
                    <span>{label}</span>
                    <output>{value.toFixed(2)}</output>
                    <input aria-label={t("局部{setting}", { setting: label })} type="range" min={min} max={max} step={0.01} value={value} disabled={locked} onChange={(event) => setValue(key, Number(event.target.value))} onDoubleClick={() => setValue(key, 0)} />
                  </label>
                );
              })}
              <label className="field">
                <span>{t("范围")}</span>
                <select aria-label={t("局部范围")} value={adjustments[`local${activeLocal}RangeMode`] ?? 0} disabled={locked} onChange={(event) => setValue(`local${activeLocal}RangeMode`, Number(event.target.value))}>
                  <option value={0}>{t("无")}</option>
                  <option value={1}>{t("亮度范围")}</option>
                  <option value={2}>{t("色彩范围")}</option>
                </select>
              </label>
              {(adjustments[`local${activeLocal}RangeMode`] === 1
                ? [
                    { suffix: "RangeLow", label: t("最暗"), min: 0, max: 1, fallback: 0 },
                    { suffix: "RangeHigh", label: t("最亮"), min: 0.02, max: 1, fallback: 1 },
                  ]
                : adjustments[`local${activeLocal}RangeMode`] === 2
                  ? [
                      { suffix: "RangeHue", label: t("色相"), min: 0, max: 360, fallback: 0 },
                      { suffix: "RangeWidth", label: t("色相宽度"), min: 5, max: 90, fallback: 30 },
                    ]
                  : []
              ).map(({ suffix, label, min, max, fallback }) => {
                const key = `local${activeLocal}${suffix}`;
                const value = adjustments[key] ?? fallback;
                return (
                  <label className={`slider ${suffix === "RangeHue" ? "hue-slider" : ""}`} key={suffix}>
                    <span>{label}</span>
                    <output>{value.toFixed(max > 1 ? 0 : 2)}</output>
                    <input aria-label={t("局部{setting}", { setting: label })} type="range" min={min} max={max} step={max > 1 ? 1 : 0.01} value={value} disabled={locked} onChange={(event) => setValue(key, Number(event.target.value))} />
                  </label>
                );
              })}
              <div className="row">
                <button
                  className={`button ${(adjustments.localOverlay ?? 0) > 0 ? "" : "secondary"}`}
                  disabled={locked}
                  onClick={() => setValue("localOverlay", (adjustments.localOverlay ?? 0) > 0 ? 0 : activeLocal + 1)}
                >
                  {t("显示蒙版")}
                </button>
                <button className="button secondary" disabled={locked} onClick={invertLocal}>{t("反转")}</button>
                <button className="button secondary" disabled={locked} onClick={deleteLocal}>{t("删除图层")}</button>
              </div>
            </>
          )}
        </Section>

        <Section id="liquify" title={t("液化")} active={panelTab}>
          <div className="row">
            <button className={`button ${liquifying ? "" : "secondary"}`} disabled={!loaded || locked} onClick={toggleLiquify}>
              {liquifying ? t("退出液化") : t("推动变形")}
            </button>
            <select aria-label={t("液化模式")} value={liquifyMode} disabled={!loaded || locked} onChange={(event) => setLiquifyMode(Number(event.target.value))}>
              {LIQUIFY_MODES.map((mode) => (
                <option key={mode.value} value={mode.value}>{mode.label}</option>
              ))}
            </select>
            <button className="button secondary" disabled={warpGroups.length === 0 || locked} onClick={() => void replaceWarps(warpGroups.slice(0, -1))}>{t("撤销一笔")}</button>
            <button className="button secondary" disabled={warpGroups.length === 0 || locked} onClick={() => void replaceWarps([])}>{t("清除")}</button>
          </div>
          <label className="slider">
            <span>{t("液化大小")}</span>
            <output>{(liquifyRadius * 100).toFixed(1)}</output>
            <input aria-label={t("液化大小")} type="range" min={0.01} max={0.25} step={0.005} value={liquifyRadius} disabled={!loaded || locked} onChange={(event) => setLiquifyRadius(Number(event.target.value))} />
          </label>
          <label className="slider">
            <span>{t("液化强度")}</span>
            <output>{liquifyStrength.toFixed(2)}</output>
            <input aria-label={t("液化强度")} type="range" min={0.05} max={1} step={0.05} value={liquifyStrength} disabled={!loaded || locked} onChange={(event) => setLiquifyStrength(Number(event.target.value))} />
          </label>
          <p className="progress" data-stat="warps">{t("{count} 笔", { count: warpGroups.length })}</p>
        </Section>

        <Section id="skin" title={t("皮肤")} active={panelTab}>
          <button className="button" disabled={!loaded || findingFaces || locked} onClick={() => void findFaces()}>
            {findingFaces ? t("识别中…") : skinStats ? t("重新识别人脸") : t("识别人脸")}
          </button>
          {skinStats && (
            <p className="progress" data-stat="faces">
              {skinStats.faces > 0
                ? t("检测到 {count} 张人脸，{ms} ms", { count: skinStats.faces, ms: skinStats.ms.toFixed(0) })
                : t("未检测到完整人脸，按画面中的肤色处理")}
            </p>
          )}
          {[
            { key: "skinSmooth", label: t("磨皮") },
            { key: "skinBlemish", label: t("祛斑") },
          ].map(({ key, label }) => {
            const value = adjustments[key] ?? 0;
            return (
              <label className="slider" key={key}>
                <span>{label}</span>
                <output>{value.toFixed(2)}</output>
                <input
                  aria-label={label}
                  type="range"
                  min={0}
                  max={1}
                  step={0.01}
                  value={value}
                  disabled={!skinReady || locked}
                  onChange={(event) => setValue(key, Number(event.target.value))}
                  onDoubleClick={() => setValue(key, 0)}
                />
              </label>
            );
          })}
        </Section>

        <Section id="history" title={t("历史与快照")} active={panelTab}>
          <div className="row">
            <button className="button secondary" disabled={!loaded || locked} onClick={addSnapshot}>{t("保存快照")}</button>
          </div>
          {snapshots.map((snapshot, index) => (
            <div className="list-row" key={index}>
              <button className="list-item" disabled={locked} onClick={() => applyEdit({ ...snapshot.edit, maskOverlay: adjustments.maskOverlay ?? 0 }, true)}>{snapshot.name}</button>
              <button className="list-remove" aria-label={t("删除{name}", { name: snapshot.name })} disabled={locked} onClick={() => storeSnapshots(snapshots.filter((_, position) => position !== index))}>×</button>
            </div>
          ))}
          <div className="history-list" data-testid="history-list">
            {!history?.past.length && <p className="progress">{t("还没有调整。")}</p>}
            {[...(history?.past ?? [])].map((state, index, past) => ({ index, label: describeChange(state, past[index + 1] ?? adjustments), stepsBack: past.length - index })).reverse().map(({ index, label: text, stepsBack }) => (
              <button
                className="list-item"
                key={index}
                disabled={locked}
                title={t("回到这一步之前")}
                onClick={() => {
                  for (let step = 0; step < stepsBack; step++) stepHistory("undo");
                }}
              >
                {text}
              </button>
            ))}
          </div>
        </Section>

        <Section id="presets" title={t("预设")} active={panelTab}>
          <div className="row">
            <button className="button secondary" disabled={!loaded || locked} onClick={copySettings}>{t("复制设置")}</button>
            <button className="button secondary" disabled={!copiedEdit || !loaded || locked} onClick={pasteSettings}>{selectedIds.size > 0 ? t("粘贴到 {count} 张", { count: selectedIds.size + 1 }) : t("粘贴设置")}</button>
          </div>
          <div className="row">
            <select
              aria-label={t("选择预设")}
              value={selectedPreset}
              disabled={locked}
              onChange={(event) => setSelectedPreset(event.target.value)}
            >
              <option value="">{t("选择预设…")}</option>
              {Object.keys(presets).map((name) => (
                <option key={name} value={name}>{name}</option>
              ))}
            </select>
            <button className="button secondary" disabled={!selectedPreset || !loaded || locked} onClick={applyPreset}>{t("套用")}</button>
            <button className="button secondary" disabled={!selectedPreset || locked} onClick={deletePreset}>{t("删除")}</button>
          </div>
          <div className="row">
            <input
              aria-label={t("预设名称")}
              placeholder={t("预设名称")}
              value={presetName}
              disabled={locked}
              onChange={(event) => setPresetName(event.target.value)}
              onKeyDown={(event) => event.key === "Enter" && savePreset()}
            />
            <button className="button secondary" disabled={!presetName.trim() || !loaded || locked} onClick={savePreset}>{t("保存当前")}</button>
          </div>
        </Section>

        <Section id="batch" title={t("批量（{count} 张）", { count: shownPhotos.length })} active={panelTab}>
          <div className="row">
            <button className="button secondary" disabled={photos.length < 2 || locked} onClick={syncToAll}>{t("同步到全部")}</button>
            <button className="button secondary" disabled={selectedIds.size === 0 || locked} onClick={syncToSelected}>{t("同步到选中（{count}）", { count: selectedIds.size })}</button>
            <button className="button" disabled={shownPhotos.length === 0 || locked} onClick={() => void runBatch()}>{t("批量导出")}</button>
          </div>
          {batch && (
            <p className="progress" data-stat="batch">
              {t("{done}/{total}，{seconds} 秒", { done: batch.done, total: batch.total, seconds: (batch.ms / 1000).toFixed(1) })}
            </p>
          )}
          {batchUrl && <a className="download" data-stat="batch-zip" href={batchUrl} download="openpixels-export.zip">{t("下载 ZIP")}</a>}
        </Section>

        <Section id="proof" title={t("软打样")} active={panelTab}>
          <label className="check">
            <input type="checkbox" aria-label={t("显示软打样")} checked={proofActive} disabled={!proofProfile || locked} onChange={() => updateProof({ on: !proof.on })} />
            {t("按打样配置预览（S）")}
          </label>
          <div className="row">
            <select aria-label={t("打样配置")} value={proof.profileId ?? ""} disabled={locked || profiles.length === 0} onChange={(event) => updateProof({ profileId: event.target.value ? Number(event.target.value) : null })}>
              <option value="">{profiles.length ? t("选择配置…") : t("尚未导入配置")}</option>
              {profiles.map((profile) => (
                <option key={profile.id} value={profile.id}>
                  {t("{name}（{kind} {space}）", { name: profile.name, kind: profile.isOutput ? t("打印") : t("显示"), space: profile.space })}
                </option>
              ))}
            </select>
            <label className={`button secondary ${locked ? "disabled" : ""}`}>
              {t("导入 ICC")}
              <input
                type="file"
                hidden
                accept=".icc,.icm"
                aria-label={t("导入 ICC 配置")}
                disabled={locked}
                onChange={(event) => {
                  const file = event.target.files?.[0];
                  if (file) void importProfile(file);
                  event.target.value = "";
                }}
              />
            </label>
            {proofProfile && (
              <button className="button secondary" disabled={locked} onClick={() => void removeProfile(proofProfile.id)}>
                {t("删除")}
              </button>
            )}
          </div>
          <label className="field">
            <span>{t("渲染方式")}</span>
            <select aria-label={t("渲染方式")} value={proof.perceptual ? "perceptual" : "relative"} disabled={locked} onChange={(event) => updateProof({ perceptual: event.target.value === "perceptual" })}>
              <option value="perceptual">{t("可感知")}</option>
              <option value="relative">{t("相对比色")}</option>
            </select>
          </label>
          <label className="check">
            <input type="checkbox" aria-label={t("模拟纸张和油墨")} checked={proof.paper} disabled={locked} onChange={() => updateProof({ paper: !proof.paper })} />
            {t("模拟纸张和油墨")}
          </label>
          <label className="check">
            <input type="checkbox" aria-label={t("目标色域警告")} checked={proof.gamut} disabled={locked} onChange={() => updateProof({ gamut: !proof.gamut })} />
            {t("目标色域警告（粉色标出无法还原的颜色）")}
          </label>
          {profiles.length === 0 && (
            <p className="progress">
              {t("导入打印机或另一台显示器的 ICC 文件。macOS 在 /System/Library/ColorSync/Profiles，Windows 在 C:\\Windows\\System32\\spool\\drivers\\color。打样只影响预览，不改变导出。")}
            </p>
          )}
        </Section>

        <Section id="export" title={t("导出设置")} active={panelTab}>
          <div className="row">
            <select
              aria-label={t("导出预设")}
              value=""
              disabled={locked || Object.keys(outputPresets).length === 0}
              onChange={(event) => {
                const preset = outputPresets[event.target.value];
                if (preset) updateOutput(preset);
              }}
            >
              <option value="">{Object.keys(outputPresets).length > 0 ? t("套用导出预设…") : t("暂无导出预设")}</option>
              {Object.keys(outputPresets).map((name) => (
                <option key={name} value={name}>{name}</option>
              ))}
            </select>
          </div>
          <div className="row">
            <input aria-label={t("导出预设名称")} placeholder={t("导出预设名称")} value={outputPresetName} disabled={locked} onChange={(event) => setOutputPresetName(event.target.value)} onKeyDown={(event) => event.key === "Enter" && saveOutputPreset()} />
            <button className="button secondary" disabled={!outputPresetName.trim() || locked} onClick={saveOutputPreset}>{t("保存")}</button>
          </div>
          {Object.keys(outputPresets).map((name) => (
            <div className="list-row" key={name}>
              <span className="list-item static">{name}</span>
              <button className="list-remove" aria-label={t("删除导出预设{name}", { name })} disabled={locked} onClick={() => deleteOutputPreset(name)}>×</button>
            </div>
          ))}
          <div className="row">
            <select aria-label={t("导出格式")} value={output.format} disabled={locked} onChange={(event) => updateOutput({ format: event.target.value as OutputFormat })}>
              {(Object.keys(FORMAT_DETAILS) as OutputFormat[]).map((format) => (
                <option key={format} value={format}>{FORMAT_DETAILS[format].label}</option>
              ))}
            </select>
            <select aria-label={t("导出尺寸")} value={output.longEdge} disabled={locked} onChange={(event) => updateOutput({ longEdge: Number(event.target.value) })}>
              {LONG_EDGE_CHOICES.map((edge) => (
                <option key={edge} value={edge}>{edge === 0 ? t("原尺寸") : t("长边 {pixels}", { pixels: edge })}</option>
              ))}
            </select>
          </div>
          <label className="field">
            <span>{t("色彩空间")}</span>
            <select aria-label={t("色彩空间")} value={output.colorSpace ?? "srgb"} disabled={locked} onChange={(event) => updateOutput({ colorSpace: event.target.value as OutputColorSpace })}>
              <option value="srgb">{t("sRGB（网页、手机）")}</option>
              <option value="p3">Display P3</option>
              <option value="adobe">Adobe RGB (1998)</option>
              <option value="prophoto">ProPhoto RGB</option>
            </select>
          </label>
          {output.colorSpace === "prophoto" && (output.format === "jpeg" || output.bitDepth === 8) && (
            <p className="progress">{t("ProPhoto RGB 范围很宽，8 位容易出现色阶断层，建议用 16 位 PNG 或 TIFF。")}</p>
          )}
          {output.format !== "jpeg" && (
            <div className="row">
              <select aria-label={t("位深")} value={output.bitDepth} disabled={locked} onChange={(event) => updateOutput({ bitDepth: Number(event.target.value) as 8 | 16 })}>
                <option value={8}>{t("8 位")}</option>
                <option value={16}>{t("16 位")}</option>
              </select>
            </div>
          )}
          {output.format === "jpeg" && (
            <label className="slider">
              <span>{t("质量")}</span>
              <output>{output.quality}</output>
              <input aria-label={t("导出质量")} type="range" min={40} max={100} step={1} value={output.quality} disabled={locked} onChange={(event) => updateOutput({ quality: Number(event.target.value) })} />
            </label>
          )}
          <div className="row">
            <input aria-label={t("文件名规则")} title={t("可用 {name}（原文件名）、{n}（序号）、{date}（日期）")} value={output.template} disabled={locked} onChange={(event) => updateOutput({ template: event.target.value })} />
          </div>
          <p className="progress">{t("文件名可用 {name}、{n}、{date}")}</p>
          <label className="check">
            <input type="checkbox" aria-label={t("添加水印")} checked={output.watermark.enabled} disabled={locked} onChange={(event) => updateWatermark({ enabled: event.target.checked })} />
            {t("添加水印")}
          </label>
          {output.watermark.enabled && (
            <>
              <div className="row">
                <input aria-label={t("水印文字")} placeholder={t("水印文字")} value={output.watermark.text} disabled={locked} onChange={(event) => updateWatermark({ text: event.target.value })} />
              </div>
              <div className="row">
                <label className={`button secondary ${locked ? "disabled" : ""}`}>
                  {output.watermark.logo ? t("更换 Logo") : t("选择 Logo")}
                  <input
                    type="file"
                    hidden
                    accept="image/*"
                    aria-label={t("水印图片")}
                    disabled={locked}
                    onChange={(event) => {
                      void chooseLogo(event.target.files?.[0]);
                      event.target.value = "";
                    }}
                  />
                </label>
                {output.watermark.logo && <button className="button secondary" disabled={locked} onClick={() => updateWatermark({ logo: null })}>{t("移除")}</button>}
                <select aria-label={t("水印位置")} value={output.watermark.position} disabled={locked} onChange={(event) => updateWatermark({ position: event.target.value as WatermarkPosition })}>
                  {WATERMARK_POSITIONS.map((item) => (
                    <option key={item.value} value={item.value}>{item.label}</option>
                  ))}
                </select>
              </div>
              <label className="slider">
                <span>{t("水印大小")}</span>
                <output>{(output.watermark.size * 100).toFixed(1)}</output>
                <input aria-label={t("水印大小")} type="range" min={0.01} max={0.12} step={0.001} value={output.watermark.size} disabled={locked} onChange={(event) => updateWatermark({ size: Number(event.target.value) })} />
              </label>
              <label className="slider">
                <span>{t("水印不透明度")}</span>
                <output>{output.watermark.opacity.toFixed(2)}</output>
                <input aria-label={t("水印不透明度")} type="range" min={0.1} max={1} step={0.01} value={output.watermark.opacity} disabled={locked} onChange={(event) => updateWatermark({ opacity: Number(event.target.value) })} />
              </label>
            </>
          )}
        </Section>

        <Section id="stats" title={t("性能")} active={panelTab} testId="stats">
          <dl className="stats">
            {loaded && (
              <>
                <dt>{t("图像")}</dt><dd>{loaded.width}×{loaded.height}{loaded.isRaw ? " RAW" : ""}</dd>
                <dt>{t("文件")}</dt><dd>{loaded.fileMB.toFixed(0)} MB</dd>
                <dt>{t("解码")}</dt><dd data-stat="decode">{loaded.decodeMs.toFixed(0)} ms</dd>
                {loaded.baseExposure !== null && (
                  <><dt>{t("基础曝光")}</dt><dd data-stat="base-exposure">{loaded.baseExposure.toFixed(2)} EV</dd></>
                )}
                <dt>{t("代理图")}</dt><dd>{t("{width}×{height}，{ms} ms", { width: loaded.proxyWidth, height: loaded.proxyHeight, ms: loaded.proxyMs.toFixed(0) })}</dd>
              </>
            )}
            {frame && (<><dt>{t("单帧中位")}</dt><dd data-stat="frame">{frame.median.toFixed(1)} ms</dd></>)}
            {exported && (
              <>
                <dt>{t("导出")}</dt><dd data-stat="export">{exported.ms.toFixed(0)} ms</dd>
                <dt>{t("文件")}</dt><dd data-stat="export-size">{exported.sizeMB.toFixed(1)} MB</dd>
              </>
            )}
            {maskStats && (
              <>
                <dt>{t("模型输入")}</dt><dd>{maskStats.inputMs.toFixed(0)} ms</dd>
                {maskStats.loadMs > 0 && (<><dt>{t("模型加载")}</dt><dd data-stat="model-load">{maskStats.loadMs.toFixed(0)} ms</dd></>)}
                <dt>{t("推理")}</dt><dd data-stat="infer">{maskStats.inferMs.toFixed(0)} ms</dd>
              </>
            )}
            {heapMB > 0 && (<><dt>{t("WASM 堆")}</dt><dd data-stat="heap">{heapMB.toFixed(0)} MB</dd></>)}
          </dl>
        </Section>
        </div>
      </aside>
      </div>
    </div>
  );
}
