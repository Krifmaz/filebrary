/**
 * View modes for the file listing, and where the chosen mode is remembered.
 *
 * The listing can show entries as a details table (sortable columns), a
 * compact multi-column list, or a grid of small, medium or large tiles with
 * thumbnails. The choice is stored in `localStorage` per signed-in user:
 * picking a mode makes it that user's default and also records it for the
 * current folder, so a photo folder can stay on large tiles while everything
 * else opens as details. Only the most recent {@link MAX_FOLDERS} folders are
 * kept so the stored value stays small.
 */
import type { Entry } from "../../api";

/** Every view mode, in the order the switcher shows them. */
export const VIEW_MODES = ["details", "compact", "small", "medium", "large"] as const;

/** One way of laying out the listing. */
export type ViewMode = (typeof VIEW_MODES)[number];

/** Labels and keyboard shortcuts for each mode. */
export const VIEW_INFO: Record<ViewMode, { label: string; icon: string; key: string }> = {
  details: { label: "Details", icon: "☰", key: "1" },
  compact: { label: "Compact list", icon: "≣", key: "2" },
  small: { label: "Small icons", icon: "▦", key: "3" },
  medium: { label: "Medium icons", icon: "▣", key: "4" },
  large: { label: "Large icons", icon: "⬛", key: "5" },
};

/** Whether a mode lays entries out as a tile grid. */
export function isGrid(mode: ViewMode): boolean {
  return mode === "small" || mode === "medium" || mode === "large";
}

/** The mode bound to a shortcut key (`1`–`5`), if any. */
export function modeForKey(key: string): ViewMode | undefined {
  return VIEW_MODES.find((m) => VIEW_INFO[m].key === key);
}

/** The mode after `mode`, wrapping around; used by the `V` shortcut. */
export function nextMode(mode: ViewMode, step = 1): ViewMode {
  const i = VIEW_MODES.indexOf(mode);
  const n = VIEW_MODES.length;
  return VIEW_MODES[(((i + step) % n) + n) % n] ?? "details";
}

/** How many folder-specific choices are kept per user. */
export const MAX_FOLDERS = 200;

/** What is stored for one user. */
interface Stored {
  default: ViewMode;
  /** `source:path` → mode, oldest first. */
  folders: Record<string, ViewMode>;
}

const isMode = (v: unknown): v is ViewMode => typeof v === "string" && (VIEW_MODES as readonly string[]).includes(v);

const storageKey = (user: number | string) => `filebrary.view.${user}`;
const folderKey = (source: string, path: string) => `${source}:${path}`;

/** Reads a user's stored choices, tolerating missing or corrupt data. */
function read(store: Storage | undefined, user: number | string): Stored {
  const empty: Stored = { default: "details", folders: {} };
  try {
    const raw = store?.getItem(storageKey(user));
    if (!raw) return empty;
    const v = JSON.parse(raw) as Partial<Stored>;
    const folders: Record<string, ViewMode> = {};
    if (v.folders && typeof v.folders === "object") {
      for (const [k, m] of Object.entries(v.folders)) if (isMode(m)) folders[k] = m;
    }
    return { default: isMode(v.default) ? v.default : "details", folders };
  } catch {
    return empty;
  }
}

/** `localStorage`, or nothing when the browser blocks it. */
function defaultStore(): Storage | undefined {
  try {
    return globalThis.localStorage;
  } catch {
    return undefined;
  }
}

/**
 * The mode to open a folder in: the folder's own choice if one was made,
 * otherwise the user's default, otherwise `details`.
 */
export function loadViewMode(user: number | string, source: string, path: string, store = defaultStore()): ViewMode {
  const s = read(store, user);
  return s.folders[folderKey(source, path)] ?? s.default;
}

/**
 * Remembers `mode` for one folder and makes it the user's default. Storage
 * errors (quota, private mode) are ignored: the mode still applies until the
 * page is reloaded.
 */
export function saveViewMode(user: number | string, source: string, path: string, mode: ViewMode, store = defaultStore()): void {
  const s = read(store, user);
  const key = folderKey(source, path);
  // Re-insert so the key moves to the end (most recent).
  delete s.folders[key];
  s.folders[key] = mode;
  const keys = Object.keys(s.folders);
  for (const k of keys.slice(0, Math.max(0, keys.length - MAX_FOLDERS))) delete s.folders[k];
  s.default = mode;
  try {
    store?.setItem(storageKey(user), JSON.stringify(s));
  } catch {
    // Not fatal; see above.
  }
}

/** File kinds the preview service can make thumbnails for. */
export type PreviewKind = "image" | "raw" | "heif" | "video" | "audio" | "pdf" | "office";

const KIND_EXT: Record<PreviewKind, string[]> = {
  image: ["jpg", "jpeg", "jpe", "jfif", "png", "apng", "gif", "webp", "tif", "tiff", "bmp", "ico"],
  raw: ["cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "dng", "raf", "orf", "rw2", "pef", "srw", "x3f", "3fr", "iiq", "erf", "kdc", "mrw", "mos", "rwl"],
  heif: ["heic", "heif", "hif", "avif"],
  video: ["mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "flv", "mpg", "mpeg", "m2ts", "mts", "ts", "3gp", "ogv"],
  audio: ["mp3", "flac", "ogg", "oga", "opus", "m4a", "aac", "wav", "wma", "aiff", "aif", "alac", "ape", "wv"],
  pdf: ["pdf"],
  office: ["doc", "docx", "odt", "rtf", "xls", "xlsx", "ods", "csv", "ppt", "pptx", "odp", "odg", "vsd", "vsdx", "pages", "numbers", "key"],
};

const EXT_KIND = new Map<string, PreviewKind>(
  (Object.entries(KIND_EXT) as [PreviewKind, string[]][]).flatMap(([k, exts]) => exts.map((e) => [e, k] as const)),
);

/**
 * The preview kind of a file, judged by its extension the same way the
 * server's `FileKind::detect` does. Folders and unknown files have none.
 */
export function previewKind(entry: Pick<Entry, "name" | "is_dir">): PreviewKind | undefined {
  if (entry.is_dir) return undefined;
  const dot = entry.name.lastIndexOf(".");
  if (dot <= 0) return undefined;
  return EXT_KIND.get(entry.name.slice(dot + 1).toLowerCase());
}

/**
 * Sets the user's default mode (from their saved preference) without
 * touching folder-specific choices.
 */
export function setDefaultViewMode(user: number | string, mode: ViewMode, store = defaultStore()): void {
  if (!isMode(mode)) return;
  const s = read(store, user);
  s.default = mode;
  try {
    store?.setItem(storageKey(user), JSON.stringify(s));
  } catch {
    // Not fatal.
  }
}
