/** Floating panel listing uploads with progress, pause, resume and cancel. */
import { useState, useSyncExternalStore } from "react";
import { formatSize } from "../paths";
import type { UploadManager } from "../upload";

/** Props for {@link UploadPanel}. */
export interface UploadPanelProps {
  manager: UploadManager;
}

/** Shows the upload queue; hidden when the queue is empty. */
export function UploadPanel({ manager }: UploadPanelProps) {
  const items = useSyncExternalStore(manager.subscribe, manager.getSnapshot);
  const [collapsed, setCollapsed] = useState(false);
  if (items.length === 0) return null;
  const active = items.filter((i) => i.status === "uploading" || i.status === "queued").length;
  const total = items.reduce((n, i) => n + i.size, 0);
  const sent = items.reduce((n, i) => n + i.sent, 0);

  return (
    <aside className="uploads card" aria-label="Uploads">
      <header>
        <strong>{active ? `Uploading ${active} file${active === 1 ? "" : "s"}` : "Uploads"}</strong>
        <span className="muted">{total ? Math.floor((sent / total) * 100) : 100}%</span>
        <button className="icon" onClick={() => setCollapsed(!collapsed)} aria-label={collapsed ? "Expand" : "Collapse"}>
          {collapsed ? "▴" : "▾"}
        </button>
        <button className="icon" onClick={() => manager.clearFinished()} aria-label="Clear finished">✕</button>
      </header>
      {!collapsed && (
        <ul>
          {items.map((i) => {
            const pct = i.size ? (i.sent / i.size) * 100 : i.status === "done" ? 100 : 0;
            return (
              <li key={i.key} className={`upload ${i.status}`}>
                <div className="upload-name" title={i.path}>{i.name}</div>
                <progress max={100} value={pct} />
                <div className="upload-meta">
                  <span>
                    {i.status === "failed" ? i.error : i.status === "done" ? formatSize(i.size) : `${formatSize(i.sent)} / ${formatSize(i.size)}`}
                  </span>
                  {(i.status === "uploading" || i.status === "queued") && (
                    <button className="link" onClick={() => manager.pause(i.key)}>Pause</button>
                  )}
                  {(i.status === "paused" || i.status === "failed") && (
                    <button className="link" onClick={() => manager.resume(i.key)}>{i.status === "failed" ? "Retry" : "Resume"}</button>
                  )}
                  {i.status !== "done" && i.status !== "cancelled" && (
                    <button className="link" onClick={() => manager.cancel(i.key)}>Cancel</button>
                  )}
                  {i.status === "cancelled" && <span className="muted">Cancelled</span>}
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </aside>
  );
}
