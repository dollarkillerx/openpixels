import { useCallback, useEffect, useRef, useState } from "react";
import { MODEL, modelInputSize } from "./model";
import type { Adjustments, FromAi, FromWorker, LoadResult, ToAi, ToWorker } from "./protocol";

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
      { key: "vignette", label: "暗角", min: -1, max: 1 },
    ],
  },
];

const JPEG_QUALITY = 92;
const FRAME_WINDOW = 30;

interface MaskStats {
  inputMs: number;
  loadMs: number;
  inferMs: number;
}

export function App() {
  const stageRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const workerRef = useRef<Worker | null>(null);
  const aiWorkerRef = useRef<Worker | null>(null);
  const framesRef = useRef<number[]>([]);
  const generationRef = useRef(0);
  const inputMsRef = useRef(0);

  const [ready, setReady] = useState(false);
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [adjustments, setAdjustments] = useState<Adjustments>({});
  const [loaded, setLoaded] = useState<LoadResult | null>(null);
  const [heapMB, setHeapMB] = useState(0);
  const [frame, setFrame] = useState<{ last: number; median: number } | null>(null);
  const [exported, setExported] = useState<{ url: string; ms: number; sizeMB: number } | null>(null);
  const [detecting, setDetecting] = useState(false);
  const [maskStats, setMaskStats] = useState<MaskStats | null>(null);

  const send = useCallback((message: ToWorker, transfer: Transferable[] = []) => {
    workerRef.current?.postMessage(message, transfer);
  }, []);

  const loadBuffer = useCallback(
    (buffer: ArrayBuffer, name: string) => {
      generationRef.current += 1;
      setBusy("解码中…");
      setError("");
      setExported(null);
      setDetecting(false);
      setMaskStats(null);
      send({ type: "load", buffer, name }, [buffer]);
    },
    [send],
  );

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || workerRef.current) return;

    const worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
    workerRef.current = worker;

    worker.onmessage = ({ data }: MessageEvent<FromWorker>) => {
      switch (data.type) {
        case "ready":
          setReady(true);
          break;
        case "loaded":
          setLoaded(data.result);
          setHeapMB(data.heapMB);
          setBusy("");
          break;
        case "frame": {
          const frames = framesRef.current;
          frames.push(data.ms);
          if (frames.length > FRAME_WINDOW) frames.shift();
          const sorted = [...frames].sort((a, b) => a - b);
          setFrame({ last: data.ms, median: sorted[Math.floor(sorted.length / 2)] });
          break;
        }
        case "exported": {
          const blob = new Blob([data.jpeg], { type: "image/jpeg" });
          setExported({ url: URL.createObjectURL(blob), ms: data.ms, sizeMB: blob.size / 1048576 });
          setHeapMB(data.heapMB);
          setBusy("");
          break;
        }
        case "modelInput": {
          inputMsRef.current = data.ms;
          const { tensor, width, height, generation } = data;
          const message: ToAi = { type: "segment", tensor, width, height, generation };
          aiWorker.postMessage(message, [tensor]);
          break;
        }
        case "error":
          setError(data.message);
          setBusy("");
          setDetecting(false);
          break;
      }
    };
    worker.onerror = (event) => setError(event.message);

    const aiWorker = new Worker(new URL("./ai-worker.ts", import.meta.url), { type: "module" });
    aiWorkerRef.current = aiWorker;
    aiWorker.onmessage = ({ data }: MessageEvent<FromAi>) => {
      if (data.type === "error") {
        setError(`主体识别失败：${data.message}`);
        setDetecting(false);
        return;
      }
      if (data.generation !== generationRef.current) return;
      const { mask, width, height, loadMs, inferMs } = data;
      worker.postMessage({ type: "setMask", mask, width, height } satisfies ToWorker, [mask]);
      setMaskStats({ inputMs: inputMsRef.current, loadMs, inferMs });
      setDetecting(false);
    };
    aiWorker.onerror = (event) => {
      setError(`主体识别失败：${event.message}`);
      setDetecting(false);
    };

    const offscreen = canvas.transferControlToOffscreen();
    worker.postMessage({ type: "init", canvas: offscreen } satisfies ToWorker, [offscreen]);
  }, []);

  useEffect(() => {
    const stage = stageRef.current;
    if (!stage || !ready) return;
    const observer = new ResizeObserver(([entry]) => {
      const ratio = window.devicePixelRatio;
      send({
        type: "resize",
        width: Math.round(entry.contentRect.width * ratio),
        height: Math.round(entry.contentRect.height * ratio),
      });
    });
    observer.observe(stage);
    return () => observer.disconnect();
  }, [ready, send]);

  // Development shortcut: /?fixture=/fixtures/name.dng loads a file without a picker.
  useEffect(() => {
    if (!ready || !import.meta.env.DEV) return;
    const fixture = new URLSearchParams(location.search).get("fixture");
    if (!fixture) return;
    fetch(fixture)
      .then((response) => {
        if (!response.ok) throw new Error(`${fixture}: HTTP ${response.status}`);
        return response.arrayBuffer();
      })
      .then((buffer) => loadBuffer(buffer, fixture))
      .catch((reason: Error) => setError(reason.message));
  }, [ready, loadBuffer]);

  const openFile = async (file: File | undefined) => {
    if (file) loadBuffer(await file.arrayBuffer(), file.name);
  };

  const setValue = (key: string, value: number) => {
    const next = { ...adjustments, [key]: value };
    setAdjustments(next);
    send({ type: "adjust", adjustments: next });
  };

  const reset = () => {
    setAdjustments({});
    send({ type: "adjust", adjustments: {} });
  };

  const detectSubject = () => {
    if (!loaded) return;
    setDetecting(true);
    setError("");
    send({
      type: "modelInput",
      ...modelInputSize(loaded.proxyWidth, loaded.proxyHeight),
      mean: MODEL.mean,
      std: MODEL.std,
      generation: generationRef.current,
    });
  };

  const backgroundExposure = adjustments.backgroundExposure ?? 0;

  const startExport = () => {
    setBusy("导出中…");
    setExported(null);
    send({ type: "export", quality: JPEG_QUALITY });
  };

  return (
    <div className="app">
      <main
        className="stage"
        ref={stageRef}
        onDragOver={(event) => event.preventDefault()}
        onDrop={(event) => {
          event.preventDefault();
          void openFile(event.dataTransfer.files[0]);
        }}
      >
        <canvas ref={canvasRef} />
        {!loaded && !busy && <p className="hint">{ready ? "拖入 RAW、JPEG 或 PNG" : "正在初始化 WebGPU…"}</p>}
        {busy && <p className="hint">{busy}</p>}
      </main>

      <aside className="panel">
        <header>
          <h1>OpenPixels</h1>
          <label className={`button ${ready ? "" : "disabled"}`}>
            打开图片
            <input
              type="file"
              hidden
              disabled={!ready}
              onChange={(event) => void openFile(event.target.files?.[0])}
            />
          </label>
        </header>

        {error && <p className="error" role="alert">{error}</p>}

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
                    disabled={!loaded}
                    onChange={(event) => setValue(slider.key, Number(event.target.value))}
                    onDoubleClick={() => setValue(slider.key, 0)}
                  />
                </label>
              );
            })}
          </section>
        ))}

        <section>
          <h2>AI 主体</h2>
          <button className="button" disabled={!loaded || detecting} onClick={detectSubject}>
            {detecting ? "识别中…" : maskStats ? "重新识别" : "识别主体"}
          </button>
          <label className="check">
            <input
              type="checkbox"
              aria-label="显示蒙版"
              disabled={!maskStats}
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
              disabled={!maskStats}
              onChange={(event) => setValue("backgroundExposure", Number(event.target.value))}
              onDoubleClick={() => setValue("backgroundExposure", 0)}
            />
          </label>
        </section>

        <section className="actions">
          <button className="button secondary" disabled={!loaded} onClick={reset}>重置</button>
          <button className="button" disabled={!loaded || !!busy} onClick={startExport}>导出 JPEG</button>
        </section>
        {exported && (
          <a className="download" href={exported.url} download="openpixels-export.jpg">下载导出结果</a>
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
