/**
 * Client for the editor endpoints, `GET` and `PUT /api/sources/{id}/text`.
 *
 * These calls do not go through `request()` in `../api` because the editor
 * needs the raw text body on save and the current ETag from a `412` reply;
 * they still use the shared {@link headersFor} so the CSRF token is sent.
 */
import { ApiError, headersFor, query } from "../api";

/** A text file opened for editing. */
export interface TextFile {
  path: string;
  content: string;
  /** Strong ETag to send back with the next save. */
  etag: string;
  size: number;
  modified: string | null;
}

/** The result of a successful save. */
export interface Saved {
  path: string;
  etag: string;
  size: number;
  modified: string | null;
}

/**
 * Thrown when a save is refused because the file changed (or appeared, or
 * vanished) since it was opened.
 */
export class SaveConflict extends Error {
  /** ETag of the file now on the server, or null if it no longer exists. */
  readonly currentEtag: string | null;

  constructor(message: string, currentEtag: string | null) {
    super(message);
    this.name = "SaveConflict";
    this.currentEtag = currentEtag;
  }
}

const url = (source: string, path: string) =>
  `/api/sources/${encodeURIComponent(source)}/text${query({ path })}`;

async function fail(res: Response): Promise<never> {
  let body: { error?: string; message?: string; etag?: string | null } = {};
  try {
    body = (await res.json()) as typeof body;
  } catch {
    /* not JSON */
  }
  if (res.status === 412) {
    throw new SaveConflict(body.message ?? "The file changed since you opened it", body.etag ?? null);
  }
  throw new ApiError(res.status, body.message ?? body.error ?? (res.statusText || `HTTP ${res.status}`), body.error);
}

async function send(input: string, init: RequestInit): Promise<Response> {
  try {
    return await fetch(input, { ...init, credentials: "same-origin" });
  } catch (e) {
    if (e instanceof DOMException && e.name === "AbortError") throw e;
    throw new ApiError(0, "Cannot reach the server");
  }
}

/** Loads a text file. Throws {@link ApiError} (`not_text`, `payload_too_large`, ...). */
export async function loadText(source: string, path: string, signal?: AbortSignal): Promise<TextFile> {
  const res = await send(url(source, path), { method: "GET", headers: headersFor("GET"), signal });
  if (!res.ok) return fail(res);
  return (await res.json()) as TextFile;
}

/**
 * Saves `content`.
 *
 * @param etag the ETag the content was based on, or `null` to create a new
 *   file (fails if one already exists).
 * @throws {SaveConflict} if the file changed on the server meanwhile.
 */
export async function saveText(source: string, path: string, content: string, etag: string | null): Promise<Saved> {
  const headers = headersFor("PUT", { "Content-Type": "text/plain; charset=utf-8" });
  if (etag === null) headers.set("If-None-Match", "*");
  else headers.set("If-Match", etag);
  const res = await send(url(source, path), { method: "PUT", headers, body: content });
  if (!res.ok) return fail(res);
  return (await res.json()) as Saved;
}
