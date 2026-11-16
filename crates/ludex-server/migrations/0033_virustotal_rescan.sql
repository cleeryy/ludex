-- VirusTotal periodic re-check (#239), SQLite edition.
--
-- One row (id = 1) holds the administrator's policy for re-checking every
-- archive that already has a published SHA-256. The default is off: nothing is
-- looked up until an administrator enables it.
--
-- `interval_hours` is one of 0 (manual only), 24, 168 or 720. `next_run_at` is
-- persisted so a restart keeps the schedule instead of resetting it.
-- `cursor` is the last processed SHA-256, kept as a progress marker for the
-- pass that is currently draining. `last_error` distinguishes an invalid key,
-- an exhausted quota and an outage with its category prefix, and never holds
-- the API key. `scanned_last_run` is the number of hashes the last pass
-- actually refreshed, for the admin card.
--
-- The worker never uploads: it reuses `virustotal_cache`, `virustotal_budget`
-- and `virustotal_links`, and therefore the same 4/minute and 500/day limits
-- as a manual scan.

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
