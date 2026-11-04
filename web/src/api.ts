/**
 * Typed client for the Filebrary REST API.
 *
 * Every request the web UI makes goes through this module, so the API
 * contract lives in one place. All endpoints are under `/api`, exchange JSON,
 * and report failures as a non-2xx status with an `{ error }` body.
 *
 * Authentication uses an HttpOnly session cookie set by the server. For
 * CSRF protection, `POST /api/auth/login` and `GET /api/me` return a
 * `csrf_token` that this client keeps in memory and echoes in the
 * `X-CSRF-Token` header on every request that changes state. A readable
 * `filebrary_csrf` cookie is used as a fallback if the server sets one.
 */

/** The signed-in user. */
export interface User {
  id: number;
  username: string;
  is_admin: boolean;
}

/** What the current user may do inside a storage source. */
export interface Permissions {
  view: boolean;
  download: boolean;
  upload: boolean;
  edit: boolean;
  delete: boolean;
  share: boolean;
}

/** A storage source (local folder, S3 bucket, ...) the user can access. */
export interface Source {
  id: string;
  name: string;
  permissions: Permissions;
}

/** One file or folder in a directory listing. */
export interface Entry {
  name: string;
  /** Absolute path inside the source, always starting with `/`. */
  path: string;
  is_dir: boolean;
  size: number;
  /** Last modification time, RFC 3339. */
  modified: string;
  mime?: string | null;
}

/** What the server's preview service can do (`GET /api/previews`). */
export interface PreviewCapabilities {
  enabled: boolean;
  /** Longest side of `thumb` images, in pixels. */
  thumb_size?: number;
  large_size?: number;
  /** File kind (`image`, `video`, `pdf`, ...) → whether it is supported. */
  kinds?: Record<string, boolean>;
  transcode?: boolean;
}

/** The contents of one directory. */
export interface Listing {
  path: string;
  entries: Entry[];
}

/** State of a chunked upload session on the server. */
export interface UploadSession {
  id: string;
  offset: number;
  size?: number;
}

/** A share link as returned by the server. */
export interface Share {
  id: string;
  token: string;
  /** Full public URL of the link. */
  url: string;
  source: string;
  paths: string[];
  expires_at: string | null;
  max_downloads: number | null;
  downloads: number;
  upload_only: boolean;
  has_password: boolean;
  /** Set for a link to a whole collection (then `source` and `paths` are empty). */
  collection_id?: number | null;
  created_at: string;
}

/** Options when creating or editing a share link. */
export interface ShareInput {
  source: string;
  paths: string[];
  password?: string | null;
  expires_at?: string | null;
  max_downloads?: number | null;
  upload_only?: boolean;
}

/** What a visitor sees when opening a public share link. */
export interface PublicShare {
  name: string;
  is_dir: boolean;
  upload_only: boolean;
  needs_password: boolean;
  entries?: Entry[];
}

/** An error reported by the API, carrying the HTTP status. */
export class ApiError extends Error {
  /** HTTP status code (0 for network failures). */
  readonly status: number;
  /** Machine-readable error code from the server, such as `totp_required`. */
  readonly code?: string;

  constructor(status: number, message: string, code?: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
  }
}

/** Reply of the login and "who am I" endpoints. */
export interface SessionInfo {
  user: User;
  csrf_token: string;
}

/** CSRF token of the current session, from the last login or `me` call. */
let sessionCsrf: string | undefined;

/** Name of the readable cookie that carries the CSRF token. */
export const CSRF_COOKIE = "filebrary_csrf";

/** Reads the CSRF token from `document.cookie`, if present. */
export function csrfToken(cookie: string = document.cookie): string | undefined {
  for (const part of cookie.split(";")) {
    const [k, ...v] = part.trim().split("=");
    if (k === CSRF_COOKIE) return decodeURIComponent(v.join("="));
  }
  return undefined;
}

/** Called whenever the server answers 401, so the app can show the login page. */
let onUnauthorized: () => void = () => {};

/** Registers the handler run when a request comes back 401. */
export function setUnauthorizedHandler(fn: () => void): void {
  onUnauthorized = fn;
}

/** Builds request headers, adding the CSRF token for unsafe methods. */
export function headersFor(method: string, extra?: HeadersInit): Headers {
  const h = new Headers(extra);
  if (!["GET", "HEAD", "OPTIONS"].includes(method.toUpperCase())) {
    const t = sessionCsrf ?? csrfToken();
    if (t) h.set("X-CSRF-Token", t);
  }
  return h;
}

/**
 * Sends a request and decodes the JSON reply.
 *
 * @throws {ApiError} for network errors and non-2xx responses.
 */
export async function request<T>(
  method: string,
  url: string,
  body?: unknown,
  init: RequestInit = {},
): Promise<T> {
  const headers = headersFor(method, init.headers);
  let payload: BodyInit | undefined;
  if (body !== undefined) {
    if (body instanceof Blob || body instanceof ArrayBuffer) {
      payload = body;
    } else {
      headers.set("Content-Type", "application/json");
      payload = JSON.stringify(body);
    }
  }
  let res: Response;
  try {
    res = await fetch(url, { ...init, method, headers, body: payload, credentials: "same-origin" });
  } catch (e) {
    if (e instanceof DOMException && e.name === "AbortError") throw e;
    throw new ApiError(0, "Cannot reach the server");
  }
  if (res.status === 401 && !url.startsWith("/api/auth/login") && url !== "/api/me" && !url.startsWith("/api/public/")) {
    onUnauthorized();
  }
  if (!res.ok) {
    let message = res.statusText || `HTTP ${res.status}`;
    let code: string | undefined;
    try {
      // The server sends `{ error: code, message: text }`; older shapes send only `error`.
      const j = (await res.json()) as { error?: string; message?: string };
      if (j.message) message = j.message;
      else if (j.error) message = j.error;
      code = j.error;
    } catch {
      /* body was not JSON */
    }
    throw new ApiError(res.status, message, code);
  }
  if (res.status === 204) return undefined as T;
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

/** Encodes a source id for use in a URL path segment. */
const src = (id: string) => `/api/sources/${encodeURIComponent(id)}`;

/** Builds a query string from repeated or single parameters. */
export function query(params: Record<string, string | string[] | undefined>): string {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v === undefined) continue;
    for (const item of Array.isArray(v) ? v : [v]) q.append(k, item);
  }
  const s = q.toString();
  return s ? `?${s}` : "";
}

/** The API surface used by the UI. */
export const api = {
  /** Signs in with a username, password and optional TOTP code. */
  login: async (username: string, password: string, totp?: string): Promise<User> => {
    const s = await request<SessionInfo>("POST", "/api/auth/login", { username, password, totp: totp || undefined });
    sessionCsrf = s.csrf_token;
    return s.user;
  },
  /** Ends the current session. */
  logout: async () => {
    await request<void>("POST", "/api/auth/logout");
    sessionCsrf = undefined;
  },
  /** Returns the signed-in user, or throws an `ApiError` with status 401. */
  me: async (): Promise<User> => {
    const s = await request<SessionInfo>("GET", "/api/me");
    sessionCsrf = s.csrf_token;
    return s.user;
  },

  /** Lists the sources the user can see. */
  sources: () => request<Source[]>("GET", "/api/sources"),
  /** Lists one directory. */
  list: (source: string, path: string, signal?: AbortSignal) =>
    request<Listing>("GET", `${src(source)}/list${query({ path })}`, undefined, { signal }),
  /** Creates a directory (and any missing parents). */
  mkdir: (source: string, path: string) => request<void>("POST", `${src(source)}/mkdir`, { path }),
  /** Renames or moves a file or folder. */
  move: (source: string, from: string, to: string) =>
    request<void>("POST", `${src(source)}/move`, { from, to }),
  /** Copies a file or folder. */
  copy: (source: string, from: string, to: string) =>
    request<void>("POST", `${src(source)}/copy`, { from, to }),
  /** Deletes files or folders. */
  remove: (source: string, paths: string[]) =>
    request<void>("POST", `${src(source)}/delete`, { paths }),

  /** URL that downloads one file (supports Range requests). */
  downloadUrl: (source: string, path: string, inline = false) =>
    `${src(source)}/download${query({ path, inline: inline ? "1" : undefined })}`,
  /** Which file kinds the server can make thumbnails and previews for. */
  previewCapabilities: () => request<PreviewCapabilities>("GET", "/api/previews"),
  /**
   * URL of a preview image. `version` (e.g. the modification time) only
   * busts the browser cache when the file changes; the server ignores it.
   */
  previewUrl: (source: string, path: string, variant: "thumb" | "large" | "pdf" = "thumb", version?: string) =>
    `${src(source)}/preview${query({ path, variant, v: version })}`,
  /** URL that streams a zip of the given paths. */
  zipUrl: (source: string, paths: string[]) => `${src(source)}/zip${query({ path: paths })}`,

  /** Starts a chunked upload of `size` bytes to `path`. */
  uploadStart: (source: string, path: string, size: number, overwrite = false) =>
    request<UploadSession>("POST", `${src(source)}/uploads`, { path, size, overwrite }),
  /** Returns how many bytes of an upload the server has stored. */
  uploadStatus: (id: string) =>
    request<UploadSession>("GET", `/api/uploads/${encodeURIComponent(id)}`),
  /** Sends one chunk starting at `offset`. */
  uploadChunk: (id: string, offset: number, chunk: Blob, signal?: AbortSignal) =>
    request<UploadSession>("PATCH", `/api/uploads/${encodeURIComponent(id)}`, chunk, {
      signal,
      headers: { "Upload-Offset": String(offset), "Content-Type": "application/octet-stream" },
    }),
  /** Cancels an upload and discards its data. */
  uploadCancel: (id: string) => request<void>("DELETE", `/api/uploads/${encodeURIComponent(id)}`),

  /** Lists the user's share links. */
  shares: () => request<Share[]>("GET", "/api/shares"),
  /** Creates a share link. */
  createShare: (input: ShareInput) => request<Share>("POST", "/api/shares", input),
  /** Changes a share link; `password: null` removes the password. */
  updateShare: (id: string, input: Partial<ShareInput>) =>
    request<Share>("PATCH", `/api/shares/${encodeURIComponent(id)}`, input),
  /** Revokes a share link. */
  deleteShare: (id: string) => request<void>("DELETE", `/api/shares/${encodeURIComponent(id)}`),

  /** Opens a public share; `path` lists a subfolder of a folder share. */
  publicShare: (token: string, path?: string) =>
    request<PublicShare>("GET", `/api/public/shares/${encodeURIComponent(token)}${query({ path })}`),
  /** Unlocks a password-protected share (sets a cookie). */
  unlockShare: (token: string, password: string) =>
    request<void>("POST", `/api/public/shares/${encodeURIComponent(token)}/unlock`, { password }),
  /** URL that downloads a share, or one path inside a folder share. */
  publicDownloadUrl: (token: string, path?: string) =>
    `/api/public/shares/${encodeURIComponent(token)}/download${query({ path })}`,
  /** Uploads one file to an upload-only (drop) share. */
  publicUpload: (token: string, name: string, file: Blob, signal?: AbortSignal) =>
    request<void>(
      "POST",
      `/api/public/shares/${encodeURIComponent(token)}/uploads${query({ path: name })}`,
      file,
      { signal, headers: { "Content-Type": "application/octet-stream" } },
    ),
};
