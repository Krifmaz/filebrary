/**
 * The file browser: breadcrumbs, a sortable listing with multi-select, a
 * toolbar for file operations, and a drop zone for uploads.
 *
 * Keyboard: Enter opens, Delete deletes, F2 renames, Ctrl/Cmd+A selects all,
 * Escape clears the selection, Backspace goes to the parent folder, arrow
 * keys move the selection, `1`–`5` pick a view mode and `V` / `Shift+V`
 * cycle through them.
 *
 * View modes (details, compact list, small/medium/large icons) are
 * remembered per user and per folder; see `features/view/viewMode.ts`.
 *
 * The parent mounts one instance per folder (keyed by source and path), so
 * selection and filter start fresh in every folder.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, type Entry, type Source } from "../api";
import { basename, crumbs, formatDate, formatSize, iconFor, join, parent, validateName } from "../paths";
import { filesUrl, navigate } from "../router";
import { filesFromDrop, filesFromInput, type UploadManager } from "../upload";
import { Dialog } from "./Dialog";
import { FolderPicker } from "./FolderPicker";
import { ShareDialog } from "./ShareDialog";
import { Suspense } from "react";
import { viewerFor, type Viewer } from "../viewers";
import { fileActions, type FileAction } from "../actions";
import { FileGrid } from "../features/view/FileGrid";
import { usePreviewCapabilities } from "../features/view/previews";
import { ViewSwitcher } from "../features/view/ViewSwitcher";
import { isGrid, loadViewMode, modeForKey, nextMode, saveViewMode, type ViewMode } from "../features/view/viewMode";

/** Props for {@link Browser}. */
export interface BrowserProps {
  source: Source;
  path: string;
  uploads: UploadManager;
  /** Bumped by the parent when uploads finish, to trigger a reload. */
  refreshKey: number;
  /** Signed-in user's id; the view mode is remembered per user. */
  user?: number | string;
}

type SortKey = "name" | "size" | "modified";

type Modal =
  | { kind: "mkdir" }
  | { kind: "rename"; entry: Entry }
  | { kind: "move" | "copy"; paths: string[] }
  | { kind: "delete"; paths: string[] }
  | { kind: "share"; paths: string[]; singleFolder: boolean }
  | { kind: "view"; entry: Entry; viewer: Viewer }
  | { kind: "action"; action: FileAction }
  | null;

/** Sorts entries with folders first, then by the chosen key. */
export function sortEntries(entries: Entry[], key: SortKey, asc: boolean): Entry[] {
  const dir = asc ? 1 : -1;
  return [...entries].sort((a, b) => {
    if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
    let c = 0;
    if (key === "size") c = a.size - b.size;
    else if (key === "modified") c = Date.parse(a.modified) - Date.parse(b.modified);
    if (c === 0) c = a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" });
    return c * dir;
  });
}

/** File browser for one folder of one source. */
export function Browser({ source, path, uploads, refreshKey, user = "anonymous" }: BrowserProps) {
  const perms = source.permissions;
  const [entries, setEntries] = useState<Entry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [anchor, setAnchor] = useState<string | null>(null);
  const [sort, setSort] = useState<{ key: SortKey; asc: boolean }>({ key: "name", asc: true });
  const [filter, setFilter] = useState("");
  const [modal, setModal] = useState<Modal>(null);
  const [dragging, setDragging] = useState(false);
  const [reload, setReload] = useState(0);
  const [toast, setToast] = useState<string | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const folderInput = useRef<HTMLInputElement>(null);
  const dragDepth = useRef(0);
  const listRef = useRef<HTMLDivElement>(null);
  const [mode, setModeState] = useState<ViewMode>(() => loadViewMode(user, source.id, path));
  const previews = usePreviewCapabilities(mode !== "details");

  const setMode = (m: ViewMode) => {
    setModeState(m);
    saveViewMode(user, source.id, path, m);
  };

  const refresh = useCallback(() => setReload((n) => n + 1), []);

  useEffect(() => {
    const ctl = new AbortController();
    api
      .list(source.id, path, ctl.signal)
      .then((l) => {
        setEntries(l.entries);
        setError(null);
      })
      .catch((e: unknown) => {
        if (ctl.signal.aborted) return;
        setEntries([]);
        setError(e instanceof Error ? e.message : String(e));
      });
    return () => ctl.abort();
  }, [source.id, path, reload, refreshKey]);

  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), 4000);
    return () => clearTimeout(t);
  }, [toast]);

  const visible = useMemo(() => {
    const f = filter.trim().toLowerCase();
    const list = entries ?? [];
    return sortEntries(f ? list.filter((e) => e.name.toLowerCase().includes(f)) : list, sort.key, sort.asc);
  }, [entries, filter, sort]);

  const selectedEntries = visible.filter((e) => selected.has(e.path));
  const selectedPaths = selectedEntries.map((e) => e.path);

  /** Runs an operation, reporting failures in a toast and refreshing after. */
  const run = async (label: string, fn: () => Promise<unknown>) => {
    try {
      await fn();
      setToast(label);
    } catch (e) {
      setToast(e instanceof Error ? e.message : String(e));
    } finally {
      refresh();
    }
  };

  const open = (e: Entry) => {
    if (e.is_dir) return navigate(filesUrl(source.id, e.path));
    const viewer = viewerFor(e, source);
    if (viewer) setModal({ kind: "view", entry: e, viewer });
    else if (perms.download) window.open(api.downloadUrl(source.id, e.path, true), "_blank", "noopener");
  };

  const download = () => {
    if (!perms.download || selectedEntries.length === 0) return;
    const only = selectedEntries[0];
    const url =
      selectedEntries.length === 1 && only && !only.is_dir
        ? api.downloadUrl(source.id, only.path)
        : api.zipUrl(source.id, selectedPaths);
    const a = document.createElement("a");
    a.href = url;
    a.download = "";
    document.body.appendChild(a);
    a.click();
    a.remove();
  };

  const clickRow = (e: React.MouseEvent, entry: Entry) => {
    const next = new Set(e.ctrlKey || e.metaKey || e.shiftKey ? selected : []);
    if (e.shiftKey && anchor) {
      const a = visible.findIndex((x) => x.path === anchor);
      const b = visible.findIndex((x) => x.path === entry.path);
      const [lo, hi] = a < b ? [a, b] : [b, a];
      for (const x of visible.slice(lo, hi + 1)) next.add(x.path);
    } else if ((e.ctrlKey || e.metaKey) && next.has(entry.path)) {
      next.delete(entry.path);
    } else {
      next.add(entry.path);
      setAnchor(entry.path);
    }
    setSelected(next);
  };

  const toggle = (entry: Entry) => {
    const next = new Set(selected);
    if (next.has(entry.path)) next.delete(entry.path);
    else next.add(entry.path);
    setSelected(next);
    setAnchor(entry.path);
  };

  /** Items per row in the current layout, measured from the DOM. */
  const columns = (): number => {
    if (mode === "details") return 1;
    const items = listRef.current?.querySelectorAll<HTMLElement>("[data-path]") ?? [];
    const top = items[0]?.offsetTop;
    let n = 0;
    for (const el of items) {
      if (el.offsetTop !== top) break;
      n++;
    }
    return Math.max(1, n);
  };

  /** Moves the selection by `delta` items and scrolls it into view. */
  const moveSelection = (delta: number) => {
    if (!visible.length) return;
    const at = anchor ? visible.findIndex((x) => x.path === anchor) : -1;
    const i = at < 0 ? (delta > 0 ? 0 : visible.length - 1) : Math.min(visible.length - 1, Math.max(0, at + delta));
    const target = visible[i];
    if (!target) return;
    setSelected(new Set([target.path]));
    setAnchor(target.path);
    const el = listRef.current?.querySelector<HTMLElement>(`[data-path="${CSS.escape(target.path)}"]`);
    el?.scrollIntoView?.({ block: "nearest" });
  };

  const onKey = (e: React.KeyboardEvent) => {
    if ((e.target as HTMLElement).closest("input, textarea, select, dialog")) return;
    const plain = !e.ctrlKey && !e.metaKey && !e.altKey;
    const arrows: Record<string, () => number> = {
      ArrowDown: columns,
      ArrowUp: () => -columns(),
      ArrowRight: () => (mode === "details" ? 0 : 1),
      ArrowLeft: () => (mode === "details" ? 0 : -1),
    };
    const byKey = plain && !e.shiftKey ? modeForKey(e.key) : undefined;
    if (byKey) setMode(byKey);
    else if (plain && (e.key === "v" || e.key === "V")) setMode(nextMode(mode, e.shiftKey ? -1 : 1));
    else if (plain && !e.shiftKey && arrows[e.key]) {
      const d = arrows[e.key]?.() ?? 0;
      if (d) {
        e.preventDefault();
        moveSelection(d);
      }
    } else if (e.key === "Escape") setSelected(new Set());
    else if (e.key === "a" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      setSelected(new Set(visible.map((x) => x.path)));
    } else if (e.key === "Delete" && perms.delete && selectedPaths.length) {
      setModal({ kind: "delete", paths: selectedPaths });
    } else if (e.key === "F2" && perms.edit && selectedEntries.length === 1 && selectedEntries[0]) {
      e.preventDefault();
      setModal({ kind: "rename", entry: selectedEntries[0] });
    } else if (e.key === "Enter" && selectedEntries.length === 1 && selectedEntries[0]) {
      open(selectedEntries[0]);
    } else if (e.key === "Backspace" && path !== "/") {
      navigate(filesUrl(source.id, parent(path)));
    }
  };

  const onDrop = async (e: React.DragEvent) => {
    e.preventDefault();
    dragDepth.current = 0;
    setDragging(false);
    if (!perms.upload) return;
    const files = await filesFromDrop(e.dataTransfer);
    if (files.length) uploads.add(source.id, path, files);
  };

  const hasFiles = (e: React.DragEvent) => Array.from(e.dataTransfer.types).includes("Files");

  const header = (key: SortKey, label: string) => (
    <th aria-sort={sort.key === key ? (sort.asc ? "ascending" : "descending") : "none"}>
      <button className="link" onClick={() => setSort({ key, asc: sort.key === key ? !sort.asc : true })}>
        {label} {sort.key === key ? (sort.asc ? "▲" : "▼") : ""}
      </button>
    </th>
  );

  const one = selectedEntries.length === 1 ? selectedEntries[0] : undefined;

  return (
    <section
      className={`browser${dragging ? " dragging" : ""}`}
      tabIndex={-1}
      onKeyDown={onKey}
      onDragEnter={(e) => {
        if (!hasFiles(e) || !perms.upload) return;
        dragDepth.current++;
        setDragging(true);
      }}
      onDragOver={(e) => {
        if (hasFiles(e) && perms.upload) e.preventDefault();
      }}
      onDragLeave={() => {
        if (--dragDepth.current <= 0) {
          dragDepth.current = 0;
          setDragging(false);
        }
      }}
      onDrop={onDrop}
    >
      <nav className="crumbs" aria-label="Breadcrumb">
        <span className="source-name">{source.name}</span>
        {crumbs(path).map((c, i, all) => (
          <span key={c.path}>
            {i > 0 && <span className="sep">/</span>}
            {i === all.length - 1 ? (
              <strong>{i === 0 ? "" : c.name}</strong>
            ) : (
              <a
                href={filesUrl(source.id, c.path)}
                onClick={(e) => {
                  e.preventDefault();
                  navigate(filesUrl(source.id, c.path));
                }}
              >
                {i === 0 ? "" : c.name}
              </a>
            )}
          </span>
        ))}
      </nav>

      <div className="toolbar" role="toolbar" aria-label="File actions">
        {perms.upload && (
          <>
            <button onClick={() => setModal({ kind: "mkdir" })}>New folder</button>
            <button onClick={() => fileInput.current?.click()}>Upload files</button>
            <button onClick={() => folderInput.current?.click()}>Upload folder</button>
          </>
        )}
        <span className="toolbar-sep" />
        <button disabled={!perms.download || !selectedEntries.length} onClick={download}>
          {selectedEntries.length > 1 || one?.is_dir ? "Download zip" : "Download"}
        </button>
        <button disabled={!perms.edit || !one} onClick={() => one && setModal({ kind: "rename", entry: one })}>Rename</button>
        <button disabled={!perms.edit || !selectedEntries.length} onClick={() => setModal({ kind: "move", paths: selectedPaths })}>Move</button>
        <button disabled={!perms.upload || !selectedEntries.length} onClick={() => setModal({ kind: "copy", paths: selectedPaths })}>Copy</button>
        <button
          disabled={!perms.share || (!selectedEntries.length && path === "/")}
          onClick={() => {
            const paths = selectedEntries.length ? selectedPaths : [path];
            const singleFolder = selectedEntries.length ? !!one?.is_dir : true;
            setModal({ kind: "share", paths, singleFolder });
          }}
        >
          Share
        </button>
        <button className="danger" disabled={!perms.delete || !selectedEntries.length} onClick={() => setModal({ kind: "delete", paths: selectedPaths })}>
          Delete
        </button>
        {fileActions().map((a) => (
          <button key={a.id} disabled={!a.enabled({ source, path, selected: selectedEntries })} onClick={() => setModal({ kind: "action", action: a })}>
            {a.label}
          </button>
        ))}
        <span className="view-controls">
          {mode !== "details" && (
            <span className="sort-control">
              <select aria-label="Sort by" value={sort.key} onChange={(e) => setSort({ key: e.target.value as SortKey, asc: sort.asc })}>
                <option value="name">Name</option>
                <option value="size">Size</option>
                <option value="modified">Modified</option>
              </select>
              <button type="button" className="icon" aria-label={sort.asc ? "Ascending" : "Descending"} title="Reverse order" onClick={() => setSort({ key: sort.key, asc: !sort.asc })}>
                {sort.asc ? "▲" : "▼"}
              </button>
            </span>
          )}
          <ViewSwitcher mode={mode} onChange={setMode} />
        </span>
        <input className="filter" type="search" placeholder="Filter" value={filter} onChange={(e) => setFilter(e.target.value)} aria-label="Filter this folder" />
      </div>

      <input
        ref={fileInput}
        type="file"
        multiple
        hidden
        onChange={(e) => {
          if (e.target.files) uploads.add(source.id, path, filesFromInput(e.target.files));
          e.target.value = "";
        }}
      />
      <input
        ref={folderInput}
        type="file"
        hidden
        // @ts-expect-error: non-standard but supported by every current browser
        webkitdirectory=""
        onChange={(e) => {
          if (e.target.files) uploads.add(source.id, path, filesFromInput(e.target.files));
          e.target.value = "";
        }}
      />

      {error && <p className="error" role="alert">{error}</p>}

      {mode !== "details" && (
        <div className="grid-wrap" ref={listRef}>
          <FileGrid
            mode={mode}
            source={source.id}
            entries={visible}
            selected={selected}
            previews={isGrid(mode) ? previews : undefined}
            onClickItem={clickRow}
            onToggle={toggle}
            onOpen={open}
            leading={
              path !== "/" && (
                <li className="up" role="option" aria-selected={false} onDoubleClick={() => navigate(filesUrl(source.id, parent(path)))}>
                  <a
                    href={filesUrl(source.id, parent(path))}
                    onClick={(e) => {
                      e.preventDefault();
                      navigate(filesUrl(source.id, parent(path)));
                    }}
                  >
                    {isGrid(mode) ? <span className="thumb"><span className="thumb-icon">⬆</span></span> : <span className="ico">⬆</span>}
                    <span className="tile-name">..</span>
                  </a>
                </li>
              )
            }
            placeholder={
              entries === null ? (
                <li className="muted placeholder">Loading…</li>
              ) : visible.length === 0 && !error ? (
                <li className="muted placeholder empty">
                  {filter ? "Nothing matches the filter." : perms.upload ? "This folder is empty. Drop files here to upload." : "This folder is empty."}
                </li>
              ) : null
            }
          />
        </div>
      )}

      {mode === "details" && (
        <div className="table-wrap" ref={listRef}>
          <table className="files">
            <thead>
              <tr>
                <th className="check">
                  <input
                    type="checkbox"
                    aria-label="Select all"
                    checked={visible.length > 0 && selectedEntries.length === visible.length}
                    onChange={(e) => setSelected(e.target.checked ? new Set(visible.map((x) => x.path)) : new Set())}
                  />
                </th>
                {header("name", "Name")}
                {header("size", "Size")}
                {header("modified", "Modified")}
              </tr>
            </thead>
            <tbody>
              {path !== "/" && (
                <tr className="up" onDoubleClick={() => navigate(filesUrl(source.id, parent(path)))}>
                  <td />
                  <td colSpan={3}>
                    <a
                      href={filesUrl(source.id, parent(path))}
                      onClick={(e) => {
                        e.preventDefault();
                        navigate(filesUrl(source.id, parent(path)));
                      }}
                    >
                      ⬆ ..
                    </a>
                  </td>
                </tr>
              )}
              {entries === null && (
                <tr>
                  <td />
                  <td colSpan={3} className="muted">Loading…</td>
                </tr>
              )}
              {entries !== null && visible.length === 0 && !error && (
                <tr>
                  <td />
                  <td colSpan={3} className="muted empty">
                    {filter ? "Nothing matches the filter." : perms.upload ? "This folder is empty. Drop files here to upload." : "This folder is empty."}
                  </td>
                </tr>
              )}
              {visible.map((e) => (
                <tr
                  key={e.path}
                  data-path={e.path}
                  className={selected.has(e.path) ? "selected" : undefined}
                  onClick={(ev) => clickRow(ev, e)}
                  onDoubleClick={() => open(e)}
                  aria-selected={selected.has(e.path)}
                >
                  <td className="check" onClick={(ev) => ev.stopPropagation()}>
                    <input type="checkbox" checked={selected.has(e.path)} onChange={() => toggle(e)} aria-label={`Select ${e.name}`} />
                  </td>
                  <td className="name">
                    <span className="ico" aria-hidden>{iconFor(e.name, e.is_dir)}</span>
                    {e.is_dir ? (
                      <a
                        href={filesUrl(source.id, e.path)}
                        onClick={(ev) => {
                          ev.preventDefault();
                          ev.stopPropagation();
                          navigate(filesUrl(source.id, e.path));
                        }}
                      >
                        {e.name}
                      </a>
                    ) : (
                      <span>{e.name}</span>
                    )}
                  </td>
                  <td className="size">{e.is_dir ? "" : formatSize(e.size)}</td>
                  <td className="date">{formatDate(e.modified)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {dragging && <div className="drop-overlay">Drop files or folders to upload to {path}</div>}
      {toast && <div className="toast" role="status">{toast}</div>}

      {modal?.kind === "mkdir" && (
        <NameDialog
          title="New folder"
          initial=""
          confirm="Create"
          onClose={() => setModal(null)}
          onSave={(name) => {
            setModal(null);
            void run(`Created ${name}`, () => api.mkdir(source.id, join(path, name)));
          }}
        />
      )}
      {modal?.kind === "rename" && (
        <NameDialog
          title={`Rename ${modal.entry.name}`}
          initial={modal.entry.name}
          confirm="Rename"
          onClose={() => setModal(null)}
          onSave={(name) => {
            const from = modal.entry.path;
            setModal(null);
            if (name !== modal.entry.name) void run(`Renamed to ${name}`, () => api.move(source.id, from, join(parent(from), name)));
          }}
        />
      )}
      {(modal?.kind === "move" || modal?.kind === "copy") && (
        <FolderPicker
          title={`${modal.kind === "move" ? "Move" : "Copy"} ${modal.paths.length} item${modal.paths.length === 1 ? "" : "s"} to`}
          source={source.id}
          start={path}
          confirmLabel={modal.kind === "move" ? "Move here" : "Copy here"}
          onClose={() => setModal(null)}
          onPick={(dir) => {
            const { kind, paths } = modal;
            setModal(null);
            const op = kind === "move" ? api.move : api.copy;
            void run(`${kind === "move" ? "Moved" : "Copied"} ${paths.length} item${paths.length === 1 ? "" : "s"}`, async () => {
              for (const p of paths) {
                const to = join(dir, basename(p));
                if (to !== p) await op(source.id, p, to);
              }
            });
          }}
        />
      )}
      {modal?.kind === "delete" && (
        <Dialog
          title="Delete"
          onClose={() => setModal(null)}
          onSubmit={() => {
            const { paths } = modal;
            setModal(null);
            setSelected(new Set());
            void run(`Deleted ${paths.length} item${paths.length === 1 ? "" : "s"}`, () => api.remove(source.id, paths));
          }}
          actions={
            <>
              <button type="button" onClick={() => setModal(null)}>Cancel</button>
              <button className="danger" type="submit" autoFocus>Delete</button>
            </>
          }
        >
          <p>
            Delete {modal.paths.length === 1 ? <strong>{basename(modal.paths[0] ?? "")}</strong> : `${modal.paths.length} items`}?
          </p>
        </Dialog>
      )}
      {modal?.kind === "view" && (
        <div className="viewer" role="dialog" aria-label={modal.entry.name}>
          <Suspense fallback={<p className="muted center">Loading…</p>}>
            <modal.viewer.component source={source} entry={modal.entry} onClose={() => setModal(null)} onChanged={refresh} />
          </Suspense>
        </div>
      )}
      {modal?.kind === "action" && (
        <Suspense fallback={null}>
          <modal.action.dialog source={source} path={path} selected={selectedEntries} onClose={() => setModal(null)} onChanged={refresh} />
        </Suspense>
      )}
      {modal?.kind === "share" && (
        <ShareDialog source={source.id} paths={modal.paths} singleFolder={modal.singleFolder} onClose={() => setModal(null)} />
      )}
    </section>
  );
}

/** Small dialog asking for a single file or folder name. */
function NameDialog(props: { title: string; initial: string; confirm: string; onSave: (name: string) => void; onClose: () => void }) {
  const [name, setName] = useState(props.initial);
  const error = name === props.initial && props.initial === "" ? null : validateName(name);
  return (
    <Dialog
      title={props.title}
      onClose={props.onClose}
      onSubmit={() => {
        if (!validateName(name)) props.onSave(name.trim());
      }}
      actions={
        <>
          <button type="button" onClick={props.onClose}>Cancel</button>
          <button className="primary" type="submit" disabled={!!validateName(name)}>{props.confirm}</button>
        </>
      }
    >
      <label>
        Name
        <input
          autoFocus
          value={name}
          onChange={(e) => setName(e.target.value)}
          onFocus={(e) => {
            // Select the name without its extension, like desktop file managers.
            const dot = e.target.value.lastIndexOf(".");
            e.target.setSelectionRange(0, dot > 0 ? dot : e.target.value.length);
          }}
        />
      </label>
      {error && <p className="error">{error}</p>}
    </Dialog>
  );
}
