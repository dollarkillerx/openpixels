import { useCallback, useEffect, useRef, useState } from "react";
import { CropOverlay } from "./CropOverlay";
import { ToneCurve } from "./ToneCurve";
import { chooseBatchTarget } from "./batch";
import { EngineClient, type MaskStats, type SkinStats } from "./engine";
import { FULL_FRAME, cropFields, cropForRatio, cropOf, fitBox, frameSize, outputToSource, shrinkToImage, sourceToOutput, type Crop } from "./frame";
import { BUILT_IN_LOOKS, builtInLut, parseCube } from "./lut";
import { forgetHandles, getLut, listLuts, recallHandles, rememberHandle, saveLut, type StoredLut } from "./store";
import { LOCAL_SLIDERS, MAX_LOCAL_LAYERS, removeLocalValues, renderLocalMask, type LocalKind, type LocalLayer } from "./local";
import {
  exportName,
  fileId,
  loadEdit,
  loadHeals,
  loadLocals,
  loadPresets,
  loadRatings,
  loadWarps,
  persistable,
  saveHeals,
  saveLocals,
  saveRating,
  saveWarps,
  savePresets,
  saveEdits,
  type Presets,
} from "./library";
import type { Adjustments, HealSpot, LoadResult, Thumbnail, WarpStroke } from "./protocol";

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
      { key: "temperature", label: "色温", min: -1, max: 1 },
      { key: "tint", label: "色调", min: -1, max: 1 },
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
      { key: "clarity", label: "清晰度", min: -1, max: 1 },
      { key: "sharpen", label: "锐化", min: 0, max: 1 },
      { key: "denoiseLuma", label: "明度降噪", min: 0, max: 1 },
      { key: "denoiseChroma", label: "色彩降噪", min: 0, max: 1 },
      { key: "vignette", label: "暗角", min: -1, max: 1 },
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
const JPEG_QUALITY = 92;
const FRAME_WINDOW = 30;
const THUMBNAIL_LONG_EDGE = 200;
const SETTLE_MS = 400;
const HISTORY_LIMIT = 100;
const RAW_ACCEPT = ".arw,.cr2,.cr3,.nef,.nrw,.raf,.rw2,.orf,.dng,.pef,.srw,.iiq,.3fr,.jpg,.jpeg,.png,.tif,.tiff";

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
const needsSubject = (edit: Adjustments) => (edit.backgroundExposure ?? 0) !== 0;
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
  const [importedLuts, setImportedLuts] = useState<StoredLut[]>([]);
  const [restorable, setRestorable] = useState<FileSystemFileHandle[]>([]);
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
  const shownPhotos = photos.filter((photo) => (ratings[photo.id] ?? 0) >= minRating);
  const locked = batchRunning;

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
    recallHandles().then(setRestorable).catch(() => {});
  }, []);

  const resolveLook = useCallback(async (id: number) => {
    if (!id) return null;
    return builtInLut(id) ?? (await getLut(id).catch(() => undefined)) ?? null;
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
      const result = await engine.open(await photo.file.arrayBuffer(), edit, spots, warps.flat());
      if (ticket !== openTicketRef.current) throw new Error(STALE);
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

  // The look follows the edit, whatever changed it: a slider, undo, a preset or a new photo.
  const lookId = adjustments.lutId ?? 0;
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
    };
    window.addEventListener("keydown", onRate);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("keydown", onRate);
    };
  }, [stepHistory, currentId, rate]);

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
    setLiquifying(false);
    setLocalTool(null);
    setHealing((on) => !on);
  };

  const toggleLiquify = () => {
    if (cropEditing) setValue("cropEditing", 0);
    setHealing(false);
    setLocalTool(null);
    setLiquifying((on) => !on);
  };

  const chooseLocalTool = (kind: LocalKind) => {
    if (cropEditing) setValue("cropEditing", 0);
    setHealing(false);
    setLiquifying(false);
    setLocalTool((current) => (current === kind ? null : kind));
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
    return outputToSource(s, t, adjustments, loaded.width, loaded.height);
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

  const restoreLibrary = async () => {
    const granted: FileSystemFileHandle[] = [];
    for (const handle of restorable) {
      const access = handle as FileSystemFileHandle & {
        requestPermission?: (d: { mode: string }) => Promise<PermissionState>;
      };
      if (!access.requestPermission || (await access.requestPermission({ mode: "read" })) === "granted") {
        granted.push(handle);
      }
    }
    setRestorable([]);
    try {
      await addHandles(granted);
    } catch (reason) {
      setError(`恢复图库失败：${(reason as Error).message}`);
    }
  };

  const clearLibrary = () => {
    void forgetHandles();
    setRestorable([]);
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
    const point = outputToSource(s, t, adjustments, loaded.width, loaded.height);
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
    if (!healing || !currentId) return;
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
      const result = await engineRef.current!.exportJpeg(JPEG_QUALITY);
      const blob = new Blob([result.jpeg], { type: "image/jpeg" });
      setExported({
        url: URL.createObjectURL(blob),
        name: exportName(current.file.name, new Set()),
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
        if (needsSubject(edit)) await engine.detectSubject(result.proxyWidth, result.proxyHeight);
        if (needsSkin(edit)) await engine.prepareSkin(result.proxyWidth, result.proxyHeight);
        const { jpeg, heapMB: heap } = await engine.exportJpeg(JPEG_QUALITY);
        await target.write(exportName(photo.file.name, used), new Uint8Array(jpeg));
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
          style={healing || liquifying ? { cursor: "none" } : localTool ? { cursor: "crosshair" } : undefined}
          onPointerDown={(event) => {
            startLiquify(event);
            startLocal(event);
          }}
          onPointerUp={() => {
            void finishLiquify();
            finishLocal();
          }}
          onPointerCancel={() => {
            void finishLiquify();
            finishLocal();
          }}
          onClick={(event) => void healAtPointer(event)}
          onPointerMove={(event) => {
            continueLocal(event);
            if (!healing && !liquifying) return;
            const bounds = event.currentTarget.getBoundingClientRect();
            setPointer({ x: event.clientX - bounds.left, y: event.clientY - bounds.top });
            continueLiquify(event);
          }}
          onPointerLeave={() => setPointer(null)}
          onDragOver={(event) => event.preventDefault()}
          onDrop={(event) => {
            event.preventDefault();
            addFiles([...event.dataTransfer.files]);
          }}
        >
          <canvas ref={canvasRef} />
          {!loaded && !busy && <p className="hint">{ready ? "拖入一组 RAW、JPEG 或 PNG" : "正在初始化 WebGPU…"}</p>}
          {busy && <p className="hint">{busy}</p>}
          {(adjustments.localOverlay ?? 0) > 0 && loaded && locals[activeLocal] && locals[activeLocal].kind !== "brush" &&
            (["a", "b"] as const).map((which) => {
              const point = sourceToOutput(locals[activeLocal][which].x, locals[activeLocal][which].y, adjustments, loaded.width, loaded.height);
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
                className={`thumb ${photo.id === currentId ? "current" : ""}`}
                disabled={locked}
                title={photo.file.name}
                onClick={() => void selectPhoto(photo)}
              >
                {photo.thumbnail ? <img src={photo.thumbnail} alt={photo.file.name} /> : <span>{photo.file.name}</span>}
              </button>
            ))}
          </nav>
        )}
      </div>

      <aside className="panel">
        <header>
          <h1>OpenPixels</h1>
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
          </section>
        )}

        <section className="history">
          <button className="button secondary" disabled={!history?.past.length || locked} onClick={() => stepHistory("undo")}>撤销</button>
          <button className="button secondary" disabled={!history?.future.length || locked} onClick={() => stepHistory("redo")}>重做</button>
          <button className="button secondary" disabled={!loaded || locked} onClick={reset}>重置</button>
        </section>

        {GROUPS.map((group) => (
          <section key={group.title}>
            <h2>{group.title}</h2>
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
        </section>

        <section>
          <h2>色调分离</h2>
          {GRADE_RANGES.map(({ key, label }) => (
            <div key={key}>
              {[
                { suffix: "Hue", name: "色相", max: 360, step: 1 },
                { suffix: "Amount", name: "强度", max: 1, step: 0.01 },
              ].map(({ suffix, name, max, step }) => {
                const field = `grade${key}${suffix}`;
                const value = adjustments[field] ?? 0;
                return (
                  <label className="slider" key={suffix}>
                    <span>{label}{name}</span>
                    <output>{suffix === "Hue" ? value.toFixed(0) : value.toFixed(2)}</output>
                    <input aria-label={`${label}${name}`} type="range" min={0} max={max} step={step} value={value} disabled={!loaded || locked} onChange={(event) => setValue(field, Number(event.target.value))} onDoubleClick={() => setValue(field, 0)} />
                  </label>
                );
              })}
            </div>
          ))}
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
              {healing ? "退出修复" : "点击修复"}
            </button>
            <button className="button secondary" disabled={heals.length === 0 || locked} onClick={() => void replaceHeals(heals.slice(0, -1))}>撤销一点</button>
            <button className="button secondary" disabled={heals.length === 0 || locked} onClick={() => void replaceHeals([])}>清除</button>
          </div>
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
          <p className="progress" data-stat="heals">已修复 {heals.length} 处</p>
        </section>

        <section>
          <h2>局部调整（{locals.length}/{MAX_LOCAL_LAYERS}）</h2>
          <div className="row">
            {([["linear", "线性"], ["radial", "径向"], ["brush", "画笔"]] as [LocalKind, string][]).map(([kind, label]) => (
              <button key={kind} className={`button ${localTool === kind ? "" : "secondary"}`} disabled={!loaded || locked} onClick={() => chooseLocalTool(kind)}>{label}</button>
            ))}
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
                    {layer.kind === "linear" ? "线" : layer.kind === "radial" ? "圆" : "笔"}{index + 1}
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
                <dt>JPEG</dt><dd>{exported.sizeMB.toFixed(1)} MB</dd>
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
