/// <reference lib="webworker" />
import init, { process_develop_tile } from "./wasm-pkg/op_wasm";
import type { DevelopTileReply, DevelopTileRequest } from "./protocol";

const ready = init();

self.onmessage = async ({ data }: MessageEvent<DevelopTileRequest>) => {
  const reply = (message: DevelopTileReply, transfer: Transferable[] = []) =>
    self.postMessage(message, { transfer });
  try {
    await ready;
    const result = process_develop_tile(new Float32Array(data.input), data.tileJson, data.recipeJson);
    const rgb = result.buffer.slice(result.byteOffset, result.byteOffset + result.byteLength) as ArrayBuffer;
    reply({ index: data.index, rgb }, [rgb]);
  } catch (error) {
    reply({ index: data.index, error: error instanceof Error ? error.message : String(error) });
  }
};
