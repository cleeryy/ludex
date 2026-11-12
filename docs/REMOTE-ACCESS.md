# Reaching Ludex from outside your network (#167)

The mobile companion is a normal web client: it needs an HTTPS origin that
reaches the Ludex server. Nothing in this repository opens a tunnel, and no
tunnel is enabled by default. Choose one of the transports below.

Ludex authenticates **every** remote install request. A tunnel is transport
only: it never replaces a session, a device token or the API rate limiter.

## 1. VPN (recommended)

WireGuard, Tailscale or your existing site-to-site VPN is the safest option:
the server stays unreachable from the public internet and there is no TLS
certificate or public hostname to leak.

1. Put the phone and the server on the same VPN.
2. Point the PWA at `https://<vpn-name-or-ip>` (or the LAN address when at
   home).
3. Sign in normally. The session is the only credential the phone needs.

Nothing else changes: remote installs still require a signed-in account.

## 2. Your own HTTPS reverse proxy

Expose only the Ludex HTTP port through a reverse proxy you control (Caddy,
nginx, Traefik), with a real certificate and normal security headers. Keep the
server itself bound to the private interface/VPN.

Checklist:

- TLS terminated at the proxy; the upstream is `127.0.0.1:<port>` or the VPN
  address, never a plain public HTTP hop;
- `X-Forwarded-For` set so the login throttle and API rate limiter see the
  client, not the proxy;
- no "auth off" switch: Ludex still returns `401` for a missing session and
  `403` for a read-only API key;
- keep the server updated, and revoke sessions from **Settings → Account** if a
  phone is lost.

## 3. A public tunnel (OpenTunnel, Cloudflare Tunnel, ngrok, ...)

A tunnel can be convenient, but it changes the threat model.

OpenTunnel is a blind TLS tunnel: the relay forwards encrypted bytes and the
private key stays on your machine. It does **not** protect you:

- the `*.opentunnel.xyz` hostname is public in Certificate Transparency logs;
- anyone who learns the URL reaches the Ludex sign-in page;
- the relay sees traffic metadata (connection times, sizes), not content;
- the tunnel URL is not an authentication factor. Ludex still requires a
  valid session/device token on every request, keeps CSRF/origin checks and
  rate limits, and refuses unauthenticated routes.

If you use it:

1. Finish first-run setup **before** starting the tunnel, and use a strong
   unique password (plus SSO/2FA where available).
2. Treat the URL as a secret; rotate it when you no longer need it.
3. Do not create a public share link "just for the tunnel"; share links are
   separate, expiring capabilities.
4. Stop the tunnel when you are done. It is a temporary door, not a deployment
   mode.
5. Watch the audit log: every remote install request, claim, completion and
   cancellation is recorded.

Ludex does not bundle, resell or auto-start a tunnel. There is no server
configuration flag that turns one on, and no endpoint accepts a tunnel hostname
as proof of identity.

For stricter isolation, combine a tunnel with a VPN or an authenticating proxy
in front of Ludex; then the tunnel only carries already-authenticated traffic.

## Device model (v1 limitation)

The v1 mobile companion uses the **account session** on both the phone and the
desktop. A phone "pairs" by signing into the same account; revoking the session
from the account page stops its remote actions immediately.

A one-time pairing code/QR and per-device tokens are follow-ups (the issue
sketch): they would let the desktop show a list of paired phones and revoke one
without signing the whole account out. Until then:

- use one account per household member, not a shared account;
- the `device` label on a request is cosmetic and never grants access;
- a read-only API key can *follow* the queue but cannot create, claim or
  cancel requests (`403 insufficient_scope`).

## What the desktop must do

The desktop polls `GET /api/remote-installs?status=queued`, claims a row with
`POST /api/remote-installs/{id}/status {"status":"claimed"}`, downloads the
game through the normal download endpoint, reports `downloading` + `progress`,
and finishes with `ready` or `failed`. See `docs/CONTRACT.md` for the exact
payloads.
