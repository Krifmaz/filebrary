/**
 * The user's own settings, in three tabs:
 *
 * * Account: who they are, their storage use, and changing the password.
 * * Security: two-factor sign-in and API keys.
 * * Preferences: theme and the default view mode for folders.
 *
 * Custom colours and branding stay on the Appearance page.
 */
import { useState } from "react";
import type { User } from "../../api";
import { formatSize } from "../../paths";
import { navigate } from "../../router";
import { setTheme, useTheme, type Theme } from "../../theme";
import { Avatar, Badge, Button, Card, EmptyState, ErrorState, Field, Icon, PageHeader, Skeleton, Switch, toast, type IconName } from "../../ui";
import { setDefaultViewMode, VIEW_INFO, VIEW_MODES, type ViewMode } from "../view/viewMode";
import { accountsApi, KEY_SCOPES, passwordProblem, roleLabel, type ApiKey, type NewApiKey, type Quota, type Role } from "./api";
import { useAsync } from "./hooks";
import "./accounts.css";

/** Tabs of the settings page, as they appear in the URL. */
export type SettingsTab = "account" | "security" | "preferences";

/** Props for {@link SettingsPage}. */
export interface SettingsPageProps {
  user: User;
  tab: SettingsTab;
}

const TABS: { id: SettingsTab; label: string; icon: IconName }[] = [
  { id: "account", label: "Account", icon: "user" },
  { id: "security", label: "Security", icon: "lock" },
  { id: "preferences", label: "Preferences", icon: "settings" },
];

/** The settings page. */
export function SettingsPage({ user, tab }: SettingsPageProps) {
  return (
    <section className="acct">
      <PageHeader title="Settings" description="Your account, sign-in security and how Filebrary looks for you." />
      <div className="segmented acct-tabs" role="tablist" aria-label="Settings sections">
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            aria-selected={tab === t.id}
            className={tab === t.id ? "active" : undefined}
            onClick={() => navigate(`/settings/${t.id}`)}
          >
            <Icon name={t.icon} />
            {t.label}
          </button>
        ))}
      </div>
      <div role="tabpanel" className="acct-panel">
        {tab === "account" && <AccountTab user={user} />}
        {tab === "security" && <SecurityTab user={user} />}
        {tab === "preferences" && <PreferencesTab user={user} />}
      </div>
    </section>
  );
}

function AccountTab({ user }: { user: User }) {
  const quota = useAsync<Quota>(() => accountsApi.myQuota(), []);
  const role: Role = user.role ?? (user.is_admin ? "admin" : "user");
  return (
    <>
      <Card className="acct-who">
        <Avatar name={user.username} size={56} />
        <div className="grow">
          <div className="acct-name">{user.username}</div>
          <Badge tone={role === "admin" ? "accent" : role === "read_only" ? "info" : "neutral"}>{roleLabel(role)}</Badge>
        </div>
        <div className="acct-who-quota">{quota.loading ? <Skeleton width="100%" height={10} /> : quota.data && <QuotaBar q={quota.data} />}</div>
      </Card>
      <PasswordCard />
    </>
  );
}

/** A bar showing storage use against the quota. */
export function QuotaBar({ q }: { q: Quota }) {
  if (q.limit == null) {
    return <p className="muted small acct-quota-text">{formatSize(q.used)} used · no storage limit</p>;
  }
  const pct = q.limit > 0 ? Math.min(100, Math.round((q.used / q.limit) * 100)) : 100;
  return (
    <div className="acct-quota">
      <div className="row small">
        <span className="grow">Storage</span>
        <span className="muted">
          {formatSize(q.used)} of {formatSize(q.limit)}
        </span>
      </div>
      <div className={`meter${pct >= 90 ? " warn" : ""}`} role="meter" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct} aria-label="Storage used">
        <span style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}

function PasswordCard() {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    const p = passwordProblem(next, confirm);
    setProblem(p);
    setError(null);
    if (p) return;
    setBusy(true);
    try {
      await accountsApi.changePassword(current, next);
      setCurrent("");
      setNext("");
      setConfirm("");
      toast.success("Password changed. Your other devices were signed out.");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card>
      <form onSubmit={submit} aria-labelledby="pw-title">
        <h2 id="pw-title" className="acct-h2">Change password</h2>
        <p className="muted acct-sub">Changing it signs you out everywhere else.</p>
        <div className="acct-grid">
          <div className="span-2">
            <Field label="Current password" error={error}>
              <input type="password" autoComplete="current-password" value={current} onChange={(e) => setCurrent(e.target.value)} required />
            </Field>
          </div>
          <Field label="New password" hint="At least 8 characters. A long phrase is easy to remember and hard to guess.">
            <input type="password" autoComplete="new-password" value={next} onChange={(e) => setNext(e.target.value)} required minLength={8} />
          </Field>
          <Field label="Confirm new password" error={problem}>
            <input type="password" autoComplete="new-password" value={confirm} onChange={(e) => setConfirm(e.target.value)} required minLength={8} />
          </Field>
        </div>
        <div className="acct-actions">
          <Button type="submit" variant="primary" busy={busy}>
            Change password
          </Button>
        </div>
      </form>
    </Card>
  );
}

function SecurityTab({ user }: { user: User }) {
  return (
    <>
      <TotpCard />
      <ApiKeysCard isAdmin={user.is_admin} />
    </>
  );
}

function TotpCard() {
  const status = useAsync(() => accountsApi.totp(), []);
  const [pending, setPending] = useState<{ secret: string; uri: string } | null>(null);
  const [codes, setCodes] = useState<string[] | null>(null);
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await fn();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const enabled = status.data?.enabled;
  return (
    <Card aria-labelledby="totp-title">
      <div className="acct-card-head">
        <span className="acct-icon"><Icon name="lock" /></span>
        <div className="grow">
          <h2 id="totp-title" className="acct-h2">Two-factor sign-in</h2>
          <p className="muted acct-sub">Ask for a code from an authenticator app (such as Aegis, 2FAS or 1Password) after your password.</p>
        </div>
        {status.data && <Badge tone={enabled ? "ok" : "neutral"}>{enabled ? "On" : "Off"}</Badge>}
      </div>
      {status.loading && !status.data && <Skeleton width="40%" height={14} />}
      {status.error && <ErrorState compact error={status.error} onRetry={status.reload} />}

      {codes && (
        <div className="acct-codes">
          <p>
            <strong>Your recovery codes.</strong> Each one works once if you lose your device. Save them somewhere safe now: they are not shown again.
          </p>
          <ul className="mono">{codes.map((c) => <li key={c}>{c}</li>)}</ul>
          <Button size="sm" icon="copy" onClick={() => void navigator.clipboard?.writeText(codes.join("\n")).then(() => toast.success("Codes copied"))}>
            Copy codes
          </Button>
        </div>
      )}

      {status.data && !enabled && !pending && (
        <div className="acct-actions start">
          <Button variant="primary" busy={busy} onClick={() => run(async () => setPending(await accountsApi.totpSetup()))}>
            Set up two-factor sign-in
          </Button>
        </div>
      )}

      {pending && (
        <form
          className="acct-steps"
          onSubmit={(e) => {
            e.preventDefault();
            void run(async () => {
              const r = await accountsApi.totpConfirm(code.trim());
              setCodes(r.recovery_codes);
              setPending(null);
              setCode("");
              status.reload();
              toast.success("Two-factor sign-in is on.");
            });
          }}
        >
          <ol>
            <li>
              <p>
                In your authenticator app, add an account with this key, or <a href={pending.uri}>open it in the app</a>.
              </p>
              <code className="acct-secret">{pending.secret.replace(/(.{4})/g, "$1 ").trim()}</code>
            </li>
            <li>
              <Field label="Enter the 6-digit code it shows" error={error}>
                <input inputMode="numeric" autoComplete="one-time-code" pattern="[0-9]{6,8}" value={code} onChange={(e) => setCode(e.target.value)} required />
              </Field>
            </li>
          </ol>
          <div className="acct-actions">
            <Button variant="ghost" onClick={() => setPending(null)}>Cancel</Button>
            <Button type="submit" variant="primary" busy={busy}>Turn on</Button>
          </div>
        </form>
      )}

      {enabled && (
        <form className="acct-inline" onSubmit={(e) => e.preventDefault()}>
          <Field label="Current code" hint={status.data ? `${status.data.recovery_codes_left} recovery codes left` : undefined} error={error}>
            <input inputMode="numeric" autoComplete="one-time-code" value={code} onChange={(e) => setCode(e.target.value)} placeholder="123456" />
          </Field>
          <div className="btn-group">
            <Button
              disabled={!code}
              busy={busy}
              onClick={() =>
                run(async () => {
                  setCodes((await accountsApi.totpRecovery(code.trim())).recovery_codes);
                  setCode("");
                  status.reload();
                })
              }
            >
              New recovery codes
            </Button>
            <Button
              variant="danger"
              disabled={!code || busy}
              onClick={() =>
                run(async () => {
                  await accountsApi.totpDisable(code.trim());
                  setCode("");
                  setCodes(null);
                  status.reload();
                  toast("Two-factor sign-in is off.");
                })
              }
            >
              Turn off
            </Button>
          </div>
        </form>
      )}
    </Card>
  );
}

function ApiKeysCard({ isAdmin }: { isAdmin: boolean }) {
  const keys = useAsync<ApiKey[]>(() => accountsApi.apiKeys(), []);
  const [name, setName] = useState("");
  const [scopes, setScopes] = useState<string[]>(["files:read"]);
  const [days, setDays] = useState("");
  const [created, setCreated] = useState<NewApiKey | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const create = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    setBusy(true);
    try {
      const d = days.trim() ? Number(days) : undefined;
      setCreated(await accountsApi.createApiKey(name.trim(), scopes, d));
      setName("");
      setDays("");
      keys.reload();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const revoke = async (k: ApiKey) => {
    if (!confirm(`Revoke the key “${k.name}”? Anything using it stops working.`)) return;
    try {
      await accountsApi.revokeApiKey(k.id);
      keys.reload();
      toast("Key revoked.");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : String(err));
    }
  };

  const date = (s: number | null) => (s ? new Date(s * 1000).toLocaleDateString() : "never");
  return (
    <Card aria-labelledby="keys-title">
      <div className="acct-card-head">
        <span className="acct-icon"><Icon name="key" /></span>
        <div className="grow">
          <h2 id="keys-title" className="acct-h2">API keys</h2>
          <p className="muted acct-sub">
            For scripts and apps such as rclone. Send as <code>Authorization: Bearer …</code>
          </p>
        </div>
      </div>
      {created && (
        <div className="acct-codes" role="status">
          <p><strong>Copy your new key now.</strong> It is not shown again.</p>
          <code className="acct-secret">{created.token}</code>
          <Button size="sm" icon="copy" onClick={() => void navigator.clipboard?.writeText(created.token).then(() => toast.success("Key copied"))}>
            Copy key
          </Button>
        </div>
      )}
      {keys.loading && !keys.data && <Skeleton width="60%" height={14} />}
      {keys.error && <ErrorState compact error={keys.error} onRetry={keys.reload} />}
      {keys.data && keys.data.length === 0 && (
        <EmptyState compact icon="key" title="No API keys yet">Create one below for a script or app.</EmptyState>
      )}
      {keys.data && keys.data.length > 0 && (
        <ul className="acct-list">
          {keys.data.map((k) => (
            <li key={k.id}>
              <div className="grow">
                <strong>{k.name}</strong>
                <div className="acct-chips">{k.scopes.map((s) => <Badge key={s}>{s}</Badge>)}</div>
                <span className="faint small">
                  Created {date(k.created_at)} · last used {date(k.last_used_at)} · expires {date(k.expires_at)}
                </span>
              </div>
              <Button size="sm" variant="danger" onClick={() => revoke(k)}>Revoke</Button>
            </li>
          ))}
        </ul>
      )}
      <form className="acct-new" onSubmit={create}>
        <h3 className="acct-h3">New key</h3>
        <div className="acct-grid">
          <Field label="Name">
            <input value={name} onChange={(e) => setName(e.target.value)} required maxLength={64} placeholder="rclone on my laptop" />
          </Field>
          <Field label="Expires after (days)" hint="Leave empty for a key that does not expire.">
            <input inputMode="numeric" pattern="[0-9]*" value={days} onChange={(e) => setDays(e.target.value)} placeholder="never" />
          </Field>
        </div>
        <fieldset className="acct-checks">
          <legend>Allowed</legend>
          {KEY_SCOPES.filter((s) => isAdmin || s.id !== "admin").map((s) => (
            <Switch
              key={s.id}
              label={s.label}
              checked={scopes.includes(s.id)}
              onChange={(e) => setScopes((cur) => (e.target.checked ? [...cur, s.id] : cur.filter((x) => x !== s.id)))}
            />
          ))}
        </fieldset>
        {error && <p className="error small" role="alert">{error}</p>}
        <div className="acct-actions">
          <Button type="submit" variant="primary" icon="plus" busy={busy} disabled={!name.trim() || scopes.length === 0}>
            Create key
          </Button>
        </div>
      </form>
    </Card>
  );
}

const THEME_CHOICES: { id: Theme; label: string; hint: string; icon: IconName }[] = [
  { id: "system", label: "Automatic", hint: "Follow this device", icon: "monitor" },
  { id: "light", label: "Light", hint: "Cool neutrals", icon: "sun" },
  { id: "dark", label: "Dark", hint: "Charcoal and cream", icon: "moon" },
];

function PreferencesTab({ user }: { user: User }) {
  const [theme] = useTheme();
  const prefs = useAsync(() => accountsApi.preferences(), []);
  const [mode, setMode] = useState<ViewMode | null>(null);
  const shown = mode ?? prefs.data?.view_mode ?? null;

  const saveMode = async (m: ViewMode) => {
    const before = shown;
    setMode(m);
    setDefaultViewMode(user.id, m);
    try {
      await accountsApi.savePreferences({ view_mode: m });
      toast.success(`Folders now open as ${VIEW_INFO[m].label.toLowerCase()}.`);
    } catch (err) {
      setMode(before);
      toast.error(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <>
      <Card>
        <fieldset className="acct-fieldset">
          <legend className="acct-h2">Theme</legend>
          <div className="acct-choices" role="radiogroup">
            {THEME_CHOICES.map((t) => (
              <label key={t.id} className={`acct-choice${theme === t.id ? " on" : ""}`}>
                <input type="radio" name="theme" className="sr-only" checked={theme === t.id} onChange={() => setTheme(t.id)} />
                <span className={`acct-preview theme-${t.id}`} aria-hidden><Icon name={t.icon} /></span>
                <strong>{t.label}</strong>
                <span className="faint small">{t.hint}</span>
              </label>
            ))}
          </div>
          <p className="muted small">
            Custom colours are on the{" "}
            <a href="/settings/appearance" onClick={(e) => { e.preventDefault(); navigate("/settings/appearance"); }}>
              Appearance
            </a>{" "}
            page.
          </p>
        </fieldset>
      </Card>
      <Card>
        <fieldset className="acct-fieldset">
          <legend className="acct-h2">Default view</legend>
          <p className="muted small acct-sub">How folders open. You can still switch per folder; folders you changed keep their own view.</p>
          {prefs.loading && !shown && <Skeleton width="100%" height={72} radius="var(--radius-lg)" />}
          {prefs.error && <ErrorState compact error={prefs.error} onRetry={prefs.reload} />}
          {shown && (
            <div className="acct-choices five" role="radiogroup">
              {VIEW_MODES.map((m) => (
                <label key={m} className={`acct-choice${shown === m ? " on" : ""}`}>
                  <input type="radio" name="view" className="sr-only" checked={shown === m} onChange={() => void saveMode(m)} />
                  <span className="acct-preview" aria-hidden><Icon name={m === "details" ? "list" : m === "compact" ? "rows" : m === "small" ? "gridSmall" : m === "medium" ? "grid" : "square"} /></span>
                  <strong>{VIEW_INFO[m].label}</strong>
                </label>
              ))}
            </div>
          )}
        </fieldset>
      </Card>
    </>
  );
}
