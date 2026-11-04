/**
 * The "Collections" page: the user's collections, their items, and share
 * links that serve a whole collection (items may come from several sources).
 */
import { useCallback, useEffect, useState } from "react";
import { type Share } from "../../api";
import { Dialog } from "../../components/Dialog";
import { copyText, localToIso } from "../../components/ShareDialog";
import { formatSize, parent } from "../../paths";
import { Badge, Button, EmptyState, ErrorState, Icon, IconButton, PageHeader, SkeletonRows, fileIcon } from "../../ui";
import { filesUrl, navigate } from "../../router";
import { collectionsApi, type Collection, type CollectionSummary } from "./api";

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** Lists collections; selecting one shows its items. */
export function CollectionsPage() {
  const [list, setList] = useState<CollectionSummary[] | null>(null);
  const [open, setOpen] = useState<Collection | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [dialog, setDialog] = useState<"new" | "rename" | "delete" | "share" | null>(null);

  const reload = useCallback(async (keep?: number) => {
    try {
      setList(await collectionsApi.list());
      setOpen(keep ? await collectionsApi.get(keep) : null);
      setError(null);
    } catch (e) {
      setError(message(e));
    }
  }, []);
  useEffect(() => {
    collectionsApi
      .list()
      .then(setList)
      .catch((e: unknown) => setError(message(e)));
  }, []);

  const show = async (id: number) => {
    try {
      setOpen(await collectionsApi.get(id));
    } catch (e) {
      setError(message(e));
    }
  };

  return (
    <div className="page collections">
      <PageHeader
        title="Collections"
        description="Group files from any source and share them with one link."
        actions={<Button variant="primary" icon="plus" onClick={() => setDialog("new")}>New collection</Button>}
      />
      {error && <ErrorState compact error={error} />}
      <div className="collections-layout">
        <ul className="collection-list" aria-label="Your collections">
          {list === null && <li><SkeletonRows rows={3} /></li>}
          {list?.length === 0 && (
            <li><EmptyState compact icon="collection" title="No collections yet">Select files and choose “Add to collection”.</EmptyState></li>
          )}
          {list?.map((c) => (
            <li key={c.id}>
              <button className={open?.id === c.id ? "active" : undefined} aria-current={open?.id === c.id || undefined} onClick={() => void show(c.id)}>
                <Icon name="collection" />
                <span className="grow ellipsis" style={{ textAlign: "left" }}>{c.name}</span>
                <Badge>{c.item_count}</Badge>
              </button>
            </li>
          ))}
        </ul>
        {!open && list && list.length > 0 && (
          <div className="card"><EmptyState icon="collection" title="Pick a collection">Choose one on the left to see its items.</EmptyState></div>
        )}
        {open && (
          <div className="collection-detail">
            <div className="row wrap" style={{ marginBottom: "var(--space-3)" }}>
              <h2 className="grow">{open.name}</h2>
              <Button icon="share" disabled={open.items.length === 0} onClick={() => setDialog("share")}>Share</Button>
              <Button icon="edit" onClick={() => setDialog("rename")}>Rename</Button>
              <Button icon="trash" variant="danger" onClick={() => setDialog("delete")}>Delete</Button>
            </div>
            {open.items.length === 0 ? (
              <div className="card"><EmptyState compact icon="inbox" title="This collection is empty">Select files in a source and choose “Add to collection”.</EmptyState></div>
            ) : (
              <div className="table-wrap">
                <table className="files">
                  <tbody>
                    {open.items.map((it) => (
                      <tr key={it.id} className={it.kind ? undefined : "missing"}>
                        <td className="name">
                          <span className="name-cell">
                            <span className={it.kind === "dir" ? "file-icon dir" : "file-icon"}><Icon name={fileIcon(it.name, it.kind === "dir")} /></span>
                            {it.kind && it.path ? (
                              <a
                                className="name-text"
                                href={filesUrl(it.source, it.kind === "dir" ? it.path : parent(it.path))}
                                onClick={(e) => {
                                  e.preventDefault();
                                  if (it.path) navigate(filesUrl(it.source, it.kind === "dir" ? it.path : parent(it.path)));
                                }}
                              >
                                {it.name}
                              </a>
                            ) : (
                              <span className="name-text" title="Missing or no longer accessible">{it.name} (unavailable)</span>
                            )}
                          </span>
                          <span className="name-meta">{it.source}{it.path ? `:${it.path}` : ""}</span>
                        </td>
                        <td className="date">{it.source}{it.path ? `:${it.path}` : ""}</td>
                        <td className="size">{it.size != null ? formatSize(it.size) : "—"}</td>
                        <td className="actions">
                          <IconButton
                            size="sm"
                            icon="x"
                            label={`Remove ${it.name}`}
                            onClick={async () => {
                              try {
                                await collectionsApi.removeItem(open.id, it.id);
                                await reload(open.id);
                              } catch (e) {
                                setError(message(e));
                              }
                            }}
                          />
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        )}
      </div>

      {(dialog === "new" || dialog === "rename") && (
        <NameDialog
          title={dialog === "new" ? "New collection" : `Rename ${open?.name ?? ""}`}
          initial={dialog === "rename" ? (open?.name ?? "") : ""}
          onClose={() => setDialog(null)}
          onSave={async (name) => {
            if (dialog === "new") {
              const c = await collectionsApi.create(name);
              await reload(c.id);
            } else if (open) {
              await collectionsApi.rename(open.id, name);
              await reload(open.id);
            }
            setDialog(null);
          }}
        />
      )}
      {dialog === "delete" && open && (
        <Dialog
          title="Delete collection"
          onClose={() => setDialog(null)}
          onSubmit={async () => {
            setDialog(null);
            try {
              await collectionsApi.remove(open.id);
              await reload();
            } catch (e) {
              setError(message(e));
            }
          }}
          actions={
            <>
              <button type="button" onClick={() => setDialog(null)}>Cancel</button>
              <button className="danger" type="submit" autoFocus>Delete</button>
            </>
          }
        >
          <p>
            Delete <strong>{open.name}</strong>? The files stay where they are; share links to this collection stop working.
          </p>
        </Dialog>
      )}
      {dialog === "share" && open && <ShareCollection collection={open} onClose={() => setDialog(null)} />}
    </div>
  );
}

/** Asks for a collection name. `onSave` errors are shown in the dialog. */
function NameDialog(props: { title: string; initial: string; onSave: (name: string) => Promise<void>; onClose: () => void }) {
  const [name, setName] = useState(props.initial);
  const [error, setError] = useState<string | null>(null);
  return (
    <Dialog
      title={props.title}
      onClose={props.onClose}
      onSubmit={() => props.onSave(name.trim()).catch((e: unknown) => setError(message(e)))}
      actions={
        <>
          <button type="button" onClick={props.onClose}>Cancel</button>
          <button className="primary" type="submit" disabled={!name.trim()}>Save</button>
        </>
      }
    >
      <label>
        Name
        <input autoFocus value={name} maxLength={200} onChange={(e) => setName(e.target.value)} />
      </label>
      {error && <p className="error" role="alert">{error}</p>}
    </Dialog>
  );
}

/** Creates a share link for a whole collection. */
function ShareCollection({ collection, onClose }: { collection: Collection; onClose: () => void }) {
  const [password, setPassword] = useState("");
  const [expires, setExpires] = useState("");
  const [limit, setLimit] = useState("");
  const [share, setShare] = useState<Share | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  if (share) {
    return (
      <Dialog title="Share link ready" onClose={onClose} actions={<button className="primary" type="button" onClick={onClose}>Done</button>}>
        <p>Anyone with this link can open the {collection.items.length} items of {collection.name}.</p>
        <div className="row">
          <input readOnly value={share.url} onFocus={(e) => e.target.select()} aria-label="Share URL" />
          <button type="button" onClick={async () => setCopied(await copyText(share.url))}>{copied ? "Copied" : "Copy"}</button>
        </div>
      </Dialog>
    );
  }
  return (
    <Dialog
      title={`Share ${collection.name}`}
      onClose={onClose}
      onSubmit={async () => {
        setError(null);
        try {
          const n = limit ? Number.parseInt(limit, 10) : 0;
          setShare(
            await collectionsApi.share({
              collection_id: collection.id,
              password: password || undefined,
              expires_at: localToIso(expires),
              max_downloads: n > 0 ? n : null,
            }),
          );
        } catch (e) {
          setError(message(e));
        }
      }}
      actions={
        <>
          <button type="button" onClick={onClose}>Cancel</button>
          <button className="primary" type="submit">Create link</button>
        </>
      }
    >
      <p className="muted">The link always shows the collection's current items that you are still allowed to share.</p>
      <label>
        Password (optional)
        <input type="password" autoComplete="new-password" value={password} onChange={(e) => setPassword(e.target.value)} />
      </label>
      <label>
        Expires (optional)
        <input type="datetime-local" value={expires} onChange={(e) => setExpires(e.target.value)} />
      </label>
      <label>
        Download limit (optional)
        <input type="number" min={1} value={limit} onChange={(e) => setLimit(e.target.value)} />
      </label>
      {error && <p className="error" role="alert">{error}</p>}
    </Dialog>
  );
}
