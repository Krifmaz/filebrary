/**
 * Modal dialog built on the native `<dialog>` element, so focus trapping,
 * the inert background and Escape-to-close come from the browser. It has a
 * header with the title and a close button, a scrolling body and a footer
 * for actions. On phones it becomes a bottom sheet.
 */
import { useEffect, useId, useRef, type ReactNode } from "react";
import { Icon } from "./Icon";

/** Props for {@link Dialog}. */
export interface DialogProps {
  title: string;
  onClose: () => void;
  children: ReactNode;
  /** Short text under the title. */
  description?: ReactNode;
  /** Buttons shown at the bottom, right-aligned. */
  actions?: ReactNode;
  /** Called when the form inside the dialog is submitted (Enter key). */
  onSubmit?: () => void;
  /** A wider dialog for pickers and editors. */
  wide?: boolean;
}

/** A modal dialog with a title, body and action row. */
export function Dialog({ title, onClose, children, description, actions, onSubmit, wide }: DialogProps) {
  const ref = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const descId = useId();
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
      onClick={(e) => {
        // A click on the backdrop lands on the <dialog> itself.
        if (e.target === e.currentTarget) onClose();
      }}
      aria-labelledby={titleId}
      aria-describedby={description ? descId : undefined}
    >
      <form
        method="dialog"
        className="dialog-form"
        onSubmit={(e) => {
          e.preventDefault();
          onSubmit?.();
        }}
      >
        <header className="dialog-head">
          <div className="dialog-titles">
            <h2 id={titleId}>{title}</h2>
            {description && <p className="dialog-desc" id={descId}>{description}</p>}
          </div>
          <button type="button" className="btn ghost icon-only" aria-label="Close" title="Close" onClick={onClose}>
            <Icon name="x" />
          </button>
        </header>
        <div className="dialog-body">{children}</div>
        {actions && <footer className="dialog-actions">{actions}</footer>}
      </form>
    </dialog>
  );
}
