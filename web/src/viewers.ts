/**
 * Registry of file viewers (editor, image/video previews, PDF, ...).
 *
 * Feature modules call {@link registerViewer} once at startup (import them
 * from `features/index.ts`). When the user opens a file, the browser asks
 * {@link viewerFor} for the first viewer that accepts it and shows it full
 * screen; with no match the file opens in a new tab. Viewers should be
 * `React.lazy` components so their code loads only when used.
 */
import type { ComponentType } from "react";
import type { Entry, Source } from "./api";

/** Props every viewer receives. */
export interface ViewerProps {
  source: Source;
  entry: Entry;
  /** Closes the viewer. */
  onClose: () => void;
  /** Call after the viewer changed the file, so the listing reloads. */
  onChanged: () => void;
}

/** A viewer registration. */
export interface Viewer {
  /** Unique id, e.g. `"editor"`. */
  id: string;
  /** Higher priorities are asked first; default 0. */
  priority?: number;
  /** Returns true if this viewer can show the entry. */
  accepts: (entry: Entry, source: Source) => boolean;
  component: ComponentType<ViewerProps>;
}

const viewers: Viewer[] = [];

/** Adds a viewer. Registering the same id twice replaces the first one. */
export function registerViewer(v: Viewer): void {
  const i = viewers.findIndex((x) => x.id === v.id);
  if (i >= 0) viewers.splice(i, 1);
  viewers.push(v);
  viewers.sort((a, b) => (b.priority ?? 0) - (a.priority ?? 0));
}

/** Returns the best viewer for an entry, if any. */
export function viewerFor(entry: Entry, source: Source): Viewer | undefined {
  return viewers.find((v) => v.accepts(entry, source));
}
