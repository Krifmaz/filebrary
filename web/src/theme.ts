/**
 * Light/dark theme handling.
 *
 * The choice ("light", "dark" or "system") is saved per browser and applied
 * as a `data-theme` attribute on `<html>`, which the stylesheet keys off.
 */
import { useCallback, useEffect, useState } from "react";

/** A theme preference. */
export type Theme = "light" | "dark" | "system";

const KEY = "filebrary.theme";

/** Reads the saved preference, defaulting to following the system. */
export function loadTheme(): Theme {
  try {
    const v = localStorage.getItem(KEY);
    if (v === "light" || v === "dark" || v === "system") return v;
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

/** React hook returning the current theme and a function that cycles it. */
export function useTheme(): [Theme, () => void] {
  const [theme, setTheme] = useState<Theme>(loadTheme);
  useEffect(() => {
    applyTheme(theme);
    try {
      localStorage.setItem(KEY, theme);
    } catch {
      /* ignore */
    }
  }, [theme]);
  const cycle = useCallback(
    () => setTheme((t) => (t === "system" ? "light" : t === "light" ? "dark" : "system")),
    [],
  );
  return [theme, cycle];
}
