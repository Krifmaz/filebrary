/**
 * Dialog that creates a share link for the selected files or folder, with an
 * optional password, expiry date, download limit, or upload-only mode.
 */
import { useState } from "react";
import { api, type Share } from "../api";
import { basename } from "../paths";
import { Dialog } from "./Dialog";

/** Props for {@link ShareDialog}. */
export interface ShareDialogProps {
  source: string;
  paths: string[];
  /** Upload-only links are offered only when sharing a single folder. */
  singleFolder: boolean;
  onClose: () => void;
}

/** Copies text to the clipboard, returning whether it worked. */
export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

/** Converts a `datetime-local` input value to RFC 3339 in UTC. */
export function localToIso(value: string): string | null {
  if (!value) return null;
  const d = new Date(value);
  return Number.isNaN(d.getTime()) ? null : d.toISOString();
}

/** Share-link creation dialog. */
export function ShareDialog({ source, paths, singleFolder, onClose }: ShareDialogProps) {
  const [password, setPassword] = useState("");
  const [expires, setExpires] = useState("");
  const [maxDownloads, setMaxDownloads] = useState("");
  const [uploadOnly, setUploadOnly] = useState(false);
  const [share, setShare] = useState<Share | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);

  const create = async () => {
    setBusy(true);
    setError(null);
    try {
      const limit = maxDownloads ? Number.parseInt(maxDownloads, 10) : null;
      setShare(
        await api.createShare({
          source,
          paths,
          password: password || undefined,
          expires_at: localToIso(expires),
          max_downloads: limit && limit > 0 ? limit : null,
          upload_only: singleFolder && uploadOnly,
        }),
      );
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const label = paths.length === 1 ? basename(paths[0] ?? "") || "/" : `${paths.length} items`;

  if (share) {
    return (
      <Dialog
        title="Share link ready"
        onClose={onClose}
        actions={<button className="primary" type="button" onClick={onClose}>Done</button>}
      >
        <p>Anyone with this link can {share.upload_only ? "upload into" : "open"} {label}.</p>
        <div className="row">
          <input readOnly value={share.url} onFocus={(e) => e.target.select()} aria-label="Share URL" />
          <button
            type="button"
            onClick={async () => setCopied(await copyText(share.url))}
          >
            {copied ? "Copied" : "Copy"}
          </button>
        </div>
      </Dialog>
    );
  }

  return (
    <Dialog
      title={`Share ${label}`}
      onClose={onClose}
      onSubmit={create}
      actions={
        <>
          <button type="button" onClick={onClose}>Cancel</button>
          <button className="primary" type="submit" disabled={busy}>
            {busy ? "Creating…" : "Create link"}
          </button>
        </>
      }
    >
      <label>
        Password (optional)
        <input type="password" autoComplete="new-password" value={password} onChange={(e) => setPassword(e.target.value)} />
      </label>
      <label>
        Expires (optional)
        <input type="datetime-local" value={expires} onChange={(e) => setExpires(e.target.value)} />
      </label>
      {!uploadOnly && (
        <label>
          Download limit (optional)
          <input type="number" min={1} value={maxDownloads} onChange={(e) => setMaxDownloads(e.target.value)} />
        </label>
      )}
      {singleFolder && (
        <label className="check">
          <input type="checkbox" checked={uploadOnly} onChange={(e) => setUploadOnly(e.target.checked)} />
          Upload-only drop link (visitors can add files but not see or download any)
        </label>
      )}
      {error && <p className="error" role="alert">{error}</p>}
    </Dialog>
  );
}
