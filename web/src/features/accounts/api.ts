/**
 * Client for the account endpoints: first-run setup, the user's own
 * settings (password, preferences, two-factor sign-in, API keys) and user
 * administration (users, roles, home folders, permission rules, quotas).
 */
import { request, setSessionCsrf, type Permissions, type User } from "../../api";

/** What kind of account a user has. */
export type Role = "admin" | "user" | "read_only";

/** Names of the six permissions, in display order. */
export const PERMS = ["view", "download", "upload", "edit", "delete", "share"] as const;

/** One permission name. */
export type Perm = (typeof PERMS)[number];

/** A user account as administrators see it. */
export interface Account {
  id: number;
  username: string;
  auth_source: string;
  role: Role;
  is_admin: boolean;
  read_only: boolean;
  disabled: boolean;
  /** Unix seconds. */
  created_at: number;
}

/** Who a permission rule applies to. */
export interface Subject {
  type: "user" | "group";
  id: number;
}

/** A permission rule: `perms` on `path` (and below) of `source`. */
export interface Rule {
  id: number;
  source: string;
  path: string;
  subject: Subject;
  perms: Perm[];
}

/** A storage quota; sizes in bytes, `limit` null for none. */
export interface Quota {
  limit: number | null;
  used: number;
  free: number | null;
}

/** Status of two-factor sign-in. */
export interface TotpStatus {
  enabled: boolean;
  recovery_codes_left: number;
}

/** An API key (the secret itself is only in {@link NewApiKey}). */
export interface ApiKey {
  id: string;
  name: string;
  scopes: string[];
  created_at: number;
  expires_at: number | null;
  last_used_at: number | null;
}

/** A just-created API key, with its secret shown once. */
export interface NewApiKey extends ApiKey {
  token: string;
}

/** Scopes an API key can carry. */
export const KEY_SCOPES = [
  { id: "files:read", label: "Read files" },
  { id: "files:write", label: "Upload and edit files" },
  { id: "files:delete", label: "Delete files" },
  { id: "shares", label: "Manage share links" },
  { id: "admin", label: "Administration (administrators only)" },
] as const;


export type { ViewMode } from "../view/viewMode";
import type { ViewMode } from "../view/viewMode";

/** The user's preferences. */
export interface Preferences {
  view_mode: ViewMode;
}

interface SessionReply {
  user: User;
  csrf_token: string;
}

const enc = encodeURIComponent;

/** Account endpoints. */
export const accountsApi = {
  /** Whether first-run setup is waiting for its administrator. */
  setupStatus: () => request<{ needed: boolean }>("GET", "/api/setup"),
  /** Creates the first administrator with the setup token and signs in. */
  setup: async (token: string, username: string, password: string, remember: boolean): Promise<User> => {
    const s = await request<SessionReply>("POST", "/api/setup", { token, username, password, remember });
    setSessionCsrf(s.csrf_token);
    return s.user;
  },

  /** Changes the signed-in user's password. Other sessions end. */
  changePassword: (current: string, next: string) =>
    request<void>("POST", "/api/me/password", { current, new: next }),
  /** The signed-in user's preferences. */
  preferences: () => request<Preferences>("GET", "/api/me/preferences"),
  /** Saves the signed-in user's preferences. */
  savePreferences: (p: Preferences) => request<Preferences>("PUT", "/api/me/preferences", p),
  /** The signed-in user's storage quota. */
  myQuota: () => request<Quota>("GET", "/api/me/quota"),

  /** Two-factor status. */
  totp: () => request<TotpStatus>("GET", "/api/me/totp"),
  /** Starts two-factor setup: a secret and an `otpauth://` URI for the app. */
  totpSetup: () => request<{ secret: string; uri: string }>("POST", "/api/me/totp/setup"),
  /** Turns two-factor on with a code from the app; returns recovery codes. */
  totpConfirm: (code: string) => request<{ recovery_codes: string[] }>("POST", "/api/me/totp/confirm", { code }),
  /** Replaces the recovery codes. */
  totpRecovery: (code: string) =>
    request<{ recovery_codes: string[] }>("POST", "/api/me/totp/recovery-codes", { code }),
  /** Turns two-factor off. */
  totpDisable: (code: string) => request<void>("POST", "/api/me/totp/disable", { code }),

  /** The signed-in user's API keys. */
  apiKeys: () => request<ApiKey[]>("GET", "/api/me/api-keys"),
  /** Creates an API key; the reply carries the secret once. */
  createApiKey: (name: string, scopes: string[], expires_in_days?: number) =>
    request<NewApiKey>("POST", "/api/me/api-keys", { name, scopes, expires_in_days }),
  /** Revokes one of the signed-in user's keys. */
  revokeApiKey: (id: string) => request<void>("DELETE", `/api/me/api-keys/${enc(id)}`),

  /** Every account (administrators). */
  users: () => request<Account[]>("GET", "/api/users"),
  /** Creates an account, optionally with a home folder. */
  createUser: (u: { username: string; password: string; role: Role; home?: { source: string; path?: string } }) =>
    request<Account>("POST", "/api/users", u),
  /** Changes an account; a `password` resets it and ends its sessions. */
  updateUser: (id: number, u: { username?: string; role?: Role; disabled?: boolean; password?: string }) =>
    request<Account>("PUT", `/api/users/${id}`, u),
  /** Deletes an account. */
  deleteUser: (id: number) => request<void>("DELETE", `/api/users/${id}`),
  /** Sets a user's home folder on a source (created if missing). */
  setHome: (id: number, source: string, path?: string) =>
    request<{ source: string; path: string }>("PUT", `/api/users/${id}/home`, { source, path }),
  /** A user's home folder (scope) per source. */
  scopes: (id: number) => request<Record<string, string>>("GET", `/api/users/${id}/scopes`),
  /** Sets (or with "/" clears) a user's scope on a source. */
  setScope: (id: number, source: string, scope: string) =>
    request<Record<string, string>>("PUT", `/api/users/${id}/scopes`, { source, scope }),
  /** Permission rules, optionally for one source. */
  rules: () => request<Rule[]>("GET", "/api/permissions"),
  /** Creates or replaces the rule for a subject on a folder. */
  setRule: (source: string, path: string, subject: Subject, perms: Perm[]) =>
    request<Rule>("PUT", "/api/permissions", { source, path, subject, perms }),
  /** Deletes a rule. */
  deleteRule: (id: number) => request<void>("DELETE", `/api/permissions/${id}`),
  /** A user's quota. */
  quota: (id: number) => request<Quota>("GET", `/api/users/${id}/quota`),
  /** Sets a user's quota: bytes, a size such as "10G", or null for none. */
  setQuota: (id: number, limit: number | string | null) =>
    request<Quota>("PUT", `/api/users/${id}/quota`, { limit }),
  /** Turns off two-factor sign-in for a user who lost their device. */
  resetTotp: (id: number) => request<void>("DELETE", `/api/users/${id}/totp`),
};

/** Human label of a role. */
export function roleLabel(r: Role): string {
  return r === "admin" ? "Administrator" : r === "read_only" ? "Read-only" : "User";
}

/** One-line description of a role. */
export function roleHint(r: Role): string {
  if (r === "admin") return "Every permission on every source; manages users and settings.";
  if (r === "read_only") return "Can view and download what their permissions allow, nothing more.";
  return "Can do what their permissions allow.";
}

/** Adds or removes `p`, keeping the implied permissions consistent: any permission implies view, and removing view removes everything. */
export function togglePerm(perms: readonly Perm[], p: Perm, on: boolean): Perm[] {
  const set = new Set(perms);
  if (on) {
    set.add(p);
    set.add("view");
  } else if (p === "view") {
    set.clear();
  } else {
    set.delete(p);
  }
  return PERMS.filter((x) => set.has(x));
}

/** The permissions summary of a source for display, from the `/api/sources` shape. */
export function permsOf(p: Permissions): Perm[] {
  return PERMS.filter((x) => p[x]);
}

/** Parses a size such as "10G", "500 MB" or "1048576" into bytes; null for empty, NaN when invalid. */
export function parseSize(s: string): number | null {
  const t = s.trim();
  if (!t) return null;
  const m = /^(\d+(?:\.\d+)?)\s*([kmgtp]?)i?b?$/i.exec(t);
  if (!m) return NaN;
  const [, num = "", unit = ""] = m;
  const pow = " kmgtp".indexOf(unit.toLowerCase() || " ");
  return Math.round(parseFloat(num) * 1024 ** pow);
}

/** A password strong enough for the server's rule (checked again there). */
export function passwordProblem(pw: string, confirm?: string): string | null {
  if (pw.length < 8) return "Use at least 8 characters.";
  if (confirm !== undefined && pw !== confirm) return "The passwords do not match.";
  return null;
}
