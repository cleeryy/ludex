-- Manual metadata markers (#135), PostgreSQL edition.
--
-- Same column as `0020_metadata_manual_fields.sql`: camelCase metadata field
-- names an editor changed by hand. Applying a provider match preserves them
-- unless the request sets `replaceManual: true`.

ALTER TABLE games ADD COLUMN manual_fields_json TEXT NOT NULL DEFAULT '[]';
