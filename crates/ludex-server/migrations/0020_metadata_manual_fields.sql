-- Manual metadata markers (#135).
--
-- `games.manual_fields_json` lists the metadata fields an editor changed by
-- hand through `PATCH /api/games/{id}` (camelCase names, for example
-- `["genres", "description"]`). Applying a provider match preserves those
-- fields unless the request sets `replaceManual: true`, so picking the correct
-- provider record never silently discards a hand-written summary.
--
-- Provider identity (`provider_slug`, `provider_id`, `provider_url`) and the
-- cached artwork always follow the selected record: they are not listed here.

ALTER TABLE games ADD COLUMN manual_fields_json TEXT NOT NULL DEFAULT '[]';
