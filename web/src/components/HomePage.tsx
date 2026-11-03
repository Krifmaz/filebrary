/**
 * The start page: a card for every source the user can reach, with what
 * they may do there, followed by their most recent stars.
 */
import { useEffect, useState } from "react";
import type { Permissions, Source, User } from "../api";
import { library, type Star } from "../library";
import { filesUrl, navigate } from "../router";
import { Badge, EmptyState, Icon, PageHeader, Skeleton, fileIcon } from "../ui";
import { sourceIcon } from "./Shell";

/** Props for {@link HomePage}. */
export interface HomePageProps {
  user: User;
  sources: Source[] | null;
}

/** A short summary of a permission set, e.g. "Full access" or "View only". */
export function accessLabel(p: Permissions): { text: string; tone: "accent" | "neutral" | "info" } {
  const all = p.view && p.download && p.upload && p.edit && p.delete && p.share;
  if (all) return { text: "Full access", tone: "accent" };
  if (!p.upload && !p.edit && !p.delete) return { text: p.download ? "Read only" : "View only", tone: "neutral" };
  if (p.upload && !p.edit && !p.delete) return { text: "Can upload", tone: "info" };
  return { text: "Can edit", tone: "info" };
}

const PERMS: [keyof Permissions, string][] = [
  ["view", "View"],
  ["download", "Download"],
  ["upload", "Upload"],
  ["edit", "Edit"],
  ["delete", "Delete"],
  ["share", "Share"],
];

/** A greeting for the time of day. */
function greeting(now = new Date()): string {
  const h = now.getHours();
  return h < 5 ? "Good evening" : h < 12 ? "Good morning" : h < 18 ? "Good afternoon" : "Good evening";
}

/** The sources overview. */
export function HomePage({ user, sources }: HomePageProps) {
  const [stars, setStars] = useState<Star[] | null>(null);
  useEffect(() => {
    library.stars().then(setStars, () => setStars([]));
  }, []);

  return (
    <div className="page home">
      <PageHeader title={`${greeting()}, ${user.username}`} description="Pick a source to browse, or jump back to something you starred." />

      <section aria-labelledby="sources-h">
        <h2 id="sources-h" className="section-title">Sources</h2>
        {sources === null ? (
          <div className="source-grid">
            {[0, 1, 2].map((i) => (
              <div className="source-card" key={i} aria-hidden>
                <Skeleton width={44} height={44} radius="var(--radius-md)" />
                <Skeleton width="60%" height={14} />
                <Skeleton width="40%" height={10} />
              </div>
            ))}
          </div>
        ) : sources.length === 0 ? (
          <EmptyState icon="hardDrive" title="No sources yet">
            No storage is shared with you. Ask an administrator to give you access to a source.
          </EmptyState>
        ) : (
          <ul className="source-grid">
            {sources.map((s) => {
              const access = accessLabel(s.permissions);
              return (
                <li key={s.id}>
                  <a
                    className="source-card"
                    href={filesUrl(s.id)}
                    onClick={(e) => {
                      if (e.metaKey || e.ctrlKey || e.shiftKey) return;
                      e.preventDefault();
                      navigate(filesUrl(s.id));
                    }}
                  >
                    <span className="source-icon"><Icon name={sourceIcon(s)} /></span>
                    <span className="source-name">{s.name}</span>
                    <span className="source-meta">
                      <Badge tone={access.tone}>{access.text}</Badge>
                    </span>
                    <span className="source-perms" aria-label="Permissions">
                      {PERMS.map(([k, label]) => (
                        <span key={k} className={s.permissions[k] ? "perm on" : "perm"} title={`${label}: ${s.permissions[k] ? "yes" : "no"}`}>
                          {s.permissions[k] ? <Icon name="check" /> : <Icon name="x" />}
                          {label}
                        </span>
                      ))}
                    </span>
                    <Icon name="chevronRight" className="source-go" />
                  </a>
                </li>
              );
            })}
          </ul>
        )}
      </section>

      <section aria-labelledby="starred-h" className="home-section">
        <div className="section-head">
          <h2 id="starred-h" className="section-title">Starred</h2>
          {stars && stars.length > 0 && (
            <a href="/starred" onClick={(e) => { e.preventDefault(); navigate("/starred"); }}>
              See all
            </a>
          )}
        </div>
        {stars === null ? (
          <div className="card"><div className="skeleton-row"><Skeleton width="40%" /></div></div>
        ) : stars.length === 0 ? (
          <div className="card">
            <EmptyState compact icon="star" title="Nothing starred yet">
              Star files and folders from their menu to keep them one click away.
            </EmptyState>
          </div>
        ) : (
          <ul className="card quick-list">
            {stars.slice(0, 6).map((st) => (
              <li key={`${st.source}:${st.path}`}>
                <StarLink star={st} sources={sources} />
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

/** A link to a starred item: folders open themselves, files open the folder they are in. */
export function StarLink({ star, sources }: { star: Star; sources: Source[] | null }) {
  const guessDir = !/\.[^/.]+$/.test(star.name);
  const folderOf = (isDir: boolean) => (isDir ? star.path : star.path.slice(0, star.path.lastIndexOf("/")) || "/");
  const href = filesUrl(star.source, folderOf(guessDir));
  const sourceName = sources?.find((s) => s.id === star.source)?.name ?? star.source;
  return (
    <a
      className="quick-item"
      href={href}
      onClick={(e) => {
        if (e.metaKey || e.ctrlKey || e.shiftKey) return;
        e.preventDefault();
        // The star list does not say what is a folder; ask, and fall back to the name.
        library.stat(star.source, star.path).then(
          (st) => navigate(filesUrl(star.source, folderOf(st.is_dir))),
          () => navigate(href),
        );
      }}
    >
      <span className="quick-icon"><Icon name={fileIcon(star.name, guessDir)} /></span>
      <span className="quick-text">
        <span className="quick-name">{star.name}</span>
        <span className="quick-path">{sourceName} · {star.path}</span>
      </span>
      <Icon name="chevronRight" className="quick-go" />
    </a>
  );
}
