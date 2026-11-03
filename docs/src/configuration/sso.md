# Single sign-on, 2FA, API keys and quotas

## Single sign-on (OpenID Connect)

Filebrary signs users in with any OpenID Connect provider: Authelia,
Authentik, Keycloak, Zitadel, Google, Microsoft Entra and others. It uses the
authorization code flow with PKCE and a nonce, and checks the ID token's
signature, issuer, audience and expiry.

1. Register a client at your provider with the redirect URL
   `<public_url>/api/auth/oidc/callback`, for example
   `https://files.example.com/api/auth/oidc/callback`.
2. Add an `[oidc]` section and set `public_url`:

```toml
public_url = "https://files.example.com"

[oidc]
issuer_url = "https://auth.example.com"
client_id = "filebrary"
client_secret_file = "/run/secrets/oidc_client_secret"
admin_group = "filebrary-admins"
```

The login page then shows a **Sign in with SSO** button.

How accounts are matched:

- An account is linked to the provider's issuer and subject (`sub`), never to
  a username or e-mail alone. Renaming a user at the provider does not create
  a second account.
- On the first login, Filebrary uses an account an administrator created with
  the same name and the `oidc` source, if there is one. Otherwise it creates
  one (unless `auto_create = false`).
- A local password account with the same name is never taken over: the login
  fails, and an administrator has to rename one of them.
- With `admin_group` set, administrator rights follow membership of that
  group at every login.
- When the provider sends a `groups` claim, the user joins each local group
  with the same name and leaves local groups the claim no longer lists.
  Create local groups and give them permissions to map provider groups to
  folders.
- `allowed_groups` limits who may sign in at all.
- `disable_password_login = true` turns the password form off for everyone
  except administrators, who keep it as a way in if the provider is down.

Two-factor login for SSO accounts is handled by the provider.

## LDAP

Users who are not local password accounts can sign in with their directory
password (OpenLDAP, Active Directory, FreeIPA, lldap and others). Filebrary
searches for the user with a service account, then binds as the user to check
the password. Empty passwords are always refused, because many servers treat
them as an anonymous login.

```toml
[ldap]
url = "ldaps://ldap.example.com"
bind_dn = "cn=filebrary,ou=services,dc=example,dc=com"
bind_password_file = "/run/secrets/ldap_password"
base_dn = "ou=people,dc=example,dc=com"
user_filter = "(&(objectClass=person)(uid={username}))"
admin_group = "cn=filebrary-admins,ou=groups,dc=example,dc=com"
```

For Active Directory use `user_filter = "(&(objectClass=user)(sAMAccountName={username}))"`
and `username_attribute = "sAMAccountName"`. The user's groups come from
`memberOf`; each group's `cn` is matched to local group names the same way as
for SSO. Use `ldaps://` or `starttls = true` so passwords are not sent in
clear text.

## Two-factor authentication (TOTP)

Every user with a password (local or LDAP) can turn on a second factor in
their settings. Any authenticator app works (Aegis, 2FAS, Google
Authenticator, 1Password, Bitwarden...).

1. **Settings → Two-factor → Set up** shows a QR code.
2. Enter a code from the app to switch it on. Filebrary then shows ten
   recovery codes once; keep them somewhere safe. Each works once, in place
   of a code.

Codes are 6 digits, change every 30 seconds, and each code is accepted only
once. If a user loses their device and their recovery codes, an administrator
can reset 2FA for them (`DELETE /api/users/{id}/totp`).

The secret is stored encrypted with the server key in `data_dir`, so a copy
of the database alone does not reveal it. Keep the server key backed up
together with the database.

## API keys

Scripts and tools (rclone, curl, backup jobs) use long-lived API keys instead
of a password. Create one under **Settings → API keys** or with
`POST /api/me/api-keys`:

```sh
curl -H "Authorization: Bearer fbk_..." https://files.example.com/api/sources
```

A key acts as its owner but only within its scopes:

| Scope | Allows |
|---|---|
| `files:read` | list, view, download |
| `files:write` | upload, create folders, rename, move, copy, overwrite |
| `files:delete` | delete |
| `shares` | create and manage share links |
| `admin` | administration endpoints (administrators only) |

A key can never do more than its owner can. Keys can expire
(`expires_in_days`), are shown in full only once, and are stored as hashes.
Keys cannot create other keys or change 2FA.

## Storage quotas

Each user can have a storage limit. Set a default for everyone with
`default_quota = "10G"`, and change it per user in the admin pages or with
`PUT /api/users/{id}/quota` (`{"limit": "50G"}`, a number of bytes, or `null`
for unlimited). Uploads that would go over the limit fail with `507
quota_exceeded`.

Usage is counted as files are uploaded. Every `quota_rescan_hours` (default
6) Filebrary measures the real size of each user's home folder (their scope
on each source), which accounts for deletes and for files changed outside
Filebrary. Users without a home folder are counted by uploads only. An
administrator can measure one user immediately with
`POST /api/users/{id}/quota/rescan`.
