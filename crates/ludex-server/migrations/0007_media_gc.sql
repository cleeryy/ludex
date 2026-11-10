-- Media cache garbage collection (#18).
--
-- `media_manifest` is the integrity baseline for cached artwork: the first
-- successful hash of a referenced file is stored here, later runs compare
-- against it. Rows are removed when the garbage collector deletes a file.
--
-- `media_gc_runs` keeps the last runs for observability; the API returns the
-- most recent one with the live counters.

CREATE TABLE IF NOT EXISTS media_manifest (
    name        TEXT PRIMARY KEY,
    size        INTEGER NOT NULL DEFAULT 0,
    sha256      TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    verified_at TEXT
);

CREATE TABLE IF NOT EXISTS media_gc_runs (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    dry_run       INTEGER NOT NULL DEFAULT 1,
    scanned       INTEGER NOT NULL DEFAULT 0,
    referenced    INTEGER NOT NULL DEFAULT 0,
    orphan_files  INTEGER NOT NULL DEFAULT 0,
    orphan_bytes  INTEGER NOT NULL DEFAULT 0,
    deleted_files INTEGER NOT NULL DEFAULT 0,
    deleted_bytes INTEGER NOT NULL DEFAULT 0,
    started_at    TEXT NOT NULL,
    finished_at   TEXT
);

CREATE INDEX IF NOT EXISTS idx_media_gc_runs_started
    ON media_gc_runs(started_at DESC, id DESC);
