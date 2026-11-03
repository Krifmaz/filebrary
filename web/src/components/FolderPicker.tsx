/**
 * Dialog for choosing a destination folder, used by move and copy.
 * Lets the user browse folders in the source or type a path directly.
 */
import { useEffect, useState } from "react";
import { api, type Entry } from "../api";
import { crumbs, join } from "../paths";
import { Dialog } from "./Dialog";

/** Props for {@link FolderPicker}. */
export interface FolderPickerProps {
  title: string;
  source: string;
  start: string;
  confirmLabel: string;
  onPick: (dir: string) => void;
  onClose: () => void;
}

/** Folder browser dialog that returns the chosen directory path. */
export function FolderPicker({ title, source, start, confirmLabel, onPick, onClose }: FolderPickerProps) {
  const [dir, setDir] = useState(start);
  const [typed, setTyped] = useState(start);
  const [dirs, setDirs] = useState<Entry[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const ctl = new AbortController();
    api
      .list(source, dir, ctl.signal)
      .then((l) => {
        setDirs(l.entries.filter((e) => e.is_dir).sort((a, b) => a.name.localeCompare(b.name)));
        setError(null);
      })
      .catch((e: unknown) => {
        if (!ctl.signal.aborted) {
          setDirs([]);
          setError(e instanceof Error ? e.message : String(e));
        }
      });
    return () => ctl.abort();
  }, [source, dir]);

  const go = (p: string) => {
    setDirs(null);
    setDir(p);
    setTyped(p);
  };

  return (
    <Dialog
      title={title}
      onClose={onClose}
      onSubmit={() => onPick(join(typed))}
      wide
      actions={
        <>
          <button type="button" onClick={onClose}>Cancel</button>
          <button className="primary" type="submit">{confirmLabel}</button>
        </>
      }
    >
      <nav className="crumbs small" aria-label="Folder path">
        {crumbs(dir).map((c) => (
          <button type="button" key={c.path} className="link" onClick={() => go(c.path)}>
            {c.name}
          </button>
        ))}
      </nav>
      <ul className="picker-list">
        {dirs === null && <li className="muted">Loading…</li>}
        {dirs?.length === 0 && !error && <li className="muted">No subfolders</li>}
        {error && <li className="error">{error}</li>}
        {dirs?.map((d) => (
          <li key={d.path}>
            <button type="button" className="link" onClick={() => go(d.path)}>
              📁 {d.name}
            </button>
          </li>
        ))}
      </ul>
      <label>
        Destination folder
        <input value={typed} onChange={(e) => setTyped(e.target.value)} />
      </label>
    </Dialog>
  );
}
