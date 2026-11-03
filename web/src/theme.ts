/**
 * Theme handling: light, dark, system or custom.
 *
 * The choice is saved per browser (so it applies before sign-in) and, once
 * signed in, on the server (`/api/me/theme`) so it follows the user between
 * devices. It is applied as a `data-theme` attribute on `<html>`, which the
 * stylesheet keys off; `custom` colours and branding overrides are injected
 * by `features/theme/branding.ts`.
 */
import { useCallback, useSyncExternalStore } from "react";

/** A theme preference. */
export type Theme = "light" | "dark" | "system" | "custom";

const KEY = "filebrary.theme";
const THEMES: readonly Theme[] = ["light", "dark", "system", "custom"];

/** Whether a value is a theme name. */
export const isTheme = (v: unknown): v is Theme => typeof v === "string" && (THEMES as readonly string[]).includes(v);

/** Reads the saved preference, defaulting to following the system. */
export function loadTheme(): Theme {
  try {
    const v = localStorage.getItem(KEY);
    if (isTheme(v)) return v;
  } catch {
    /* storage blocked */
  }
  return "system";
}

/** Applies a preference to the document. */
export function applyTheme(theme: Theme): void {
  const root = document.documentElement;
  if (theme === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", theme);
}

let current: Theme | null = null;
const listeners = new Set<() => void>();
const listeners2 = new Set<(t: Theme) => void>();

/** The current theme. */
export function getTheme(): Theme {
  current ??= loadTheme();
  return current;
}

/**
 * Sets, applies and saves the theme. `fromServer` skips notifying
 * {@link onThemeChange} listeners (which save to the server).
 */
export function setTheme(theme: Theme, fromServer = false): void {
  current = theme;
  applyTheme(theme);
  try {
    localStorage.setItem(KEY, theme);
  } catch {
    /* ignore */
  }
  listeners.forEach((fn) => fn());
  if (!fromServer) listeners2.forEach((fn) => fn(theme));
}

/** Calls `fn` whenever the user changes the theme. Returns an unsubscribe function. */
export function onThemeChange(fn: (t: Theme) => void): () => void {
  listeners2.add(fn);
  return () => listeners2.delete(fn);
}

const subscribe = (fn: () => void) => {
  listeners.add(fn);
  return () => listeners.delete(fn);
};

/** The theme after `t` when cycling with the header button. */
export function nextTheme(t: Theme, hasCustom: boolean): Theme {
  if (t === "system") return "light";
  if (t === "light") return "dark";
  if (t === "dark") return hasCustom ? "custom" : "system";
  return "system";
}

/**
 * React hook returning the current theme and a function that cycles it
 * (through `custom` only when `hasCustom`).
 */
export function useTheme(hasCustom = false): [Theme, () => void] {
  const theme = useSyncExternalStore(subscribe, getTheme);
  const cycle = useCallback(() => setTheme(nextTheme(getTheme(), hasCustom)), [hasCustom]);
  return [theme, cycle];
}
