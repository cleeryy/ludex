-- Parental controls (#39), PostgreSQL edition.

ALTER TABLE users ADD COLUMN age_limit BIGINT;

CREATE INDEX IF NOT EXISTS idx_users_age_limit ON users(age_limit);
