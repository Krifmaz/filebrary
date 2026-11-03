/**
 * Comment thread on one file or folder: read, post, edit and delete.
 * Bodies are rendered as plain text with line breaks preserved.
 */
import { useCallback, useEffect, useState } from "react";
import type { ActionDialogProps } from "../../actions";
import { Dialog } from "../../components/Dialog";
import { commentsApi, type Comment } from "./api";

const MAX_CHARS = 10_000;

/** Formats Unix seconds for display. */
export function formatWhen(secs: number): string {
  return new Date(secs * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

/** The dialog opened by the "Comments" toolbar action. */
export default function CommentsDialog({ source, path, selected, onClose }: ActionDialogProps) {
  const target = selected[0]?.path ?? path;
  const name = selected[0]?.name ?? (path === "/" ? source.name : path.split("/").pop() ?? path);
  const [comments, setComments] = useState<Comment[] | null>(null);
  const [draft, setDraft] = useState("");
  const [editing, setEditing] = useState<{ id: number; body: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(() => {
    commentsApi
      .list(source.id, target)
      .then(setComments)
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
  }, [source.id, target]);
  useEffect(load, [load]);

  const act = async (fn: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await fn();
      load();
      return true;
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      return false;
    } finally {
      setBusy(false);
    }
  };

  const post = async () => {
    if (!draft.trim()) return;
    if (await act(() => commentsApi.add(source.id, target, draft))) setDraft("");
  };

  return (
    <Dialog title={`Comments on ${name}`} onClose={onClose} wide actions={<button type="button" onClick={onClose}>Close</button>}>
      <ul className="comments" aria-live="polite">
        {comments === null && !error && <li className="muted">Loading…</li>}
        {comments?.length === 0 && <li className="muted">No comments yet.</li>}
        {comments?.map((c) => (
          <li key={c.id} className="comment">
            <div className="comment-head">
              <strong>{c.username ?? "deleted user"}</strong>
              <span className="muted">
                {formatWhen(c.created_at)}
                {c.edited_at ? " (edited)" : ""}
              </span>
              <span className="grow" />
              {c.can_edit && editing?.id !== c.id && (
                <button type="button" className="link" onClick={() => setEditing({ id: c.id, body: c.body })}>Edit</button>
              )}
              {c.can_delete && (
                <button type="button" className="link danger" disabled={busy} onClick={() => void act(() => commentsApi.remove(c.id))}>
                  Delete
                </button>
              )}
            </div>
            {editing?.id === c.id ? (
              <div className="comment-edit">
                <textarea
                  aria-label="Edit comment"
                  value={editing.body}
                  maxLength={MAX_CHARS}
                  onChange={(e) => setEditing({ id: c.id, body: e.target.value })}
                />
                <div className="row">
                  <button type="button" onClick={() => setEditing(null)}>Cancel</button>
                  <button
                    type="button"
                    className="primary"
                    disabled={busy || !editing.body.trim()}
                    onClick={async () => {
                      if (await act(() => commentsApi.edit(c.id, editing.body))) setEditing(null);
                    }}
                  >
                    Save
                  </button>
                </div>
              </div>
            ) : (
              <p className="comment-body">{c.body}</p>
            )}
          </li>
        ))}
      </ul>
      {source.permissions.download && (
        <div className="comment-new">
          <textarea
            aria-label="New comment"
            placeholder="Write a comment… (Ctrl+Enter to post)"
            value={draft}
            maxLength={MAX_CHARS}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
                e.preventDefault();
                void post();
              }
            }}
          />
          <button type="button" className="primary" disabled={busy || !draft.trim()} onClick={() => void post()}>
            Post
          </button>
        </div>
      )}
      {error && <p className="error" role="alert">{error}</p>}
    </Dialog>
  );
}
