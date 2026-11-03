/** Client for `/api/comments`. */
import { query, request } from "../../api";

/** A comment on a file or folder. */
export interface Comment {
  id: number;
  user_id: number | null;
  username: string | null;
  /** Plain text: always render as text, never as HTML. */
  body: string;
  /** Unix seconds. */
  created_at: number;
  edited_at: number | null;
  can_edit: boolean;
  can_delete: boolean;
}

export const commentsApi = {
  /** The thread on one file, oldest first. */
  list: (source: string, path: string) => request<Comment[]>("GET", `/api/comments${query({ source, path })}`),
  /** Adds a comment. */
  add: (source: string, path: string, body: string) => request<Comment>("POST", "/api/comments", { source, path, body }),
  /** Edits one's own comment. */
  edit: (id: number, body: string) => request<void>("PATCH", `/api/comments/${id}`, { body }),
  /** Deletes a comment. */
  remove: (id: number) => request<void>("DELETE", `/api/comments/${id}`),
  /** Comment counts for the children of `dir`, by name. */
  counts: (source: string, dir: string) =>
    request<Record<string, number>>("GET", `/api/comments/counts${query({ source, dir })}`),
};
