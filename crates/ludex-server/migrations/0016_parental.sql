-- Parental controls (#39).
--
-- `age_limit` is the maximum game age rating (PEGI/ESRB mapped to a number)
-- that this account may see, open or download. NULL means no restriction.
-- The limit is enforced server-side in list, detail and download paths; it is
-- a local authorisation control like the role, not an identity-provider field.

ALTER TABLE users ADD COLUMN age_limit INTEGER;

CREATE INDEX IF NOT EXISTS idx_users_age_limit ON users(age_limit);
