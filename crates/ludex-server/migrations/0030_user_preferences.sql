-- Account preference document sync (#201).
--
-- One row per account that has synced at least once. The document is a JSON
-- object with an allowlisted key set (theme, Big Picture, library view, update
-- prompt) validated by `routes::preferences` before it is stored: no paths,
-- tokens, server URLs or free-form blobs can reach this column. `revision`
-- is the optimistic-concurrency token behind `If-Match`/409 handling;
-- `schema_version` lets a later client migrate the document in place.
--
-- Deleting the row is the reset: clients fall back to their defaults and the
-- next write recreates it with revision 1.

CREATE TABLE IF NOT EXISTS user_preferences (
    user_id        INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    document_json  TEXT NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1,
    revision       INTEGER NOT NULL DEFAULT 1,
    updated_at     TEXT NOT NULL
);
