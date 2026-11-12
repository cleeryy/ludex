-- Device registry and targeted remote installs (#184).
--
-- A desktop registers once with a stable, client-generated id (`client_id`),
-- then heartbeats its presence. The server assigns its own uuid (`id`) so the
-- phone can target an install at a device without ever seeing the client id,
-- which stays a routing key and never grants access.
--
-- `capabilities_json` keeps the raw payload (installed game ids, streaming
-- availability, ...) for future features; the three columns next to it are the
-- values the phone lists today, so a device row never needs JSON parsing on
-- the read path.
--
-- Revocation is soft (`revoked_at`): the phone keeps showing the PC as offline
-- and the device can no longer heartbeat or claim work. No token is stored
-- here; authentication still goes through the normal session.

CREATE TABLE IF NOT EXISTS devices (
    id                  TEXT PRIMARY KEY,
    user_id             BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    client_id           TEXT NOT NULL,
    name                TEXT NOT NULL,
    platform            TEXT,
    app_version         TEXT,
    last_seen_at        TEXT NOT NULL,
    capabilities_json   TEXT NOT NULL DEFAULT '{}',
    installed_count     INTEGER NOT NULL DEFAULT 0,
    free_disk_bytes     BIGINT,
    streaming_available INTEGER NOT NULL DEFAULT 0,
    created_at          TEXT NOT NULL,
    revoked_at          TEXT,
    UNIQUE (user_id, client_id)
);

CREATE INDEX IF NOT EXISTS idx_devices_user
    ON devices(user_id, last_seen_at DESC);

-- `target_device_id` is null for the account-wide queue of #167: any signed-in
-- desktop may claim those. `device_id` attributes a claim so the
-- phone can show which PC is installing and a second PC cannot take over.
ALTER TABLE remote_installs ADD COLUMN target_device_id TEXT REFERENCES devices(id) ON DELETE SET NULL;
ALTER TABLE remote_installs ADD COLUMN device_id TEXT REFERENCES devices(id) ON DELETE SET NULL;
