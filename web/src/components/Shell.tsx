/**
 * The signed-in app frame: a sidebar with the sources and library links, a
 * top bar with search and the user menu, and the page in between.
 *
 * Desktop and tablet show the sidebar beside the page (tablet collapses it
 * to icons). Phones hide it behind the menu button as a drawer.
 */
import { useEffect, useState, type ReactNode } from "react";
import type { Source, User } from "../api";
import { filesUrl, navigate, type Route } from "../router";
import { getTheme, setTheme, useTheme, type Theme } from "../theme";
import { BrandMark } from "../features/theme/Brand";
import { Avatar, Icon, IconButton, MenuButton, SearchInput, type IconName, type MenuEntry } from "../ui";

/** Props for {@link Shell}. */
export interface ShellProps {
  user: User;
  sources: Source[] | null;
  route: Route;
  /** Whether a custom theme exists, so it can be offered. */
  hasCustom: boolean;
  onLogout: () => void;
  children: ReactNode;
}

const THEME_ICON: Record<Theme, IconName> = { system: "monitor", light: "sun", dark: "moon", custom: "palette" };
const THEME_LABEL: Record<Theme, string> = { system: "Follow the system", light: "Light", dark: "Dark", custom: "Custom" };

/** Icon for a source, guessed from its id and name. */
export function sourceIcon(s: Pick<Source, "id" | "name">): IconName {
  const t = `${s.id} ${s.name}`.toLowerCase();
  if (/\b(s3|minio|bucket|cloud|b2|r2)\b/.test(t)) return "cloud";
  if (/\b(sftp|smb|nas|server|share|cifs)\b/.test(t)) return "server";
  if (/\bhome\b/.test(t)) return "home";
  return "hardDrive";
}

/** A sidebar link. */
function NavLink({ href, icon, label, active, onNavigate }: { href: string; icon: IconName; label: string; active: boolean; onNavigate: () => void }) {
  return (
    <a
      href={href}
      className={active ? "nav-link active" : "nav-link"}
      aria-current={active ? "page" : undefined}
      title={label}
      onClick={(e) => {
        if (e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
        e.preventDefault();
        onNavigate();
        navigate(href);
      }}
    >
      <Icon name={icon} />
      <span className="nav-text">{label}</span>
    </a>
  );
}

/** The app frame around every signed-in page. */
export function Shell({ user, sources, route, hasCustom, onLogout, children }: ShellProps) {
  const [drawer, setDrawer] = useState(false);
  const [theme, cycleTheme] = useTheme(hasCustom);
  const routeQ = route.name === "search" ? route.q : null;
  const [query, setQuery] = useState(routeQ ?? "");
  const [seenQ, setSeenQ] = useState(routeQ);
  // Follow the address bar when a search is opened from elsewhere (adjusting state while rendering).
  if (routeQ !== seenQ) {
    setSeenQ(routeQ);
    if (routeQ !== null) setQuery(routeQ);
  }

  // Close the drawer with Escape.
  useEffect(() => {
    if (!drawer) return;
    const k = (e: KeyboardEvent) => e.key === "Escape" && setDrawer(false);
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [drawer]);

  const close = () => setDrawer(false);
  const themes: Theme[] = hasCustom ? ["system", "light", "dark", "custom"] : ["system", "light", "dark"];
  const userMenu: MenuEntry[] = [
    { heading: user.username },
    { label: "Appearance", icon: "palette", onSelect: () => navigate("/settings/appearance") },
    { separator: true },
    { heading: "Theme" },
    ...themes.map((t) => ({ label: THEME_LABEL[t], icon: THEME_ICON[t], checked: getTheme() === t, onSelect: () => setTheme(t) })),
    { separator: true },
    { label: "Sign out", icon: "logout", onSelect: onLogout },
  ];

  return (
    <div className={drawer ? "shell drawer-open" : "shell"}>
      <a className="skip-link" href="#main">Skip to content</a>
      <aside className="sidebar" aria-label="Navigation">
        <div className="sidebar-head">
          <a
            className="brand"
            href="/"
            onClick={(e) => {
              e.preventDefault();
              close();
              navigate("/");
            }}
          >
            <BrandMark />
          </a>
          <IconButton className="drawer-close" icon="x" label="Close menu" onClick={close} />
        </div>
        <nav className="nav">
          <NavLink href="/" icon="grid" label="Overview" active={route.name === "home"} onNavigate={close} />
          <div className="nav-label">Sources</div>
          {sources === null && <div className="nav-skeleton" aria-hidden><span /><span /></div>}
          {sources?.map((s) => (
            <NavLink
              key={s.id}
              href={filesUrl(s.id)}
              icon={sourceIcon(s)}
              label={s.name}
              active={route.name === "files" && route.source === s.id}
              onNavigate={close}
            />
          ))}
          <div className="nav-label">Library</div>
          <NavLink href="/starred" icon="star" label="Starred" active={route.name === "starred"} onNavigate={close} />
          <NavLink href="/shares" icon="share" label="Shared" active={route.name === "shares"} onNavigate={close} />
          <NavLink href="/collections" icon="collection" label="Collections" active={route.name === "collections"} onNavigate={close} />
        </nav>
        <div className="sidebar-foot">
          <NavLink href="/settings/appearance" icon="settings" label="Settings" active={route.name === "appearance"} onNavigate={close} />
        </div>
      </aside>
      <div className="scrim" onClick={close} aria-hidden />

      <div className="main-col">
        <header className="topbar">
          <IconButton className="menu-toggle" icon="menu" label="Open menu" onClick={() => setDrawer(true)} aria-expanded={drawer} />
          <form
            className="global-search"
            role="search"
            onSubmit={(e) => {
              e.preventDefault();
              const q = query.trim();
              if (q) navigate(`/search?q=${encodeURIComponent(q)}`);
            }}
          >
            <SearchInput
              placeholder="Search all files"
              aria-label="Search all files"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onClear={() => setQuery("")}
            />
          </form>
          <span className="grow" />
          <IconButton icon={THEME_ICON[theme]} label={`Theme: ${THEME_LABEL[theme]} (click to change)`} onClick={cycleTheme} />
          <MenuButton items={userMenu} align="end" label="Account">
            {(p) => (
              <button type="button" className="user-button" aria-label={`Account: ${user.username}`} {...p}>
                <Avatar name={user.username} size={30} />
                <span className="user-name">{user.username}</span>
                <Icon name="chevronDown" className="hide-phone" />
              </button>
            )}
          </MenuButton>
        </header>
        <main id="main" className="content" tabIndex={-1}>
          {children}
        </main>
      </div>
    </div>
  );
}
