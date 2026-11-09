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
  return {
    async write(name, bytes) {
      const file = await folder.getFileHandle(name, { create: true });
      const stream = await file.createWritable();
      await stream.write(bytes);
      await stream.close();
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
