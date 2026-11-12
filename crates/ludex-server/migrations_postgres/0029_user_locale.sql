-- Account interface language (#188), PostgreSQL edition.
--
-- Same column and semantics as `0029_user_locale.sql`: NULL or empty means
-- "inherit". Existing rows stay NULL, so no account is pinned to a language by
-- the migration.

ALTER TABLE users ADD COLUMN locale TEXT;
