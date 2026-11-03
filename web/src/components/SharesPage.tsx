/** Lists the user's share links with their limits, and lets them copy or revoke each one. */
import { useCallback, useEffect, useState } from "react";
import { api, type Share } from "../api";
import { formatDate } from "../paths";
import { Badge, Button, Dialog, EmptyState, ErrorState, Icon, IconButton, PageHeader, SkeletonRows, errorMessage, toast } from "../ui";
import { copyText } from "./ShareDialog";

/** What a share link points at, in words. */
function target(s: Share): string {
  if (s.collection_id != null) return "A collection";
  if (s.paths.length === 1) return s.paths[0] === "/" ? `All of ${s.source}` : (s.paths[0]?.split("/").pop() ?? s.paths[0] ?? "");
  return `${s.paths.length} items`;
}

/** Whether a link can no longer be used. */
function expired(s: Share, now = Date.now()): boolean {
  return (!!s.expires_at && Date.parse(s.expires_at) < now) || (!!s.max_downloads && s.downloads >= s.max_downloads);
}

/** The "Shared" page. */
export function SharesPage() {
  const [shares, setShares] = useState<Share[] | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [revoking, setRevoking] = useState<Share | null>(null);

  const fetchShares = useCallback(() => {
    api.shares().then(
      (s) => {
        setShares(s);
        setError(null);
      },
      (e: unknown) => setError(e),
    );
  }, []);
  useEffect(fetchShares, [fetchShares]);

  const revoke = async (s: Share) => {
    setRevoking(null);
    try {
      await api.deleteShare(s.id);
      setShares((list) => list?.filter((x) => x.id !== s.id) ?? null);
      toast.success("Link revoked");
    } catch (e) {
      toast.error(errorMessage(e));
    }
  };

  return (
    <div className="page">
      <PageHeader title="Shared" description="Links you created. Anyone with a link can open it until it expires or you revoke it." />
      {error ? (
        <ErrorState error={error} onRetry={() => { setError(null); setShares(null); fetchShares(); }} />
      ) : shares === null ? (
        <div className="card"><SkeletonRows rows={4} /></div>
      ) : shares.length === 0 ? (
        <div className="card">
          <EmptyState icon="share" title="No share links yet">
            Select files or a folder and choose <strong>Share</strong> to create a link.
          </EmptyState>
        </div>
      ) : (
        <ul className="card share-list">
          {shares.map((s) => {
            const dead = expired(s);
            return (
              <li key={s.id} className="share-item">
                <span className="quick-icon"><Icon name={s.collection_id != null ? "collection" : s.upload_only ? "inbox" : "link"} /></span>
                <span className="share-title" title={s.paths.join("\n")}>
                  {target(s)}
                  {s.upload_only && <Badge tone="info">Upload only</Badge>}
                  {s.has_password && <Badge><Icon name="lock" size={11} /> Password</Badge>}
                  {dead && <Badge tone="danger">Expired</Badge>}
                </span>
                <span className="share-meta">
                  {s.collection_id == null && <span>{s.source}</span>}
                  <span>{s.expires_at ? `${dead && !s.max_downloads ? "Expired" : "Expires"} ${formatDate(s.expires_at)}` : "Never expires"}</span>
                  <span>{s.downloads}{s.max_downloads ? ` of ${s.max_downloads}` : ""} download{s.downloads === 1 && !s.max_downloads ? "" : "s"}</span>
                  <span>Created {formatDate(s.created_at)}</span>
                </span>
                <span className="share-actions">
                  <Button
                    size="sm"
                    icon="copy"
                    onClick={async () => {
                      if (await copyText(s.url)) toast.success("Link copied");
                      else toast.error("Could not copy. Select the link and copy it by hand.");
                    }}
                  >
                    Copy link
                  </Button>
                  <IconButton size="sm" icon="trash" variant="danger" label="Revoke link" onClick={() => setRevoking(s)} />
                </span>
              </li>
            );
          })}
        </ul>
      )}
      {revoking && (
        <Dialog
          title="Revoke this link?"
          description="Anyone using it will lose access straight away. This cannot be undone."
          onClose={() => setRevoking(null)}
          onSubmit={() => void revoke(revoking)}
          actions={
            <>
              <Button onClick={() => setRevoking(null)}>Cancel</Button>
              <Button type="submit" variant="danger-solid" icon="trash" autoFocus>Revoke</Button>
            </>
          }
        >
          <p className="muted">{target(revoking)}</p>
        </Dialog>
      )}
    </div>
  );
}
