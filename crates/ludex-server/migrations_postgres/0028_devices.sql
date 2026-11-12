-- Device registry and targeted remote installs (#184), PostgreSQL edition.
--
-- Same tables and columns as `0028_devices.sql`. Counters are BIGINT because
-- the Rust row decoders read i64 on both engines.

CREATE TABLE IF NOT EXISTS devices (
    id                  TEXT PRIMARY KEY,
    user_id             BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    client_id           TEXT NOT NULL,
    name                TEXT NOT NULL,
    platform            TEXT,
    app_version         TEXT,
    last_seen_at        TEXT NOT NULL,
    capabilities_json   TEXT NOT NULL DEFAULT '{}',
    installed_count     BIGINT NOT NULL DEFAULT 0,
    free_disk_bytes     BIGINT,
    streaming_available INTEGER NOT NULL DEFAULT 0,
    created_at          TEXT NOT NULL,
    revoked_at          TEXT,
    UNIQUE (user_id, client_id)
);

CREATE INDEX IF NOT EXISTS idx_devices_user
    ON devices(user_id, last_seen_at DESC);

ALTER TABLE remote_installs ADD COLUMN target_device_id TEXT REFERENCES devices(id) ON DELETE SET NULL;
ALTER TABLE remote_installs ADD COLUMN device_id TEXT REFERENCES devices(id) ON DELETE SET NULL;
