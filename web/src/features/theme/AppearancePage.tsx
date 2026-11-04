/**
 * Appearance settings: the user's theme and custom colours and, for
 * administrators, the server's branding (name, logo, default theme and the
 * colours of each theme).
 */
import { useState } from "react";
import { setTheme, useTheme, type Theme } from "../../theme";
import { PageHeader } from "../../ui";
import {
  THEME_VARS,
  themeApi,
  setBranding,
  setUserColours,
  useBranding,
  useUserColours,
  validColour,
  type Branding,
  type Palette,
} from "./branding";

const THEMES: { id: Theme; label: string }[] = [
  { id: "system", label: "Follow the system" },
  { id: "light", label: "Light" },
  { id: "dark", label: "Dark" },
  { id: "custom", label: "Custom" },
];

const LABELS: Record<string, string> = {
  "--bg": "Background",
  "--surface": "Panels",
  "--surface-2": "Panels (secondary)",
  "--text": "Text",
  "--muted": "Muted text",
  "--border": "Borders",
  "--accent": "Accent",
  "--accent-contrast": "Text on accent",
  "--accent-soft": "Accent (soft)",
  "--danger": "Danger",
  "--ok": "Success",
};

/** Expands `#abc` to `#aabbcc` for `<input type="color">`. */
const full = (c: string) => (c.length === 4 ? `#${c[1]}${c[1]}${c[2]}${c[2]}${c[3]}${c[3]}` : c);

/** Editor for a palette: each variable can be left at its default or set. */
export function PaletteEditor({ value, onChange, idPrefix }: { value: Palette; onChange: (p: Palette) => void; idPrefix: string }) {
  return (
    <div className="palette">
      {THEME_VARS.map((k) => {
        const v = value[k];
        const id = `${idPrefix}${k}`;
        const current = v ?? getComputedStyle(document.documentElement).getPropertyValue(k).trim();
        return (
          <div key={k} className="palette-row">
            <label htmlFor={id}>{LABELS[k] ?? k}</label>
            <input
              id={id}
              type="color"
              value={validColour(current) ? full(current) : "#000000"}
              onChange={(e) => onChange({ ...value, [k]: e.target.value })}
            />
            {v ? (
              <button
                type="button"
                className="link"
                onClick={() => {
                  const next = { ...value };
                  delete next[k];
                  onChange(next);
                }}
              >
                Reset
              </button>
            ) : (
              <span className="muted">default</span>
            )}
          </div>
        );
      })}
    </div>
  );
}

/** The appearance page. */
export function AppearancePage({ isAdmin }: { isAdmin: boolean }) {
  const [theme] = useTheme();
  const colours = useUserColours();
  const [status, setStatus] = useState<string | null>(null);

  const saveColours = async (p: Palette) => {
    setUserColours(p);
    try {
      await themeApi.put({ theme, colours: p });
      setStatus(null);
    } catch (e) {
      setStatus(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <section className="settings">
      <PageHeader title="Appearance" description="How Filebrary looks on this account. Your choice follows you to every device." />
      <fieldset>
        <legend>Theme</legend>
        <div className="theme-options">
          {THEMES.map((t) => (
            <label key={t.id} className={theme === t.id ? "theme-option on" : "theme-option"}>
              <span className={`theme-preview ${t.id}`} aria-hidden>
                <i /><i /><i />
              </span>
              <span className="row">
                <input type="radio" name="theme" checked={theme === t.id} onChange={() => setTheme(t.id)} />
                {t.label}
              </span>
            </label>
          ))}
        </div>
      </fieldset>
      {theme === "custom" && (
        <fieldset>
          <legend>My custom colours</legend>
          <p className="muted">Unset colours come from the server's custom theme.</p>
          <PaletteEditor idPrefix="me" value={colours} onChange={(p) => void saveColours(p)} />
        </fieldset>
      )}
      {status && <p className="error" role="alert">{status}</p>}
      {isAdmin && <BrandingEditor />}
    </section>
  );
}

/** Administrator editor for server-wide branding. */
function BrandingEditor() {
  const live = useBranding();
  const [draft, setDraft] = useState<Branding>(live);
  const [tab, setTab] = useState<"light" | "dark" | "custom">("light");
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);

  const run = async (fn: () => Promise<Branding | void>, ok: string) => {
    setMsg(null);
    try {
      const b = await fn();
      if (b) {
        setBranding(b);
        setDraft(b);
      }
      setMsg({ ok: true, text: ok });
    } catch (e) {
      setMsg({ ok: false, text: e instanceof Error ? e.message : String(e) });
    }
  };

  const reloadBranding = async () => {
    const r = await fetch("/api/branding", { credentials: "same-origin" });
    return (await r.json()) as Branding;
  };

  return (
    <form
      className="branding"
      onSubmit={(e) => {
        e.preventDefault();
        const { name, default_theme, light, dark, custom } = draft;
        void run(() => themeApi.putBranding({ name, default_theme, light, dark, custom }), "Branding saved.");
      }}
    >
      <h2>Branding (all users)</h2>
      <label>
        Name
        <input value={draft.name} maxLength={60} onChange={(e) => setDraft({ ...draft, name: e.target.value })} />
      </label>
      <label>
        Default theme
        <select value={draft.default_theme} onChange={(e) => setDraft({ ...draft, default_theme: e.target.value as Theme })}>
          {THEMES.map((t) => (
            <option key={t.id} value={t.id}>{t.label}</option>
          ))}
        </select>
      </label>
      <div className="logo-row">
        {live.logo_url ? <img src={live.logo_url} alt="Current logo" height={40} /> : <span className="muted">No logo</span>}
        <label className="button">
          Upload logo
          <input
            type="file"
            hidden
            accept="image/png,image/jpeg,image/webp,image/gif,image/svg+xml"
            onChange={(e) => {
              const f = e.target.files?.[0];
              e.target.value = "";
              if (f) void run(async () => { await themeApi.putLogo(f); return reloadBranding(); }, "Logo updated.");
            }}
          />
        </label>
        {live.logo_url && (
          <button type="button" onClick={() => void run(async () => { await themeApi.deleteLogo(); return reloadBranding(); }, "Logo removed.")}>
            Remove logo
          </button>
        )}
      </div>
      <div className="tabs" role="tablist">
        {(["light", "dark", "custom"] as const).map((t) => (
          <button key={t} type="button" role="tab" aria-selected={tab === t} className={tab === t ? "active" : undefined} onClick={() => setTab(t)}>
            {t[0]!.toUpperCase() + t.slice(1)} colours
          </button>
        ))}
      </div>
      <PaletteEditor key={tab} idPrefix={`brand-${tab}`} value={draft[tab]} onChange={(p) => setDraft({ ...draft, [tab]: p })} />
      <div className="row">
        <button className="primary" type="submit">Save branding</button>
      </div>
      {msg && <p className={msg.ok ? "muted" : "error"} role="status">{msg.text}</p>}
    </form>
  );
}
