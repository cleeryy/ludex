# Integrations: API keys, audit log and webhooks

Everything below uses the normal JSON API under `/api`. Authenticate either
with a session JWT (`Authorization: Bearer <jwt>`) or an API key
(`Authorization: Bearer ldx_<token>`). API keys never grant more than their
scopes: `read`, `write` or `admin` (each implies the previous one).

## API keys

Create a read-only key for a script:

```bash
curl -sS -X POST https://ludex.example.com/api/keys \
  -H "Authorization: Bearer $LUDEX_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"name":"backup-job","scopes":["read"]}'
```

```json
{
  "id": 3,
  "userId": 1,
  "name": "backup-job",
  "prefix": "ldx_9f3a1c2b",
  "scopes": ["read"],
  "createdAt": "2026-11-10T12:00:00Z",
  "lastUsedAt": null,
  "revokedAt": null,
  "secret": "ldx_9f3a1c2b...64-hex..."
}
```

The `secret` is shown once. Store it in the client's secret store:

```bash
export LUDEX_API_KEY='ldx_9f3a1c2b...'
curl -sS https://ludex.example.com/api/games?limit=5 \
  -H "Authorization: Bearer $LUDEX_API_KEY"
```

Mutating endpoints require the `write` scope (for example starting a scan):

```bash
curl -sS -X POST https://ludex.example.com/api/library/scan \
  -H "Authorization: Bearer $LUDEX_API_KEY"
```

Administrator routes (users, audit, webhooks) require the `admin` scope **and**
an administrator account. List and revoke:

```bash
curl -sS https://ludex.example.com/api/keys -H "Authorization: Bearer $LUDEX_TOKEN"
curl -sS -X DELETE https://ludex.example.com/api/keys/3 \
  -H "Authorization: Bearer $LUDEX_TOKEN"
```

Revocation is immediate: the next request with the key gets `401 unauthorized`.
Every successful use updates `lastUsedAt`.

## Audit log

Administrators can page through the audit trail. Newest entries come first.

```bash
# Last 50 events
curl -sS 'https://ludex.example.com/api/audit' \
  -H "Authorization: Bearer $LUDEX_TOKEN"

# Failed logins, 20 per page
curl -sS 'https://ludex.example.com/api/audit?action=login.failed&limit=20' \
  -H "Authorization: Bearer $LUDEX_TOKEN"

# Next page: pass the previous response's nextBefore
curl -sS 'https://ludex.example.com/api/audit?limit=20&before=912' \
  -H "Authorization: Bearer $LUDEX_TOKEN"
```

```json
{
  "items": [
    {
      "id": 913,
      "actorId": 1,
      "actorKind": "user",
      "action": "api_key.created",
      "targetType": "api_key",
      "targetId": "3",
      "metadata": {"name": "backup-job", "prefix": "ldx_9f3a1c2b", "scopes": ["read"], "userId": 1},
      "ip": "192.0.2.10",
      "createdAt": "2026-11-10T12:00:00Z"
    }
  ],
  "nextBefore": 912
}
```

Filters: `actorId`, `actorKind` (`user`, `api_key`, `system`), `action`,
`targetType`, `targetId`. Secrets, passwords and plaintext API keys are never
written to the audit trail.

## Webhooks

Register an endpoint (admin only). `events` accepts `game.ingested`,
`game.deleted`, `download.completed`, `user.created` and `save.uploaded`.

```bash
curl -sS -X POST https://ludex.example.com/api/webhooks \
  -H "Authorization: Bearer $LUDEX_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"url":"https://hooks.example.com/ludex","events":["game.ingested","user.created"]}'
```

```json
{
  "id": 1,
  "url": "https://hooks.example.com/ludex",
  "events": ["game.ingested", "user.created"],
  "enabled": true,
  "createdAt": "2026-11-10T12:00:00Z",
  "secret": "64-hex-hmac-key"
}
```

`GET /api/webhooks` lists hooks without the secret. `PATCH` updates `url`,
`events` or `enabled`; send `{"rotateSecret": true}` to get a new secret once.
`DELETE /api/webhooks/1` removes the hook and its deliveries. Queue a test
delivery with:

```bash
curl -sS -X POST https://ludex.example.com/api/webhooks/1/test \
  -H "Authorization: Bearer $LUDEX_TOKEN"
# 202 {"deliveryId": 7}
```

### Verifying deliveries

Each request carries the exact body, `X-Ludex-Event`, `X-Ludex-Delivery` and
`X-Ludex-Signature: sha256=<hex>`, where the hex is HMAC-SHA256 of the raw
request body using the webhook secret. Verify before parsing:

```python
import hashlib, hmac

def verify(secret: str, body: bytes, header: str) -> bool:
    expected = "sha256=" + hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, header)

# Flask example
# body = request.get_data()
# assert verify(WEBHOOK_SECRET, body, request.headers["X-Ludex-Signature"])
```

```bash
# Inspect a delivery by hand
SECRET='64-hex-hmac-key'
BODY='{"event":"game.ingested","timestamp":"2026-11-10T12:00:00Z","data":{"title":"Celeste"}}'
printf '%s' "$BODY" | openssl dgst -sha256 -hmac "$SECRET"
```

Payload shape:

```json
{
  "event": "save.uploaded",
  "timestamp": "2026-11-10T12:00:00Z",
  "data": {
    "snapshotId": 12,
    "gameId": 4,
    "userId": 2,
    "size": 1048576,
    "sha256": "...",
    "label": "before boss"
  }
}
```

### Delivery semantics

- Asynchronous, at-least-once, ordered per webhook by delivery id.
- 10 s timeout, no redirects, `Content-Type: application/json`.
- 5 attempts with exponential backoff (1 s, 2 s, 4 s, 8 s by default; override
  the base with `LUDEX_WEBHOOKS_BACKOFF_MS` for tests). A 2xx marks the delivery
  as done; anything else is retried.
- The stored payload is re-sent byte-for-byte, so the signature stays valid.
- Loopback, private, link-local and unique-local addresses are rejected unless
  `LUDEX_WEBHOOKS_ALLOW_PRIVATE=true`. The host is re-resolved and the
  connection pinned before every attempt to limit DNS-rebinding SSRF.
- A disabled hook receives no deliveries; its queued deliveries resume when it
  is re-enabled.
