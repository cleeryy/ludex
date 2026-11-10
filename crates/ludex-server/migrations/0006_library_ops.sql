-- Multiple library roots (#45), folder watcher (#44) and integrity checks (#43).
--
-- Root #1 is seeded as a placeholder (`path = ''`) so an upgraded database
-- always has an ordered root list. `roots::bootstrap` replaces the empty path
-- with `LUDEX_LIBRARY_ROOTS` (or `LUDEX_GAMES_DIR`) on the next start and
-- assigns legacy `game_versions` rows to that root.
--
-- `game_versions` is rebuilt to scope file paths per root: two roots may hold
-- a file with the same relative path. Row ids are preserved so
-- `save_snapshots.version_id` keeps pointing at the same version; the
-- references are backed up and restored around the rebuild because
-- `DROP TABLE` fires the `ON DELETE SET NULL` action.

CREATE TABLE IF NOT EXISTS library_roots (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    label        TEXT NOT NULL,
    path         TEXT NOT NULL,
    enabled      INTEGER NOT NULL DEFAULT 1,
    available    INTEGER NOT NULL DEFAULT 1,
    position     INTEGER NOT NULL DEFAULT 0,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    last_scan_at TEXT,
    last_error   TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_library_roots_path ON library_roots(path);
CREATE INDEX IF NOT EXISTS idx_library_roots_position ON library_roots(position, id);

INSERT INTO library_roots (label, path, enabled, available, position, created_at, updated_at)
SELECT 'Library', '', 1, 1, 0,
       strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
       strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
WHERE NOT EXISTS (SELECT 1 FROM library_roots);

ALTER TABLE games ADD COLUMN root_id INTEGER REFERENCES library_roots(id) ON DELETE SET NULL;

CREATE TEMP TABLE save_version_backup AS
    SELECT id, version_id FROM save_snapshots WHERE version_id IS NOT NULL;

CREATE TABLE game_versions_new (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    game_id         INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    root_id         INTEGER REFERENCES library_roots(id) ON DELETE SET NULL,
    file_path       TEXT NOT NULL,
    file_name       TEXT NOT NULL,
    size            INTEGER NOT NULL DEFAULT 0,
    version         TEXT,
    release_date    TEXT,
    early_access    INTEGER NOT NULL DEFAULT 0,
    missing         INTEGER NOT NULL DEFAULT 0,
    root_unavailable INTEGER NOT NULL DEFAULT 0,
    content_hash    TEXT,
    indexed_at      TEXT NOT NULL,
    UNIQUE (root_id, file_path)
);

INSERT INTO game_versions_new
    (id, game_id, file_path, file_name, size, version, release_date,
     early_access, missing, indexed_at)
SELECT id, game_id, file_path, file_name, size, version, release_date,
       early_access, missing, indexed_at
FROM game_versions;

DROP TABLE game_versions;
ALTER TABLE game_versions_new RENAME TO game_versions;

CREATE INDEX IF NOT EXISTS idx_game_versions_game_id ON game_versions(game_id);
CREATE INDEX IF NOT EXISTS idx_game_versions_missing ON game_versions(missing);
CREATE INDEX IF NOT EXISTS idx_game_versions_root ON game_versions(root_id, missing);

UPDATE save_snapshots SET version_id = (
    SELECT version_id FROM save_version_backup
    WHERE save_version_backup.id = save_snapshots.id
) WHERE id IN (SELECT id FROM save_version_backup);

DROP TABLE save_version_backup;

CREATE TABLE IF NOT EXISTS integrity_reports (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    scope          TEXT NOT NULL DEFAULT 'all',
    status         TEXT NOT NULL DEFAULT 'running',
    checked        INTEGER NOT NULL DEFAULT 0,
    findings_count INTEGER NOT NULL DEFAULT 0,
    error          TEXT,
    started_at     TEXT NOT NULL,
    finished_at    TEXT
);

CREATE INDEX IF NOT EXISTS idx_integrity_reports_started
    ON integrity_reports(started_at DESC, id DESC);

CREATE TABLE IF NOT EXISTS integrity_findings (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    report_id   INTEGER NOT NULL REFERENCES integrity_reports(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL,
    target_type TEXT NOT NULL,
    target_id   TEXT,
    path        TEXT,
    detail      TEXT,
    created_at  TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_integrity_findings_report
    ON integrity_findings(report_id, id);
