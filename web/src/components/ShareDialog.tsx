/**
 * Dialog that creates a share link for the selected files or folder, with an
 * optional password, expiry date, download limit, or upload-only mode.
 */
import { useState } from "react";
import { api, type Share } from "../api";
import { basename } from "../paths";
import { Button, Dialog, Field, Icon, Switch, toast } from "../ui";

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
        title="Your link is ready"
        description={`Anyone with this link can ${share.upload_only ? "upload into" : "open"} ${label}.`}
        onClose={onClose}
        actions={<Button variant="primary" onClick={onClose}>Done</Button>}
      >
        <div className="share-url">
          <Icon name="link" />
          <input readOnly value={share.url} onFocus={(e) => e.target.select()} aria-label="Share URL" />
          <Button
            variant={copied ? "secondary" : "primary"}
            icon={copied ? "check" : "copy"}
            onClick={async () => {
              const ok = await copyText(share.url);
              setCopied(ok);
              if (!ok) toast.error("Could not copy. Select the link and copy it by hand.");
            }}
          >
            {copied ? "Copied" : "Copy"}
          </Button>
        </div>
        <ul className="share-summary">
          {share.has_password && <li><Icon name="lock" /> Protected by a password</li>}
          <li><Icon name="info" /> {share.expires_at ? `Expires ${new Date(share.expires_at).toLocaleString()}` : "Never expires"}</li>
          {share.max_downloads && <li><Icon name="download" /> Up to {share.max_downloads} downloads</li>}
        </ul>
      </Dialog>
    );
  }

  return (
    <Dialog
      title={`Share ${label}`}
      description="Create a link anyone can open. All settings are optional."
      onClose={onClose}
      onSubmit={create}
      actions={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" type="submit" icon="link" busy={busy}>
            {busy ? "Creating…" : "Create link"}
          </Button>
        </>
      }
    >
      <Field label="Password" hint="Visitors must enter it before they can see anything.">
        <input type="password" autoComplete="new-password" value={password} onChange={(e) => setPassword(e.target.value)} placeholder="No password" />
      </Field>
      <div className="field-row">
        <Field label="Expires">
          <input type="datetime-local" value={expires} onChange={(e) => setExpires(e.target.value)} />
        </Field>
        {!uploadOnly && (
          <Field label="Download limit">
            <input type="number" min={1} value={maxDownloads} onChange={(e) => setMaxDownloads(e.target.value)} placeholder="Unlimited" />
          </Field>
        )}
      </div>
      {singleFolder && (
        <div className="field">
          <Switch checked={uploadOnly} onChange={(e) => setUploadOnly(e.target.checked)} label="Upload-only drop link" />
          <p className="field-hint">Visitors can add files but cannot see or download anything.</p>
        </div>
      )}
      {error && <p className="field-error" role="alert">{error}</p>}
    </Dialog>
  );
}
