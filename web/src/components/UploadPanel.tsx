/**
 * Floating panel listing uploads with overall and per-file progress, and
 * pause, resume, retry and cancel. Hidden when the queue is empty.
 */
import { useState, useSyncExternalStore } from "react";
import { formatSize } from "../paths";
import type { UploadManager } from "../upload";
import { Icon, IconButton, Spinner, fileIcon } from "../ui";

/** Props for {@link UploadPanel}. */
export interface UploadPanelProps {
  manager: UploadManager;
}

/** Shows the upload queue. */
export function UploadPanel({ manager }: UploadPanelProps) {
  const items = useSyncExternalStore(manager.subscribe, manager.getSnapshot);
  const [collapsed, setCollapsed] = useState(false);
  if (items.length === 0) return null;
  const active = items.filter((i) => i.status === "uploading" || i.status === "queued").length;
  const failed = items.filter((i) => i.status === "failed").length;
  const total = items.reduce((n, i) => n + i.size, 0);
  const sent = items.reduce((n, i) => n + i.sent, 0);
  const pct = total ? Math.floor((sent / total) * 100) : 100;

  return (
    <aside className="uploads" aria-label="Uploads">
      <header className="uploads-head">
        {active ? <Spinner size="sm" /> : failed ? <Icon name="alert" className="error" /> : <Icon name="checkCircle" className="upload-done" />}
        <div className="uploads-title">
          <strong>
            {active ? `Uploading ${active} file${active === 1 ? "" : "s"}` : failed ? `${failed} upload${failed === 1 ? "" : "s"} failed` : "Uploads complete"}
          </strong>
          <span>
            {formatSize(sent)} of {formatSize(total)} · {pct}%
          </span>
        </div>
        <IconButton size="sm" icon={collapsed ? "chevronUp" : "chevronDown"} label={collapsed ? "Expand" : "Collapse"} onClick={() => setCollapsed(!collapsed)} />
        <IconButton size="sm" icon="x" label="Clear finished" onClick={() => manager.clearFinished()} />
      </header>
      <div className="uploads-total" role="progressbar" aria-label="Total progress" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct}>
        <i style={{ width: `${pct}%` }} />
      </div>
      {!collapsed && (
        <ul>
          {items.map((i) => {
            const p = i.size ? (i.sent / i.size) * 100 : i.status === "done" ? 100 : 0;
            return (
              <li key={i.key} className={`upload ${i.status}`}>
                <span className="file-icon"><Icon name={fileIcon(i.name, false)} /></span>
                <div className="upload-name" title={i.path}>{i.name}</div>
                <span className="upload-actions">
                  {(i.status === "uploading" || i.status === "queued") && (
                    <IconButton size="sm" icon="pause" label={`Pause ${i.name}`} onClick={() => manager.pause(i.key)} />
                  )}
                  {(i.status === "paused" || i.status === "failed") && (
                    <IconButton size="sm" icon={i.status === "failed" ? "refresh" : "play"} label={`${i.status === "failed" ? "Retry" : "Resume"} ${i.name}`} onClick={() => manager.resume(i.key)} />
                  )}
                  {i.status !== "done" && i.status !== "cancelled" && (
                    <IconButton size="sm" icon="x" label={`Cancel ${i.name}`} onClick={() => manager.cancel(i.key)} />
                  )}
                  {i.status === "done" && <Icon name="check" className="upload-done" label="Done" />}
                </span>
                <div className="upload-meta">
                  <span className="upload-bar" role="progressbar" aria-label={`${i.name} progress`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(p)}>
                    <i style={{ width: `${p}%` }} />
                  </span>
                  <span>
                    {i.status === "failed"
                      ? i.error
                      : i.status === "cancelled"
                        ? "Cancelled"
                        : i.status === "paused"
                          ? "Paused"
                          : i.status === "done"
                            ? formatSize(i.size)
                            : `${formatSize(i.sent)} / ${formatSize(i.size)}`}
                  </span>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </aside>
  );
}
