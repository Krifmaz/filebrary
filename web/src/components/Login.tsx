/** The sign-in page: username, password, and a TOTP code when the server asks. */
import { useState } from "react";
import { api, ApiError, type User } from "../api";
import { BrandMark } from "../features/theme/Brand";

/** Props for {@link Login}. */
export interface LoginProps {
  onLogin: (user: User) => void;
}

/** Sign-in form. Shows the TOTP field after the server reports it is required. */
export function Login({ onLogin }: LoginProps) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [totp, setTotp] = useState("");
  const [needTotp, setNeedTotp] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      onLogin(await api.login(username, password, needTotp ? totp : undefined));
    } catch (err) {
      if (err instanceof ApiError && err.code === "totp_required" && !needTotp) {
        setNeedTotp(true);
        setError("Enter the code from your authenticator app.");
      } else if (err instanceof ApiError && err.status === 429) {
        setError("Too many attempts. Wait a moment and try again.");
      } else {
        setError(err instanceof Error ? err.message : "Sign-in failed");
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className="login">
      <form className="card login-card" onSubmit={submit}>
        <div className="brand big">
          <BrandMark size={40} />
        </div>
        <label>
          Username
          <input autoFocus autoComplete="username" value={username} onChange={(e) => setUsername(e.target.value)} required />
        </label>
        <label>
          Password
          <input type="password" autoComplete="current-password" value={password} onChange={(e) => setPassword(e.target.value)} required />
        </label>
        {needTotp && (
          <label>
            Authentication code
            <input inputMode="numeric" autoComplete="one-time-code" pattern="[0-9]{6,8}" value={totp} onChange={(e) => setTotp(e.target.value)} required />
          </label>
        )}
        {error && <p className="error" role="alert">{error}</p>}
        <button className="primary" disabled={busy}>
          {busy ? "Signing in…" : "Sign in"}
        </button>
      </form>
    </main>
  );
}
