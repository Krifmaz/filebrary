/**
 * The compact list and the small/medium/large tile grids. The details view
 * stays a table in `Browser`; this component renders every other mode with
 * the same selection, open and context-menu behaviour.
 *
 * Tiles show a server thumbnail (`variant=thumb`) for files the preview
 * service supports and fall back to the file-type icon when the server has
 * previews switched off, the kind is unsupported, or the request fails.
 */
import { useState } from "react";
import { api, type Entry, type PreviewCapabilities } from "../../api";
import "./view.css";
import { formatDate, formatSize } from "../../paths";
import { Icon, fileIcon } from "../../ui/Icon";
import { isGrid, previewKind, type ViewMode } from "./viewMode";

/** Props for {@link FileGrid}. */
export interface FileGridProps {
  mode: Exclude<ViewMode, "details">;
  source: string;
  entries: Entry[];
  selected: Set<string>;
  previews: PreviewCapabilities | undefined;
  /** Whether an entry is starred (shows a star badge). */
  starred?: (entry: Entry) => boolean;
  /** Extra first item. */
  leading?: React.ReactNode;
  /** Shown instead of the items when there are none. */
  placeholder?: React.ReactNode;
  onClickItem: (e: React.MouseEvent, entry: Entry) => void;
  onToggle: (entry: Entry) => void;
  onOpen: (entry: Entry) => void;
  onContextMenu?: (e: React.MouseEvent, entry: Entry) => void;
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
      {entries.map((e) => {
        const on = selected.has(e.path);
        const star = props.starred?.(e);
        return (
          <li
            key={e.path}
            role="option"
            data-path={e.path}
            aria-selected={on}
            className={on ? "selected" : undefined}
            title={tooltip(e)}
            onClick={(ev) => props.onClickItem(ev, e)}
            onDoubleClick={() => props.onOpen(e)}
            onContextMenu={props.onContextMenu && ((ev) => props.onContextMenu?.(ev, e))}
          >
            <input
              type="checkbox"
              className="tile-check"
              checked={on}
              onClick={(ev) => ev.stopPropagation()}
              onChange={() => props.onToggle(e)}
              aria-label={`Select ${e.name}`}
            />
            {grid ? (
              <Thumb source={props.source} entry={e} show={wantsThumbnail(e, props.previews)} />
            ) : (
              <span className={e.is_dir ? "file-icon dir" : "file-icon"}><Icon name={fileIcon(e.name, e.is_dir)} /></span>
            )}
            <span className="tile-text">
              <span className="tile-name">{e.name}</span>
              {mode !== "small" && <span className="tile-meta">{e.is_dir ? "Folder" : formatSize(e.size)}</span>}
            </span>
            {star && <Icon name="star" filled className="star-mark" label="Starred" />}
          </li>
        );
      })}
    </ul>
  );
}

/** Thumbnail image with an icon fallback. */
function Thumb({ source, entry, show }: { source: string; entry: Entry; show: boolean }) {
  const [failed, setFailed] = useState(false);
  return (
    <span className={entry.is_dir ? "thumb dir" : "thumb"} aria-hidden>
      {show && !failed ? (
        <img src={api.previewUrl(source, entry.path, "thumb", entry.modified)} alt="" loading="lazy" decoding="async" draggable={false} onError={() => setFailed(true)} />
      ) : (
        <span className="thumb-icon"><Icon name={fileIcon(entry.name, entry.is_dir)} /></span>
      )}
    </span>
  );
}

function tooltip(e: Entry): string {
  return e.is_dir ? `${e.name}\n${formatDate(e.modified)}` : `${e.name}\n${formatSize(e.size)} · ${formatDate(e.modified)}`;
}
