/**
 * Loads the server's preview capabilities once per page and shares them
 * between every listing.
 */
import { useEffect, useState } from "react";
import { api, type PreviewCapabilities } from "../../api";

let cached: Promise<PreviewCapabilities> | undefined;

/** Fetches (once) which kinds the server can thumbnail; failures mean none. */
export function loadPreviewCapabilities(): Promise<PreviewCapabilities> {
  cached ??= api.previewCapabilities().catch(() => ({ enabled: false }));
  return cached;
}

/** Forgets the cached capabilities (for tests and after signing in again). */
export function resetPreviewCapabilities(): void {
  cached = undefined;
}

/**
 * React hook returning the preview capabilities, or `undefined` until they
 * have loaded. Pass `enabled = false` to skip the request (details view
 * shows no thumbnails).
 */
export function usePreviewCapabilities(enabled: boolean): PreviewCapabilities | undefined {
  const [caps, setCaps] = useState<PreviewCapabilities>();
  useEffect(() => {
    if (!enabled) return;
    let live = true;
    void loadPreviewCapabilities().then((c) => {
      if (live) setCaps(c);
    });
    return () => {
      live = false;
    };
  }, [enabled]);
  return caps;
}
