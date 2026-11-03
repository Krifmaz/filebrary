/**
 * Chunked, resumable uploads.
 *
 * Each file is sent in fixed-size chunks to an upload session on the server.
 * The session id is remembered in `localStorage` under a key built from the
 * destination and the file's size and modification time, so an upload that
 * was interrupted (closed tab, lost network) continues from the last byte the
 * server stored when the same file is picked again.
 *
 * The {@link UploadManager} runs a small number of files at once, retries
 * failed chunks with exponential backoff, and exposes its state through a
 * subscribe/snapshot pair that fits React's `useSyncExternalStore`.
 */
import { api, ApiError } from "./api";
import { join } from "./paths";

/** Size of one chunk. Large enough to be efficient, small enough to retry cheaply. */
export const CHUNK_SIZE = 8 * 1024 * 1024;
/** How many files upload in parallel. */
export const CONCURRENCY = 3;
/** How many times one chunk is retried before the file is marked failed. */
export const MAX_RETRIES = 5;

/** A file picked for upload, with its path relative to the target folder. */
export interface PickedFile {
  file: File;
  /** Relative path such as `photos/2026/a.jpg`; just the name for plain files. */
  relativePath: string;
}

/** Lifecycle state of one upload. */
export type UploadStatus = "queued" | "uploading" | "paused" | "done" | "failed" | "cancelled";

/** One item in the upload queue. */
export interface UploadItem {
  key: string;
  source: string;
  /** Destination path inside the source. */
  path: string;
  name: string;
  size: number;
  sent: number;
  status: UploadStatus;
  error?: string;
}

/** The minimal API the manager needs; replaced with a fake in tests. */
export interface UploadApi {
  uploadStart: typeof api.uploadStart;
  uploadStatus: typeof api.uploadStatus;
  uploadChunk: typeof api.uploadChunk;
  uploadCancel: typeof api.uploadCancel;
}

/** Storage for resumable session ids; `localStorage` by default. */
export interface KeyStore {
  get(key: string): string | null;
  set(key: string, value: string): void;
  delete(key: string): void;
}

/** A {@link KeyStore} backed by `localStorage`, ignoring storage errors. */
export const localKeyStore: KeyStore = {
  get: (k) => {
    try {
      return localStorage.getItem(k);
    } catch {
      return null;
    }
  },
  set: (k, v) => {
    try {
      localStorage.setItem(k, v);
    } catch {
      /* private mode or quota: resume just won't survive a reload */
    }
  },
  delete: (k) => {
    try {
      localStorage.removeItem(k);
    } catch {
      /* ignore */
    }
  },
};

/** Builds the key that identifies "this file going to this place". */
export function resumeKey(source: string, path: string, file: File): string {
  return `filebrary.upload:${source}:${path}:${file.size}:${file.lastModified}`;
}

const sleep = (ms: number, signal?: AbortSignal) =>
  new Promise<void>((resolve, reject) => {
    const t = setTimeout(resolve, ms);
    signal?.addEventListener("abort", () => {
      clearTimeout(t);
      reject(new DOMException("Aborted", "AbortError"));
    });
  });

interface Job {
  item: UploadItem;
  file: File;
  abort?: AbortController;
  sessionId?: string;
}

/** Queues and runs uploads, notifying subscribers on every change. */
export class UploadManager {
  private jobs: Job[] = [];
  private listeners = new Set<() => void>();
  private snapshot: UploadItem[] = [];
  private running = 0;
  private readonly api: UploadApi;
  private readonly store: KeyStore;
  private readonly backoffMs: number;
  /** Called after each file finishes, with its source and destination path. */
  private readonly onComplete: (source: string, path: string) => void;

  constructor(
    opts: {
      api?: UploadApi;
      store?: KeyStore;
      backoffMs?: number;
      onComplete?: (source: string, path: string) => void;
    } = {},
  ) {
    this.onComplete = opts.onComplete ?? (() => {});
    this.api = opts.api ?? api;
    this.store = opts.store ?? localKeyStore;
    this.backoffMs = opts.backoffMs ?? 500;
  }

  /** Registers a change listener; returns an unsubscribe function. */
  subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };

  /** Returns the current queue (a new array after each change). */
  getSnapshot = (): UploadItem[] => this.snapshot;

  private emit() {
    this.snapshot = this.jobs.map((j) => ({ ...j.item }));
    for (const fn of this.listeners) fn();
  }

  /** Adds files to the queue, uploading them into `dir` of `source`. */
  add(source: string, dir: string, files: PickedFile[]): void {
    for (const { file, relativePath } of files) {
      const path = join(dir, relativePath);
      const key = `${source}:${path}:${Date.now()}:${Math.random()}`;
      this.jobs.push({
        file,
        item: { key, source, path, name: relativePath, size: file.size, sent: 0, status: "queued" },
      });
    }
    this.emit();
    this.pump();
  }

  /** Pauses an upload; it can be resumed from where it stopped. */
  pause(key: string): void {
    const job = this.find(key);
    if (!job || (job.item.status !== "uploading" && job.item.status !== "queued")) return;
    job.item.status = "paused";
    job.abort?.abort();
    this.emit();
  }

  /** Resumes a paused or failed upload. */
  resume(key: string): void {
    const job = this.find(key);
    if (!job || (job.item.status !== "paused" && job.item.status !== "failed")) return;
    job.item.status = "queued";
    job.item.error = undefined;
    this.emit();
    this.pump();
  }

  /** Cancels an upload and asks the server to discard what it stored. */
  cancel(key: string): void {
    const job = this.find(key);
    if (!job || job.item.status === "done") return;
    job.item.status = "cancelled";
    job.abort?.abort();
    const id = job.sessionId ?? this.store.get(resumeKey(job.item.source, job.item.path, job.file));
    if (id) this.api.uploadCancel(id).catch(() => {});
    this.store.delete(resumeKey(job.item.source, job.item.path, job.file));
    this.emit();
  }

  /** Removes finished, failed and cancelled items from the list. */
  clearFinished(): void {
    this.jobs = this.jobs.filter((j) => ["queued", "uploading", "paused"].includes(j.item.status));
    this.emit();
  }

  private find(key: string) {
    return this.jobs.find((j) => j.item.key === key);
  }

  private pump() {
    while (this.running < CONCURRENCY) {
      const next = this.jobs.find((j) => j.item.status === "queued");
      if (!next) return;
      this.running++;
      next.item.status = "uploading";
      this.emit();
      void this.run(next).finally(() => {
        this.running--;
        this.pump();
      });
    }
  }

  /** Finds or creates the server session and returns the offset to resume from. */
  private async open(job: Job): Promise<number> {
    const { source, path } = job.item;
    const key = resumeKey(source, path, job.file);
    const saved = this.store.get(key);
    if (saved) {
      try {
        const s = await this.api.uploadStatus(saved);
        job.sessionId = saved;
        return s.offset;
      } catch (e) {
        if (!(e instanceof ApiError) || e.status !== 404) throw e;
        this.store.delete(key);
      }
    }
    const s = await this.api.uploadStart(source, path, job.file.size);
    job.sessionId = s.id;
    this.store.set(key, s.id);
    return s.offset;
  }

  private async run(job: Job) {
    const abort = new AbortController();
    job.abort = abort;
    const key = resumeKey(job.item.source, job.item.path, job.file);
    try {
      let offset = await this.open(job);
      job.item.sent = offset;
      this.emit();
      let failures = 0;
      while (offset < job.file.size) {
        const chunk = job.file.slice(offset, Math.min(offset + CHUNK_SIZE, job.file.size));
        try {
          const r = await this.api.uploadChunk(job.sessionId!, offset, chunk, abort.signal);
          offset = r.offset;
          failures = 0;
        } catch (e) {
          if (abort.signal.aborted) throw e;
          // 409: the server's offset differs from ours; ask and continue from there.
          if (e instanceof ApiError && e.status === 409) {
            offset = (await this.api.uploadStatus(job.sessionId!)).offset;
          } else if (e instanceof ApiError && e.status >= 400 && e.status < 500 && e.status !== 408 && e.status !== 429) {
            throw e;
          } else if (++failures > MAX_RETRIES) {
            throw e;
          } else {
            await sleep(this.backoffMs * 2 ** (failures - 1), abort.signal);
          }
        }
        job.item.sent = offset;
        this.emit();
      }
      // A zero-byte upload is complete as soon as the server creates it.
      this.store.delete(key);
      job.item.status = "done";
      this.emit();
      this.onComplete(job.item.source, job.item.path);
    } catch (e) {
      if (abort.signal.aborted) return; // paused or cancelled: status already set
      job.item.status = "failed";
      job.item.error = e instanceof Error ? e.message : String(e);
      this.emit();
    }
  }
}

/**
 * Collects files from a drag-and-drop, walking into dropped folders.
 * Falls back to the flat file list when the browser lacks the entries API.
 */
export async function filesFromDrop(dt: DataTransfer): Promise<PickedFile[]> {
  const entries = Array.from(dt.items)
    .filter((i) => i.kind === "file")
    .map((i) => i.webkitGetAsEntry?.())
    .filter((e): e is FileSystemEntry => !!e);
  if (entries.length === 0) {
    return Array.from(dt.files).map((file) => ({ file, relativePath: file.name }));
  }
  const out: PickedFile[] = [];
  const walk = async (entry: FileSystemEntry, prefix: string): Promise<void> => {
    if (entry.isFile) {
      const file = await new Promise<File>((res, rej) => (entry as FileSystemFileEntry).file(res, rej));
      out.push({ file, relativePath: prefix + entry.name });
    } else if (entry.isDirectory) {
      const reader = (entry as FileSystemDirectoryEntry).createReader();
      // readEntries returns results in batches; keep reading until empty.
      for (;;) {
        const batch = await new Promise<FileSystemEntry[]>((res, rej) => reader.readEntries(res, rej));
        if (batch.length === 0) break;
        for (const child of batch) await walk(child, `${prefix}${entry.name}/`);
      }
    }
  };
  for (const e of entries) await walk(e, "");
  return out;
}

/** Converts an `<input type=file>` selection, keeping folder structure if present. */
export function filesFromInput(list: FileList): PickedFile[] {
  return Array.from(list).map((file) => ({
    file,
    relativePath: file.webkitRelativePath || file.name,
  }));
}
