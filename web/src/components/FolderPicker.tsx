/**
 * Dialog for choosing a destination folder, used by move and copy.
 * Lets the user browse folders in the source or type a path directly.
 */
import { useEffect, useState } from "react";
import { api, type Entry } from "../api";
import { crumbs, join } from "../paths";
import { Button, Dialog, EmptyState, Field, Icon, SkeletonRows, errorMessage } from "../ui";

/** Props for {@link FolderPicker}. */
export interface FolderPickerProps {
  title: string;
  source: string;
  /** Display name of the source, shown at the start of the path. */
  sourceName?: string;
  start: string;
  confirmLabel: string;
  onPick: (dir: string) => void;
  onClose: () => void;
}

/** Folder browser dialog that returns the chosen directory path. */
export function FolderPicker({ title, source, sourceName, start, confirmLabel, onPick, onClose }: FolderPickerProps) {
  const [dir, setDir] = useState(start);
  const [typed, setTyped] = useState(start);
  const [listing, setListing] = useState<{ dir: string; dirs: Entry[]; error?: string } | null>(null);

  useEffect(() => {
    const ctl = new AbortController();
    api.list(source, dir, ctl.signal).then(
      (l) => setListing({ dir, dirs: l.entries.filter((e) => e.is_dir).sort((a, b) => a.name.localeCompare(b.name)) }),
      (e: unknown) => {
        if (!ctl.signal.aborted) setListing({ dir, dirs: [], error: errorMessage(e) });
      },
    );
    return () => ctl.abort();
  }, [source, dir]);

  const go = (p: string) => {
    setDir(p);
    setTyped(p);
  };
  const current = listing?.dir === dir ? listing : null;
  const trail = crumbs(dir);

  return (
    <Dialog
      title={title}
      onClose={onClose}
      onSubmit={() => onPick(join(typed))}
      wide
      actions={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" type="submit">{confirmLabel}</Button>
        </>
      }
    >
      <nav className="crumbs small" aria-label="Folder path">
        <ol>
          {trail.map((c, i) => (
            <li key={c.path}>
              {i > 0 && <Icon name="chevronRight" className="crumb-sep" />}
              <button type="button" className={i === trail.length - 1 ? "crumb current link" : "crumb link"} onClick={() => go(c.path)}>
                {i === 0 && <Icon name="hardDrive" />}
                {i === 0 ? (sourceName ?? source) : c.name}
              </button>
            </li>
          ))}
        </ol>
      </nav>
      <ul className="picker-list" aria-label="Subfolders">
        {!current ? (
          <li><SkeletonRows rows={3} /></li>
        ) : current.error ? (
          <li className="error" role="alert">{current.error}</li>
        ) : current.dirs.length === 0 ? (
          <li><EmptyState compact icon="folder" title="No subfolders">Move or copy into this folder, or type a new path below.</EmptyState></li>
        ) : (
          current.dirs.map((d) => (
            <li key={d.path}>
              <button type="button" onClick={() => go(d.path)}>
                <Icon name="folder" />
                {d.name}
              </button>
            </li>
          ))
        )}
      </ul>
      <div style={{ marginTop: "var(--space-4)" }}>
        <Field label="Destination folder">
          <input value={typed} onChange={(e) => setTyped(e.target.value)} />
        </Field>
      </div>
    </Dialog>
  );
}
