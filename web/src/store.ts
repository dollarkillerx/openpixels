import type { PhotoAnalysis } from "./analysis";
import type { Lut } from "./lut";
import type { ErasePatch, ProfileInfo } from "./protocol";

const DB_NAME = "openpixels";
const VERSION = 5;
const HANDLES = "handles";
const LUTS = "luts";
const PROJECTS = "projects";
const ERASES = "erases";
const ASSETS = "assets";
const ANALYSES = "analyses";
const PROFILES = "profiles";

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
      if (!db.objectStoreNames.contains(ASSETS)) db.createObjectStore(ASSETS, { keyPath: "id", autoIncrement: true });
      if (!db.objectStoreNames.contains(ANALYSES)) db.createObjectStore(ANALYSES);
      if (!db.objectStoreNames.contains(PROFILES)) db.createObjectStore(PROFILES, { keyPath: "id", autoIncrement: true });
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

/** An imported ICC profile for soft proofing. */
export interface StoredProfile extends ProfileInfo {
  id: number;
  bytes: ArrayBuffer;
}

export async function saveProfile(profile: Omit<StoredProfile, "id">): Promise<StoredProfile> {
  const id = await run<IDBValidKey>(PROFILES, "readwrite", (s) => s.add(profile));
  return { ...profile, id: Number(id) };
}

export const listProfiles = () => run<StoredProfile[]>(PROFILES, "readonly", (s) => s.getAll());
export const deleteProfile = (id: number) => run(PROFILES, "readwrite", (s) => s.delete(id));

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

/** An imported image for replacing backgrounds and skies. */
export interface StoredAsset {
  id: number;
  name: string;
  mode: number;
  blob: Blob;
}

/** Imported ids start here, clear of the built-in images. */
const FIRST_ASSET_ID = 100;

export async function saveAsset(name: string, mode: number, blob: Blob): Promise<StoredAsset> {
  const existing = await listAssets();
  const id = Math.max(FIRST_ASSET_ID - 1, ...existing.map((item) => item.id)) + 1;
  const stored: StoredAsset = { id, name, mode, blob };
  await run(ASSETS, "readwrite", (s) => s.put(stored));
  return stored;
}

export const listAssets = () => run<StoredAsset[]>(ASSETS, "readonly", (s) => s.getAll());
export const getAsset = (id: number) => run<StoredAsset | undefined>(ASSETS, "readonly", (s) => s.get(id));

export const saveAnalysis = (id: string, analysis: PhotoAnalysis) => run(ANALYSES, "readwrite", (s) => s.put(analysis, id));

/** Every stored analysis, by photo id. */
export async function loadAnalyses(): Promise<Record<string, PhotoAnalysis>> {
  const [ids, values] = await Promise.all([
    run<IDBValidKey[]>(ANALYSES, "readonly", (s) => s.getAllKeys()),
    run<PhotoAnalysis[]>(ANALYSES, "readonly", (s) => s.getAll()),
  ]);
  return Object.fromEntries(ids.map((id, index) => [String(id), values[index]]));
}

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
