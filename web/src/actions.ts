/**
 * Registry of file actions shown in the browser toolbar (comments, "add to
 * collection", ...), so features can add buttons without editing the
 * browser. Feature modules call {@link registerAction} once at startup
 * (import them from `features/index.ts`).
 */
import type { ComponentType } from "react";
import type { Entry, Source } from "./api";

/** What an action sees: the folder and the current selection. */
export interface ActionContext {
  source: Source;
  /** The folder being shown. */
  path: string;
  /** Selected entries (may be empty). */
  selected: Entry[];
}

/** Props of the dialog an action opens. */
export interface ActionDialogProps extends ActionContext {
  onClose: () => void;
  /** Reloads the listing. */
  onChanged: () => void;
}

/** A toolbar action. */
export interface FileAction {
  /** Unique id, e.g. `"comments"`. */
  id: string;
  /** Button label. */
  label: string;
  /** Higher priorities come first; default 0. */
  priority?: number;
  /** Whether the button is enabled for this context. */
  enabled: (ctx: ActionContext) => boolean;
  /** Dialog shown when the button is pressed (lazy-load it if large). */
  dialog: ComponentType<ActionDialogProps>;
}

const actions: FileAction[] = [];

/** Adds an action. Registering the same id twice replaces the first one. */
export function registerAction(a: FileAction): void {
  const i = actions.findIndex((x) => x.id === a.id);
  if (i >= 0) actions.splice(i, 1);
  actions.push(a);
  actions.sort((x, y) => (y.priority ?? 0) - (x.priority ?? 0));
}

/** All registered actions, highest priority first. */
export function fileActions(): readonly FileAction[] {
  return actions;
}
