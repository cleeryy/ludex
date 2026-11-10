# SSO with OIDC (Authentik, Keycloak, Authelia)

Ludex can use an external OpenID Connect provider for web login. Local accounts
remain available, and the first-run setup still creates a local administrator.

## Server configuration

Add these variables to the server environment (`/srv/ludex/ludex.env` on the
CT258 deployment, or the Docker Compose environment):

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

The redirect URI registered at the provider must be exactly:

```
{LUDEX_OIDC_PUBLIC_BASE_URL}/api/auth/oidc/callback
```

For the CT258 test deployment:

```
https://ludex.home.cleeryy.cloud/api/auth/oidc/callback
```

Restart the server after changing the environment:

```bash
sudo systemctl restart ludex.service
curl -fsS http://127.0.0.1:8081/api/config | python3 -m json.tool
```

`features.oidc` must be `true` and `oidcDisplayName` should show the provider
label. The login screen then shows **Sign in with <name>**.

## Authentik example

1. In Authentik, create an **OAuth2/OpenID Provider**:
   - Client type: confidential (or public + PKCE).
   - Redirect URI: `https://ludex.home.cleeryy.cloud/api/auth/oidc/callback`.
   - Signing key: the default certificate.
   - Scopes: `openid`, `profile`, `email` (add `groups` if you want role mapping).
2. Create an **Application** bound to that provider. The application slug is a
   good value for `LUDEX_OIDC_ISSUER` when using Authentik's per-application
   issuer (`https://auth.example.com/application/o/<slug>/`).
3. Copy the client ID and secret into `LUDEX_OIDC_CLIENT_ID` /
   `LUDEX_OIDC_CLIENT_SECRET`.
4. To grant admins automatically, put the group name in
   `LUDEX_OIDC_ADMIN_GROUPS` and ensure the provider includes the `groups` claim
   (`LUDEX_OIDC_GROUPS_CLAIM=groups`).

## Behaviour

- Authorization Code + PKCE, state/nonce, discovery and JWKS signature checks.
- Unknown identities are created locally (unless `AUTO_CREATE=false`); existing
  accounts are linked by verified email without touching the local password.
- The callback returns a one-time code to the SPA; the JWT is never placed in a
  URL. The code expires after 60 seconds and can be used once.
- The desktop app hides SSO for now because the OAuth callback returns to the
  web UI; local/remote password login still works in Tauri. Desktop deep-link
  SSO is tracked in issue #50.
- If OIDC is disabled or misconfigured, local login is unaffected.

## Security notes

- Keep the client secret in the environment file (0600), never in the repo.
- Use HTTPS for the public base URL; do not expose the callback over plain HTTP
  outside localhost.
- The provider must return a verified email for account linking; otherwise a
  new local account is created.
- Review `LUDEX_OIDC_ADMIN_GROUPS` carefully: it grants full server administration.
