import { freeName } from "./names";
import { createZip } from "./zip";

/** Where a batch export writes its files. */
export interface BatchTarget {
  write(name: string, bytes: Uint8Array<ArrayBuffer>): Promise<void>;
  /** Returns a URL to download the result, when there is something to download. */
  finish(): Promise<string | null>;
}

type DirectoryPicker = (options: { mode: "readwrite" }) => Promise<FileSystemDirectoryHandle>;

/**
 * Writes straight into a folder where the browser allows it, so a large batch need
 * not fit in memory, and otherwise collects a ZIP. Returns null if the user cancels.
 * Must be called from a user gesture.
 */
export async function chooseBatchTarget(): Promise<BatchTarget | null> {
  const picker = (window as unknown as { showDirectoryPicker?: DirectoryPicker }).showDirectoryPicker;
  if (!picker) return zipTarget();

  let folder: FileSystemDirectoryHandle;
  try {
    folder = await picker.call(window, { mode: "readwrite" });
  } catch (error) {
    if (error instanceof DOMException && error.name === "AbortError") return null;
    throw error;
  }
  // Never overwrite: the folder may hold the originals or an earlier export. Names are
  // compared without case, as macOS and Windows do.
  const taken = new Set<string>();
  for await (const existing of (folder as unknown as { keys(): AsyncIterable<string> }).keys()) taken.add(existing.toLowerCase());
  return {
    async write(name, bytes) {
      const free = freeName(name, taken);
      taken.add(free.toLowerCase());
      const file = await folder.getFileHandle(free, { create: true });
      const stream = await file.createWritable();
      try {
        await stream.write(bytes);
        await stream.close();
      } catch (error) {
        await stream.abort().catch(() => {});
        throw error;
      }
    },
    finish: async () => null,
  };
}

function zipTarget(): BatchTarget {
  const files: { name: string; bytes: Uint8Array<ArrayBuffer> }[] = [];
  return {
    async write(name, bytes) {
      files.push({ name, bytes });
    },
    finish: async () => URL.createObjectURL(createZip(files)),
  };
}
