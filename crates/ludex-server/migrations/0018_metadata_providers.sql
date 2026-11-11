-- Metadata provider plugins (#28).
--
-- A provider is an HTTP sidecar with a documented JSON contract: the server
-- calls `GET {base_url}/search?title=...&year=...` instead of executing any
-- third-party code. IGDB stays the built-in default and is always tried
-- first; enabled providers run afterwards in ascending priority order.
--
-- `token_enc` is the write-only bearer token, encrypted with the same
-- AES-256-GCM/HKDF scheme as the OIDC client secret. It is never returned by
-- the API. `mapping_json` maps canonical metadata fields to dot-paths in a
-- non-canonical response, e.g.
--   {"title":"data.name","coverUrl":"data.images.cover"}.
--
-- `provider_fetches` is an observability log capped by retention (see
-- `crate::providers::FETCH_RETENTION_DAYS`): rows older than the window are
-- pruned on write. It is not part of the audit trail, so it never contains
-- secrets; `error` is a short, human-readable message.

CREATE TABLE IF NOT EXISTS metadata_providers (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    name         TEXT NOT NULL UNIQUE,
    base_url     TEXT NOT NULL,
    token_enc    TEXT,
    priority     INTEGER NOT NULL DEFAULT 100,
    enabled      INTEGER NOT NULL DEFAULT 0,
    mapping_json TEXT NOT NULL DEFAULT '{}',
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_metadata_providers_enabled
    ON metadata_providers(enabled, priority, id);

CREATE TABLE IF NOT EXISTS provider_fetches (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    provider_id INTEGER NOT NULL REFERENCES metadata_providers(id) ON DELETE CASCADE,
    game_id     INTEGER,
    ok          INTEGER NOT NULL,
    status_code INTEGER,
    error       TEXT,
    fetched_at  TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_provider_fetches_provider
    ON provider_fetches(provider_id, fetched_at DESC);
CREATE INDEX IF NOT EXISTS idx_provider_fetches_game
    ON provider_fetches(game_id, fetched_at DESC);
CREATE INDEX IF NOT EXISTS idx_provider_fetches_fetched
    ON provider_fetches(fetched_at);
