-- VirusTotal periodic re-check (#239), PostgreSQL edition.
--
-- Same table and columns as `0033_virustotal_rescan.sql`. The booleans are
-- INTEGER on purpose: the Rust row decoder reads i64 on both engines, and a
-- SQLite/PostgreSQL document must decode identically.

CREATE TABLE IF NOT EXISTS virustotal_rescan (
    id                INTEGER PRIMARY KEY CHECK (id = 1),
    enabled           INTEGER NOT NULL DEFAULT 0,
    interval_hours    INTEGER NOT NULL DEFAULT 168,
    include_malicious INTEGER NOT NULL DEFAULT 0,
    last_run_at       TEXT,
    next_run_at       TEXT,
    cursor            TEXT,
    last_error        TEXT,
    scanned_last_run  BIGINT NOT NULL DEFAULT 0,
    updated_at        TEXT NOT NULL
);
