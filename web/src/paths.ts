/**
 * Path and formatting helpers for the web UI.
 *
 * Paths inside a source are always absolute and `/`-separated. These helpers
 * only shape paths for display and requests; the server does the real
 * normalisation and safety checks.
 */

/** Joins path segments into one absolute path, collapsing duplicate slashes. */
export function join(...parts: string[]): string {
  const segs = parts.flatMap((p) => p.split("/")).filter((s) => s !== "" && s !== ".");
  return "/" + segs.join("/");
}

/** Returns the parent directory of `path` (`/` stays `/`). */
export function parent(path: string): string {
  const segs = path.split("/").filter(Boolean);
  segs.pop();
  return "/" + segs.join("/");
}

/** Returns the last segment of `path`. */
export function basename(path: string): string {
  const segs = path.split("/").filter(Boolean);
  return segs[segs.length - 1] ?? "";
}

/** Splits a path into breadcrumbs: `[{name, path}]`, starting at the root. */
export function crumbs(path: string): { name: string; path: string }[] {
  const out = [{ name: "/", path: "/" }];
  let acc = "";
  for (const s of path.split("/").filter(Boolean)) {
    acc += "/" + s;
    out.push({ name: s, path: acc });
  }
  return out;
}

/**
 * Checks that a single file name is usable: not empty, not `.` or `..`, and
 * without slashes or control characters. Returns an error message or `null`.
 */
export function validateName(name: string): string | null {
  if (!name.trim()) return "Name cannot be empty";
  if (name === "." || name === "..") return "Name cannot be . or ..";
  if (name.includes("/") || name.includes("\\")) return "Name cannot contain slashes";
  // eslint-disable-next-line no-control-regex
  if (/[\x00-\x1f\x7f]/.test(name)) return "Name cannot contain control characters";
  if (name.length > 255) return "Name is too long";
  return null;
}

/** Formats a byte count, e.g. `1.5 MB`. */
export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "";
  if (bytes < 1000) return `${bytes} B`;
  const units = ["kB", "MB", "GB", "TB", "PB"];
  let v = bytes;
  let i = -1;
  do {
    v /= 1000;
    i++;
  } while (v >= 1000 && i < units.length - 1);
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

/** Formats an RFC 3339 timestamp in the user's locale. */
export function formatDate(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

/** Picks a small icon for an entry based on its type and extension. */
export function iconFor(name: string, isDir: boolean): string {
  if (isDir) return "📁";
  const ext = name.toLowerCase().split(".").pop() ?? "";
  if (["png", "jpg", "jpeg", "gif", "webp", "svg", "heic", "avif", "bmp"].includes(ext)) return "🖼️";
  if (["mp4", "mkv", "webm", "mov", "avi", "m4v"].includes(ext)) return "🎞️";
  if (["mp3", "flac", "ogg", "wav", "m4a", "opus", "aac"].includes(ext)) return "🎵";
  if (["zip", "tar", "gz", "7z", "rar", "xz", "bz2", "zst"].includes(ext)) return "🗜️";
  if (ext === "pdf") return "📕";
  if (["doc", "docx", "odt", "rtf"].includes(ext)) return "📝";
  if (["xls", "xlsx", "ods", "csv"].includes(ext)) return "📊";
  if (["ppt", "pptx", "odp"].includes(ext)) return "📽️";
  return "📄";
}
