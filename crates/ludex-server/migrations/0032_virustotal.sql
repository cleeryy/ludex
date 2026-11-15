-- VirusTotal integration (#224), SQLite edition.
--
-- Hash lookups only by default: Ludex sends one `GET /files/{sha256}` per
-- unique archive hash and never uploads a file unless an administrator
-- explicitly confirms one. The API key is write-only and encrypted at rest
-- with the same AES-256-GCM helper as the OIDC and provider secrets, so the
-- database never stores plaintext and no endpoint ever returns it.
--
-- `virustotal_integration` holds at most one row (id = 1). The environment
-- variable `LUDEX_VIRUSTOTAL_API_KEY` wins over this row and the admin UI
-- refuses to overwrite or delete it.
--
-- `virustotal_cache` is keyed by the full archive SHA-256 (or an extracted
-- binary hash). `stats_json` and `engine_hits_json` keep the raw-ish API
-- payload so a later threshold change does not need a re-scan. `ttl_seconds`
-- is stored per row: 7 days for clean/not_found, 1 day for suspicious and an
-- effectively indefinite value for malicious.
--
-- `virustotal_budget` persists the daily request counter so a restart cannot
-- reset the 500/day public quota. `virustotal_links` records which game and
-- version a hash was checked for; one hash scanned through two versions is one
-- API call and two associations.

CREATE TABLE IF NOT EXISTS virustotal_integration (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    api_key_enc  TEXT NOT NULL,
    fingerprint  TEXT NOT NULL,
    added_at     TEXT NOT NULL,
    last_used_at TEXT,
    last_error   TEXT
);

CREATE TABLE IF NOT EXISTS virustotal_cache (
    sha256             TEXT PRIMARY KEY,
    size_bytes         BIGINT,
    verdict            TEXT NOT NULL,
    stats_json         TEXT NOT NULL DEFAULT '{}',
    engine_hits_json   TEXT NOT NULL DEFAULT '[]',
    last_analysis_date TEXT,
    checked_at         TEXT NOT NULL,
    ttl_seconds        BIGINT NOT NULL,
    permalink          TEXT
);

CREATE TABLE IF NOT EXISTS virustotal_budget (
    day  TEXT PRIMARY KEY,
    used BIGINT NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS virustotal_links (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    game_id    BIGINT NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    version_id BIGINT REFERENCES game_versions(id) ON DELETE SET NULL,
    sha256     TEXT NOT NULL,
    verdict    TEXT NOT NULL,
    mode       TEXT NOT NULL,
    source     TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_virustotal_links_game
    ON virustotal_links (game_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_virustotal_links_version
    ON virustotal_links (version_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_virustotal_links_sha
    ON virustotal_links (sha256);
