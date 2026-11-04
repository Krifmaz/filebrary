/**
 * The file browser for one folder of one source.
 *
 * Layout: a header with breadcrumbs and the folder's main actions (upload,
 * new folder); a toolbar with the folder filter, sort and view switcher,
 * which turns into a selection bar while items are selected; then the
 * listing as a details table, a compact list or a grid of tiles. On phones
 * the actions move to a bar at the bottom of the screen.
 *
 * Selection: click selects one item, Ctrl/Cmd+click toggles, Shift+click
 * selects a range, checkboxes toggle. On touch screens a tap opens unless a
 * selection is in progress, then it toggles. Right-click (or long-press)
 * opens a context menu for the item, or for the folder on empty space.
 *
 * Keyboard: Enter opens, Delete deletes, F2 renames, Ctrl/Cmd+A selects all,
 * Escape clears the selection, Backspace goes to the parent folder, arrow
 * keys move the selection, `1`–`5` pick a view mode and `V` / `Shift+V`
 * cycle through them, `S` stars the selection, Shift+F10 or the context
 * menu key opens the menu.
 *
 * View modes are remembered per user and per folder; see
 * `features/view/viewMode.ts`. The parent mounts one instance per folder
 * (keyed by source and path), so selection and filter start fresh.
 */
import { Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, type Entry, type Source } from "../api";
import { fileActions, type FileAction } from "../actions";
import { library, type Annotation } from "../library";
import { basename, crumbs, formatDate, formatSize, join, parent, validateName } from "../paths";
import { filesUrl, navigate } from "../router";
import { filesFromDrop, filesFromInput, type UploadManager } from "../upload";
import { viewerFor, type Viewer } from "../viewers";
import { FileGrid } from "../features/view/FileGrid";
import { usePreviewCapabilities } from "../features/view/previews";
import { ViewSwitcher } from "../features/view/ViewSwitcher";
import { isGrid, loadViewMode, modeForKey, nextMode, saveViewMode, type ViewMode } from "../features/view/viewMode";
import {
  Button,
  Dialog,
  EmptyState,
  ErrorState,
  Field,
  Icon,
  IconButton,
  Menu,
  MenuButton,
  SearchInput,
  SkeletonRows,
  SkeletonTiles,
  errorMessage,
  fileIcon,
  toast,
  type MenuAnchor,
  type MenuEntry,
} from "../ui";
import { FolderPicker } from "./FolderPicker";
import { ShareDialog } from "./ShareDialog";
import { sourceIcon } from "./Shell";

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

/** What the listing can be sorted by. */
export type SortKey = "name" | "size" | "modified";

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

/**
 * The selection after a click on `target`: plain click selects only it,
 * Ctrl/Cmd toggles it, Shift selects the range from `anchor`.
 */
export function nextSelection(
  order: string[],
  current: Set<string>,
  anchor: string | null,
  target: string,
  mods: { shift?: boolean; toggle?: boolean },
): { selected: Set<string>; anchor: string | null } {
  if (mods.shift && anchor && order.includes(anchor)) {
    const a = order.indexOf(anchor);
    const b = order.indexOf(target);
    const [lo, hi] = a < b ? [a, b] : [b, a];
    const next = new Set(mods.toggle ? current : []);
    for (const p of order.slice(lo, hi + 1)) next.add(p);
    return { selected: next, anchor };
  }
  if (mods.toggle) {
    const next = new Set(current);
    if (next.has(target)) next.delete(target);
    else next.add(target);
    return { selected: next, anchor: target };
  }
  return { selected: new Set([target]), anchor: target };
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** File browser for one folder of one source. */
export function Browser({ source, path, uploads, refreshKey, user = "anonymous" }: BrowserProps) {
  const perms = source.permissions;
  const [entries, setEntries] = useState<Entry[] | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [notes, setNotes] = useState<Record<string, Annotation>>({});
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [anchor, setAnchor] = useState<string | null>(null);
  const [sort, setSort] = useState<{ key: SortKey; asc: boolean }>({ key: "name", asc: true });
  const [filter, setFilter] = useState("");
  const [modal, setModal] = useState<Modal>(null);
  const [menu, setMenu] = useState<{ anchor: MenuAnchor; items: MenuEntry[] } | null>(null);
  const [dragging, setDragging] = useState(false);
  const [reload, setReload] = useState(0);
  const fileInput = useRef<HTMLInputElement>(null);
  const folderInput = useRef<HTMLInputElement>(null);
  const dragDepth = useRef(0);
  const listRef = useRef<HTMLDivElement>(null);
  const pointer = useRef<string>("mouse");
  const longPress = useRef<number | undefined>(undefined);
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
        setError(e);
      });
    library
      .annotations(source.id, path, ctl.signal)
      .then((a) => setNotes(a ?? {}))
      .catch(() => {
        /* stars and tags are extras; the listing works without them */
      });
    return () => ctl.abort();
  }, [source.id, path, reload, refreshKey]);

  const visible = useMemo(() => {
    const f = filter.trim().toLowerCase();
    const list = entries ?? [];
    return sortEntries(f ? list.filter((e) => e.name.toLowerCase().includes(f)) : list, sort.key, sort.asc);
  }, [entries, filter, sort]);

  const selectedEntries = visible.filter((e) => selected.has(e.path));
  const selectedPaths = selectedEntries.map((e) => e.path);
  const one = selectedEntries.length === 1 ? selectedEntries[0] : undefined;
  const starred = (e: Entry) => !!notes[e.name]?.starred;

  /** Runs an operation, reporting the outcome in a toast and refreshing after. */
  const run = async (label: string, fn: () => Promise<unknown>) => {
    try {
      await fn();
      toast.success(label);
    } catch (e) {
      toast.error(errorMessage(e));
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

  const download = (items = selectedEntries) => {
    if (!perms.download || items.length === 0) return;
    const only = items[0];
    const url =
      items.length === 1 && only && !only.is_dir ? api.downloadUrl(source.id, only.path) : api.zipUrl(source.id, items.map((e) => e.path));
    const a = document.createElement("a");
    a.href = url;
    a.download = "";
    document.body.appendChild(a);
    a.click();
    a.remove();
  };

  const toggleStar = async (items: Entry[]) => {
    const on = !items.every(starred);
    try {
      for (const e of items) await (on ? library.star(source.id, e.path) : library.unstar(source.id, e.path));
      setNotes((n) => {
        const next = { ...n };
        for (const e of items) next[e.name] = { tags: n[e.name]?.tags ?? [], starred: on };
        return next;
      });
      toast.success(on ? `Starred ${items.length === 1 ? items[0]?.name : plural(items.length, "item")}` : `Removed ${plural(items.length, "star")}`);
    } catch (e) {
      toast.error(errorMessage(e));
    }
  };

  const share = (items = selectedEntries) => {
    const paths = items.length ? items.map((e) => e.path) : [path];
    const singleFolder = items.length ? items.length === 1 && !!items[0]?.is_dir : true;
    setModal({ kind: "share", paths, singleFolder });
  };

  const select = (s: Set<string>, a: string | null) => {
    setSelected(s);
    setAnchor(a);
  };

  const clickItem = (e: React.MouseEvent, entry: Entry) => {
    const touch = pointer.current === "touch";
    if (touch && selected.size === 0) return open(entry);
    const r = nextSelection(
      visible.map((x) => x.path),
      selected,
      anchor,
      entry.path,
      { shift: e.shiftKey, toggle: e.ctrlKey || e.metaKey || touch },
    );
    select(r.selected, r.anchor);
  };

  const toggle = (entry: Entry) => {
    const r = nextSelection(visible.map((x) => x.path), selected, anchor, entry.path, { toggle: true });
    select(r.selected, r.anchor);
  };

  /** Commands for a set of items (the context menu, the "More" button and the phone bar). */
  const itemMenu = (items: Entry[]): MenuEntry[] => {
    const single = items.length === 1 ? items[0] : undefined;
    const allStarred = items.length > 0 && items.every(starred);
    const extra: MenuEntry[] = fileActions().map((a) => ({
      label: a.label,
      icon: a.icon,
      disabled: !a.enabled({ source, path, selected: items }),
      onSelect: () => setModal({ kind: "action", action: a }),
    }));
    return [
      ...(single ? [{ label: "Open", icon: single.is_dir ? ("folder" as const) : ("eye" as const), shortcut: "Enter", onSelect: () => open(single) }] : []),
      { label: items.length > 1 || single?.is_dir ? "Download as zip" : "Download", icon: "download", disabled: !perms.download, onSelect: () => download(items) },
      { label: "Share…", icon: "share", disabled: !perms.share, onSelect: () => share(items) },
      { label: allStarred ? "Remove star" : "Star", icon: "star", shortcut: "S", onSelect: () => void toggleStar(items) },
      { separator: true },
      { label: "Rename…", icon: "edit", shortcut: "F2", disabled: !perms.edit || !single, onSelect: () => single && setModal({ kind: "rename", entry: single }) },
      { label: "Move to…", icon: "move", disabled: !perms.edit, onSelect: () => setModal({ kind: "move", paths: items.map((e) => e.path) }) },
      { label: "Copy to…", icon: "copy", disabled: !perms.upload, onSelect: () => setModal({ kind: "copy", paths: items.map((e) => e.path) }) },
      ...(extra.length ? [{ separator: true } as const, ...extra] : []),
      { separator: true },
      { label: "Delete…", icon: "trash", shortcut: "Del", danger: true, disabled: !perms.delete, onSelect: () => setModal({ kind: "delete", paths: items.map((e) => e.path) }) },
    ];
  };

  const folderMenu = (): MenuEntry[] => [
    { label: "Upload files", icon: "upload", disabled: !perms.upload, onSelect: () => fileInput.current?.click() },
    { label: "Upload folder", icon: "folder", disabled: !perms.upload, onSelect: () => folderInput.current?.click() },
    { label: "New folder…", icon: "folderPlus", disabled: !perms.upload, onSelect: () => setModal({ kind: "mkdir" }) },
    { separator: true },
    { label: "Share this folder…", icon: "share", disabled: !perms.share || path === "/", onSelect: () => share([]) },
    { label: "Select all", icon: "check", shortcut: "Ctrl A", disabled: visible.length === 0, onSelect: () => select(new Set(visible.map((x) => x.path)), null) },
    { label: "Refresh", icon: "refresh", onSelect: refresh },
  ];

  const onItemContext = (ev: React.MouseEvent, entry: Entry) => {
    ev.preventDefault();
    ev.stopPropagation();
    let items = selectedEntries;
    if (!selected.has(entry.path)) {
      select(new Set([entry.path]), entry.path);
      items = [entry];
    }
    setMenu({ anchor: { x: ev.clientX, y: ev.clientY }, items: itemMenu(items) });
  };

  const onListContext = (ev: React.MouseEvent) => {
    if ((ev.target as HTMLElement).closest("[data-path]")) return;
    ev.preventDefault();
    select(new Set(), null);
    setMenu({ anchor: { x: ev.clientX, y: ev.clientY }, items: folderMenu() });
  };

  // Long-press on touch opens the item menu.
  const onPointerDown = (ev: React.PointerEvent) => {
    pointer.current = ev.pointerType;
    if (ev.pointerType !== "touch") return;
    const target = (ev.target as HTMLElement).closest<HTMLElement>("[data-path]");
    const p = target?.dataset.path;
    if (!p) return;
    const { clientX: x, clientY: y } = ev;
    window.clearTimeout(longPress.current);
    longPress.current = window.setTimeout(() => {
      const entry = visible.find((v) => v.path === p);
      if (!entry) return;
      const items = selected.has(p) ? selectedEntries : [entry];
      if (!selected.has(p)) select(new Set([p]), p);
      setMenu({ anchor: { x, y }, items: itemMenu(items) });
    }, 500);
  };
  const cancelLongPress = () => window.clearTimeout(longPress.current);

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
  const moveSelection = (delta: number, extend: boolean) => {
    if (!visible.length) return;
    const cursor = extend ? [...selected].pop() ?? anchor : anchor;
    const at = cursor ? visible.findIndex((x) => x.path === cursor) : -1;
    const i = at < 0 ? (delta > 0 ? 0 : visible.length - 1) : Math.min(visible.length - 1, Math.max(0, at + delta));
    const target = visible[i];
    if (!target) return;
    if (extend) {
      const next = new Set(selected);
      next.add(target.path);
      setSelected(next);
    } else select(new Set([target.path]), target.path);
    const el = listRef.current?.querySelector<HTMLElement>(`[data-path="${CSS.escape(target.path)}"]`);
    el?.scrollIntoView?.({ block: "nearest" });
  };

  const onKey = (e: React.KeyboardEvent) => {
    if ((e.target as HTMLElement).closest("input, textarea, select, dialog, .menu")) return;
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
    else if (plain && arrows[e.key]) {
      const d = arrows[e.key]?.() ?? 0;
      if (d) {
        e.preventDefault();
        moveSelection(d, e.shiftKey);
      }
    } else if (e.key === "Escape") select(new Set(), null);
    else if (e.key === "a" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      select(new Set(visible.map((x) => x.path)), null);
    } else if (e.key === "Delete" && perms.delete && selectedPaths.length) {
      setModal({ kind: "delete", paths: selectedPaths });
    } else if (e.key === "F2" && perms.edit && one) {
      e.preventDefault();
      setModal({ kind: "rename", entry: one });
    } else if (e.key === "Enter" && one) {
      open(one);
    } else if (plain && (e.key === "s" || e.key === "S") && selectedEntries.length) {
      void toggleStar(selectedEntries);
    } else if (e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey)) {
      e.preventDefault();
      const el = one && listRef.current?.querySelector<HTMLElement>(`[data-path="${CSS.escape(one.path)}"]`);
      const r = (el ?? listRef.current)?.getBoundingClientRect();
      const at = r ? { x: r.left + 24, y: r.top + Math.min(r.height, 40) } : { x: 100, y: 100 };
      setMenu({ anchor: at, items: selectedEntries.length ? itemMenu(selectedEntries) : folderMenu() });
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
    if (files.length) {
      uploads.add(source.id, path, files);
      toast(`Uploading ${plural(files.length, "file")}`);
    }
  };

  const hasFiles = (e: React.DragEvent) => Array.from(e.dataTransfer.types).includes("Files");

  const header = (key: SortKey, label: string, className?: string) => (
    <th className={className} aria-sort={sort.key === key ? (sort.asc ? "ascending" : "descending") : "none"}>
      <button type="button" className="th-sort" onClick={() => setSort({ key, asc: sort.key === key ? !sort.asc : true })}>
        {label}
        {sort.key === key && <Icon name={sort.asc ? "arrowUp" : "arrowDown"} />}
      </button>
    </th>
  );

  const goUp = () => navigate(filesUrl(source.id, parent(path)));
  const count = selectedEntries.length;
  const loading = entries === null;
  const empty = !loading && visible.length === 0 && !error;
  const trail = crumbs(path);

  const emptyState = filter ? (
    <EmptyState compact icon="search" title="No matches" actions={<Button onClick={() => setFilter("")}>Clear filter</Button>}>
      Nothing in this folder matches “{filter}”.
    </EmptyState>
  ) : (
    <EmptyState
      icon={perms.upload ? "upload" : "folder"}
      title="This folder is empty"
      actions={
        perms.upload && (
          <>
            <Button variant="primary" icon="upload" onClick={() => fileInput.current?.click()}>Upload files</Button>
            <Button icon="folderPlus" onClick={() => setModal({ kind: "mkdir" })}>New folder</Button>
          </>
        )
      }
    >
      {perms.upload ? "Drop files or folders anywhere on this page, or use the buttons below." : "There is nothing here yet."}
    </EmptyState>
  );

  return (
    <section
      className={`browser${dragging ? " dragging" : ""}${count ? " has-selection" : ""}`}
      tabIndex={-1}
      aria-label={`${source.name} ${path}`}
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
      <header className="browser-head">
        <div className="browser-titles">
          <nav className="crumbs" aria-label="Breadcrumb">
            <ol>
              {trail.map((c, i) => {
                const last = i === trail.length - 1;
                const label = i === 0 ? source.name : c.name;
                return (
                  <li key={c.path}>
                    {i > 0 && <Icon name="chevronRight" className="crumb-sep" />}
                    {last ? (
                      <span className="crumb current" aria-current="page">
                        {i === 0 && <Icon name={sourceIcon(source)} />}
                        {label}
                      </span>
                    ) : (
                      <a
                        className="crumb"
                        href={filesUrl(source.id, c.path)}
                        onClick={(e) => {
                          e.preventDefault();
                          navigate(filesUrl(source.id, c.path));
                        }}
                      >
                        {i === 0 && <Icon name={sourceIcon(source)} />}
                        {label}
                      </a>
                    )}
                  </li>
                );
              })}
            </ol>
          </nav>
          <p className="browser-sub">
            {loading ? "Loading…" : `${plural(entries?.filter((e) => e.is_dir).length ?? 0, "folder")}, ${plural(entries?.filter((e) => !e.is_dir).length ?? 0, "file")}`}
          </p>
        </div>
        <div className="browser-actions hide-phone">
          {path !== "/" && <IconButton icon="arrowUp" label="Up one folder (Backspace)" onClick={goUp} variant="secondary" />}
          {perms.share && path !== "/" && <Button icon="share" onClick={() => share([])}>Share</Button>}
          {perms.upload && <Button icon="folderPlus" onClick={() => setModal({ kind: "mkdir" })}>New folder</Button>}
          {perms.upload && (
            <MenuButton
              align="end"
              label="Upload"
              items={[
                { label: "Upload files", icon: "upload", onSelect: () => fileInput.current?.click() },
                { label: "Upload a folder", icon: "folder", onSelect: () => folderInput.current?.click() },
              ]}
            >
              {(p) => (
                <Button variant="primary" icon="upload" iconAfter="chevronDown" {...p}>
                  Upload
                </Button>
              )}
            </MenuButton>
          )}
        </div>
      </header>

      <div className="toolbar" role="toolbar" aria-label={count ? "Selection actions" : "View options"}>
        {count > 0 ? (
          <div className="selection-bar">
            <IconButton icon="x" label="Clear selection (Esc)" onClick={() => select(new Set(), null)} />
            <span className="selection-count" aria-live="polite">{plural(count, "item")} selected</span>
            <span className="selection-actions hide-phone">
              <Button size="sm" icon="download" disabled={!perms.download} onClick={() => download()}>
                {count > 1 || one?.is_dir ? "Download zip" : "Download"}
              </Button>
              <Button size="sm" icon="share" disabled={!perms.share} onClick={() => share()}>Share</Button>
              <Button size="sm" icon="move" disabled={!perms.edit} onClick={() => setModal({ kind: "move", paths: selectedPaths })}>Move</Button>
              <Button size="sm" icon="copy" disabled={!perms.upload} onClick={() => setModal({ kind: "copy", paths: selectedPaths })}>Copy</Button>
              <Button size="sm" icon="edit" disabled={!perms.edit || !one} onClick={() => one && setModal({ kind: "rename", entry: one })}>Rename</Button>
              <Button size="sm" variant="danger" icon="trash" disabled={!perms.delete} onClick={() => setModal({ kind: "delete", paths: selectedPaths })}>Delete</Button>
            </span>
            <MenuButton align="end" label="More actions" items={itemMenu(selectedEntries)}>
              {(p) => (
                <Button size="sm" variant="ghost" icon="more" aria-label="More actions" title="More actions" {...p}>
                  <span className="hide-phone">More</span>
                </Button>
              )}
            </MenuButton>
          </div>
        ) : (
          <SearchInput
            className="filter"
            icon="filter"
            placeholder="Filter this folder"
            aria-label="Filter this folder"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            onClear={() => setFilter("")}
          />
        )}
        <span className="view-controls">
          <span className="sort-control">
            <select aria-label="Sort by" value={sort.key} onChange={(e) => setSort({ key: e.target.value as SortKey, asc: sort.asc })}>
              <option value="name">Name</option>
              <option value="size">Size</option>
              <option value="modified">Modified</option>
            </select>
            <IconButton
              icon={sort.asc ? "arrowUp" : "arrowDown"}
              label={sort.asc ? "Ascending" : "Descending"}
              variant="secondary"
              onClick={() => setSort({ key: sort.key, asc: !sort.asc })}
            />
          </span>
          <ViewSwitcher mode={mode} onChange={setMode} />
        </span>
      </div>

      <input
        ref={fileInput}
        type="file"
        multiple
        hidden
        onChange={(e) => {
          if (e.target.files?.length) uploads.add(source.id, path, filesFromInput(e.target.files));
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
          if (e.target.files?.length) uploads.add(source.id, path, filesFromInput(e.target.files));
          e.target.value = "";
        }}
      />

      <div
        className="listing"
        ref={listRef}
        onContextMenu={onListContext}
        onPointerDown={onPointerDown}
        onPointerUp={cancelLongPress}
        onPointerMove={cancelLongPress}
        onPointerCancel={cancelLongPress}
        onClick={(e) => {
          if (e.target === e.currentTarget) select(new Set(), null);
        }}
      >
        {error ? (
          <ErrorState title="Could not open this folder" error={error} onRetry={refresh} />
        ) : mode === "details" ? (
          <div className="table-wrap">
            <table className="files">
              <thead>
                <tr>
                  <th className="check">
                    <input
                      type="checkbox"
                      aria-label="Select all"
                      checked={visible.length > 0 && count === visible.length}
                      ref={(el) => {
                        if (el) el.indeterminate = count > 0 && count < visible.length;
                      }}
                      onChange={(e) => select(e.target.checked ? new Set(visible.map((x) => x.path)) : new Set(), null)}
                    />
                  </th>
                  {header("name", "Name", "name")}
                  {header("size", "Size", "size")}
                  {header("modified", "Modified", "date")}
                  <th className="actions"><span className="sr-only">Actions</span></th>
                </tr>
              </thead>
              <tbody>
                {loading && (
                  <tr className="placeholder">
                    <td colSpan={5}><SkeletonRows rows={7} /></td>
                  </tr>
                )}
                {empty && (
                  <tr className="placeholder">
                    <td colSpan={5}>{emptyState}</td>
                  </tr>
                )}
                {visible.map((e) => {
                  const on = selected.has(e.path);
                  const note = notes[e.name];
                  return (
                    <tr
                      key={e.path}
                      data-path={e.path}
                      className={on ? "selected" : undefined}
                      onClick={(ev) => clickItem(ev, e)}
                      onDoubleClick={() => open(e)}
                      onContextMenu={(ev) => onItemContext(ev, e)}
                      aria-selected={on}
                    >
                      <td className="check" onClick={(ev) => ev.stopPropagation()}>
                        <input type="checkbox" checked={on} onChange={() => toggle(e)} aria-label={`Select ${e.name}`} />
                      </td>
                      <td className="name">
                        <span className="name-cell">
                          <span className={e.is_dir ? "file-icon dir" : "file-icon"}><Icon name={fileIcon(e.name, e.is_dir)} /></span>
                          {e.is_dir ? (
                            <a
                              className="name-text"
                              href={filesUrl(source.id, e.path)}
                              onClick={(ev) => {
                                if (ev.shiftKey || ev.ctrlKey || ev.metaKey) {
                                  ev.preventDefault();
                                  return;
                                }
                                ev.preventDefault();
                                ev.stopPropagation();
                                navigate(filesUrl(source.id, e.path));
                              }}
                            >
                              {e.name}
                            </a>
                          ) : (
                            <span className="name-text">{e.name}</span>
                          )}
                          {note?.starred && <Icon name="star" filled className="star-mark" label="Starred" />}
                          {note?.tags.map((t) => (
                            <span key={t.id} className="tag" style={t.color ? { ["--tag" as string]: t.color } : undefined}>{t.name}</span>
                          ))}
                        </span>
                        <span className="name-meta">{e.is_dir ? formatDate(e.modified) : `${formatSize(e.size)} · ${formatDate(e.modified)}`}</span>
                      </td>
                      <td className="size">{e.is_dir ? "—" : formatSize(e.size)}</td>
                      <td className="date">{formatDate(e.modified)}</td>
                      <td className="actions" onClick={(ev) => ev.stopPropagation()}>
                        <button
                          type="button"
                          className="btn ghost icon-only sm row-menu"
                          aria-label={`Actions for ${e.name}`}
                          title="Actions"
                          aria-haspopup="menu"
                          onClick={(ev) => {
                            const items = on && count > 1 ? selectedEntries : [e];
                            if (!on) select(new Set([e.path]), e.path);
                            setMenu({ anchor: { element: ev.currentTarget, align: "end" }, items: itemMenu(items) });
                          }}
                        >
                          <Icon name="moreVertical" />
                        </button>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        ) : (
          <div className="grid-wrap">
            {loading ? (
              mode === "compact" ? <SkeletonRows rows={6} /> : <SkeletonTiles count={mode === "large" ? 6 : 12} />
            ) : (
              <FileGrid
                mode={mode}
                source={source.id}
                entries={visible}
                selected={selected}
                previews={isGrid(mode) ? previews : undefined}
                starred={(e) => starred(e)}
                onClickItem={clickItem}
                onToggle={toggle}
                onOpen={open}
                onContextMenu={onItemContext}
                placeholder={empty ? <li className="placeholder">{emptyState}</li> : null}
              />
            )}
          </div>
        )}
      </div>

      {/* Phone: actions live in a bar at the bottom of the screen. */}
      <div className="bottom-bar" role="toolbar" aria-label={count ? "Selection actions" : "Folder actions"}>
        {count > 0 ? (
          <>
            <BarButton icon="download" label="Download" disabled={!perms.download} onClick={() => download()} />
            <BarButton icon="share" label="Share" disabled={!perms.share} onClick={() => share()} />
            <BarButton icon="move" label="Move" disabled={!perms.edit} onClick={() => setModal({ kind: "move", paths: selectedPaths })} />
            <BarButton icon="trash" label="Delete" danger disabled={!perms.delete} onClick={() => setModal({ kind: "delete", paths: selectedPaths })} />
            <MenuButton align="end" label="More actions" items={itemMenu(selectedEntries)}>
              {(p) => (
                <button type="button" className="bar-button" {...p}>
                  <Icon name="more" />
                  <span>More</span>
                </button>
              )}
            </MenuButton>
          </>
        ) : (
          <>
            {path !== "/" && <BarButton icon="arrowUp" label="Up" onClick={goUp} />}
            <BarButton icon="upload" label="Upload" disabled={!perms.upload} onClick={() => fileInput.current?.click()} />
            <BarButton icon="folderPlus" label="New folder" disabled={!perms.upload} onClick={() => setModal({ kind: "mkdir" })} />
            <BarButton icon="share" label="Share" disabled={!perms.share || path === "/"} onClick={() => share([])} />
            <MenuButton align="end" label="Folder actions" items={folderMenu()}>
              {(p) => (
                <button type="button" className="bar-button" {...p}>
                  <Icon name="more" />
                  <span>More</span>
                </button>
              )}
            </MenuButton>
          </>
        )}
      </div>

      {dragging && (
        <div className="drop-overlay" aria-hidden>
          <div className="drop-card">
            <span className="state-icon"><Icon name="upload" /></span>
            <strong>Drop to upload</strong>
            <span className="muted">to {source.name}{path === "/" ? "" : ` · ${path}`}</span>
          </div>
        </div>
      )}

      {menu && <Menu anchor={menu.anchor} items={menu.items} onClose={() => setMenu(null)} />}

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
          title={`Rename ${modal.entry.is_dir ? "folder" : "file"}`}
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
          title={`${modal.kind === "move" ? "Move" : "Copy"} ${plural(modal.paths.length, "item")} to`}
          source={source.id}
          sourceName={source.name}
          start={path}
          confirmLabel={modal.kind === "move" ? "Move here" : "Copy here"}
          onClose={() => setModal(null)}
          onPick={(dir) => {
            const { kind, paths } = modal;
            setModal(null);
            if (kind === "move") select(new Set(), null);
            const op = kind === "move" ? api.move : api.copy;
            void run(`${kind === "move" ? "Moved" : "Copied"} ${plural(paths.length, "item")}`, async () => {
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
          title={`Delete ${modal.paths.length === 1 ? `“${basename(modal.paths[0] ?? "")}”` : plural(modal.paths.length, "item")}?`}
          description="Deleted items go to the trash when it is turned on; otherwise they are removed for good."
          onClose={() => setModal(null)}
          onSubmit={() => {
            const { paths } = modal;
            setModal(null);
            select(new Set(), null);
            void run(`Deleted ${plural(paths.length, "item")}`, () => api.remove(source.id, paths));
          }}
          actions={
            <>
              <Button onClick={() => setModal(null)}>Cancel</Button>
              <Button variant="danger-solid" type="submit" icon="trash" autoFocus>Delete</Button>
            </>
          }
        >
          {modal.paths.length > 1 && (
            <ul className="delete-list">
              {modal.paths.slice(0, 8).map((p) => <li key={p}>{basename(p)}</li>)}
              {modal.paths.length > 8 && <li className="muted">and {modal.paths.length - 8} more</li>}
            </ul>
          )}
        </Dialog>
      )}
      {modal?.kind === "view" && (
        <div className="viewer" role="dialog" aria-label={modal.entry.name}>
          <Suspense fallback={<div className="center"><span className="spinner lg" /></div>}>
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

/** A button in the phone bottom bar: icon over a short label. */
function BarButton({ icon, label, onClick, disabled, danger }: { icon: Parameters<typeof Icon>[0]["name"]; label: string; onClick: () => void; disabled?: boolean; danger?: boolean }) {
  return (
    <button type="button" className={danger ? "bar-button danger" : "bar-button"} onClick={onClick} disabled={disabled}>
      <Icon name={icon} />
      <span>{label}</span>
    </button>
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
          <Button onClick={props.onClose}>Cancel</Button>
          <Button variant="primary" type="submit" disabled={!!validateName(name)}>{props.confirm}</Button>
        </>
      }
    >
      <Field label="Name" error={error}>
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
      </Field>
    </Dialog>
  );
}
