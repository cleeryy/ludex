# OpenAPI

Ludex serves a generated OpenAPI 3.1 document at:

```
GET /api/openapi.json
```

The endpoint is public (no auth) and always enabled: it lets clients discover
the API without credentials and is safe to expose because it contains no
secrets. `GET /api/docs` renders a minimal, dependency-free explorer (it loads
`openapi.json` from the same origin) when `LUDEX_OPENAPI_DOCS=true`; otherwise
that route returns 404 `docs_disabled`.

## Generated, not written

The document is built at request time by `crates/ludex-server/src/openapi.rs`:

- `openapi::ROUTES` is the single source of truth for documented operations
  (method, path, tag, summary, auth level, request/response schema names, status
  and query parameters).
- Response and request schemas come from `#[derive(utoipa::ToSchema)]` on the
  DTOs (`ludex-core::models`, `ludex-server::models` and the feature modules
  such as `share` and `achievements`), so field names and types follow the Rust
  structs, not a hand-maintained copy.
- The `bearerAuth` security scheme documents both session JWTs and `ldx_` API
  keys. Operations that require a role also document 403; everything
  authenticated documents 401; login documents 429.
- Paths are relative to the `/api` server entry (`servers: [{ "url": "/api" }]`).
  Ingest paths are registered with an absolute `/api` prefix by their router and
  normalised to the same base in the document.

## Contract test

`crates/ludex-server/tests/openapi.rs` runs in CI with the rest of the tests:

1. **Router ↔ document equality.** It parses every `.route(...)` registration in
   `src/routes/*.rs` and `src/ingest/mod.rs`, extracts the HTTP methods from the
   handler expression, normalises absolute ingest paths and compares the set
   with `openapi::documented_operations()`. Adding a route without documenting
   it (or documenting a route that no longer exists) fails the test with the
   exact `(method, path)` diff.
2. **Schema integrity.** Every `$ref` in the document must resolve to a
   registered component; the `bearerAuth` scheme must exist.
3. **Live mounting.** It boots a real server and sends `PATCH` to every
   documented path. A registered path answers 405/401/400; an unmounted path
   answers 404 with `"Endpoint not found"`, which fails the test.

The client SDK types in `apps/web/src/lib/types.ts` mirror the same schemas;
`docs/CONTRACT.md` remains the human-readable contract and is updated together
with the DTOs.

## Working on the schema

- Add or change a DTO: derive/keep `utoipa::ToSchema` and (if referenced by a
  route) add it to the `ComponentsBuilder` list in `openapi.rs`.
- Add a route: add a `RouteDoc` entry to `openapi::ROUTES`. The contract test
  fails until you do.
- Body types too large or deeply nested for a precise schema can use
  `Payload::Object`; prefer a real schema whenever the type is public.
- Bump the API version via the crate version (`env!("CARGO_PKG_VERSION")` is
  embedded in `info.version`).
