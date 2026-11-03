/** Client for `POST /api/cast`. */
import { request } from "../../api";

/** A short-lived URL a cast device can fetch without the session cookie. */
export interface CastUrl {
  url: string;
  absolute_url: string | null;
  /** Unix seconds. */
  expires_at: number;
}

/** Asks the server for a cast URL for one file (needs download permission). */
export const castUrl = (source: string, path: string) => request<CastUrl>("POST", "/api/cast", { source, path });

/** Whether a MIME type is something a cast device can play. */
export const isCastable = (mime: string | null | undefined) => !!mime && /^(video|audio)\//.test(mime);
