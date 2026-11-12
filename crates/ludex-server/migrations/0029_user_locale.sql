-- Account interface language (#188).
--
-- NULL or an empty string means "inherit": the account follows the server
-- default, then the browser languages, then English. Values are canonical
-- locale codes from `apps/web/src/locales/manifest.json` and are validated by
-- the API against the same coverage gate the picker uses, so a hidden locale
-- can never be stored. Column-level backfill is implicit: every existing row
-- becomes NULL and keeps following the server/browser.

ALTER TABLE users ADD COLUMN locale TEXT;
