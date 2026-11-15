-- VirusTotal integration (#224), PostgreSQL edition.
--
-- Same tables and columns as `0032_virustotal.sql`. Attached ids and byte
-- counters are BIGINT because the Rust row decoders read i64 on both engines.
-- `virustotal_links.id` uses a sequence so inserts can omit it portably.

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
    id         BIGSERIAL PRIMARY KEY,
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
