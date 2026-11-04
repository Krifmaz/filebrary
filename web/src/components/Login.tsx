/**
 * The sign-in page: username, password, "remember me", a TOTP code when the
 * server asks for one, and a single sign-on button when OIDC is configured.
 */
import { useEffect, useState } from "react";
import { api, ApiError, request, type User } from "../api";
import { AuthCard } from "../features/accounts/AuthCard";
import { Button, Field, Switch } from "../ui";

/** Props for {@link Login}. */
export interface LoginProps {
  onLogin: (user: User) => void;
  /** Where to go after single sign-on. */
  next?: string;
}

/** Sign-in form. Shows the TOTP field after the server reports it is required. */
export function Login({ onLogin, next = "/" }: LoginProps) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [totp, setTotp] = useState("");
  const [needTotp, setNeedTotp] = useState(false);
  const [remember, setRemember] = useState(false);
  const [error, setError] = useState<string | null>(() =>
    new URLSearchParams(location.search).get("error") === "sso_failed" ? "Single sign-on failed. Try again or use your password." : null,
  );
  const [busy, setBusy] = useState(false);
  const [sso, setSso] = useState<string | null>(null);

  useEffect(() => {
    request<{ oidc?: { label: string } | null }>("GET", "/api/auth/methods")
      .then((m) => setSso(m.oidc?.label ?? null))
      .catch(() => {});
  }, []);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      onLogin(await api.login(username, password, needTotp ? totp : undefined, remember));
    } catch (err) {
      if (err instanceof ApiError && err.code === "totp_required" && !needTotp) {
        setNeedTotp(true);
        setError(null);
      } else if (err instanceof ApiError && err.status === 429) {
        setError("Too many attempts. Wait a moment and try again.");
      } else if (err instanceof ApiError && err.status === 401) {
        setError(needTotp ? "That code did not work. Check your authenticator app." : "Wrong username or password.");
      } else {
        setError(err instanceof Error ? err.message : "Sign-in failed");
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <AuthCard title="Sign in" lead="Welcome back. Sign in to reach your files." onSubmit={submit}>
      <Field label="Username">
        <input autoFocus autoComplete="username" value={username} onChange={(e) => setUsername(e.target.value)} required />
      </Field>
      <Field label="Password">
        <input type="password" autoComplete="current-password" value={password} onChange={(e) => setPassword(e.target.value)} required />
      </Field>
      {needTotp && (
        <Field label="Authentication code" hint="The 6-digit code from your authenticator app, or a recovery code.">
          <input autoFocus inputMode="numeric" autoComplete="one-time-code" value={totp} onChange={(e) => setTotp(e.target.value)} required />
        </Field>
      )}
      <Switch label="Remember me on this device" checked={remember} onChange={(e) => setRemember(e.target.checked)} />
      {error && <p className="auth-error" role="alert">{error}</p>}
      <Button type="submit" variant="primary" size="lg" block busy={busy}>
        {busy ? "Signing in…" : "Sign in"}
      </Button>
      {sso && (
        <>
          <div className="auth-or"><span>or</span></div>
          <a className="btn lg block" href={`/api/auth/oidc/login?return_to=${encodeURIComponent(next)}`}>
            {sso}
          </a>
        </>
      )}
    </AuthCard>
  );
}
