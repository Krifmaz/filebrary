/**
 * The "Collections" page: the user's collections, their items, and share
 * links that serve a whole collection (items may come from several sources).
 */
import { useCallback, useEffect, useState } from "react";
import { type Share } from "../../api";
import { Dialog } from "../../components/Dialog";
import { copyText, localToIso } from "../../components/ShareDialog";
import { formatSize, iconFor, parent } from "../../paths";
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
    <section className="collections">
      <div className="toolbar">
        <h1 className="page-title">Collections</h1>
        <span className="grow" />
        <button className="primary" onClick={() => setDialog("new")}>New collection</button>
      </div>
      {error && <p className="error" role="alert">{error}</p>}
      <div className="collections-layout">
        <ul className="collection-list">
          {list === null && <li className="muted">Loading…</li>}
          {list?.length === 0 && (
            <li className="muted">No collections yet. Select files in the browser and press “Add to collection”.</li>
          )}
          {list?.map((c) => (
            <li key={c.id}>
              <button className={`link${open?.id === c.id ? " active" : ""}`} onClick={() => void show(c.id)}>
                {c.name}
              </button>{" "}
              <span className="muted">{c.item_count}</span>
            </li>
          ))}
        </ul>
        {open && (
          <div className="collection-detail">
            <div className="toolbar">
              <h2>{open.name}</h2>
              <span className="grow" />
              <button disabled={open.items.length === 0} onClick={() => setDialog("share")}>Share</button>
              <button onClick={() => setDialog("rename")}>Rename</button>
              <button className="danger" onClick={() => setDialog("delete")}>Delete</button>
            </div>
            <table className="files">
              <tbody>
                {open.items.length === 0 && (
                  <tr>
                    <td className="muted">This collection is empty.</td>
                  </tr>
                )}
                {open.items.map((it) => (
                  <tr key={it.id} className={it.kind ? undefined : "missing"}>
                    <td className="name">
                      <span className="ico" aria-hidden>{iconFor(it.name, it.kind === "dir")}</span>
                      {it.kind && it.path ? (
                        <a
                          href={filesUrl(it.source, it.kind === "dir" ? it.path : parent(it.path))}
                          onClick={(e) => {
                            e.preventDefault();
                            if (it.path) navigate(filesUrl(it.source, it.kind === "dir" ? it.path : parent(it.path)));
                          }}
                        >
                          {it.name}
                        </a>
                      ) : (
                        <span title="Missing or no longer accessible">{it.name} (unavailable)</span>
                      )}
                    </td>
                    <td className="muted">{it.source}{it.path ? `:${it.path}` : ""}</td>
                    <td className="size">{it.size != null ? formatSize(it.size) : ""}</td>
                    <td>
                      <button
                        className="link"
                        aria-label={`Remove ${it.name}`}
                        onClick={async () => {
                          try {
                            await collectionsApi.removeItem(open.id, it.id);
                            await reload(open.id);
                          } catch (e) {
                            setError(message(e));
                          }
                        }}
                      >
                        Remove
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
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
    </section>
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
