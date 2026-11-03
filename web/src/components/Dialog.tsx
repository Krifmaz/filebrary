/**
 * Modal dialog built on the native `<dialog>` element, so focus trapping and
 * Escape-to-close come from the browser.
 */
import { useEffect, useRef, type ReactNode } from "react";

/** Props for {@link Dialog}. */
export interface DialogProps {
  title: string;
  onClose: () => void;
  children: ReactNode;
  /** Buttons shown at the bottom, right-aligned. */
  actions?: ReactNode;
  /** Called when the form inside the dialog is submitted (Enter key). */
  onSubmit?: () => void;
  wide?: boolean;
}

/** A modal dialog with a title, body and action row. */
export function Dialog({ title, onClose, children, actions, onSubmit, wide }: DialogProps) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const d = ref.current;
    if (d && !d.open) d.showModal?.();
    return () => d?.close?.();
  }, []);
  return (
    <dialog
      ref={ref}
      className={wide ? "dialog wide" : "dialog"}
      onCancel={(e) => {
        e.preventDefault();
        onClose();
      }}
      aria-label={title}
    >
      <form
        method="dialog"
        onSubmit={(e) => {
          e.preventDefault();
          onSubmit?.();
        }}
      >
        <h2>{title}</h2>
        <div className="dialog-body">{children}</div>
        {actions && <div className="dialog-actions">{actions}</div>}
      </form>
    </dialog>
  );
}
