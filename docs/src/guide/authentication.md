# Signing in

## Local passwords

Passwords are hashed with **argon2id** and must be at least 8 characters.
They are never stored or logged in clear text. After sign-in the browser
gets a session cookie that is `HttpOnly`, `SameSite=Lax`, and `Secure` when
served over HTTPS (see
[`secure_cookies`](../configuration/reference.md#secure_cookies)). Sessions
expire after [`session_ttl_hours`](../configuration/reference.md#session_ttl_hours)
of inactivity. Every request that changes something must send the CSRF
token in the `X-CSRF-Token` header, which the web UI does for you.
Disabling a user or resetting their password ends all their sessions.

Repeated failed logins, for one username or from one client address, are
refused with `429 Too Many Requests` for a while; see
[`login_max_attempts`](../configuration/reference.md#login_max_attempts) and
[`login_window_secs`](../configuration/reference.md#login_window_secs).

Accounts, groups and permission rules are described in
[Users and permissions](../configuration/users.md).

## Two-factor authentication (TOTP)

> **Planned for Phase 3.** See [Feature status](../status.md).

Users can turn on a time-based one-time password from any authenticator app
(Aegis, 2FAS, Google Authenticator, 1Password, …) and are then asked for a
6-digit code after their password.

## Single sign-on (OIDC)

> **Planned for Phase 3.** See [Feature status](../status.md).

Sign in with any OpenID Connect provider, such as Authentik, Authelia,
Keycloak, Zitadel, Google or Microsoft Entra ID. Users are created on first
sign-in. The settings will be listed in the
[settings reference](../configuration/reference.md).

## LDAP

> **Planned for Phase 3.** See [Feature status](../status.md).

Check passwords against an LDAP directory (OpenLDAP, lldap, Active
Directory) and map directory groups to Filebrary groups.

## API keys

> **Planned for Phase 3.** See [Feature status](../status.md).

Long-lived keys for scripts and other programs, each limited to a set of
scopes (for example read-only on one source). Send them in the
`Authorization: Bearer <key>` header. Keys are shown once when created and
can be revoked at any time. See the [REST API](../api.md).
