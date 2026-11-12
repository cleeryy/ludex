-- Opt-in anonymous diagnostics reports (#171).
--
-- Rows are written by `POST /api/diagnostics`. Reports are explicitly sent by
-- a user/device that turned the feature on; nothing is uploaded automatically
-- without that consent. The table stores no account id and no IP address, only
-- a random install id generated locally on the device. Admin deletion is the
-- only mutation besides the retention sweep.
--
-- `detail_json` is a bounded, already-redacted JSON object. Timestamps are
-- RFC 3339 UTC strings, like every other table.

CREATE TABLE IF NOT EXISTS diagnostics_reports (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    install_id  TEXT NOT NULL,
    app_version TEXT NOT NULL,
    platform    TEXT NOT NULL,
    os          TEXT NOT NULL,
    locale      TEXT NOT NULL DEFAULT '',
    channel     TEXT NOT NULL DEFAULT '',
    error_kind  TEXT NOT NULL,
    summary     TEXT NOT NULL,
    detail_json TEXT NOT NULL DEFAULT '{}',
    created_at  TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_diagnostics_created_at ON diagnostics_reports(created_at);
CREATE INDEX IF NOT EXISTS idx_diagnostics_install_id ON diagnostics_reports(install_id);
CREATE INDEX IF NOT EXISTS idx_diagnostics_error_kind ON diagnostics_reports(error_kind);
