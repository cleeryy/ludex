# Example metadata provider

A dependency-free Node server implementing the Ludex metadata provider
contract (#28). It is the reference for third-party providers: a small HTTP
sidecar that returns JSON, never code that runs inside the Ludex server.

## Run

```bash
node examples/metadata-provider/server.mjs --port 8787
# optional: --host 0.0.0.0 --token my-secret --fixture ./fixtures/games.json
```

Then in **Admin → Metadata providers → Add provider**:

| Field | Value |
|---|---|
| Name | `Example fixture` |
| Base URL | `http://localhost:8787` |
| Token | empty, or the `--token` value |
| Priority | `100` |
| Enabled | on |
| Field mapping | `{}` |

A loopback base URL is refused unless the server runs with
`LUDEX_PROVIDER_ALLOW_PRIVATE=true`; that switch exists for local tests and must
stay off in production. In production, put the sidecar behind HTTPS.

Use **Test** in the admin UI (or `POST
/api/admin/metadata-providers/{id}/test` with `{"title": "Celeste"}`) before
enabling the provider. Enrichment runs after IGDB and before the Steam and
file-name fallbacks; it only fills fields that are still empty.

## Endpoints

### `GET /search?title=<text>&year=<optional>`

Returns the candidates, best match first:

```json
{
  "results": [
    {
      "id": "celeste-1",
      "title": "Celeste",
      "description": "Help Madeline survive her inner demons…",
      "releaseDate": "2018-01-25",
      "coverUrl": "https://cdn.example.invalid/celeste/cover.jpg",
      "backgroundUrl": "https://cdn.example.invalid/celeste/hero.jpg",
      "screenshots": ["https://cdn.example.invalid/celeste/1.jpg"],
      "genres": ["Platformer", "Indie"],
      "developers": ["Maddy Makes Games"],
      "publishers": ["Extremely OK Games"],
      "tags": ["precision-platformer"],
      "rating": 92,
      "ageRating": 7,
      "providerUrl": "https://metadata.example.invalid/celeste"
    }
  ]
}
```

`id` and `title` are required. `releaseDate` accepts `YYYY-MM-DD`, RFC 3339 or a
bare year; `rating` is 0-100; `ageRating` is a minimum age (0-21). Unknown fields
are ignored. Descriptions may contain HTML, which the server strips.

### `GET /games/{id}`

Returns one candidate object (or a `{"results": [ ... ]}` envelope). Used by
future metadata refreshes.

### `GET /health`

`{"status":"ok","games":3}`. Not part of the contract; handy for monitoring.

## Field mapping

The fixture already uses the canonical shape, so `{}` is correct. If your
provider nests its data, map canonical fields to dot paths instead of reshaping
the response:

```json
{
  "title": "data.name",
  "coverUrl": "data.images.cover",
  "releaseDate": "data.released_at",
  "genres": "data.genres"
}
```

Allowed canonical fields: `id`, `title`, `description`, `releaseDate`,
`coverUrl`, `backgroundUrl`, `screenshots`, `genres`, `developers`,
`publishers`, `tags`, `rating`, `ageRating`, `providerUrl`. Paths are
dot-separated object keys; missing paths are ignored and the canonical key is
tried instead.

## Security expectations

The Ludex server:

- fetches providers server-side only (the webview never calls them);
- uses a 10 s timeout, at most 2 MiB of response body, and disables redirects;
- refuses loopback/private targets unless explicitly allowed for tests;
- pins the connection to the resolved, validated address;
- sends `Authorization: Bearer <token>` only when a token is configured;
- encrypts the token at rest and never returns it from the API.

Providers should therefore serve HTTPS in production, answer quickly, keep
responses small, and treat the bearer token as a write-only secret.

## Extending

The fixture lives in `fixtures/games.json`; add entries there or pass
`--fixture /path/to/games.json`. The search is a case-insensitive substring
match with a release-year bonus, which is enough to exercise the admin test
dialog and the enrichment chain.
