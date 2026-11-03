/**
 * The page a visitor sees when opening a share link (`/s/{token}`): a
 * password form when the link is protected, a drop zone for upload-only
 * links, or a listing with downloads. Works without signing in.
 */
import { useEffect, useState } from "react";
import { api, ApiError, type PublicShare as Info } from "../api";
import { crumbs, formatDate, formatSize } from "../paths";
import { Button, EmptyState, ErrorState, Field, Icon, SkeletonRows, Spinner, fileIcon } from "../ui";
import { encodePath, navigate } from "../router";
import { BrandMark } from "../features/theme/Brand";

/** Props for {@link PublicShare}. */
export interface PublicShareProps {
  token: string;
  /** Share-relative folder path, `/` for the top of the share. */
  path: string;
}

/** Share paths are relative and have no leading slash. */
const rel = (p: string) => p.replace(/^\/+/, "");
const shareUrl = (token: string, path: string) => `/s/${encodeURIComponent(token)}/${encodePath(path)}`;

/** Public share-link page. */
export function PublicShare({ token, path }: PublicShareProps) {
  const [info, setInfo] = useState<Info | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [password, setPassword] = useState("");
  const [reload, setReload] = useState(0);

  useEffect(() => {
    api
      .publicShare(token, rel(path) || undefined)
      .then((i) => {
        setInfo(i);
        setError(null);
      })
      .catch((e: unknown) => {
        if (e instanceof ApiError && e.status === 404) setError("This link does not exist or has expired.");
        else if (e instanceof ApiError && e.status === 410) setError("This link has reached its download limit.");
        else setError(e instanceof Error ? e.message : String(e));
      });
  }, [token, path, reload]);

  const unlock = async (e: React.FormEvent) => {
    e.preventDefault();
    try {
      await api.unlockShare(token, password);
      setPassword("");
      setReload((n) => n + 1);
    } catch (err) {
      setError(err instanceof ApiError && err.status === 401 ? "Wrong password." : err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <main className="public">
      <header className="public-head">
        <div className="brand">
          <BrandMark size={28} />
        </div>
      </header>
      <section className="card public-card">
        {error && !info?.needs_password && <ErrorState compact title="This link cannot be opened" error={error} />}
        {!info && !error && <SkeletonRows rows={4} />}
        {info?.needs_password && (
          <form onSubmit={unlock}>
            <span className="state-icon"><Icon name="lock" /></span>
            <h1>This link is protected</h1>
            <p className="muted">Enter the password you were given.</p>
            <Field label="Password" error={error}>
              <input type="password" autoFocus value={password} onChange={(e) => setPassword(e.target.value)} required />
            </Field>
            <Button variant="primary" type="submit">Open</Button>
          </form>
        )}
        {info && !info.needs_password && info.upload_only && <DropBox token={token} name={info.name} />}
        {info && !info.needs_password && !info.upload_only && (
          <>
            <div className="row spread">
              <div className="row">
                <span className={info.is_dir ? "file-icon dir" : "file-icon"}><Icon name={fileIcon(info.name, info.is_dir)} /></span>
                <h1>{info.name}</h1>
              </div>
              <a className="button primary" href={api.publicDownloadUrl(token, rel(path) || undefined)} download>
                <Icon name="download" />
                {info.is_dir ? "Download all (zip)" : "Download"}
              </a>
            </div>
            {info.is_dir && (
              <>
                {path !== "/" && (
                  <nav className="crumbs small" aria-label="Folder path" style={{ marginTop: "var(--space-4)" }}>
                    <ol>
                      {crumbs(path).map((c, i) => (
                        <li key={c.path}>
                          {i > 0 && <Icon name="chevronRight" className="crumb-sep" />}
                          <a className="crumb" href={shareUrl(token, c.path)} onClick={(e) => { e.preventDefault(); navigate(shareUrl(token, c.path)); }}>
                            {i === 0 ? info.name || "/" : c.name}
                          </a>
                        </li>
                      ))}
                    </ol>
                  </nav>
                )}
                {info.entries?.length === 0 ? (
                  <EmptyState compact icon="folder" title="This folder is empty" />
                ) : (
                  <div className="table-wrap">
                    <table className="files">
                      <tbody>
                        {info.entries?.map((e) => (
                          <tr key={e.path}>
                            <td className="name">
                              <span className="name-cell">
                                <span className={e.is_dir ? "file-icon dir" : "file-icon"}><Icon name={fileIcon(e.name, e.is_dir)} /></span>
                                {e.is_dir ? (
                                  <a className="name-text" href={shareUrl(token, e.path)} onClick={(ev) => { ev.preventDefault(); navigate(shareUrl(token, e.path)); }}>
                                    {e.name}
                                  </a>
                                ) : (
                                  <a className="name-text" href={api.publicDownloadUrl(token, rel(e.path))} download>{e.name}</a>
                                )}
                              </span>
                              <span className="name-meta">{e.is_dir ? formatDate(e.modified) : `${formatSize(e.size)} · ${formatDate(e.modified)}`}</span>
                            </td>
                            <td className="size">{e.is_dir ? "—" : formatSize(e.size)}</td>
                            <td className="date">{formatDate(e.modified)}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}
              </>
            )}
          </>
        )}
      </section>
    </main>
  );
}

/** Upload area for drop links. Visitors can add files but never see the folder. */
function DropBox({ token, name }: { token: string; name: string }) {
  const [status, setStatus] = useState<{ name: string; state: "sending" | "done" | "failed"; error?: string }[]>([]);
  const [over, setOver] = useState(false);

  const send = async (files: File[]) => {
    for (const f of files) {
      setStatus((s) => [...s, { name: f.name, state: "sending" }]);
      const set = (state: "done" | "failed", error?: string) =>
        setStatus((s) => s.map((x) => (x.name === f.name && x.state === "sending" ? { ...x, state, error } : x)));
      try {
        await api.publicUpload(token, f.name, f);
        set("done");
      } catch (e) {
        set("failed", e instanceof Error ? e.message : String(e));
      }
    }
  };

  return (
    <div
      className={`dropbox${over ? " over" : ""}`}
      onDragOver={(e) => { e.preventDefault(); setOver(true); }}
      onDragLeave={() => setOver(false)}
      onDrop={(e) => { e.preventDefault(); setOver(false); void send(Array.from(e.dataTransfer.files)); }}
    >
      <span className="state-icon"><Icon name="upload" /></span>
      <h1>Send files{name ? ` to ${name}` : ""}</h1>
      <p className="muted">Drop files here or choose them. You will not be able to see other files in this folder.</p>
      <label className="button primary">
        <Icon name="plus" />
        Choose files
        <input type="file" multiple hidden onChange={(e) => { if (e.target.files) void send(Array.from(e.target.files)); e.target.value = ""; }} />
      </label>
      <ul className="drop-status">
        {status.map((s, i) => (
          <li key={i} className={s.state}>
            {s.state === "sending" ? <Spinner size="sm" /> : <Icon name={s.state === "done" ? "checkCircle" : "alert"} />} {s.name}
            {s.state === "failed" && `: ${s.error}`}
          </li>
        ))}
      </ul>
    </div>
  );
}
