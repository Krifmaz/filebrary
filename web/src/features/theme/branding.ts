/**
 * Branding and theme colours from the server.
 *
 * {@link loadBranding} fetches `/api/branding` once (it is public, so the
 * login page and share pages are branded too), and {@link brandingCss}
 * turns the palettes into CSS that overrides the variables in `styles.css`.
 * Colours are checked again here so only `#rgb`/`#rrggbb` reach the page.
 */
import { useSyncExternalStore } from "react";
import { request } from "../../api";
import type { Theme } from "../../theme";

/** CSS variable name → colour. */
export type Palette = Record<string, string>;

/** Variables a theme may set (must match the server's list). */
export const THEME_VARS = [
  "--bg",
  "--surface",
  "--surface-2",
  "--text",
  "--muted",
  "--border",
  "--accent",
  "--accent-contrast",
  "--accent-soft",
  "--danger",
  "--ok",
] as const;

/** Server-wide branding. */
export interface Branding {
  name: string;
  default_theme: Theme;
  light: Palette;
  dark: Palette;
  custom: Palette;
  logo_url: string | null;
}

/** A user's theme as stored on the server. */
export interface UserTheme {
  theme: Theme;
  colours: Palette;
}

export const DEFAULT_BRANDING: Branding = {
  name: "Filebrary",
  default_theme: "system",
  light: {},
  dark: {},
  custom: {},
  logo_url: null,
};

/** Checks a colour value. */
export const validColour = (c: string) => /^#(?:[0-9a-f]{3}|[0-9a-f]{6})$/i.test(c);

/** Renders `name: value;` pairs for the allowed, valid entries of a palette. */
export function declarations(p: Palette): string {
  return THEME_VARS.filter((k) => typeof p[k] === "string" && validColour(p[k]!))
    .map((k) => `${k}:${p[k]};`)
    .join("");
}

/** Relative luminance of a `#rgb`/`#rrggbb` colour (0 black … 1 white). */
export function luminance(hex: string): number {
  let h = hex.slice(1);
  if (h.length === 3) h = [...h].map((c) => c + c).join("");
  const [r, g, b] = [0, 2, 4].map((i) => {
    const v = parseInt(h.slice(i, i + 2), 16) / 255;
    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  }) as [number, number, number];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/**
 * CSS for the branding palettes plus the user's own custom colours. The
 * custom theme starts from the light or dark defaults depending on how dark
 * its background is, so unspecified colours still fit.
 */
export function brandingCss(b: Branding, userCustom: Palette = {}): string {
  const light = declarations(b.light);
  const dark = declarations(b.dark);
  const custom = { ...b.custom, ...userCustom };
  const bg = custom["--bg"];
  const isDark = !!bg && validColour(bg) && luminance(bg) < 0.2;
  let css = "";
  if (light) css += `:root[data-theme="light"]{${light}}@media (prefers-color-scheme: light){:root:not([data-theme]){${light}}}`;
  if (dark) css += `:root[data-theme="dark"]{${dark}}@media (prefers-color-scheme: dark){:root:not([data-theme]){${dark}}}`;
  // Custom inherits the light defaults from :root; a dark custom theme also takes the dark set.
  const base = isDark
    ? "--bg:#0f141b;--surface:#161c25;--surface-2:#1e2631;--text:#e5e9f0;--muted:#94a0b2;--border:#2a3441;--accent:#4f8cff;--accent-soft:#1d2e4d;--danger:#f05252;--ok:#31c46b;color-scheme:dark;"
    : "color-scheme:light;";
  css += `:root[data-theme="custom"]{${base}${declarations(custom)}}`;
  return css;
}

let branding: Branding = DEFAULT_BRANDING;
let userColours: Palette = {};
const listeners = new Set<() => void>();

function apply() {
  let el = document.getElementById("fb-branding") as HTMLStyleElement | null;
  if (!el) {
    el = document.createElement("style");
    el.id = "fb-branding";
    document.head.appendChild(el);
  }
  el.textContent = brandingCss(branding, userColours);
  document.title = branding.name;
  const meta = document.querySelector('meta[name="theme-color"]');
  const accent = branding.light["--accent"];
  if (meta && accent && validColour(accent)) meta.setAttribute("content", accent);
  listeners.forEach((fn) => fn());
}

/** Current branding. */
export const getBranding = () => branding;

/** Replaces the branding (after loading or an admin edit) and applies it. */
export function setBranding(b: Branding): void {
  branding = { ...DEFAULT_BRANDING, ...b };
  apply();
}

/** Sets the signed-in user's custom colours and applies them. */
export function setUserColours(p: Palette): void {
  userColours = p;
  apply();
}

/** The signed-in user's custom colours. */
export const getUserColours = () => userColours;

/** Fetches the branding; failures keep the defaults. */
export async function loadBranding(): Promise<Branding> {
  try {
    setBranding(await request<Branding>("GET", "/api/branding"));
  } catch {
    /* unbranded is fine */
  }
  return branding;
}

const subscribe = (fn: () => void) => {
  listeners.add(fn);
  return () => listeners.delete(fn);
};

/** React hook: the current branding, re-rendering when it changes. */
export function useBranding(): Branding {
  return useSyncExternalStore(subscribe, getBranding);
}

/** React hook: the user's custom colours. */
export function useUserColours(): Palette {
  return useSyncExternalStore(subscribe, getUserColours);
}

export const themeApi = {
  get: () => request<UserTheme>("GET", "/api/me/theme"),
  put: (t: UserTheme) => request<UserTheme>("PUT", "/api/me/theme", t),
  putBranding: (b: Omit<Branding, "logo_url">) => request<Branding>("PUT", "/api/admin/branding", b),
  putLogo: (file: Blob) => request<void>("PUT", "/api/admin/branding/logo", file),
  deleteLogo: () => request<void>("DELETE", "/api/admin/branding/logo"),
};
