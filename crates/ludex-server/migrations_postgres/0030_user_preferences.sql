-- Account preference document sync (#201), PostgreSQL edition.
--
-- Same table and semantics as `0030_user_preferences.sql`: one JSON document
-- per account, an allowlisted key set validated by the API, `revision` for
-- `If-Match` optimistic concurrency and `schema_version` for in-place
-- migrations. Deleting the row is the reset.

CREATE TABLE IF NOT EXISTS user_preferences (
    user_id        BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    document_json  TEXT NOT NULL,
    schema_version BIGINT NOT NULL DEFAULT 1,
    revision       BIGINT NOT NULL DEFAULT 1,
    updated_at     TEXT NOT NULL
);
