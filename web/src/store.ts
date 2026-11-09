import type { Lut } from "./lut";

const DB_NAME = "openpixels";
const VERSION = 1;
const HANDLES = "handles";
const LUTS = "luts";

export interface StoredLut extends Lut {
  id: number;
  name: string;
}

function open(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, VERSION);
    request.onupgradeneeded = () => {
      request.result.createObjectStore(HANDLES);
      request.result.createObjectStore(LUTS, { keyPath: "id", autoIncrement: true });
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

async function run<T>(store: string, mode: IDBTransactionMode, work: (s: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  const db = await open();
  return new Promise((resolve, reject) => {
    const request = work(db.transaction(store, mode).objectStore(store));
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

/** Imported LUT ids start here, clear of the built-in looks. */
const FIRST_IMPORTED_ID = 100;

export async function saveLut(name: string, lut: Lut): Promise<StoredLut> {
  const existing = await listLuts();
  const id = Math.max(FIRST_IMPORTED_ID - 1, ...existing.map((item) => item.id)) + 1;
  const stored: StoredLut = { id, name, size: lut.size, data: lut.data };
  await run(LUTS, "readwrite", (s) => s.put(stored));
  return stored;
}

export const listLuts = () => run<StoredLut[]>(LUTS, "readonly", (s) => s.getAll());
export const getLut = (id: number) => run<StoredLut | undefined>(LUTS, "readonly", (s) => s.get(id));

export const rememberHandle = (id: string, handle: FileSystemFileHandle) =>
  run(HANDLES, "readwrite", (s) => s.put(handle, id));

export const recallHandles = () => run<FileSystemFileHandle[]>(HANDLES, "readonly", (s) => s.getAll());
export const forgetHandles = () => run(HANDLES, "readwrite", (s) => s.clear());
