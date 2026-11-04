/**
 * Client for stars, listing annotations (stars and tags of a folder's
 * entries) and the indexed search. Kept apart from `api.ts` so features
 * that build on those endpoints don't touch the core client.
 */
import { request, type Entry } from "./api";

/** A starred file or folder (`GET /api/stars`). */
export interface Star {
  source: string;
  path: string;
  name: string;
  /** Unix seconds. */
  starred_at: number;
}

/** A tag shown next to a file. */
export interface TagRef {
  id: number;
  name: string;
  /** `#rrggbb`, if set. */
  color: string | null;
}

/** Star and tags of one entry in a listing. */
export interface Annotation {
  starred: boolean;
  tags: TagRef[];
}

/** One search result (`GET /api/search`). */
export interface SearchHit {
  source: string;
  path: string;
  name: string;
  /** Coarse type: `dir`, `image`, `video`, `audio`, `document`, `text`, `code`, `archive` or `other`. */
  type: string;
  is_dir: boolean;
  size: number;
  modified: string | null;
  mime: string | null;
  /** HTML-escaped excerpt with matches in `<b>`, for content searches. */
  snippet?: string;
}

/** The reply of `GET /api/search`. */
export interface SearchResults {
  hits: SearchHit[];
  has_more: boolean;
  /** A source is still being indexed, so results may be incomplete. */
  indexing: boolean;
}

/** Search filters; see the server's `SearchParams`. */
export interface SearchQuery {
  q?: string;
  source?: string;
  type?: string;
  sort?: "name" | "path" | "size" | "modified";
  order?: "asc" | "desc";
  limit?: number;
  offset?: number;
}

const qs = (o: Record<string, string | number | undefined>) => {
  const p = new URLSearchParams();
  for (const [k, v] of Object.entries(o)) if (v !== undefined && v !== "") p.set(k, String(v));
  const s = p.toString();
  return s ? `?${s}` : "";
};

export const library = {
  /** The current user's starred items, newest first. */
  stars: () => request<Star[]>("GET", "/api/stars"),
  /** Stars a file or folder. */
  star: (source: string, path: string) => request<void>("PUT", "/api/stars", { source, path }),
  /** Removes a star. */
  unstar: (source: string, path: string) => request<void>("DELETE", `/api/stars${qs({ source, path })}`),
  /** Stars and tags of the entries of one folder, keyed by entry name. */
  annotations: (source: string, path: string, signal?: AbortSignal) =>
    request<Record<string, Annotation>>("GET", `/api/annotations${qs({ source, path })}`, undefined, { signal }),
  /** Details of one path (used to tell starred folders from files). */
  stat: (source: string, path: string) => request<Entry>("GET", `/api/sources/${encodeURIComponent(source)}/stat${qs({ path })}`),
  /** Searches the index. */
  search: (q: SearchQuery, signal?: AbortSignal) =>
    request<SearchResults>("GET", `/api/search${qs({ ...q })}`, undefined, { signal }),
};
