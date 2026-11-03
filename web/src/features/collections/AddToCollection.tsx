/** Dialog that adds the selected files and folders to a new or existing collection. */
import { useEffect, useState } from "react";
import type { ActionDialogProps } from "../../actions";
import { Dialog } from "../../components/Dialog";
import { collectionsApi, type CollectionSummary } from "./api";

/** The dialog opened by the "Add to collection" toolbar action. */
export default function AddToCollection({ source, selected, onClose }: ActionDialogProps) {
  const [list, setList] = useState<CollectionSummary[] | null>(null);
  const [choice, setChoice] = useState<string>("new");
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    collectionsApi
      .list()
      .then((l) => {
        setList(l);
        if (l[0]) setChoice(String(l[0].id));
      })
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
  }, []);

  const items = selected.map((e) => ({ source: source.id, path: e.path }));
  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      const c = choice === "new" ? await collectionsApi.create(name, items) : await collectionsApi.addItems(Number(choice), items);
      setDone(c.name);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  if (done) {
    return (
      <Dialog title="Added" onClose={onClose} actions={<button className="primary" type="button" onClick={onClose}>Done</button>}>
        <p>
          Added {items.length} item{items.length === 1 ? "" : "s"} to <strong>{done}</strong>. Manage and share it under{" "}
          <a href="/collections">Collections</a>.
        </p>
      </Dialog>
    );
  }

  return (
    <Dialog
      title={`Add ${items.length} item${items.length === 1 ? "" : "s"} to a collection`}
      onClose={onClose}
      onSubmit={save}
      actions={
        <>
          <button type="button" onClick={onClose}>Cancel</button>
          <button className="primary" type="submit" disabled={busy || (choice === "new" && !name.trim())}>Add</button>
        </>
      }
    >
      <label>
        Collection
        <select value={choice} onChange={(e) => setChoice(e.target.value)} disabled={list === null}>
          {list?.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name} ({c.item_count})
            </option>
          ))}
          <option value="new">New collection…</option>
        </select>
      </label>
      {choice === "new" && (
        <label>
          Name
          <input autoFocus value={name} maxLength={200} onChange={(e) => setName(e.target.value)} />
        </label>
      )}
      {error && <p className="error" role="alert">{error}</p>}
    </Dialog>
  );
}
