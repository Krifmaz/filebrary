/**
 * Top-level component: checks the session, shows the login page when needed,
 * and renders the header plus the page for the current route.
 */
import { useCallback, useEffect, useState } from "react";
import { api, ApiError, setUnauthorizedHandler, type Source, type User } from "./api";
import { Browser } from "./components/Browser";
import { Login } from "./components/Login";
import { PublicShare } from "./components/PublicShare";
import { SharesPage } from "./components/SharesPage";
import { UploadPanel } from "./components/UploadPanel";
import { filesUrl, navigate, useRoute } from "./router";
import { useTheme } from "./theme";
import { CollectionsPage } from "./features/collections/CollectionsPage";
import { AppearancePage } from "./features/theme/AppearancePage";
import { useBranding, useUserColours } from "./features/theme/branding";
import { syncUserTheme } from "./features/theme/sync";
import { BrandMark } from "./features/theme/Brand";
import { UploadManager } from "./upload";

const THEME_LABEL = { system: "◐ Auto", light: "☀ Light", dark: "☾ Dark", custom: "🎨 Custom" } as const;

/** The application shell. */
export function App() {
  const route = useRoute();
  const branding = useBranding();
  const userColours = useUserColours();
  const [theme, cycleTheme] = useTheme(Object.keys(branding.custom).length > 0 || Object.keys(userColours).length > 0);
  const [user, setUser] = useState<User | null | undefined>(undefined);
  const [sources, setSources] = useState<Source[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [uploads] = useState(() => new UploadManager({ onComplete: () => setRefreshKey((n) => n + 1) }));

  useEffect(() => {
    setUnauthorizedHandler(() => setUser(null));
    if (route.name === "public") return;
    api
      .me()
      .then(setUser)
      .catch((e: unknown) => {
        if (e instanceof ApiError && e.status === 401) setUser(null);
        else setError(e instanceof Error ? e.message : String(e));
      });
    // Only on first load; later 401s arrive through the handler.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    void syncUserTheme(!!user);
  }, [user]);

  useEffect(() => {
    if (!user) return;
    api.sources().then(setSources).catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
  }, [user]);

  // Send signed-out visitors to the login page, remembering where they were going.
  useEffect(() => {
    if (user === null && route.name !== "login" && route.name !== "public") {
      navigate(`/login?next=${encodeURIComponent(location.pathname + location.search)}`, true);
    }
  }, [user, route.name]);

  // "/" opens the first source.
  useEffect(() => {
    if (route.name === "home" && sources?.[0]) navigate(filesUrl(sources[0].id), true);
  }, [route.name, sources]);

  const onLogin = useCallback(
    (u: User) => {
      setUser(u);
      const next = route.name === "login" ? route.next : "/";
      navigate(next.startsWith("/") && !next.startsWith("//") && !next.startsWith("/login") ? next : "/", true);
    },
    [route],
  );

  const logout = async () => {
    try {
      await api.logout();
    } finally {
      setUser(null);
      setSources(null);
    }
  };

  if (route.name === "public") return <PublicShare token={route.token} path={route.path} />;
  if (error && user === undefined) return <main className="center"><p className="error">{error}</p></main>;
  if (user === undefined) return <main className="center muted">Loading…</main>;
  if (user === null) return <Login onLogin={onLogin} />;

  const current = route.name === "files" ? sources?.find((s) => s.id === route.source) : undefined;

  return (
    <div className="app">
      <header className="topbar">
        <a className="brand" href="/" onClick={(e) => { e.preventDefault(); navigate("/"); }}>
          <BrandMark />
        </a>
        {sources && sources.length > 1 && (
          <select
            aria-label="Source"
            value={route.name === "files" ? route.source : ""}
            onChange={(e) => navigate(filesUrl(e.target.value))}
          >
            {route.name !== "files" && <option value="">Choose a source</option>}
            {sources.map((s) => (
              <option key={s.id} value={s.id}>{s.name}</option>
            ))}
          </select>
        )}
        <span className="grow" />
        <a href="/shares" className={route.name === "shares" ? "active" : undefined} onClick={(e) => { e.preventDefault(); navigate("/shares"); }}>
          Shares
        </a>
        <a href="/collections" className={route.name === "collections" ? "active" : undefined} onClick={(e) => { e.preventDefault(); navigate("/collections"); }}>
          Collections
        </a>
        <a href="/settings/appearance" className={route.name === "appearance" ? "active" : undefined} onClick={(e) => { e.preventDefault(); navigate("/settings/appearance"); }}>
          Appearance
        </a>
        <button className="ghost" onClick={cycleTheme} title="Change theme">{THEME_LABEL[theme]}</button>
        <span className="muted user">{user.username}</span>
        <button className="ghost" onClick={logout}>Sign out</button>
      </header>
      <main className="content">
        {error && <p className="error" role="alert">{error}</p>}
        {route.name === "files" && sources && !current && <p className="error">Unknown source “{route.source}”.</p>}
        {route.name === "files" && current && (
          <Browser key={`${current.id}:${route.path}`} source={current} path={route.path} uploads={uploads} refreshKey={refreshKey} user={user.id} />
        )}
        {route.name === "home" && sources?.length === 0 && (
          <p className="muted">No storage sources are available to you. Ask an administrator for access.</p>
        )}
        {route.name === "shares" && <SharesPage />}
        {route.name === "collections" && <CollectionsPage />}
        {route.name === "appearance" && <AppearancePage isAdmin={user.is_admin} />}
        {route.name === "notFound" && <p className="muted">Page not found.</p>}
      </main>
      <UploadPanel manager={uploads} />
    </div>
  );
}
