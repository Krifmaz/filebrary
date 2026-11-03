/**
 * The compact list and the small/medium/large tile grids. The details view
 * stays a table in `Browser`; this component renders every other mode with
 * the same selection and open behaviour.
 *
 * Tiles show a server thumbnail (`variant=thumb`) for files the preview
 * service supports and fall back to the file-type icon when the server has
 * previews switched off, the kind is unsupported, or the request fails.
 */
import { useState } from "react";
import { api, type Entry, type PreviewCapabilities } from "../../api";
import "./view.css";
import { formatDate, formatSize, iconFor } from "../../paths";
import { isGrid, previewKind, type ViewMode } from "./viewMode";

/** Props for {@link FileGrid}. */
export interface FileGridProps {
  mode: Exclude<ViewMode, "details">;
  source: string;
  entries: Entry[];
  selected: Set<string>;
  previews: PreviewCapabilities | undefined;
  /** Extra first item, e.g. the "up" link. */
  leading?: React.ReactNode;
  /** Shown instead of the items when there are none. */
  placeholder?: React.ReactNode;
  onClickItem: (e: React.MouseEvent, entry: Entry) => void;
  onToggle: (entry: Entry) => void;
  onOpen: (entry: Entry) => void;
}

/** Whether to ask the server for a thumbnail of this entry. */
export function wantsThumbnail(entry: Entry, previews: PreviewCapabilities | undefined): boolean {
  const kind = previewKind(entry);
  return !!(kind && previews?.enabled && previews.kinds?.[kind]);
}

/** Compact list or icon grid of entries. */
export function FileGrid(props: FileGridProps) {
  const { mode, entries, selected } = props;
  const grid = isGrid(mode);
  return (
    <ul className={grid ? `file-grid ${mode}` : "file-compact"} role="listbox" aria-multiselectable aria-label="Files">
      {props.leading}
      {props.placeholder}
      {entries.map((e) => (
        <li
          key={e.path}
          role="option"
          data-path={e.path}
          aria-selected={selected.has(e.path)}
          className={selected.has(e.path) ? "selected" : undefined}
          title={tooltip(e)}
          onClick={(ev) => props.onClickItem(ev, e)}
          onDoubleClick={() => props.onOpen(e)}
        >
          <input
            type="checkbox"
            className="tile-check"
            checked={selected.has(e.path)}
            onClick={(ev) => ev.stopPropagation()}
            onChange={() => props.onToggle(e)}
            aria-label={`Select ${e.name}`}
          />
          {grid ? (
            <Thumb source={props.source} entry={e} show={wantsThumbnail(e, props.previews)} />
          ) : (
            <span className="ico" aria-hidden>{iconFor(e.name, e.is_dir)}</span>
          )}
          <span className="tile-name">{e.name}</span>
          {mode === "large" && <span className="tile-meta muted">{e.is_dir ? formatDate(e.modified) : formatSize(e.size)}</span>}
        </li>
      ))}
    </ul>
  );
}

/** Thumbnail image with an icon fallback. */
function Thumb({ source, entry, show }: { source: string; entry: Entry; show: boolean }) {
  const [failed, setFailed] = useState(false);
  return (
    <span className="thumb" aria-hidden>
      {show && !failed ? (
        <img src={api.previewUrl(source, entry.path, "thumb", entry.modified)} alt="" loading="lazy" decoding="async" draggable={false} onError={() => setFailed(true)} />
      ) : (
        <span className="thumb-icon">{iconFor(entry.name, entry.is_dir)}</span>
      )}
    </span>
  );
}

function tooltip(e: Entry): string {
  return e.is_dir ? `${e.name}\n${formatDate(e.modified)}` : `${e.name}\n${formatSize(e.size)} · ${formatDate(e.modified)}`;
}
