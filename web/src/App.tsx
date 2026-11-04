/**
 * Top-level component: checks the session, shows the login page when needed,
 * and renders the app frame ({@link Shell}) with the page for the current
 * route.
 */
import { useCallback, useEffect, useState } from "react";
import { api, ApiError, setUnauthorizedHandler, type Source, type User } from "./api";
import { Browser } from "./components/Browser";
import { Login } from "./components/Login";
import { PublicShare } from "./components/PublicShare";
import { SharesPage } from "./components/SharesPage";
import { UploadPanel } from "./components/UploadPanel";
import { navigate, useRoute } from "./router";
import { Shell } from "./components/Shell";
import { HomePage } from "./components/HomePage";
import { StarredPage } from "./components/StarredPage";
import { SearchPage } from "./components/SearchPage";
import { Button, EmptyState, ErrorState, Spinner, Toaster } from "./ui";
import { CollectionsPage } from "./features/collections/CollectionsPage";
import { AppearancePage } from "./features/theme/AppearancePage";
import { useBranding, useUserColours } from "./features/theme/branding";
import { syncUserTheme } from "./features/theme/sync";
import { UploadManager } from "./upload";
import { SetupPage } from "./features/accounts/SetupPage";
import { SettingsPage } from "./features/accounts/SettingsPage";
import { UsersPage } from "./features/accounts/UsersPage";
import { accountsApi } from "./features/accounts/api";
import { setDefaultViewMode } from "./features/view/viewMode";

/** The application shell. */
export function App() {
  const route = useRoute();
  const branding = useBranding();
  const userColours = useUserColours();
  const [user, setUser] = useState<User | null | undefined>(undefined);
  const [sources, setSources] = useState<Source[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  // Whether first-run setup is waiting (checked once the visitor is signed out).
  const [setup, setSetup] = useState<boolean | undefined>(undefined);
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

  // Signed out: is this a fresh install waiting for its administrator?
  useEffect(() => {
    if (user !== null || setup !== undefined) return;
    accountsApi
      .setupStatus()
      .then((s) => setSetup(s.needed))
      .catch(() => setSetup(false));
  }, [user, setup]);

  // Signed in: apply the saved default view mode on this device.
  useEffect(() => {
    if (!user) return;
    accountsApi
      .preferences()
      .then((p) => setDefaultViewMode(user.id, p.view_mode))
      .catch(() => {});
  }, [user]);

  useEffect(() => {
    if (!user) return;
    api.sources().then(setSources).catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
  }, [user]);

  // Send signed-out visitors to the login page (or setup), remembering where they were going.
  useEffect(() => {
    if (user !== null || route.name === "public" || setup === undefined) return;
    if (setup && route.name !== "setup") navigate("/setup", true);
    else if (!setup && route.name !== "login") {
      const here = route.name === "setup" ? "/" : location.pathname + location.search;
      navigate(`/login?next=${encodeURIComponent(here)}`, true);
    }
  }, [user, route.name, setup]);

  // Setup and login pages are for signed-out visitors only.
  useEffect(() => {
    if (user && route.name === "setup") navigate("/", true);
  }, [user, route.name]);

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

  if (route.name === "public") return <><PublicShare token={route.token} path={route.path} /><Toaster /></>;
  if (error && user === undefined) return <main className="center"><ErrorState error={error} onRetry={() => location.reload()} /></main>;
  if (user === undefined) return <main className="center" aria-busy="true"><Spinner size="lg" label="Loading" /></main>;
  if (user === null && setup === undefined) return <main className="center" aria-busy="true"><Spinner size="lg" label="Loading" /></main>;
  if (user === null && setup) {
    return (
      <>
        <SetupPage
          onDone={(u) => {
            setSetup(false);
            setUser(u);
            navigate("/settings/account", true);
          }}
        />
        <Toaster />
      </>
    );
  }
  if (user === null) return <><Login onLogin={onLogin} next={route.name === "login" ? route.next : "/"} /><Toaster /></>;

  const current = route.name === "files" ? sources?.find((s) => s.id === route.source) : undefined;
  const hasCustom = Object.keys(branding.custom).length > 0 || Object.keys(userColours).length > 0;

  return (
    <>
      <Shell user={user} sources={sources} route={route} hasCustom={hasCustom} onLogout={logout}>
        {error && <ErrorState error={error} compact />}
        {route.name === "files" && sources && !current && (
          <EmptyState icon="hardDrive" title="Source not found" actions={<Button onClick={() => navigate("/")}>Back to overview</Button>}>
            There is no source called “{route.source}”, or you no longer have access to it.
          </EmptyState>
        )}
        {route.name === "files" && current && (
          <Browser key={`${current.id}:${route.path}`} source={current} path={route.path} uploads={uploads} refreshKey={refreshKey} user={user.id} />
        )}
        {route.name === "home" && <HomePage user={user} sources={sources} />}
        {route.name === "starred" && <StarredPage sources={sources} />}
        {route.name === "search" && <SearchPage q={route.q} type={route.type} sources={sources} />}
        {route.name === "shares" && <SharesPage />}
        {route.name === "collections" && <CollectionsPage />}
        {route.name === "appearance" && <AppearancePage isAdmin={user.is_admin} />}
        {route.name === "settings" && <SettingsPage user={user} tab={route.tab} />}
        {route.name === "users" && user.is_admin && <UsersPage me={user} sources={sources} />}
        {route.name === "users" && !user.is_admin && (
          <EmptyState icon="lock" title="Administrators only" actions={<Button onClick={() => navigate("/")}>Back to overview</Button>}>
            Ask an administrator if you need an account changed.
          </EmptyState>
        )}
        {route.name === "notFound" && (
          <EmptyState icon="alert" title="Page not found" actions={<Button onClick={() => navigate("/")}>Back to overview</Button>}>
            The address may be mistyped, or the page has moved.
          </EmptyState>
        )}
      </Shell>
      <UploadPanel manager={uploads} />
      <Toaster />
    </>
  );
}
