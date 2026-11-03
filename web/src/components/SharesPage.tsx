/** Lists the user's share links with their limits, and lets them copy or revoke each one. */
import { useEffect, useState } from "react";
import { api, type Share } from "../api";
import { formatDate } from "../paths";
import { copyText } from "./ShareDialog";

/** The "My shares" page. */
export function SharesPage() {
  const [shares, setShares] = useState<Share[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [reload, setReload] = useState(0);

  useEffect(() => {
    api
      .shares()
      .then((s) => {
        setShares(s);
        setError(null);
      })
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
  }, [reload]);

  const revoke = async (s: Share) => {
    if (!confirm(`Revoke the link to ${s.collection_id != null ? "this collection" : s.paths.join(", ")}? Anyone using it will lose access.`)) return;
    try {
      await api.deleteShare(s.id);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
    setReload((n) => n + 1);
  };

  return (
    <section className="page">
      <h1>Share links</h1>
      {error && <p className="error" role="alert">{error}</p>}
      {shares === null && !error && <p className="muted">Loading…</p>}
      {shares?.length === 0 && <p className="muted">You have no share links. Select files and press Share to create one.</p>}
      {shares && shares.length > 0 && (
        <div className="table-wrap">
          <table className="files shares">
            <thead>
              <tr>
                <th>Items</th>
                <th>Type</th>
                <th>Expires</th>
                <th>Downloads</th>
                <th>Created</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {shares.map((s) => (
                <tr key={s.id}>
                  <td className="name">
                    <span title={s.paths.join("\n")}>
                      {s.collection_id != null
                        ? "Collection"
                        : `${s.source}: ${s.paths.length === 1 ? s.paths[0] : `${s.paths.length} items`}`}
                    </span>
                    {s.has_password && <span className="badge">password</span>}
                  </td>
                  <td>{s.upload_only ? "Upload-only" : "Download"}</td>
                  <td>{s.expires_at ? formatDate(s.expires_at) : "Never"}</td>
                  <td>
                    {s.downloads}
                    {s.max_downloads ? ` / ${s.max_downloads}` : ""}
                  </td>
                  <td>{formatDate(s.created_at)}</td>
                  <td className="actions">
                    <button
                      onClick={async () => {
                        if (await copyText(s.url)) setCopied(s.id);
                      }}
                    >
                      {copied === s.id ? "Copied" : "Copy link"}
                    </button>
                    <button className="danger" onClick={() => revoke(s)}>Revoke</button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}
