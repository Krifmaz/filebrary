/**
 * First-run setup: shown instead of the sign-in page while the server has
 * no users. The person picks the administrator's name and password and
 * proves they run the server with the one-time setup token from its log.
 */
import { useState } from "react";
import { ApiError, type User } from "../../api";
import { Button, Field, Switch } from "../../ui";
import { accountsApi, passwordProblem } from "./api";
import { AuthCard } from "./AuthCard";

/** Props for {@link SetupPage}. */
export interface SetupPageProps {
  /** Called with the new administrator once they are signed in. */
  onDone: (user: User) => void;
}

/** The first-run setup form. */
export function SetupPage({ onDone }: SetupPageProps) {
  const [token, setToken] = useState("");
  const [username, setUsername] = useState("admin");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [remember, setRemember] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [pwError, setPwError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    const problem = passwordProblem(password, confirm);
    setPwError(problem);
    if (problem) return;
    setBusy(true);
    setError(null);
    try {
      onDone(await accountsApi.setup(token.trim(), username.trim(), password, remember));
    } catch (err) {
      if (err instanceof ApiError && err.status === 429) setError("Too many attempts. Wait a moment and try again.");
      else if (err instanceof ApiError && err.status === 409) setError("Setup is already complete. Reload the page to sign in.");
      else setError(err instanceof Error ? err.message : "Setup failed");
    } finally {
      setBusy(false);
    }
  };

  return (
    <AuthCard
      title="Set up your server"
      lead="Create the administrator account. There is no default password: you choose it now."
      onSubmit={submit}
      footer="The setup token is printed once in the server log each time it starts while no account exists."
    >
      <Field label="Setup token" hint={<>From the server log. With Docker: <code>docker logs filebrary</code></>}>
        <input autoFocus autoComplete="off" spellCheck={false} className="mono" value={token} onChange={(e) => setToken(e.target.value)} required />
      </Field>
      <Field label="Administrator name">
        <input autoComplete="username" value={username} onChange={(e) => setUsername(e.target.value)} required maxLength={64} />
      </Field>
      <Field label="Password" hint="At least 8 characters.">
        <input type="password" autoComplete="new-password" value={password} onChange={(e) => setPassword(e.target.value)} required minLength={8} />
      </Field>
      <Field label="Confirm password" error={pwError}>
        <input type="password" autoComplete="new-password" value={confirm} onChange={(e) => setConfirm(e.target.value)} required minLength={8} />
      </Field>
      <Switch label="Keep me signed in on this device" checked={remember} onChange={(e) => setRemember(e.target.checked)} />
      {error && <p className="auth-error" role="alert">{error}</p>}
      <Button type="submit" variant="primary" size="lg" block busy={busy}>
        Create administrator
      </Button>
    </AuthCard>
  );
}
