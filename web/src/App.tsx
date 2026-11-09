import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ColorWheel } from "./ColorWheel";
import { CropOverlay } from "./CropOverlay";
import { ToneCurve } from "./ToneCurve";
import { chooseBatchTarget } from "./batch";
import { EngineClient, type MaskStats, type SkinStats } from "./engine";
import { FULL_FRAME, cropFields, cropForRatio, cropOf, fitBox, frameSize, outputToSource, shrinkToImage, sourceToOutput, type Crop } from "./frame";
import { BUILT_IN_LOOKS, builtInLut, parseCube } from "./lut";
import { deleteProject, forgetHandle, getAsset, getLut, listAssets, listLuts, listProjects, loadAnalyses, loadErases, recallHandle, recallHandleEntries, rememberHandle, saveAnalysis, saveAsset, saveErases, saveLut, saveProject, type Project, type StoredAsset, type StoredLut } from "./store";
import { analyzeImage, groupPhotos, judgeGroup, type PhotoAnalysis, type Verdict } from "./analysis";
import { matchColours } from "./colormatch";
import { locateFaces } from "./face";
import { BUILT_IN_REPLACEMENTS, REPLACE_BACKGROUND, REPLACE_SKY, builtInReplacement, decodeReplacement } from "./replace";
import { eraseCrop, rasterizeErase, type EraseDab } from "./erase";
import { lensToSource, lensView, sourceToLens } from "./lens";
import { FORMAT_DETAILS, type OutputFormat, type OutputSettings, type Watermark, type WatermarkPosition } from "./output";
import { ProjectLibrary } from "./ProjectLibrary";
import { WatermarkPreview } from "./WatermarkPreview";
import { LOCAL_SLIDERS, MAX_LOCAL_LAYERS, removeLocalValues, renderLocalMask, type LocalKind, type LocalLayer } from "./local";
import {
  exportName,
  fileId,
  loadEdit,
  loadFlags,
  loadHeals,
  loadLocals,
  loadOutput,
  loadPresets,
  loadRatings,
  loadWarps,
  persistable,
  saveHeals,
  saveFlag,
  saveLocals,
  saveOutput,
  saveRating,
  saveWarps,
  savePresets,
  saveEdits,
  type Presets, loadLastProject, saveLastProject } from "./library";
import type { Adjustments, ErasePatch, HealSpot, LoadResult, Thumbnail, WarpStroke } from "./protocol";
import { INPAINT_MODEL } from "./model";

interface SliderDef {
  key: string;
  label: string;
  min: number;
  max: number;
}

const GROUPS: { title: string; sliders: SliderDef[] }[] = [
  {
    title: "白平衡",
    sliders: [
      { key: "temperature", label: "色温", min: -2, max: 2 },
      { key: "tint", label: "色调", min: -2, max: 2 },
    ],
  },
  {
    title: "影调",
    sliders: [
      { key: "exposure", label: "曝光", min: -4, max: 4 },
      { key: "contrast", label: "对比度", min: -1, max: 1 },
      { key: "highlights", label: "高光", min: -1, max: 1 },
      { key: "shadows", label: "阴影", min: -1, max: 1 },
      { key: "whites", label: "白色", min: -1, max: 1 },
      { key: "blacks", label: "黑色", min: -1, max: 1 },
    ],
  },
  {
    title: "色彩与效果",
    sliders: [
      { key: "vibrance", label: "自然饱和度", min: -1, max: 1 },
      { key: "saturation", label: "饱和度", min: -1, max: 1 },
      { key: "dehaze", label: "去雾", min: -1, max: 1 },
      { key: "clarity", label: "清晰度", min: -1, max: 1 },
      { key: "texture", label: "纹理", min: -1, max: 1 },
      { key: "sharpen", label: "锐化", min: 0, max: 1 },
      { key: "denoiseLuma", label: "明度降噪", min: 0, max: 1 },
      { key: "denoiseChroma", label: "色彩降噪", min: 0, max: 1 },
      { key: "vignette", label: "暗角", min: -1, max: 1 },
      { key: "grain", label: "颗粒", min: 0, max: 1 },
      { key: "grainSize", label: "颗粒大小", min: 0, max: 1 },
    ],
  },
  {
    title: "镜头校正",
    sliders: [
      { key: "lensDistortion", label: "畸变", min: -1, max: 1 },
      { key: "lensVignette", label: "暗角补偿", min: -1, max: 1 },
      { key: "lensCaRed", label: "红/青边", min: -1, max: 1 },
      { key: "lensCaBlue", label: "蓝/黄边", min: -1, max: 1 },
    ],
  },
  {
    title: "透视矫正",
    sliders: [
      { key: "perspectiveV", label: "垂直透视", min: -1, max: 1 },
      { key: "perspectiveH", label: "水平透视", min: -1, max: 1 },
    ],
  },
];

const HSL_BANDS = [
  { name: "红", color: "#e0483e" },
  { name: "橙", color: "#e88a2e" },
  { name: "黄", color: "#e5cb3a" },
  { name: "绿", color: "#55b04a" },
  { name: "青", color: "#3fbcc0" },
  { name: "蓝", color: "#3f73d8" },
  { name: "紫", color: "#7e52c8" },
  { name: "品红", color: "#cc4aa8" },
];

const HSL_CHANNELS = [
  { prefix: "hslHue", label: "色相" },
  { prefix: "hslSat", label: "饱和度" },
  { prefix: "hslLum", label: "明度" },
];

const GRADE_RANGES = [
  { key: "Shadow", label: "阴影" },
  { key: "Mid", label: "中间调" },
  { key: "High", label: "高光" },
];

const LIQUIFY_MODES = [
  { value: 0, label: "推动" },
  { value: 1, label: "膨胀" },
  { value: 2, label: "收缩" },
];

const CROP_RATIOS: { label: string; value: number | null }[] = [
  { label: "自由", value: null },
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
  { value: "bottomRight", label: "右下" },
  { value: "bottomLeft", label: "左下" },
  { value: "topRight", label: "右上" },
  { value: "topLeft", label: "左上" },
  { value: "center", label: "居中" },
];
const WATERMARK_LOGO_LONG_EDGE = 512;
/** Past the right edge, so the whole frame shows the unedited image. */
const COMPARE_ALL = 1.01;
const STAMP_SPACING = 0.35;
const STAMP_FLUSH_MS = 40;
const ERASE_SPACING = 0.3;
const ANALYSIS_LONG_EDGE = 1024;
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
const needsSubject = (edit: Adjustments) => (edit.backgroundExposure ?? 0) !== 0 || edit.replaceMode === REPLACE_BACKGROUND;
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
  const [pickingWhite, setPickingWhite] = useState(false);
  const [toolHint, setToolHint] = useState("");
  const [output, setOutput] = useState<OutputSettings>(loadOutput);
  const [healMode, setHealMode] = useState<"spot" | "clone" | "erase">("spot");
  const [eraseDabs, setEraseDabs] = useState<EraseDab[]>([]);
  const [eraseCount, setEraseCount] = useState(0);
  const [erasing, setErasing] = useState(false);
  const erasesRef = useRef<ErasePatch[]>([]);
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
    const items = photos.filter((photo) => analyses[photo.id]).map((photo) => ({ id: photo.id, analysis: analyses[photo.id] }));
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
  const shownPhotos = photos.filter((photo) => (ratings[photo.id] ?? 0) >= minRating && passesFlag(photo.id) && passesGroup(photo.id));
  // An erase takes seconds and lands on the photo it started on.
  const locked = batchRunning || erasing;
  const projectOfActive = projects.find((item) => item.id === activeProject) ?? null;
  const resumable = activeProject === null ? (projects.find((item) => item.id === lastProject) ?? null) : null;
  const lens = loaded ? lensView(loaded.lens, adjustments, loaded.width, loaded.height) : null;

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || engineRef.current) return;
    const engine = new EngineClient(canvas.transferControlToOffscreen());
    engineRef.current = engine;
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
    if (!engine) return;
    const ticket = openTicketRef.current;
    const thumbnail = await engine.thumbnail(THUMBNAIL_LONG_EDGE, true);
    if (ticket !== openTicketRef.current) return;
    const url = thumbnailUrl(thumbnail);
    setPhotos((list) => list.map((photo) => (photo.id === id ? { ...photo, thumbnail: url } : photo)));
  }, []);

  /** Opens a photo in the engine. Rejects with STALE if another open overtook it. */
  const openPhoto = useCallback(
    async (photo: Photo, edit: Adjustments, spots: HealSpot[], warps: WarpStroke[][]) => {
      const engine = engineRef.current!;
      const ticket = ++openTicketRef.current;
      const erases = await loadErases(photo.id);
      const result = await engine.open(await photo.file.arrayBuffer(), edit, spots, warps.flat(), erases);
      if (ticket !== openTicketRef.current) throw new Error(STALE);
      erasesRef.current = erases;
      setEraseCount(erases.length);
      return result;
    },
    [],
  );

  /** Records an edit still waiting to settle, before anything changes the context. */
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
      void engineRef.current?.setLocalMask(renderLocalMask(localsRef.current, width, height), width, height);
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
      setPickingWhite(false);
      setToolHint("");
      stampRef.current = { source: null, offset: null, last: null, group: 0, drawing: false };
      setStampMarker(null);
      setEraseDabs([]);
      setWarpGroups(warps);
      setLiquifying(false);
      const layers = loadLocals(photo.id);
      localsRef.current = layers;
      setLocals(layers);
      setActiveLocal(0);
      setLocalTool(null);
      settledRef.current = edit;
      setCurrentId(photo.id);
      setAdjustments(edit);
      setBusy("解码中…");
      setError("");
      setExported(null);
      setMaskStats(null);
      setSkinStats(null);
      setDetecting(false);
      setFindingFaces(false);
      try {
        const result = await openPhoto(photo, edit, spots, warps);
        setLoaded(result);
        setHeapMB(result.heapMB);
        proxySizeRef.current = { width: result.proxyWidth, height: result.proxyHeight };
        if (layers.length > 0) {
          await engineRef.current!.setLocalMask(
            renderLocalMask(layers, result.proxyWidth, result.proxyHeight),
            result.proxyWidth,
            result.proxyHeight,
          );
        }
        setBusy("");
        const ticket = openTicketRef.current;
        // A saved edit needs its analysis back before it can be shown.
        if (needsSubject(edit)) {
          setDetecting(true);
          const stats = await engineRef.current!.detectSubject(result.proxyWidth, result.proxyHeight);
          if (ticket === openTicketRef.current) setMaskStats(stats);
          setDetecting(false);
        }
        if (edit.replaceMode === REPLACE_SKY) {
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
  const replaceMode = adjustments.replaceMode ?? 0;
  const replaceAsset = adjustments.replaceAsset ?? 0;

  useEffect(() => {
    if (!ready || !loaded || !replaceMode) return;
    let cancelled = false;
    void resolveReplace(replaceAsset).then((image) => {
      if (!cancelled && image) void engineRef.current?.setReplaceImage(image);
    });
    return () => {
      cancelled = true;
    };
  }, [ready, loaded, replaceMode, replaceAsset, resolveReplace]);

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
        saveEdits({ [currentId]: next });
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
      to.push(settledRef.current);
      settledRef.current = target;
      setAdjustments(target);
      editsRef.current[currentId] = target;
      engineRef.current?.adjust(target);
      saveEdits({ [currentId]: target });
      void refreshThumbnail(currentId).catch(() => {});
    },
    [currentId, flushPendingEdit, refreshThumbnail],
  );

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "z") {
        event.preventDefault();
        stepHistory(event.shiftKey ? "redo" : "undo");
      }
    };
    window.addEventListener("keydown", onKey);
    const onRate = (event: KeyboardEvent) => {
      const tag = (event.target as HTMLElement).tagName;
      if (tag === "INPUT" || tag === "SELECT" || event.metaKey || event.ctrlKey) return;
      if (/^[0-5]$/.test(event.key) && currentId) rate(currentId, Number(event.key));
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

  const quarter = adjustments.quarter ?? 0;
  const rotate = adjustments.rotate ?? 0;
  const imageFrame = frameSize(loaded?.width ?? 1, loaded?.height ?? 1, quarter);
  const crop = cropOf(adjustments);
  const cropEditing = (adjustments.cropEditing ?? 0) !== 0;
  const shownCrop = cropEditing ? FULL_FRAME : crop;
  const displayBox = fitBox(
    (imageFrame.width * shownCrop.w) / (imageFrame.height * shownCrop.h),
    stageSize.width,
    stageSize.height,
  );

  const updateCrop = (next: Crop) =>
    applyEdit({ ...adjustments, ...cropFields(shrinkToImage(next, rotate, imageFrame.width, imageFrame.height)) });

  const toggleCropEditing = () => {
    setPickingWhite(false);
    setHealing(false);
    setLiquifying(false);
    setLocalTool(null);
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
    setPickingWhite(false);
    setLiquifying(false);
    setLocalTool(null);
    setHealing((on) => !on);
  };

  const toggleLiquify = () => {
    if (cropEditing) setValue("cropEditing", 0);
    setPickingWhite(false);
    setHealing(false);
    setLocalTool(null);
    setLiquifying((on) => !on);
  };

  const chooseLocalTool = (kind: LocalKind) => {
    if (cropEditing) setValue("cropEditing", 0);
    setPickingWhite(false);
    setHealing(false);
    setLiquifying(false);
    setLocalTool((current) => (current === kind ? null : kind));
  };

  const togglePickingWhite = () => {
    if (cropEditing) setValue("cropEditing", 0);
    setHealing(false);
    setLiquifying(false);
    setLocalTool(null);
    setToolHint("");
    setPickingWhite((on) => !on);
  };

  const pickWhite = async (event: React.MouseEvent<HTMLElement>) => {
    if (!pickingWhite) return;
    const bounds = event.currentTarget.getBoundingClientRect();
    const u = (event.clientX - bounds.left) / bounds.width;
    const v = (event.clientY - bounds.top) / bounds.height;
    try {
      const balance = await engineRef.current!.whiteBalanceAt(u, v, Math.round(bounds.width), Math.round(bounds.height));
      if (!balance) {
        setToolHint("这里太暗或已过曝，请点一处中性灰。");
        return;
      }
      setToolHint("");
      setPickingWhite(false);
      applyEdit({ ...adjustments, ...balance }, true);
    } catch (reason) {
      setError((reason as Error).message);
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
      setError("无法读取这张水印图片");
    }
  };

  const moveCompare = (event: React.PointerEvent<HTMLElement>) => {
    const stage = stageRef.current;
    if (!compareDragRef.current || !stage) return;
    const bounds = stage.getBoundingClientRect();
    const box = fitBox((imageFrame.width * shownCrop.w) / (imageFrame.height * shownCrop.h), bounds.width, bounds.height);
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
      setToolHint("先按住 Alt（Option）点击要取样的位置。");
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
    const { proxyWidth, proxyHeight } = loaded;
    setErasing(true);
    setError("");
    try {
      const crop = eraseCrop(eraseDabs, proxyWidth, proxyHeight);
      const { mask, alpha } = rasterizeErase(eraseDabs, crop, proxyWidth, proxyHeight);
      const { rgb } = await engineRef.current!.inpaint(crop, mask);
      await commitErases([
        ...erasesRef.current,
        {
          x: crop.x / proxyWidth,
          y: crop.y / proxyHeight,
          width: crop.width / proxyWidth,
          height: crop.height / proxyHeight,
          size: INPAINT_MODEL.size,
          rgb,
          alpha,
        },
      ]);
      setEraseDabs([]);
    } catch (reason) {
      setError(`消除失败：${(reason as Error).message}`);
    } finally {
      setErasing(false);
    }
  };

  const addFullLocal = () => {
    const layers = localsRef.current;
    if (layers.length >= MAX_LOCAL_LAYERS) return;
    const centre = { x: 0.5, y: 0.5 };
    const index = layers.length;
    commitLocals([...layers, { kind: "full", a: centre, b: centre, strokes: [], radius: localBrush, invert: false }]);
    setActiveLocal(index);
    setLocalTool(null);
    applyEdit({ ...adjustments, [`local${index}Exposure`]: 0.5, [`local${index}RangeMode`]: 1 }, true);
  };

  const commitLocals = (next: LocalLayer[]) => {
    localsRef.current = next;
    setLocals(next);
    scheduleLocalMask();
    if (currentId) saveLocals(currentId, next);
  };

  /** A stage position in source UV, or null outside the displayed image. */
  const pointerToSource = (event: React.PointerEvent<HTMLElement>) => {
    const bounds = event.currentTarget.getBoundingClientRect();
    // Measured now: layout state can lag behind, and does not update in a hidden page.
    const box = fitBox((imageFrame.width * shownCrop.w) / (imageFrame.height * shownCrop.h), bounds.width, bounds.height);
    const s = (event.clientX - bounds.left - box.left) / box.width;
    const t = (event.clientY - bounds.top - box.top) / box.height;
    if (!loaded || s < 0 || s > 1 || t < 0 || t > 1) return null;
    const shown = outputToSource(s, t, adjustments, loaded.width, loaded.height);
    return lens ? lensToSource(lens, shown.x, shown.y) : shown;
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

  const chooseReplaceMode = (mode: number) => {
    const fits = (id: number) =>
      BUILT_IN_REPLACEMENTS.some((item) => item.id === id && item.mode === mode) || replaceAssets.some((item) => item.id === id && item.mode === mode);
    const asset = fits(replaceAsset) ? replaceAsset : (BUILT_IN_REPLACEMENTS.find((item) => item.mode === mode)?.id ?? 0);
    applyEdit({ ...adjustments, replaceMode: mode, replaceAsset: asset, replaceAmount: adjustments.replaceAmount || 1 }, true);
    void findReplaceMask(mode);
  };

  const importReplacement = async (file: File | undefined) => {
    if (!file || !replaceMode) return;
    try {
      await decodeReplacement(file);
      const stored = await saveAsset(file.name, replaceMode, file);
      setReplaceAssets((list) => [...list, stored]);
      applyEdit({ ...adjustments, replaceAsset: stored.id }, true);
    } catch {
      setError("无法读取这张图片");
    }
  };

  /** Builds a look from a reference, measured on this photo as it is without one. */
  const runColourMatch = async (reference: Blob, name: string) => {
    if (!loaded) return;
    const engine = engineRef.current!;
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
      const stored = await saveLut(`追色 ${name}`, lut);
      setImportedLuts((list) => [...list, stored]);
      applyEdit({ ...adjustments, lutId: stored.id, lutAmount: 1 }, true);
    } catch (reason) {
      setError(reason instanceof DOMException ? "浏览器读不了这个文件。RAW 请先打开一次，再用它作参考。" : `追色失败：${(reason as Error).message}`);
      await engine.setLook(await resolveLook(lookId));
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
      setCullStatus(`${photos.length} 张都已分析过，共 ${groupCount} 组。`);
      return;
    }
    flushPendingEdit();
    setBatchRunning(true);
    setAnalyzing({ done: 0, total: pending.length });
    try {
      for (const [index, photo] of pending.entries()) {
        const edit = persistable(editsRef.current[photo.id] ?? loadEdit(photo.id));
        const result = await openPhoto(photo, edit, loadHeals(photo.id), loadWarps(photo.id));
        const thumbnail = await engine.thumbnail(ANALYSIS_LONG_EDGE, false);
        const faces = await locateFaces(thumbnail);
        const captured = captureMillis(result.captureTime);
        const analysis = analyzeImage(new Uint8Array(thumbnail.rgb), thumbnail.width, thumbnail.height, faces, captured ?? photo.file.lastModified, captured !== null);
        await saveAnalysis(photo.id, analysis);
        setAnalyses((all) => ({ ...all, [photo.id]: analysis }));
        setAnalyzing({ done: index + 1, total: pending.length });
      }
    } catch (reason) {
      setError(`分析失败：${(reason as Error).message}`);
    } finally {
      setBatchRunning(false);
      setAnalyzing(null);
      await selectPhoto(returnTo);
    }
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
    setCullStatus(`选中 ${picked} 张，淘汰 ${rejected} 张。已手动标记的照片没有改动。`);
  };

  const importLut = async (file: File | undefined) => {
    if (!file) return;
    try {
      const stored = await saveLut(file.name.replace(/\.cube$/i, ""), parseCube(await file.text()));
      setImportedLuts((list) => [...list, stored]);
      applyEdit({ ...adjustments, lutId: stored.id, lutAmount: 1 }, true);
    } catch (reason) {
      setError(`导入 LUT 失败：${(reason as Error).message}`);
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
      setError(`恢复图库失败：${(reason as Error).message}`);
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
      if (missing > 0) setProjectNotice(`有 ${missing} 张没有读取权限或已移动，重新添加同一文件即可恢复修图。`);
    } catch (reason) {
      setError(`打开项目失败：${(reason as Error).message}`);
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
    const box = fitBox((imageFrame.width * shownCrop.w) / (imageFrame.height * shownCrop.h), bounds.width, bounds.height);
    const s = (event.clientX - bounds.left - box.left) / box.width;
    const t = (event.clientY - bounds.top - box.top) / box.height;
    const shown = outputToSource(s, t, adjustments, loaded.width, loaded.height);
    const point = lens ? lensToSource(lens, shown.x, shown.y) : shown;
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
    if (!localTool) return;
    const point = pointerToSource(event);
    if (!point) return;
    event.currentTarget.setPointerCapture?.(event.pointerId);
    localDragRef.current = true;
    const layers = localsRef.current;
    const active = layers[activeLocal];
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
    const updated =
      layer.kind === "brush"
        ? { ...layer, strokes: [...layer.strokes.slice(0, -1), [...layer.strokes[layer.strokes.length - 1], point]] }
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
    applyEdit(removeLocalValues(adjustments, activeLocal, layers.length), true);
    commitLocals(layers.filter((_, index) => index !== activeLocal));
    setActiveLocal(0);
    if ((adjustments.localOverlay ?? 0) > 0) setValue("localOverlay", 0);
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
    // Let replies for the last pointer moves arrive before recording the drag.
    await new Promise((resolve) => window.setTimeout(resolve, LIQUIFY_SETTLE_MS));
    if (drag.strokes.length === 0) return;
    setWarpGroups((groups) => {
      const next = [...groups, drag.strokes];
      saveWarps(currentId, next);
      return next;
    });
    void refreshThumbnail(currentId).catch(() => {});
  };

  const replaceWarps = async (next: WarpStroke[][]) => {
    if (!currentId) return;
    setWarpGroups(next);
    saveWarps(currentId, next);
    await engineRef.current!.setWarps(next.flat());
    void refreshThumbnail(currentId).catch(() => {});
  };

  const healAtPointer = async (event: React.MouseEvent<HTMLElement>) => {
    if (!healing || healMode !== "spot" || !currentId) return;
    const bounds = event.currentTarget.getBoundingClientRect();
    const u = (event.clientX - bounds.left) / bounds.width;
    const v = (event.clientY - bounds.top) / bounds.height;
    try {
      const spot = await engineRef.current!.healAt(u, v, Math.round(bounds.width), Math.round(bounds.height), healRadius);
      if (!spot) return;
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
      setError(`主体识别失败：${(reason as Error).message}`);
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
      setError(`人脸识别失败：${(reason as Error).message}`);
    } finally {
      setFindingFaces(false);
    }
  };

  const startExport = async () => {
    if (!current) return;
    setBusy("导出中…");
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
    const next = { ...presets, [name]: persistable(adjustments) };
    setPresets(next);
    savePresets(next);
    setSelectedPreset(name);
    setPresetName("");
  };

  const applyPreset = () => {
    const preset = presets[selectedPreset];
    if (preset) applyEdit({ ...preset, maskOverlay: adjustments.maskOverlay ?? 0 }, true);
  };

  const deletePreset = () => {
    const { [selectedPreset]: _removed, ...rest } = presets;
    setPresets(rest);
    savePresets(rest);
    setSelectedPreset("");
  };

  const syncToAll = () => {
    flushPendingEdit();
    const edit = persistable(adjustments);
    const changes: Record<string, Adjustments> = {};
    for (const photo of photos) {
      if (photo.id === currentId) continue;
      const previous = editsRef.current[photo.id] ?? loadEdit(photo.id);
      const history = (historyRef.current[photo.id] ??= { past: [], future: [] });
      history.past = [...history.past, previous].slice(-HISTORY_LIMIT);
      history.future = [];
      editsRef.current[photo.id] = edit;
      changes[photo.id] = edit;
    }
    saveEdits(changes);
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
          await engine.setLocalMask(
            renderLocalMask(layers, result.proxyWidth, result.proxyHeight),
            result.proxyWidth,
            result.proxyHeight,
          );
        }
        if (edit.replaceMode) {
          const image = await resolveReplace(edit.replaceAsset ?? 0);
          if (image) await engine.setReplaceImage(image);
        }
        if (edit.replaceMode === REPLACE_SKY) await engine.detectSky();
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
      setError(`批量导出失败：${(reason as Error).message}`);
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
      <div className="workspace">
        <main
          className="stage"
          ref={stageRef}
          style={healing || liquifying ? { cursor: "none" } : localTool || pickingWhite ? { cursor: "crosshair" } : undefined}
          onPointerDown={(event) => {
            startLiquify(event);
            startLocal(event);
            startStamp(event);
            startErase(event);
          }}
          onPointerUp={() => {
            void finishLiquify();
            finishLocal();
            finishStamp();
            finishErase();
          }}
          onPointerCancel={() => {
            void finishLiquify();
            finishLocal();
            finishStamp();
            finishErase();
          }}
          onClick={(event) => {
            void healAtPointer(event);
            void pickWhite(event);
          }}
          onPointerMove={(event) => {
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
          <canvas ref={canvasRef} style={{ visibility: loaded ? undefined : "hidden" }} />
          {!loaded && !busy && <p className="hint">{ready ? "拖入一组 RAW、JPEG、PNG 或 HEIC" : "正在初始化 WebGPU…"}</p>}
          {busy && <p className="hint">{busy}</p>}
          {(adjustments.localOverlay ?? 0) > 0 && loaded && locals[activeLocal] && locals[activeLocal].kind !== "brush" && locals[activeLocal].kind !== "full" &&
            (["a", "b"] as const).map((which) => {
              const handle = locals[activeLocal][which];
              const corrected = lens ? sourceToLens(lens, handle.x, handle.y) : handle;
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
              <span className="compare-label before">修图前</span>
              <span className="compare-label after">修图后</span>
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
                  const corrected = lens ? sourceToLens(lens, dab.x, dab.y) : dab;
                  const point = sourceToOutput(corrected.x, corrected.y, adjustments, loaded.width, loaded.height);
                  return <circle key={index} cx={point.x * displayBox.width} cy={point.y * displayBox.height} r={(brushDiameter / 2) * (dab.radius / healRadius)} />;
                })}
              </g>
            </svg>
          )}
          {healing && healMode === "clone" && stampMarker && loaded && (() => {
            const corrected = lens ? sourceToLens(lens, stampMarker.x, stampMarker.y) : stampMarker;
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

        {photos.length > 0 && (
          <nav className="strip" aria-label="图库">
            {shownPhotos.map((photo) => (
              <button
                key={photo.id}
                className={`thumb ${photo.id === currentId ? "current" : ""} ${flags[photo.id] === FLAG_REJECT ? "rejected" : ""}`}
                disabled={locked}
                title={photo.file.name}
                onClick={() => void selectPhoto(photo)}
              >
                {photo.thumbnail ? <img src={photo.thumbnail} alt={photo.file.name} /> : <span>{photo.file.name}</span>}
                {groupOf.has(photo.id) && <i className={`thumb-score ${verdicts.get(photo.id)?.best ? "best" : ""}`} title="分组 · 评分">{groupOf.get(photo.id)} · {verdicts.get(photo.id)?.score}</i>}
                {flags[photo.id] === FLAG_PICK && <i className="thumb-flag pick" aria-label="已选中" />}
                {flags[photo.id] === FLAG_REJECT && <i className="thumb-flag reject" aria-label="已淘汰" />}
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
        <header>
          <h1>OpenPixels</h1>
          <button className="button secondary" disabled={!ready || locked} onClick={() => setLibraryOpen(true)}>项目库</button>
          <button className="button" disabled={!ready || locked} onClick={() => void openPicker()}>添加图片</button>
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
        </header>

        {error && <p className="error" role="alert">{error}</p>}

        {projectOfActive && (
          <div className="project-bar" data-testid="active-project">
            <span>项目：{projectOfActive.name}</span>
            <button className="button secondary" disabled={!current || locked} onClick={removeFromProject}>移出当前图片</button>
          </div>
        )}
        {projectNotice && <p className="progress">{projectNotice}</p>}
        {resumable && photos.length === 0 && (
          <div className="restore" data-testid="resume-project">
            <span>上次的项目：{resumable.name}（{resumable.photoIds.length} 张）</span>
            <button className="button" disabled={!ready} onClick={() => void openProject(resumable)}>打开</button>
          </div>
        )}
        {restorable.length > 0 && photos.length === 0 && (
          <div className="restore" data-testid="restore">
            <span>上次有 {restorable.length} 张图片</span>
            <button className="button" disabled={!ready} onClick={() => void restoreLibrary()}>恢复</button>
            <button className="button secondary" onClick={clearLibrary}>忽略</button>
          </div>
        )}

        {photos.length > 0 && (
          <section>
            <div className="row">
              <div className="stars" role="group" aria-label="星标">
                {[1, 2, 3, 4, 5].map((star) => (
                  <button
                    key={star}
                    className={`star ${(current ? (ratings[current.id] ?? 0) : 0) >= star ? "on" : ""}`}
                    aria-label={`${star} 星`}
                    disabled={!current || locked}
                    onClick={() => current && rate(current.id, ratings[current.id] === star ? 0 : star)}
                  >
                    ★
                  </button>
                ))}
              </div>
              <select aria-label="星标筛选" value={minRating} disabled={locked} onChange={(event) => setMinRating(Number(event.target.value))}>
                <option value={0}>全部</option>
                {[1, 2, 3, 4, 5].map((star) => (
                  <option key={star} value={star}>{star} 星及以上</option>
                ))}
              </select>
            </div>
            <div className="row">
              <button className={`button ${current && flags[current.id] === FLAG_PICK ? "" : "secondary"}`} aria-label="选中" disabled={!current || locked} onClick={() => current && flag(current.id, flags[current.id] === FLAG_PICK ? 0 : FLAG_PICK)}>选中</button>
              <button className={`button ${current && flags[current.id] === FLAG_REJECT ? "" : "secondary"}`} aria-label="淘汰" disabled={!current || locked} onClick={() => current && flag(current.id, flags[current.id] === FLAG_REJECT ? 0 : FLAG_REJECT)}>淘汰</button>
              <select aria-label="旗标筛选" value={flagFilter} disabled={locked} onChange={(event) => setFlagFilter(Number(event.target.value))}>
                <option value={0}>全部旗标</option>
                <option value={FLAG_PICK}>只看选中</option>
                <option value={2}>隐藏淘汰</option>
                <option value={FLAG_REJECT}>只看淘汰</option>
              </select>
            </div>
            <div className="row">
              <button className="button secondary" disabled={!current || locked} onClick={() => void runAnalysis()}>分析并分组</button>
              <button className="button secondary" disabled={verdicts.size === 0 || locked} onClick={autoPick}>自动选片</button>
              <select aria-label="分组筛选" value={groupFilter} disabled={locked || verdicts.size === 0} onChange={(event) => setGroupFilter(Number(event.target.value))}>
                <option value={0}>全部分组</option>
                <option value={GROUP_BEST}>每组最佳</option>
                {Array.from({ length: groupCount }, (_, index) => (
                  <option key={index} value={index + 1}>第 {index + 1} 组</option>
                ))}
              </select>
            </div>
            {(analyzing || cullStatus) && (
              <p className="progress" data-testid="cull-status">{analyzing ? `正在分析 ${analyzing.done}/${analyzing.total}…` : cullStatus}</p>
            )}
          </section>
        )}

        <section className="history">
          <button className="button secondary" disabled={!history?.past.length || locked} onClick={() => stepHistory("undo")}>撤销</button>
          <button className="button secondary" disabled={!history?.future.length || locked} onClick={() => stepHistory("redo")}>重做</button>
          <button className="button secondary" disabled={!loaded || locked} onClick={reset}>重置</button>
          <button className={`button ${compare > 0 ? "" : "secondary"}`} title="按住 \\ 键可查看整张修图前" disabled={!loaded || locked} onClick={() => setCompare((split) => (split > 0 ? 0 : 0.5))}>对比</button>
        </section>
        {toolHint && <p className="progress" data-testid="tool-hint">{toolHint}</p>}

        {GROUPS.map((group) => (
          <section key={group.title}>
            <h2>{group.title}</h2>
            {group.title === "白平衡" && (
              <div className="row">
                <button className={`button ${pickingWhite ? "" : "secondary"}`} disabled={!loaded || locked} onClick={togglePickingWhite}>
                  {pickingWhite ? "点击画面中性灰处…" : "吸管"}
                </button>
              </div>
            )}
            {group.sliders.map((slider) => {
              const value = adjustments[slider.key] ?? 0;
              return (
                <label className="slider" key={slider.key}>
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
                    onDoubleClick={() => setValue(slider.key, 0)}
                  />
                </label>
              );
            })}
          </section>
        ))}

        <section>
          <h2>相机配置</h2>
          <label className="field">
            <span>色彩配置</span>
            <select
              aria-label="色彩配置"
              value={loaded?.cameraMatch && adjustments.cameraProfile ? 1 : 0}
              disabled={!loaded?.cameraMatch || locked}
              onChange={(event) => setValue("cameraProfile", Number(event.target.value))}
            >
              <option value={0}>标准</option>
              <option value={1}>相机匹配</option>
            </select>
          </label>
          <label className="check">
            <input
              type="checkbox"
              aria-label="使用自动镜头校正"
              checked={!!loaded?.lens && !!adjustments.lensAuto}
              disabled={!loaded?.lens || locked}
              onChange={(event) => setValue("lensAuto", event.target.checked ? 1 : 0)}
            />
            使用自动镜头校正
          </label>
          {loaded?.isRaw && (
            <p className="progress" data-testid="camera-status">
              {loaded.cameraMatch ? "已按机内 JPEG 拟合相机色彩。" : "默认色彩已接近机内 JPEG，无需相机匹配。"}
              {loaded.lensSource === "lensfun"
                ? `Lensfun 镜头库有 ${loaded.lensQuery?.lens} 的校正数据（CC BY-SA 3.0），勾选上方选项后生效。`
                : loaded.lens
                  ? "文件内含镜头数据。"
                  : loaded.lensQuery
                    ? `镜头库里没有 ${loaded.lensQuery.lens} 的数据，可用下方滑块手动校正。`
                    : "文件没有可用的镜头数据，可用下方滑块手动校正。"}
            </p>
          )}
        </section>

        <section>
          <h2>曲线</h2>
          <ToneCurve adjustments={adjustments} disabled={!loaded || locked} onChange={setValue} />
        </section>

        <section>
          <h2>滤镜</h2>
          <div className="row">
            <select aria-label="滤镜" value={lookId} disabled={!loaded || locked} onChange={(event) => chooseLook(Number(event.target.value))}>
              <option value={0}>无</option>
              {BUILT_IN_LOOKS.map((look) => (
                <option key={look.id} value={look.id}>{look.name}</option>
              ))}
              {importedLuts.map((lut) => (
                <option key={lut.id} value={lut.id}>{lut.name}</option>
              ))}
            </select>
            <label className={`button secondary ${!loaded || locked ? "disabled" : ""}`}>
              导入 .cube
              <input
                type="file"
                hidden
                accept=".cube"
                aria-label="导入 LUT"
                disabled={!loaded || locked}
                onChange={(event) => {
                  void importLut(event.target.files?.[0]);
                  event.target.value = "";
                }}
              />
            </label>
          </div>
          <label className="slider">
            <span>滤镜强度</span>
            <output>{(adjustments.lutAmount ?? 0).toFixed(2)}</output>
            <input aria-label="滤镜强度" type="range" min={0} max={1} step={0.01} value={adjustments.lutAmount ?? 0} disabled={!lookId || locked} onChange={(event) => setValue("lutAmount", Number(event.target.value))} />
          </label>
          <div className="row">
            <select aria-label="追色参考" value={matchReference} disabled={!loaded || locked || matching} onChange={(event) => setMatchReference(event.target.value)}>
              <option value="">选择参考照片</option>
              {photos.filter((photo) => photo.id !== currentId).map((photo) => (
                <option key={photo.id} value={photo.id}>{photo.file.name}</option>
              ))}
            </select>
            <button className="button secondary" disabled={!matchReference || !loaded || locked || matching} onClick={() => void matchFromStrip()}>{matching ? "追色中…" : "追色"}</button>
            <label className={`button secondary ${!loaded || locked || matching ? "disabled" : ""}`}>
              导入参考图
              <input
                type="file"
                hidden
                accept="image/*"
                aria-label="导入追色参考图"
                disabled={!loaded || locked || matching}
                onChange={(event) => {
                  const file = event.target.files?.[0];
                  if (file) void runColourMatch(file, file.name.replace(/\.[^.]+$/, ""));
                  event.target.value = "";
                }}
              />
            </label>
          </div>
        </section>

        <section>
          <h2>色调分离</h2>
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
        </section>

        <section>
          <h2>HSL</h2>
          <div className="bands" role="group" aria-label="颜色通道">
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
            const name = `${HSL_BANDS[hslBand].name}${label}`;
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
        </section>

        <section>
          <h2>AI 主体</h2>
          <button className="button" disabled={!loaded || detecting || locked} onClick={() => void detectSubject()}>
            {detecting ? "识别中…" : maskStats ? "重新识别" : "识别主体"}
          </button>
          <label className="check">
            <input
              type="checkbox"
              aria-label="显示蒙版"
              disabled={!maskStats || locked}
              checked={(adjustments.maskOverlay ?? 0) > 0}
              onChange={(event) => setValue("maskOverlay", event.target.checked ? 1 : 0)}
            />
            显示蒙版（背景标红）
          </label>
          <label className="slider">
            <span>背景曝光</span>
            <output>{backgroundExposure.toFixed(2)}</output>
            <input
              aria-label="背景曝光"
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
        </section>

        <section>
          <h2>换背景 / 换天</h2>
          <div className="row">
            <select aria-label="替换内容" value={replaceMode} disabled={!loaded || locked} onChange={(event) => chooseReplaceMode(Number(event.target.value))}>
              <option value={0}>不替换</option>
              <option value={REPLACE_BACKGROUND}>换背景</option>
              <option value={REPLACE_SKY}>换天</option>
            </select>
            <select aria-label="替换素材" value={replaceAsset} disabled={!replaceMode || locked} onChange={(event) => setValue("replaceAsset", Number(event.target.value))}>
              {BUILT_IN_REPLACEMENTS.filter((item) => item.mode === replaceMode).map((item) => (
                <option key={item.id} value={item.id}>{item.name}</option>
              ))}
              {replaceAssets.filter((item) => item.mode === replaceMode).map((item) => (
                <option key={item.id} value={item.id}>{item.name}</option>
              ))}
            </select>
            <label className={`button secondary ${!replaceMode || locked ? "disabled" : ""}`}>
              导入图片
              <input
                type="file"
                hidden
                accept="image/*"
                aria-label="导入替换图片"
                disabled={!replaceMode || locked}
                onChange={(event) => {
                  void importReplacement(event.target.files?.[0]);
                  event.target.value = "";
                }}
              />
            </label>
          </div>
          {detecting && replaceMode > 0 && <p className="progress">正在识别…</p>}
          {([["融合强度", "replaceAmount", 0, 1], ["替换亮度", "replaceExposure", -3, 3], ["上下位置", "replaceShift", -0.5, 0.5]] as const).map(([label, key, min, max]) => (
            <label className="slider" key={key}>
              <span>{label}</span>
              <output>{(adjustments[key] ?? 0).toFixed(2)}</output>
              <input aria-label={label} type="range" min={min} max={max} step={0.01} value={adjustments[key] ?? 0} disabled={!replaceMode || locked} onChange={(event) => setValue(key, Number(event.target.value))} />
            </label>
          ))}
        </section>

        <section>
          <h2>裁剪与旋转</h2>
          <div className="row">
            <button className={`button ${cropEditing ? "" : "secondary"}`} disabled={!loaded || locked} onClick={toggleCropEditing}>
              {cropEditing ? "完成裁剪" : "裁剪"}
            </button>
            <select
              aria-label="裁剪比例"
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
            <span>旋转</span>
            <output>{rotate.toFixed(1)}°</output>
            <input
              aria-label="旋转"
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
            <button className="button secondary" disabled={!loaded || locked} onClick={() => turn(-1)}>左转 90°</button>
            <button className="button secondary" disabled={!loaded || locked} onClick={() => turn(1)}>右转 90°</button>
            <button className="button secondary" disabled={!loaded || locked} onClick={resetCrop}>还原</button>
          </div>
        </section>

        <section>
          <h2>污点修复</h2>
          <div className="row">
            <button className={`button ${healing ? "" : "secondary"}`} disabled={!loaded || locked} onClick={toggleHealing}>
              {healing ? "退出修复" : "开始修复"}
            </button>
            <select aria-label="修复方式" value={healMode} disabled={!loaded || locked} onChange={(event) => setHealMode(event.target.value as "spot" | "clone" | "erase")}>
              <option value="spot">污点修复</option>
              <option value="clone">仿制图章</option>
              <option value="erase">AI 消除</option>
            </select>
          </div>
          {healMode === "erase" ? (
            <>
              <div className="row">
                <button className="button" disabled={eraseDabs.length === 0 || locked} onClick={() => void runErase()}>{erasing ? "消除中…" : "消除涂抹区域"}</button>
                <button className="button secondary" disabled={eraseDabs.length === 0 || locked} onClick={() => setEraseDabs([])}>清空涂抹</button>
              </div>
              <div className="row">
                <button className="button secondary" disabled={eraseCount === 0 || locked} onClick={() => void commitErases(erasesRef.current.slice(0, -1))}>撤销一步</button>
                <button className="button secondary" disabled={eraseCount === 0 || locked} onClick={() => void commitErases([])}>清除</button>
              </div>
            </>
          ) : (
            <div className="row">
              <button className="button secondary" disabled={heals.length === 0 || locked} onClick={undoHeal}>撤销一步</button>
              <button className="button secondary" disabled={heals.length === 0 || locked} onClick={() => void replaceHeals([])}>清除</button>
            </div>
          )}
          <label className="slider">
            <span>画笔大小</span>
            <output>{(healRadius * 100).toFixed(1)}</output>
            <input
              aria-label="画笔大小"
              type="range"
              min={0.002}
              max={0.06}
              step={0.001}
              value={healRadius}
              disabled={!loaded || locked}
              onChange={(event) => setHealRadius(Number(event.target.value))}
            />
          </label>
          {healMode === "clone" && <p className="progress">按住 Alt（Option）点击取样，再拖动涂抹。</p>}
          {healMode === "erase" && <p className="progress">{erasing ? "正在计算，通常需要 10–20 秒…" : "涂抹要去掉的内容，可以多涂几笔，再点“消除涂抹区域”。"}</p>}
          <p className="progress" data-stat="heals">{healMode === "erase" ? `已消除 ${eraseCount} 处` : `已修复 ${heals.length} 处`}</p>
        </section>

        <section>
          <h2>局部调整（{locals.length}/{MAX_LOCAL_LAYERS}）</h2>
          <div className="row">
            {([["linear", "线性"], ["radial", "径向"], ["brush", "画笔"]] as [LocalKind, string][]).map(([kind, label]) => (
              <button key={kind} className={`button ${localTool === kind ? "" : "secondary"}`} disabled={!loaded || locked} onClick={() => chooseLocalTool(kind)}>{label}</button>
            ))}
            <button className="button secondary" disabled={!loaded || locked || locals.length >= MAX_LOCAL_LAYERS} onClick={addFullLocal}>全图</button>
          </div>
          {localTool === "brush" && (
            <label className="slider">
              <span>局部画笔</span>
              <output>{(localBrush * 100).toFixed(1)}</output>
              <input aria-label="局部画笔" type="range" min={0.005} max={0.2} step={0.005} value={localBrush} onChange={(event) => setLocalBrush(Number(event.target.value))} />
            </label>
          )}
          {locals.length > 0 && (
            <>
              <div className="row" role="group" aria-label="局部图层">
                {locals.map((layer, index) => (
                  <button key={index} className={`button ${index === activeLocal ? "" : "secondary"}`} disabled={locked} onClick={() => setActiveLocal(index)}>
                    {{ linear: "线", radial: "圆", brush: "笔", full: "全" }[layer.kind]}{index + 1}
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
                    <input aria-label={`局部${label}`} type="range" min={min} max={max} step={0.01} value={value} disabled={locked} onChange={(event) => setValue(key, Number(event.target.value))} onDoubleClick={() => setValue(key, 0)} />
                  </label>
                );
              })}
              <label className="field">
                <span>范围</span>
                <select aria-label="局部范围" value={adjustments[`local${activeLocal}RangeMode`] ?? 0} disabled={locked} onChange={(event) => setValue(`local${activeLocal}RangeMode`, Number(event.target.value))}>
                  <option value={0}>无</option>
                  <option value={1}>亮度范围</option>
                  <option value={2}>色彩范围</option>
                </select>
              </label>
              {(adjustments[`local${activeLocal}RangeMode`] === 1
                ? [
                    { suffix: "RangeLow", label: "最暗", min: 0, max: 1, fallback: 0 },
                    { suffix: "RangeHigh", label: "最亮", min: 0.02, max: 1, fallback: 1 },
                  ]
                : adjustments[`local${activeLocal}RangeMode`] === 2
                  ? [
                      { suffix: "RangeHue", label: "色相", min: 0, max: 360, fallback: 0 },
                      { suffix: "RangeWidth", label: "色相宽度", min: 5, max: 90, fallback: 30 },
                    ]
                  : []
              ).map(({ suffix, label, min, max, fallback }) => {
                const key = `local${activeLocal}${suffix}`;
                const value = adjustments[key] ?? fallback;
                return (
                  <label className={`slider ${suffix === "RangeHue" ? "hue-slider" : ""}`} key={suffix}>
                    <span>{label}</span>
                    <output>{value.toFixed(max > 1 ? 0 : 2)}</output>
                    <input aria-label={`局部${label}`} type="range" min={min} max={max} step={max > 1 ? 1 : 0.01} value={value} disabled={locked} onChange={(event) => setValue(key, Number(event.target.value))} />
                  </label>
                );
              })}
              <div className="row">
                <button
                  className={`button ${(adjustments.localOverlay ?? 0) > 0 ? "" : "secondary"}`}
                  disabled={locked}
                  onClick={() => setValue("localOverlay", (adjustments.localOverlay ?? 0) > 0 ? 0 : activeLocal + 1)}
                >
                  显示蒙版
                </button>
                <button className="button secondary" disabled={locked} onClick={invertLocal}>反转</button>
                <button className="button secondary" disabled={locked} onClick={deleteLocal}>删除图层</button>
              </div>
            </>
          )}
        </section>

        <section>
          <h2>液化</h2>
          <div className="row">
            <button className={`button ${liquifying ? "" : "secondary"}`} disabled={!loaded || locked} onClick={toggleLiquify}>
              {liquifying ? "退出液化" : "推动变形"}
            </button>
            <select aria-label="液化模式" value={liquifyMode} disabled={!loaded || locked} onChange={(event) => setLiquifyMode(Number(event.target.value))}>
              {LIQUIFY_MODES.map((mode) => (
                <option key={mode.value} value={mode.value}>{mode.label}</option>
              ))}
            </select>
            <button className="button secondary" disabled={warpGroups.length === 0 || locked} onClick={() => void replaceWarps(warpGroups.slice(0, -1))}>撤销一笔</button>
            <button className="button secondary" disabled={warpGroups.length === 0 || locked} onClick={() => void replaceWarps([])}>清除</button>
          </div>
          <label className="slider">
            <span>液化大小</span>
            <output>{(liquifyRadius * 100).toFixed(1)}</output>
            <input aria-label="液化大小" type="range" min={0.01} max={0.25} step={0.005} value={liquifyRadius} disabled={!loaded || locked} onChange={(event) => setLiquifyRadius(Number(event.target.value))} />
          </label>
          <label className="slider">
            <span>液化强度</span>
            <output>{liquifyStrength.toFixed(2)}</output>
            <input aria-label="液化强度" type="range" min={0.05} max={1} step={0.05} value={liquifyStrength} disabled={!loaded || locked} onChange={(event) => setLiquifyStrength(Number(event.target.value))} />
          </label>
          <p className="progress" data-stat="warps">{warpGroups.length} 笔</p>
        </section>

        <section>
          <h2>皮肤</h2>
          <button className="button" disabled={!loaded || findingFaces || locked} onClick={() => void findFaces()}>
            {findingFaces ? "识别中…" : skinStats ? "重新识别人脸" : "识别人脸"}
          </button>
          {skinStats && (
            <p className="progress" data-stat="faces">
              {skinStats.faces > 0
                ? `检测到 ${skinStats.faces} 张人脸，${skinStats.ms.toFixed(0)} ms`
                : "未检测到完整人脸，按画面中的肤色处理"}
            </p>
          )}
          {[
            { key: "skinSmooth", label: "磨皮" },
            { key: "skinBlemish", label: "祛斑" },
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
        </section>

        <section>
          <h2>预设</h2>
          <div className="row">
            <select
              aria-label="选择预设"
              value={selectedPreset}
              disabled={locked}
              onChange={(event) => setSelectedPreset(event.target.value)}
            >
              <option value="">选择预设…</option>
              {Object.keys(presets).map((name) => (
                <option key={name} value={name}>{name}</option>
              ))}
            </select>
            <button className="button secondary" disabled={!selectedPreset || !loaded || locked} onClick={applyPreset}>套用</button>
            <button className="button secondary" disabled={!selectedPreset || locked} onClick={deletePreset}>删除</button>
          </div>
          <div className="row">
            <input
              aria-label="预设名称"
              placeholder="预设名称"
              value={presetName}
              disabled={locked}
              onChange={(event) => setPresetName(event.target.value)}
              onKeyDown={(event) => event.key === "Enter" && savePreset()}
            />
            <button className="button secondary" disabled={!presetName.trim() || !loaded || locked} onClick={savePreset}>保存当前</button>
          </div>
        </section>

        <section>
          <h2>批量（{shownPhotos.length} 张）</h2>
          <div className="row">
            <button className="button secondary" disabled={photos.length < 2 || locked} onClick={syncToAll}>同步到全部</button>
            <button className="button" disabled={shownPhotos.length === 0 || locked} onClick={() => void runBatch()}>批量导出</button>
          </div>
          {batch && (
            <p className="progress" data-stat="batch">
              {batch.done}/{batch.total}，{(batch.ms / 1000).toFixed(1)} 秒
            </p>
          )}
          {batchUrl && <a className="download" data-stat="batch-zip" href={batchUrl} download="openpixels-export.zip">下载 ZIP</a>}
        </section>

        <section>
          <h2>导出设置</h2>
          <div className="row">
            <select aria-label="导出格式" value={output.format} disabled={locked} onChange={(event) => updateOutput({ format: event.target.value as OutputFormat })}>
              {(Object.keys(FORMAT_DETAILS) as OutputFormat[]).map((format) => (
                <option key={format} value={format}>{FORMAT_DETAILS[format].label}</option>
              ))}
            </select>
            <select aria-label="导出尺寸" value={output.longEdge} disabled={locked} onChange={(event) => updateOutput({ longEdge: Number(event.target.value) })}>
              {LONG_EDGE_CHOICES.map((edge) => (
                <option key={edge} value={edge}>{edge === 0 ? "原尺寸" : `长边 ${edge}`}</option>
              ))}
            </select>
          </div>
          {output.format !== "jpeg" && (
            <div className="row">
              <select aria-label="位深" value={output.bitDepth} disabled={locked} onChange={(event) => updateOutput({ bitDepth: Number(event.target.value) as 8 | 16 })}>
                <option value={8}>8 位</option>
                <option value={16}>16 位</option>
              </select>
            </div>
          )}
          {output.format === "jpeg" && (
            <label className="slider">
              <span>质量</span>
              <output>{output.quality}</output>
              <input aria-label="导出质量" type="range" min={40} max={100} step={1} value={output.quality} disabled={locked} onChange={(event) => updateOutput({ quality: Number(event.target.value) })} />
            </label>
          )}
          <div className="row">
            <input aria-label="文件名规则" title="可用 {name}（原文件名）、{n}（序号）、{date}（日期）" value={output.template} disabled={locked} onChange={(event) => updateOutput({ template: event.target.value })} />
          </div>
          <p className="progress">文件名可用 {"{name}"}、{"{n}"}、{"{date}"}</p>
          <label className="check">
            <input type="checkbox" aria-label="添加水印" checked={output.watermark.enabled} disabled={locked} onChange={(event) => updateWatermark({ enabled: event.target.checked })} />
            添加水印
          </label>
          {output.watermark.enabled && (
            <>
              <div className="row">
                <input aria-label="水印文字" placeholder="水印文字" value={output.watermark.text} disabled={locked} onChange={(event) => updateWatermark({ text: event.target.value })} />
              </div>
              <div className="row">
                <label className={`button secondary ${locked ? "disabled" : ""}`}>
                  {output.watermark.logo ? "更换 Logo" : "选择 Logo"}
                  <input
                    type="file"
                    hidden
                    accept="image/*"
                    aria-label="水印图片"
                    disabled={locked}
                    onChange={(event) => {
                      void chooseLogo(event.target.files?.[0]);
                      event.target.value = "";
                    }}
                  />
                </label>
                {output.watermark.logo && <button className="button secondary" disabled={locked} onClick={() => updateWatermark({ logo: null })}>移除</button>}
                <select aria-label="水印位置" value={output.watermark.position} disabled={locked} onChange={(event) => updateWatermark({ position: event.target.value as WatermarkPosition })}>
                  {WATERMARK_POSITIONS.map((item) => (
                    <option key={item.value} value={item.value}>{item.label}</option>
                  ))}
                </select>
              </div>
              <label className="slider">
                <span>水印大小</span>
                <output>{(output.watermark.size * 100).toFixed(1)}</output>
                <input aria-label="水印大小" type="range" min={0.01} max={0.12} step={0.001} value={output.watermark.size} disabled={locked} onChange={(event) => updateWatermark({ size: Number(event.target.value) })} />
              </label>
              <label className="slider">
                <span>水印不透明度</span>
                <output>{output.watermark.opacity.toFixed(2)}</output>
                <input aria-label="水印不透明度" type="range" min={0.1} max={1} step={0.01} value={output.watermark.opacity} disabled={locked} onChange={(event) => updateWatermark({ opacity: Number(event.target.value) })} />
              </label>
            </>
          )}
        </section>

        <section className="actions">
          <button className="button" disabled={!loaded || !!busy || locked} onClick={() => void startExport()}>导出当前</button>
        </section>
        {exported && (
          <a className="download" href={exported.url} download={exported.name}>下载 {exported.name}</a>
        )}

        <section className="stats" data-testid="stats">
          <h2>性能</h2>
          <dl>
            {loaded && (
              <>
                <dt>图像</dt><dd>{loaded.width}×{loaded.height}{loaded.isRaw ? " RAW" : ""}</dd>
                <dt>文件</dt><dd>{loaded.fileMB.toFixed(0)} MB</dd>
                <dt>解码</dt><dd data-stat="decode">{loaded.decodeMs.toFixed(0)} ms</dd>
                {loaded.baseExposure !== null && (
                  <><dt>基础曝光</dt><dd data-stat="base-exposure">{loaded.baseExposure.toFixed(2)} EV</dd></>
                )}
                <dt>代理图</dt><dd>{loaded.proxyWidth}×{loaded.proxyHeight}，{loaded.proxyMs.toFixed(0)} ms</dd>
              </>
            )}
            {frame && (<><dt>单帧中位</dt><dd data-stat="frame">{frame.median.toFixed(1)} ms</dd></>)}
            {exported && (
              <>
                <dt>导出</dt><dd data-stat="export">{exported.ms.toFixed(0)} ms</dd>
                <dt>文件</dt><dd data-stat="export-size">{exported.sizeMB.toFixed(1)} MB</dd>
              </>
            )}
            {maskStats && (
              <>
                <dt>模型输入</dt><dd>{maskStats.inputMs.toFixed(0)} ms</dd>
                {maskStats.loadMs > 0 && (<><dt>模型加载</dt><dd data-stat="model-load">{maskStats.loadMs.toFixed(0)} ms</dd></>)}
                <dt>推理</dt><dd data-stat="infer">{maskStats.inferMs.toFixed(0)} ms</dd>
              </>
            )}
            {heapMB > 0 && (<><dt>WASM 堆</dt><dd data-stat="heap">{heapMB.toFixed(0)} MB</dd></>)}
          </dl>
        </section>
      </aside>
    </div>
  );
}
