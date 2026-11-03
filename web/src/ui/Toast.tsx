/**
 * Toast notifications. Call {@link toast} from anywhere; the single
 * {@link Toaster} mounted by the app shows them in a polite live region,
 * stacked bottom-right (bottom-centre on phones), and removes them after a
 * few seconds. Errors stay longer and can be dismissed.
 */
import { useSyncExternalStore } from "react";
import { Icon, type IconName } from "./Icon";

/** Kind of toast, which picks its icon and stripe colour. */
export type ToastKind = "info" | "success" | "error";

/** One toast on screen. */
export interface ToastItem {
  id: number;
  kind: ToastKind;
  message: string;
  /** Optional action button, e.g. "Undo". */
  action?: { label: string; run: () => void };
}

let items: ToastItem[] = [];
let nextId = 1;
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((fn) => fn());

/** Removes a toast. */
export function dismissToast(id: number): void {
  items = items.filter((t) => t.id !== id);
  emit();
}

function push(kind: ToastKind, message: string, action?: ToastItem["action"]): number {
  const id = nextId++;
  items = [...items.slice(-3), { id, kind, message, action }];
  emit();
  const ms = kind === "error" ? 8000 : action ? 6000 : 4000;
  setTimeout(() => dismissToast(id), ms);
  return id;
}

/** Shows a toast. `toast("Saved")` is an info toast. */
export const toast = Object.assign((message: string, action?: ToastItem["action"]) => push("info", message, action), {
  success: (message: string, action?: ToastItem["action"]) => push("success", message, action),
  error: (message: string) => push("error", message),
});

const subscribe = (fn: () => void) => {
  listeners.add(fn);
  return () => listeners.delete(fn);
};
const snapshot = () => items;

const ICON: Record<ToastKind, IconName> = { info: "info", success: "checkCircle", error: "alert" };

/** Renders the toasts. Mount once, near the root. */
export function Toaster() {
  const list = useSyncExternalStore(subscribe, snapshot);
  return (
    <div className="toaster" aria-live="polite" aria-relevant="additions">
      {list.map((t) => (
        <div key={t.id} className={`toast ${t.kind}`} role={t.kind === "error" ? "alert" : "status"}>
          <Icon name={ICON[t.kind]} />
          <span className="toast-text">{t.message}</span>
          {t.action && (
            <button
              type="button"
              className="btn ghost sm"
              onClick={() => {
                t.action?.run();
                dismissToast(t.id);
              }}
            >
              {t.action.label}
            </button>
          )}
          <button type="button" className="toast-close" aria-label="Dismiss" onClick={() => dismissToast(t.id)}>
            <Icon name="x" />
          </button>
        </div>
      ))}
    </div>
  );
}
