/**
 * A tiny client-side router built on the History API.
 *
 * The app has only a handful of routes, so this avoids a router dependency.
 * The server serves `index.html` for every non-API path, so deep links work.
 */
import { useSyncExternalStore } from "react";

/** A parsed location. */
export type Route =
  | { name: "login"; next: string }
  | { name: "home" }
  | { name: "files"; source: string; path: string }
  | { name: "shares" }
  | { name: "collections" }
  | { name: "appearance" }
  | { name: "public"; token: string; path: string }
  | { name: "notFound" };

const listeners = new Set<() => void>();
const notify = () => listeners.forEach((fn) => fn());
if (typeof window !== "undefined") window.addEventListener("popstate", notify);

/** Navigates to `url`, adding a history entry unless `replace` is set. */
export function navigate(url: string, replace = false): void {
  if (url === location.pathname + location.search) return;
  if (replace) history.replaceState(null, "", url);
  else history.pushState(null, "", url);
  notify();
}

/** Encodes a path inside a source as URL path segments. */
export function encodePath(path: string): string {
  return path
    .split("/")
    .filter(Boolean)
    .map(encodeURIComponent)
    .join("/");
}

/** URL of a folder in the file browser. */
export const filesUrl = (source: string, path = "/") =>
  `/files/${encodeURIComponent(source)}/${encodePath(path)}`;

/** Parses a pathname and search string into a {@link Route}. */
export function parse(pathname: string, search = ""): Route {
  const segs = pathname.split("/").filter(Boolean).map((s) => {
    try {
      return decodeURIComponent(s);
    } catch {
      return s;
    }
  });
  const q = new URLSearchParams(search);
  if (segs.length === 0) return { name: "home" };
  if (segs[0] === "login") return { name: "login", next: q.get("next") ?? "/" };
  if (segs[0] === "shares" && segs.length === 1) return { name: "shares" };
  if (segs[0] === "collections" && segs.length === 1) return { name: "collections" };
  if (segs[0] === "settings" && segs[1] === "appearance" && segs.length === 2) return { name: "appearance" };
  if (segs[0] === "files" && segs[1]) {
    return { name: "files", source: segs[1], path: "/" + segs.slice(2).join("/") };
  }
  if (segs[0] === "s" && segs[1]) {
    return { name: "public", token: segs[1], path: "/" + segs.slice(2).join("/") };
  }
  return { name: "notFound" };
}

const subscribe = (fn: () => void) => {
  listeners.add(fn);
  return () => listeners.delete(fn);
};
const snapshot = () => location.pathname + location.search;

/** Returns the current route and re-renders when it changes. */
export function useRoute(): Route {
  const href = useSyncExternalStore(subscribe, snapshot);
  const i = href.indexOf("?");
  return parse(i < 0 ? href : href.slice(0, i), i < 0 ? "" : href.slice(i));
}
