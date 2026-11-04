/**
 * User administration (administrators only): a searchable list of
 * accounts, a dialog to create one (with an optional home folder) and a
 * dialog to edit one: role, disabling, password reset, home folder and
 * permissions per source, storage quota, two-factor reset and deletion.
 */
import { useMemo, useState } from "react";
import type { Source, User } from "../../api";
import { formatSize } from "../../paths";
import { Avatar, Badge, Button, Dialog, EmptyState, ErrorState, Field, PageHeader, SearchInput, SkeletonRows, Switch, toast } from "../../ui";
import {
  accountsApi,
  parseSize,
  passwordProblem,
  PERMS,
  roleHint,
  roleLabel,
  togglePerm,
  type Account,
  type Perm,
  type Role,
  type Rule,
} from "./api";
import { useAsync } from "./hooks";
import "./accounts.css";

const ROLES: Role[] = ["user", "read_only", "admin"];

/** Props for {@link UsersPage}. */
export interface UsersPageProps {
  /** The signed-in administrator. */
  me: User;
  /** Every source (administrators see all). */
  sources: Source[] | null;
}

/** The Users page. */
export function UsersPage({ me, sources }: UsersPageProps) {
  const users = useAsync(() => accountsApi.users(), []);
  const rules = useAsync(() => accountsApi.rules(), []);
  const [q, setQ] = useState("");
  const [creating, setCreating] = useState(false);
  const [editing, setEditing] = useState<Account | null>(null);

  const list = useMemo(() => {
    const t = q.trim().toLowerCase();
    return (users.data ?? []).filter((u) => !t || u.username.toLowerCase().includes(t) || roleLabel(u.role).toLowerCase().includes(t));
  }, [users.data, q]);

  const reload = () => {
    users.reload();
    rules.reload();
  };

  return (
    <section className="acct">
      <PageHeader
        title="Users"
        description="Create accounts, choose what each person can reach, and reset passwords."
        actions={
          <Button variant="primary" icon="plus" onClick={() => setCreating(true)}>
            New user
          </Button>
        }
      />
      <div className="acct-toolbar">
        <SearchInput placeholder="Find a user" aria-label="Find a user" value={q} onChange={(e) => setQ(e.target.value)} onClear={() => setQ("")} />
        {users.data && <span className="faint small">{users.data.length} {users.data.length === 1 ? "account" : "accounts"}</span>}
      </div>
      {users.loading && !users.data && <div className="card"><SkeletonRows rows={4} /></div>}
      {users.error && <ErrorState error={users.error} onRetry={users.reload} />}
      {users.data && list.length === 0 && (
        <EmptyState icon="user" title={q ? "No matching users" : "No users yet"}>
          {q ? "Try another name." : "Create the first account with New user."}
        </EmptyState>
      )}
      {list.length > 0 && (
        <ul className="card user-list" aria-label="Users">
          {list.map((u) => (
            <li key={u.id}>
              <button type="button" className="user-row" onClick={() => setEditing(u)} aria-label={`Edit ${u.username}`}>
                <Avatar name={u.username} size={36} />
                <span className="user-main">
                  <span className="user-title">
                    <strong>{u.username}</strong>
                    {u.id === me.id && <Badge tone="accent">You</Badge>}
                    {u.disabled && <Badge tone="danger">Disabled</Badge>}
                    {u.auth_source !== "local" && <Badge tone="info">{u.auth_source.toUpperCase()}</Badge>}
                  </span>
                  <span className="faint small">{accessSummary(u, rules.data, sources)}</span>
                </span>
                <span className="user-role">
                  <Badge tone={u.role === "admin" ? "accent" : u.role === "read_only" ? "info" : "neutral"}>{roleLabel(u.role)}</Badge>
                </span>
                <span className="user-date faint small hide-phone">Since {new Date(u.created_at * 1000).toLocaleDateString()}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
      {creating && (
        <NewUserDialog
          sources={sources ?? []}
          onClose={() => setCreating(false)}
          onCreated={(u) => {
            setCreating(false);
            reload();
            setEditing(u);
            toast.success(`Created ${u.username}.`);
          }}
        />
      )}
      {editing && (
        <EditUserDialog
          key={editing.id}
          account={editing}
          me={me}
          sources={sources ?? []}
          rules={rules.data ?? []}
          onClose={() => setEditing(null)}
          onChanged={(u) => {
            reload();
            if (u) setEditing(u);
          }}
          onDeleted={() => {
            setEditing(null);
            reload();
          }}
        />
      )}
    </section>
  );
}

/** "Everything", or the sources a user has rules on. */
function accessSummary(u: Account, rules: Rule[] | undefined, sources: Source[] | null): string {
  if (u.role === "admin") return "Every source";
  if (!rules) return "";
  const names = [...new Set(rules.filter((r) => r.subject.type === "user" && r.subject.id === u.id && r.perms.length > 0).map((r) => r.source))];
  if (names.length === 0) return "No access yet (or only through groups)";
  const label = (id: string) => sources?.find((s) => s.id === id)?.name ?? id;
  return names.map(label).join(", ");
}

/** A random, readable password for a new account or a reset. */
export function generatePassword(len = 16): string {
  const alphabet = "abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
  const bytes = new Uint32Array(len);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => alphabet[b % alphabet.length]).join("");
}

function RoleSelect({ value, onChange, disabled }: { value: Role; onChange: (r: Role) => void; disabled?: boolean }) {
  return (
    <Field label="Role" hint={roleHint(value)}>
      <select value={value} onChange={(e) => onChange(e.target.value as Role)} disabled={disabled}>
        {ROLES.map((r) => (
          <option key={r} value={r}>{roleLabel(r)}</option>
        ))}
      </select>
    </Field>
  );
}

function NewUserDialog({ sources, onClose, onCreated }: { sources: Source[]; onClose: () => void; onCreated: (u: Account) => void }) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [role, setRole] = useState<Role>("user");
  const [home, setHome] = useState(sources.length > 0);
  const [homeSource, setHomeSource] = useState(sources[0]?.id ?? "");
  const [homePath, setHomePath] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    const p = passwordProblem(password);
    if (p) {
      setError(p);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const u = await accountsApi.createUser({
        username: username.trim(),
        password,
        role,
        home: home && role !== "admin" && homeSource ? { source: homeSource, path: homePath.trim() || undefined } : undefined,
      });
      onCreated(u);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      title="New user"
      description="They can change the password after signing in."
      onClose={onClose}
      onSubmit={submit}
      actions={
        <>
          <Button variant="ghost" onClick={onClose}>Cancel</Button>
          <Button type="submit" variant="primary" busy={busy} disabled={!username.trim() || !password}>Create user</Button>
        </>
      }
    >
      <Field label="Username" hint="Letters, digits, '.', '_', '-' or '@'.">
        <input autoFocus autoComplete="off" value={username} onChange={(e) => setUsername(e.target.value)} required maxLength={64} />
      </Field>
      <Field label="Password">
        <span className="acct-pw">
          <input type="text" className="mono" autoComplete="new-password" value={password} onChange={(e) => setPassword(e.target.value)} required minLength={8} />
          <Button icon="refresh" onClick={() => setPassword(generatePassword())}>Generate</Button>
        </span>
      </Field>
      <RoleSelect value={role} onChange={setRole} />
      {role !== "admin" && sources.length > 0 && (
        <div className="acct-home">
          <Switch label="Give them a home folder" checked={home} onChange={(e) => setHome(e.target.checked)} />
          {home && (
            <div className="acct-grid">
              <Field label="On source">
                <select value={homeSource} onChange={(e) => setHomeSource(e.target.value)}>
                  {sources.map((s) => <option key={s.id} value={s.id}>{s.name}</option>)}
                </select>
              </Field>
              <Field label="Folder" hint="Created if missing. They see it as the top of the source.">
                <input value={homePath} onChange={(e) => setHomePath(e.target.value)} placeholder={`/home/${username.trim() || "name"}`} />
              </Field>
            </div>
          )}
        </div>
      )}
      {error && <p className="error small" role="alert">{error}</p>}
    </Dialog>
  );
}

interface EditProps {
  account: Account;
  me: User;
  sources: Source[];
  rules: Rule[];
  onClose: () => void;
  /** After a change; with the updated account when there is one. */
  onChanged: (u?: Account) => void;
  onDeleted: () => void;
}

function EditUserDialog({ account, me, sources, rules, onClose, onChanged, onDeleted }: EditProps) {
  const self = account.id === me.id;
  const [role, setRole] = useState<Role>(account.role);
  const [disabled, setDisabled] = useState(account.disabled);
  const [busy, setBusy] = useState(false);

  const saveAccount = async () => {
    setBusy(true);
    try {
      const u = await accountsApi.updateUser(account.id, { role, disabled });
      toast.success("Saved.");
      onChanged(u);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (!confirm(`Delete ${account.username}? Their sessions, keys, rules and shares go too. Their files stay where they are.`)) return;
    try {
      await accountsApi.deleteUser(account.id);
      toast(`Deleted ${account.username}.`);
      onDeleted();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : String(err));
    }
  };

  const dirty = role !== account.role || disabled !== account.disabled;
  return (
    <Dialog
      wide
      title={account.username}
      description={`${roleLabel(account.role)} · ${account.auth_source === "local" ? "password sign-in" : `signs in with ${account.auth_source.toUpperCase()}`}`}
      onClose={onClose}
      actions={<Button onClick={onClose}>Done</Button>}
    >
      <section className="acct-section" aria-labelledby="sec-account">
        <h3 id="sec-account" className="acct-h3">Account</h3>
        <RoleSelect value={role} onChange={setRole} disabled={self} />
        <Switch label="Account disabled" checked={disabled} disabled={self} onChange={(e) => setDisabled(e.target.checked)} />
        <p className="faint small">{self ? "You cannot change your own role or disable yourself." : "A disabled account cannot sign in, and its current sessions end."}</p>
        <div className="acct-actions">
          <Button variant="primary" disabled={!dirty} busy={busy} onClick={saveAccount}>Save</Button>
        </div>
      </section>

      {account.auth_source === "local" && <ResetPassword account={account} />}

      {account.role !== "admin" && (
        <section className="acct-section" aria-labelledby="sec-access">
          <h3 id="sec-access" className="acct-h3">Access</h3>
          <p className="muted small">
            A home folder confines them to one folder of a source, which they see as its top. Permissions apply to that folder and everything inside.
            {account.role === "read_only" && " As a read-only user they can only view and download, whatever is ticked."}
          </p>
          {sources.length === 0 && <p className="faint small">There are no sources.</p>}
          {sources.map((s) => (
            <SourceAccess key={s.id} account={account} source={s} rules={rules} onChanged={() => onChanged()} />
          ))}
        </section>
      )}

      <QuotaSection account={account} />

      <section className="acct-section danger-zone" aria-labelledby="sec-danger">
        <h3 id="sec-danger" className="acct-h3">Danger zone</h3>
        <div className="btn-group">
          <Button
            onClick={async () => {
              if (!confirm(`Turn off two-factor sign-in for ${account.username}? Use this when they lost their device.`)) return;
              try {
                await accountsApi.resetTotp(account.id);
                toast.success("Two-factor sign-in reset.");
              } catch (err) {
                toast.error(err instanceof Error ? err.message : String(err));
              }
            }}
          >
            Reset two-factor
          </Button>
          <Button variant="danger" icon="trash" disabled={self} onClick={remove}>Delete user</Button>
        </div>
      </section>
    </Dialog>
  );
}

function ResetPassword({ account }: { account: Account }) {
  const [pw, setPw] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const reset = async () => {
    const p = passwordProblem(pw);
    setError(p);
    if (p) return;
    setBusy(true);
    try {
      await accountsApi.updateUser(account.id, { password: pw });
      toast.success(`Password reset. ${account.username} was signed out everywhere.`);
      setPw("");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="acct-section" aria-labelledby="sec-pw">
      <h3 id="sec-pw" className="acct-h3">Reset password</h3>
      <Field label="New password" error={error} hint="Give it to them privately; they can change it in Settings.">
        <span className="acct-pw">
          <input type="text" className="mono" autoComplete="new-password" value={pw} onChange={(e) => setPw(e.target.value)} />
          <Button icon="refresh" onClick={() => setPw(generatePassword())}>Generate</Button>
          <Button variant="primary" busy={busy} disabled={!pw} onClick={reset}>Reset</Button>
        </span>
      </Field>
    </section>
  );
}

function SourceAccess({ account, source, rules, onChanged }: { account: Account; source: Source; rules: Rule[]; onChanged: () => void }) {
  const scopes = useAsync(() => accountsApi.scopes(account.id), [account.id]);
  const scope = scopes.data?.[source.id] ?? "/";
  const own = rules.filter((r) => r.source === source.id && r.subject.type === "user" && r.subject.id === account.id);
  const atScope = own.find((r) => r.path === scope);
  const [perms, setPerms] = useState<Perm[] | null>(null);
  const [home, setHome] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const shownPerms = perms ?? atScope?.perms ?? [];
  const shownHome = home ?? (scope === "/" ? "" : scope);

  const save = async () => {
    setBusy(true);
    try {
      const target = shownHome.trim() || "/";
      let path = scope;
      if (target !== scope) {
        if (target === "/") {
          await accountsApi.setScope(account.id, source.id, "/");
          path = "/";
        } else {
          // Creates the folder and grants every permission there.
          path = (await accountsApi.setHome(account.id, source.id, target)).path;
        }
      }
      // A new home folder keeps its full grant unless the boxes were changed.
      if (perms !== null || target === scope) {
        if (shownPerms.length > 0) {
          await accountsApi.setRule(source.id, path, { type: "user", id: account.id }, shownPerms);
        } else {
          const fresh = await accountsApi.rules();
          const rule = fresh.find((r) => r.source === source.id && r.subject.type === "user" && r.subject.id === account.id && r.path === path);
          if (rule) await accountsApi.deleteRule(rule.id);
        }
      }
      setPerms(null);
      setHome(null);
      scopes.reload();
      onChanged();
      toast.success(`Saved access to ${source.name}.`);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const others = own.filter((r) => r.path !== scope);
  const dirty = perms !== null || home !== null;
  const capped = account.role === "read_only";
  return (
    <div className="acct-source">
      <div className="acct-source-head">
        <strong>{source.name}</strong>
        {shownPerms.length === 0 ? <Badge>No access</Badge> : <Badge tone="ok">{shownPerms.length === PERMS.length ? "Full access" : "Some access"}</Badge>}
      </div>
      <Field label="Home folder" hint="Empty: the whole source.">
        <input value={shownHome} onChange={(e) => setHome(e.target.value)} placeholder={`/home/${account.username}`} />
      </Field>
      <div className="acct-perms" role="group" aria-label={`Permissions on ${source.name}`}>
        {PERMS.map((p) => (
          <label key={p} className={`acct-perm${shownPerms.includes(p) ? " on" : ""}${capped && p !== "view" && p !== "download" ? " capped" : ""}`}>
            <input type="checkbox" checked={shownPerms.includes(p)} onChange={(e) => setPerms(togglePerm(shownPerms, p, e.target.checked))} />
            {p}
          </label>
        ))}
      </div>
      {others.length > 0 && (
        <ul className="acct-rules">
          {others.map((r) => (
            <li key={r.id}>
              <code>{r.path}</code>
              <span className="faint small">{r.perms.length ? r.perms.join(", ") : "no access"}</span>
              <Button
                size="sm"
                variant="ghost"
                onClick={async () => {
                  try {
                    await accountsApi.deleteRule(r.id);
                    onChanged();
                  } catch (err) {
                    toast.error(err instanceof Error ? err.message : String(err));
                  }
                }}
              >
                Remove
              </Button>
            </li>
          ))}
        </ul>
      )}
      <div className="acct-actions">
        <Button size="sm" variant="primary" disabled={!dirty} busy={busy} onClick={save}>Save {source.name}</Button>
      </div>
    </div>
  );
}

function QuotaSection({ account }: { account: Account }) {
  const quota = useAsync(() => accountsApi.quota(account.id), [account.id]);
  const [limit, setLimit] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const shown = limit ?? (quota.data?.limit != null ? formatSize(quota.data.limit).replace(/\s/g, "") : "");
  const save = async () => {
    const bytes = parseSize(shown);
    if (Number.isNaN(bytes)) {
      setError("Use a size such as 500M, 10G or 1T, or leave it empty for no limit.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await accountsApi.setQuota(account.id, bytes);
      setLimit(null);
      quota.reload();
      toast.success("Quota saved.");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="acct-section" aria-labelledby="sec-quota">
      <h3 id="sec-quota" className="acct-h3">Storage quota</h3>
      {quota.data && <p className="muted small">Using {formatSize(quota.data.used)}{quota.data.limit != null ? ` of ${formatSize(quota.data.limit)}` : ", no limit"}.</p>}
      <Field label="Limit" error={error} hint="For example 50G. Empty means no limit.">
        <span className="acct-pw">
          <input value={shown} onChange={(e) => setLimit(e.target.value)} placeholder="no limit" />
          <Button variant="primary" busy={busy} disabled={limit === null} onClick={save}>Save</Button>
        </span>
      </Field>
    </section>
  );
}
