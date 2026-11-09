import { useCallback, useEffect, useRef, useState } from "react";
import { chooseBatchTarget } from "./batch";
import { EngineClient, type MaskStats } from "./engine";
import {
  exportName,
  fileId,
  loadEdit,
  loadPresets,
  persistable,
  savePresets,
  saveEdits,
  type Presets,
} from "./library";
import type { Adjustments, LoadResult, Thumbnail } from "./protocol";

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

const JPEG_QUALITY = 92;
const FRAME_WINDOW = 30;
const THUMBNAIL_LONG_EDGE = 200;
const SETTLE_MS = 400;
const HISTORY_LIMIT = 100;
const RAW_ACCEPT = ".arw,.cr2,.cr3,.nef,.nrw,.raf,.rw2,.orf,.dng,.pef,.srw,.iiq,.3fr,.jpg,.jpeg,.png";

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
  const [presets, setPresets] = useState<Presets>(loadPresets);
  const [selectedPreset, setSelectedPreset] = useState("");
  const [presetName, setPresetName] = useState("");
  const [batch, setBatch] = useState<{ done: number; total: number; ms: number } | null>(null);
  const [batchRunning, setBatchRunning] = useState(false);
  const [batchUrl, setBatchUrl] = useState<string | null>(null);
  const [hslBand, setHslBand] = useState(0);

  const current = photos.find((photo) => photo.id === currentId) ?? null;
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
      engineRef.current?.resize(
        Math.round(entry.contentRect.width * ratio),
        Math.round(entry.contentRect.height * ratio),
      );
    });
    observer.observe(stage);
    return () => observer.disconnect();
  }, [ready]);

  const refreshThumbnail = useCallback(async (id: string) => {
    const engine = engineRef.current;
    if (!engine) return;
    const ticket = openTicketRef.current;
    const thumbnail = await engine.thumbnail(THUMBNAIL_LONG_EDGE);
    if (ticket !== openTicketRef.current) return;
    const url = thumbnailUrl(thumbnail);
    setPhotos((list) => list.map((photo) => (photo.id === id ? { ...photo, thumbnail: url } : photo)));
  }, []);

  /** Opens a photo in the engine. Rejects with STALE if another open overtook it. */
  const openPhoto = useCallback(
    async (photo: Photo, edit: Adjustments) => {
      const engine = engineRef.current!;
      const ticket = ++openTicketRef.current;
      const result = await engine.open(await photo.file.arrayBuffer(), edit);
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

  const selectPhoto = useCallback(
    async (photo: Photo) => {
      flushPendingEdit();
      const edit = editsRef.current[photo.id] ?? loadEdit(photo.id);
      editsRef.current[photo.id] = edit;
      settledRef.current = edit;
      setCurrentId(photo.id);
      setAdjustments(edit);
      setBusy("解码中…");
      setError("");
      setExported(null);
      setMaskStats(null);
      setDetecting(false);
      try {
        const result = await openPhoto(photo, edit);
        setLoaded(result);
        setHeapMB(result.heapMB);
        setBusy("");
        // A saved background edit needs its mask back before it can be shown.
        if ((edit.backgroundExposure ?? 0) !== 0) {
          setDetecting(true);
          setMaskStats(await engineRef.current!.detectSubject(result.proxyWidth, result.proxyHeight));
          setDetecting(false);
        }
        await refreshThumbnail(photo.id);
      } catch (reason) {
        if (isDifferentImage(reason)) return;
        setError((reason as Error).message);
        setBusy("");
        setDetecting(false);
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
    return () => window.removeEventListener("keydown", onKey);
  }, [stepHistory]);

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
    if (photos.length === 0 || !current) return;
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
    setBatch({ done: 0, total: photos.length, ms: 0 });
    flushPendingEdit();

    try {
      for (const [index, photo] of photos.entries()) {
        const edit = persistable(editsRef.current[photo.id] ?? loadEdit(photo.id));
        setCurrentId(photo.id);
        setAdjustments(edit);
        const result = await openPhoto(photo, edit);
        setLoaded(result);
        if ((edit.backgroundExposure ?? 0) !== 0) {
          await engine.detectSubject(result.proxyWidth, result.proxyHeight);
        }
        const { jpeg, heapMB: heap } = await engine.exportJpeg(JPEG_QUALITY);
        await target.write(exportName(photo.file.name, used), new Uint8Array(jpeg));
        setHeapMB(heap);
        await refreshThumbnail(photo.id);
        setBatch({ done: index + 1, total: photos.length, ms: performance.now() - started });
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
  const history = currentId ? historyRef.current[currentId] : undefined;

  return (
    <div className="app">
      <div className="workspace">
        <main
          className="stage"
          ref={stageRef}
          onDragOver={(event) => event.preventDefault()}
          onDrop={(event) => {
            event.preventDefault();
            addFiles([...event.dataTransfer.files]);
          }}
        >
          <canvas ref={canvasRef} />
          {!loaded && !busy && <p className="hint">{ready ? "拖入一组 RAW、JPEG 或 PNG" : "正在初始化 WebGPU…"}</p>}
          {busy && <p className="hint">{busy}</p>}
        </main>

        {photos.length > 0 && (
          <nav className="strip" aria-label="图库">
            {photos.map((photo) => (
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
          <label className={`button ${ready && !locked ? "" : "disabled"}`}>
            添加图片
            <input
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
          </label>
        </header>

        {error && <p className="error" role="alert">{error}</p>}

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
          <h2>批量（{photos.length} 张）</h2>
          <div className="row">
            <button className="button secondary" disabled={photos.length < 2 || locked} onClick={syncToAll}>同步到全部</button>
            <button className="button" disabled={photos.length === 0 || locked} onClick={() => void runBatch()}>批量导出</button>
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
