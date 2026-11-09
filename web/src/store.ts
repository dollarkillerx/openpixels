import type { Lut } from "./lut";
import type { ErasePatch } from "./protocol";

const DB_NAME = "openpixels";
const VERSION = 3;
const HANDLES = "handles";
const LUTS = "luts";
const PROJECTS = "projects";
const ERASES = "erases";

export interface StoredLut extends Lut {
  id: number;
  name: string;
}

function open(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, VERSION);
    request.onupgradeneeded = () => {
      const db = request.result;
      if (!db.objectStoreNames.contains(HANDLES)) db.createObjectStore(HANDLES);
      if (!db.objectStoreNames.contains(LUTS)) db.createObjectStore(LUTS, { keyPath: "id", autoIncrement: true });
      if (!db.objectStoreNames.contains(PROJECTS)) db.createObjectStore(PROJECTS, { keyPath: "id", autoIncrement: true });
      if (!db.objectStoreNames.contains(ERASES)) db.createObjectStore(ERASES);
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

async function run<T>(store: string, mode: IDBTransactionMode, work: (s: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  const db = await open();
  return new Promise((resolve, reject) => {
    const request = work(db.transaction(store, mode).objectStore(store));
    // Lets the transaction finish; an open connection would block later version upgrades.
    db.close();
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

/** Kept here rather than with the other edits: each patch is about a megabyte. */
export const loadErases = async (id: string) =>
  (await run<ErasePatch[] | undefined>(ERASES, "readonly", (s) => s.get(id))) ?? [];

export const saveErases = (id: string, erases: ErasePatch[]) =>
  erases.length > 0
    ? run(ERASES, "readwrite", (s) => s.put(erases, id))
    : run(ERASES, "readwrite", (s) => s.delete(id));

export const rememberHandle = (id: string, handle: FileSystemFileHandle) =>
  run(HANDLES, "readwrite", (s) => s.put(handle, id));

export const recallHandle = (id: string) => run<FileSystemFileHandle | undefined>(HANDLES, "readonly", (s) => s.get(id));
export const forgetHandle = (id: string) => run(HANDLES, "readwrite", (s) => s.delete(id));

/** Every remembered photo, with its id. */
export async function recallHandleEntries(): Promise<{ id: string; handle: FileSystemFileHandle }[]> {
  const [ids, handles] = await Promise.all([
    run<IDBValidKey[]>(HANDLES, "readonly", (s) => s.getAllKeys()),
    run<FileSystemFileHandle[]>(HANDLES, "readonly", (s) => s.getAll()),
  ]);
  return ids.map((id, index) => ({ id: String(id), handle: handles[index] }));
}

/** A named group of photos. Their edits stay keyed by photo, so projects can share them. */
export interface Project {
  id: number;
  name: string;
  photoIds: string[];
  /** Data URL of a small preview. */
  cover?: string;
  updatedAt: number;
}

export const listProjects = () => run<Project[]>(PROJECTS, "readonly", (s) => s.getAll());
export const deleteProject = (id: number) => run(PROJECTS, "readwrite", (s) => s.delete(id));

export async function saveProject(project: Omit<Project, "id"> & { id?: number }): Promise<Project> {
  const record = { ...project, updatedAt: Date.now() };
  const id = await run<IDBValidKey>(PROJECTS, "readwrite", (s) => s.put(record));
  return { ...record, id: Number(id) };
}
