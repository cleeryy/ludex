# SSO with OIDC (Authentik, Keycloak, Authelia)

Ludex can use an external OpenID Connect provider for web login. Local accounts
remain available, and the first-run setup still creates a local administrator.
SSO is web-only: the desktop app keeps using the server password.

Since v0.3 the provider can be configured entirely from **Admin →
Authentication**, without editing environment files or restarting the server.

## Configure from the admin UI

1. Sign in as an administrator and open **Admin → Authentication**.
2. Pick a preset (Authentik, Keycloak, Authelia or Custom).
3. Fill in the issuer URL and client ID.
4. For a confidential client, use **Replace secret** and paste the secret. It is
   sent once, encrypted at rest and never returned by any endpoint.
5. Leave **Redirect URI** empty to use the default callback, or set a custom one.
   The read-only line under the field shows the exact URI to register at the
   provider; use the copy button.
6. Set the public base URL when the browser reaches Ludex through a reverse
   proxy (`https://ludex.example.com`).
7. Click **Test discovery** to check the provider, then **Save & apply**.

The login page shows **Sign in with `<button label>`** as soon as the settings
are saved; no restart is needed. Disabling SSO hides the button and leaves
password sign-in working.

The redirect URI registered at the provider is normally:

```
{LUDEX_OIDC_PUBLIC_BASE_URL}/api/auth/oidc/callback
```

For the CT258 test deployment:

```
https://ludex.home.cleeryy.cloud/api/auth/oidc/callback
```

### Provider presets

| Preset | Scopes | Groups claim |
|---|---|---|
| Authentik | `openid profile email` | `groups` |
| Keycloak | `openid profile email` | `realm_access.roles` |
| Authelia | `openid profile email` | `groups` |
| Custom | `openid profile email` | `groups` |

Presets only fill defaults; every field stays editable.

## Environment variables (config as code)

`LUDEX_OIDC_*` variables stay supported and **win per field** over the values
saved from the UI. This keeps GitOps / container deployments working exactly as
before. Fields that come from the environment are shown locked in the admin
page, with an explanation.

| Variable | Required | Example / default |
|---|---|---|
| `LUDEX_OIDC_ISSUER` | yes | `https://auth.example.com/application/o/ludex/` |
| `LUDEX_OIDC_CLIENT_ID` | yes | client id from the provider |
| `LUDEX_OIDC_CLIENT_SECRET` | usually | client secret (omit for a public PKCE client) |
| `LUDEX_OIDC_REDIRECT_URL` | no | `/api/auth/oidc/callback` (relative is resolved against the public base) |
| `LUDEX_OIDC_PUBLIC_BASE_URL` | recommended | `https://ludex.example.com` |
| `LUDEX_OIDC_SCOPES` | no | `openid profile email` |
| `LUDEX_OIDC_DISPLAY_NAME` | no | `Authentik` (button label) |
| `LUDEX_OIDC_AUTO_CREATE` | no | `true`; create local accounts on first SSO login |
| `LUDEX_OIDC_DEFAULT_ROLE` | no | `user` (`admin`, `editor`, `user`) |
| `LUDEX_OIDC_GROUPS_CLAIM` | no | `groups` (dot paths allowed, e.g. `realm_access.roles`) |
| `LUDEX_OIDC_ADMIN_GROUPS` | no | comma-separated group/role values that grant `admin` |

Setting `LUDEX_OIDC_ISSUER` pins `enabled` too: SSO is on regardless of the
value saved in the UI. Unset the variable and restart to manage that field from
the UI again.

**Reset to environment defaults** in the admin page deletes the saved row and
falls back to the environment only. With no `LUDEX_OIDC_*` variables, SSO is
disabled.

## Secret storage and rotation

- The client secret is encrypted at rest with AES-256-GCM. The key is derived
  from the server secret (`LUDEX_JWT_SECRET`, or the generated
  `{data_dir}/jwt.secret`) with HKDF-SHA256.
- The database stores `v1.<base64url(nonce||ciphertext)>`; the plaintext is
  never written to disk, returned by the API, logged, or copied into audit
  metadata.
- Saving the same secret twice produces different ciphertext (fresh nonce).
- **Rotating the server secret requires re-entering the OIDC client secret.**
  After a rotation the stored value cannot be decrypted; the server starts with
  SSO disabled, reports the problem on the Authentication page, and asks for the
  secret again. Use **Replace secret** (or **Clear** for a public client).
- On a fresh install, `jwt.secret` is generated once under the data directory;
  keep it stable (backups include it).

## Precedence and hot reload

- Resolution is per field: a non-empty `LUDEX_OIDC_*` value wins, otherwise the
  saved value is used.
- Saving validates the input, rebuilds the client and runs discovery against the
  provider **before** persisting or swapping anything. Discovery uses a short
  timeout (10 s) and does not follow redirects.
- On success the live client is swapped atomically; `GET /api/config` and the
  login page reflect it immediately.
- On failure the previous configuration stays active and the endpoint returns
  the provider error (`502 oidc_discovery_failed`). Nothing is written to the
  database, so a typo cannot break a working login.

## Validation rules

- Issuer: absolute URL, HTTPS required; plain HTTP is only accepted for
  `localhost`, `127.0.0.1` and `[::1]` (development).
- Client ID: required when SSO is enabled, at most 512 characters.
- Redirect URI: either a single-slash relative path without fragment, or an
  absolute http(s) URL.
- Public base URL: absolute http(s) origin/path without query or fragment.
- Scopes: whitespace-separated; `openid` is always added.
- Groups claim: non-empty (defaults to `groups`).
- Admin groups: trimmed, empty entries removed, case-insensitive duplicates
  collapsed.
- Local password login is never disabled by these settings.

Changing the issuer invalidates every existing `oidc_issuer` / `oidc_subject`
link: those accounts keep their local password access but must sign in again with
the new provider to be re-linked. The admin page warns before you save and shows
the number of linked accounts.

## Authentik example

1. In Authentik, create an **OAuth2/OpenID Provider**:
   - Client type: confidential (or public + PKCE).
   - Redirect URI: `https://ludex.home.cleeryy.cloud/api/auth/oidc/callback`.
   - Signing key: the default certificate.
   - Scopes: `openid`, `profile`, `email` (add `groups` if you want role mapping).
2. Create an **Application** bound to that provider. The application slug is a
   good issuer when using Authentik's per-application issuer
   (`https://auth.example.com/application/o/<slug>/`).
3. In Ludex, open **Admin → Authentication**, choose the Authentik preset, paste
   the issuer and client ID, replace the secret, then **Test discovery** and
   **Save & apply**.
4. To grant admins automatically, list the group in **Admin groups** and ensure
   the provider includes the `groups` claim (or set the matching claim path).

## Keycloak / Authelia notes

- Keycloak: use the realm issuer (`https://keycloak.example.com/realms/<realm>`)
  and the groups claim `realm_access.roles`.
- Authelia: register a confidential client, issuer
  `https://auth.example.com`, groups claim `groups`.
- Any provider that publishes `/.well-known/openid-configuration` and supports
  Authorization Code + PKCE works.

## Behaviour

- Authorization Code + PKCE, state/nonce, discovery and JWKS signature checks.
- Unknown identities are created locally (unless auto-create is off); existing
  accounts are linked by verified email without touching the local password.
- The callback returns a one-time code to the SPA; the JWT is never placed in a
  URL. The code expires after 60 seconds and can be used once.
- The desktop app hides SSO because the OAuth callback returns to the web UI;
  local/remote password login still works in Tauri. Desktop deep-link SSO is
  tracked in issue #50.
- If OIDC is disabled or misconfigured, local login is unaffected.

## API reference

All routes require an administrator session. See `docs/CONTRACT.md` for the
payloads.

| Method | Route | Purpose |
|---|---|---|
| GET | `/api/admin/oidc` | effective settings, `secretConfigured`, `envLockedFields`, `status`, `linkedAccounts` |
| PUT | `/api/admin/oidc` | validate, encrypt the secret, run discovery, persist and hot-swap |
| POST | `/api/admin/oidc/test` | discovery against the submitted values, no persistence |
| POST | `/api/admin/oidc/reset` | delete the saved row, fall back to the environment |

## Security notes

- Keep the client secret out of the repo and out of shell history. Prefer the
  admin UI or a 0600 environment file.
- Use HTTPS for the public base URL; do not expose the callback over plain HTTP
  outside localhost.
- The provider must return a verified email for account linking; otherwise a
  new local account is created.
- Review the admin groups carefully: they grant full server administration.
- Audit entries (`settings.oidc_updated`) contain the issuer, the enabled flag
  and the changed field names only — never the secret or tokens.
